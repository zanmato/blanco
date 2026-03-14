use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::connection_trait::ColumnType;

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

        // Estimate capacity: ~80 bytes per cell on average (including formatting)
        let cell_count = data.selected_rows.iter().map(|r| r.cells.len()).sum::<usize>();
        let estimated_capacity = (cell_count * 80) + (data.columns.len() * 30) + 200;
        let mut output = String::with_capacity(estimated_capacity);

        // Add table name as a header if available
        if let Some(table_name) = &data.table_name {
            output.push_str("# Table: ");
            output.push_str(table_name);
            output.push_str("\n\n");
        }

        // Determine which columns to include based on selection
        let mut min_col = usize::MAX;
        let mut max_col = 0;

        // Check selected rows
        for row in &data.selected_rows {
            for cell in &row.cells {
                min_col = min_col.min(cell.col);
                max_col = max_col.max(cell.col);
            }
        }

        // If no columns were found, include all columns
        if min_col == usize::MAX {
            min_col = 0;
            max_col = data.columns.len().saturating_sub(1);
        }

        // Build column list and widths for formatting
        let mut columns: Vec<(usize, String)> = Vec::new();
        let mut column_widths: Vec<usize> = Vec::new();

        for col_idx in min_col..=max_col {
            if let Some(col_name) = data.columns.get(col_idx) {
                columns.push((col_idx, col_name.clone()));
                let mut max_width = col_name.len();

                // Consider selected rows for this column - direct iteration
                for row in &data.selected_rows {
                    for cell in &row.cells {
                        if cell.col == col_idx {
                            max_width = max_width.max(cell.value.as_ref().map_or(4, |v| v.len()));
                            break; // Found the cell for this column, move to next row
                        }
                    }
                }

                column_widths.push(max_width.max(3)); // Minimum width
            }
        }

        // Create table header
        output.push('|');
        for (i, (_, col_name)) in columns.iter().enumerate() {
            output.push(' ');
            format_cell_to(col_name, column_widths[i], &mut output);
            output.push_str(" |");
        }
        output.push('\n');

        // Create separator row
        output.push('|');
        for width in &column_widths {
            output.push(' ');
            for _ in 0..*width {
                output.push('-');
            }
            output.push_str(" |");
        }
        output.push('\n');

        // If we have selected rows, output complete rows
        if !data.selected_rows.is_empty() {
            for row in &data.selected_rows {
                output.push('|');
                for (i, &(col_idx, _)) in columns.iter().enumerate() {
                    output.push(' ');
                    // Find cell by column index - direct iteration
                    let mut cell_found = false;
                    for cell in &row.cells {
                        if cell.col == col_idx {
                            format_cell_to(cell.value.as_deref().unwrap_or("NULL"), column_widths[i], &mut output);
                            cell_found = true;
                            break;
                        }
                    }
                    if !cell_found {
                        format_cell_to("", column_widths[i], &mut output);
                    }
                    output.push_str(" |");
                }
                output.push('\n');
            }
        }

        Ok(output)
    }

    fn transform_single_cell(
        &self,
        value: &str,
        _column_type: &ColumnType,
    ) -> Result<String, TransformError> {
        // For markdown, we just return the value as-is
        Ok(value.to_string())
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        _columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        // Markdown streaming is not well-supported due to column width calculation,
        // but we provide a basic implementation for completeness
        let mut output = String::new();
        output.push('|');
        for value in row_data {
            output.push(' ');
            output.push_str(value.as_deref().unwrap_or("NULL"));
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

/// Format a cell value with proper padding for markdown table - returns a new String
fn format_cell(value: &str, width: usize) -> String {
    if value.len() >= width {
        value.to_string()
    } else {
        format!("{}{}", value, " ".repeat(width - value.len()))
    }
}

/// Format a cell value with proper padding - writes directly to buffer
/// This avoids allocating a new String for each cell
fn format_cell_to(value: &str, width: usize, output: &mut String) {
    output.push_str(value);
    let padding = width.saturating_sub(value.len());
    for _ in 0..padding {
        output.push(' ');
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
            transformer.transform_single_cell("test", &ColumnType::Text).unwrap(),
            "test"
        );
    }
}
