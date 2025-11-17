use crate::transformers::{DataTransformer, SelectedTableData, TransformError};

pub struct MarkdownTransformer;

impl DataTransformer for MarkdownTransformer {
    fn format_name(&self) -> &'static str {
        "Markdown"
    }

    fn file_extension(&self) -> &'static str {
        "md"
    }

    fn description(&self) -> &'static str {
        "GitHub-flavored markdown table format"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        let mut output = String::new();

        // Add table name as a header if available
        if let Some(table_name) = &data.table_name {
            output.push_str(&format!("# Table: {}\n\n", table_name));
        }

        // Determine which columns to include based on selection
        let mut included_columns = std::collections::HashSet::new();
        let mut min_col = usize::MAX;
        let mut max_col = 0;

        // Check selected rows
        for row in &data.selected_rows {
            for cell in &row.cells {
                included_columns.insert(cell.col);
                min_col = min_col.min(cell.col);
                max_col = max_col.max(cell.col);
            }
        }

        // If no columns were found, include all columns
        if included_columns.is_empty() {
            min_col = 0;
            max_col = data.columns.len().saturating_sub(1);
            for i in 0..=max_col {
                included_columns.insert(i);
            }
        }

        // Build column list and widths for formatting
        let mut columns: Vec<(usize, String)> = Vec::new();
        let mut column_widths: Vec<usize> = Vec::new();

        for col_idx in min_col..=max_col {
            if let Some(col_name) = data.columns.get(col_idx) {
                columns.push((col_idx, col_name.clone()));
                let mut max_width = col_name.len();

                // Consider selected rows for this column
                for row in &data.selected_rows {
                    if let Some(cell) = row.cells.iter().find(|c| c.col == col_idx) {
                        max_width = max_width.max(cell.value.len());
                    }
                }

                column_widths.push(max_width.max(3)); // Minimum width
            }
        }

        // Create table header
        output.push('|');
        for (i, (_, col_name)) in columns.iter().enumerate() {
            output.push(' ');
            output.push_str(&format_cell(col_name, column_widths[i]));
            output.push_str(" |");
        }
        output.push('\n');

        // Create separator row
        output.push('|');
        for width in &column_widths {
            output.push(' ');
            output.push_str(&"-".repeat(*width));
            output.push_str(" |");
        }
        output.push('\n');

        // If we have selected rows, output complete rows
        if !data.selected_rows.is_empty() {
            let mut row_indices: Vec<usize> = data.selected_rows.iter().map(|r| r.row).collect();
            row_indices.sort();

            for row_idx in row_indices {
                if let Some(row) = data.selected_rows.iter().find(|r| r.row == row_idx) {
                    output.push('|');
                    for (i, &(col_idx, _)) in columns.iter().enumerate() {
                        output.push(' ');
                        if let Some(cell) = row.cells.iter().find(|c| c.col == col_idx) {
                            output.push_str(&format_cell(&cell.value, column_widths[i]));
                        } else {
                            output.push_str(&format_cell("", column_widths[i]));
                        }
                        output.push_str(" |");
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
        // For markdown, we just return the value as-is
        Ok(value.to_string())
    }

    fn transform_stream_row(
        &self,
        row_data: &[String],
        columns: &[String],
        _column_types: &[String],
    ) -> Result<String, TransformError> {
        // Markdown streaming is not well-supported due to column width calculation,
        // but we provide a basic implementation for completeness
        let mut output = String::new();
        output.push('|');
        for value in row_data {
            output.push(' ');
            output.push_str(value);
            output.push_str(" |");
        }
        output.push('\n');
        Ok(output)
    }

    fn supports_streaming(&self) -> bool {
        // Markdown doesn't support streaming due to column width calculation
        false
    }
}

/// Format a cell value with proper padding for markdown table
fn format_cell(value: &str, width: usize) -> String {
    if value.len() >= width {
        value.to_string()
    } else {
        format!("{}{}", value, " ".repeat(width - value.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_cell() {
        assert_eq!(format_cell("hello", 10), "hello     ");
        assert_eq!(format_cell("world", 5), "world");
        assert_eq!(format_cell("", 3), "   ");
        assert_eq!(format_cell("too long", 3), "too long");
    }

    #[test]
    fn test_markdown_transformer() {
        let transformer = MarkdownTransformer;
        assert_eq!(transformer.format_name(), "Markdown");
        assert_eq!(transformer.file_extension(), "md");
        assert_eq!(
            transformer.transform_single_cell("test", "").unwrap(),
            "test"
        );
    }
}
