use crate::transformers::sql_transformer::{
    should_quote_value, sql_escape_string_to, sql_identifier,
};
use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::ColumnType;
use database::DatabaseType;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ValuesTransformer {
    table_name: Option<String>,
    db_type: DatabaseType,
    first_row: AtomicBool,
    // Captured during `initialize_stream` so `finalize_stream` can emit the
    // trailing `AS t(col, ...)` column list. The streaming API hands columns to
    // `initialize_stream` but not to `finalize_stream`, so we stash them here.
    stream_columns: Mutex<Vec<String>>,
}

impl ValuesTransformer {
    pub fn new() -> Self {
        Self {
            table_name: None,
            db_type: DatabaseType::PostgreSQL,
            first_row: AtomicBool::new(true),
            stream_columns: Mutex::new(Vec::new()),
        }
    }

    #[expect(dead_code)]
    pub fn with_table_name(table_name: String, db_type: DatabaseType) -> Self {
        Self {
            table_name: Some(table_name),
            db_type,
            first_row: AtomicBool::new(true),
            stream_columns: Mutex::new(Vec::new()),
        }
    }
}

fn format_row_values(
    cells: &[Option<String>],
    column_types: &[Option<ColumnType>],
    use_row_keyword: bool,
) -> String {
    let mut output = String::with_capacity(cells.len() * 30);

    if use_row_keyword {
        output.push_str("ROW(");
    } else {
        output.push('(');
    }

    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            output.push_str(", ");
        }

        let col_type = column_types
            .get(i)
            .and_then(|ct| ct.as_ref())
            .unwrap_or(&ColumnType::Unknown);

        match cell {
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
    output
}

fn effective_table_name(table_name: Option<&String>) -> &str {
    table_name.map(|s| s.as_str()).unwrap_or("t")
}

fn build_column_list(columns: &[String], db_type: DatabaseType) -> String {
    columns
        .iter()
        .map(|col| sql_identifier(col, db_type))
        .collect::<Vec<_>>()
        .join(", ")
}

impl DataTransformer for ValuesTransformer {
    fn format_name(&self) -> &'static str {
        "VALUES"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        let db_type = data.db_type.unwrap_or(self.db_type);
        let table_name = effective_table_name(data.table_name.as_ref());
        let column_list = build_column_list(&data.columns, db_type);

        let estimated_capacity = (data.selected_rows.len() * 150) + 300;
        let mut output = String::with_capacity(estimated_capacity);

        if matches!(db_type, DatabaseType::SQLite) {
            output.push_str("WITH ");
            output.push_str(&sql_identifier(table_name, db_type));
            output.push('(');
            output.push_str(&column_list);
            output.push_str(")\nAS (\nVALUES\n");
        } else {
            output.push_str("SELECT *\nFROM (\nVALUES\n");
        }

        let use_row_keyword = matches!(db_type, DatabaseType::MySQL);
        let row_count = data.selected_rows.len();

        for (i, row) in data.selected_rows.iter().enumerate() {
            let column_types: Vec<Option<ColumnType>> =
                row.cells.iter().map(|cell| cell.column_type).collect();

            let values = format_row_values(
                &row.cells
                    .iter()
                    .map(|c| c.value.clone())
                    .collect::<Vec<_>>(),
                &column_types,
                use_row_keyword,
            );

            output.push_str("  ");
            output.push_str(&values);

            if i < row_count - 1 {
                output.push_str(",\n");
            } else {
                output.push('\n');
            }
        }

        if matches!(db_type, DatabaseType::SQLite) {
            output.push_str(")\nSELECT *\nFROM ");
            output.push_str(&sql_identifier(table_name, db_type));
            output.push_str(";\n");
        } else {
            output.push_str(") AS ");
            output.push_str(&sql_identifier(table_name, db_type));
            output.push('(');
            output.push_str(&column_list);
            output.push_str(");\n");
        }

        Ok(output)
    }

    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let db_type = self.db_type;
        let table_name = effective_table_name(self.table_name.as_ref());
        let column_list = build_column_list(columns, db_type);

        if let Ok(mut stored) = self.stream_columns.lock() {
            *stored = columns.to_vec();
        }

        let mut output = String::with_capacity(200);

        if matches!(db_type, DatabaseType::SQLite) {
            output.push_str("WITH ");
            output.push_str(&sql_identifier(table_name, db_type));
            output.push('(');
            output.push_str(&column_list);
            output.push_str(")\nAS (\nVALUES\n");
        } else {
            output.push_str("SELECT *\nFROM (\nVALUES\n");
        }

        Ok(output)
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        _columns: &[String],
        column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let db_type = self.db_type;
        let use_row_keyword = matches!(db_type, DatabaseType::MySQL);

        let mut output = String::with_capacity((row_data.len() * 50) + 20);

        if self
            .first_row
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            output.push_str(",\n");
        }

        output.push_str("  ");

        let column_type_refs: Vec<Option<ColumnType>> =
            column_types.iter().map(|ct| Some(*ct)).collect();

        let values = format_row_values(row_data, &column_type_refs, use_row_keyword);
        output.push_str(&values);

        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        let db_type = self.db_type;
        let table_name = effective_table_name(self.table_name.as_ref());

        let mut output = String::with_capacity(200);
        output.push('\n');

        if matches!(db_type, DatabaseType::SQLite) {
            output.push_str(")\nSELECT *\nFROM ");
            output.push_str(&sql_identifier(table_name, db_type));
            output.push_str(";\n");
        } else {
            let column_list = self
                .stream_columns
                .lock()
                .map(|columns| build_column_list(&columns, db_type))
                .unwrap_or_default();
            output.push_str(") AS ");
            output.push_str(&sql_identifier(table_name, db_type));
            output.push('(');
            output.push_str(&column_list);
            output.push_str(");\n");
        }

        Ok(output)
    }

    fn transform_header_row(&self, _columns: &[String]) -> Result<String, TransformError> {
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results_panel::{SelectedCell, SelectedRow, SelectedTableData};
    use blanco_core::ColumnType;

    fn make_data(db_type: DatabaseType, table_name: Option<&str>) -> SelectedTableData {
        SelectedTableData {
            table_name: table_name.map(|s| s.to_string()),
            db_type: Some(db_type),
            columns: vec!["id".to_string(), "name".to_string()],
            selected_rows: vec![
                SelectedRow {
                    row: 0,
                    cells: vec![
                        SelectedCell {
                            col: 0,
                            value: Some("1".to_string()),
                            column_name: Some("id".to_string()),
                            column_type: Some(ColumnType::Integer),
                        },
                        SelectedCell {
                            col: 1,
                            value: Some("Alice".to_string()),
                            column_name: Some("name".to_string()),
                            column_type: Some(ColumnType::Text),
                        },
                    ],
                },
                SelectedRow {
                    row: 1,
                    cells: vec![
                        SelectedCell {
                            col: 0,
                            value: None,
                            column_name: Some("id".to_string()),
                            column_type: Some(ColumnType::Integer),
                        },
                        SelectedCell {
                            col: 1,
                            value: Some("Bob's".to_string()),
                            column_name: Some("name".to_string()),
                            column_type: Some(ColumnType::Text),
                        },
                    ],
                },
            ],
        }
    }

    #[test]
    fn test_format_name() {
        let transformer = ValuesTransformer::new();
        assert_eq!(transformer.format_name(), "VALUES");
    }

    #[test]
    fn test_postgres_output() {
        let transformer = ValuesTransformer::new();
        let data = make_data(DatabaseType::PostgreSQL, Some("users"));
        let result = transformer.transform_selected_data(&data).unwrap();

        assert!(result.starts_with("SELECT *\nFROM (\nVALUES\n"));
        assert!(result.contains("  (1, 'Alice')"));
        assert!(result.contains("  (NULL, 'Bob''s')"));
        assert!(result.contains(") AS \"users\"(\"id\", \"name\");\n"));
    }

    #[test]
    fn test_clickhouse_output() {
        let transformer = ValuesTransformer::new();
        let data = make_data(DatabaseType::ClickHouse, Some("events"));
        let result = transformer.transform_selected_data(&data).unwrap();

        assert!(result.starts_with("SELECT *\nFROM (\nVALUES\n"));
        assert!(result.contains(") AS \"events\"(\"id\", \"name\");\n"));
    }

    #[test]
    fn test_mysql_output() {
        let transformer = ValuesTransformer::new();
        let data = make_data(DatabaseType::MySQL, Some("users"));
        let result = transformer.transform_selected_data(&data).unwrap();

        assert!(result.starts_with("SELECT *\nFROM (\nVALUES\n"));
        assert!(result.contains("  ROW(1, 'Alice')"));
        assert!(result.contains("  ROW(NULL, 'Bob''s')"));
        assert!(result.contains(") AS `users`(`id`, `name`);\n"));
    }

    #[test]
    fn test_sqlite_output() {
        let transformer = ValuesTransformer::new();
        let data = make_data(DatabaseType::SQLite, Some("users"));
        let result = transformer.transform_selected_data(&data).unwrap();

        assert!(result.starts_with("WITH \"users\"(\"id\", \"name\")\nAS (\nVALUES\n"));
        assert!(result.contains("  (1, 'Alice')"));
        assert!(result.contains("  (NULL, 'Bob''s')"));
        assert!(result.contains(")\nSELECT *\nFROM \"users\";\n"));
    }

    #[test]
    fn test_no_table_name_falls_back_to_t() {
        let transformer = ValuesTransformer::new();
        let data = make_data(DatabaseType::PostgreSQL, None);
        let result = transformer.transform_selected_data(&data).unwrap();

        assert!(result.contains(") AS \"t\"("));
    }

    #[test]
    fn test_empty_selection_returns_error() {
        let transformer = ValuesTransformer::new();
        let data = SelectedTableData::default();
        let result = transformer.transform_selected_data(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_single_row() {
        let transformer = ValuesTransformer::new();
        let data = SelectedTableData {
            table_name: Some("t".to_string()),
            db_type: Some(DatabaseType::PostgreSQL),
            columns: vec!["x".to_string()],
            selected_rows: vec![SelectedRow {
                row: 0,
                cells: vec![SelectedCell {
                    col: 0,
                    value: Some("42".to_string()),
                    column_name: Some("x".to_string()),
                    column_type: Some(ColumnType::Integer),
                }],
            }],
        };

        let result = transformer.transform_selected_data(&data).unwrap();
        assert_eq!(
            result,
            "SELECT *\nFROM (\nVALUES\n  (42)\n) AS \"t\"(\"x\");\n"
        );
    }

    #[test]
    fn test_stream_finalize_emits_column_list() {
        // The streaming path hands columns to `initialize_stream` only, so the
        // transformer must carry them through to `finalize_stream` to produce a
        // valid `AS t(col, ...)` clause for non-SQLite dialects.
        let transformer = ValuesTransformer::new();
        let columns = vec!["id".to_string(), "name".to_string()];
        let column_types = vec![ColumnType::Integer, ColumnType::Text];

        let mut output = transformer
            .initialize_stream(&columns, &column_types)
            .unwrap();
        output.push_str(
            &transformer
                .transform_stream_row(
                    &[Some("1".to_string()), Some("Widget".to_string())],
                    &columns,
                    &column_types,
                )
                .unwrap(),
        );
        output.push_str(&transformer.finalize_stream().unwrap());

        assert!(
            output.ends_with(") AS \"t\"(\"id\", \"name\");\n"),
            "streamed VALUES output should close with the column list, got: {output}"
        );
    }
}
