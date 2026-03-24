use crate::transformers::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::connection_trait::ColumnType;

pub struct MarkdownTransformer;

impl DataTransformer for MarkdownTransformer {
    fn format_name(&self) -> &'static str {
        "Markdown"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        // Estimate capacity: ~80 bytes per cell on average (including formatting)
        let cell_count = data
            .selected_rows
            .iter()
            .map(|r| r.cells.len())
            .sum::<usize>();
        let estimated_capacity = (cell_count * 80) + (data.columns.len() * 30) + 200;
        let mut output = String::with_capacity(estimated_capacity);

        // Add table name as a header if available
        if let Some(table_name) = &data.table_name {
            output.push_str("# Table: ");
            output.push_str(&escape_markdown(table_name));
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

        // Build column list with escaped names, and compute widths
        let mut columns: Vec<(usize, String, String)> = Vec::new(); // (index, original, escaped)
        let mut column_widths: Vec<usize> = Vec::new();

        for col_idx in min_col..=max_col {
            if let Some(col_name) = data.columns.get(col_idx) {
                let escaped_name = escape_markdown(col_name);
                let mut max_width = escaped_name.len();

                // Consider selected rows for this column, using escaped cell lengths
                for row in &data.selected_rows {
                    for cell in &row.cells {
                        if cell.col == col_idx {
                            let cell_len = cell
                                .value
                                .as_ref()
                                .map_or(4, |v| escape_markdown(v).len());
                            max_width = max_width.max(cell_len);
                            break;
                        }
                    }
                }

                columns.push((col_idx, col_name.clone(), escaped_name));
                column_widths.push(max_width.max(3)); // Minimum width
            }
        }

        // Create table header
        output.push('|');
        for (i, (_, _, escaped_name)) in columns.iter().enumerate() {
            output.push(' ');
            format_cell_to(escaped_name, column_widths[i], &mut output);
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
                for (i, (col_idx, _, _)) in columns.iter().enumerate() {
                    output.push(' ');
                    let mut cell_found = false;
                    for cell in &row.cells {
                        if cell.col == *col_idx {
                            let display = cell
                                .value
                                .as_ref()
                                .map_or_else(|| "NULL".to_string(), |v| escape_markdown(v));
                            format_cell_to(&display, column_widths[i], &mut output);
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
            let display = value
                .as_ref()
                .map_or_else(|| "NULL".to_string(), |v| escape_markdown(v));
            output.push_str(&display);
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

/// Returns true if the value contains characters that have special meaning
/// in markdown and would alter the rendered output.
fn needs_escaping(value: &str) -> bool {
    value
        .bytes()
        .any(|b| matches!(b, b'*' | b'_' | b'~' | b'`' | b'[' | b']' | b'|' | b'\\'))
}

/// Escapes a value for use in a markdown table cell by wrapping it in
/// backticks if it contains any markdown special characters.
fn escape_markdown(value: &str) -> String {
    if needs_escaping(value) {
        format!("`{}`", value)
    } else {
        value.to_string()
    }
}

/// Format a cell value with proper padding. Writes directly to buffer.
/// This avoids allocating a new String for each cell.
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
    use crate::results_panel::{SelectedCell, SelectedRow, SelectedTableData};

    fn create_test_data() -> SelectedTableData {
        SelectedTableData {
            table_name: Some("users".to_string()),
            db_type: None,
            columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
            selected_rows: vec![
                SelectedRow {
                    row: 0,
                    cells: vec![
                        SelectedCell {
                            row: 0,
                            col: 0,
                            value: Some("1".to_string()),
                            column_name: Some("id".to_string()),
                            column_type: Some(ColumnType::Integer),
                        },
                        SelectedCell {
                            row: 0,
                            col: 1,
                            value: Some("Alice".to_string()),
                            column_name: Some("name".to_string()),
                            column_type: Some(ColumnType::Text),
                        },
                        SelectedCell {
                            row: 0,
                            col: 2,
                            value: Some("alice@example.com".to_string()),
                            column_name: Some("email".to_string()),
                            column_type: Some(ColumnType::Text),
                        },
                    ],
                    primary_key_value: Some("1".to_string()),
                },
                SelectedRow {
                    row: 1,
                    cells: vec![
                        SelectedCell {
                            row: 1,
                            col: 0,
                            value: Some("2".to_string()),
                            column_name: Some("id".to_string()),
                            column_type: Some(ColumnType::Integer),
                        },
                        SelectedCell {
                            row: 1,
                            col: 1,
                            value: None,
                            column_name: Some("name".to_string()),
                            column_type: Some(ColumnType::Text),
                        },
                        SelectedCell {
                            row: 1,
                            col: 2,
                            value: Some("bob@example.com".to_string()),
                            column_name: Some("email".to_string()),
                            column_type: Some(ColumnType::Text),
                        },
                    ],
                    primary_key_value: Some("2".to_string()),
                },
            ],
        }
    }

    #[test]
    fn test_markdown_output_structure() {
        let transformer = MarkdownTransformer;
        let data = create_test_data();
        let result = transformer.transform_selected_data(&data).unwrap();

        assert!(result.contains("# Table: users"));
        assert!(result.contains("| id"));
        assert!(result.contains("| name"));
        assert!(result.contains("| email"));
        // Separator row
        assert!(result.contains("| ---"));
        // Data rows
        assert!(result.contains("Alice"));
        assert!(result.contains("NULL"));
        assert!(result.contains("bob@example.com"));
    }

    #[test]
    fn test_markdown_empty_selection() {
        let transformer = MarkdownTransformer;
        let data = SelectedTableData::default();
        let result = transformer.transform_selected_data(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_escape_markdown_special_chars() {
        assert_eq!(escape_markdown("normal text"), "normal text");
        assert_eq!(escape_markdown("has*stars"), "`has*stars`");
        assert_eq!(escape_markdown("has|pipe"), "`has|pipe`");
        assert_eq!(escape_markdown("has_under"), "`has_under`");
    }

    #[test]
    fn test_markdown_no_streaming() {
        let transformer = MarkdownTransformer;
        assert!(!transformer.supports_streaming());
    }
}
