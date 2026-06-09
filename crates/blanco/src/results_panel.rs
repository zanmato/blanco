use std::collections::HashSet;
use std::time::Duration;

use blanco_ui::IconName;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{InputEvent, InputState},
    table::{DataTable, TableEvent, TableState},
    v_flex,
};

use blanco_core::ColumnType;
use blanco_core::QueryResult;

use crate::transformers::CopyHandler;

mod cell_edit_state;
mod clipboard;
mod commit;
mod export_actions;
mod foreign_key_popover;
mod results_table_delegate;
mod row_ops;
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

    /// Generate the SQL statements that would be executed by Apply edits,
    /// without committing or executing anything.
    pub fn preview_pending_sql(&self, cx: &App) -> Vec<String> {
        self.table_state
            .read(cx)
            .delegate()
            .create_change_operations()
            .iter()
            .map(|op| op.to_sql_query())
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
