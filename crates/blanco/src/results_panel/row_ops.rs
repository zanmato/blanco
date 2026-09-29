use gpui::{App, Context, Window};

use super::{ChangeType, ResultsPanel, TableChange};
use crate::app::{AddRow, DeleteRow, DuplicateRow, SetCellDefault, SetCellNull};

impl ResultsPanel {
    pub fn add_new_row(&mut self, cx: &mut Context<Self>) {
        if self.commit_in_progress || !self.table_state.read(cx).delegate().is_editable() {
            return;
        }

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            let column_count = delegate.columns.len();

            // Create a new row with empty values
            let new_row: Vec<Option<String>> = (0..column_count).map(|_| None).collect();
            delegate.rows.push(new_row);

            // Mark this as a pending new row
            let new_row_index = delegate.rows.len() - 1;
            delegate.edit_state.pending_new_rows.push(new_row_index);

            // Track the INSERT change with NULL values (excluding primary key columns)
            if let Some(table_name) = &delegate.table_name {
                // For new rows, exclude primary key to avoid UPDATE/INSERT confusion
                let column_names = delegate.get_insert_column_names(new_row_index, true); // exclude_primary_key = true
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
        if self.commit_in_progress {
            return;
        }

        for row_ix in self.sorted_selected_rows(cx) {
            self.duplicate_row_with_row(row_ix, cx);
        }
    }

    /// The rows an action from a row's context menu applies to: the whole
    /// selection when the clicked row is part of it, otherwise just that row.
    fn rows_for_context_action(&self, row_ix: usize, cx: &App) -> Vec<usize> {
        let selected_rows = self.table_state.read(cx).selected_rows();
        if selected_rows.contains(&row_ix) {
            self.sorted_selected_rows(cx)
        } else {
            vec![row_ix]
        }
    }

    fn sorted_selected_rows(&self, cx: &App) -> Vec<usize> {
        let mut rows: Vec<usize> = self
            .table_state
            .read(cx)
            .selected_rows()
            .iter()
            .copied()
            .collect();
        rows.sort_unstable();
        rows
    }

    pub fn duplicate_row_with_row(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        if self.commit_in_progress {
            return;
        }

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            if let Some(mut row_to_duplicate) = delegate.rows.get(row_ix).cloned() {
                // The copy must get its own key from the server, so its primary
                // key cells start untouched and are omitted from the INSERT.
                for pk_index in delegate.primary_key_column_indices().unwrap_or_default() {
                    if let Some(cell) = row_to_duplicate.get_mut(pk_index) {
                        *cell = None;
                    }
                }
                delegate.rows.push(row_to_duplicate);

                // Mark this as a pending new row
                let new_row_index = delegate.rows.len() - 1;
                delegate.edit_state.pending_new_rows.push(new_row_index);

                if let Some(table_name) = &delegate.table_name {
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
        if self.commit_in_progress {
            return;
        }

        for row_ix in self.sorted_selected_rows(cx).into_iter().rev() {
            self.delete_row_with_row(row_ix, cx);
        }
    }

    pub fn delete_row_with_row(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        if self.commit_in_progress {
            return;
        }

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
                        .position(|c| c.name.as_str() == pk_name)?;
                    let value = delegate
                        .rows
                        .get(row_ix)
                        .and_then(|row| row.get(pk_col_idx))
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

    // Row action handlers

    pub(super) fn on_add_row(
        &mut self,
        _action: &AddRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_new_row(cx);
    }

    pub(super) fn on_duplicate_row(
        &mut self,
        action: &DuplicateRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for row_ix in self.rows_for_context_action(action.row, cx) {
            self.duplicate_row_with_row(row_ix, cx);
        }
    }

    pub(super) fn on_delete_row(
        &mut self,
        action: &DeleteRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Descending so removing a pending new row cannot shift the indices
        // of rows still to be handled.
        for row_ix in self
            .rows_for_context_action(action.row, cx)
            .into_iter()
            .rev()
        {
            self.delete_row_with_row(row_ix, cx);
        }
    }

    pub(super) fn on_set_cell_null(
        &mut self,
        action: &SetCellNull,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_in_progress {
            return;
        }

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

    /// Revert a cell in a pending new row to its untouched state, so the
    /// column is omitted from the INSERT and the server default applies.
    pub(super) fn on_set_cell_default(
        &mut self,
        action: &SetCellDefault,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.commit_in_progress {
            return;
        }

        self.table_state.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            if !delegate.edit_state.is_new_row(action.row) {
                return;
            }

            delegate
                .edit_state
                .edited_values
                .remove(&(action.row, action.col));
            delegate
                .edit_state
                .original_values
                .remove(&(action.row, action.col));
            if let Some(cell) = delegate.get_cell_mut(action.row, action.col) {
                *cell = None;
            }
            state.refresh(cx);
        });

        cx.notify();
    }
}
