use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use blanco_ui::IconName;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, InputState},
    notification::NotificationType,
    table::{DataTable, TableEvent, TableState},
    v_flex,
};

use blanco_core::QueryResult;
use blanco_core::connection_trait::ColumnType;
use database::DatabaseService;

use crate::app::{
    AddRow, CopyAsCSV, CopyAsJSON, CopyAsMarkdown, CopyAsSQL, CopyAsTSV, CopyAsVALUES, DeleteRow,
    DuplicateRow, ExportAsCSV, ExportAsJSON, ExportAsMarkdown, ExportAsSQL, ExportAsTSV,
    SetCellNull,
};
use crate::export::service::{ExportResult, ExportService};
use crate::result_ext::ResultExt;
use crate::time_format;
use crate::transformers::{
    CopyHandler, CsvTransformer, DataTransformer, JsonTransformer, MarkdownTransformer,
    SqlTransformer,
};

mod cell_edit_state;
mod foreign_key_popover;
mod results_table_delegate;
mod table_operations;

// Re-exports
pub use cell_edit_state::{ChangeType, TableChange};
pub use results_table_delegate::ResultsTableDelegate;

// Response structure for table operations
#[derive(Debug, Clone)]
pub struct TableOperationResponse {
    pub success: bool,
    pub rows_affected: Option<u64>,
    pub error_message: Option<String>,
    pub operations_executed: usize,
    pub duration: Duration,
    pub sql_queries: Vec<String>,
}

// Data structures for copy functionality
#[derive(Clone, Debug)]
pub struct SelectedCell {
    pub col: usize,
    pub value: Option<String>,
    pub column_name: Option<String>,
    pub column_type: Option<ColumnType>,
}

#[derive(Clone, Debug)]
pub struct SelectedRow {
    pub row: usize,
    pub cells: Vec<SelectedCell>,
}

#[derive(Clone, Debug, Default)]
pub struct SelectedTableData {
    pub table_name: Option<String>,
    pub db_type: Option<database::DatabaseType>,
    pub columns: Vec<String>,
    pub selected_rows: Vec<SelectedRow>,
}

/// One materialized result-set rendered as a sub-tab inside `ResultsPanel`.
/// A query script with N statements produces N `ResultTab`s; each owns its own
/// `TableState` so edit state and selection stay isolated per tab.
pub struct ResultTab {
    pub title: SharedString,
    pub pinned: bool,
    pub table_state: Entity<TableState<ResultsTableDelegate>>,
    pub _subscriptions: Vec<Subscription>,
}

pub struct ResultsPanel {
    focus_handle: FocusHandle,
    // Cached handle to `result_tabs[active_tab].table_state`. The 30+ existing
    // call sites keep reading `self.table_state`; whenever `active_tab` moves
    // we rebind this so they always see the current tab's state.
    table_state: Entity<TableState<ResultsTableDelegate>>,
    result_tabs: Vec<ResultTab>,
    active_tab: usize,
    /// Whether any query has produced results yet. Used to keep the tab strip
    /// hidden until there's something to actually render.
    has_results: bool,
    connection_id: i64,
    database_name: String,
    db_type: database::DatabaseType,
    editing_input: Option<Entity<InputState>>,
    editing_cell: Option<(usize, usize)>,
    copy_handler: CopyHandler,
    _subscriptions: Vec<Subscription>, // Store subscriptions to prevent them from being dropped
}

impl ResultsPanel {
    pub fn new(
        connection_id: i64,
        database_name: &str,
        db_type: database::DatabaseType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_tab = Self::build_result_tab(
            connection_id,
            database_name,
            db_type,
            SharedString::from("Result"),
            window,
            cx,
        );
        let table_state = initial_tab.table_state.clone();

        Self {
            table_state,
            result_tabs: vec![initial_tab],
            active_tab: 0,
            has_results: false,
            connection_id,
            database_name: database_name.to_string(),
            db_type,
            focus_handle: cx.focus_handle(),
            editing_input: None,
            editing_cell: None,
            copy_handler: CopyHandler::new(),
            _subscriptions: vec![],
        }
    }

    fn build_result_tab(
        connection_id: i64,
        database_name: &str,
        db_type: database::DatabaseType,
        title: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ResultTab {
        let mut delegate = ResultsTableDelegate::default();
        delegate.set_connection_id(connection_id, database_name, db_type);

        let table_state = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .row_selectable(true)
                .cell_selectable(true)
                .col_selectable(false)
        });

        let subscription = cx.subscribe_in(
            &table_state,
            window,
            move |panel, table_state, event: &TableEvent, window, cx| {
                if let TableEvent::DoubleClickedCell(row_ix, col_ix) = event {
                    // Only handle events on the currently active tab.
                    if panel.table_state.entity_id() != table_state.entity_id() {
                        return;
                    }
                    panel.table_state.update(cx, |state, _cx| {
                        state.delegate_mut().clear_selection();
                    });
                    panel.start_cell_edit(*row_ix, *col_ix, window, cx);
                }
            },
        );

        ResultTab {
            title,
            pinned: false,
            table_state,
            _subscriptions: vec![subscription],
        }
    }

    /// Replace all unpinned tabs with one tab per provided result. The first
    /// newly added tab becomes active; pinned tabs are preserved at the front
    /// of the strip.
    pub fn set_query_results(
        &mut self,
        results: Vec<QueryResult>,
        connection_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Drop any tabs the user hasn't pinned.
        self.result_tabs.retain(|tab| tab.pinned);

        if results.is_empty() {
            // Nothing new to show; if everything got cleared, leave a fresh
            // empty tab so the panel still has something to render.
            if self.result_tabs.is_empty() {
                let placeholder = Self::build_result_tab(
                    self.connection_id,
                    &self.database_name.clone(),
                    self.db_type,
                    SharedString::from("Result"),
                    window,
                    cx,
                );
                self.table_state = placeholder.table_state.clone();
                self.result_tabs.push(placeholder);
                self.active_tab = 0;
            }
            cx.notify();
            return;
        }

        let first_new_index = self.result_tabs.len();
        let total = results.len();
        for (i, result) in results.into_iter().enumerate() {
            let title = if total > 1 {
                SharedString::from(format!("Result {}", i + 1))
            } else {
                SharedString::from("Result")
            };
            let tab = Self::build_result_tab(
                self.connection_id,
                &self.database_name.clone(),
                self.db_type,
                title,
                window,
                cx,
            );
            let query_text = result.query_text.clone();
            tab.table_state.update(cx, |state, cx| {
                if let Some(ref q) = query_text {
                    state.delegate_mut().set_original_query(q.clone());
                }
                state.clear_selection(cx);
                state.delegate_mut().set_query_result(result, window, cx);
                state.refresh(cx);
            });
            self.result_tabs.push(tab);
        }

        self.active_tab = first_new_index;
        self.table_state = self.result_tabs[self.active_tab].table_state.clone();
        self.has_results = true;
        self.editing_input = None;
        self.editing_cell = None;
        let _ = connection_id; // accepted for API parity with single-result path
        cx.notify();
    }

    pub fn activate_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.result_tabs.len() || index == self.active_tab {
            return;
        }
        self.active_tab = index;
        self.table_state = self.result_tabs[index].table_state.clone();
        self.editing_input = None;
        self.editing_cell = None;
        cx.notify();
    }

    pub fn toggle_pin(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.result_tabs.get_mut(index) {
            tab.pinned = !tab.pinned;
            cx.notify();
        }
    }

    pub fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.result_tabs.len() {
            return;
        }
        self.result_tabs.remove(index);
        if self.result_tabs.is_empty() {
            let placeholder = Self::build_result_tab(
                self.connection_id,
                &self.database_name.clone(),
                self.db_type,
                SharedString::from("Result"),
                window,
                cx,
            );
            self.table_state = placeholder.table_state.clone();
            self.result_tabs.push(placeholder);
            self.active_tab = 0;
        } else {
            if self.active_tab >= self.result_tabs.len() {
                self.active_tab = self.result_tabs.len() - 1;
            } else if index < self.active_tab {
                self.active_tab -= 1;
            }
            self.table_state = self.result_tabs[self.active_tab].table_state.clone();
        }
        self.editing_input = None;
        self.editing_cell = None;
        cx.notify();
    }

    #[cfg(test)]
    pub fn table_state(&self) -> &Entity<TableState<ResultsTableDelegate>> {
        &self.table_state
    }

    #[allow(dead_code)]
    pub fn set_query_result(
        &mut self,
        result: QueryResult,
        connection_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_query_results(vec![result], connection_id, window, cx);
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
            .unwrap_or(None);

        // Check if this is a new row (pending insert) or existing row
        let is_new_row = self
            .table_state
            .read(cx)
            .delegate()
            .edit_state
            .pending_new_rows
            .contains(&row);

        // Create input state for editing with the current cell value
        let display_value = current_value.clone().unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(&display_value));

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
                table
                    .delegate_mut()
                    .edit_state
                    .edited_values
                    .insert((row_clone, col_clone), Some(new_text.clone()));

                // Debug: Input change handled in edited_values for commit_cell_edit
                // Note: Can't refresh here due to borrowing issues
            } else if let InputEvent::Blur = event {
                // Save current edit to edited_values when input loses focus
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
        let mut old_value: Option<String> = None;
        let mut table_name = None;

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();

            if !delegate.is_editable() {
                tracing::info!("Table is not editable, bailing commit");
                delegate.edit_state.editing_cell = None;
                return;
            }

            old_value = delegate
                .rows
                .get(row)
                .and_then(|r| r.get(col))
                .and_then(|v| v.clone());
            table_name = delegate.table_name.clone();

            delegate.update_cell_value(row, col, Some(new_value.clone()));
            committed_value = delegate.commit_cell_edit(row, col).and_then(|v| v);

            if let (Some(old_val), Some(tbl_name)) = (&old_value, &table_name)
                && old_val != &new_value
                && !delegate.edit_state.is_new_row(row)
            {
                let primary_key_values: Vec<(String, Option<String>)> = delegate
                    .primary_key_column_names()
                    .into_iter()
                    .filter_map(|pk_name: &str| {
                        let pk_col_index = delegate
                            .columns
                            .iter()
                            .skip(1)
                            .position(|c| c.name.as_str() == pk_name)?;
                        let full_index = pk_col_index + 1;
                        let value = if full_index == col {
                            delegate
                                .edit_state
                                .original_values
                                .get(&(row, col))
                                .and_then(|v| v.clone())
                        } else {
                            delegate
                                .rows
                                .get(row)
                                .and_then(|r| r.get(full_index))
                                .and_then(|v| v.clone())
                        };
                        Some((pk_name.to_string(), value))
                    })
                    .collect();

                let all_present = primary_key_values
                    .iter()
                    .all(|(_, v): &(String, Option<String>)| v.is_some());
                if !all_present {
                    tracing::warn!(
                        "Skipping change: missing primary key value(s) for row {}",
                        row
                    );
                } else {
                    let change = TableChange::new(
                        ChangeType::UpdateCell,
                        tbl_name.clone(),
                        row,
                        Some(col),
                        Some(old_val.clone()),
                        Some(new_value.clone()),
                        primary_key_values,
                        None,
                    );
                    delegate.edit_state.add_change(change);
                }
            }

            delegate.edit_state.stop_editing();
            state.refresh(cx);
        });

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
                    original_value.clone(),
                    new_value.clone(),
                    Vec::new(), // primary_key_values
                    None,       // No insert_values for UpdateCell operations
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

    pub fn commit_all_edits(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<(usize, usize, Option<String>)> {
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

    /// Whether there are any uncommitted cell edits, pending new rows, or pending deletions.
    pub fn has_pending_edits(&self, cx: &App) -> bool {
        let edit_state = &self.table_state.read(cx).delegate().edit_state;
        !edit_state.edited_values.is_empty()
            || !edit_state.pending_new_rows.is_empty()
            || !edit_state.pending_deleted_rows.is_empty()
    }

    pub fn update_editing_cell_value(
        &mut self,
        row: usize,
        col: usize,
        new_value: String,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |state, cx| {
            state
                .delegate_mut()
                .update_cell_value(row, col, Some(new_value));
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

        let change_operations_for_pipeline = change_operations.clone();
        let connection_id_for_pipeline = delegate.connection_id;
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
                                    time_format::format_current_timestamp(),
                                    response.operations_executed,
                                    response.rows_affected.unwrap_or(0),
                                    time_format::format_duration(
                                        response.duration.as_millis() as i64
                                    )
                                );
                                log.append_text(
                                    &blanco_ui::SqlLogMessage::Comment(log_message),
                                    cx,
                                );
                            });
                        }
                    })
                    .log_err();
            } else {
                // Handle failed operations, show error but keep edits for retry
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
                                    *cell = Some(old_value.clone());
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

        // Clear all changes
        self.clear_changes(cx);

        let changes_count = changes.len();
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
            let new_row: Vec<Option<String>> = (0..column_count).map(|_| None).collect();
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
                    Vec::new(),       // No primary key values for new rows
                    Some(values_vec), // Use insert_values parameter instead
                );
                delegate.edit_state.add_change(change);
            }

            state.refresh(cx);
        });
        cx.notify();
    }

    pub fn duplicate_row(&mut self, cx: &mut Context<Self>) {
        let selected_rows = self.table_state.read(cx).selected_rows().clone();
        for row_ix in selected_rows {
            self.duplicate_row_with_row(row_ix, cx);
        }
    }

    pub fn duplicate_row_with_row(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
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
                    let values_vec = delegate.get_insert_values(new_row_index, true); // exclude_primary_key = true

                    let change = TableChange::new(
                        ChangeType::InsertRow,
                        table_name.clone(),
                        new_row_index,
                        None,
                        None,
                        None,             // No single new_value for insert operations
                        Vec::new(),       // No primary key values for new rows
                        Some(values_vec), // Use insert_values parameter instead
                    );
                    delegate.edit_state.add_change(change);
                }

                state.refresh(cx);
            }
        });
        cx.notify();
    }

    pub fn delete_row(&mut self, cx: &mut Context<Self>) {
        let selected_rows = self.table_state.read(cx).selected_rows().clone();
        for row_ix in selected_rows {
            self.delete_row_with_row(row_ix, cx);
        }
    }

    pub fn delete_row_with_row(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();

            // Skip if already marked for deletion
            if delegate.edit_state.is_row_deleted(row_ix) {
                return;
            }

            // Skip if this is a new row (just remove it instead)
            if delegate.edit_state.is_new_row(row_ix) {
                delegate.remove_row(row_ix);
                delegate.edit_state.remove_new_row(row_ix);
                state.refresh(cx);
                return;
            }

            let pk_values: Vec<(String, Option<String>)> = delegate
                .primary_key_column_names()
                .into_iter()
                .filter_map(|pk_name| {
                    let pk_col_idx = delegate
                        .columns
                        .iter()
                        .skip(1)
                        .position(|c| c.name.as_str() == pk_name)?;
                    let display_col = pk_col_idx + 1;
                    let value = delegate
                        .rows
                        .get(row_ix)
                        .and_then(|row| row.get(display_col))
                        .and_then(|v| v.clone());
                    Some((pk_name.to_string(), value))
                })
                .collect();

            // Mark the row as deleted
            delegate.edit_state.pending_deleted_rows.insert(row_ix);

            // Track the DELETE change
            if let Some(table_name) = &delegate.table_name {
                let change = TableChange::new(
                    ChangeType::DeleteRow,
                    table_name.clone(),
                    row_ix,
                    None,
                    None,
                    None,
                    pk_values,
                    None,
                );
                delegate.edit_state.add_change(change);
            }

            state.refresh(cx);
        });
        cx.notify();
    }

    // Copy and selection action handlers

    fn on_copy_as_csv(
        &mut self,
        _action: &CopyAsCSV,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let mut selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            if let Some(cell) = table_state.selected_cell() {
                selected_rows.insert(cell.0);
            } else {
                tracing::error!("Failed to copy as CSV: No rows selected for copying");
                return;
            }
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler.copy_as_format(&selected_data, "csv", cx);
    }

    fn on_copy_as_tsv(
        &mut self,
        _action: &CopyAsTSV,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let mut selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            if let Some(cell) = table_state.selected_cell() {
                selected_rows.insert(cell.0);
            } else {
                tracing::error!("Failed to copy as TSV: No rows selected for copying");
                return;
            }
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler.copy_as_format(&selected_data, "tsv", cx);
    }

    fn on_copy_as_json(
        &mut self,
        _action: &CopyAsJSON,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let mut selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            if let Some(cell) = table_state.selected_cell() {
                selected_rows.insert(cell.0);
            } else {
                tracing::error!("Failed to copy as JSON: No rows selected for copying");
                return;
            }
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler.copy_as_format(&selected_data, "json", cx);
    }

    fn on_copy_as_sql(
        &mut self,
        _action: &CopyAsSQL,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let mut selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            if let Some(cell) = table_state.selected_cell() {
                selected_rows.insert(cell.0);
            } else {
                tracing::error!("Failed to copy as SQL: No rows selected for copying");
                return;
            }
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler.copy_as_format(&selected_data, "sql", cx);
    }

    fn on_copy_as_values(
        &mut self,
        _action: &CopyAsVALUES,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let mut selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            if let Some(cell) = table_state.selected_cell() {
                selected_rows.insert(cell.0);
            } else {
                tracing::error!("Failed to copy as VALUES: No rows selected for copying");
                return;
            }
        }

        let selected_data = self.get_selected_data_for_rows(&selected_rows, delegate);

        self.copy_handler
            .copy_as_format(&selected_data, "values", cx);
    }

    fn on_copy_as_markdown(
        &mut self,
        _action: &CopyAsMarkdown,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_state = self.table_state.read(cx);
        let mut selected_rows = table_state.selected_rows().clone();
        let delegate = table_state.delegate();

        if selected_rows.is_empty() {
            if let Some(cell) = table_state.selected_cell() {
                selected_rows.insert(cell.0);
            } else {
                tracing::error!("Failed to copy as Markdown: No rows selected for copying");
                return;
            }
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
        action: &DuplicateRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.duplicate_row_with_row(action.row, cx);
    }

    fn on_delete_row(&mut self, action: &DeleteRow, _window: &mut Window, cx: &mut Context<Self>) {
        self.delete_row_with_row(action.row, cx);
    }

    fn on_set_cell_null(
        &mut self,
        action: &SetCellNull,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();

            // Store original value before editing
            if let Some(cell_value) = delegate
                .rows
                .get(action.row)
                .and_then(|r| r.get(action.col))
            {
                delegate
                    .edit_state
                    .original_values
                    .entry((action.row, action.col))
                    .or_insert_with(|| cell_value.clone());
            }

            // Set the cell to NULL
            delegate.update_cell_value(action.row, action.col, None);
            delegate.commit_cell_edit(action.row, action.col);
            state.refresh(cx);
        });

        cx.notify();
    }

    fn on_export_as_csv(
        &mut self,
        _action: &ExportAsCSV,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("csv", window, cx);
    }

    fn on_export_as_tsv(
        &mut self,
        _action: &ExportAsTSV,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("tsv", window, cx);
    }

    fn on_export_as_json(
        &mut self,
        _action: &ExportAsJSON,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("json", window, cx);
    }

    fn on_export_as_sql(
        &mut self,
        _action: &ExportAsSQL,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_selected_as("sql", window, cx);
    }

    fn on_export_as_markdown(
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
        let db_type = selected_data
            .db_type
            .unwrap_or(database::DatabaseType::PostgreSQL);

        cx.spawn_in(window, async move |entity, cx| {
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

            // Create export service
            let export_service = ExportService::new();

            // Execute export
            match export_service
                .export_selected_data(&selected_data, transformer.as_ref(), &file_path)
                .await
            {
                Ok(result) => {
                    match result {
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
                            // Show success notification via entity update
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
                    }
                }
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
                        col: col - 1, // Adjust for row number column
                        value: value.clone(),
                        column_name: delegate.columns.get(col).map(|c| c.name.to_string()),
                        column_type: delegate.column_types.get(col - 1).cloned(), // Adjust for row number column
                    })
                    .collect();

                selected_rows_data.push(SelectedRow { row, cells });
            }
        }

        SelectedTableData {
            table_name: delegate.table_name.clone(),
            db_type: delegate.db_type,
            columns: delegate
                .columns
                .iter()
                .skip(1)
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>(),
            selected_rows: selected_rows_data,
        }
    }
}

impl Focusable for ResultsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ResultsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let theme = cx.theme();
        let border_color = theme.border;
        let muted_fg = theme.muted_foreground;
        let active_fg = theme.table_head_foreground;
        // Strip mimics the table itself: solid `theme.table` background, then
        // each tab paints `theme.table_head` (active) or `theme.title_bar`
        // (inactive) on top. The active tab composites identically to the
        // real column header because the underlying surface matches.
        let strip_bg = theme.table;
        let tab_count = self.result_tabs.len();
        let show_strip = self.has_results;
        let show_close = tab_count > 1;
        let mut strip = gpui_component::h_flex()
            .id("result-tabs-strip")
            .w_full()
            .text_sm()
            .border_b_1()
            .border_color(border_color)
            .bg(strip_bg);
        for (idx, tab) in self.result_tabs.iter().enumerate() {
            let is_active = idx == active;
            let label = tab.title.clone();
            let pinned = tab.pinned;
            let is_last = idx == tab_count - 1;
            strip = strip.child(
                gpui_component::h_flex()
                    .id(("result-tab", idx))
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .when(!is_last, |this| {
                        this.border_r_1().border_color(border_color)
                    })
                    .when(is_active, |this| {
                        this.bg(theme.table_head).text_color(active_fg)
                    })
                    .when(!is_active, |this| {
                        this.bg(theme.title_bar).text_color(muted_fg)
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.activate_tab(idx, cx);
                        }),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        gpui_component::h_flex()
                            .gap_1()
                            .child(
                                Button::new(("pin-tab", idx))
                                    .ghost()
                                    .xsmall()
                                    .icon(if pinned {
                                        IconName::PinOff
                                    } else {
                                        IconName::Pin
                                    })
                                    .tooltip(if pinned { "Unpin" } else { "Pin" })
                                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                                        this.toggle_pin(idx, cx);
                                    })),
                            )
                            .when(show_close, |this| {
                                this.child(
                                    Button::new(("close-tab", idx))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("Close result")
                                        .on_click(cx.listener(move |this, _ev, window, cx| {
                                            this.close_tab(idx, window, cx);
                                        })),
                                )
                            }),
                    ),
            );
        }

        v_flex()
            .size_full()
            .border_t_1()
            .border_color(border_color)
            // Handle copy and selection actions
            .on_action(cx.listener(Self::on_copy_as_csv))
            .on_action(cx.listener(Self::on_copy_as_tsv))
            .on_action(cx.listener(Self::on_copy_as_json))
            .on_action(cx.listener(Self::on_copy_as_sql))
            .on_action(cx.listener(Self::on_copy_as_values))
            .on_action(cx.listener(Self::on_copy_as_markdown))
            .on_action(cx.listener(Self::on_export_as_csv))
            .on_action(cx.listener(Self::on_export_as_tsv))
            .on_action(cx.listener(Self::on_export_as_json))
            .on_action(cx.listener(Self::on_export_as_sql))
            .on_action(cx.listener(Self::on_export_as_markdown))
            .on_action(cx.listener(Self::on_add_row))
            .on_action(cx.listener(Self::on_duplicate_row))
            .on_action(cx.listener(Self::on_delete_row))
            .on_action(cx.listener(Self::on_set_cell_null))
            .when(show_strip, |this| this.child(strip))
            // The table component (table should have built-in scrolling)
            .child(
                div()
                    .id("results-table")
                    .border_b_1()
                    .border_color(border_color)
                    .flex_1() // Allow table to fill available space
                    .overflow_hidden()
                    .min_h(px(200.0)) // Minimum height for table
                    .child(DataTable::new(&self.table_state).bordered(false)),
            )
    }
}
