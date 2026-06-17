use gpui::{AppContext, Context, Entity, Window};

use database::DatabaseService;

use super::table_operations::TableChangeOperation;
use super::{ChangeType, ResultsPanel, TableOperationResponse};
use crate::result_ext::ResultExt;
use crate::time_format;

impl ResultsPanel {
    pub fn commit_changes_with_sql_view(
        &mut self,
        _window: &mut Window,
        sql_view: &Entity<blanco_ui::SqlView>,
        cx: &mut Context<Self>,
    ) {
        self.commit_changes_internal(_window, Some(sql_view), cx)
    }

    #[allow(dead_code)]
    pub fn commit_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.commit_changes_internal(_window, None, cx)
    }

    fn commit_changes_internal(
        &mut self,
        _window: &mut Window,
        sql_view: Option<&Entity<blanco_ui::SqlView>>,
        cx: &mut Context<Self>,
    ) {
        // First, commit any currently editing cell
        if let Some((row, col)) = self.get_current_editing_cell(cx) {
            // Get the current value from the input
            if let Some(input) = &self.editing_input {
                let current_value = input.read(cx).text().to_string();
                // Update the cell value before committing
                self.update_editing_cell_value(row, col, current_value.clone(), cx);

                // Commit the cell edit
                self.commit_cell_edit(row, col, current_value, cx);
            } else {
                self.cancel_current_edit(cx);
            }
        }

        // Get changes and convert to database-agnostic operations
        let change_operations = self
            .table_state
            .read(cx)
            .delegate()
            .create_change_operations();

        if change_operations.is_empty() {
            return;
        }

        tracing::debug!("committing {} table operation(s)", change_operations.len());

        let delegate = self.table_state.read(cx).delegate();

        let change_operations_for_pipeline = change_operations;
        let connection_id_for_pipeline = delegate.connection_id;
        let database_name = delegate.database_name.clone();

        // Spawn background task to execute table operations
        let db_service = DatabaseService::global(cx).clone();
        let _table_entity = self.table_state.clone();
        let _sql_view_entity: Option<Entity<blanco_ui::SqlView>> = sql_view.cloned();

        let table_operations_task = cx.background_spawn(async move {
            let start_time = std::time::Instant::now();

            // Execute table operations using DatabaseService
            let result = match db_service
                .get_or_create_connection(connection_id_for_pipeline, Some(&database_name))
                .await
            {
                Ok(connection) => {
                    // Convert table operations to SQL and execute them
                    let mut total_rows_affected = 0u64;
                    let mut operations_executed = 0;
                    let mut error_message = None;
                    let mut success = true;
                    let mut sql_queries = Vec::new();

                    for operation in &change_operations_for_pipeline {
                        tracing::debug!("Got operation {:?}", operation);
                        let sql_query = (operation as &TableChangeOperation).to_sql_query();
                        sql_queries.push(sql_query.clone());
                        match connection
                            .execute_query(&sql_query, Some(&database_name), None)
                            .await
                        {
                            Ok(query_result) => {
                                total_rows_affected += query_result.rows_affected;
                                operations_executed += 1;
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to execute operation '{}': {}",
                                    sql_query,
                                    e
                                );
                                success = false;
                                error_message = Some(format!("{e:#}"));
                                break;
                            }
                        }
                    }

                    TableOperationResponse {
                        success,
                        rows_affected: Some(total_rows_affected),
                        error_message,
                        operations_executed,
                        duration: start_time.elapsed(),
                        sql_queries,
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to get connection for table operations: {}", e);
                    TableOperationResponse {
                        success: false,
                        rows_affected: None,
                        error_message: Some(format!("Connection error: {}", e)),
                        operations_executed: 0,
                        duration: start_time.elapsed(),
                        sql_queries: Vec::new(),
                    }
                }
            };

            tracing::debug!(
                "table operations completed in {:?}, success: {}",
                start_time.elapsed(),
                result.success
            );

            result
        });

        // Spawn async task to handle the response
        let sql_view_response_entity: Option<Entity<blanco_ui::SqlView>> = sql_view.cloned();
        cx.spawn(async move |entity, cx| {
            let response = table_operations_task.await;

            tracing::debug!(
                "received table operation response: success={}, rows_affected={:?}",
                response.success,
                response.rows_affected
            );

            // Handle successful operations
            if response.success {
                // Clear edits, remove deleted rows, and refresh the table
                entity
                    .update(cx, |panel, cx| {
                        panel.table_state.update(cx, |state, cx| {
                            let delegate = state.delegate_mut();

                            // Collect deleted row indices and sort in reverse order to remove from bottom up
                            let mut deleted_rows: Vec<usize> = delegate
                                .edit_state
                                .pending_deleted_rows
                                .iter()
                                .copied()
                                .collect();
                            deleted_rows.sort_by(|a, b| b.cmp(a)); // Reverse sort

                            // Remove deleted rows from the table
                            for row_idx in deleted_rows {
                                if row_idx < delegate.rows.len() {
                                    delegate.rows.remove(row_idx);
                                }
                            }

                            delegate.edit_state.clear_edits();
                            state.refresh(cx);
                        });

                        // Update SQL log with queries and success message
                        if let Some(sql_view) = sql_view_response_entity {
                            sql_view.update(cx, |log, cx| {
                                // Log each SQL query
                                for sql_query in &response.sql_queries {
                                    log.append_text(
                                        &blanco_ui::SqlViewMessage::SqlStatement(sql_query.clone()),
                                        cx,
                                    );
                                }
                                // Log summary comment
                                let log_message = format!(
                                    "{}, {} operations, {} rows affected in {}",
                                    time_format::format_current_timestamp(),
                                    response.operations_executed,
                                    response.rows_affected.unwrap_or(0),
                                    time_format::format_duration(
                                        response.duration.as_micros() as i64
                                    )
                                );
                                log.append_text(
                                    &blanco_ui::SqlViewMessage::Comment(log_message),
                                    cx,
                                );
                            });
                        }
                    })
                    .log_err();
            } else {
                // Handle failed operations, show error but keep edits for retry
                if let Some(sql_view) = sql_view_response_entity {
                    let error_message_clone = response.error_message.clone();
                    let sql_queries_clone = response.sql_queries;
                    sql_view.update(cx, |log, cx| {
                        // Log each SQL query that was attempted
                        for sql_query in &sql_queries_clone {
                            log.append_text(
                                &blanco_ui::SqlViewMessage::SqlStatement(sql_query.clone()),
                                cx,
                            );
                        }
                        let error_msg = format!(
                            "✗ Table operations failed: {}",
                            error_message_clone.unwrap_or_else(|| "Unknown error".to_string())
                        );
                        log.append_text(&blanco_ui::SqlViewMessage::Comment(error_msg), cx);
                    });
                }
            }
        })
        .detach();
    }

    /// Rollback all pending changes
    pub fn rollback_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let changes = self
            .table_state
            .read(cx)
            .delegate()
            .edit_state
            .changes
            .clone();

        if changes.is_empty() && !self.has_pending_edits(cx) {
            return;
        }

        // Get table name and connection for events
        let table_name = self
            .table_state
            .read(cx)
            .delegate()
            .table_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        tracing::debug!(
            "rolling back {} change(s) on table {}",
            changes.len(),
            table_name
        );

        // Restore non-insert changes first; remove inserted rows last in reverse
        // index order so removals don't shift the indices of surviving inserts.
        let mut insert_rows: Vec<usize> = Vec::new();
        for change in &changes {
            match change.change_type {
                ChangeType::UpdateCell => {
                    if let Some(col) = change.column_index {
                        let row = change.row_index;
                        if let Some(old_value) = &change.old_value {
                            self.table_state.update(cx, |state, _cx| {
                                if let Some(cell) = state.delegate_mut().get_cell_mut(row, col) {
                                    *cell = Some(old_value.clone());
                                }
                            });
                        }
                    }
                }
                ChangeType::InsertRow => {
                    insert_rows.push(change.row_index);
                }
                ChangeType::DeleteRow => {
                    // Remove deletion mark, the row stays in the table
                    let row = change.row_index;
                    self.table_state.update(cx, |state, _cx| {
                        state
                            .delegate_mut()
                            .edit_state
                            .pending_deleted_rows
                            .remove(&row);
                    });
                }
            }
        }

        insert_rows.sort_unstable();
        for row in insert_rows.into_iter().rev() {
            self.table_state.update(cx, |state, _cx| {
                state.delegate_mut().remove_row(row);
            });
        }

        // Clear all changes
        self.clear_changes(cx);

        cx.notify();
    }
}
