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

            // Sorted once: looking each row up by its index made a large
            // selection take time quadratic in its size.
            let mut rows: Vec<_> = data.selected_rows.iter().collect();
            rows.sort_by_key(|row| row.row);
            for row in rows {
                for (i, cell) in row.cells.iter().enumerate() {
                    if i > 0 {
                        output.push('\t');
                    }
                    tsv_escape_to(cell.value.as_deref().unwrap_or(""), &mut output);
                }
                output.push('\n');
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
    use crate::{SelectedCell, SelectedRow};

    fn selection(rows: Vec<(usize, Vec<Option<&str>>)>, columns: &[&str]) -> SelectedTableData {
        SelectedTableData {
            columns: columns.iter().map(|column| column.to_string()).collect(),
            selected_rows: rows
                .into_iter()
                .map(|(row, values)| SelectedRow {
                    row,
                    cells: values
                        .into_iter()
                        .enumerate()
                        .map(|(col, value)| SelectedCell {
                            col,
                            value: value.map(str::to_string),
                            column_name: None,
                            column_type: None,
                        })
                        .collect(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Reads TSV the way a spreadsheet does: a field in double quotes may
    /// hold tabs, line breaks and doubled quotes.
    fn parse(text: &str) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut field = String::new();
        let mut quoted = false;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' if quoted => quoted = false,
                '"' if field.is_empty() => quoted = true,
                '\t' if !quoted => row.push(std::mem::take(&mut field)),
                '\n' if !quoted => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                c => field.push(c),
            }
        }
        rows
    }

    const AWKWARD: [&str; 9] = [
        "plain",
        "has\ttab",
        "two\nlines",
        "windows\r\nbreak",
        "say \"hi\"",
        "\"quoted from the start\"",
        "tab\tand\nbreak and \"quote\"",
        " padded ",
        "åäö 表",
    ];

    #[test]
    fn copied_values_with_tabs_breaks_and_quotes_read_back() {
        let data = selection(
            AWKWARD
                .iter()
                .enumerate()
                .map(|(row, value)| (row, vec![Some("before"), Some(*value), None, Some("after")]))
                .collect(),
            &["a", "tab\tin name", "c", "d"],
        );
        let parsed = parse(&TsvTransformer.transform_selected_data(&data).unwrap());
        assert_eq!(parsed[0], ["a", "tab\tin name", "c", "d"]);
        assert_eq!(parsed.len(), AWKWARD.len() + 1);
        for (value, row) in AWKWARD.iter().zip(&parsed[1..]) {
            assert_eq!(row, &["before", value, "", "after"], "{value:?}");
        }
    }

    #[test]
    fn exported_values_with_tabs_breaks_and_quotes_read_back() {
        let transformer = TsvTransformer;
        let columns = vec!["a".to_string(), "b".to_string()];
        let mut output = transformer.initialize_stream(&columns, &[]).unwrap();
        for value in AWKWARD {
            let row = [Some(value.to_string()), None];
            output.push_str(
                &transformer
                    .transform_stream_row(&row, &columns, &[])
                    .unwrap(),
            );
        }
        output.push_str(&transformer.finalize_stream().unwrap());
        let parsed = parse(&output);
        assert_eq!(parsed.len(), AWKWARD.len() + 1);
        for (value, row) in AWKWARD.iter().zip(&parsed[1..]) {
            assert_eq!(row, &[value, ""], "{value:?}");
        }
    }

    #[test]
    fn rows_come_out_in_grid_order_however_large_the_selection() {
        // Handed over back to front, as a set of selected rows may be.
        let count = 60_000;
        let data = selection(
            (0..count).rev().map(|row| (row, vec![Some("x")])).collect(),
            &["n"],
        );
        let mut data = data;
        for row in &mut data.selected_rows {
            row.cells[0].value = Some(row.row.to_string());
        }
        let output = TsvTransformer.transform_selected_data(&data).unwrap();
        let mut lines = output.lines();
        assert_eq!(lines.next(), Some("n"));
        for (expected, line) in (0..count).zip(lines) {
            assert_eq!(line, expected.to_string());
        }
        assert_eq!(output.lines().count(), count + 1);
    }

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
