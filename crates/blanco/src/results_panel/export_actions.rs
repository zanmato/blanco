//! Action handlers for the `ExportAs*` actions on `ResultsPanel`.

use std::path::PathBuf;

use gpui::{Context, SharedString, Window};
use gpui_component::{WindowExt as _, notification::NotificationType};

use crate::app::{ExportAsCSV, ExportAsJSON, ExportAsMarkdown, ExportAsSQL, ExportAsTSV};
use crate::export::service::{ExportResult, ExportService};
use crate::result_ext::ResultExt;
use transformers::{
    CsvTransformer, DataTransformer, JsonTransformer, MarkdownTransformer, SqlTransformer,
};

use super::ResultsPanel;

impl ResultsPanel {
    pub(super) fn on_export_as_csv(
        &mut self,
        _action: &ExportAsCSV,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("csv", window, cx);
    }

    pub(super) fn on_export_as_tsv(
        &mut self,
        _action: &ExportAsTSV,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("tsv", window, cx);
    }

    pub(super) fn on_export_as_json(
        &mut self,
        _action: &ExportAsJSON,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("json", window, cx);
    }

    pub(super) fn on_export_as_sql(
        &mut self,
        _action: &ExportAsSQL,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("sql", window, cx);
    }

    pub(super) fn on_export_as_markdown(
        &mut self,
        _action: &ExportAsMarkdown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("markdown", window, cx);
    }

    /// Export selected data in the specified format
    fn export_selected_as(&mut self, format: &str, window: &mut Window, cx: &mut Context<Self>) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!(
                "Failed to export as {}: No rows selected for exporting",
                format
            );
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);
        let table_name = selected_data
            .table_name
            .clone()
            .unwrap_or_else(|| "export".to_string());

        let extension = match format {
            "csv" => "csv",
            "json" => "json",
            "sql" => "sql",
            "markdown" => "md",
            _ => "txt",
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let default_filename = format!("{}_{}.{}", table_name, timestamp, extension);

        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let path = cx.prompt_for_new_path(&home_dir, Some(&default_filename));

        let format_owned = format.to_string();
        let table_name_for_sql = table_name;
        let db_type = selected_data
            .db_type
            .unwrap_or(database::DatabaseType::PostgreSQL);

        cx.spawn_in(window, async move |entity, cx| {
            let outer_result = path.await;
            let inner_result = match outer_result {
                Ok(inner) => inner,
                Err(_) => return,
            };

            let file_path = match inner_result {
                Ok(Some(path_buf)) => path_buf,
                _ => return,
            };

            let transformer: Box<dyn DataTransformer> = match format_owned.as_str() {
                "csv" => Box::new(CsvTransformer),
                "json" => Box::new(JsonTransformer::new()),
                "sql" => Box::new(SqlTransformer::with_table_name(
                    table_name_for_sql.clone(),
                    db_type,
                )),
                "markdown" => Box::new(MarkdownTransformer),
                _ => {
                    tracing::error!("Unknown format: {}", format_owned);
                    return;
                }
            };

            let export_service = ExportService::new();

            match export_service
                .export_selected_data(&selected_data, transformer.as_ref(), &file_path)
                .await
            {
                Ok(result) => match result {
                    ExportResult::Success {
                        file_path,
                        rows_exported,
                        ..
                    } => {
                        tracing::info!(
                            "Export completed successfully: {} -> {} ({} rows)",
                            format_owned,
                            file_path,
                            rows_exported
                        );
                        entity
                            .update_in(cx, |_panel, window, cx| {
                                window.push_notification(
                                    (
                                        NotificationType::Success,
                                        SharedString::from(format!(
                                            "Exported {} rows to {}",
                                            rows_exported, file_path
                                        )),
                                    ),
                                    cx,
                                );
                            })
                            .log_err();
                    }
                    ExportResult::Cancelled => {
                        tracing::info!("Export cancelled by user");
                    }
                },
                Err(e) => {
                    tracing::error!("Export failed: {}", e);
                    entity
                        .update_in(cx, |_panel, window, cx| {
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    SharedString::from(format!("Export failed: {}", e)),
                                ),
                                cx,
                            );
                        })
                        .log_err();
                }
            }
        })
        .detach();
    }
}
