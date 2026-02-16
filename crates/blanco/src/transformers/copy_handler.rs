#![allow(dead_code)]

use blanco_core::connection_trait::ColumnType;
use gpui::{App, AppContext, ClipboardItem, Context};
use std::sync::Arc;

use crate::results_panel::ResultsPanel;
use crate::transformers::{SelectedTableData, TransformError, TransformerRegistry};

#[derive(Debug, Clone)]
pub enum CopyError {
    TransformError(TransformError),
    ClipboardError(String),
    NoDataSelected,
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::TransformError(e) => write!(f, "Transform error: {}", e),
            CopyError::ClipboardError(e) => write!(f, "Clipboard error: {}", e),
            CopyError::NoDataSelected => write!(f, "No data selected for copying"),
        }
    }
}

impl std::error::Error for CopyError {}

/// Handles copying table data to the clipboard in various formats
#[derive(Clone)]
pub struct CopyHandler {
    registry: Arc<TransformerRegistry>,
}

impl CopyHandler {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(TransformerRegistry::default()),
        }
    }

    pub fn with_registry(registry: TransformerRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }

    /// Get all available format names
    pub fn get_available_formats(&self) -> Vec<String> {
        self.registry
            .get_available_formats()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Get format descriptions
    pub fn get_format_descriptions(&self) -> Vec<(String, &'static str)> {
        let mut descriptions = Vec::new();
        for format in self.registry.get_available_formats() {
            if let Some(transformer) = self.registry.get_transformer(format) {
                descriptions.push((format.to_string(), transformer.description()));
            }
        }
        descriptions
    }

    /// Copy selected data in the specified format asynchronously
    /// This spawns a background task to transform the data, keeping the UI responsive
    pub fn copy_as_format(
        &self,
        data: &SelectedTableData,
        format: &str,
        cx: &mut Context<ResultsPanel>,
    ) {
        if !data.has_selection() {
            tracing::error!("Failed to copy as {}: No data selected for copying", format);
            return;
        }

        // Clone data for background task
        let data_clone = data.clone();
        let format_owned = format.to_string();

        // Clone the Arc for the background task
        let registry = self.registry.clone();

        // Spawn background task for transformation
        let transform_task = cx.background_spawn(async move {
            tracing::debug!("Starting copy transformation");
            let result = registry.transform_data(&data_clone, &format_owned);
            tracing::debug!("Copy transformation completed");
            result
        });

        // Spawn async task to handle the result on the main thread
        cx.spawn(async move |entity, cx| {
            let transformed = match transform_task.await {
                Ok(transformed) => transformed,
                Err(e) => {
                    tracing::error!("Failed to receive copy result: {}", e);
                    return;
                }
            };

            if !transformed.is_empty() {
                let _ = entity.update(cx, |_, cx| {
                    tracing::debug!("Writing transformed data to clipboard");
                    cx.write_to_clipboard(ClipboardItem::new_string(transformed));
                });
            } else {
                tracing::error!("Copy transformation produced empty result");
            }
        })
        .detach();
    }

    /// Copy selected data as CSV (default format)
    pub fn copy_as_csv(&self, data: &SelectedTableData, cx: &mut Context<ResultsPanel>) {
        self.copy_as_format(data, "csv", cx)
    }

    /// Copy selected data as SQL
    pub fn copy_as_sql(&self, data: &SelectedTableData, cx: &mut Context<ResultsPanel>) {
        self.copy_as_format(data, "sql", cx)
    }

    /// Copy selected data as JSON
    pub fn copy_as_json(&self, data: &SelectedTableData, cx: &mut Context<ResultsPanel>) {
        self.copy_as_format(data, "json", cx)
    }

    /// Copy selected data as Markdown
    pub fn copy_as_markdown(&self, data: &SelectedTableData, cx: &mut Context<ResultsPanel>) {
        self.copy_as_format(data, "markdown", cx)
    }

    /// Copy a single cell value (synchronous, for small single-cell operations)
    pub fn copy_single_cell(
        &self,
        value: &str,
        column_type: &ColumnType,
        format: &str,
        cx: &mut App,
    ) -> Result<(), CopyError> {
        let transformed = self
            .registry
            .get_transformer(format)
            .ok_or_else(|| {
                CopyError::TransformError(TransformError::FormatError(format!(
                    "Unknown format: {}",
                    format
                )))
            })?
            .transform_single_cell(value, column_type)
            .map_err(CopyError::TransformError)?;

        cx.write_to_clipboard(ClipboardItem::new_string(transformed));
        Ok(())
    }

    /// Get a preview of the transformed data without copying to clipboard
    pub fn preview_transform(
        &self,
        data: &SelectedTableData,
        format: &str,
        max_chars: usize,
    ) -> Result<String, CopyError> {
        if !data.has_selection() {
            return Err(CopyError::NoDataSelected);
        }

        let transformed = self
            .registry
            .transform_data(data, format)
            .map_err(CopyError::TransformError)?;

        if transformed.len() <= max_chars {
            Ok(transformed)
        } else {
            let preview = transformed.chars().take(max_chars).collect::<String>();
            Ok(format!("{}...", preview))
        }
    }
}

impl Default for CopyHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results_panel::{SelectedCell, SelectedRow, SelectedTableData};

    fn create_test_data() -> SelectedTableData {
        SelectedTableData {
            table_name: Some("users".to_string()),
            columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
            column_types: vec![ColumnType::Integer, ColumnType::Text, ColumnType::Text],
            selected_rows: vec![SelectedRow {
                row: 1,
                cells: vec![
                    SelectedCell {
                        row: 1,
                        col: 0,
                        value: "2".to_string(),
                        column_name: Some("id".to_string()),
                        column_type: Some(ColumnType::Integer),
                    },
                    SelectedCell {
                        row: 1,
                        col: 1,
                        value: "Bob".to_string(),
                        column_name: Some("name".to_string()),
                        column_type: Some(ColumnType::Text),
                    },
                    SelectedCell {
                        row: 1,
                        col: 2,
                        value: "bob@example.com".to_string(),
                        column_name: Some("email".to_string()),
                        column_type: Some(ColumnType::Text),
                    },
                ],
                primary_key_value: Some("2".to_string()),
            }],
        }
    }

    #[test]
    fn test_copy_handler_available_formats() {
        let handler = CopyHandler::new();
        let formats = handler.get_available_formats();

        assert!(formats.contains(&"csv".to_string()));
        assert!(formats.contains(&"sql".to_string()));
        assert!(formats.contains(&"json".to_string()));
        assert!(formats.contains(&"markdown".to_string()));
    }

    #[test]
    fn test_copy_handler_preview() {
        let handler = CopyHandler::new();
        let data = create_test_data();

        let csv_preview = handler.preview_transform(&data, "csv", 50).unwrap();
        assert!(csv_preview.contains("name"));
        assert!(csv_preview.contains("Bob"));

        let json_preview = handler.preview_transform(&data, "json", 100).unwrap();
        assert!(json_preview.contains("\"name\": \"Bob\""));
        assert!(json_preview.contains("\"email\": \"bob@example.com\""));
    }

    #[test]
    fn test_copy_handler_no_selection() {
        let handler = CopyHandler::new();
        let empty_data = SelectedTableData::default();

        let result = handler.preview_transform(&empty_data, "csv", 50);
        assert!(matches!(result, Err(CopyError::NoDataSelected)));
    }

    #[test]
    fn test_copy_handler_unknown_format() {
        let handler = CopyHandler::new();
        let data = create_test_data();

        let result = handler.preview_transform(&data, "unknown_format", 50);
        assert!(matches!(
            result,
            Err(CopyError::TransformError(TransformError::FormatError(_)))
        ));
    }
}
