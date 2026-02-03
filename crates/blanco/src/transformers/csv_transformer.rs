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

        // Estimate capacity: ~50 bytes per cell on average
        let cell_count = data.selected_rows.iter().map(|r| r.cells.len()).sum::<usize>();
        let estimated_capacity = (cell_count * 50) + (data.columns.len() * 20) + 100;
        let mut output = String::with_capacity(estimated_capacity);

        // If we have selected rows, export complete rows
        if !data.selected_rows.is_empty() {
            // Output header
            if !data.columns.is_empty() {
                for (i, col) in data.columns.iter().enumerate() {
                    if i > 0 {
                        output.push(';');
                    }
                    csv_escape_to(col, &mut output);
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
                        csv_escape_to(&cell.value, &mut output);
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
        // Estimate capacity for header
        let estimated_capacity = columns.len() * 20 + 10;
        let mut output = String::with_capacity(estimated_capacity);

        // Output CSV header
        if !columns.is_empty() {
            for (i, col) in columns.iter().enumerate() {
                if i > 0 {
                    output.push(';');
                }
                csv_escape_to(col, &mut output);
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
        // Estimate capacity for row
        let estimated_capacity = row_data.len() * 50 + 10;
        let mut output = String::with_capacity(estimated_capacity);

        for (i, value) in row_data.iter().enumerate() {
            if i > 0 {
                output.push(';');
            }
            csv_escape_to(value, &mut output);
        }
        output.push('\n');

        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        // CSV doesn't need any special finalization
        Ok(String::new())
    }
}

/// Escape a value for CSV format - returns a new String
fn csv_escape(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }

    // Check if we need to quote the value
    let needs_quoting =
        value.contains(';') || value.contains('"') || value.contains('\n') || value.contains('\r');

    if needs_quoting {
        // Double up any quotes and wrap in quotes
        let mut result = String::with_capacity(value.len() + 10);
        result.push('"');
        for c in value.chars() {
            if c == '"' {
                result.push_str("\"\"");
            } else {
                result.push(c);
            }
        }
        result.push('"');
        result
    } else {
        value.to_string()
    }
}

/// Escape a value for CSV format - writes directly to buffer
/// This avoids allocating a new String for each cell value
fn csv_escape_to(value: &str, output: &mut String) {
    if value.is_empty() {
        return;
    }

    // Check if we need to quote the value
    let needs_quoting =
        value.contains(';') || value.contains('"') || value.contains('\n') || value.contains('\r');

    if needs_quoting {
        output.push('"');
        for c in value.chars() {
            if c == '"' {
                output.push_str("\"\"");
            } else {
                output.push(c);
            }
        }
        output.push('"');
    } else {
        output.push_str(value);
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
