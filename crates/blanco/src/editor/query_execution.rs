use gpui::{AppContext, Context, ParentElement, SharedString, Window, px};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    notification::NotificationType,
};
use ropey::Rope;
use std::cell::Cell;
use std::rc::Rc;
use tracing::{debug, error};

use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::sql::{extract_statement_info, extract_statement_info_with_styles};
use crate::status_bar::{ActivityReporter, ActivityResult};
use crate::time_format;
use app_database::{AppDatabase, EnvironmentType, QueryHistoryData, QueryTabData};
use blanco_core::StatementAccess;
use database::{DatabaseService, DatabaseServiceTrait};
use sql_parser::statement_parser::QueryParameter;

use super::parameter_form::ParameterForm;
use super::{EditorPanel, ProdWritePrompt, SQL_QUERY_LOG_MAX_LENGTH, TabType};

/// Thresholds above which a loaded result gets a size warning.
const LARGE_RESULT_ROWS: usize = 100_000;
const LARGE_RESULT_CELLS: usize = 1_000_000;
const LARGE_RESULT_BYTES: usize = 64 * 1024 * 1024;

impl EditorPanel {
    /// Handler for Run Query button/keyboard
    /// Checks for parameters and shows modal if needed, otherwise executes directly
    pub fn on_run_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Extract the values we need before any mutable borrows
        let tab_index = self.active_tab_ix;

        if let Some(TabType::Script(script_tab)) = self.tabs.get(tab_index) {
            // A script can issue any statement through `db`, so the whole run
            // is confirmed rather than individual statements.
            if script_tab.context.environment_type == Some(EnvironmentType::Prod) {
                let connection_name = script_tab.context.connection_name.clone();
                self.confirm_prod_write(
                    ProdWritePrompt {
                        title: "Run script on PROD?",
                        message: format!(
                            "\"{connection_name}\" is tagged as a production connection. Scripts can modify data through `db.execute` and `db.transaction`."
                        ),
                        confirm_label: "Run on PROD",
                        tab_index,
                    },
                    |panel, window, cx| panel.execute_script_tab(window, cx),
                    None,
                    window,
                    cx,
                );
                return;
            }
            self.execute_script_tab(window, cx);
            return;
        }

        if let Some(tab) = self.tabs.get(tab_index)
            && let TabType::Query(query_tab) = tab
        {
            // Get text, cursor position, and selection from editor
            let editor = query_tab.editor.read(cx);
            let full_text = editor.text().to_string();
            let cursor_pos = editor.cursor();
            let selected_text = editor.selected_text().to_string();

            // Clone the values we need
            let connection_id = query_tab.context.connection_id;
            let database_name = query_tab.context.database_name.clone();
            let db_type = query_tab.context.db_type;

            // Non-SQL backends (Redis) have no grammar to parse and no bind
            // parameters: treat the buffer as one command per line and run the
            // line at the cursor (or the selection verbatim).
            if !db_type.supports_sql() {
                let command = if !selected_text.trim().is_empty() {
                    selected_text.trim().to_string()
                } else {
                    crate::sql::extract_command_at_cursor(&full_text, cursor_pos)
                        .unwrap_or_default()
                };
                if command.is_empty() {
                    window.push_notification((NotificationType::Error, "No command to run"), cx);
                    cx.emit(super::EditorPanelEvent::QueryRunEnded {
                        tab_index,
                        outcome: Err("No command to run".to_string()),
                    });
                    return;
                }
                self.execute_query(command, connection_id, &database_name, window, cx);
                return;
            }

            // Only treat the dialect's actual placeholder syntaxes as parameters,
            // so e.g. Postgres jsonb operators (`?`, `?|`, `?&`) don't trigger the
            // parameter form.
            let param_styles = db_type.parameter_styles();

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
                cx.emit(super::EditorPanelEvent::QueryRunEnded {
                    tab_index,
                    outcome: Err("No query to execute".to_string()),
                });
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
        let connection_id = query_tab.context.connection_id;
        let database_name = query_tab.context.database_name.clone();
        let db_type = query_tab.context.db_type;

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
        let wrapped = sql_parser::explain::wrap_explain(db_type, &raw);
        self.execute_query(wrapped, connection_id, &database_name, window, cx);
    }

    /// Execute a query with the given parameters
    /// Run `query` on the active tab, first asking for confirmation when the
    /// tab's connection is tagged PROD and the statement modifies data. The
    /// classifier is conservative (unknown statements count as writes), so a
    /// production connection never runs a write without a click.
    pub(super) fn execute_query(
        &mut self,
        query: String,
        connection_id: i64,
        database_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(TabType::Query(query_tab)) = self.tabs.get(self.active_tab_ix) else {
            return;
        };
        let is_prod = query_tab.context.environment_type == Some(EnvironmentType::Prod);
        let is_write = blanco_core::write_guard::classify(query_tab.context.db_type, &query)
            == StatementAccess::Write;
        if !(is_prod && is_write) {
            self.execute_query_unchecked(query, connection_id, database_name, window, cx);
            return;
        }

        let connection_name = query_tab.context.connection_name.clone();
        let database_name = database_name.to_string();
        let tab_index = self.active_tab_ix;
        self.confirm_prod_write(
            ProdWritePrompt {
                title: "Run write statement on PROD?",
                message: format!(
                    "\"{connection_name}\" is tagged as a production connection and this statement modifies data."
                ),
                confirm_label: "Run on PROD",
                tab_index,
            },
            move |panel, window, cx| {
                panel.execute_query_unchecked(
                    query.clone(),
                    connection_id,
                    &database_name,
                    window,
                    cx,
                )
            },
            Some(Box::new(move |_, cx| {
                cx.emit(super::EditorPanelEvent::QueryRunEnded {
                    tab_index,
                    outcome: Err("The user did not confirm the write on PROD.".to_string()),
                });
            })),
            window,
            cx,
        );
    }

    fn execute_query_unchecked(
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
        let run_timestamp = chrono::Utc::now().timestamp();
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(tab_index) {
            query_tab.last_run_at = Some(run_timestamp);
        }
        let tab = self.tabs.get(self.active_tab_ix);

        if let Some(TabType::Query(query_tab)) = tab {
            let content = query_tab.editor.read(cx).text().to_string();

            let _results_panel = query_tab.results_panel.clone();
            let _sql_view = query_tab.sql_view.clone();

            let tab_data = QueryTabData {
                id: query_tab.db_id,
                title: query_tab.title.clone(),
                content: content,
                position: tab_index as i32,
                connection_id: Some(query_tab.context.connection_id),
                connection_type: Some(query_tab.context.db_type.as_str().to_string()),
                connection_name: Some(query_tab.context.connection_name.clone()),
                database_name: Some(query_tab.context.database_name.clone()),
                schema_name: query_tab.context.schema_name.clone(),
                environment_type: query_tab.context.environment_type,
                tab_kind: app_database::EditorKind::Query,
                last_run_at: Some(run_timestamp),
            };

            // Trigger the save operation in background
            let app_database = AppDatabase::global(cx).clone();
            let _title = query_tab.title.clone();
            let connection_id = query_tab.context.connection_id;
            let db_type = query_tab.context.db_type;

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

                app_database
                    .touch_query_tab_last_run(tab_db_id, run_timestamp)
                    .await
                    .map_err(anyhow::Error::from)
                    .log_err();

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
            let activity_label = query_tab.context.connection_name.clone();
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

            // Optional per-query timeout (0 disables it). Enforced below by
            // racing the execution future against a timer so a runaway query
            // fails on its own instead of relying solely on manual cancel.
            let query_timeout = {
                let seconds = AppSettings::global(cx)
                    .settings
                    .database
                    .query_timeout_seconds;
                (seconds > 0).then(|| std::time::Duration::from_secs(seconds as u64))
            };

            // The background task drives the actual database call and reports
            // its result back via a oneshot channel. We keep its `Task` handle
            // in `abort_query_task` so the user can cancel: dropping the task
            // cancels the in-flight future, which causes `tx` to be dropped
            // and the foreground task sees `rx.await` return `Err(Canceled)`.
            let (tx, rx) = futures::channel::oneshot::channel();
            let query_task = cx.background_spawn(async move {
                let start_time = std::time::Instant::now();
                let execution_future = db_service.execute_script(
                    connection_id,
                    Some(&database_name_for_background),
                    &query_clone,
                );
                let execution_result = match query_timeout {
                    Some(timeout) => {
                        let deadline = async {
                            smol::Timer::after(timeout).await;
                            Err(anyhow::anyhow!(
                                "Query exceeded the configured {}s timeout",
                                timeout.as_secs()
                            ))
                        };
                        smol::future::or(execution_future, deadline).await
                    }
                    None => execution_future.await,
                };

                let elapsed = start_time.elapsed();
                match &execution_result {
                    Ok(results) => tracing::debug!(
                        connection_id,
                        statements = results.len(),
                        duration_ms = elapsed.as_millis(),
                        "query execution finished"
                    ),
                    Err(error) => tracing::debug!(
                        connection_id,
                        duration_ms = elapsed.as_millis(),
                        %error,
                        "query execution failed"
                    ),
                }
                let _ = tx.send((execution_result, elapsed));
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
            let history_connection_name = query_tab.context.connection_name.clone();
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
                                    cx.emit(super::EditorPanelEvent::QueryRunEnded {
                                        tab_index,
                                        outcome: Err("Query cancelled by the user.".to_string()),
                                    });
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
                        let duration_us = query_duration.as_micros() as i64;

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
                        // flow targets single SELECTs anyway. Skipped entirely for
                        // non-SQL backends, which have no table/column catalog.
                        if db_type.supports_sql()
                            && results.len() == 1
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
                        if db_type.supports_sql()
                            && is_ddl_query(&query_for_metadata)
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
                                time_format::format_duration(duration_us)
                            )
                            .into(),
                        ));

                        let large_result = results
                            .iter()
                            .map(|result| {
                                let cells = result.rows.len() * result.columns.len();
                                let bytes: usize = result
                                    .rows
                                    .iter()
                                    .flatten()
                                    .map(|cell| cell.as_ref().map_or(0, String::len))
                                    .sum();
                                (result.rows.len(), cells, bytes)
                            })
                            .find(|(rows, cells, bytes)| {
                                *rows >= LARGE_RESULT_ROWS
                                    || *cells >= LARGE_RESULT_CELLS
                                    || *bytes >= LARGE_RESULT_BYTES
                            });

                        // The final result set as the MCP bridge reports it,
                        // built before the results move into the grid. Capped
                        // rows keep this cheap on every run.
                        let run_outcome = results
                            .last()
                            .map(|result| {
                                crate::mcp::tools::result_to_json(
                                    result,
                                    crate::mcp::tools::DEFAULT_MAX_ROWS,
                                )
                            })
                            .unwrap_or(serde_json::Value::Null);

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

                                // Blanco loads whole results on purpose, so
                                // a heads-up is the user's cue to narrow the
                                // query rather than a limit imposed on them.
                                if let Some((rows, _, bytes)) = large_result {
                                    window.push_notification(
                                        (
                                            NotificationType::Warning,
                                            format!(
                                                "Large result loaded: {rows} rows, {:.1} MB of cell data",
                                                bytes as f64 / (1024.0 * 1024.0)
                                            ),
                                        ),
                                        cx,
                                    );
                                }

                                // Log execution result to SQL log
                                sql_view_clone.update(cx, |sql_view, cx| {
                                    let log_message = format!(
                                        "{}, {} rows in {}",
                                        time_format::format_current_timestamp(),
                                        rows_affected,
                                        time_format::format_duration(duration_us)
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
                                        cx.emit(super::EditorPanelEvent::QueryRunEnded {
                                            tab_index,
                                            outcome: Ok(run_outcome),
                                        });
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
                                connection_name: Some(history_connection_name.clone()),
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
                                        cx.emit(super::EditorPanelEvent::QueryRunEnded {
                                            tab_index,
                                            outcome: Err(error_message.clone()),
                                        });
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
                                connection_name: Some(history_connection_name.clone()),
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
        let tab_index = self.active_tab_ix;
        let submitted = Rc::new(Cell::new(false));
        window.open_dialog(cx, move |modal, _, _| {
            let submitted_on_ok = submitted.clone();
            let submitted_on_close = submitted.clone();
            let weak_on_close = weak_editor_panel.clone();
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
                        submitted_on_ok.set(true);
                        let substituted_query = param_form.read(cx).get_substituted_query(cx);
                        let entered_values = param_form.read(cx).current_values(cx);
                        // Close this dialog before running: `execute_query` may
                        // open the PROD confirmation, and returning `true` here
                        // would then pop that dialog off the stack instead of
                        // this one, leaving the parameter form stuck open.
                        window.close_dialog(cx);
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
                        false
                    }
                })
                .on_close(move |_, _, cx| {
                    if submitted_on_close.get() {
                        return;
                    }
                    weak_on_close
                        .update(cx, |_, cx| {
                            cx.emit(super::EditorPanelEvent::QueryRunEnded {
                                tab_index,
                                outcome: Err(
                                    "The statement has bind parameters and the user cancelled the parameter form."
                                        .to_string(),
                                ),
                            });
                        })
                        .log_err();
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
