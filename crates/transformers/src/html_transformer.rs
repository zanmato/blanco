use crate::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::ColumnType;

/// Writes rows as an HTML `<table>` with a header row, for pasting into an
/// email or a document.
pub struct HtmlTransformer;

impl DataTransformer for HtmlTransformer {
    fn format_name(&self) -> &'static str {
        "HTML"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        // The selection may cover only some columns; the table spans the
        // range between the leftmost and rightmost selected one.
        let (min_col, max_col) = data
            .selected_rows
            .iter()
            .flat_map(|row| &row.cells)
            .fold(None, |range: Option<(usize, usize)>, cell| {
                Some(match range {
                    Some((min, max)) => (min.min(cell.col), max.max(cell.col)),
                    None => (cell.col, cell.col),
                })
            })
            .unwrap_or((0, data.columns.len().saturating_sub(1)));
        let columns: Vec<(usize, &String)> = (min_col..=max_col)
            .filter_map(|col| data.columns.get(col).map(|name| (col, name)))
            .collect();

        let mut output = String::from("<table>\n");
        if let Some(table_name) = &data.table_name {
            output.push_str("<caption>");
            push_escaped(table_name, &mut output);
            output.push_str("</caption>\n");
        }
        push_header(columns.iter().map(|(_, name)| name.as_str()), &mut output);
        for row in &data.selected_rows {
            output.push_str("<tr>");
            for (col, _) in &columns {
                let value = row
                    .cells
                    .iter()
                    .find(|cell| cell.col == *col)
                    .map(|cell| cell.value.as_deref());
                push_cell(value, &mut output);
            }
            output.push_str("</tr>\n");
        }
        output.push_str("</tbody>\n</table>\n");
        Ok(output)
    }

    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let mut output = String::from("<table>\n");
        push_header(columns.iter().map(String::as_str), &mut output);
        Ok(output)
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        _columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let mut output = String::from("<tr>");
        for value in row_data {
            push_cell(Some(value.as_deref()), &mut output);
        }
        output.push_str("</tr>\n");
        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        Ok("</tbody>\n</table>\n".to_string())
    }
}

/// The header row, and the body opened after it.
fn push_header<'a>(columns: impl Iterator<Item = &'a str>, output: &mut String) {
    output.push_str("<thead>\n<tr>");
    for column in columns {
        output.push_str("<th>");
        push_escaped(column, output);
        output.push_str("</th>");
    }
    output.push_str("</tr>\n</thead>\n<tbody>\n");
}

/// One cell: `None` for a column the row has no selected cell in, which
/// stays empty, and `Some(None)` for SQL NULL.
fn push_cell(value: Option<Option<&str>>, output: &mut String) {
    output.push_str("<td>");
    match value {
        Some(Some(value)) => push_escaped(value, output),
        Some(None) => output.push_str("NULL"),
        None => {}
    }
    output.push_str("</td>");
}

fn push_escaped(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            character => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SelectedCell, SelectedRow};

    fn cell(col: usize, value: Option<&str>) -> SelectedCell {
        SelectedCell {
            col,
            value: value.map(str::to_string),
            column_name: None,
            column_type: Some(ColumnType::Text),
        }
    }

    #[test]
    fn selected_rows_become_an_escaped_table() {
        let data = SelectedTableData {
            table_name: Some("users & co".to_string()),
            db_type: None,
            columns: vec!["id".to_string(), "<name>".to_string(), "note".to_string()],
            selected_rows: vec![
                SelectedRow {
                    row: 0,
                    cells: vec![
                        cell(0, Some("1")),
                        cell(1, Some("Ada \"<b>\"")),
                        cell(2, Some("it's")),
                    ],
                },
                SelectedRow {
                    row: 1,
                    cells: vec![cell(0, Some("2")), cell(1, None)],
                },
            ],
        };
        let html = HtmlTransformer.transform_selected_data(&data).unwrap();
        assert_eq!(
            html,
            "<table>\n\
             <caption>users &amp; co</caption>\n\
             <thead>\n<tr><th>id</th><th>&lt;name&gt;</th><th>note</th></tr>\n</thead>\n\
             <tbody>\n\
             <tr><td>1</td><td>Ada &quot;&lt;b&gt;&quot;</td><td>it&#39;s</td></tr>\n\
             <tr><td>2</td><td>NULL</td><td></td></tr>\n\
             </tbody>\n</table>\n"
        );
    }

    #[test]
    fn only_the_selected_column_range_is_written() {
        let data = SelectedTableData {
            table_name: None,
            db_type: None,
            columns: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            selected_rows: vec![SelectedRow {
                row: 0,
                cells: vec![cell(1, Some("x"))],
            }],
        };
        let html = HtmlTransformer.transform_selected_data(&data).unwrap();
        assert_eq!(
            html,
            "<table>\n<thead>\n<tr><th>b</th></tr>\n</thead>\n<tbody>\n\
             <tr><td>x</td></tr>\n</tbody>\n</table>\n"
        );
    }

    #[test]
    fn empty_selection_is_an_error() {
        assert!(
            HtmlTransformer
                .transform_selected_data(&SelectedTableData::default())
                .is_err()
        );
    }

    #[test]
    fn streaming_writes_the_same_table() {
        let columns = vec!["id".to_string(), "name".to_string()];
        let mut html = HtmlTransformer.initialize_stream(&columns, &[]).unwrap();
        html.push_str(
            &HtmlTransformer
                .transform_stream_row(&[Some("1".into()), None], &columns, &[])
                .unwrap(),
        );
        html.push_str(&HtmlTransformer.finalize_stream().unwrap());
        assert_eq!(
            html,
            "<table>\n<thead>\n<tr><th>id</th><th>name</th></tr>\n</thead>\n<tbody>\n\
             <tr><td>1</td><td>NULL</td></tr>\n</tbody>\n</table>\n"
        );
    }
}
