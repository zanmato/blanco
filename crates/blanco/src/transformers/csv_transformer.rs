use crate::transformers::{DataTransformer, SelectedTableData, TransformError};

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

        // If we have individual cell selections, export just those cells
        if !data.selected_cells.is_empty() {
            // Group cells by row for CSV format
            let mut rows: std::collections::HashMap<
                usize,
                Vec<&crate::results_panel::SelectedCell>,
            > = std::collections::HashMap::new();
            for cell in &data.selected_cells {
                rows.entry(cell.row).or_default().push(cell);
            }

            // Find the range of columns involved
            let mut min_col = usize::MAX;
            let mut max_col = 0;
            for cell in &data.selected_cells {
                min_col = min_col.min(cell.col);
                max_col = max_col.max(cell.col);
            }

            // Create header for the selected columns
            if min_col <= max_col && min_col < data.columns.len() {
                for col_idx in min_col..=max_col.min(data.columns.len() - 1) {
                    if col_idx > min_col {
                        output.push(',');
                    }
                    output.push_str(&csv_escape(&data.columns[col_idx]));
                }
                output.push('\n');
            }

            // Output the selected rows with only selected cells
            let mut row_indices: Vec<usize> = rows.keys().cloned().collect();
            row_indices.sort();

            for row_idx in row_indices {
                let cells = rows.get(&row_idx).unwrap();

                for col_idx in min_col..=max_col.min(data.columns.len() - 1) {
                    if col_idx > min_col {
                        output.push(',');
                    }

                    // Find the cell for this column, if any
                    if let Some(cell) = cells.iter().find(|c| c.col == col_idx) {
                        output.push_str(&csv_escape(&cell.value));
                    } else {
                        output.push_str(&csv_escape(""));
                    }
                }
                output.push('\n');
            }
        }
        // If we have selected rows, export complete rows
        else if !data.selected_rows.is_empty() {
            // Output header
            if !data.columns.is_empty() {
                for (i, col) in data.columns.iter().enumerate() {
                    if i > 0 {
                        output.push(',');
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
                            output.push(',');
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
        _column_type: &str,
    ) -> Result<String, TransformError> {
        Ok(csv_escape(value))
    }
}

/// Escape a value for CSV format
fn csv_escape(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }

    // Check if we need to quote the value
    let needs_quoting =
        value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r');

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
        assert_eq!(csv_escape("contains, comma"), "\"contains, comma\"");
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
                .transform_single_cell("test, value", "")
                .unwrap(),
            "\"test, value\""
        );
    }
}
