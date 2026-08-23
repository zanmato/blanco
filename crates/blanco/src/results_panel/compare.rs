//! "Select for comparison" / "Compare with selected" on result cells and rows,
//! opening a side by side diff dialog.

use gpui::{AppContext as _, Context, ParentElement as _, SharedString, Styled as _, Window, px};
use gpui_component::{
    WindowExt as _,
    button::Button,
    dialog::{DialogClose, DialogFooter},
};

use blanco_core::connection_trait::ColumnType;
use blanco_ui::{DiffSource, DiffView};

use crate::app::{
    ClearCompareSelection, CompareCellWithSelected, CompareRowWithSelected, SelectCellForCompare,
    SelectRowForCompare,
};

use super::{ResultsPanel, ResultsTableDelegate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareKind {
    Cell,
    Row,
}

/// Snapshot of the first half of a comparison. Holding the text rather than a
/// cell coordinate keeps it valid across result tabs and query re-runs.
pub struct CompareSelection {
    pub kind: CompareKind,
    pub source: DiffSource,
}

/// Pretty print `text` when it parses as JSON, otherwise return it unchanged.
pub fn pretty_json_or_original(text: String) -> String {
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or(text),
        Err(_) => text,
    }
}

/// The value the grid currently shows for a cell: a pending edit wins over
/// the loaded value. `None` is SQL NULL.
fn displayed_value(
    delegate: &ResultsTableDelegate,
    row: usize,
    col: usize,
) -> Option<Option<String>> {
    if delegate.edit_state.is_edited(row, col) {
        return delegate.edit_state.get_edited_value(row, col).cloned();
    }
    delegate.rows.get(row)?.get(col).cloned()
}

fn is_json_column(delegate: &ResultsTableDelegate, col: usize) -> bool {
    delegate.column_types.get(col) == Some(&ColumnType::Json)
}

pub fn cell_compare_source(
    delegate: &ResultsTableDelegate,
    row: usize,
    col: usize,
) -> Option<DiffSource> {
    let value = displayed_value(delegate, row, col)?;
    let column_name = delegate
        .columns
        .get(col)
        .map(|column| column.name.to_string())
        .unwrap_or_else(|| format!("column {}", col + 1));
    let is_json = is_json_column(delegate, col);
    let text = match value {
        None => "NULL".to_string(),
        Some(text) if is_json => pretty_json_or_original(text),
        Some(text) => text,
    };
    Some(DiffSource {
        title: SharedString::from(format!("{column_name} (row {})", row + 1)),
        text,
        language: is_json.then(|| SharedString::from("json")),
    })
}

/// Render a row as one `column: value` line per column. JSON values are
/// pretty printed and indented under the column name so the diff aligns by
/// JSON line rather than treating the whole document as one changed line.
pub fn row_compare_source(delegate: &ResultsTableDelegate, row: usize) -> Option<DiffSource> {
    if row >= delegate.rows.len() {
        return None;
    }
    let mut lines = Vec::new();
    for (col, column) in delegate.columns.iter().enumerate() {
        let name = column.name.as_ref();
        match displayed_value(delegate, row, col).flatten() {
            None => lines.push(format!("{name}: NULL")),
            Some(text) if is_json_column(delegate, col) => {
                let pretty = pretty_json_or_original(text);
                if pretty.contains('\n') {
                    lines.push(format!("{name}:"));
                    lines.extend(pretty.lines().map(|line| format!("  {line}")));
                } else {
                    lines.push(format!("{name}: {pretty}"));
                }
            }
            Some(text) => lines.push(format!("{name}: {text}")),
        }
    }
    Some(DiffSource {
        title: SharedString::from(format!("Row {}", row + 1)),
        text: lines.join("\n"),
        language: None,
    })
}

impl ResultsPanel {
    pub(super) fn compare_selection_kind(&self) -> Option<CompareKind> {
        self.compare_selection
            .as_ref()
            .map(|selection| selection.kind)
    }

    /// Push the panel wide selection kind into every tab's delegate so each
    /// table's context menu offers the matching "Compare with Selected" item.
    pub(super) fn sync_compare_selection_kind(&mut self, cx: &mut Context<Self>) {
        let kind = self.compare_selection_kind();
        for tab in &self.result_tabs {
            tab.table_state.update(cx, |state, _cx| {
                state.delegate_mut().compare_selection_kind = kind;
            });
        }
    }

    fn set_compare_selection(
        &mut self,
        kind: CompareKind,
        source: Option<DiffSource>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = source else {
            tracing::warn!("Nothing to select for comparison at the requested position");
            return;
        };
        self.compare_selection = Some(CompareSelection { kind, source });
        self.sync_compare_selection_kind(cx);
        cx.notify();
    }

    pub(super) fn on_select_cell_for_compare(
        &mut self,
        action: &SelectCellForCompare,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source =
            cell_compare_source(self.table_state.read(cx).delegate(), action.row, action.col);
        self.set_compare_selection(CompareKind::Cell, source, cx);
    }

    pub(super) fn on_select_row_for_compare(
        &mut self,
        action: &SelectRowForCompare,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = row_compare_source(self.table_state.read(cx).delegate(), action.row);
        self.set_compare_selection(CompareKind::Row, source, cx);
    }

    pub(super) fn on_clear_compare_selection(
        &mut self,
        _action: &ClearCompareSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.compare_selection = None;
        self.sync_compare_selection_kind(cx);
        cx.notify();
    }

    pub(super) fn on_compare_cell_with_selected(
        &mut self,
        action: &CompareCellWithSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let right =
            cell_compare_source(self.table_state.read(cx).delegate(), action.row, action.col);
        self.open_compare_dialog(CompareKind::Cell, right, window, cx);
    }

    pub(super) fn on_compare_row_with_selected(
        &mut self,
        action: &CompareRowWithSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let right = row_compare_source(self.table_state.read(cx).delegate(), action.row);
        self.open_compare_dialog(CompareKind::Row, right, window, cx);
    }

    /// Diff the stored selection (left) against `right`. The selection is kept
    /// so the same base can be compared against several targets in turn.
    fn open_compare_dialog(
        &mut self,
        kind: CompareKind,
        right: Option<DiffSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.compare_selection.as_ref().filter(|s| s.kind == kind) else {
            tracing::warn!("Compare requested without a matching comparison selection");
            return;
        };
        let Some(right) = right else {
            tracing::warn!("Nothing to compare at the requested position");
            return;
        };
        let left = DiffSource {
            title: selection.source.title.clone(),
            text: selection.source.text.clone(),
            language: selection.source.language.clone(),
        };

        let diff_view = cx.new(|cx| DiffView::new(left, right, cx));
        self.last_diff_view = Some(diff_view.clone());
        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title("Compare")
                .w(px(1100.))
                .h(px(700.))
                .child(diff_view.clone())
                .footer(
                    DialogFooter::new().child(
                        DialogClose::new().child(Button::new("close").label("Close").outline()),
                    ),
                )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_component::table::Column;

    fn delegate(
        columns: &[(&str, ColumnType)],
        rows: Vec<Vec<Option<&str>>>,
    ) -> ResultsTableDelegate {
        let mut delegate = ResultsTableDelegate::default();
        delegate.columns = columns
            .iter()
            .enumerate()
            .map(|(i, (name, _))| Column::new(format!("col_{}", i + 1), name.to_string()))
            .collect();
        delegate.column_types = columns.iter().map(|(_, kind)| *kind).collect();
        delegate.rows = rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|cell| cell.map(str::to_string))
                    .collect()
            })
            .collect();
        delegate
    }

    #[test]
    fn row_source_lists_columns_in_order_with_null_and_pretty_json() {
        let delegate = delegate(
            &[
                ("id", ColumnType::Integer),
                ("meta", ColumnType::Json),
                ("note", ColumnType::Text),
            ],
            vec![vec![Some("42"), Some(r#"{"plan":"pro"}"#), None]],
        );
        let source = row_compare_source(&delegate, 0).expect("row exists");
        assert_eq!(source.title.as_ref(), "Row 1");
        assert_eq!(
            source.text,
            "id: 42\nmeta:\n  {\n    \"plan\": \"pro\"\n  }\nnote: NULL"
        );
        assert!(source.language.is_none());
        assert!(row_compare_source(&delegate, 1).is_none());
    }

    #[test]
    fn cell_source_pretty_prints_json_and_prefers_pending_edit() {
        let mut delegate = delegate(
            &[("meta", ColumnType::Json)],
            vec![vec![Some(r#"{"a":1}"#)]],
        );
        let source = cell_compare_source(&delegate, 0, 0).expect("cell exists");
        assert_eq!(source.text, "{\n  \"a\": 1\n}");
        assert_eq!(source.language.as_deref(), Some("json"));
        assert_eq!(source.title.as_ref(), "meta (row 1)");

        delegate.update_cell_value(0, 0, Some(r#"{"a":2}"#.to_string()));
        let source = cell_compare_source(&delegate, 0, 0).expect("cell exists");
        assert_eq!(source.text, "{\n  \"a\": 2\n}");

        let delegate = delegate_null();
        let source = cell_compare_source(&delegate, 0, 0).expect("cell exists");
        assert_eq!(source.text, "NULL");
        assert!(source.language.is_none());
    }

    fn delegate_null() -> ResultsTableDelegate {
        delegate(&[("note", ColumnType::Text)], vec![vec![None]])
    }
}
