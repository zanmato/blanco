use std::collections::HashSet;
use std::time::Duration;

use blanco_ui::IconName;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::Scrollbar,
    table::{DataTable, TableEvent, TableState},
    tooltip::Tooltip,
    v_flex,
};

use blanco_core::QueryResult;
use blanco_core::{
    KeyValueResult, RedisValue, ResultPayload,
    explain_plan::{self, PlanNode, PlanTree},
};
use std::sync::Arc;

use crate::copy_handler::CopyHandler;
pub use transformers::{SelectedCell, SelectedRow, SelectedTableData};

mod cell_edit_state;
#[cfg(test)]
mod cell_edit_test;
mod chart_view;
mod clipboard;
mod commit;
mod compare;
mod export_actions;
mod foreign_key_popover;
mod results_table_delegate;
mod row_ops;
mod table_operations;

// Re-exports
pub use cell_edit_state::{CellInput, ChangeType, TableChange};
pub use chart_view::{ChartView, ResultViewMode};
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
    pub applied: usize,
}

/// One materialized result-set rendered as a sub-tab inside `ResultsPanel`.
/// A query script with N statements produces N `ResultTab`s; each owns its own
/// `TableState` so edit state and selection stay isolated per tab.
pub struct ResultTab {
    pub title: SharedString,
    pub pinned: bool,
    pub table_state: Entity<TableState<ResultsTableDelegate>>,
    pub view_mode: ResultViewMode,
    pub chart_view: Entity<ChartView>,
    /// Set when this tab shows a non-tabular Redis key/value payload. When
    /// present, the panel renders the key inspector instead of the table/chart.
    pub key_value: Option<KeyValueResult>,
    /// Parsed EXPLAIN output. Present when the tab's query was an EXPLAIN the
    /// backend returns in a structured form; the panel then defaults to the
    /// plan view with the raw table one toggle away.
    pub plan: Option<Arc<PlanTree>>,
    /// Client-side text filter over the loaded rows of this tab.
    pub filter_input: Entity<InputState>,
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
    commit_in_progress: bool,
    copy_handler: CopyHandler,
    /// Scroll position for the key/value inspector body (Redis key view).
    key_value_scroll_handle: ScrollHandle,
    plan_scroll_handle: ScrollHandle,
    /// Horizontal scroll position for the result tab strip.
    tab_strip_scroll_handle: ScrollHandle,
    /// Pending first half of a cell/row comparison, panel wide so it survives
    /// switching result tabs and re-running the query.
    compare_selection: Option<compare::CompareSelection>,
    /// The most recently opened diff, kept so tests can inspect it.
    last_diff_view: Option<Entity<blanco_ui::DiffView>>,
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
            commit_in_progress: false,
            copy_handler: CopyHandler::new(),
            key_value_scroll_handle: ScrollHandle::new(),
            plan_scroll_handle: ScrollHandle::new(),
            tab_strip_scroll_handle: ScrollHandle::new(),
            compare_selection: None,
            last_diff_view: None,
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
                .row_numbers(true)
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

        let chart_view = cx.new(|cx| ChartView::new(table_state.clone(), window, cx));

        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter rows..."));
        let filter_subscription = cx.subscribe_in(
            &filter_input,
            window,
            move |panel, filter_input, event: &InputEvent, window, cx| {
                if let InputEvent::Change = event {
                    let needle = filter_input.read(cx).value().to_string();
                    panel.apply_row_filter(filter_input, &needle, window, cx);
                }
            },
        );

        ResultTab {
            title,
            pinned: false,
            table_state,
            view_mode: ResultViewMode::default(),
            chart_view,
            key_value: None,
            plan: None,
            filter_input,
            _subscriptions: vec![subscription, filter_subscription],
        }
    }

    /// Apply the text filter of whichever tab owns `filter_input`. The tab is
    /// looked up by its input so a filter typed into a background (pinned) tab
    /// never lands on the active one.
    fn apply_row_filter(
        &mut self,
        filter_input: &Entity<InputState>,
        needle: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self
            .result_tabs
            .iter()
            .find(|tab| tab.filter_input.entity_id() == filter_input.entity_id())
        else {
            return;
        };
        let table_state = tab.table_state.clone();
        let chart_view = tab.chart_view.clone();
        let applied = table_state.update(cx, |state, cx| {
            let applied = state.delegate_mut().apply_filter(needle);
            if applied {
                state.clear_selection(cx);
                state.refresh(cx);
            }
            applied
        });
        if applied {
            chart_view.update(cx, |view, cx| view.rebuild_column_selects(window, cx));
            cx.notify();
        }
    }

    pub fn set_view_mode(&mut self, mode: ResultViewMode, cx: &mut Context<Self>) {
        if let Some(tab) = self.result_tabs.get_mut(self.active_tab)
            && tab.view_mode != mode
        {
            tab.view_mode = mode;
            cx.notify();
        }
    }

    /// Replace all unpinned tabs with one tab per provided tabular result. Thin
    /// wrapper over [`set_result_payloads`] for the SQL execution path.
    pub fn set_query_results(
        &mut self,
        results: Vec<QueryResult>,
        connection_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let payloads = results.into_iter().map(ResultPayload::Tabular).collect();
        self.set_result_payloads(payloads, connection_id, window, cx);
    }

    /// Show a single Redis key/value payload in a fresh tab and activate it.
    /// Invoked by the connections sidebar when a Redis key is opened.
    pub fn set_key_value_result(
        &mut self,
        result: KeyValueResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_result_payloads(
            vec![ResultPayload::KeyValue(result)],
            Some(self.connection_id),
            window,
            cx,
        );
    }

    /// Replace all unpinned tabs with one tab per provided result. The first
    /// newly added tab becomes active; pinned tabs are preserved at the front
    /// of the strip. Tabular payloads feed the SQL results table; key/value
    /// payloads feed the Redis key inspector.
    pub fn set_result_payloads(
        &mut self,
        results: Vec<ResultPayload>,
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
        for (i, payload) in results.into_iter().enumerate() {
            let title = match &payload {
                ResultPayload::KeyValue(kv) => SharedString::from(kv.key.clone()),
                ResultPayload::Tabular(_) if total > 1 => {
                    SharedString::from(format!("Result {}", i + 1))
                }
                ResultPayload::Tabular(_) => SharedString::from("Result"),
            };
            let mut tab = Self::build_result_tab(
                self.connection_id,
                &self.database_name.clone(),
                self.db_type,
                title,
                window,
                cx,
            );
            match payload {
                ResultPayload::Tabular(result) => {
                    let query_text = result.query_text.clone();
                    if query_text
                        .as_deref()
                        .is_some_and(|text| is_explain_statement(text))
                    {
                        if let Some(plan) = explain_plan::parse_plan(self.db_type, &result) {
                            tab.plan = Some(Arc::new(plan));
                            tab.view_mode = ResultViewMode::Plan;
                        }
                    }
                    tab.table_state.update(cx, |state, cx| {
                        if let Some(ref q) = query_text {
                            state.delegate_mut().set_original_query(q.clone());
                        }
                        state.clear_selection(cx);
                        state.delegate_mut().set_query_result(result, window, cx);
                        state.refresh(cx);
                    });
                    tab.chart_view.update(cx, |view, cx| {
                        view.rebuild_column_selects(window, cx);
                    });
                }
                ResultPayload::KeyValue(kv) => {
                    tab.key_value = Some(kv);
                }
            }
            self.result_tabs.push(tab);
        }

        self.active_tab = first_new_index;
        self.table_state = self.result_tabs[self.active_tab].table_state.clone();
        self.has_results = true;
        self.sync_compare_selection_kind(cx);
        let _ = connection_id; // accepted for API parity with single-result path
        cx.notify();
    }

    pub fn activate_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.result_tabs.len() || index == self.active_tab {
            return;
        }
        self.active_tab = index;
        self.table_state = self.result_tabs[index].table_state.clone();
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
        cx.notify();
    }

    #[cfg(test)]
    pub fn active_plan(&self) -> Option<Arc<PlanTree>> {
        self.result_tabs
            .get(self.active_tab)
            .and_then(|tab| tab.plan.clone())
    }

    #[cfg(test)]
    pub fn active_view_mode(&self) -> ResultViewMode {
        self.result_tabs
            .get(self.active_tab)
            .map(|tab| tab.view_mode)
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn table_state(&self) -> &Entity<TableState<ResultsTableDelegate>> {
        &self.table_state
    }

    #[cfg(test)]
    pub fn last_diff_view(&self) -> Option<Entity<blanco_ui::DiffView>> {
        self.last_diff_view.clone()
    }

    pub fn cancel_current_edit(&mut self, cx: &mut Context<Self>) {
        // Use the delegate's editing state instead of the panel's
        let editing_cell = self.table_state.read(cx).delegate().edit_state.editing_cell;

        if let Some((row, col)) = editing_cell {
            self.cancel_cell_edit(row, col, cx);
        }
    }

    /// Start editing the table's selected cell (Enter / F2 on the table).
    fn on_start_cell_edit(
        &mut self,
        _action: &crate::app::StartCellEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_in_progress || self.get_current_editing_cell(cx).is_some() {
            return;
        }
        let (selected_cell, is_editable) = {
            let state = self.table_state.read(cx);
            (state.selected_cell(), state.delegate().is_editable())
        };
        let Some((row, col)) = selected_cell else {
            return;
        };
        if !is_editable {
            return;
        }
        self.table_state.update(cx, |state, _cx| {
            state.delegate_mut().clear_selection();
        });
        self.start_cell_edit(row, col, window, cx);
    }

    /// Commit the in-flight cell edit (Enter inside the cell input). The
    /// single-line input propagates its Enter action, so this fires on the
    /// panel while a cell editor is focused.
    fn on_input_enter(
        &mut self,
        _action: &gpui_component::input::Enter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cell) = self.get_current_editing_cell(cx) else {
            cx.propagate();
            return;
        };
        self.finalize_active_cell_edit(cx);
        self.stop_editing_and_focus_table(cell, window, cx);
    }

    /// Cancel the in-flight cell edit (Escape inside the cell input). The
    /// expanded editor overlay handles Escape itself (minimize), so this only
    /// sees the inline case.
    fn on_input_escape(
        &mut self,
        _action: &gpui_component::input::Escape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cell) = self.get_current_editing_cell(cx) else {
            cx.propagate();
            return;
        };
        self.cancel_current_edit(cx);
        self.stop_editing_and_focus_table(cell, window, cx);
    }

    fn on_edit_next_cell(
        &mut self,
        _action: &crate::app::EditNextCell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_cell_edit(true, window, cx);
    }

    fn on_edit_prev_cell(
        &mut self,
        _action: &crate::app::EditPrevCell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_cell_edit(false, window, cx);
    }

    /// Commit the current cell edit and start editing the neighbouring cell,
    /// wrapping across row boundaries at either end.
    fn move_cell_edit(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((row, col)) = self.get_current_editing_cell(cx) else {
            return;
        };
        // Fold the typed text into tracked edits; a no-op for untouched input.
        self.finalize_active_cell_edit(cx);
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().edit_state.stop_editing();
            state.refresh(cx);
        });

        let (row_count, col_count) = {
            let delegate = self.table_state.read(cx).delegate();
            (delegate.rows.len(), delegate.columns.len())
        };
        let target = if forward {
            if col + 1 < col_count {
                Some((row, col + 1))
            } else if row + 1 < row_count {
                Some((row + 1, 0))
            } else {
                None
            }
        } else if col > 0 {
            Some((row, col - 1))
        } else if row > 0 && col_count > 0 {
            Some((row - 1, col_count - 1))
        } else {
            None
        };

        match target {
            Some((next_row, next_col)) => {
                self.table_state.update(cx, |state, cx| {
                    state.set_selected_cell(next_row, next_col, cx);
                });
                self.start_cell_edit(next_row, next_col, window, cx);
            }
            None => self.stop_editing_and_focus_table((row, col), window, cx),
        }
    }

    /// Return keyboard focus to the table and leave the finished cell selected,
    /// so arrow-key navigation picks up where the edit ended.
    fn stop_editing_and_focus_table(
        &mut self,
        cell: (usize, usize),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |state, cx| {
            state.delegate_mut().edit_state.stop_editing();
            state.set_selected_cell(cell.0, cell.1, cx);
            state.refresh(cx);
        });
        self.table_state.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub fn start_cell_edit(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_in_progress {
            return;
        }

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
            delegate
                .edit_state
                .start_editing(row, col, CellInput::Inline(input.clone()));

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
    }

    /// Subscribe to input events (blur/change) for a given cell.
    ///
    /// Generic over the state type because a cell edits in a single-line input
    /// inline and in a multi-line editor when expanded, and those are separate
    /// types. The edited text is read back from the delegate's `editing_input`,
    /// which is the handle those two share.
    fn subscribe_to_input_events<S: gpui::EventEmitter<InputEvent> + 'static>(
        _state: &mut TableState<ResultsTableDelegate>,
        input: &Entity<S>,
        row: usize,
        col: usize,
        cx: &mut Context<TableState<ResultsTableDelegate>>,
    ) {
        let row_clone = row;
        let col_clone = col;
        cx.subscribe(input, move |table, _input, event, cx| {
            if let InputEvent::Change = event {
                let new_text = table
                    .delegate()
                    .edit_state
                    .editing_input
                    .as_ref()
                    .map(|input| input.text(cx))
                    .unwrap_or_default();
                table
                    .delegate_mut()
                    .edit_state
                    .edited_values
                    .insert((row_clone, col_clone), Some(new_text));

                // Debug: Input change handled in edited_values for commit_cell_edit
                // Note: Can't refresh here due to borrowing issues
            } else if let InputEvent::Blur = event {
                // Save current edit to edited_values when input loses focus
                // Get the current editing cell and value
                let editing_cell = table.delegate_mut().edit_state.editing_cell;

                if let Some((row, col)) = editing_cell {
                    if table.delegate_mut().edit_state.is_expanded(row, col) {
                        return;
                    }

                    table.delegate_mut().commit_cell_edit(row, col);
                    table.refresh(cx);
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
                        let full_index = delegate
                            .columns
                            .iter()
                            .position(|c| c.name.as_str() == pk_name)?;
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

        cx.notify();
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

    /// Commit the cell currently being edited (if any) into a tracked change,
    /// using the delegate's live input. The delegate's `editing_input` is the
    /// single source of truth: maximize/minimize recreate the input entity, so
    /// any other handle to it goes stale and would commit outdated text.
    pub fn finalize_active_cell_edit(&mut self, cx: &mut Context<Self>) {
        let Some((row, col)) = self.get_current_editing_cell(cx) else {
            return;
        };

        let editing_input = self
            .table_state
            .read(cx)
            .delegate()
            .edit_state
            .get_editing_input();

        if let Some(input) = editing_input {
            // Only commit if the user actually typed (a Change event recorded
            // the cell as edited); otherwise an untouched editor would emit a
            // no-op UPDATE, and turn a NULL into an empty string.
            if !self
                .table_state
                .read(cx)
                .delegate()
                .edit_state
                .is_edited(row, col)
            {
                return;
            }
            let current_value = input.text(cx);
            self.update_editing_cell_value(row, col, current_value.clone(), cx);
            self.commit_cell_edit(row, col, current_value, cx);
        } else {
            self.cancel_current_edit(cx);
        }
    }

    /// Whether there are any uncommitted cell edits, pending new rows, or pending deletions.
    pub fn has_pending_edits(&self, cx: &App) -> bool {
        let edit_state = &self.table_state.read(cx).delegate().edit_state;
        !edit_state.edited_values.is_empty()
            || !edit_state.pending_new_rows.is_empty()
            || !edit_state.pending_deleted_rows.is_empty()
    }

    pub fn is_commit_in_progress(&self) -> bool {
        self.commit_in_progress
    }

    /// Generate the SQL statements that would be executed by Apply edits,
    /// without committing or executing anything.
    pub fn preview_pending_sql(&self, cx: &App) -> Vec<String> {
        self.table_state
            .read(cx)
            .delegate()
            .create_change_operations()
            .iter()
            .map(|operation| operation.to_sql_query(self.db_type))
            .collect()
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
                    .map(|(col, value)| SelectedCell {
                        col,
                        value: value.clone(),
                        column_name: delegate.columns.get(col).map(|c| c.name.to_string()),
                        column_type: delegate.column_types.get(col).cloned(),
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
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>(),
            selected_rows: selected_rows_data,
        }
    }

    /// Build the result tab strip: one entry per result tab plus the
    /// table/chart view-mode toggle anchored on the right.
    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let table_head = theme.table_head;
        let title_bar = theme.title_bar;
        let tab_count = self.result_tabs.len();
        let show_close = tab_count > 1;

        let mut tabs = h_flex()
            .id("result-tabs-scroll")
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.tab_strip_scroll_handle);
        for (idx, tab) in self.result_tabs.iter().enumerate() {
            let is_active = idx == active;
            let label = tab.title.clone();
            let pinned = tab.pinned;
            let is_last = idx == tab_count - 1;
            let read_only = !tab.table_state.read(cx).delegate().is_editable();
            tabs = tabs.child(
                h_flex()
                    .id(("result-tab", idx))
                    .flex_1()
                    .min_w(px(140.))
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .when(!is_last, |this| {
                        this.border_r_1().border_color(border_color)
                    })
                    .when(is_active, |this| {
                        this.bg(table_head).text_color(active_fg)
                    })
                    .when(!is_active, |this| {
                        this.bg(title_bar).text_color(muted_fg)
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.activate_tab(idx, cx);
                        }),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1()
                            .when(read_only, |this| {
                                this.child(
                                    div()
                                        .id(("read-only-lock", idx))
                                        .flex_shrink_0()
                                        .child(
                                            Icon::new(IconName::Lock)
                                                .size(px(12.))
                                                .text_color(muted_fg),
                                        )
                                        .tooltip(|window, cx| {
                                            Tooltip::new(
                                                "Read-only: editing requires a complete primary key",
                                            )
                                            .build(window, cx)
                                        }),
                                )
                            })
                            .child(div().flex_1().min_w_0().truncate().child(label)),
                    )
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

        // View-mode toggle (table / chart) for the active tab, anchored right.
        // Hidden for key/value (Redis) tabs, which have their own inspector view.
        let active_view_mode = self
            .result_tabs
            .get(active)
            .map(|t| t.view_mode)
            .unwrap_or_default();
        let is_key_value = self
            .result_tabs
            .get(active)
            .is_some_and(|t| t.key_value.is_some());
        let is_table = matches!(active_view_mode, ResultViewMode::Table);
        let is_plan = matches!(active_view_mode, ResultViewMode::Plan);
        let has_plan = self
            .result_tabs
            .get(active)
            .is_some_and(|tab| tab.plan.is_some());
        let filter = self.result_tabs.get(active).map(|tab| {
            let delegate_state = tab.table_state.read(cx);
            let delegate = delegate_state.delegate();
            (
                tab.filter_input.clone(),
                delegate.has_pending_changes(),
                delegate.is_filtered(),
                delegate.rows.len(),
                delegate.unfiltered_row_count(),
            )
        });

        h_flex()
            .id("result-tabs-strip")
            .w_full()
            .text_sm()
            .border_b_1()
            .border_color(border_color)
            .bg(strip_bg)
            .child(tabs)
            .when_some(
                filter.filter(|_| !is_key_value && is_table),
                |strip, filter| {
                    let (filter_input, edits_pending, is_filtered, shown, total) = filter;
                    strip.child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .when(is_filtered, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .whitespace_nowrap()
                                        .child(format!("{shown} of {total}")),
                                )
                            })
                            .child(
                                div()
                                    .id("result-filter")
                                    .w(px(180.))
                                    .child(
                                        Input::new(&filter_input).xsmall().disabled(edits_pending),
                                    )
                                    .when(edits_pending, |this| {
                                        this.tooltip(|window, cx| {
                                            Tooltip::new(
                                                "Commit or discard pending edits to filter",
                                            )
                                            .build(window, cx)
                                        })
                                    }),
                            ),
                    )
                },
            )
            // View-mode toggle stays anchored right, outside the scroll region.
            .when(!is_key_value, |strip| {
                strip.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap_1()
                        .px_2()
                        .when(has_plan, |this| {
                            this.child(
                                Button::new("view-mode-plan")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ListTree)
                                    .selected(is_plan)
                                    .tooltip("Query plan")
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.set_view_mode(ResultViewMode::Plan, cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new("view-mode-table")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Sheet)
                                .selected(is_table)
                                .tooltip("Table view")
                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                    this.set_view_mode(ResultViewMode::Table, cx);
                                })),
                        )
                        .child(
                            Button::new("view-mode-chart")
                                .ghost()
                                .xsmall()
                                .icon(IconName::ChartBar)
                                .selected(!is_table && !is_plan)
                                .tooltip("Chart view")
                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                    this.set_view_mode(ResultViewMode::Chart, cx);
                                })),
                        ),
                )
            })
    }

    /// Render a parsed EXPLAIN as an indented tree. Each node shows its
    /// label, row estimate vs actual, and (when the backend reports timings) a
    /// bar scaled to the slowest node so hot spots stand out. Self time
    /// (inclusive minus children) drives the bar color.
    fn render_plan_view(&self, plan: &PlanTree, cx: &Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let border_color = cx.theme().border;
        let mono = cx.theme().mono_font_family.clone();
        let max_time = plan.max_time_ms().unwrap_or(0.0);
        let bar_cold = cx.theme().green;
        let bar_warm = cx.theme().yellow;
        let bar_hot = cx.theme().red;

        let header = h_flex()
            .gap_4()
            .items_center()
            .px_3()
            .py(px(6.))
            .border_b_1()
            .border_color(border_color)
            .font_family(mono.clone())
            .text_size(px(12.))
            .when_some(plan.planning_time_ms, |this, time| {
                this.child(
                    h_flex()
                        .gap_1()
                        .child(div().text_color(muted).child("planning:"))
                        .child(format!("{time:.2} ms")),
                )
            })
            .when_some(plan.execution_time_ms, |this, time| {
                this.child(
                    h_flex()
                        .gap_1()
                        .child(div().text_color(muted).child("execution:"))
                        .child(format!("{time:.2} ms")),
                )
            })
            .when(!plan.has_timings(), |this| {
                this.child(
                    div()
                        .text_color(muted)
                        .child("estimated plan (no timings for this statement)"),
                )
            });

        fn rows(
            node: &PlanNode,
            depth: usize,
            max_time: f64,
            colors: (gpui::Hsla, gpui::Hsla, gpui::Hsla),
            muted: gpui::Hsla,
            border_color: gpui::Hsla,
            out: &mut Vec<gpui::AnyElement>,
        ) {
            let (cold, warm, hot) = colors;
            let share = node
                .actual_time_ms
                .filter(|_| max_time > 0.0)
                .map(|time| (time / max_time).clamp(0.0, 1.0));
            let self_share = node
                .self_time_ms()
                .filter(|_| max_time > 0.0)
                .map(|time| (time / max_time).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let bar_color = if self_share > 0.5 {
                hot
            } else if self_share > 0.15 {
                warm
            } else {
                cold
            };

            let mut stats: Vec<String> = Vec::new();
            match (node.estimated_rows, node.actual_rows) {
                (Some(estimated), Some(actual)) => {
                    stats.push(format!("rows {actual:.0} (est {estimated:.0})"));
                }
                (Some(estimated), None) => stats.push(format!("est rows {estimated:.0}")),
                (None, Some(actual)) => stats.push(format!("rows {actual:.0}")),
                (None, None) => {}
            }
            if let Some(time) = node.actual_time_ms {
                stats.push(format!("{time:.2} ms"));
            }
            if let Some(cost) = node.total_cost {
                stats.push(format!("cost {cost:.1}"));
            }

            let details = node.details.clone();
            out.push(
                v_flex()
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(border_color)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(div().w(px(16.0 * depth as f32)).flex_shrink_0())
                            .when(depth > 0, |this| {
                                this.child(div().text_color(muted).child("└"))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(node.label.clone()),
                            )
                            .child(
                                div()
                                    .text_color(muted)
                                    .text_size(px(11.))
                                    .whitespace_nowrap()
                                    .child(stats.join("  ·  ")),
                            )
                            .when_some(share, |this, share| {
                                this.child(
                                    div().w(px(120.)).h(px(6.)).flex_shrink_0().child(
                                        div()
                                            .h_full()
                                            .w(px(120.0 * share as f32))
                                            .rounded(px(3.))
                                            .bg(bar_color),
                                    ),
                                )
                            }),
                    )
                    .children(details.into_iter().map(|(key, value)| {
                        h_flex()
                            .gap_2()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(div().w(px(16.0 * depth as f32 + 24.0)).flex_shrink_0())
                            .child(format!("{key}: {value}"))
                    }))
                    .into_any_element(),
            );
            for child in &node.children {
                rows(child, depth + 1, max_time, colors, muted, border_color, out);
            }
        }

        let mut elements = Vec::new();
        for root in &plan.roots {
            rows(
                root,
                0,
                max_time,
                (bar_cold, bar_warm, bar_hot),
                muted,
                border_color,
                &mut elements,
            );
        }

        v_flex().size_full().child(header).child(
            div()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("plan-body")
                        .size_full()
                        .overflow_scroll()
                        .track_scroll(&self.plan_scroll_handle)
                        .font_family(mono)
                        .text_size(px(12.))
                        .child(v_flex().children(elements)),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(Scrollbar::vertical(&self.plan_scroll_handle)),
                ),
        )
    }

    /// Render the key inspector: a compact metadata header followed by a
    /// scrollable, monospace value view shaped per the key's type.
    ///
    /// The key name already labels the result tab, so it is not repeated here.
    /// The header mirrors the SQL table's column header (a monospace strip of
    /// badges); which badges appear is backend-specific. For key/value stores
    /// (Redis) it shows the value type and TTL.
    fn render_key_value_inspector(
        &self,
        kv: &KeyValueResult,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let border_color = cx.theme().border;
        let mono = cx.theme().mono_font_family.clone();

        let badge = |label: &'static str, value: String| {
            h_flex()
                .gap_1()
                .items_center()
                .child(div().text_color(muted).child(label))
                .child(SharedString::from(value))
        };
        let ttl_value = match kv.ttl {
            Some(seconds) => format!("{seconds}s"),
            None => "none".to_string(),
        };

        let header = h_flex()
            .gap_4()
            .items_center()
            .px_3()
            .py(px(6.))
            .border_b_1()
            .border_color(border_color)
            .font_family(mono.clone())
            .text_size(px(12.))
            .child(badge("type:", kv.key_type.as_str().to_string()))
            .child(badge("ttl:", ttl_value));

        v_flex().size_full().child(header).child(
            div()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("key-value-body")
                        .size_full()
                        .overflow_scroll()
                        .track_scroll(&self.key_value_scroll_handle)
                        .font_family(mono)
                        .child(self.render_redis_value(&kv.value, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(Scrollbar::vertical(&self.key_value_scroll_handle)),
                ),
        )
    }

    /// Render a Redis value as a simple key/value or list view. List, set and
    /// sorted-set members render one per row; hashes and streams render as
    /// field/value pairs.
    fn render_redis_value(&self, value: &RedisValue, cx: &Context<Self>) -> impl IntoElement {
        let border_color = cx.theme().border;
        let muted = cx.theme().muted_foreground;

        let row = |left: SharedString, right: Option<SharedString>| {
            h_flex()
                .gap_3()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(border_color)
                .when_some(right.clone(), |this, _| {
                    this.child(div().w(px(200.0)).text_color(muted).child(left.clone()))
                })
                .child(div().flex_1().child(right.unwrap_or(left)))
        };

        // Height grows with content so the inspector's outer container handles
        // scrolling; `size_full`/`overflow_hidden` here would clip long values.
        let mut container = v_flex().w_full().text_sm();
        match value {
            RedisValue::Str(s) => {
                container = container.child(
                    div()
                        .px_3()
                        .py_2()
                        .whitespace_normal()
                        .child(SharedString::from(s.clone())),
                );
            }
            RedisValue::List(items) | RedisValue::Set(items) => {
                for item in items {
                    container = container.child(row(SharedString::from(item.clone()), None));
                }
            }
            RedisValue::Hash(pairs) => {
                for (field, val) in pairs {
                    container = container.child(row(
                        SharedString::from(field.clone()),
                        Some(SharedString::from(val.clone())),
                    ));
                }
            }
            RedisValue::ZSet(members) => {
                for (member, score) in members {
                    container = container.child(row(
                        SharedString::from(member.clone()),
                        Some(SharedString::from(score.to_string())),
                    ));
                }
            }
            RedisValue::Stream(entries) => {
                for (id, fields) in entries {
                    let rendered = fields
                        .iter()
                        .map(|(f, v)| format!("{}={}", f, v))
                        .collect::<Vec<_>>()
                        .join(" ");
                    container = container.child(row(
                        SharedString::from(id.clone()),
                        Some(SharedString::from(rendered)),
                    ));
                }
            }
            RedisValue::None => {
                container = container.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(muted)
                        .child("(key does not exist)"),
                );
            }
        }
        container
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
        let border_color = cx.theme().border;
        let show_strip = self.has_results;
        let active_view_mode = self
            .result_tabs
            .get(active)
            .map(|t| t.view_mode)
            .unwrap_or_default();
        let active_chart_view = self.result_tabs.get(active).map(|t| t.chart_view.clone());
        let active_key_value = self
            .result_tabs
            .get(active)
            .and_then(|t| t.key_value.clone());
        let active_plan = self.result_tabs.get(active).and_then(|t| t.plan.clone());

        v_flex()
            // The action handlers below live on this node, so the focus handle
            // has to be tracked here. An untracked handle is absent from the
            // dispatch tree, and GPUI then dispatches from the tree root along
            // a path that misses this node.
            .track_focus(&self.focus_handle)
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
            .on_action(cx.listener(Self::on_set_cell_default))
            .on_action(cx.listener(Self::on_select_cell_for_compare))
            .on_action(cx.listener(Self::on_compare_cell_with_selected))
            .on_action(cx.listener(Self::on_select_row_for_compare))
            .on_action(cx.listener(Self::on_compare_row_with_selected))
            .on_action(cx.listener(Self::on_clear_compare_selection))
            .on_action(cx.listener(Self::on_start_cell_edit))
            .on_action(cx.listener(Self::on_edit_next_cell))
            .on_action(cx.listener(Self::on_edit_prev_cell))
            .on_action(cx.listener(Self::on_input_enter))
            .on_action(cx.listener(Self::on_input_escape))
            .when(show_strip, |this| this.child(self.render_tab_strip(cx)))
            .child(
                div()
                    .id("results-body")
                    .border_b_1()
                    .border_color(border_color)
                    .flex_1()
                    .overflow_hidden()
                    .min_h(px(200.0))
                    .child(
                        match (active_key_value, active_view_mode, active_chart_view) {
                            (Some(kv), _, _) => {
                                self.render_key_value_inspector(&kv, cx).into_any_element()
                            }
                            (None, ResultViewMode::Plan, _) => match active_plan {
                                Some(plan) => self.render_plan_view(&plan, cx).into_any_element(),
                                None => DataTable::new(&self.table_state)
                                    .bordered(false)
                                    .into_any_element(),
                            },
                            (None, ResultViewMode::Chart, Some(chart)) => chart.into_any_element(),
                            _ => DataTable::new(&self.table_state)
                                .bordered(false)
                                .into_any_element(),
                        },
                    ),
            )
    }
}

/// Whether a statement text is an EXPLAIN (including SQLite's
/// `EXPLAIN QUERY PLAN`), so its result can be offered as a plan tree.
fn is_explain_statement(text: &str) -> bool {
    text.trim_start()
        .split_whitespace()
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case("EXPLAIN"))
}
