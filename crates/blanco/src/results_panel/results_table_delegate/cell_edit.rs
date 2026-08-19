use gpui::{AppContext, Context, Focusable, Window};
use gpui_component::input::{EditorState, InputState};
use gpui_component::table::TableState;
use serde_json::Value;

use super::ResultsTableDelegate;
use crate::results_panel::ResultsPanel;
use crate::results_panel::cell_edit_state::{CellInput, ChangeType, TableChange};

impl ResultsTableDelegate {
    pub fn start_editing_cell(&mut self, row: usize, col: usize) {
        if let Some(cell_value) = self.rows.get(row).and_then(|r| r.get(col)) {
            // Store the original value if not already stored
            self.edit_state
                .original_values
                .entry((row, col))
                .or_insert_with(|| cell_value.clone());
            self.edit_state.editing_cell = Some((row, col));
        }
    }

    pub fn update_cell_value(&mut self, row: usize, col: usize, new_value: Option<String>) {
        self.edit_state.edited_values.insert((row, col), new_value);
    }

    pub fn commit_cell_edit(&mut self, row: usize, col: usize) -> Option<Option<String>> {
        // Bail early if table is not editable (no table_name or incomplete primary key)
        if !self.is_editable() {
            self.edit_state.editing_cell = None;
            return None;
        }

        if let Some(new_value) = self.edit_state.edited_values.get(&(row, col)).cloned() {
            // Get the original value
            let original_value = self.edit_state.original_values.get(&(row, col)).cloned();

            // Update the actual row data
            if let Some(row_data) = self.rows.get_mut(row)
                && let Some(cell) = row_data.get_mut(col)
            {
                *cell = new_value.clone();
            }

            // Track the change for SQL generation (but not for new rows)
            // original_value is Option<Option<String>>, the outer Option is whether editing started
            if let (Some(original), Some(table_name)) = (&original_value, &self.table_name) {
                // Check if this is a new row, if so, don't create UPDATE changes
                // New rows should be handled by INSERT operations only
                if !self.edit_state.is_new_row(row) {
                    let primary_key_values: Vec<(String, Option<String>)> = self
                        .primary_key_column_names()
                        .into_iter()
                        .filter_map(|pk_name| {
                            let full_index = self
                                .columns
                                .iter()
                                .position(|c| c.name.as_str() == pk_name)?;
                            let value = if full_index == col {
                                self.edit_state
                                    .original_values
                                    .get(&(row, col))
                                    .and_then(|v| v.clone())
                            } else {
                                self.rows
                                    .get(row)
                                    .and_then(|r| r.get(full_index))
                                    .and_then(|v| v.clone())
                            };
                            Some((pk_name.to_string(), value))
                        })
                        .collect();

                    let all_present = primary_key_values.iter().all(|(_, v)| v.is_some());
                    if !all_present {
                        tracing::warn!(
                            "Skipping change tracking: missing primary key value(s) for row {}",
                            row
                        );
                    } else {
                        let change = TableChange::new(
                            ChangeType::UpdateCell,
                            table_name.clone(),
                            row,
                            Some(col),
                            original.clone(),
                            new_value.clone(),
                            primary_key_values,
                            None,
                        );

                        self.edit_state.add_change(change);
                    }
                }
            }

            // Clear only the editing state, keep edited_values for visual indicator
            self.edit_state.editing_cell = None;
            // Note: Keep in edited_values to maintain yellow border until committed to database

            Some(new_value)
        } else {
            // No changes to commit, just clear editing state
            self.edit_state.editing_cell = None;
            None
        }
    }

    pub fn cancel_cell_edit(&mut self, row: usize, col: usize) {
        // Clear editing state and any edited values for this cell
        self.edit_state.editing_cell = None;
        self.edit_state.edited_values.remove(&(row, col));
    }

    pub fn is_editable(&self) -> bool {
        if matches!(self.db_type, Some(database::DatabaseType::ClickHouse)) {
            return false;
        }
        self.table_name.is_some() && self.primary_key_is_complete()
    }

    /// Collapse an expanded cell editor back to a single-line inline input.
    pub(crate) fn handle_minimize(
        state: &mut TableState<ResultsTableDelegate>,
        cell: (usize, usize),
        is_json: bool,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        // Get current text before recreating input
        let mut current_text = state
            .delegate_mut()
            .edit_state
            .editing_input
            .as_ref()
            .map(|input| input.text(cx))
            .unwrap_or_default();

        // Re-compact prettified JSON back to its single-line form. The
        // single-line input strips newlines but leaves indentation behind,
        // which otherwise mangles the value, so reserialize it compactly.
        if is_json && let Ok(value) = serde_json::from_str::<Value>(&current_text) {
            if let Ok(compact) = serde_json::to_string(&value) {
                current_text = compact;
            }
        }

        // Recreate the state in single-line mode and subscribe to events
        let new_input = cx.new(|cx| InputState::new(window, cx).default_value(current_text));
        state.delegate_mut().edit_state.editing_input = Some(CellInput::Inline(new_input.clone()));

        // Re-subscribe to input events (blur/change)
        ResultsPanel::subscribe_to_input_events(state, &new_input, cell.1, cell.0, cx);

        // Re-focus the input after recreation
        new_input.focus_handle(cx).focus(window, cx);

        // Toggle expanded state
        state
            .delegate_mut()
            .edit_state
            .toggle_expanded(cell.1, cell.0);

        state.refresh(cx);
        cx.notify();
    }

    /// Expand an inline cell editor into a larger multi-line input, prettifying
    /// JSON columns when the current text parses as valid JSON.
    pub(crate) fn handle_maximize(
        state: &mut TableState<ResultsTableDelegate>,
        row_ix: usize,
        col_ix: usize,
        is_json: bool,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        // Get current text before recreating input
        let current_text = state
            .delegate_mut()
            .edit_state
            .editing_input
            .as_ref()
            .map(|input| input.text(cx))
            .unwrap_or_default();

        // Toggle expanded state
        state
            .delegate_mut()
            .edit_state
            .toggle_expanded(row_ix, col_ix);

        // Recreate the state as a multi-line editor
        let new_input = cx.new(|cx| {
            // Line numbers fill the gutter the code editor reserves anyway.
            let editor = EditorState::new(window, cx)
                .line_number(true)
                .soft_wrap(true);

            if is_json {
                // Prettify JSON if valid
                let prettified_text =
                    if let Ok(value) = serde_json::from_str::<Value>(&current_text) {
                        serde_json::to_string_pretty(&value).unwrap_or(current_text)
                    } else {
                        current_text
                    };
                editor.language("json").default_value(prettified_text)
            } else {
                editor.default_value(current_text)
            }
        });
        state.delegate_mut().edit_state.editing_input =
            Some(CellInput::Expanded(new_input.clone()));

        // Re-subscribe to input events (blur/change)
        ResultsPanel::subscribe_to_input_events(state, &new_input, row_ix, col_ix, cx);

        // Re-focus the input after recreation
        new_input.focus_handle(cx).focus(window, cx);

        state.refresh(cx);
        cx.notify();
    }
}
