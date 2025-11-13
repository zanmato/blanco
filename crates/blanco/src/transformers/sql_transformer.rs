use crate::transformers::{DataTransformer, SelectedTableData, TransformError};

pub struct SqlTransformer;

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

            for row_idx in row_indices {
                if let Some(row) = data.selected_rows.iter().find(|r| r.row == row_idx) {
                    // Create the INSERT statement
                    output.push_str(&format!(
                        "INSERT INTO {} ({})\nVALUES (",
                        sql_identifier(&table_name),
                        column_list
                    ));

                    // Add values with proper escaping
                    let values: Vec<String> = row
                        .cells
                        .iter()
                        .map(|cell| {
                            if cell.value.is_empty() || cell.value.eq_ignore_ascii_case("null") {
                                "NULL".to_string()
                            } else {
                                format!("'{}'", sql_escape_string(&cell.value))
                            }
                        })
                        .collect();

                    output.push_str(&values.join(", "));
                    output.push_str(");\n\n");
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
        let transformer = SqlTransformer;
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
