//! Jump to Column: a fuzzy list of the active result's column names. Picking
//! one selects its cell on the current row and scrolls it into view, which
//! saves scrolling sideways through a wide result.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Focusable as _, ParentElement as _, SharedString, Styled as _,
    Task, Window, px,
};
use gpui_component::{
    IndexPath, WindowExt as _,
    list::{List, ListDelegate, ListEvent, ListItem, ListState},
};

use super::ResultsPanel;
use crate::app::JumpToColumn;
use crate::command_palette::fuzzy_score;

struct ColumnPickerDelegate {
    /// Column names in result order.
    columns: Vec<SharedString>,
    /// Indices into `columns` that match the query, best first.
    matches: Vec<usize>,
    selected: Option<IndexPath>,
}

impl ColumnPickerDelegate {
    fn new(columns: Vec<SharedString>) -> Self {
        let mut delegate = Self {
            columns,
            matches: Vec::new(),
            selected: None,
        };
        delegate.filter("");
        delegate
    }

    fn filter(&mut self, query: &str) {
        self.matches = matching_columns(&self.columns, query);
        self.selected = (!self.matches.is_empty()).then(|| IndexPath::new(0));
    }

    fn column_at(&self, ix: IndexPath) -> Option<usize> {
        self.matches.get(ix.row).copied()
    }
}

/// The indices of `columns` whose name matches `query`, best match first and
/// otherwise in result order. Matching ignores case.
fn matching_columns(columns: &[SharedString], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut scored: Vec<(i64, usize)> = columns
        .iter()
        .enumerate()
        .filter_map(|(ix, name)| Some((fuzzy_score(&name.to_lowercase(), &query)?, ix)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, ix)| ix).collect()
}

impl ListDelegate for ColumnPickerDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.matches.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let name = self.columns.get(self.column_at(ix)?)?.clone();
        Some(ListItem::new(ix).child(name))
    }

    fn perform_search(
        &mut self,
        query: &str,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.filter(query);
        cx.notify();
        Task::ready(())
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
        cx.notify();
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
    }
}

impl ResultsPanel {
    pub(super) fn on_jump_to_column(
        &mut self,
        _: &JumpToColumn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_column_picker(window, cx);
    }

    /// Open the column list of the active result.
    pub fn open_column_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let table_state = self.table_state.clone();
        let columns: Vec<SharedString> = table_state
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();
        if columns.is_empty() {
            return;
        }

        let list = cx.new(|cx| {
            ListState::new(ColumnPickerDelegate::new(columns), window, cx).searchable(true)
        });
        let jump: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new(move |col_ix, window, cx| {
            window.close_dialog(cx);
            table_state.update(cx, |state, cx| {
                let row_ix = state
                    .selected_cell()
                    .map(|(row_ix, _)| row_ix)
                    .or_else(|| state.selected_row())
                    .unwrap_or(0);
                state.set_selected_cell(row_ix, col_ix, cx);
                state.focus_handle(cx).focus(window, cx);
            });
        });
        cx.subscribe_in(
            &list,
            window,
            move |_, list, event: &ListEvent, window, cx| match event {
                ListEvent::Confirm(ix) => {
                    if let Some(col_ix) = list.read(cx).delegate().column_at(*ix) {
                        jump(col_ix, window, cx);
                    }
                }
                ListEvent::Cancel => window.close_dialog(cx),
                _ => {}
            },
        )
        .detach();

        window.open_dialog(cx, {
            let list = list.clone();
            move |dialog, _window, _cx| {
                dialog
                    .title("Jump to Column")
                    .w(px(420.))
                    .child(List::new(&list).h(px(320.)))
            }
        });
        list.update(cx, |list, cx| list.focus(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matching_columns() {
        let columns: Vec<SharedString> = ["id", "created_at", "customer_id", "Updated_At"]
            .into_iter()
            .map(SharedString::from)
            .collect();
        assert_eq!(matching_columns(&columns, ""), [0, 1, 2, 3]);
        assert_eq!(matching_columns(&columns, "updat"), [3]);
        assert_eq!(matching_columns(&columns, "cu"), [2]);
        assert!(matching_columns(&columns, "zzz").is_empty());
    }
}
