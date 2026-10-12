//! Tab strip operations: reorder, duplicate, pin, move to another window and
//! the bulk closes.

use app_database::AppDatabase;
use gpui::{Context, Window};

use super::tabs::TabFile;
use super::{EditorPanel, TabCreationParams, TabType};
use crate::result_ext::ResultExt as _;

/// A query or script tab taken out of one window to be shown in another.
/// The tab's entities belong to the window they were made in, so the other
/// window rebuilds the tab from this, keeping its row in the app database.
pub struct MovedTab {
    is_script: bool,
    params: TabCreationParams,
    file: Option<TabFile>,
}

impl EditorPanel {
    pub(super) fn is_pinned(&self, ix: usize) -> bool {
        ix < self.pinned_count
    }

    /// Whether the tab at `from` may be dropped at `to`. The strip mixes the
    /// tabs of every connection, so only pinned and unpinned tabs stay apart.
    pub(super) fn can_move_tab(&self, from: usize, to: usize) -> bool {
        from != to
            && from < self.tabs.len()
            && to < self.tabs.len()
            && self.is_pinned(from) == self.is_pinned(to)
    }

    /// Move the tab at `from` to `to`, keeping the same tab active.
    pub(super) fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if !self.can_move_tab(from, to) {
            return;
        }
        self.reposition_tab(from, to);
        self.save_tab_positions(cx);
        cx.notify();
    }

    /// Move a tab without the group checks, keeping the same tab active.
    fn reposition_tab(&mut self, from: usize, to: usize) {
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active_tab_ix = moved_index(self.active_tab_ix, from, to);
    }

    /// Pin the tab, moving it to the end of the pinned tabs, or unpin it,
    /// moving it to the start of the others.
    pub(super) fn toggle_pin(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let target = if self.is_pinned(ix) {
            self.pinned_count -= 1;
            self.pinned_count
        } else {
            self.pinned_count += 1;
            self.pinned_count - 1
        };
        self.reposition_tab(ix, target);
        self.scroll_tabbar_to_tab(target, None, window);
        self.save_tab_positions(cx);
        cx.notify();
    }

    /// Whether the tab can be duplicated: query and script tabs, which are
    /// text on a connection.
    pub(super) fn can_duplicate_tab(&self, ix: usize) -> bool {
        self.tabs
            .get(ix)
            .is_some_and(|tab| tab.connection_tab().is_some())
    }

    /// Open a copy of the query or script tab right after it, with the same
    /// text and connection.
    pub(super) fn duplicate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        let Some(base) = tab.connection_tab() else {
            return;
        };
        let params = TabCreationParams {
            title: format!("{} copy", base.title),
            content: Some(base.editor.read(cx).text().to_string()),
            db_id: None,
            last_run_at: None,
            context: base.context.clone(),
        };
        let is_script = matches!(tab, TabType::Script(_));
        // Adding a tab scrolls the strip to its end, where new tabs appear.
        // The copy goes next to its original, so scroll from here instead.
        let scroll_offset = self.tabbar_scroll_handle.offset();
        if is_script {
            self.create_and_add_script_tab(window, params, cx);
        } else {
            self.create_and_add_tab_with_connection(window, params, cx);
        }
        // The new tab is last; put it next to the original.
        let new_ix = self.tabs.len() - 1;
        let target = (ix + 1).max(self.pinned_count);
        self.reposition_tab(new_ix, target);
        self.active_tab_ix = target;
        self.scroll_tabbar_to_tab(target, Some(scroll_offset), window);
        self.save_tab_positions(cx);
        cx.notify();
    }

    pub(crate) fn active_tab_index(&self) -> usize {
        self.active_tab_ix
    }

    /// The names of the files whose tabs have unsaved changes.
    pub(crate) fn unsaved_file_names(&self) -> Vec<String> {
        self.tabs
            .iter()
            .filter_map(TabType::connection_tab)
            .filter_map(|tab| tab.file.as_ref())
            .filter(|file| file.dirty)
            .map(TabFile::name)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn tab_db_id(&self, ix: usize) -> Option<i64> {
        self.tabs.get(ix).and_then(TabType::db_id)
    }

    /// Whether the tab at `ix` can go to a window of its own: a query or
    /// script tab, and not the only tab, which is a window already.
    pub(crate) fn can_move_tab_to_new_window(&self, ix: usize) -> bool {
        self.tabs.len() > 1
            && self
                .tabs
                .get(ix)
                .is_some_and(|tab| tab.connection_tab().is_some())
    }

    /// Take the tab at `ix` out of this window for `adopt_tab` in another.
    /// Unlike closing it, its row stays and it is not offered by Reopen
    /// Closed Tab.
    pub(crate) fn take_tab(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<MovedTab> {
        if !self.can_move_tab_to_new_window(ix) {
            return None;
        }
        let is_script = matches!(self.tabs.get(ix), Some(TabType::Script(_)));
        let connection_tab = self.tabs.get_mut(ix)?.connection_tab_mut()?;
        // The other window saves the tab as soon as it has it.
        self.autosave_tasks
            .remove(&connection_tab.editor.entity_id());
        let moved = MovedTab {
            is_script,
            file: connection_tab.file.take(),
            params: TabCreationParams {
                title: connection_tab.title.clone(),
                content: Some(connection_tab.editor.read(cx).text().to_string()),
                db_id: connection_tab.db_id,
                last_run_at: connection_tab.last_run_at,
                context: connection_tab.context.clone(),
            },
        };
        self.tabs.remove(ix);
        if ix < self.pinned_count {
            self.pinned_count -= 1;
        }
        if ix < self.active_tab_ix {
            self.active_tab_ix -= 1;
        } else if self.active_tab_ix >= self.tabs.len() {
            self.active_tab_ix = self.tabs.len().saturating_sub(1);
        }
        self.save_tab_positions(cx);
        cx.notify();
        Some(moved)
    }

    /// Give the tab at `ix` a window of its own.
    pub(crate) fn move_tab_to_new_window(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.take_tab(ix, cx) else {
            return;
        };
        // The new window's app is built while this panel is still updating.
        cx.defer(move |cx| {
            crate::windows::open_new(Some(tab), cx).log_err();
        });
    }

    /// Show a tab taken from another window as this window's last tab, and
    /// store it as this window's.
    pub(crate) fn adopt_tab(
        &mut self,
        moved: MovedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if moved.is_script {
            self.create_and_add_script_tab(window, moved.params, cx);
        } else {
            self.create_and_add_tab_with_connection(window, moved.params, cx);
        }
        let Some(tab) = self.tabs.last_mut().and_then(TabType::connection_tab_mut) else {
            return;
        };
        tab.file = moved.file;
        let editor = tab.editor.entity_id();
        self.save_tab_for_editor(editor, cx);
        self.focus_active_tab(window, cx);
    }

    /// Close every unpinned tab but the one at `keep`.
    pub(super) fn close_other_tabs(&mut self, keep: usize, cx: &mut Context<Self>) {
        let doomed: Vec<usize> = (self.pinned_count..self.tabs.len())
            .filter(|&ix| ix != keep)
            .collect();
        self.close_tabs(doomed, cx);
    }

    /// Close the unpinned tabs after the one at `ix`.
    pub(super) fn close_tabs_to_the_right(&mut self, ix: usize, cx: &mut Context<Self>) {
        let doomed: Vec<usize> = ((ix + 1).max(self.pinned_count)..self.tabs.len()).collect();
        self.close_tabs(doomed, cx);
    }

    /// Close the tabs at `indices`, highest first so the others keep theirs.
    /// A tab with unsaved changes to its file stays open: closing it asks
    /// first, and a bulk close has no good moment for a question per tab.
    fn close_tabs(&mut self, mut indices: Vec<usize>, cx: &mut Context<Self>) {
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for ix in indices {
            let unsaved = self
                .tabs
                .get(ix)
                .and_then(TabType::connection_tab)
                .and_then(|tab| tab.file.as_ref())
                .is_some_and(|file| file.dirty);
            if !unsaved {
                self.close_tab(ix, cx);
            }
        }
    }

    /// Store the saved tabs' order so a restart restores it.
    fn save_tab_positions(&self, cx: &mut Context<Self>) {
        let positions: Vec<(i64, i32)> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, tab)| Some((tab.db_id()?, ix as i32)))
            .collect();
        if positions.is_empty() {
            return;
        }
        let app_database = AppDatabase::global(cx).clone();
        cx.spawn(async move |_, _| {
            app_database
                .set_query_tab_positions(positions)
                .await
                .map_err(anyhow::Error::from)
                .log_err();
        })
        .detach();
    }
}

/// Where the tab at `ix` ends up after the tab at `from` moves to `to`.
fn moved_index(ix: usize, from: usize, to: usize) -> usize {
    if ix == from {
        to
    } else if from < to && (from + 1..=to).contains(&ix) {
        ix - 1
    } else if to < from && (to..from).contains(&ix) {
        ix + 1
    } else {
        ix
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::test_harness::TestHarness;

    fn titles(harness: &TestHarness, cx: &VisualTestContext) -> Vec<String> {
        harness.editor_panel.read_with(cx, |panel, cx| {
            panel.tabs.iter().map(|tab| tab.title(cx)).collect()
        })
    }

    /// Four query tabs on the harness connection, titled a to d, with `b`
    /// active.
    fn four_tabs(harness: &TestHarness, cx: &mut VisualTestContext) {
        harness.editor_panel.update_in(cx, |panel, window, cx| {
            let context = panel.tabs[0].context(cx).expect("query tab context");
            panel.tabs.clear();
            for title in ["a", "b", "c", "d"] {
                panel.create_and_add_tab_with_connection(
                    window,
                    TabCreationParams {
                        title: title.into(),
                        content: Some(format!("select '{title}'")),
                        db_id: None,
                        last_run_at: None,
                        context: context.clone(),
                    },
                    cx,
                );
            }
            panel.active_tab_ix = 1;
        });
    }

    #[gpui::test]
    async fn reorder_pin_and_close(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        four_tabs(&harness, &mut cx);

        // Moving keeps the active tab active.
        harness
            .editor_panel
            .update(&mut cx, |panel, cx| panel.move_tab(0, 2, cx));
        assert_eq!(titles(&harness, &cx), ["b", "c", "a", "d"]);
        let active = harness
            .editor_panel
            .read_with(&cx, |panel, cx| panel.tabs[panel.active_tab_ix].title(cx));
        assert_eq!(active, "b");

        // Pinning moves the tab to the front, and pinned tabs stay put.
        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| panel.toggle_pin(3, window, cx));
        assert_eq!(titles(&harness, &cx), ["d", "b", "c", "a"]);
        harness.editor_panel.update(&mut cx, |panel, cx| {
            assert!(!panel.can_move_tab(1, 0), "unpinned cannot pass pinned");
            panel.close_other_tabs(2, cx);
        });
        assert_eq!(titles(&harness, &cx), ["d", "c"]);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.toggle_pin(0, window, cx);
                panel.close_tabs_to_the_right(0, cx);
            });
        assert_eq!(titles(&harness, &cx), ["d"]);
    }

    /// Saving a temporary connection recreates its tabs on the saved one.
    /// The pinned ones stay pinned, and no other tab becomes pinned.
    #[gpui::test]
    async fn moving_tabs_to_a_connection_keeps_pins(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        four_tabs(&harness, &mut cx);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.toggle_pin(2, window, cx);
                let context = panel.tabs[0].context(cx).expect("query tab context");
                let from = context.connection_id;
                panel.move_tabs_to_connection(from, &context, window, cx);
            });
        assert_eq!(titles(&harness, &cx), ["c", "a", "b", "d"]);
        harness.editor_panel.read_with(&cx, |panel, _| {
            assert_eq!(panel.pinned_count, 1);
        });
    }

    #[gpui::test]
    async fn duplicate_copies_text_next_to_the_original(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        four_tabs(&harness, &mut cx);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.duplicate_tab(1, window, cx)
            });
        assert_eq!(titles(&harness, &cx), ["a", "b", "b copy", "c", "d"]);
        harness.editor_panel.read_with(&cx, |panel, cx| {
            assert_eq!(panel.active_tab_ix, 2);
            let text = panel.tabs[2]
                .connection_tab()
                .expect("query tab")
                .editor
                .read(cx)
                .text()
                .to_string();
            assert_eq!(text, "select 'b'");
            assert_eq!(panel.tabs[2].context(cx), panel.tabs[1].context(cx));
        });
    }
}
