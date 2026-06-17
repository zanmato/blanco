use gpui::{AppContext, Context, ParentElement, SharedString, Window, px};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    notification::NotificationType,
};
use ropey::Rope;
use tracing::{debug, error};

use crate::app_database::{AppDatabase, QueryHistoryData, QueryTabData};
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::sql::statement_parser::QueryParameter;
use crate::sql::{extract_statement_info, extract_statement_info_with_styles};
use crate::status_bar::{ActivityReporter, ActivityResult};
use crate::time_format;
use database::{DatabaseService, DatabaseServiceTrait};

use super::parameter_form::ParameterForm;
use super::{EditorPanel, SQL_QUERY_LOG_MAX_LENGTH, TabType};

impl EditorPanel {
    /// Handler for Run Query button/keyboard
    /// Checks for parameters and shows modal if needed, otherwise executes directly
    pub fn on_run_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Extract the values we need before any mutable borrows
        let tab_index = self.active_tab_ix;

        if let Some(tab) = self.tabs.get(tab_index)
            && let TabType::Query(query_tab) = tab
        {
            // Get text, cursor position, and selection from editor
            let editor = query_tab.editor.read(cx);
            let full_text = editor.text().to_string();
            let cursor_pos = editor.cursor();
            let selected_text = editor.selected_text().to_string();

            // Clone the values we need
            let connection_id = query_tab.connection_id;
            let database_name = query_tab.database_name.clone();
            // Only treat the dialect's actual placeholder syntaxes as parameters,
            // so e.g. Postgres jsonb operators (`?`, `?|`, `?&`) don't trigger the
            // parameter form.
            let param_styles = query_tab._db_type.parameter_styles();

            // Use extract_statement_info to get parameters
            // For selected text, parse from the selection; otherwise use cursor position
            let statement_info = if !selected_text.trim().is_empty() {
                let selected_text = Rope::from_str(&selected_text);
                extract_statement_info_with_styles(&selected_text, 0, param_styles)
            } else {
                let full_text = Rope::from_str(&full_text);
                extract_statement_info_with_styles(&full_text, cursor_pos, param_styles)
            };

            // Determine the query to execute: use selected text if available, otherwise extract from statement_info
            let query = if !selected_text.trim().is_empty() {
                selected_text
            } else {
                statement_info
                    .as_ref()
                    .map(|info| info.text.clone())
                    .unwrap_or_default()
            };

            if query.is_empty() {
                window.push_notification((NotificationType::Error, "No query to execute"), cx);
                return;
            }

            // Check if query has parameters
            let params = statement_info
                .map(|info| info.parameters)
                .unwrap_or_default();

            if !params.is_empty() {
                self.show_parameter_modal(query, params, connection_id, database_name, window, cx);
            } else {
                self.execute_query(query, connection_id, &database_name, window, cx);
            }
        }
    }

    /// Wrap the statement at the cursor in an EXPLAIN appropriate to the
    /// active connection's dialect and run it. The plan lands as a normal
    /// result tab (multi-result aware) so the user can pin it.
    pub fn on_explain_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;
        let Some(TabType::Query(query_tab)) = self.tabs.get(tab_index) else {
            return;
        };
        let editor = query_tab.editor.read(cx);
        let full_text = editor.text().to_string();
        let cursor_pos = editor.cursor();
        let selected_text = editor.selected_text().to_string();
        let connection_id = query_tab.connection_id;
        let database_name = query_tab.database_name.clone();
        let db_type = query_tab._db_type;

        let raw = if !selected_text.trim().is_empty() {
            selected_text
        } else {
            let full = Rope::from_str(&full_text);
            extract_statement_info(&full, cursor_pos)
                .map(|info| info.text)
                .unwrap_or_default()
        };
        if raw.trim().is_empty() {
            window.push_notification((NotificationType::Error, "No query to explain"), cx);
            return;
        }
        let wrapped = crate::sql::explain::wrap_explain(db_type, &raw);
        self.execute_query(wrapped, connection_id, &database_name, window, cx);
    }

    /// Execute a query with the given parameters
    pub(super) fn execute_query(
        &mut self,
        query: String,
        _connection_id: i64,
        database_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Save the current tab before executing the query
        // Extract the tab data we need before starting async operations
        let tab_index = self.active_tab_ix;
        let tab = self.tabs.get(self.active_tab_ix);

        if let Some(TabType::Query(query_tab)) = tab {
            let content = query_tab.editor.read(cx).text().to_string();

            let _results_panel = query_tab.results_panel.clone();
            let _sql_view = query_tab.sql_view.clone();

            let connection_type = None;
            let tab_data = QueryTabData {
                id: query_tab.db_id,
                title: query_tab.title.clone(),
                content: content,
                position: tab_index as i32,
                connection_id: Some(query_tab.connection_id),
                connection_type,
                connection_name: query_tab.connection_name.clone(),
                database_name: Some(query_tab.database_name.clone()),
                schema_name: query_tab.schema_name.clone(),
                environment_type: query_tab.environment_type,
            };

            // Trigger the save operation in background
            let app_database = AppDatabase::global(cx).clone();
            let _title = query_tab.title.clone();
            let connection_id = query_tab.connection_id;

            cx.spawn(async move |entity_handle, cx| {
                // Create the final tab data with connection_id
                let mut final_tab_data = tab_data;
                final_tab_data.connection_id = Some(connection_id);

                // First save to get or create the database ID
                let tab_db_id = match app_database.save_query_tab(&final_tab_data).await {
                    Ok(db_id) => {
                        debug!("Tab saved successfully with db_id: {}", db_id);
                        db_id
                    }
                    Err(e) => {
                        error!("Failed to save tab: {}", e);
                        return;
                    }
                };

                entity_handle
                    .update(cx, |editor_panel: &mut EditorPanel, _| {
                        if let Some(tab) = editor_panel.tabs.get_mut(editor_panel.active_tab_ix)
                            && let TabType::Query(query_tab) = tab
                        {
                            query_tab.db_id = Some(tab_db_id);
                        }
                    })
                    .log_err();
            })
            .detach();

            // Report the in-flight query to the status bar. The guard moves into
            // the foreground result task: success/error finish it with an
            // outcome, cancellation drops it (clearing the line).
            let activity_label = query_tab
                .connection_name
                .clone()
                .unwrap_or_else(|| query_tab.database_name.clone());
            let activity =
                ActivityReporter::global(cx).begin(format!("{activity_label}: executing query"));

            // Set loading state to true
            self.loading = true;
            cx.notify();

            // Log the query to the SQL log, truncating if too long
            let query_for_log = if query.chars().count() > SQL_QUERY_LOG_MAX_LENGTH {
                format!(
                    "{}... (truncated)",
                    query
                        .chars()
                        .take(SQL_QUERY_LOG_MAX_LENGTH)
                        .collect::<String>()
                )
            } else {
                query.clone()
            };
            query_tab.sql_view.update(cx, |sql_view, cx| {
                sql_view.append_text(&blanco_ui::SqlViewMessage::SqlStatement(query_for_log), cx);
            });

            // Clone values for background task
            let query_clone = query.clone();
            let db_service = DatabaseService::global(cx).clone();
            let database_name = database_name.to_string();
            let database_name_for_background = database_name.clone();

            // The background task drives the actual database call and reports
            // its result back via a oneshot channel. We keep its `Task` handle
            // in `abort_query_task` so the user can cancel: dropping the task
            // cancels the in-flight future, which causes `tx` to be dropped
            // and the foreground task sees `rx.await` return `Err(Canceled)`.
            let (tx, rx) = futures::channel::oneshot::channel();
            let query_task = cx.background_spawn(async move {
                let start_time = std::time::Instant::now();
                let execution_result = db_service
                    .execute_script(
                        connection_id,
                        Some(&database_name_for_background),
                        &query_clone,
                    )
                    .await;
                // Measure the duration here, on the background thread, right when
                // the query completes. Reading the elapsed time on the foreground
                // task instead would also count the oneshot hand-off plus however
                // long the GPUI foreground executor takes to poll us back, which
                // is several milliseconds and unrelated to query execution.
                let _ = tx.send((execution_result, start_time.elapsed()));
            });
            self.abort_query_task = Some(query_task);

            // Spawn foreground task to handle the result and update UI
            let results_panel_clone = query_tab.results_panel.clone();
            let sql_view_clone = query_tab.sql_view.clone();
            let db_service = DatabaseService::global(cx).clone();
            let query_for_metadata = query;
            let completion_provider = query_tab.completion_provider.clone();

            // Static metadata captured for the query-history log. The dynamic
            // bits (duration, row counts, success/error) are filled in once the
            // result arrives.
            let history_db = AppDatabase::global(cx).clone();
            let history_query = query_for_metadata.clone();
            let history_connection_id = connection_id;
            let history_connection_name = query_tab.connection_name.clone();
            let history_database_name = database_name.clone();
            let history_max_items =
                AppSettings::global(cx).settings.database.max_history_items as i64;

            self._run_query_task = cx.spawn_in(window, async move |editor_panel_entity, window| {
                let activity = activity;
                let Ok((execution_result, query_duration)) = rx.await else {
                    // Background task was dropped (user clicked Abort).
                    window
                        .update(|window, cx| {
                            sql_view_clone.update(cx, |sql_view, cx| {
                                sql_view.append_text(
                                    &blanco_ui::SqlViewMessage::Comment(
                                        "query cancelled by user".to_string(),
                                    ),
                                    cx,
                                );
                            });
                            editor_panel_entity
                                .update(cx, |editor_panel, cx| {
                                    editor_panel.loading = false;
                                    editor_panel.abort_query_task = None;
                                    cx.notify();
                                })
                                .ok();
                            let _ = window;
                        })
                        .log_err();
                    return;
                };

                // The branches below move `editor_panel_entity` into their UI
                // update closures; keep a handle for the post-match emit.
                let emit_handle = editor_panel_entity.clone();

                match execution_result {
                    Ok(mut results) => {
                        let duration_ms = query_duration.as_millis() as i64;

                        // Annotate every result with execution metadata. The
                        // duration is recorded against the first result only;
                        // we don't have per-statement timings here.
                        let mut total_rows_affected: u64 = 0;
                        for (idx, result) in results.iter_mut().enumerate() {
                            result.query_text = Some(query_for_metadata.clone());
                            if idx == 0 {
                                result.execution_time_ms = Some(duration_ms);
                            }
                            result.is_error = false;
                            result.connection_id = Some(connection_id);
                            total_rows_affected = total_rows_affected.saturating_add(
                                std::cmp::max(result.rows_affected, result.row_count() as u64),
                            );
                        }

                        // Resolve table metadata only when a single statement
                        // ran; merging FK/PK info across N statements would be
                        // ambiguous, and the typical "edit rows in the grid"
                        // flow targets single SELECTs anyway.
                        if results.len() == 1
                            && let Ok(connection) = db_service
                                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                                .await
                        {
                            let result = &mut results[0];
                            let table_name = connection
                                .extract_table_name_from_query(&query_for_metadata, false)
                                .ok()
                                .flatten();
                            result.table_name = table_name.clone();

                            if let (Some(table_name), false) = (&table_name, result.rows.is_empty())
                                && let Ok(columns) =
                                    connection.get_columns_for_table(table_name, None).await
                            {
                                result.table_columns = Some(columns);
                            }
                        }

                        // Invalidate completion cache after DDL statements
                        if is_ddl_query(&query_for_metadata)
                            && let Some(provider) = &completion_provider
                        {
                            provider.invalidate_cache();
                        }

                        let rows_affected = total_rows_affected;
                        let returned_row_count: i64 =
                            results.iter().map(|result| result.row_count() as i64).sum();

                        activity.finish(ActivityResult::Ok(
                            format!(
                                "Query OK · {} rows · {}",
                                rows_affected,
                                time_format::format_duration(duration_ms)
                            )
                            .into(),
                        ));

                        window
                            .update(move |window, cx| {
                                // Update results panel
                                results_panel_clone.update(cx, |panel, cx| {
                                    panel.set_query_results(
                                        results,
                                        Some(connection_id),
                                        window,
                                        cx,
                                    );
                                });

                                // Log execution result to SQL log
                                sql_view_clone.update(cx, |sql_view, cx| {
                                    let log_message = format!(
                                        "{}, {} rows in {}",
                                        time_format::format_current_timestamp(),
                                        rows_affected,
                                        time_format::format_duration(duration_ms)
                                    );
                                    sql_view.append_text(
                                        &blanco_ui::SqlViewMessage::Comment(log_message),
                                        cx,
                                    );
                                });

                                editor_panel_entity
                                    .update(cx, |editor_panel, cx| {
                                        editor_panel.loading = false;
                                        editor_panel.abort_query_task = None;
                                        cx.notify();
                                    })
                                    .ok();
                            })
                            .log_err();

                        history_db
                            .record_query_history(&QueryHistoryData {
                                id: None,
                                query_text: history_query.clone(),
                                executed_at: chrono::Utc::now().timestamp(),
                                duration_ms: Some(duration_ms),
                                rows_affected: Some(rows_affected as i64),
                                row_count: Some(returned_row_count),
                                success: true,
                                error_message: None,
                                connection_id: Some(history_connection_id),
                                connection_name: history_connection_name.clone(),
                                database_name: Some(history_database_name.clone()),
                            })
                            .await
                            .map_err(anyhow::Error::from)
                            .log_err();
                    }
                    Err(e) => {
                        tracing::error!("Query execution failed: {e:#}");
                        let duration_ms = query_duration.as_millis() as i64;
                        let error_message = format!("{e:#}");

                        activity.finish(ActivityResult::Err("query execution failed".into()));

                        window
                            .update(|window, cx| {
                                // Log execution error to SQL log
                                sql_view_clone.update(cx, |sql_view, cx| {
                                    let log_message =
                                        format!("query execution failed: {error_message}");
                                    sql_view.append_text(
                                        &blanco_ui::SqlViewMessage::Comment(log_message),
                                        cx,
                                    );
                                });

                                editor_panel_entity
                                    .update(cx, |editor_panel, cx| {
                                        editor_panel.loading = false;
                                        editor_panel.abort_query_task = None;
                                        cx.notify();
                                    })
                                    .ok();

                                window.push_notification(
                                    (
                                        NotificationType::Error,
                                        SharedString::from(error_message.clone()),
                                    ),
                                    cx,
                                );
                            })
                            .log_err();

                        history_db
                            .record_query_history(&QueryHistoryData {
                                id: None,
                                query_text: history_query.clone(),
                                executed_at: chrono::Utc::now().timestamp(),
                                duration_ms: Some(duration_ms),
                                rows_affected: None,
                                row_count: None,
                                success: false,
                                error_message: Some(error_message),
                                connection_id: Some(history_connection_id),
                                connection_name: history_connection_name.clone(),
                                database_name: Some(history_database_name.clone()),
                            })
                            .await
                            .map_err(anyhow::Error::from)
                            .log_err();
                    }
                }

                history_db
                    .prune_query_history(history_max_items)
                    .await
                    .map_err(anyhow::Error::from)
                    .log_err();

                // Let observers (e.g. the history panel) refresh now that a new
                // entry has been written.
                window
                    .update(|_window, cx| {
                        emit_handle
                            .update(cx, |_editor_panel, cx| {
                                cx.emit(super::EditorPanelEvent::QueryRecorded);
                            })
                            .ok();
                    })
                    .log_err();
            });
        }
    }

    /// Show parameter input modal and execute query with substituted values
    fn show_parameter_modal(
        &mut self,
        query: String,
        params: Vec<QueryParameter>,
        connection_id: i64,
        database_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial_values = match self.tabs.get(self.active_tab_ix) {
            Some(TabType::Query(query_tab)) => query_tab.last_parameter_values.clone(),
            _ => Default::default(),
        };
        let param_form =
            cx.new(|cx| ParameterForm::new(query.clone(), params, &initial_values, window, cx));

        let weak_editor_panel = cx.entity().downgrade();
        window.open_dialog(cx, move |modal, _, _| {
            modal
                .title("Query Parameters")
                .w(px(500.))
                .child(param_form.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").label("Cancel").outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").primary().label("Run"))),
                )
                .on_ok({
                    let param_form = param_form.clone();
                    let database_name = database_name.clone();
                    let weak_editor_panel = weak_editor_panel.clone();
                    move |_event, window, cx| {
                        let substituted_query = param_form.read(cx).get_substituted_query(cx);
                        let entered_values = param_form.read(cx).current_values(cx);
                        weak_editor_panel
                            .update(cx, |editor_panel, cx| {
                                if let Some(TabType::Query(query_tab)) =
                                    editor_panel.tabs.get_mut(editor_panel.active_tab_ix)
                                {
                                    query_tab.last_parameter_values = entered_values;
                                }
                                editor_panel.execute_query(
                                    substituted_query,
                                    connection_id,
                                    &database_name,
                                    window,
                                    cx,
                                );
                            })
                            .ok();
                        true // Close dialog
                    }
                })
        });
    }
}

fn is_ddl_query(query: &str) -> bool {
    let trimmed = query.trim();
    let first_word = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_uppercase();
    matches!(
        first_word.as_str(),
        "CREATE" | "DROP" | "ALTER" | "TRUNCATE"
    )
}
