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
        // Default implementation - transformers can override this
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
        self.transformers
            .get(&format_name.to_lowercase())
            .cloned()
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
