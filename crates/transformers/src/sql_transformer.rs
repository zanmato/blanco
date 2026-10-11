use crate::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::ColumnType;
use blanco_core::DatabaseType;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) fn should_quote_value(column_type: &ColumnType) -> bool {
    matches!(
        column_type,
        ColumnType::Text
            | ColumnType::DateTime
            | ColumnType::Uuid
            | ColumnType::Json
            | ColumnType::Array
            | ColumnType::Binary
            | ColumnType::Unknown
    )
}

pub struct SqlTransformer {
    table_name: Option<String>,
    db_type: DatabaseType,
    /// One INSERT holding every row, rather than one INSERT per row.
    multi_row: bool,
    first_row: AtomicBool,
    /// `INSERT INTO t (columns) VALUES` while streaming, set up once the
    /// columns are known.
    statement_prefix: Mutex<String>,
}

impl SqlTransformer {
    pub fn new() -> Self {
        Self {
            table_name: None,
            db_type: DatabaseType::PostgreSQL,
            multi_row: true,
            first_row: AtomicBool::new(true),
            statement_prefix: Mutex::new(String::new()),
        }
    }

    pub fn with_table_name(table_name: String, db_type: DatabaseType) -> Self {
        Self {
            table_name: Some(table_name),
            db_type,
            ..Self::new()
        }
    }

    /// Write one INSERT with many rows (the default), or one INSERT per row.
    pub fn multi_row(mut self, multi_row: bool) -> Self {
        self.multi_row = multi_row;
        self
    }

    fn write_values<'a>(
        db_type: DatabaseType,
        values: impl Iterator<Item = (Option<&'a str>, &'a ColumnType)>,
        output: &mut String,
    ) {
        output.push('(');
        for (i, (value, column_type)) in values.enumerate() {
            if i > 0 {
                output.push_str(", ");
            }
            match value {
                None => output.push_str("NULL"),
                Some("") => output.push_str("''"),
                Some(value) if should_quote_value(column_type) => {
                    output.push('\'');
                    sql_escape_string_to(value, db_type, output);
                    output.push('\'');
                }
                Some(value) => output.push_str(value),
            }
        }
        output.push(')');
    }
}

fn statement_prefix(table_name: &str, columns: &[String], db_type: DatabaseType) -> String {
    let column_list = columns
        .iter()
        .map(|col| sql_identifier(col, db_type))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({}) VALUES",
        sql_identifier(table_name, db_type),
        column_list
    )
}

impl DataTransformer for SqlTransformer {
    fn format_name(&self) -> &'static str {
        "SQL"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        let db_type = data.db_type.unwrap_or(self.db_type);

        // Use provided table name or a default generic name
        let table_name = data
            .table_name
            .as_ref()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        let prefix = statement_prefix(&table_name, &data.columns, db_type);
        let mut output = String::with_capacity(data.selected_rows.len() * 150 + 200);
        let unknown = ColumnType::Unknown;
        for (i, row) in data.selected_rows.iter().enumerate() {
            if self.multi_row {
                output.push_str(if i == 0 { &prefix } else { "," });
                output.push_str("\n  ");
            } else {
                output.push_str(&prefix);
                output.push(' ');
            }
            // A cell without a type falls back to Unknown, which quotes.
            Self::write_values(
                db_type,
                row.cells.iter().map(|cell| {
                    (
                        cell.value.as_deref(),
                        cell.column_type.as_ref().unwrap_or(&unknown),
                    )
                }),
                &mut output,
            );
            if !self.multi_row {
                output.push_str(";\n");
            }
        }
        if self.multi_row {
            output.push_str(";\n");
        }

        Ok(output)
    }

    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let table_name = self.table_name.as_deref().unwrap_or("exported_data");
        *self
            .statement_prefix
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            statement_prefix(table_name, columns, self.db_type);
        // The statement starts with the first row, so an export without rows
        // is an empty file rather than an INSERT without values.
        Ok(String::new())
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        _columns: &[String],
        column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let prefix = self
            .statement_prefix
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut output = String::with_capacity(prefix.len() + row_data.len() * 50 + 20);
        let first = self
            .first_row
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok();
        if self.multi_row {
            output.push_str(if first { &prefix } else { "," });
            output.push_str("\n  ");
        } else {
            output.push_str(&prefix);
            output.push(' ');
        }

        let unknown = ColumnType::Unknown;
        Self::write_values(
            self.db_type,
            row_data
                .iter()
                .enumerate()
                .map(|(i, value)| (value.as_deref(), column_types.get(i).unwrap_or(&unknown))),
            &mut output,
        );
        if !self.multi_row {
            output.push_str(";\n");
        }
        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        // Close the INSERT, unless every row closed its own or none came.
        let wrote_rows = !self.first_row.load(Ordering::Relaxed);
        Ok(if self.multi_row && wrote_rows {
            ";\n".to_string()
        } else {
            String::new()
        })
    }

    fn transform_header_row(&self, _columns: &[String]) -> Result<String, TransformError> {
        // SQL doesn't need headers as a separate row
        Ok(String::new())
    }
}

pub(crate) fn sql_escape_string_to(value: &str, db_type: DatabaseType, output: &mut String) {
    if !value.contains('\'') && !value.contains('\\') {
        output.push_str(value);
        return;
    }
    output.push_str(&db_type.dialect().escape_string_literal(value));
}

pub(crate) fn sql_identifier(name: &str, db_type: DatabaseType) -> String {
    db_type.dialect().quote_identifier(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql_escape_string(value: &str) -> String {
        let mut output = String::new();
        sql_escape_string_to(value, DatabaseType::PostgreSQL, &mut output);
        output
    }

    fn format_sql_value(value: &str, column_type: Option<&ColumnType>) -> String {
        let col_type = column_type.unwrap_or(&ColumnType::Unknown);
        if should_quote_value(col_type) {
            format!("'{}'", sql_escape_string(value))
        } else {
            value.to_string()
        }
    }

    #[test]
    fn test_sql_escape() {
        assert_eq!(sql_escape_string("simple"), "simple");
        assert_eq!(sql_escape_string("contains' quote"), "contains'' quote");
        assert_eq!(sql_escape_string("multiple''quotes"), "multiple''''quotes");
    }

    #[test]
    fn test_sql_identifier_postgres() {
        assert_eq!(
            sql_identifier("simple", DatabaseType::PostgreSQL),
            "\"simple\""
        );
        assert_eq!(
            sql_identifier("contains\"quote", DatabaseType::PostgreSQL),
            "\"contains\"\"quote\""
        );
        assert_eq!(
            sql_identifier("table name", DatabaseType::PostgreSQL),
            "\"table name\""
        );
    }

    #[test]
    fn test_sql_identifier_mysql() {
        assert_eq!(sql_identifier("simple", DatabaseType::MySQL), "`simple`");
        assert_eq!(
            sql_identifier("contains`tick", DatabaseType::MySQL),
            "`contains``tick`"
        );
        assert_eq!(
            sql_identifier("table name", DatabaseType::MySQL),
            "`table name`"
        );
    }

    #[test]
    fn test_sql_identifier_sqlite() {
        assert_eq!(sql_identifier("simple", DatabaseType::SQLite), "\"simple\"");
    }

    #[test]
    fn test_should_quote_value() {
        assert!(!should_quote_value(&ColumnType::Integer));
        assert!(!should_quote_value(&ColumnType::UnsignedInteger));
        assert!(!should_quote_value(&ColumnType::Numeric));
        assert!(!should_quote_value(&ColumnType::Boolean));

        assert!(should_quote_value(&ColumnType::Text));
        assert!(should_quote_value(&ColumnType::DateTime));
        assert!(should_quote_value(&ColumnType::Uuid));
        assert!(should_quote_value(&ColumnType::Json));
        assert!(should_quote_value(&ColumnType::Array));
        assert!(should_quote_value(&ColumnType::Binary));
        assert!(should_quote_value(&ColumnType::Unknown));
    }

    #[test]
    fn test_format_sql_value_numeric() {
        assert_eq!(format_sql_value("42", Some(&ColumnType::Integer)), "42");
        assert_eq!(format_sql_value("0", Some(&ColumnType::Integer)), "0");
        assert_eq!(format_sql_value("-123", Some(&ColumnType::Integer)), "-123");
        assert_eq!(
            format_sql_value("42", Some(&ColumnType::UnsignedInteger)),
            "42"
        );
        assert_eq!(format_sql_value("3.14", Some(&ColumnType::Numeric)), "3.14");
        assert_eq!(format_sql_value("0.0", Some(&ColumnType::Numeric)), "0.0");
        assert_eq!(
            format_sql_value("-99.99", Some(&ColumnType::Numeric)),
            "-99.99"
        );
    }

    #[test]
    fn test_format_sql_value_boolean() {
        assert_eq!(format_sql_value("true", Some(&ColumnType::Boolean)), "true");
        assert_eq!(
            format_sql_value("false", Some(&ColumnType::Boolean)),
            "false"
        );
        assert_eq!(format_sql_value("TRUE", Some(&ColumnType::Boolean)), "TRUE");
        assert_eq!(
            format_sql_value("FALSE", Some(&ColumnType::Boolean)),
            "FALSE"
        );
    }

    #[test]
    fn test_format_sql_value_text() {
        assert_eq!(
            format_sql_value("hello", Some(&ColumnType::Text)),
            "'hello'"
        );
        assert_eq!(format_sql_value("it's", Some(&ColumnType::Text)), "'it''s'");
        assert_eq!(format_sql_value("", Some(&ColumnType::Text)), "''");
        assert_eq!(
            format_sql_value("2024-01-15", Some(&ColumnType::DateTime)),
            "'2024-01-15'"
        );
        assert_eq!(
            format_sql_value(
                "550e8400-e29b-41d4-a716-446655440000",
                Some(&ColumnType::Uuid)
            ),
            "'550e8400-e29b-41d4-a716-446655440000'"
        );
        assert_eq!(
            format_sql_value("{\"key\": \"value\"}", Some(&ColumnType::Json)),
            "'{\"key\": \"value\"}'"
        );
        assert_eq!(
            format_sql_value("value", Some(&ColumnType::Unknown)),
            "'value'"
        );
    }

    #[test]
    fn test_format_sql_value_empty_and_literal_null() {
        assert_eq!(format_sql_value("", Some(&ColumnType::Integer)), "");
        assert_eq!(format_sql_value("NULL", Some(&ColumnType::Integer)), "NULL");
        assert_eq!(format_sql_value("null", Some(&ColumnType::Text)), "'null'");
        assert_eq!(format_sql_value("NULL", Some(&ColumnType::Boolean)), "NULL");
    }

    #[test]
    fn test_format_sql_value_none_type() {
        assert_eq!(format_sql_value("value", None), "'value'");
        assert_eq!(format_sql_value("", None), "''");
        assert_eq!(format_sql_value("NULL", None), "'NULL'");
    }

    #[test]
    fn test_sql_transformer() {
        let transformer = SqlTransformer::new();
        assert_eq!(transformer.format_name(), "SQL");
    }

    fn stream(transformer: &SqlTransformer, rows: &[Vec<Option<String>>]) -> String {
        let columns = ["id".to_string(), "name".to_string()];
        let types = [ColumnType::Integer, ColumnType::Text];
        let mut output = transformer.initialize_stream(&columns, &types).unwrap();
        for row in rows {
            output.push_str(
                &transformer
                    .transform_stream_row(row, &columns, &types)
                    .unwrap(),
            );
        }
        output.push_str(&transformer.finalize_stream().unwrap());
        output
    }

    #[test]
    fn test_streamed_insert_styles() {
        let rows = [
            vec![Some("1".to_string()), Some("a".to_string())],
            vec![Some("2".to_string()), None],
        ];
        let table = || SqlTransformer::with_table_name("t".into(), DatabaseType::PostgreSQL);

        assert_eq!(
            stream(&table(), &rows),
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES\n  (1, 'a'),\n  (2, NULL);\n"
        );
        assert_eq!(
            stream(&table().multi_row(false), &rows),
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES (1, 'a');\n\
             INSERT INTO \"t\" (\"id\", \"name\") VALUES (2, NULL);\n"
        );
        assert_eq!(stream(&table(), &[]), "");
    }
}
