use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div, px,
};
use gpui_component::{
    ActiveTheme, WindowExt as _,
    input::{InputEvent, InputState},
    notification::NotificationType,
    table::{Table, TableDelegate, TableState},
    v_flex,
};

use blanco_core::QueryResult;
use blanco_core::connection_trait::ColumnType;
use database::DatabaseService;

use crate::app::{AddRow, DuplicateRow};
use crate::app_events::AppEvent;
use crate::transformers::CopyHandler;

// Response structure for table operations
#[derive(Debug, Clone)]
pub struct TableOperationResponse {
    pub table_name: String,
    pub connection_id: i64,
    pub success: bool,
    pub rows_affected: Option<u64>,
    pub error_message: Option<String>,
    pub operations_executed: usize,
    pub duration: Duration,
    pub sql_queries: Vec<String>,
}
mod results_table_delegate;
mod table_operations;

// Re-exports from results_table_delegate
pub use results_table_delegate::{ChangeType, ResultsTableDelegate, TableChange};

// Data structures for copy functionality
#[derive(Clone, Debug)]
pub struct SelectedCell {
    #[allow(dead_code)]
    pub row: usize,
    pub col: usize,
    pub value: String,
    pub column_name: Option<String>,
    pub column_type: Option<ColumnType>,
}

#[derive(Clone, Debug)]
pub struct SelectedRow {
    pub row: usize,
    pub cells: Vec<SelectedCell>,
    #[allow(dead_code)]
    pub primary_key_value: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SelectedTableData {
    pub table_name: Option<String>,
    pub columns: Vec<String>,
    pub column_types: Vec<ColumnType>,
    pub selected_rows: Vec<SelectedRow>,
}

pub struct ResultsPanel {
    focus_handle: FocusHandle,
    table_state: Entity<TableState<ResultsTableDelegate>>,
    editing_input: Option<Entity<InputState>>,
    editing_cell: Option<(usize, usize)>,
    copy_handler: CopyHandler,
    _subscriptions: Vec<Subscription>, // Store subscriptions to prevent them from being dropped
}

impl ResultsPanel {
    pub fn new(
        connection_id: i64,
        database_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut delegate = ResultsTableDelegate::default();

        // Set connection ID on delegate if provided
        delegate.set_connection_id(connection_id, database_name);

        let table_state = cx.new(|cx| TableState::new(delegate, window, cx).col_selectable(false));

        // Set up event subscriptions
        let subscriptions = Vec::new();

        Self {
            table_state,
            focus_handle: cx.focus_handle(),
            editing_input: None,
            editing_cell: None,
            copy_handler: CopyHandler::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_query_result(
        &mut self,
        result: QueryResult,
        _connection_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Extract query_text before consuming the result
        let query_text = result.query_text.clone();

        self.table_state.update(cx, |state, cx| {
            // Set the original query for alias resolution
            if let Some(ref query) = query_text {
                state.delegate_mut().set_original_query(query.clone());
            }
            // Move the result into the delegate instead of cloning
            state.delegate_mut().set_query_result(result, window, cx);
            state.refresh(cx);
        });

        // Clear any panel-level editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
    }

    pub fn cancel_current_edit(&mut self, cx: &mut Context<Self>) {
        // Use the delegate's editing state instead of the panel's
        let editing_cell = self.table_state.read(cx).delegate().edit_state.editing_cell;

        if let Some((row, col)) = editing_cell {
            self.cancel_cell_edit(row, col, cx);
        }
    }

    pub fn start_cell_edit(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Get the current cell value
        let current_value = self
            .table_state
            .read(cx)
            .delegate()
            .rows
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_else(|| "".to_string());

        // Check if this is a new row (pending insert) or existing row
        let is_new_row = self
            .table_state
            .read(cx)
            .delegate()
            .edit_state
            .pending_new_rows
            .contains(&row);

        // Create input state for editing with the current cell value
        let input = cx.new(|cx| InputState::new(window, cx).default_value(&current_value));

        // Start editing in the delegate with the input
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            delegate.start_editing_cell(row, col);
            delegate.edit_state.start_editing(row, col, input.clone());

            // Store the original value for existing rows
            if !is_new_row {
                delegate
                    .edit_state
                    .original_values
                    .insert((row, col), current_value.clone());
            }

            // Subscribe to input changes to update edited_values
            Self::subscribe_to_input_events(state, &input, row, col, cx);
        });

        // Focus the input automatically when editing starts
        input.focus_handle(cx).focus(window, cx);

        // Store the editing state in the panel for commit/cancel operations
        self.editing_input = Some(input.clone());
        self.editing_cell = Some((row, col));
    }

    /// Subscribe to input events (blur/change) for a given cell
    fn subscribe_to_input_events(
        _state: &mut TableState<ResultsTableDelegate>,
        input: &Entity<InputState>,
        row: usize,
        col: usize,
        cx: &mut Context<TableState<ResultsTableDelegate>>,
    ) {
        let row_clone = row;
        let col_clone = col;
        cx.subscribe(input, move |table, input, event, cx| {
            if let InputEvent::Change = event {
                let new_text = input.read(cx).text().to_string();
                tracing::info!(
                    "Input change: '{}' at ({}, {})",
                    new_text,
                    row_clone,
                    col_clone
                );
                table
                    .delegate_mut()
                    .edit_state
                    .edited_values
                    .insert((row_clone, col_clone), new_text.clone());

                // Debug: Input change handled in edited_values for commit_cell_edit
                // Note: Can't refresh here due to borrowing issues
            } else if let InputEvent::Blur = event {
                // Handle blur - save current edit to edited_values when input loses focus
                // Get the current editing cell and value
                let editing_cell = table.delegate_mut().edit_state.editing_cell;
                tracing::info!("Blur event triggered for editing_cell: {:?}", editing_cell);

                if let Some((row, col)) = editing_cell {
                    // Ignore blur if the cell is in expanded mode
                    if table.delegate_mut().edit_state.is_expanded(row, col) {
                        tracing::info!(
                            "Blur: ignoring blur for expanded cell at ({}, {})",
                            row,
                            col
                        );
                        return;
                    }

                    let new_value = input.read(cx).text().to_string();
                    tracing::info!("Blur: saving value '{}' at ({}, {})", new_value, row, col);

                    // Commit the cell edit to create a TableChange entry
                    tracing::info!("Blur: committing cell edit at ({}, {})", row, col);
                    table.delegate_mut().commit_cell_edit(row, col);
                    table.refresh(cx);
                    tracing::info!("Blur: cell edit committed and table refreshed");
                } else {
                    tracing::info!("Blur: no editing cell found");
                }
            }
        })
        .detach();
    }

    pub fn commit_cell_edit(
        &mut self,
        row: usize,
        col: usize,
        new_value: String,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let mut committed_value = None;
        let mut old_value = None;
        let mut table_name = None;
        let mut primary_key_value = None;

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();

            // Bail early if table is not editable (no table_name or primary_key_column)
            if !delegate.is_editable() {
                tracing::info!("Table is not editable, bailing commit");
                delegate.edit_state.editing_cell = None;
                return;
            }

            // Get the original value before updating
            old_value = delegate.rows.get(row).and_then(|r| r.get(col)).cloned();
            table_name = delegate.table_name.clone();

            // Get primary key value if we have a primary key column
            if let Some(pk_column) = &delegate.primary_key_column {
                // Find the index of the primary key column
                if let Some(pk_index) = delegate
                    .columns
                    .iter()
                    .position(|col| col.name.as_str() == pk_column)
                {
                    primary_key_value = delegate
                        .rows
                        .get(row)
                        .and_then(|r| r.get(pk_index))
                        .cloned();
                }
            }

            // Update the cell value
            delegate.update_cell_value(row, col, new_value.clone());
            committed_value = delegate.commit_cell_edit(row, col);

            // Track the change for SQL generation (but not for new rows)
            if let (Some(old_val), Some(tbl_name)) = (&old_value, &table_name)
                && old_val != &new_value
            {
                // Check if this is a new row - if so, don't create UPDATE changes
                // New rows should be handled by INSERT operations only
                if !delegate.edit_state.is_new_row(row) {
                    // Get primary key value - if updating the PK column itself, use the original value
                    let primary_key_value = if let Some(pk_column) = &delegate.primary_key_column {
                        // Find the index of the primary key column
                        if let Some(pk_index) = delegate
                            .columns
                            .iter()
                            .position(|col| col.name.as_str() == pk_column)
                        {
                            // If we're updating the primary key column itself, get the original value
                            if pk_index == col {
                                delegate
                                    .edit_state
                                    .original_values
                                    .get(&(row, col))
                                    .cloned()
                            } else {
                                // Otherwise get the current value from the row
                                delegate
                                    .rows
                                    .get(row)
                                    .and_then(|r| r.get(pk_index))
                                    .cloned()
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let primary_key_column = delegate.primary_key_column.clone();

                    // Validate change data before creating
                    let _validation_msg = if primary_key_value.is_none() {
                        "Warning: No primary key value found - change may not be executable"
                    } else if primary_key_column.is_none() {
                        "Warning: No primary key column detected - using first column"
                    } else {
                        "Change validation passed"
                    };

                    let change = TableChange::new(
                        ChangeType::UpdateCell,
                        tbl_name.clone(),
                        row,
                        Some(col),
                        Some(old_val.clone()),
                        Some(new_value.clone()),
                        primary_key_value,
                        None, // No insert_values for UpdateCell operations
                    );
                    delegate.edit_state.add_change(change);

                    // Log the change tracking (this will be visible when user commits)
                    // Note: We defer detailed logging to commit time to avoid cluttering the log
                }
            }

            // Stop editing and clear input
            delegate.edit_state.stop_editing();
            state.refresh(cx);
        });

        // Clear panel editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
        committed_value
    }

    pub fn cancel_cell_edit(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().cancel_cell_edit(row, col);
            // Stop editing and clear input
            state.delegate_mut().edit_state.stop_editing();
            state.refresh(cx);
        });

        // Clear panel editing state
        self.editing_input = None;
        self.editing_cell = None;

        cx.notify();
    }

    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        self.table_state
            .read(cx)
            .delegate()
            .edit_state
            .has_unsaved_changes()
    }

    pub fn get_table_name(&self, cx: &App) -> Option<String> {
        self.table_state
            .read(cx)
            .delegate()
            .get_table_name()
            .map(|s| s.to_string())
    }

    pub fn get_changes(&self, cx: &App) -> Vec<TableChange> {
        let table_read = self.table_state.read(cx);
        let _delegate = table_read.delegate();
        // Get changes directly from edited values in delegate
        let mut changes = Vec::new();
        let table_read = self.table_state.read(cx);
        let delegate = table_read.delegate();

        for ((row, col), new_value) in &delegate.edit_state.edited_values {
            if let Some(original_value) = delegate.edit_state.original_values.get(&(*row, *col)) {
                changes.push(TableChange::new(
                    ChangeType::UpdateCell,
                    delegate.table_name.clone().unwrap_or_default(),
                    *row,
                    Some(*col),
                    Some(original_value.clone()),
                    Some(new_value.clone()),
                    None, // primary_key_value
                    None, // No insert_values for UpdateCell operations
                ));
            }
        }

        tracing::info!(
            "Commit Changes: Got {} changes from edited_values",
            changes.len()
        );
        changes
    }

    pub fn clear_changes(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().edit_state.clear_changes();
            state.refresh(cx);
        });
        cx.notify();
    }

    pub fn commit_all_edits(&mut self, cx: &mut Context<Self>) -> Vec<(usize, usize, String)> {
        let mut committed_changes = Vec::new();

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            let edited_cells: Vec<(usize, usize)> =
                delegate.edit_state.edited_values.keys().cloned().collect();

            for (row, col) in edited_cells {
                if let Some(value) = delegate.commit_cell_edit(row, col) {
                    committed_changes.push((row, col, value));
                }
            }

            // Clear any current editing cell
            if let Some((row, col)) = delegate.edit_state.editing_cell {
                delegate.cancel_cell_edit(row, col);
            }

            state.refresh(cx);
        });

        cx.notify();
        committed_changes
    }

    pub fn cancel_all_edits(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();

            // Cancel current editing cell
            if let Some((row, col)) = delegate.edit_state.editing_cell {
                delegate.cancel_cell_edit(row, col);
            }

            // Clear all edited values
            delegate.edit_state.edited_values.clear();
            delegate.edit_state.original_values.clear();

            state.refresh(cx);
        });

        cx.notify();
    }

    pub fn get_current_editing_cell(&self, cx: &App) -> Option<(usize, usize)> {
        self.table_state.read(cx).delegate().edit_state.editing_cell
    }

    pub fn update_editing_cell_value(
        &mut self,
        row: usize,
        col: usize,
        new_value: String,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().update_cell_value(row, col, new_value);
            state.refresh(cx);
        });
        cx.notify();
    }

    /// Commit all pending changes to the database
    pub fn commit_changes_with_sql_log(
        &mut self,
        _window: &mut Window,
        sql_log: &Entity<blanco_ui::SqlLog>,
        cx: &mut Context<Self>,
    ) {
        self.commit_changes_internal(_window, Some(sql_log), cx)
    }

    #[allow(dead_code)]
    pub fn commit_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.commit_changes_internal(_window, None, cx)
    }

    fn commit_changes_internal(
        &mut self,
        _window: &mut Window,
        sql_log: Option<&Entity<blanco_ui::SqlLog>>,
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
            tracing::info!("Commit Changes: No changes to commit");
            return;
        }

        tracing::info!(
            "Commit Changes: Sending {} operations to async pipeline",
            change_operations.len()
        );

        let delegate = self.table_state.read(cx).delegate();

        // Get table name for logging
        let table_name = delegate
            .table_name
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        // Create clones for different uses
        let change_operations_for_pipeline = change_operations.clone();
        let change_operations_for_logging = change_operations.clone();
        let table_name_for_logging = table_name.clone();
        let _table_name_for_event = table_name.clone();
        let connection_id_for_pipeline = delegate.connection_id;
        let connection_id_for_event = delegate.connection_id;
        let database_name = delegate.database_name.clone();

        tracing::info!("Commit Changes: Starting table operations execution");

        // Spawn background task to execute table operations
        let db_service = DatabaseService::global(cx).clone();
        let _table_entity = self.table_state.clone();
        let _sql_log_entity: Option<Entity<blanco_ui::SqlLog>> = sql_log.cloned();

        let table_operations_task = cx.background_spawn(async move {
            let start_time = std::time::Instant::now();

            // Execute table operations using DatabaseService
            let result = match db_service
                .get_or_create_connection(connection_id_for_pipeline, Some(&database_name))
                .await
            {
                Ok(connection) => {
                    tracing::info!("Got connection for table operations");

                    // Convert table operations to SQL and execute them
                    let mut total_rows_affected = 0u64;
                    let mut operations_executed = 0;
                    let mut error_message = None;
                    let mut success = true;
                    let mut sql_queries = Vec::new();

                    for operation in &change_operations_for_pipeline {
                        tracing::debug!("Got operation {:?}", operation);
                        let sql_query =
                            (operation as &table_operations::TableChangeOperation).to_sql_query();
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
                                error_message = Some(e.to_string());
                                break;
                            }
                        }
                    }

                    TableOperationResponse {
                        table_name: table_name_for_logging.clone(),
                        connection_id: connection_id_for_pipeline,
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
                        table_name: table_name_for_logging.clone(),
                        connection_id: connection_id_for_pipeline,
                        success: false,
                        rows_affected: None,
                        error_message: Some(format!("Connection error: {}", e)),
                        operations_executed: 0,
                        duration: start_time.elapsed(),
                        sql_queries: Vec::new(),
                    }
                }
            };

            tracing::info!(
                "Table operations completed in {:?}, success: {}",
                start_time.elapsed(),
                result.success
            );

            result
        });

        // Spawn async task to handle the response
        let sql_log_response_entity: Option<Entity<blanco_ui::SqlLog>> = sql_log.cloned();
        cx.spawn(async move |entity, cx| {
            let response = table_operations_task.await;

            tracing::info!(
                "Received table operation response: success={}, rows_affected={:?}",
                response.success,
                response.rows_affected
            );

            // Handle successful operations
            if response.success {
                // Clear edits and refresh the table
                let _ = entity.update(cx, |panel, cx| {
                    panel.table_state.update(cx, |state, cx| {
                        state.delegate_mut().edit_state.clear_edits();
                        state.refresh(cx);
                    });

                    // Update SQL log with queries and success message
                    if let Some(sql_log) = sql_log_response_entity {
                        sql_log.update(cx, |log, cx| {
                            // Log each SQL query
                            for sql_query in &response.sql_queries {
                                log.append_text(
                                    &blanco_ui::SqlLogMessage::SqlStatement(sql_query.clone()),
                                    cx,
                                );
                            }
                            // Log summary comment
                            let log_message = format!(
                                "{}, {} operations, {} rows affected in {}",
                                crate::time_format::format_current_timestamp(),
                                response.operations_executed,
                                response.rows_affected.unwrap_or(0),
                                crate::time_format::format_duration(
                                    response.duration.as_millis() as i64
                                )
                            );
                            log.append_text(&blanco_ui::SqlLogMessage::Comment(log_message), cx);
                        });
                    }
                });

                // Emit table operation completed event
                let _ = entity.update(cx, |_, cx| {
                    cx.emit(AppEvent::TableOperationCompleted {
                        table_name: response.table_name,
                        connection_id: response.connection_id,
                        success: true,
                        rows_affected: response.rows_affected,
                        error_message: None,
                        operations_executed: response.operations_executed,
                    });
                });
            } else {
                // Handle failed operations - show error but keep edits for retry
                if let Some(sql_log) = sql_log_response_entity {
                    let error_message_clone = response.error_message.clone();
                    let sql_queries_clone = response.sql_queries.clone();
                    sql_log.update(cx, |log, cx| {
                        // Log each SQL query that was attempted
                        for sql_query in &sql_queries_clone {
                            log.append_text(
                                &blanco_ui::SqlLogMessage::SqlStatement(sql_query.clone()),
                                cx,
                            );
                        }
                        let error_msg = format!(
                            "✗ Table operations failed: {}",
                            error_message_clone.unwrap_or_else(|| "Unknown error".to_string())
                        );
                        log.append_text(&blanco_ui::SqlLogMessage::Comment(error_msg), cx);
                    });
                }

                // Emit table operation completed event with failure
                let _ = entity.update(cx, |_, cx| {
                    cx.emit(AppEvent::TableOperationCompleted {
                        table_name: response.table_name,
                        connection_id: response.connection_id,
                        success: false,
                        rows_affected: None,
                        error_message: response.error_message,
                        operations_executed: response.operations_executed,
                    });
                });
            }
        })
        .detach();
    }

    /// Rollback all pending changes
    pub fn rollback_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let changes = self.get_changes(cx);

        if changes.is_empty() {
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
        let connection_id = self.table_state.read(cx).delegate().connection_id;

        tracing::info!(
            "Rollback Changes: Rolling back {} changes on table {}",
            changes.len(),
            table_name
        );

        // Restore all original values from changes
        for change in &changes {
            match change.change_type {
                ChangeType::UpdateCell => {
                    if let Some(col) = change.column_index {
                        let row = change.row_index;
                        if let Some(old_value) = &change.old_value {
                            self.table_state.update(cx, |state, _cx| {
                                if let Some(cell) = state.delegate_mut().get_cell_mut(row, col) {
                                    *cell = old_value.clone();
                                }
                            });
                        }
                    }
                }
                ChangeType::InsertRow => {
                    // Remove inserted rows (reverse order to maintain indices)
                    let row = change.row_index;
                    self.table_state.update(cx, |state, _cx| {
                        state.delegate_mut().remove_row(row);
                    });
                }
            }
        }

        // Clear all changes
        self.clear_changes(cx);

        // Emit rollback event
        let changes_count = changes.len();
        cx.emit(AppEvent::TableChangesRollback {
            table_name: table_name.clone(),
            connection_id,
            changes_count,
        });

        tracing::info!(
            "Rollback Changes: Successfully rolled back {} changes",
            changes_count
        );
        cx.notify();
    }

    pub fn add_new_row(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            let column_count = delegate.columns.len();

            // Create a new row with empty values
            let new_row: Vec<String> = (0..column_count).map(|_| "".to_string()).collect();
            delegate.rows.push(new_row);

            // Mark this as a pending new row
            let new_row_index = delegate.rows.len() - 1;
            delegate.edit_state.pending_new_rows.push(new_row_index);

            // Track the INSERT change with NULL values (excluding row number and primary key columns)
            if let Some(table_name) = &delegate.table_name {
                // For new rows, exclude primary key to avoid UPDATE/INSERT confusion
                let column_names = delegate.get_insert_column_names(true); // exclude_primary_key = true
                // Use None for all NULL values (new row starts with all NULLs)
                let values_vec = column_names
                    .iter()
                    .map(|_| None)
                    .collect::<Vec<Option<String>>>();

                let change = TableChange::new(
                    ChangeType::InsertRow,
                    table_name.clone(),
                    new_row_index,
                    None,
                    None,
                    None,             // No single new_value for insert operations
                    None,             // No primary key value for new rows
                    Some(values_vec), // Use insert_values parameter instead
                );
                delegate.edit_state.add_change(change);
            }

            state.refresh(cx);
        });
        cx.notify();
    }

    pub fn duplicate_row(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            let selected_rows = state.selected_rows().clone();

            // Check if the row exists
            for row_ix in selected_rows {
                let delegate = state.delegate_mut();
                if let Some(row_to_duplicate) = delegate.rows.get(row_ix).cloned() {
                    // Add the duplicated row
                    delegate.rows.push(row_to_duplicate.clone());

                    // Mark this as a pending new row
                    let new_row_index = delegate.rows.len() - 1;
                    delegate.edit_state.pending_new_rows.push(new_row_index);

                    // Track the INSERT change with proper column values (excluding row number and primary key columns)
                    if let Some(table_name) = &delegate.table_name {
                        // For new rows (duplicated rows), exclude primary key to avoid UPDATE/INSERT confusion
                        let _column_names = delegate.get_insert_column_names(true); // exclude_primary_key = true
                        let values = delegate.get_insert_values(new_row_index, true); // exclude_primary_key = true
                        let values_vec: Vec<Option<String>> = values
                            .iter()
                            .map(|val| {
                                if val == "NULL" {
                                    None // Use None for actual NULL values
                                } else {
                                    Some(val.clone()) // Keep value (including empty strings)
                                }
                            })
                            .collect();

                        let change = TableChange::new(
                            ChangeType::InsertRow,
                            table_name.clone(),
                            new_row_index,
                            None,
                            None,
                            None, // No single new_value for insert operations
                            None, // No primary key value for new rows
                            Some(values_vec.clone()), // Use insert_values parameter instead
                        );
                        delegate.edit_state.add_change(change);
                    }

                    state.refresh(cx);
                }
            }
        });
        cx.notify();
    }

    /// Handle table operation completion event
    #[allow(dead_code)]
    pub fn handle_table_operation_completed(
        &mut self,
        _table_name: &str,
        success: bool,
        rows_affected: Option<u64>,
        error_message: Option<String>,
        operations_executed: usize,
        cx: &mut Context<Self>,
    ) {
        if success {
            tracing::info!(
                "Table operation completed successfully: {} operations, {} rows affected",
                operations_executed,
                rows_affected.unwrap_or(0)
            );

            // Clear edit state after successful commit
            self.table_state.update(cx, |state, cx| {
                state.delegate_mut().edit_state.clear_edits();
                state.refresh(cx);
            });

            // Optionally refresh the data or show a success message
            cx.notify();
        } else {
            tracing::error!(
                "Table operation failed: {}",
                error_message.unwrap_or_else(|| "Unknown error".to_string())
            );

            // Keep the edit state so user can retry or fix issues
            // Don't refresh the table to preserve user's changes
        }
    }

    // Copy and selection action handlers

    fn on_copy_as_csv(
        &mut self,
        _action: &crate::app::CopyAsCSV,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as CSV: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler.copy_as_format(&selected_data, "csv", cx);
    }

    fn on_copy_as_json(
        &mut self,
        _action: &crate::app::CopyAsJSON,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as JSON: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler.copy_as_format(&selected_data, "json", cx);
    }

    fn on_copy_as_sql(
        &mut self,
        _action: &crate::app::CopyAsSQL,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as SQL: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler.copy_as_format(&selected_data, "sql", cx);
    }

    fn on_copy_as_markdown(
        &mut self,
        _action: &crate::app::CopyAsMarkdown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            tracing::error!("Failed to copy as Markdown: No rows selected for copying");
            return;
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler
            .copy_as_format(&selected_data, "markdown", cx);
    }

    fn on_add_row(&mut self, _action: &AddRow, _window: &mut Window, cx: &mut Context<Self>) {
        self.add_new_row(cx);
    }

    fn on_duplicate_row(
        &mut self,
        _action: &DuplicateRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.duplicate_row(cx);
    }

    fn on_export_as_csv(
        &mut self,
        _action: &crate::app::ExportAsCSV,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("csv", window, cx);
    }

    fn on_export_as_json(
        &mut self,
        _action: &crate::app::ExportAsJSON,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("json", window, cx);
    }

    fn on_export_as_sql(
        &mut self,
        _action: &crate::app::ExportAsSQL,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("sql", window, cx);
    }

    fn on_export_as_markdown(
        &mut self,
        _action: &crate::app::ExportAsMarkdown,
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

        // Get file extension
        let extension = match format {
            "csv" => "csv",
            "json" => "json",
            "sql" => "sql",
            "markdown" => "md",
            _ => "txt",
        };

        // Suggest default filename
        let timestamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let default_filename = format!("{}_{}.{}", table_name, timestamp, extension);

        // Get home directory as default directory
        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        // Prompt for file save location using prompt_for_new_path
        let path = cx.prompt_for_new_path(&home_dir, Some(&default_filename));

        let format_owned = format.to_string();
        let table_name_for_sql = table_name.clone();

        cx.spawn_in(window, async move |entity, cx| {
            // Await the path result - it's Result<Result<Option<PathBuf>>, Canceled>
            let outer_result = path.await;

            // Handle the outer Result (Canceled or inner Result)
            let inner_result = match outer_result {
                Ok(inner) => inner,
                Err(_) => return, // Canceled
            };

            // Extract the path from Result<Option<PathBuf>>
            let file_path = match inner_result {
                Ok(Some(path_buf)) => path_buf,
                _ => return, // User cancelled or error
            };

            // Get transformer
            let transformer: Box<dyn crate::transformers::DataTransformer> =
                match format_owned.as_str() {
                    "csv" => Box::new(crate::transformers::CsvTransformer),
                    "json" => Box::new(crate::transformers::JsonTransformer::new()),
                    "sql" => Box::new(crate::transformers::SqlTransformer::with_table_name(
                        table_name_for_sql.clone(),
                    )),
                    "markdown" => Box::new(crate::transformers::MarkdownTransformer),
                    _ => {
                        tracing::error!("Unknown format: {}", format_owned);
                        return;
                    }
                };

            // Create export service
            let export_service = crate::export_service::ExportService::new();

            // Execute export
            match export_service
                .export_selected_data(&selected_data, transformer.as_ref(), &file_path)
                .await
            {
                Ok(result) => {
                    match result {
                        crate::export_service::ExportResult::Success {
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
                            // Show success notification via entity update
                            let _ = entity.update_in(cx, |_panel, window, cx| {
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
                            });
                        }
                        crate::export_service::ExportResult::Error { message } => {
                            tracing::error!("Export failed: {}", message);
                            let _ = entity.update_in(cx, |_panel, window, cx| {
                                window.push_notification(
                                    (
                                        NotificationType::Error,
                                        SharedString::from(format!("Export failed: {}", message)),
                                    ),
                                    cx,
                                );
                            });
                        }
                        crate::export_service::ExportResult::Cancelled => {
                            tracing::info!("Export cancelled by user");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Export failed: {}", e);
                    let _ = entity.update_in(cx, |_panel, window, cx| {
                        window.push_notification(
                            (
                                NotificationType::Error,
                                SharedString::from(format!("Export failed: {}", e)),
                            ),
                            cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    pub fn get_selected_data_for_rows(
        &self,
        selected_rows: &HashSet<usize>,
        delegate: &ResultsTableDelegate,
    ) -> SelectedTableData {
        let mut selected_rows_data = Vec::new();

        // Collect selected row data
        for &row in selected_rows {
            if let Some(row_data) = delegate.rows.get(row) {
                let cells: Vec<SelectedCell> = row_data
                    .iter()
                    .enumerate()
                    .filter(|&(col, _)| col > 0) // Skip row number column
                    .map(|(col, value)| SelectedCell {
                        row,
                        col: col - 1, // Adjust for row number column
                        value: value.clone(),
                        column_name: delegate.columns.get(col).map(|c| c.name.to_string()),
                        column_type: delegate.column_types.get(col - 1).cloned(), // Adjust for row number column
                    })
                    .collect();

                selected_rows_data.push(SelectedRow {
                    row,
                    cells,
                    primary_key_value: None, // TODO: Extract primary key if needed
                });
            }
        }

        SelectedTableData {
            table_name: delegate.table_name.clone(),
            columns: delegate
                .columns
                .iter()
                .skip(1)
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>(),
            column_types: delegate.column_types.clone(),
            selected_rows: selected_rows_data,
        }
    }
}

impl EventEmitter<AppEvent> for ResultsPanel {}
impl Focusable for ResultsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ResultsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check for pending inline edits
        if let Some((row, col)) = self.table_state.read(cx).delegate().pending_edit_cell {
            // Clear the pending edit and start editing
            self.table_state.update(cx, |state, _cx| {
                state.delegate_mut().pending_edit_cell = None;
            });

            self.start_cell_edit(row, col, window, cx);
        }

        let _row_count = self.table_state.read(cx).delegate().rows_count(cx);
        let _has_unsaved_changes = self.has_unsaved_changes(cx);
        let _table_name = self.get_table_name(cx);
        let _is_editable = self.table_state.read(cx).delegate().is_editable();

        v_flex()
            .size_full()
            .border_t_1()
            .border_color(cx.theme().border)
            // Handle copy and selection actions
            .on_action(cx.listener(Self::on_copy_as_csv))
            .on_action(cx.listener(Self::on_copy_as_json))
            .on_action(cx.listener(Self::on_copy_as_sql))
            .on_action(cx.listener(Self::on_copy_as_markdown))
            .on_action(cx.listener(Self::on_export_as_csv))
            .on_action(cx.listener(Self::on_export_as_json))
            .on_action(cx.listener(Self::on_export_as_sql))
            .on_action(cx.listener(Self::on_export_as_markdown))
            .on_action(cx.listener(Self::on_add_row))
            .on_action(cx.listener(Self::on_duplicate_row))
            // The table component (table should have built-in scrolling)
            .child(
                div()
                    .id("results-table")
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .flex_1() // Allow table to fill available space
                    .overflow_hidden()
                    .min_h(px(200.0)) // Minimum height for table
                    .child(Table::new(&self.table_state).bordered(false)),
            )
    }
}
