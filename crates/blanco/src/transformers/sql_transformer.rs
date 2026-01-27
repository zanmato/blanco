use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SqlTransformer {
    table_name: Option<String>,
    first_row: AtomicBool,
    column_list: Mutex<Option<String>>,
}

impl SqlTransformer {
    pub fn new() -> Self {
        Self {
            table_name: None,
            first_row: AtomicBool::new(true),
            column_list: Mutex::new(None),
        }
    }

    pub fn with_table_name(table_name: String) -> Self {
        Self {
            table_name: Some(table_name),
            first_row: AtomicBool::new(true),
            column_list: Mutex::new(None),
        }
    }
}

impl DataTransformer for SqlTransformer {
    fn format_name(&self) -> &'static str {
        "SQL"
    }

    fn file_extension(&self) -> &'static str {
        "sql"
    }

    fn description(&self) -> &'static str {
        "SQL INSERT statements with proper escaping"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        // Use provided table name or a default generic name
        let table_name = data
            .table_name
            .as_ref()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        let mut output = String::new();

        // Handle selected rows first
        if !data.selected_rows.is_empty() {
            // Generate column list once
            let column_list = data
                .columns
                .iter()
                .map(|col| sql_identifier(col))
                .collect::<Vec<_>>()
                .join(", ");

            let mut row_indices: Vec<usize> = data.selected_rows.iter().map(|r| r.row).collect();
            row_indices.sort();

            // Create the INSERT statement
            output.push_str(&format!(
                "INSERT INTO {} ({})\nVALUES\n",
                sql_identifier(&table_name),
                column_list
            ));

            let row_count = row_indices.len();
            let mut i = 0;
            for row_idx in row_indices {
                if let Some(row) = data.selected_rows.iter().find(|r| r.row == row_idx) {
                    // Add values with proper escaping
                    output.push('(');
                    let values: Vec<String> = row
                        .cells
                        .iter()
                        .map(|cell| {
                            if cell.value.is_empty() {
                                "''".to_string()
                            } else if cell.value.eq_ignore_ascii_case("null") {
                                "NULL".to_string()
                            } else {
                                format!("'{}'", sql_escape_string(&cell.value))
                            }
                        })
                        .collect();

                    output.push_str(&values.join(", "));

                    i += 1;

                    if i < row_count {
                        output.push_str("),\n");
                    } else {
                        output.push_str(");\n");
                    }
                }
            }
        }

        Ok(output)
    }

    fn transform_single_cell(
        &self,
        value: &str,
        _column_type: &str,
    ) -> Result<String, TransformError> {
        if value.is_empty() || value.eq_ignore_ascii_case("null") {
            Ok("NULL".to_string())
        } else {
            Ok(format!("'{}'", sql_escape_string(value)))
        }
    }

    // === Streaming Methods ===

    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[String],
    ) -> Result<String, TransformError> {
        // Generate and store column list once
        let column_list = columns
            .iter()
            .map(|col| sql_identifier(col))
            .collect::<Vec<_>>()
            .join(", ");

        *self.column_list.lock().unwrap() = Some(column_list);

        // Start with INSERT statement header
        let table_name = self.table_name.as_deref().unwrap_or("exported_data");
        Ok(format!(
            "INSERT INTO {} ({})\nVALUES\n",
            sql_identifier(table_name),
            self.column_list.lock().unwrap().as_ref().unwrap()
        ))
    }

    fn transform_stream_row(
        &self,
        row_data: &[String],
        _columns: &[String],
        _column_types: &[String],
    ) -> Result<String, TransformError> {
        // Add values with proper escaping
        let values: Vec<String> = row_data
            .iter()
            .map(|value| {
                if value.is_empty() || value.eq_ignore_ascii_case("null") {
                    "NULL".to_string()
                } else {
                    format!("'{}'", sql_escape_string(value))
                }
            })
            .collect();

        // Format as a single VALUES row with proper indentation
        let row_str = format!("  ({})", values.join(", "));

        // Add comma separator if this is not the first row
        if self
            .first_row
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            // This was the first row and we successfully set it to false
            Ok(row_str)
        } else {
            // This is not the first row
            Ok(format!(",\n{}", row_str))
        }
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

/// Escape a string for SQL (single quotes)
fn sql_escape_string(value: &str) -> String {
    value.replace('\'', "''")
}

/// Quote a SQL identifier safely
fn sql_identifier(name: &str) -> String {
    // Simple identifier quoting - wrap in double quotes and escape any existing quotes
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_escape() {
        assert_eq!(sql_escape_string("simple"), "simple");
        assert_eq!(sql_escape_string("contains' quote"), "contains'' quote");
        assert_eq!(sql_escape_string("multiple''quotes"), "multiple''''quotes");
    }

    #[test]
    fn test_sql_identifier() {
        assert_eq!(sql_identifier("simple"), "\"simple\"");
        assert_eq!(sql_identifier("contains\"quote"), "\"contains\"\"quote\"");
        assert_eq!(sql_identifier("table name"), "\"table name\"");
    }

    #[test]
    fn test_sql_transformer() {
        let transformer = SqlTransformer::new();
        assert_eq!(transformer.format_name(), "SQL");
        assert_eq!(transformer.file_extension(), "sql");
        assert_eq!(
            transformer.transform_single_cell("test", "").unwrap(),
            "'test'"
        );
        assert_eq!(transformer.transform_single_cell("", "").unwrap(), "NULL");
        assert_eq!(
            transformer.transform_single_cell("NULL", "").unwrap(),
            "NULL"
        );
        assert_eq!(
            transformer.transform_single_cell("it's", "").unwrap(),
            "'it''s'"
        );
    }
}
