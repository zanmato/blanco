use crate::{DataTransformer, SelectedTableData, TransformError};
use blanco_core::ColumnType;
use serde::{Deserialize, Serialize};

/// When a CSV value is wrapped in the quote character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CsvQuoting {
    /// Every value and header. NULL is still written bare, which tells it
    /// apart from an empty string.
    Always,
    /// Values holding the delimiter, the quote character or a line break.
    #[default]
    WhenNeeded,
    /// Never. Values are written as they are, so a delimiter inside one
    /// splits it.
    Never,
}

impl CsvQuoting {
    pub const ALL: [CsvQuoting; 3] = [
        CsvQuoting::Always,
        CsvQuoting::WhenNeeded,
        CsvQuoting::Never,
    ];

    /// The stored setting value.
    pub fn as_str(self) -> &'static str {
        match self {
            CsvQuoting::Always => "always",
            CsvQuoting::WhenNeeded => "when_needed",
            CsvQuoting::Never => "never",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CsvQuoting::Always => "Always",
            CsvQuoting::WhenNeeded => "When needed",
            CsvQuoting::Never => "Never",
        }
    }

    /// Parse a stored value, falling back to the default for anything unknown.
    pub fn from_stored(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|quoting| quoting.as_str() == value)
            .unwrap_or_default()
    }
}

/// How CSV output is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvOptions {
    pub delimiter: char,
    pub header: bool,
    /// Written for a NULL value, as it is and never quoted.
    pub null_text: String,
    pub quote: char,
    pub quoting: CsvQuoting,
    /// Written in place of each line break inside a value, `None` to keep
    /// line breaks. A `\r\n` pair counts as one line break.
    pub line_break_replacement: Option<String>,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: ';',
            header: true,
            null_text: String::new(),
            quote: '"',
            quoting: CsvQuoting::WhenNeeded,
            line_break_replacement: None,
        }
    }
}

#[derive(Default)]
pub struct CsvTransformer {
    options: CsvOptions,
}

impl CsvTransformer {
    pub fn new(options: CsvOptions) -> Self {
        Self { options }
    }

    fn header_line(&self, columns: &[String]) -> String {
        if !self.options.header || columns.is_empty() {
            return String::new();
        }
        let mut output = String::with_capacity(columns.len() * 20 + 10);
        for (i, column) in columns.iter().enumerate() {
            if i > 0 {
                output.push(self.options.delimiter);
            }
            self.write_value(column, &mut output);
        }
        output.push('\n');
        output
    }

    fn write_row<'a>(&self, values: impl Iterator<Item = Option<&'a str>>, output: &mut String) {
        for (i, value) in values.enumerate() {
            if i > 0 {
                output.push(self.options.delimiter);
            }
            match value {
                Some(value) => self.write_value(value, output),
                None => output.push_str(&self.options.null_text),
            }
        }
        output.push('\n');
    }

    /// Escape a value for CSV format. Writes directly to buffer.
    /// This avoids allocating a new String for each cell value
    fn write_value(&self, value: &str, output: &mut String) {
        let options = &self.options;
        let replaced;
        let value = match &options.line_break_replacement {
            Some(replacement) if value.contains(['\n', '\r']) => {
                replaced = value
                    .replace("\r\n", "\n")
                    .replace(['\r', '\n'], replacement);
                replaced.as_str()
            }
            _ => value,
        };

        let quote = match options.quoting {
            CsvQuoting::Always => true,
            CsvQuoting::Never => false,
            CsvQuoting::WhenNeeded => value.contains(|c: char| {
                c == options.delimiter || c == options.quote || c == '\n' || c == '\r'
            }),
        };

        if !quote {
            output.push_str(value);
            return;
        }
        output.push(options.quote);
        for c in value.chars() {
            if c == options.quote {
                output.push(options.quote);
            }
            output.push(c);
        }
        output.push(options.quote);
    }
}

impl DataTransformer for CsvTransformer {
    fn format_name(&self) -> &'static str {
        "CSV"
    }

    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError> {
        if !data.has_selection() {
            return Err(TransformError::EmptySelection);
        }

        // Estimate capacity: ~50 bytes per cell on average
        let cell_count = data
            .selected_rows
            .iter()
            .map(|r| r.cells.len())
            .sum::<usize>();
        let estimated_capacity = (cell_count * 50) + (data.columns.len() * 20) + 100;
        let mut output = String::with_capacity(estimated_capacity);

        output.push_str(&self.header_line(&data.columns));

        let mut rows: Vec<_> = data.selected_rows.iter().collect();
        rows.sort_by_key(|row| row.row);
        for row in rows {
            self.write_row(
                row.cells.iter().map(|cell| cell.value.as_deref()),
                &mut output,
            );
        }

        Ok(output)
    }

    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        Ok(self.header_line(columns))
    }

    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        _columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        let mut output = String::with_capacity(row_data.len() * 50 + 10);
        self.write_row(row_data.iter().map(|value| value.as_deref()), &mut output);
        Ok(output)
    }

    fn finalize_stream(&self) -> Result<String, TransformError> {
        // CSV doesn't need any special finalization
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escape(value: &str) -> String {
        escape_with(value, CsvOptions::default())
    }

    fn escape_with(value: &str, options: CsvOptions) -> String {
        let mut output = String::new();
        CsvTransformer::new(options).write_value(value, &mut output);
        output
    }

    fn rows(options: CsvOptions) -> String {
        let transformer = CsvTransformer::new(options);
        let columns = ["id".to_string(), "note".to_string()];
        let mut output = transformer.initialize_stream(&columns, &[]).unwrap();
        for row in [
            vec![Some("1".to_string()), Some("a,b".to_string())],
            vec![Some("2".to_string()), None],
        ] {
            output.push_str(
                &transformer
                    .transform_stream_row(&row, &columns, &[])
                    .unwrap(),
            );
        }
        output
    }

    #[test]
    fn test_csv_options() {
        assert_eq!(rows(CsvOptions::default()), "id;note\n1;a,b\n2;\n");

        let comma = CsvOptions {
            delimiter: ',',
            header: false,
            null_text: "NULL".into(),
            ..CsvOptions::default()
        };
        assert_eq!(rows(comma.clone()), "1,\"a,b\"\n2,NULL\n");

        let always = CsvOptions {
            quoting: CsvQuoting::Always,
            quote: '\'',
            ..comma.clone()
        };
        assert_eq!(rows(always), "'1','a,b'\n'2',NULL\n");

        let never = CsvOptions {
            quoting: CsvQuoting::Never,
            ..comma
        };
        assert_eq!(rows(never), "1,a,b\n2,NULL\n");
    }

    #[test]
    fn test_line_break_replacement() {
        let options = CsvOptions {
            line_break_replacement: Some(" ".into()),
            ..CsvOptions::default()
        };
        assert_eq!(escape_with("one\r\ntwo\nthree", options), "one two three");
        assert_eq!(escape("one\ntwo"), "\"one\ntwo\"");
    }

    #[test]
    fn test_csv_escape() {
        assert_eq!(escape("simple"), "simple");
        assert_eq!(escape("contains; semicolon"), "\"contains; semicolon\"");
        assert_eq!(escape("contains\"quote"), "\"contains\"\"quote\"");
        assert_eq!(escape("multi\nline"), "\"multi\nline\"");
        assert_eq!(escape(""), "");
    }
}
