//! Action handlers for the `ExportAs*` actions on `ResultsPanel`.

use std::path::PathBuf;

use gpui::{Context, SharedString, Window};
use gpui_component::{WindowExt as _, notification::NotificationType};

use crate::app::{
    ExportAsCSV, ExportAsJSON, ExportAsMarkdown, ExportAsSQL, ExportAsTSV, ExportCompleteResult,
};
use crate::export::modal::{ExportModal, ExportSource, open_export_dialog};
use crate::export::service::{ExportResult, ExportService};
use crate::result_ext::ResultExt;
use blanco_core::write_guard::{self, StatementAccess};
use database::DatabaseType;
use gpui::AppContext as _;
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

    /// Runs the query of the active result again and streams every row to a
    /// file, where the grid export writes only the selected rows.
    pub(super) fn on_export_complete_result(
        &mut self,
        _action: &ExportCompleteResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_key_value = self
            .result_tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.key_value.is_some());
        let delegate = self.table_state.read(cx).delegate();
        let query = if is_key_value {
            Err(NOT_EXPORTABLE)
        } else {
            complete_result_query(self.db_type, delegate.original_query.as_deref())
        };
        let sql = match query {
            Ok(sql) => sql,
            Err(reason) => {
                window
                    .push_notification((NotificationType::Warning, SharedString::from(reason)), cx);
                return;
            }
        };
        let source = ExportSource::Query {
            sql,
            name: delegate
                .table_name
                .clone()
                .unwrap_or_else(|| "result".to_string()),
        };
        let connection_id = self.connection_id;
        let database_name = self.database_name.clone();
        let db_type = self.db_type;
        let modal = cx.new(|cx| {
            ExportModal::with_source(connection_id, database_name, db_type, source, window, cx)
        });
        open_export_dialog("Export Complete Result", modal, window, cx);
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

const NOT_EXPORTABLE: &str = "This result has no query that can be run again for export.";

/// The statement to run again to export the complete result of `query`.
///
/// Only a single statement that reads is run again: running the text of a
/// write, or of a batch, a second time would change data or export the
/// wrong result.
fn complete_result_query(
    db_type: DatabaseType,
    query: Option<&str>,
) -> Result<String, &'static str> {
    if !db_type.dialect().supports_sql() {
        return Err("Export Complete Result needs a SQL connection.");
    }
    let query = query.ok_or(NOT_EXPORTABLE)?;
    let mut statements = write_guard::split_statements(query);
    if statements.len() != 1 {
        return Err(
            "The result came from several statements. Run its statement on its own to export the complete result.",
        );
    }
    let statement = statements.remove(0);
    if write_guard::classify_sql(&statement) != StatementAccess::Read {
        return Err("Only a result of a read-only query can be exported by running it again.");
    }
    Ok(statement.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complete_result_query() {
        let pg = DatabaseType::PostgreSQL;
        assert_eq!(
            complete_result_query(pg, Some("  SELECT * FROM users;\n")),
            Ok("SELECT * FROM users".to_string())
        );
        assert_eq!(
            complete_result_query(pg, Some("WITH r AS (SELECT 1) SELECT * FROM r")),
            Ok("WITH r AS (SELECT 1) SELECT * FROM r".to_string())
        );
        assert!(complete_result_query(pg, Some("DELETE FROM users RETURNING *")).is_err());
        assert!(complete_result_query(pg, Some("SELECT 1; SELECT 2")).is_err());
        assert!(complete_result_query(pg, None).is_err());
        assert!(complete_result_query(DatabaseType::Redis, Some("GET key")).is_err());
    }
}
