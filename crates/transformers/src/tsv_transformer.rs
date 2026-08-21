use crate::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::ColumnType;

pub struct TsvTransformer;

impl DataTransformer for TsvTransformer {
    fn format_name(&self) -> &'static str {
        "TSV"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        let cell_count = data
            .selected_rows
            .iter()
            .map(|r| r.cells.len())
            .sum::<usize>();
        let estimated_capacity = (cell_count * 50) + (data.columns.len() * 20) + 100;
        let mut output = String::with_capacity(estimated_capacity);

        if !data.selected_rows.is_empty() {
            if !data.columns.is_empty() {
                for (i, col) in data.columns.iter().enumerate() {
                    if i > 0 {
                        output.push('\t');
                    }
                    tsv_escape_to(col, &mut output);
                }
                output.push('\n');
            }

            let mut row_indices: Vec<usize> = data.selected_rows.iter().map(|r| r.row).collect();
            row_indices.sort();

            for row_idx in row_indices {
                if let Some(row) = data.selected_rows.iter().find(|r| r.row == row_idx) {
                    for (i, cell) in row.cells.iter().enumerate() {
                        if i > 0 {
                            output.push('\t');
                        }
                        tsv_escape_to(cell.value.as_deref().unwrap_or(""), &mut output);
                    }
                    output.push('\n');
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
        let estimated_capacity = columns.len() * 20 + 10;
        let mut output = String::with_capacity(estimated_capacity);

        if !columns.is_empty() {
            for (i, col) in columns.iter().enumerate() {
                if i > 0 {
                    output.push('\t');
                }
                tsv_escape_to(col, &mut output);
            }
            output.push('\n');
        }

        Ok(output)
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        _columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let estimated_capacity = row_data.len() * 50 + 10;
        let mut output = String::with_capacity(estimated_capacity);

        for (i, value) in row_data.iter().enumerate() {
            if i > 0 {
                output.push('\t');
            }
            tsv_escape_to(value.as_deref().unwrap_or(""), &mut output);
        }
        output.push('\n');

        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        Ok(String::new())
    }
}

fn tsv_escape_to(value: &str, output: &mut String) {
    if value.is_empty() {
        return;
    }

    let needs_quoting =
        value.contains('\t') || value.contains('"') || value.contains('\n') || value.contains('\r');

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

    fn escape(value: &str) -> String {
        let mut output = String::new();
        tsv_escape_to(value, &mut output);
        output
    }

    #[test]
    fn test_tsv_escape() {
        assert_eq!(escape("simple"), "simple");
        assert_eq!(escape("contains\ttab"), "\"contains\ttab\"");
        assert_eq!(escape("contains\"quote"), "\"contains\"\"quote\"");
        assert_eq!(escape("multi\nline"), "\"multi\nline\"");
        assert_eq!(escape(""), "");
        assert_eq!(
            escape("UPDATE users SET name = 'Bob';"),
            "UPDATE users SET name = 'Bob';"
        );
    }
}
