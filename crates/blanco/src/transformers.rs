use blanco_core::connection_trait::ColumnType;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::results_panel::SelectedTableData;

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
pub mod copy_handler;
pub mod csv_transformer;
pub mod json_transformer;
pub mod markdown_transformer;
pub mod sql_transformer;

// Export types for convenience
pub use copy_handler::CopyHandler;
pub use csv_transformer::CsvTransformer;
pub use json_transformer::JsonTransformer;
pub use markdown_transformer::MarkdownTransformer;
pub use sql_transformer::SqlTransformer;

impl Default for TransformerRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(CsvTransformer);
        registry.register(SqlTransformer::new());
        registry.register(JsonTransformer::new());
        registry.register(MarkdownTransformer);
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
    use crate::results_panel::{SelectedCell, SelectedRow, SelectedTableData};

    fn create_test_data_with_nulls() -> SelectedTableData {
        SelectedTableData {
            table_name: Some("products".to_string()),
            db_type: Some(database::DatabaseType::PostgreSQL),
            columns: vec!["id".to_string(), "name".to_string(), "price".to_string()],
            selected_rows: vec![SelectedRow {
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
                        value: Some("Widget".to_string()),
                        column_name: Some("name".to_string()),
                        column_type: Some(ColumnType::Text),
                    },
                    SelectedCell {
                        row: 0,
                        col: 2,
                        value: None,
                        column_name: Some("price".to_string()),
                        column_type: Some(ColumnType::Numeric),
                    },
                ],
                primary_key_value: Some("1".to_string()),
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

        for format in &["csv", "json", "sql", "markdown"] {
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
        data.db_type = Some(database::DatabaseType::MySQL);

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
                    row: 0,
                    col: 0,
                    value: Some("has;semicolon and \"quotes\"".to_string()),
                    column_name: Some("value".to_string()),
                    column_type: Some(ColumnType::Text),
                }],
                primary_key_value: None,
            }],
        };

        let registry = TransformerRegistry::default();
        let csv = registry.transform_data(&data, "csv").unwrap();
        assert!(csv.contains("\"has;semicolon and \"\"quotes\"\"\""));
    }
}
