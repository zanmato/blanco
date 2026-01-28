use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::connection_trait::ColumnType;

pub struct CsvTransformer;

impl DataTransformer for CsvTransformer {
    fn format_name(&self) -> &'static str {
        "CSV"
    }

    fn file_extension(&self) -> &'static str {
        "csv"
    }

    fn description(&self) -> &'static str {
        "Comma-separated values with proper quoting"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        let mut output = String::new();

        // If we have selected rows, export complete rows
        if !data.selected_rows.is_empty() {
            // Output header
            if !data.columns.is_empty() {
                for (i, col) in data.columns.iter().enumerate() {
                    if i > 0 {
                        output.push(';');
                    }
                    output.push_str(&csv_escape(col));
                }
                output.push('\n');
            }

            // Output selected rows
            let mut row_indices: Vec<usize> = data.selected_rows.iter().map(|r| r.row).collect();
            row_indices.sort();

            for row_idx in row_indices {
                if let Some(row) = data.selected_rows.iter().find(|r| r.row == row_idx) {
                    for (i, cell) in row.cells.iter().enumerate() {
                        if i > 0 {
                            output.push(';');
                        }
                        output.push_str(&csv_escape(&cell.value));
                    }
                    output.push('\n');
                }
            }
        }

        Ok(output)
    }

    fn transform_single_cell(
        &self,
        value: &str,
        _column_type: &ColumnType,
    ) -> Result<String, TransformError> {
        Ok(csv_escape(value))
    }

    // === Streaming Methods ===

    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let mut output = String::new();

        // Output CSV header
        if !columns.is_empty() {
            for (i, col) in columns.iter().enumerate() {
                if i > 0 {
                    output.push(';');
                }
                output.push_str(&csv_escape(col));
            }
            output.push('\n');
        }

        Ok(output)
    }

    fn transform_stream_row(
        &self,
        row_data: &[String],
        _columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let mut output = String::new();

        for (i, value) in row_data.iter().enumerate() {
            if i > 0 {
                output.push(';');
            }
            output.push_str(&csv_escape(value));
        }
        output.push('\n');

        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        // CSV doesn't need any special finalization
        Ok(String::new())
    }
}

/// Escape a value for CSV format
fn csv_escape(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }

    // Check if we need to quote the value
    let needs_quoting =
        value.contains(';') || value.contains('"') || value.contains('\n') || value.contains('\r');

    if needs_quoting {
        // Double up any quotes and wrap in quotes
        let escaped = value.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csv_escape() {
        assert_eq!(csv_escape("simple"), "simple");
        assert_eq!(csv_escape("contains; semicolon"), "\"contains; semicolon\"");
        assert_eq!(csv_escape("contains\"quote"), "\"contains\"\"quote\"");
        assert_eq!(csv_escape("multi\nline"), "\"multi\nline\"");
        assert_eq!(csv_escape(""), "");
    }

    #[test]
    fn test_csv_transformer() {
        let transformer = CsvTransformer;
        assert_eq!(transformer.format_name(), "CSV");
        assert_eq!(transformer.file_extension(), "csv");
        assert_eq!(
            transformer
                .transform_single_cell("test; value", &ColumnType::Text)
                .unwrap(),
            "\"test; value\""
        );
    }
}
