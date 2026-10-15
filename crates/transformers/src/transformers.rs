use blanco_core::{ColumnType, DatabaseType};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Selection data handed to transformers by the results grid.
#[derive(Clone, Debug)]
pub struct SelectedCell {
    pub col: usize,
    pub value: Option<String>,
    pub column_name: Option<String>,
    pub column_type: Option<ColumnType>,
}

#[derive(Clone, Debug)]
pub struct SelectedRow {
    pub row: usize,
    pub cells: Vec<SelectedCell>,
}

#[derive(Clone, Debug, Default)]
pub struct SelectedTableData {
    pub table_name: Option<String>,
    pub db_type: Option<DatabaseType>,
    pub columns: Vec<String>,
    pub selected_rows: Vec<SelectedRow>,
}

#[derive(Debug, Clone)]
pub enum TransformError {
    FormatError(String),
    EmptySelection,
}

impl fmt::Display for TransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransformError::FormatError(msg) => write!(f, "Format error: {}", msg),
            TransformError::EmptySelection => write!(f, "No data selected for copying"),
        }
    }
}

impl std::error::Error for TransformError {}

/// How the bytes of a binary column are written to text formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BinaryEncoding {
    /// `0x` followed by hex digits, as the grid shows them.
    #[default]
    Hex,
    /// Base64 with the standard alphabet and padding.
    Base64,
}

impl BinaryEncoding {
    pub const ALL: [BinaryEncoding; 2] = [BinaryEncoding::Hex, BinaryEncoding::Base64];

    pub fn label(self) -> &'static str {
        match self {
            BinaryEncoding::Hex => "Hex",
            BinaryEncoding::Base64 => "Base64",
        }
    }

    /// The text written for a cell of `column_type`. Only the hex notation
    /// of a binary column is re-encoded; anything else is written as it is.
    pub fn apply<'a>(
        self,
        value: &'a str,
        column_type: Option<&ColumnType>,
    ) -> std::borrow::Cow<'a, str> {
        if self == BinaryEncoding::Base64
            && column_type == Some(&ColumnType::Binary)
            && let Some(bytes) = blanco_core::binary::decode(value)
        {
            return blanco_core::binary::to_base64(&bytes).into();
        }
        value.into()
    }
}

/// Core trait for data transformers that convert table data to different formats
pub trait DataTransformer: Send + Sync {
    /// Returns the display name of this format
    fn format_name(&self) -> &'static str;

    /// Transforms selected table data to the format
    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError>;

    // === Streaming Methods ===

    /// Initialize streaming transformation with headers and return initial output
    /// This is called once at the beginning of the streaming process
    fn initialize_stream(
        &self,
        columns: &[String],
        _column_types: &[ColumnType],
    ) -> Result<String, TransformError> {
        self.transform_header_row(columns)
    }

    /// Transform a single row of data during streaming
    /// This is called for each row as it's processed
    fn transform_stream_row(
        &self,
        row_data: &[Option<String>],
        columns: &[String],
        column_types: &[ColumnType],
    ) -> Result<String, TransformError>;

    /// Finalize streaming transformation and return any trailing output
    /// This is called once at the end of the streaming process
    fn finalize_stream(&self) -> Result<String, TransformError> {
        Ok(String::new())
    }

    /// Transform header row specifically (helper method)
    fn transform_header_row(&self, columns: &[String]) -> Result<String, TransformError> {
        let header_values: Vec<Option<String>> = columns.iter().map(|c| Some(c.clone())).collect();
        self.transform_stream_row(&header_values, columns, &[])
    }

    /// Check if this transformer supports streaming
    /// Most transformers can support streaming, but some might need all data at once
    fn supports_streaming(&self) -> bool {
        true
    }
}

/// Registry for managing available data transformers
#[derive(Clone)]
pub struct TransformerRegistry {
    transformers: HashMap<String, Arc<dyn DataTransformer>>,
}

impl TransformerRegistry {
    pub fn new() -> Self {
        Self {
            transformers: HashMap::new(),
        }
    }

    pub fn register<T: DataTransformer + 'static>(&mut self, transformer: T) {
        self.transformers.insert(
            transformer.format_name().to_lowercase(),
            Arc::new(transformer),
        );
    }

    pub fn get_transformer(&self, format_name: &str) -> Option<Arc<dyn DataTransformer>> {
        self.transformers.get(&format_name.to_lowercase()).cloned()
    }

    pub fn transform_data(
        &self,
        data: &SelectedTableData,
        format_name: &str,
    ) -> Result<String, TransformError> {
        let transformer = self.get_transformer(format_name).ok_or_else(|| {
            TransformError::FormatError(format!("Unknown format: {}", format_name))
        })?;

        transformer.transform_selected_data(data)
    }
}

// Re-export transformer modules and types
pub mod csv_transformer;
pub mod encoding;
pub mod html_transformer;
pub mod json_transformer;
pub mod markdown_transformer;
pub mod sql_transformer;
pub mod tsv_transformer;
pub mod values_transformer;

// Export types for convenience
pub use csv_transformer::{CsvOptions, CsvQuoting, CsvTransformer};
pub use encoding::TextEncoding;
pub use html_transformer::HtmlTransformer;
pub use json_transformer::JsonTransformer;
pub use markdown_transformer::MarkdownTransformer;
pub use sql_transformer::SqlTransformer;
pub use tsv_transformer::TsvTransformer;
pub use values_transformer::ValuesTransformer;

impl Default for TransformerRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(CsvTransformer::default());
        registry.register(TsvTransformer);
        registry.register(SqlTransformer::new());
        registry.register(ValuesTransformer::new());
        registry.register(JsonTransformer::new());
        registry.register(MarkdownTransformer);
        registry.register(HtmlTransformer);
        registry
    }
}

impl SelectedTableData {
    /// Check if there's any data selected
    pub fn has_selection(&self) -> bool {
        !self.selected_rows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SelectedCell, SelectedRow, SelectedTableData};

    fn binary_data(db_type: blanco_core::DatabaseType) -> SelectedTableData {
        let cell = |col, name: &str, value: &str, column_type| SelectedCell {
            col,
            value: Some(value.to_string()),
            column_name: Some(name.to_string()),
            column_type: Some(column_type),
        };
        SelectedTableData {
            table_name: Some("files".to_string()),
            db_type: Some(db_type),
            columns: vec!["name".to_string(), "body".to_string()],
            selected_rows: vec![SelectedRow {
                row: 0,
                cells: vec![
                    cell(0, "name", "0x666f6f", ColumnType::Text),
                    cell(1, "body", "0x666f6f", ColumnType::Binary),
                ],
            }],
        }
    }

    #[test]
    fn binary_columns_are_written_as_hex_or_base64() {
        let data = binary_data(blanco_core::DatabaseType::PostgreSQL);
        // Only the binary column is re-encoded: the same text in a text
        // column is text.
        let csv = CsvTransformer::default()
            .binary_encoding(BinaryEncoding::Base64)
            .transform_selected_data(&data)
            .expect("csv");
        assert_eq!(csv, "name;body\n0x666f6f;Zm9v\n");
        let csv = CsvTransformer::default()
            .transform_selected_data(&data)
            .expect("csv");
        assert_eq!(csv, "name;body\n0x666f6f;0x666f6f\n");

        let types = [ColumnType::Text, ColumnType::Binary];
        let row = [Some("0x666f6f".to_string()), Some("0x666f6f".to_string())];
        let streamed = CsvTransformer::default()
            .binary_encoding(BinaryEncoding::Base64)
            .transform_stream_row(&row, &data.columns, &types)
            .expect("row");
        assert_eq!(streamed, "0x666f6f;Zm9v\n");

        let json = JsonTransformer::new()
            .binary_encoding(BinaryEncoding::Base64)
            .transform_selected_data(&data)
            .expect("json");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed[0]["body"], "Zm9v");
        assert_eq!(parsed[0]["name"], "0x666f6f");
        // Hex digits that read like a number stay a string.
        let numeric = [Some("x".to_string()), Some("0x1234".to_string())];
        let json = JsonTransformer::new()
            .transform_stream_row(&numeric, &data.columns, &types)
            .expect("row");
        assert!(json.contains("\"0x1234\""), "{json}");
    }

    #[test]
    fn sql_formats_write_binary_literals() {
        use blanco_core::DatabaseType;
        let sql = |db_type| {
            SqlTransformer::new()
                .transform_selected_data(&binary_data(db_type))
                .expect("sql")
        };
        assert!(sql(DatabaseType::PostgreSQL).contains(r"('0x666f6f', '\x666f6f'::bytea)"));
        assert!(sql(DatabaseType::MySQL).contains("('0x666f6f', X'666f6f')"));
        assert!(sql(DatabaseType::MsSql).contains("('0x666f6f', 0x666f6f)"));
        let values = ValuesTransformer::new()
            .transform_selected_data(&binary_data(DatabaseType::SQLite))
            .expect("values");
        assert!(values.contains("('0x666f6f', X'666f6f')"), "{values}");
    }

    fn create_test_data_with_nulls() -> SelectedTableData {
        SelectedTableData {
            table_name: Some("products".to_string()),
            db_type: Some(blanco_core::DatabaseType::PostgreSQL),
            columns: vec!["id".to_string(), "name".to_string(), "price".to_string()],
            selected_rows: vec![SelectedRow {
                row: 0,
                cells: vec![
                    SelectedCell {
                        col: 0,
                        value: Some("1".to_string()),
                        column_name: Some("id".to_string()),
                        column_type: Some(ColumnType::Integer),
                    },
                    SelectedCell {
                        col: 1,
                        value: Some("Widget".to_string()),
                        column_name: Some("name".to_string()),
                        column_type: Some(ColumnType::Text),
                    },
                    SelectedCell {
                        col: 2,
                        value: None,
                        column_name: Some("price".to_string()),
                        column_type: Some(ColumnType::Numeric),
                    },
                ],
            }],
        }
    }

    #[test]
    fn test_all_formats_handle_nulls() {
        let registry = TransformerRegistry::default();
        let data = create_test_data_with_nulls();

        let csv = registry.transform_data(&data, "csv").unwrap();
        assert!(csv.contains("Widget"));

        let json = registry.transform_data(&data, "json").unwrap();
        assert!(json.contains("null"));
        assert!(json.contains("\"name\": \"Widget\""));

        let sql = registry.transform_data(&data, "sql").unwrap();
        assert!(sql.contains("NULL"));
        assert!(sql.contains("'Widget'"));

        let markdown = registry.transform_data(&data, "markdown").unwrap();
        assert!(markdown.contains("NULL"));
        assert!(markdown.contains("Widget"));
    }

    #[test]
    fn test_all_formats_reject_empty_data() {
        let registry = TransformerRegistry::default();
        let data = SelectedTableData::default();

        for format in &["csv", "json", "sql", "values", "tsv", "markdown"] {
            let result = registry.transform_data(&data, format);
            assert!(
                result.is_err(),
                "Format '{}' should reject empty data",
                format
            );
        }
    }

    #[test]
    fn test_sql_output_uses_correct_quoting_for_db_type() {
        let mut data = create_test_data_with_nulls();
        data.db_type = Some(blanco_core::DatabaseType::MySQL);

        let registry = TransformerRegistry::default();
        let sql = registry.transform_data(&data, "sql").unwrap();

        // MySQL uses backtick quoting
        assert!(sql.contains("`products`"));
        assert!(sql.contains("`id`"));
    }

    #[test]
    fn test_csv_escapes_semicolons_and_quotes() {
        let data = SelectedTableData {
            table_name: None,
            db_type: None,
            columns: vec!["value".to_string()],
            selected_rows: vec![SelectedRow {
                row: 0,
                cells: vec![SelectedCell {
                    col: 0,
                    value: Some("has;semicolon and \"quotes\"".to_string()),
                    column_name: Some("value".to_string()),
                    column_type: Some(ColumnType::Text),
                }],
            }],
        };

        let registry = TransformerRegistry::default();
        let csv = registry.transform_data(&data, "csv").unwrap();
        assert!(csv.contains("\"has;semicolon and \"\"quotes\"\"\""));
    }
}
