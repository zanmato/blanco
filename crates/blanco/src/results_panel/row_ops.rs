use gpui::{Context, Window};

use super::{ChangeType, ResultsPanel, TableChange};
use crate::app::{AddRow, DeleteRow, DuplicateRow, SetCellNull};

impl ResultsPanel {
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
                delegate.rows.push(row_to_duplicate);

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
        self.duplicate_row_with_row(action.row, cx);
    }

    pub(super) fn on_delete_row(
        &mut self,
        action: &DeleteRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_row_with_row(action.row, cx);
    }

    pub(super) fn on_set_cell_null(
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
}
