use gpui::{AppContext, Context, ParentElement, SharedString, Window, px};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    notification::NotificationType,
};
use ropey::Rope;
use tracing::{debug, error};

use crate::app_database::{AppDatabase, QueryTabData};
use crate::result_ext::ResultExt;
use crate::sql::extract_statement_info;
use crate::sql::statement_parser::QueryParameter;
use crate::time_format;
use database::{DatabaseService, DatabaseServiceTrait};

use super::parameter_form::ParameterForm;
use super::{EditorPanel, TabType, SQL_QUERY_LOG_MAX_LENGTH};

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

            // Use extract_statement_info to get parameters
            // For selected text, parse from the selection; otherwise use cursor position
            let statement_info = if !selected_text.trim().is_empty() {
                let selected_text = Rope::from_str(&selected_text);
                extract_statement_info(&selected_text, 0)
            } else {
                let full_text = Rope::from_str(&full_text);
                extract_statement_info(&full_text, cursor_pos)
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
            let has_params = statement_info
                .as_ref()
                .map(|info| !info.parameters.is_empty())
                .unwrap_or(false);

            tracing::info!("statement_info {:?}", statement_info);

            if has_params {
                // Show parameter modal instead of executing directly
                let params = statement_info.unwrap().parameters;
                self.show_parameter_modal(query, params, connection_id, database_name, window, cx);
            } else {
                // Execute directly
                self.execute_query(query, connection_id, &database_name, window, cx);
            }
        }
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
            let _sql_log = query_tab.sql_log.clone();

            let connection_type = None;
            let tab_data = QueryTabData {
                id: query_tab.db_id,
                title: query_tab.title.clone(),
                content: content.clone(),
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

                entity_handle.update(cx, |editor_panel: &mut EditorPanel, _| {
                    if let Some(tab) = editor_panel.tabs.get_mut(editor_panel.active_tab_ix)
                        && let TabType::Query(query_tab) = tab
                    {
                        query_tab.db_id = Some(tab_db_id);
                    }
                }).log_err();
            })
            .detach();

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
            query_tab.sql_log.update(cx, |sql_log, cx| {
                sql_log.append_text(&blanco_ui::SqlLogMessage::SqlStatement(query_for_log), cx);
            });

            // Clone values for background task
            let query_clone = query.clone();
            let db_service = DatabaseService::global(cx).clone();
            let database_name = database_name.to_string();
            let database_name_for_background = database_name.clone();

            // Spawn background task for query execution (keeps UI responsive)
            let query_task = cx.background_spawn(async move {
                let start_time = std::time::Instant::now();
                let execution_result = db_service
                    .execute_query(
                        connection_id,
                        Some(&database_name_for_background),
                        &query_clone,
                    )
                    .await;
                (execution_result, start_time)
            });

            // Spawn foreground task to handle the result and update UI
            let results_panel_clone = query_tab.results_panel.clone();
            let sql_log_clone = query_tab.sql_log.clone();
            let db_service = DatabaseService::global(cx).clone();
            let query_for_metadata = query.clone();

            self._run_query_task = cx.spawn_in(window, async move |editor_panel_entity, window| {
                let (execution_result, start_time) = query_task.await;

                match execution_result {
                    Ok(mut result) => {
                        let duration_ms = start_time.elapsed().as_millis() as i64;

                        // Add execution metadata
                        result.query_text = Some(query_for_metadata.clone());
                        result.execution_time_ms = Some(duration_ms);
                        result.is_error = false;
                        result.connection_id = Some(connection_id);

                        // Extract table metadata and get columns - still need connection for this
                        if let Ok(connection) = db_service
                            .get_or_create_connection_by_id(connection_id, Some(&database_name))
                            .await
                        {
                            // Extract table metadata from the query
                            let table_name = connection
                                .extract_table_name_from_query(&query_for_metadata, false)
                                .ok()
                                .flatten();
                            result.table_name = table_name.clone();

                            // Load full table metadata (including primary keys and foreign keys)
                            if let (Some(table_name), false) = (&table_name, result.rows.is_empty())
                                && let Ok(columns) =
                                    connection.get_columns_for_table(table_name, None).await
                            {
                                result.table_columns = Some(columns);
                            }
                        }

                        // Store rows_affected before moving result
                        let rows_affected =
                            std::cmp::max(result.rows_affected, result.row_count() as u64);

                        window.update(move |window, cx| {
                            // Update results panel
                            results_panel_clone.update(cx, |panel, cx| {
                                panel.set_query_result(result, Some(connection_id), window, cx);
                            });

                            // Log execution result to SQL log
                            sql_log_clone.update(cx, |sql_log, cx| {
                                let log_message = format!(
                                    "{}, {} rows in {}",
                                    time_format::format_current_timestamp(),
                                    rows_affected,
                                    time_format::format_duration(duration_ms)
                                );
                                sql_log.append_text(
                                    &blanco_ui::SqlLogMessage::Comment(log_message),
                                    cx,
                                );
                            });

                            editor_panel_entity
                                .update(cx, |editor_panel, cx| {
                                    editor_panel.loading = false;
                                    cx.notify();
                                })
                                .ok();
                        }).log_err();
                    }
                    Err(e) => {
                        tracing::error!("Query execution failed: {}", e);

                        window.update(|window, cx| {
                            // Log execution error to SQL log
                            let _error_duration = start_time.elapsed().as_millis() as i64;
                            sql_log_clone.update(cx, |sql_log, cx| {
                                let log_message = format!("query execution failed: {}", e);
                                sql_log.append_text(
                                    &blanco_ui::SqlLogMessage::Comment(log_message),
                                    cx,
                                );
                            });

                            editor_panel_entity
                                .update(cx, |editor_panel, cx| {
                                    editor_panel.loading = false;
                                    cx.notify();
                                })
                                .ok();

                            window.push_notification(
                                (NotificationType::Error, SharedString::from(e.to_string())),
                                cx,
                            );
                        }).log_err();
                    }
                }
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
        let param_form = cx.new(|cx| ParameterForm::new(query.clone(), params, window, cx));

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
                        weak_editor_panel
                            .update(cx, |editor_panel, cx| {
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
