use blanco_core::ColumnType;
use gpui::{AppContext, Context, Entity, Focusable, Window};
use gpui_component::input::{EditorState, InputState};
use gpui_component::table::TableState;

use super::{ResultsTableDelegate, results_settings};
use crate::results_panel::ResultsPanel;
use crate::results_panel::binary_viewer::{BinaryViewer, BinaryViewerEvent};
use crate::results_panel::cell_edit_state::{CellInput, ChangeType, TableChange};
use crate::results_panel::compare::pretty_json_or_original;
use crate::results_panel::date_time_picker::{DateTimePicker, DateTimePickerEvent};
use crate::results_panel::json_text::{is_json_document, json_error, minify_json};
use crate::results_panel::temporal::{TemporalShape, TemporalValue};

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
            // Committing every pending edit walks cells that have no editor
            // open, and those keep the kind they were committed with.
            if self.edit_state.is_editing(row, col) {
                if self.edit_state.editing_as_expression && new_value.is_some() {
                    self.edit_state.expression_cells.insert((row, col));
                } else {
                    self.edit_state.expression_cells.remove(&(row, col));
                }
            } else if new_value.is_none() {
                self.edit_state.expression_cells.remove(&(row, col));
            }

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

    /// Switch the cell being edited between data that gets quoted and SQL the
    /// server evaluates. The switch changes what the same text means, so it
    /// counts as an edit even when nothing was typed.
    pub(crate) fn toggle_editing_expression(
        state: &mut TableState<ResultsTableDelegate>,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        let edit_state = &mut state.delegate_mut().edit_state;
        let Some(cell) = edit_state.editing_cell else {
            return;
        };
        let Some(text) = edit_state
            .editing_input
            .as_ref()
            .map(|input| input.text(cx))
        else {
            return;
        };
        edit_state.editing_as_expression = !edit_state.editing_as_expression;
        edit_state.edited_values.insert(cell, Some(text));
        state.refresh(cx);
        cx.notify();
    }

    pub fn is_editable(&self) -> bool {
        if self
            .db_type
            .is_some_and(|db_type| !db_type.dialect().rows_editable())
        {
            return false;
        }
        self.table_name.is_some() && self.primary_key_is_complete()
    }

    /// The value a cell currently holds: the pending edit when there is one,
    /// otherwise the row data as loaded.
    pub(crate) fn stored_cell_value(&self, row_ix: usize, col_ix: usize) -> Option<String> {
        match self.edit_state.get_edited_value(row_ix, col_ix) {
            Some(edited) => edited.clone(),
            None => self
                .rows
                .get(row_ix)
                .and_then(|row| row.get(col_ix))
                .cloned()
                .flatten(),
        }
    }

    /// Re-indent or minify the JSON in the expanded editor. Text that does
    /// not parse is left as it is.
    pub(crate) fn reflow_expanded_json(
        state: &mut TableState<ResultsTableDelegate>,
        minify: bool,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        let Some(CellInput::Expanded(editor)) = state.delegate().edit_state.editing_input.clone()
        else {
            return;
        };
        let text = editor.read(cx).text().to_string();
        let reflowed = if minify {
            minify_json(&text)
        } else {
            crate::results_panel::json_text::format_json(&text)
        };
        if let Some(reflowed) = reflowed
            && reflowed != text
        {
            editor.update(cx, |editor, cx| editor.set_value(reflowed, window, cx));
        }
    }

    /// Recheck the expanded JSON after an edit.
    pub(crate) fn recheck_expanded_json(&mut self, text: &str) {
        if self.edit_state.expanded_as_json {
            self.edit_state.json_error = json_error(text);
        }
    }

    /// Why an edit pending for a JSON column is not valid JSON, naming the row
    /// and column, so it can be fixed before it is sent. SQL expressions are
    /// the server's to evaluate and are not checked.
    pub fn invalid_json_edit(&self) -> Option<String> {
        let mut cells: Vec<&(usize, usize)> = self.edit_state.edited_values.keys().collect();
        cells.sort();
        cells.into_iter().find_map(|&(row, col)| {
            if self.column_types.get(col) != Some(&ColumnType::Json)
                || self.edit_state.expression_cells.contains(&(row, col))
            {
                return None;
            }
            let text = self.edit_state.edited_values.get(&(row, col))?.as_deref()?;
            let error = json_error(text)?;
            let column = self
                .columns
                .get(col)
                .map(|column| column.name.to_string())
                .unwrap_or_else(|| format!("column {}", col + 1));
            Some(format!("Row {}, {column}: invalid JSON: {error}", row + 1))
        })
    }

    /// Why an edit pending for a binary column is not the bytes it claims to
    /// be: text written as hex (`0x…`) whose digits are not whole bytes. Text
    /// without the prefix is sent as the string it is.
    pub fn invalid_binary_edit(&self) -> Option<String> {
        let mut cells: Vec<&(usize, usize)> = self.edit_state.edited_values.keys().collect();
        cells.sort();
        cells.into_iter().find_map(|&(row, col)| {
            if self.column_types.get(col) != Some(&ColumnType::Binary)
                || self.edit_state.expression_cells.contains(&(row, col))
            {
                return None;
            }
            let text = self.edit_state.edited_values.get(&(row, col))?.as_deref()?;
            if !blanco_core::binary::is_hex_notation(text)
                || blanco_core::binary::hex_digits(text).is_some()
            {
                return None;
            }
            let column = self
                .columns
                .get(col)
                .map(|column| column.name.to_string())
                .unwrap_or_else(|| format!("column {}", col + 1));
            Some(format!(
                "Row {}, {column}: invalid hex: expected pairs of hex digits after 0x",
                row + 1
            ))
        })
    }

    /// What a date or time cell holds, which decides whether the cell editor
    /// offers the picker and what it edits. The declared column type decides
    /// when the table's metadata is known, since an interval's text can read
    /// like a time of day.
    pub(crate) fn temporal_shape(&self, row_ix: usize, col_ix: usize) -> Option<TemporalShape> {
        if self.column_types.get(col_ix) != Some(&ColumnType::DateTime) {
            return None;
        }
        match self.table_column_info(col_ix) {
            Some(info) => TemporalShape::from_data_type(&info.data_type),
            None => self
                .stored_cell_value(row_ix, col_ix)
                .and_then(|value| TemporalValue::parse(&value))
                .map(|value| value.shape()),
        }
    }

    /// Open the date and time picker on the cell being edited, starting from
    /// the text in its editor.
    pub(crate) fn open_date_time_picker(
        state: &mut TableState<ResultsTableDelegate>,
        shape: TemporalShape,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        let Some(cell) = state.delegate().edit_state.editing_cell else {
            return;
        };
        let text = state
            .delegate()
            .edit_state
            .editing_input
            .as_ref()
            .map(|input| input.text(cx))
            .unwrap_or_default();
        let time_zone = results_settings(cx).time_zone;
        let picker = cx.new(|cx| {
            DateTimePicker::new(shape, TemporalValue::parse(&text), time_zone, window, cx)
        });
        cx.subscribe_in(&picker, window, move |state, _picker, event, window, cx| {
            Self::handle_date_time_picker_event(state, cell, event, window, cx);
        })
        .detach();
        if let Some(focus_handle) = picker.read(cx).initial_focus(cx) {
            focus_handle.focus(window, cx);
        }
        state.delegate_mut().edit_state.date_picker = Some(picker);
        state.refresh(cx);
        cx.notify();
    }

    pub(crate) fn close_date_time_picker(
        state: &mut TableState<ResultsTableDelegate>,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        if state.delegate_mut().edit_state.date_picker.take().is_some() {
            state.refresh(cx);
            cx.notify();
        }
    }

    /// A picked value replaces the editor's text, in the form the driver
    /// returns, and the edit stays open so it is committed the same way as a
    /// typed one.
    fn handle_date_time_picker_event(
        state: &mut TableState<ResultsTableDelegate>,
        cell: (usize, usize),
        event: &DateTimePickerEvent,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        let edit_state = &mut state.delegate_mut().edit_state;
        edit_state.date_picker = None;
        if let DateTimePickerEvent::Pick(value) = event
            && edit_state.editing_cell == Some(cell)
        {
            let text = value.to_edit_text();
            if let Some(CellInput::Inline(input)) = edit_state.editing_input.clone() {
                input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
            }
            // A picked value is data, never SQL to evaluate.
            edit_state.editing_as_expression = false;
            edit_state.edited_values.insert(cell, Some(text));
        }
        state.refresh(cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn date_time_picker(&self) -> Option<gpui::Entity<DateTimePicker>> {
        self.edit_state.date_picker.clone()
    }

    /// Whether the cell's value is bytes the binary viewer can show: a
    /// binary column holding NULL or hex notation. Text someone typed into a
    /// binary cell without the `0x` is text and stays in the text editors.
    pub(crate) fn opens_in_binary_viewer(&self, row_ix: usize, col_ix: usize) -> bool {
        self.column_types.get(col_ix) == Some(&ColumnType::Binary)
            && !self.edit_state.is_expression(row_ix, col_ix)
            && self
                .stored_cell_value(row_ix, col_ix)
                .is_none_or(|value| blanco_core::binary::hex_digits(&value).is_some())
    }

    /// Whether a binary cell is too long for the single-line input, whose
    /// text would be twice the value's size on one line.
    pub(crate) fn binary_exceeds_inline_input(&self, row_ix: usize, col_ix: usize) -> bool {
        const INLINE_BYTES: usize = 64;
        self.column_types.get(col_ix) == Some(&ColumnType::Binary)
            && self
                .stored_cell_value(row_ix, col_ix)
                .and_then(|value| blanco_core::binary::byte_length(&value))
                .is_some_and(|length| length > INLINE_BYTES)
    }

    /// Expand a binary cell into the viewer. The cell counts as being
    /// edited while it is open, so the grid draws the viewer in its place.
    pub(crate) fn open_binary_viewer(
        state: &mut TableState<ResultsTableDelegate>,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) -> Entity<BinaryViewer> {
        let delegate = state.delegate();
        let bytes = delegate
            .stored_cell_value(row_ix, col_ix)
            .and_then(|value| blanco_core::binary::decode(&value));
        let editable = delegate.is_editable();
        let file_stem = delegate.binary_file_stem(col_ix);
        let viewer = cx.new(|cx| BinaryViewer::new(bytes, editable, file_stem, cx));
        cx.subscribe_in(&viewer, window, move |state, _, event, window, cx| {
            Self::handle_binary_viewer_event(state, (row_ix, col_ix), event, window, cx);
        })
        .detach();

        let original = delegate
            .rows
            .get(row_ix)
            .and_then(|row| row.get(col_ix))
            .cloned()
            .flatten();
        let is_new_row = delegate.edit_state.is_new_row(row_ix);
        let edit_state = &mut state.delegate_mut().edit_state;
        if !is_new_row {
            edit_state
                .original_values
                .entry((row_ix, col_ix))
                .or_insert(original);
        }
        edit_state.editing_cell = Some((row_ix, col_ix));
        edit_state.expanded_cell = Some((row_ix, col_ix));
        edit_state.editing_input = None;
        edit_state.date_picker = None;
        edit_state.editing_as_expression = false;
        edit_state.binary_viewer = Some(viewer.clone());

        viewer.focus_handle(cx).focus(window, cx);
        state.refresh(cx);
        cx.notify();
        viewer
    }

    /// File name offered when a binary cell is saved: the table and column.
    fn binary_file_stem(&self, col_ix: usize) -> String {
        let column = self
            .columns
            .get(col_ix)
            .map(|column| column.name.to_string())
            .unwrap_or_else(|| "value".to_string());
        let stem = match &self.table_name {
            Some(table) => format!("{table}_{column}"),
            None => column,
        };
        stem.chars()
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn handle_binary_viewer_event(
        state: &mut TableState<ResultsTableDelegate>,
        cell: (usize, usize),
        event: &BinaryViewerEvent,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        if state.delegate().edit_state.editing_cell != Some(cell) {
            return;
        }
        match event {
            BinaryViewerEvent::Loaded(bytes) => {
                if state.delegate().is_editable() {
                    state
                        .delegate_mut()
                        .edit_state
                        .edited_values
                        .insert(cell, Some(blanco_core::binary::encode(bytes)));
                    state.refresh(cx);
                    cx.notify();
                }
            }
            BinaryViewerEvent::Close => Self::close_binary_viewer(state, window, cx),
        }
    }

    /// Close the binary viewer, keeping a loaded file as the cell's pending
    /// edit.
    pub(crate) fn close_binary_viewer(
        state: &mut TableState<ResultsTableDelegate>,
        window: &mut Window,
        cx: &mut Context<'_, TableState<ResultsTableDelegate>>,
    ) {
        state.delegate_mut().finish_binary_viewer();
        state.focus_handle(cx).focus(window, cx);
        state.refresh(cx);
        cx.notify();
    }

    /// End the binary viewer's edit: a loaded file becomes a pending change,
    /// an untouched value is left as it was.
    pub(crate) fn finish_binary_viewer(&mut self) {
        if self.edit_state.binary_viewer.is_none() {
            return;
        }
        if let Some((row, col)) = self.edit_state.editing_cell
            && self.edit_state.is_edited(row, col)
        {
            self.commit_cell_edit(row, col);
        }
        self.edit_state.stop_editing();
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

        // Re-compact formatted JSON back to its single-line form. The
        // single-line input strips newlines but leaves indentation behind,
        // which otherwise mangles the value, so minify it.
        let is_json = is_json || state.delegate().edit_state.expanded_as_json;
        if is_json && let Some(compact) = minify_json(&current_text) {
            current_text = compact;
        }
        state.delegate_mut().edit_state.expanded_as_json = false;
        state.delegate_mut().edit_state.json_error = None;

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
        if state.delegate().opens_in_binary_viewer(row_ix, col_ix) {
            Self::open_binary_viewer(state, row_ix, col_ix, window, cx);
            return;
        }

        let inline_text = state
            .delegate_mut()
            .edit_state
            .editing_input
            .as_ref()
            .map(|input| input.text(cx))
            .unwrap_or_default();

        // The single-line input silently drops line breaks, so its text is a
        // lossy view of a multi-line value. When the user has not typed over
        // it, restore the stored value so the expanded editor shows the real
        // lines instead of them run together.
        let current_text = match state.delegate().stored_cell_value(row_ix, col_ix) {
            Some(stored) if flatten_line_breaks(&stored) == flatten_line_breaks(&inline_text) => {
                stored
            }
            _ => inline_text,
        };

        // Toggle expanded state
        state
            .delegate_mut()
            .edit_state
            .toggle_expanded(row_ix, col_ix);

        // A text value holding a JSON object or array is edited like a JSON
        // column.
        let is_json = is_json
            || (state.delegate().column_types.get(col_ix) == Some(&ColumnType::Text)
                && is_json_document(&current_text));
        {
            let edit_state = &mut state.delegate_mut().edit_state;
            edit_state.expanded_as_json = is_json;
            edit_state.json_error = is_json.then(|| json_error(&current_text)).flatten();
        }

        // Recreate the state as a multi-line editor
        let new_input = cx.new(|cx| {
            // Line numbers fill the gutter the code editor reserves anyway.
            let editor = EditorState::new(window, cx)
                .line_number(true)
                .soft_wrap(true);

            if is_json {
                editor
                    .language("json")
                    .default_value(pretty_json_or_original(current_text))
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

/// Mirror of what the single-line input does to text it is given.
fn flatten_line_breaks(text: &str) -> String {
    text.replace(['\n', '\r'], "")
}
