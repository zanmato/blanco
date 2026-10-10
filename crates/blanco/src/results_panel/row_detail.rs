//! The row detail side panel: the selected row as a vertical form, one field
//! per column, next to the results grid.
//!
//! Fields edit through the grid's own pending edit path, so a change shows in
//! the grid, in the pending SQL and in Commit exactly as a cell edit does. A
//! field applies its text when it loses focus, and the panel refills fields in
//! place rather than rebuilding them, so clicking from one field into the next
//! keeps that focus.

use gpui::{
    App, AppContext as _, Context, Entity, EntityId, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    v_flex,
};

use blanco_core::connection_trait::ColumnType;
use blanco_ui::IconName;

use super::ResultsPanel;
use super::compare::pretty_json_or_original;
use crate::app::{RowDetailNextRow, RowDetailPreviousRow};

/// Key context of the panel, which scopes its next and previous row shortcuts.
pub const ROW_DETAIL_CONTEXT: &str = "RowDetail";

/// Starting width of the panel, before the user drags the divider.
pub const ROW_DETAIL_WIDTH: f32 = 360.;

pub(crate) struct RowDetail {
    /// The result table the fields were filled from. Switching result tabs
    /// swaps the table, which refills the panel.
    table_id: Option<EntityId>,
    /// The row shown, an index into the grid's rows.
    row: Option<usize>,
    fields: Vec<DetailField>,
    scroll_handle: ScrollHandle,
}

struct DetailField {
    name: SharedString,
    data_type: SharedString,
    is_json: bool,
    input: Entity<TextareaState>,
    /// The cell's value when the field was last filled. A field whose text
    /// still matches it has no change of its own to apply.
    synced: Option<String>,
    _subscription: Subscription,
}

impl RowDetail {
    pub(crate) fn new() -> Self {
        Self {
            table_id: None,
            row: None,
            fields: Vec::new(),
            scroll_handle: ScrollHandle::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn row(&self) -> Option<usize> {
        self.row
    }

    #[cfg(test)]
    pub(crate) fn field_input(&self, col: usize) -> Option<Entity<TextareaState>> {
        self.fields.get(col).map(|field| field.input.clone())
    }
}

/// A label for a column's type when the result carries no table metadata,
/// as for a query over several tables.
fn type_label(column_type: ColumnType) -> &'static str {
    match column_type {
        ColumnType::Integer => "integer",
        ColumnType::UnsignedInteger => "unsigned integer",
        ColumnType::Numeric => "numeric",
        ColumnType::Text => "text",
        ColumnType::Boolean => "boolean",
        ColumnType::DateTime => "date/time",
        ColumnType::Uuid => "uuid",
        ColumnType::Json => "json",
        ColumnType::Array => "array",
        ColumnType::Binary => "binary",
        ColumnType::Unknown => "",
    }
}

/// The text a field shows for a cell value. NULL shows as an empty field with
/// a NULL placeholder; JSON is pretty printed so nested values get room.
fn field_text(value: Option<&str>, is_json: bool) -> String {
    match value {
        Some(value) if is_json => pretty_json_or_original(value.to_string()),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

/// The value to store for a field's text, or `None` when the text still says
/// what `synced` holds. JSON compares by value, so reformatting is no change,
/// and a changed document is stored compact as the grid's editor stores it.
fn field_change(text: &str, synced: Option<&str>, is_json: bool) -> Option<String> {
    let Some(synced) = synced else {
        // An empty field over NULL is still NULL.
        return (!text.is_empty()).then(|| text.to_string());
    };
    if text == synced {
        return None;
    }
    if is_json && let Ok(typed) = serde_json::from_str::<serde_json::Value>(text) {
        if serde_json::from_str::<serde_json::Value>(synced).is_ok_and(|stored| stored == typed) {
            return None;
        }
        return Some(serde_json::to_string(&typed).unwrap_or_else(|_| text.to_string()));
    }
    Some(text.to_string())
}

impl ResultsPanel {
    pub fn is_row_detail_open(&self) -> bool {
        self.row_detail.is_some()
    }

    pub fn toggle_row_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.row_detail.is_some() {
            self.apply_row_detail_fields(window, cx);
            self.row_detail = None;
        } else {
            self.row_detail = Some(RowDetail::new());
            self.sync_row_detail(window, cx);
        }
        cx.notify();
    }

    /// The table and row the panel should show: the selected row, or the row
    /// already shown while nothing is selected (starting a cell edit clears
    /// the selection) as long as the table and row still exist.
    fn row_detail_target(&self, cx: &App) -> (EntityId, Option<usize>) {
        let table_id = self.table_state.entity_id();
        let state = self.table_state.read(cx);
        let row_count = state.delegate().rows.len();
        let selected = state
            .selected_cell()
            .map(|(row, _)| row)
            .or_else(|| state.selected_row());
        let current = self
            .row_detail
            .as_ref()
            .filter(|detail| detail.table_id == Some(table_id))
            .and_then(|detail| detail.row);
        let row = selected.or(current).filter(|row| *row < row_count);
        (table_id, row)
    }

    /// Whether the panel shows something other than the grid's current row
    /// and values, so render can schedule a refill.
    pub(crate) fn row_detail_out_of_sync(&self, cx: &App) -> bool {
        let Some(detail) = &self.row_detail else {
            return false;
        };
        let (table_id, row) = self.row_detail_target(cx);
        if detail.table_id != Some(table_id) || detail.row != row {
            return true;
        }
        let Some(row) = row else {
            return false;
        };
        let delegate = self.table_state.read(cx).delegate();
        detail.fields.len() != delegate.columns.len()
            || detail.fields.iter().enumerate().any(|(col, field)| {
                field.name.as_str() != delegate.columns[col].name.as_str()
                    || field.synced != delegate.stored_cell_value(row, col)
            })
    }

    /// Refill the panel from the grid. A new table, row or column set
    /// rebuilds the fields; otherwise only fields whose cell changed are
    /// refilled, and a focused field keeps what is being typed into it.
    pub(crate) fn sync_row_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.row_detail_sync_scheduled = false;
        if self.row_detail.is_none() {
            return;
        }
        let (table_id, row) = self.row_detail_target(cx);
        let (columns, values): (Vec<(SharedString, SharedString, bool)>, Vec<Option<String>>) = {
            let delegate = self.table_state.read(cx).delegate();
            let columns = (0..delegate.columns.len())
                .map(|col| {
                    let column_type = delegate.column_types.get(col).copied();
                    let data_type = delegate
                        .table_column_info(col)
                        .map(|info| info.data_type.clone())
                        .or_else(|| column_type.map(|t| type_label(t).to_string()))
                        .unwrap_or_default();
                    (
                        delegate.columns[col].name.clone(),
                        SharedString::from(data_type),
                        column_type == Some(ColumnType::Json),
                    )
                })
                .collect();
            let values = match row {
                Some(row) => (0..delegate.columns.len())
                    .map(|col| delegate.stored_cell_value(row, col))
                    .collect(),
                None => Vec::new(),
            };
            (columns, values)
        };

        let Some(detail) = self.row_detail.as_ref() else {
            return;
        };
        let rebuild = detail.table_id != Some(table_id)
            || detail.row != row
            || detail.fields.len() != columns.len()
            || detail
                .fields
                .iter()
                .zip(&columns)
                .any(|(field, (name, _, _))| field.name != *name);

        if rebuild {
            let fields = match row {
                Some(_) => columns
                    .into_iter()
                    .zip(values)
                    .enumerate()
                    .map(|(col, ((name, data_type, is_json), value))| {
                        let text = field_text(value.as_deref(), is_json);
                        let input = cx.new(|cx| {
                            TextareaState::new(window, cx)
                                .auto_grow(1, 12)
                                .placeholder(if value.is_none() { "NULL" } else { "" })
                                .default_value(text)
                        });
                        let subscription = cx.subscribe_in(
                            &input,
                            window,
                            move |this, _, event: &InputEvent, window, cx| {
                                if let InputEvent::Blur = event {
                                    this.apply_row_detail_field(col, window, cx);
                                }
                            },
                        );
                        DetailField {
                            name,
                            data_type,
                            is_json,
                            input,
                            synced: value,
                            _subscription: subscription,
                        }
                    })
                    .collect(),
                None => Vec::new(),
            };
            if let Some(detail) = self.row_detail.as_mut() {
                detail.table_id = Some(table_id);
                detail.row = row;
                detail.fields = fields;
            }
        } else if let Some(detail) = self.row_detail.as_mut() {
            for (field, value) in detail.fields.iter_mut().zip(values) {
                if field.synced == value {
                    continue;
                }
                if !field.input.focus_handle(cx).is_focused(window) {
                    let text = field_text(value.as_deref(), field.is_json);
                    let placeholder = if value.is_none() { "NULL" } else { "" };
                    field.input.update(cx, |input, cx| {
                        input.set_placeholder(placeholder, window, cx);
                        input.set_value(text, window, cx);
                    });
                }
                field.synced = value;
            }
        }
        cx.notify();
    }

    /// Apply one field's text as a pending edit of its cell. Text that still
    /// says what the cell holds changes nothing.
    pub(crate) fn apply_row_detail_field(
        &mut self,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = &self.row_detail else {
            return;
        };
        let (Some(row), Some(field)) = (detail.row, detail.fields.get(col)) else {
            return;
        };
        if detail.table_id != Some(self.table_state.entity_id()) {
            return;
        }
        let text = field.input.read(cx).value().to_string();
        let Some(value) = field_change(&text, field.synced.as_deref(), field.is_json) else {
            return;
        };
        if self.commit_in_progress || !self.is_editable(cx) {
            // Not editable: put the cell's value back rather than leave text
            // that will never be saved.
            let text = field_text(field.synced.as_deref(), field.is_json);
            field
                .input
                .update(cx, |input, cx| input.set_value(text, window, cx));
            return;
        }
        // A grid cell editor open elsewhere owns `editing_cell`; finish it so
        // this edit does not overwrite its state.
        if let Some(cell) = self.get_current_editing_cell(cx)
            && cell != (row, col)
        {
            self.finalize_active_cell_edit(cx);
            self.table_state.update(cx, |state, cx| {
                state.delegate_mut().edit_state.stop_editing();
                state.refresh(cx);
            });
        }
        self.table_state.update(cx, |state, _| {
            state.delegate_mut().start_editing_cell(row, col);
        });
        self.commit_cell_edit(row, col, value, cx);
    }

    /// Apply every field, for when the panel is about to show another row or
    /// close and a focused field will not see its blur first.
    fn apply_row_detail_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self
            .row_detail
            .as_ref()
            .map_or(0, |detail| detail.fields.len());
        for col in 0..count {
            self.apply_row_detail_field(col, window, cx);
        }
    }

    /// Show the row `delta` away from the current one, selecting it in the
    /// grid, and keep focus on the same field.
    pub(crate) fn move_row_detail(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = &self.row_detail else {
            return;
        };
        let Some(row) = detail.row else {
            return;
        };
        let focused = detail
            .fields
            .iter()
            .position(|field| field.input.focus_handle(cx).is_focused(window));
        let row_count = self.table_state.read(cx).delegate().rows.len();
        let Some(target) = row.checked_add_signed(delta).filter(|row| *row < row_count) else {
            return;
        };
        self.apply_row_detail_fields(window, cx);
        let col = self
            .table_state
            .read(cx)
            .selected_cell()
            .map_or(0, |(_, col)| col);
        self.table_state.update(cx, |state, cx| {
            state.set_selected_cell(target, col, cx);
            state.scroll_to_row(target, cx);
        });
        self.sync_row_detail(window, cx);
        if let Some(field) = focused.and_then(|col| {
            self.row_detail
                .as_ref()
                .and_then(|detail| detail.fields.get(col))
        }) {
            field.input.focus_handle(cx).focus(window, cx);
        }
    }

    fn on_row_detail_next_row(
        &mut self,
        _: &RowDetailNextRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_row_detail(1, window, cx);
    }

    fn on_row_detail_previous_row(
        &mut self,
        _: &RowDetailPreviousRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_row_detail(-1, window, cx);
    }

    pub(crate) fn render_row_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (row_count, editable) = {
            let delegate = self.table_state.read(cx).delegate();
            (delegate.rows.len(), delegate.is_editable())
        };
        let detail = self.row_detail.as_ref();
        let row = detail.and_then(|detail| detail.row);
        let edited = |col: usize| {
            row.is_some_and(|row| {
                self.table_state
                    .read(cx)
                    .delegate()
                    .edit_state
                    .is_edited(row, col)
            })
        };

        v_flex()
            .key_context(ROW_DETAIL_CONTEXT)
            .on_action(cx.listener(Self::on_row_detail_next_row))
            .on_action(cx.listener(Self::on_row_detail_previous_row))
            .size_full()
            .min_w_0()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h(px(32.))
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .truncate()
                            .child(match row {
                                Some(row) => format!("Row {} of {row_count}", row + 1),
                                None => "Row Detail".to_string(),
                            }),
                    )
                    .child(
                        Button::new("row-detail-previous")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronUp)
                            .tooltip("Previous Row")
                            .disabled(row.is_none_or(|row| row == 0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.move_row_detail(-1, window, cx);
                            })),
                    )
                    .child(
                        Button::new("row-detail-next")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronDown)
                            .tooltip("Next Row")
                            .disabled(row.is_none_or(|row| row + 1 >= row_count))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.move_row_detail(1, window, cx);
                            })),
                    )
                    .child(
                        Button::new("row-detail-close")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("Close Row Detail")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_row_detail(window, cx);
                            })),
                    ),
            )
            .child(match detail.filter(|detail| detail.row.is_some()) {
                Some(detail) => {
                    div()
                        .id("row-detail-fields")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&detail.scroll_handle)
                        .child(v_flex().p_3().gap_3().children(
                            detail.fields.iter().enumerate().map(|(col, field)| {
                                v_flex()
                                    .gap_1()
                                    .when(edited(col), |this| {
                                        this.pl_2().border_l_2().border_color(theme.yellow)
                                    })
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .text_sm()
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .truncate()
                                                    .child(field.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(field.data_type.clone()),
                                            ),
                                    )
                                    .child(Textarea::new(&field.input).small().disabled(!editable))
                            }),
                        ))
                        .into_any_element()
                }
                None => div()
                    .flex_1()
                    .p_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Select a row to see its fields.")
                    .into_any_element(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::field_change;

    #[test]
    fn field_change_ignores_untouched_and_reformatted_values() {
        assert_eq!(field_change("", None, false), None);
        assert_eq!(field_change("x", None, false), Some("x".to_string()));
        assert_eq!(field_change("a", Some("a"), false), None);
        assert_eq!(field_change("", Some("a"), false), Some(String::new()));
        // Pretty printed JSON is the same document.
        assert_eq!(
            field_change("{\n  \"a\": 1\n}", Some("{\"a\":1}"), true),
            None
        );
        assert_eq!(
            field_change("{\n  \"a\": 2\n}", Some("{\"a\":1}"), true),
            Some("{\"a\":2}".to_string())
        );
        // Text that is not JSON is stored as typed.
        assert_eq!(
            field_change("{oops", Some("{\"a\":1}"), true),
            Some("{oops".to_string())
        );
    }
}
