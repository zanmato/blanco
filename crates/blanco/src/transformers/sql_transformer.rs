use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::ColumnType;
use database::DatabaseType;
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
    first_row: AtomicBool,
}

impl SqlTransformer {
    pub fn new() -> Self {
        Self {
            table_name: None,
            db_type: DatabaseType::PostgreSQL,
            first_row: AtomicBool::new(true),
        }
    }

    pub fn with_table_name(table_name: String, db_type: DatabaseType) -> Self {
        Self {
            table_name: Some(table_name),
            db_type,
            first_row: AtomicBool::new(true),
        }
    }
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

        // Estimate capacity: ~150 bytes per row on average plus header
        let estimated_capacity = (data.selected_rows.len() * 150) + 200;
        let mut output = String::with_capacity(estimated_capacity);

        // Handle selected rows first
        if !data.selected_rows.is_empty() {
            // Generate column list once
            let mut column_list = String::with_capacity(data.columns.len() * 20);
            for (i, col) in data.columns.iter().enumerate() {
                if i > 0 {
                    column_list.push_str(", ");
                }
                column_list.push_str(&sql_identifier(col, db_type));
            }

            // Create the INSERT statement header
            output.push_str("INSERT INTO ");
            output.push_str(&sql_identifier(&table_name, db_type));
            output.push_str(" (");
            output.push_str(&column_list);
            output.push_str(")\nVALUES\n");

            let row_count = data.selected_rows.len();
            for (i, row) in data.selected_rows.iter().enumerate() {
                output.push('(');

                // Write values directly without intermediate Vec<String>
                let mut cell_iter = row.cells.iter().peekable();
                while let Some(cell) = cell_iter.next() {
                    // Use column type from cell if available, fall back to Unknown (which quotes)
                    let col_type = cell.column_type.as_ref().unwrap_or(&ColumnType::Unknown);

                    match &cell.value {
                        None => output.push_str("NULL"),
                        Some(val) if val.is_empty() => output.push_str("''"),
                        Some(val) => {
                            if should_quote_value(col_type) {
                                output.push('\'');
                                sql_escape_string_to(val, &mut output);
                                output.push('\'');
                            } else {
                                output.push_str(val);
                            }
                        }
                    }

                    // Add comma separator if not last cell
                    if cell_iter.peek().is_some() {
                        output.push_str(", ");
                    }
                }

                if i < row_count - 1 {
                    output.push_str("),\n");
                } else {
                    output.push_str(");\n");
                }
            }
        }

        Ok(output)
    }

    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let db_type = self.db_type;
        let column_list = columns
            .iter()
            .map(|col| sql_identifier(col, db_type))
            .collect::<Vec<_>>()
            .join(", ");

        let table_name = self.table_name.as_deref().unwrap_or("exported_data");
        Ok(format!(
            "INSERT INTO {} ({})\nVALUES\n",
            sql_identifier(table_name, db_type),
            column_list
        ))
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        _columns: &[String],
        column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        // Estimate capacity: ~50 bytes per cell plus indentation
        let estimated_capacity = (row_data.len() * 50) + 20;
        let mut output = String::with_capacity(estimated_capacity);

        // Add comma separator if this is not the first row
        if self
            .first_row
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            output.push_str(",\n");
        }

        output.push_str("  (");

        // Write values directly without intermediate Vec<String>
        for (i, value) in row_data.iter().enumerate() {
            if i > 0 {
                output.push_str(", ");
            }

            // Get column type for this column, default to Unknown
            let col_type = column_types.get(i).unwrap_or(&ColumnType::Unknown);

            match value {
                None => output.push_str("NULL"),
                Some(val) if val.is_empty() => output.push_str("''"),
                Some(val) => {
                    if should_quote_value(col_type) {
                        output.push('\'');
                        sql_escape_string_to(val, &mut output);
                        output.push('\'');
                    } else {
                        output.push_str(val);
                    }
                }
            }
        }

        output.push(')');

        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        // End the INSERT statement with semicolon
        Ok(";\n".to_string())
    }

    fn transform_header_row(&self, _columns: &[String]) -> Result<String, TransformError> {
        // SQL doesn't need headers as a separate row
        Ok(String::new())
    }
}

pub(crate) fn sql_escape_string_to(value: &str, output: &mut String) {
    // Check if we need to escape at all
    if !value.contains('\'') {
        output.push_str(value);
        return;
    }

    for c in value.chars() {
        if c == '\'' {
            output.push_str("''");
        } else {
            output.push(c);
        }
    }
}

pub(crate) fn sql_identifier(name: &str, db_type: DatabaseType) -> String {
    match db_type {
        DatabaseType::MySQL => format!("`{}`", name.replace('`', "``")),
        DatabaseType::PostgreSQL | DatabaseType::SQLite | DatabaseType::ClickHouse => {
            format!("\"{}\"", name.replace('"', "\"\""))
        }
        DatabaseType::MsSql => format!("[{}]", name.replace(']', "]]")),
        // Redis results are not exported as SQL INSERTs; fall back to
        // double-quote identifier quoting so the match stays exhaustive.
        DatabaseType::Redis => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql_escape_string(value: &str) -> String {
        let mut output = String::new();
        sql_escape_string_to(value, &mut output);
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
}
