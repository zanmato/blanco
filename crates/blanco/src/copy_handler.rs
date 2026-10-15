use gpui::{AppContext, ClipboardItem, Context, Window};
use std::sync::Arc;

use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;
use transformers::{CsvTransformer, DataTransformer, SelectedTableData, TransformerRegistry};

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

    /// Copy selected data in the specified format asynchronously
    /// This spawns a background task to transform the data, keeping the UI responsive
    pub fn copy_as_format(
        &self,
        data: &SelectedTableData,
        format: &str,
        window: &mut Window,
        cx: &mut Context<ResultsPanel>,
    ) {
        if !data.has_selection() {
            tracing::error!("Failed to copy as {}: No data selected for copying", format);
            return;
        }

        // CSV follows the options in settings, the other formats are fixed.
        let transformer: Arc<dyn DataTransformer> = if format.eq_ignore_ascii_case("csv") {
            Arc::new(CsvTransformer::new(
                AppSettings::global(cx).settings.csv.to_options(),
            ))
        } else if let Some(transformer) = self.registry.get_transformer(format) {
            transformer
        } else {
            tracing::error!("Failed to copy: unknown format {}", format);
            return;
        };
        let is_html = format.eq_ignore_ascii_case("html");
        let data_clone = data.clone();

        let transform_task = cx.background_spawn(async move {
            tracing::debug!("Starting copy transformation");
            let result = transformer.transform_selected_data(&data_clone);
            tracing::debug!("Copy transformation completed");
            result
        });

        cx.spawn_in(window, async move |entity, cx| {
            let transformed = match transform_task.await {
                Ok(transformed) => transformed,
                Err(e) => {
                    tracing::error!("Failed to receive copy result: {}", e);
                    return;
                }
            };

            if !transformed.is_empty() {
                entity
                    .update_in(cx, |_, window, cx| {
                        tracing::debug!("Writing transformed data to clipboard");
                        if is_html {
                            // Offered as HTML so a mail or a document pastes a
                            // table, and as text so an editor pastes the markup.
                            let representations = vec![
                                blanco_clipboard::Representation::html(transformed.clone()),
                                blanco_clipboard::Representation::text(transformed.clone()),
                            ];
                            match blanco_clipboard::write(window, representations) {
                                Ok(()) => return,
                                Err(error) => tracing::warn!(
                                    "Copying as HTML fell back to plain text: {error:#}"
                                ),
                            }
                        }
                        cx.write_to_clipboard(ClipboardItem::new_string(transformed));
                    })
                    .log_err();
            } else {
                tracing::error!("Copy transformation produced empty result");
            }
        })
        .detach();
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
    use blanco_core::ColumnType;
    use transformers::{SelectedCell, SelectedRow, SelectedTableData};

    fn create_test_data() -> SelectedTableData {
        SelectedTableData {
            table_name: Some("users".to_string()),
            db_type: None,
            columns: vec!["id".to_string(), "name".to_string(), "email".to_string()],
            selected_rows: vec![SelectedRow {
                row: 1,
                cells: vec![
                    SelectedCell {
                        col: 0,
                        value: Some("2".to_string()),
                        column_name: Some("id".to_string()),
                        column_type: Some(ColumnType::Integer),
                    },
                    SelectedCell {
                        col: 1,
                        value: Some("Bob".to_string()),
                        column_name: Some("name".to_string()),
                        column_type: Some(ColumnType::Text),
                    },
                    SelectedCell {
                        col: 2,
                        value: Some("bob@example.com".to_string()),
                        column_name: Some("email".to_string()),
                        column_type: Some(ColumnType::Text),
                    },
                ],
            }],
        }
    }

    #[test]
    fn test_registry_transform() {
        let registry = TransformerRegistry::default();

        let data = create_test_data();
        let csv_result = registry.transform_data(&data, "csv").unwrap();
        assert!(csv_result.contains("name"));
        assert!(csv_result.contains("Bob"));

        let json_result = registry.transform_data(&data, "json").unwrap();
        assert!(json_result.contains("\"name\": \"Bob\""));
        assert!(json_result.contains("\"email\": \"bob@example.com\""));
    }

    #[test]
    fn test_registry_unknown_format() {
        let registry = TransformerRegistry::default();
        let data = create_test_data();

        let result = registry.transform_data(&data, "unknown_format");
        assert!(result.is_err());
    }
}
