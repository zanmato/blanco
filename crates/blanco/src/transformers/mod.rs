use std::collections::HashMap;
use std::fmt;

use crate::results_panel::{SelectedTableData, SelectedCell, SelectedRow};

#[derive(Debug, Clone)]
pub enum TransformError {
    InvalidData(String),
    MissingTableInfo(String),
    FormatError(String),
    EmptySelection,
}

impl fmt::Display for TransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransformError::InvalidData(msg) => write!(f, "Invalid data: {}", msg),
            TransformError::MissingTableInfo(msg) => write!(f, "Missing table info: {}", msg),
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

    /// Returns the file extension for this format
    fn file_extension(&self) -> &'static str;

    /// Transforms selected table data to the format
    fn transform_selected_data(&self, data: &SelectedTableData) -> Result<String, TransformError>;

    /// Transforms a single cell value (for simple copy operations)
    fn transform_single_cell(&self, value: &str, column_type: &str) -> Result<String, TransformError> {
        Ok(value.to_string())
    }

    /// Returns a description of what this transformer does
    fn description(&self) -> &'static str;
}

/// Registry for managing available data transformers
pub struct TransformerRegistry {
    transformers: HashMap<String, Box<dyn DataTransformer>>,
}

impl TransformerRegistry {
    pub fn new() -> Self {
        Self {
            transformers: HashMap::new(),
        }
    }

    pub fn register<T: DataTransformer + 'static>(&mut self, transformer: T) {
        self.transformers.insert(transformer.format_name().to_lowercase(), Box::new(transformer));
    }

    pub fn get_transformer(&self, format_name: &str) -> Option<&dyn DataTransformer> {
        self.transformers.get(&format_name.to_lowercase()).map(|t| t.as_ref())
    }

    pub fn get_available_formats(&self) -> Vec<&str> {
        self.transformers
            .keys()
            .map(|k| k.as_str())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn transform_data(&self, data: &SelectedTableData, format_name: &str) -> Result<String, TransformError> {
        let transformer = self.get_transformer(format_name)
            .ok_or_else(|| TransformError::FormatError(format!("Unknown format: {}", format_name)))?;

        transformer.transform_selected_data(data)
    }
}

// Re-export transformer modules and types
pub mod csv_transformer;
pub mod sql_transformer;
pub mod json_transformer;
pub mod markdown_transformer;
pub mod copy_handler;

// Export types for convenience
pub use csv_transformer::CsvTransformer;
pub use sql_transformer::SqlTransformer;
pub use json_transformer::JsonTransformer;
pub use markdown_transformer::MarkdownTransformer;
pub use copy_handler::{CopyHandler, CopyError};

impl Default for TransformerRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        // Register built-in transformers
        registry.register(CsvTransformer);
        registry.register(SqlTransformer);
        registry.register(JsonTransformer);
        registry.register(MarkdownTransformer);
        registry
    }
}

impl SelectedTableData {
    /// Check if there's any data selected
    pub fn has_selection(&self) -> bool {
        !self.selected_cells.is_empty() || !self.selected_rows.is_empty()
    }

    /// Get all selected data as a flat collection of values
    pub fn get_all_selected_values(&self) -> Vec<String> {
        let mut values = Vec::new();

        // Add individual cell values
        for cell in &self.selected_cells {
            values.push(cell.value.clone());
        }

        // Add row cell values
        for row in &self.selected_rows {
            for cell in &row.cells {
                values.push(cell.value.clone());
            }
        }

        values
    }

    /// Get the effective column names (excluding row number column)
    pub fn get_effective_columns(&self) -> &[String] {
        &self.columns
    }

    /// Get the effective column types (excluding row number column)
    pub fn get_effective_column_types(&self) -> &[String] {
        &self.column_types
    }
}