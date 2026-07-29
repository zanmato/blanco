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

    fn commit_changes_internal(
        &mut self,
        _window: &mut Window,
        sql_view: Option<&Entity<blanco_ui::SqlView>>,
        cx: &mut Context<Self>,
    ) {
        if self.commit_in_progress {
            return;
        }

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

        self.commit_in_progress = true;
        cx.notify();

        tracing::debug!("committing {} table operation(s)", change_operations.len());

        let delegate = self.table_state.read(cx).delegate();

        let change_operations_for_pipeline = change_operations;
        let connection_id_for_pipeline = delegate.connection_id;
        let database_name = delegate.database_name.clone();
        let database_type = delegate.db_type.unwrap_or(self.db_type);

        // Spawn background task to execute table operations
        let db_service = DatabaseService::global(cx).clone();
        let table_entity = self.table_state.clone();

        let table_operations_task = cx.background_spawn(async move {
            let start_time = std::time::Instant::now();

            // Convert table operations to SQL up front so the log reflects
            // exactly what the transaction attempted, regardless of outcome.
            let sql_queries: Vec<String> = change_operations_for_pipeline
                .iter()
                .map(|operation| (operation as &TableChangeOperation).to_sql_query(database_type))
                .collect();

            // Execute table operations using DatabaseService. The whole batch
            // runs in one transaction on backends that support DML transactions,
            // so a mid-batch failure applies nothing and the kept edits can be
            // retried without double-applying.
            let result = match db_service
                .get_or_create_connection(connection_id_for_pipeline, Some(&database_name))
                .await
            {
                Ok(connection) => match connection
                    .execute_operations_transactional(&sql_queries, Some(&database_name))
                    .await
                {
                    Ok(outcome) => TableOperationResponse {
                        success: true,
                        rows_affected: Some(outcome.rows_affected),
                        error_message: None,
                        operations_executed: outcome.operations_executed,
                        duration: start_time.elapsed(),
                        sql_queries,
                        applied: outcome.operations_executed,
                    },
                    Err(failure) => {
                        tracing::error!(
                            "table operations failed after {} applied: {:#}",
                            failure.applied,
                            failure.error
                        );
                        TableOperationResponse {
                            success: false,
                            rows_affected: None,
                            error_message: Some(format!("{:#}", failure.error)),
                            operations_executed: failure.applied,
                            duration: start_time.elapsed(),
                            sql_queries,
                            applied: failure.applied,
                        }
                    }
                },
                Err(e) => {
                    tracing::error!("Failed to get connection for table operations: {}", e);
                    TableOperationResponse {
                        success: false,
                        rows_affected: None,
                        error_message: Some(format!("Connection error: {}", e)),
                        operations_executed: 0,
                        duration: start_time.elapsed(),
                        sql_queries: Vec::new(),
                        applied: 0,
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
                        table_entity.update(cx, |state, cx| {
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
                        panel.commit_in_progress = false;
                        cx.notify();

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
                let partial = response.applied > 0;
                if partial {
                    entity
                        .update(cx, |panel, cx| {
                            table_entity.update(cx, |state, cx| {
                                state.delegate_mut().edit_state.clear_all();
                                state.refresh(cx);
                            });
                            panel.commit_in_progress = false;
                            cx.notify();
                        })
                        .log_err();
                } else {
                    entity
                        .update(cx, |panel, cx| {
                            panel.commit_in_progress = false;
                            cx.notify();
                        })
                        .log_err();
                }

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
                        let error_msg = if partial {
                            format!(
                                "✗ Table operations partially applied ({} of {} committed, backend is not transactional): {}. Re-run the query to see current state.",
                                response.applied,
                                sql_queries_clone.len(),
                                error_message_clone.unwrap_or_else(|| "Unknown error".to_string())
                            )
                        } else {
                            format!(
                                "✗ Table operations failed (rolled back, no changes applied): {}",
                                error_message_clone.unwrap_or_else(|| "Unknown error".to_string())
                            )
                        };
                        log.append_text(&blanco_ui::SqlViewMessage::Comment(error_msg), cx);
                    });
                }
            }
        })
        .detach();
    }

    /// Rollback all pending changes
    pub fn rollback_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_in_progress {
            return;
        }

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
