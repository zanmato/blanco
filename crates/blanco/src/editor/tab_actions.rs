//! Tab strip operations: reorder, duplicate, pin, move to another window and
//! the bulk closes, which ask to save the changed tabs first.

use std::rc::Rc;

use app_database::AppDatabase;
use gpui::{
    App, Context, EntityId, ParentElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogClose, DialogFooter},
    v_flex,
};

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

/// Which tabs a close command closes, around the tab it is given. Pinned
/// tabs are never among them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseScope {
    Others,
    Left,
    Right,
    All,
}

impl CloseScope {
    /// The indices of the tabs it closes among `len` tabs, the first
    /// `pinned` of them pinned, around `ix`.
    pub(super) fn indices(self, ix: usize, len: usize, pinned: usize) -> Vec<usize> {
        let pinned = pinned.min(len);
        match self {
            Self::Others => (pinned..len).filter(|&other| other != ix).collect(),
            Self::Left => (pinned..ix.min(len)).collect(),
            Self::Right => ((ix + 1).max(pinned)..len).collect(),
            Self::All => (pinned..len).collect(),
        }
    }
}

impl TabType {
    /// The id of the tab's view, which stays the same while tabs move.
    fn view_id(&self) -> EntityId {
        match self {
            TabType::Query(tab) => tab.editor.entity_id(),
            TabType::Script(tab) => tab.editor.entity_id(),
            TabType::Settings(tab) => tab.settings_view.entity_id(),
            TabType::Snippet(tab) => tab.entity_id(),
            TabType::TableStructure(tab) => tab.entity_id(),
            TabType::ObjectDdl(tab) => tab.entity_id(),
            TabType::SchemaGraph(tab) => tab.entity_id(),
            TabType::ProcessList(tab) => tab.entity_id(),
            TabType::Dashboard(tab) => tab.entity_id(),
        }
    }
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

    /// The names of the files and tables whose tabs have unsaved changes.
    pub(crate) fn unsaved_tab_names(&self, cx: &App) -> Vec<String> {
        self.tabs
            .iter()
            .filter_map(|tab| tab.unsaved_name(cx))
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

    /// Close the tabs `scope` names around the tab at `ix`, asking first
    /// when some of them have unsaved changes. Pinned tabs stay open.
    pub(crate) fn close_tabs(
        &mut self,
        scope: CloseScope,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices = scope.indices(ix, self.tabs.len(), self.pinned_count);
        if indices.is_empty() {
            return;
        }
        let ids: Vec<EntityId> = indices.iter().map(|&ix| self.tabs[ix].view_id()).collect();
        // The tab to show afterwards: the active one when it stays open,
        // else the tab the command was given, as with Close Others.
        let keep = [self.active_tab_ix, ix]
            .into_iter()
            .filter_map(|ix| self.tabs.get(ix))
            .map(TabType::view_id)
            .find(|id| !ids.contains(id));
        let unsaved: Vec<(EntityId, String)> = indices
            .iter()
            .filter_map(|&ix| {
                let tab = &self.tabs[ix];
                Some((tab.view_id(), tab.unsaved_name(cx)?))
            })
            .collect();
        if unsaved.is_empty() {
            self.close_tabs_by_id(&ids, keep, window, cx);
        } else {
            self.confirm_close_unsaved(ids, unsaved, keep, window, cx);
        }
    }

    /// Close the tabs `scope` names around the active tab.
    pub(crate) fn close_tabs_around_active(
        &mut self,
        scope: CloseScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tabs(scope, self.active_tab_ix, window, cx);
    }

    /// Ask whether to save the changed files among the tabs being closed.
    /// Saving closes the tabs that were written and keeps the others, such as
    /// a table whose structure changes are not applied.
    fn confirm_close_unsaved(
        &mut self,
        ids: Vec<EntityId>,
        unsaved: Vec<(EntityId, String)>,
        keep: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = match unsaved.as_slice() {
            [(_, name)] => format!("Save changes to {name}?"),
            _ => format!("Save changes to {} tabs?", unsaved.len()),
        };
        let names: Vec<String> = match unsaved.as_slice() {
            [_] => Vec::new(),
            _ => unsaved.iter().map(|(_, name)| name.clone()).collect(),
        };
        let save_label = if unsaved.len() > 1 {
            "Save All"
        } else {
            "Save"
        };
        let has_structure = unsaved.iter().any(|(id, _)| {
            self.tabs
                .iter()
                .any(|tab| matches!(tab, TabType::TableStructure(_)) && tab.view_id() == *id)
        });

        let unsaved_ids: Vec<EntityId> = unsaved.iter().map(|(id, _)| *id).collect();
        let weak = cx.entity().downgrade();
        let respond = Rc::new(move |save: bool, window: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| {
                this.answer_close(&ids, &unsaved_ids, keep, save, window, cx)
            })
            .log_err();
            window.close_dialog(cx);
        });

        window.open_dialog(cx, move |dialog, _, cx| {
            let save = respond.clone();
            let discard = respond.clone();
            dialog
                .title(title.clone())
                .child(
                    v_flex()
                        .gap_2()
                        .when(!names.is_empty(), |this| {
                            this.child(v_flex().children(names.iter().map(|name| {
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(name.clone())
                            })))
                        })
                        .child("Your changes are lost if you close without saving.")
                        .when(has_structure, |this| {
                            this.child(
                                "Tables with structure changes not yet applied stay open when you save.",
                            )
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("close-tabs-cancel").label("Cancel").outline()),
                        )
                        // Sibling `DialogAction`s share a click state, so the
                        // buttons close the dialog themselves.
                        .child(
                            Button::new("close-tabs-discard")
                                .outline()
                                .label("Don't Save")
                                .on_click(move |_, window, cx| discard(false, window, cx)),
                        )
                        .child(
                            Button::new("close-tabs-save")
                                .primary()
                                .label(save_label)
                                .on_click(move |_, window, cx| save(true, window, cx)),
                        ),
                )
        });
    }

    /// Close the tabs after the save prompt was answered. With `save` the
    /// unsaved tabs are written first, and those that could not be stay open.
    fn answer_close(
        &mut self,
        ids: &[EntityId],
        unsaved: &[EntityId],
        keep: Option<EntityId>,
        save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut ids = ids.to_vec();
        if save {
            for id in unsaved {
                if !self.save_tab_file(*id, window, cx) {
                    ids.retain(|other| other != id);
                }
            }
        }
        self.close_tabs_by_id(&ids, keep, window, cx);
    }

    /// Write the file of the tab whose view is `id`. Returns whether it was
    /// written, false for a tab without a file to write.
    fn save_tab_file(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.tabs.iter().position(|tab| tab.view_id() == id) else {
            return false;
        };
        let path = self.tabs[ix]
            .connection_tab()
            .and_then(|tab| tab.file.as_ref())
            .map(|file| file.path.clone());
        path.is_some_and(|path| self.write_tab_to(ix, path, window, cx))
    }

    /// Close the tabs whose views are `ids`, then show `keep` when it is
    /// still open. A window always keeps a tab, so closing every tab first
    /// opens an empty query tab on the active tab's connection.
    fn close_tabs_by_id(
        &mut self,
        ids: &[EntityId],
        mut keep: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let closes_all = self.tabs.iter().all(|tab| ids.contains(&tab.view_id()));
        if closes_all && let Some(context) = self.active_context(cx) {
            let title = match &context.schema_name {
                Some(schema) => format!("{}.{}", context.database_name, schema),
                None => context.database_name.clone(),
            };
            self.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title,
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context,
                },
                cx,
            );
            keep = self.tabs.last().map(TabType::view_id);
        }
        // Highest first, so the other indices stay put.
        for ix in (0..self.tabs.len()).rev() {
            if ids.contains(&self.tabs[ix].view_id()) {
                self.close_tab(ix, cx);
            }
        }
        if let Some(ix) =
            keep.and_then(|keep| self.tabs.iter().position(|tab| tab.view_id() == keep))
        {
            self.active_tab_ix = ix;
            self.tabbar_scroll_handle.scroll_to_item(ix);
        }
        self.focus_active_tab(window, cx);
        cx.notify();
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
        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                assert!(!panel.can_move_tab(1, 0), "unpinned cannot pass pinned");
                panel.close_tabs(CloseScope::Others, 2, window, cx);
            });
        assert_eq!(titles(&harness, &cx), ["d", "c"]);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.toggle_pin(0, window, cx);
                panel.close_tabs(CloseScope::Right, 0, window, cx);
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

    #[test]
    fn close_scopes_skip_pinned_tabs() {
        // Five tabs, the first two pinned, given the tab at 3.
        assert_eq!(CloseScope::Others.indices(3, 5, 2), [2, 4]);
        assert_eq!(CloseScope::Left.indices(3, 5, 2), [2]);
        assert_eq!(CloseScope::Right.indices(3, 5, 2), [4]);
        assert_eq!(CloseScope::All.indices(3, 5, 2), [2, 3, 4]);
        // Given a pinned tab, the scopes still reach only unpinned ones.
        assert_eq!(CloseScope::Left.indices(1, 5, 2), Vec::<usize>::new());
        assert_eq!(CloseScope::Right.indices(0, 5, 2), [2, 3, 4]);
    }

    #[gpui::test]
    async fn close_left_and_all_keep_pinned_tabs(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        four_tabs(&harness, &mut cx);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.toggle_pin(3, window, cx);
                panel.close_tabs(CloseScope::Left, 2, window, cx);
            });
        assert_eq!(titles(&harness, &cx), ["d", "b", "c"]);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.close_tabs(CloseScope::All, 2, window, cx)
            });
        assert_eq!(titles(&harness, &cx), ["d"]);
        harness.editor_panel.read_with(&cx, |panel, _| {
            assert_eq!(panel.active_tab_ix, 0);
        });
    }

    #[gpui::test]
    async fn close_all_without_pins_leaves_an_empty_tab(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        four_tabs(&harness, &mut cx);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.close_tabs(CloseScope::All, 1, window, cx)
            });
        harness.editor_panel.read_with(&cx, |panel, cx| {
            assert_eq!(panel.tabs.len(), 1);
            let tab = panel.tabs[0].connection_tab().expect("a query tab");
            assert!(tab.editor.read(cx).text().to_string().is_empty());
            assert!(!["a", "b", "c", "d"].contains(&tab.title.as_str()));
        });
    }

    /// Tab `c` gets a file in `dir` with other saved text, so it has unsaved
    /// changes. Returns the file's path.
    fn unsaved_file_on_c(
        harness: &TestHarness,
        dir: &tempfile::TempDir,
        cx: &mut VisualTestContext,
    ) -> std::path::PathBuf {
        let path = dir.path().join("c.sql");
        std::fs::write(&path, "old").unwrap();
        let file = TabFile::new(path.clone(), Some("old".into()), "select 'c'");
        harness.editor_panel.update(cx, |panel, _| {
            panel.tabs[2].connection_tab_mut().expect("query tab").file = Some(file);
        });
        path
    }

    #[gpui::test]
    async fn closing_an_unsaved_tab_asks_first(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        four_tabs(&harness, &mut cx);
        let dir = tempfile::tempdir().unwrap();
        let path = unsaved_file_on_c(&harness, &dir, &mut cx);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                panel.close_tabs(CloseScope::Others, 0, window, cx)
            });
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
        assert_eq!(titles(&harness, &cx), ["a", "b", "c", "d"]);

        // Saving writes the file and closes every tab asked for.
        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                let ids: Vec<EntityId> = panel.tabs[1..].iter().map(TabType::view_id).collect();
                let unsaved = [panel.tabs[2].view_id()];
                let keep = Some(panel.tabs[0].view_id());
                panel.answer_close(&ids, &unsaved, keep, true, window, cx);
            });
        assert_eq!(titles(&harness, &cx), ["a"]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "select 'c'");
    }

    #[gpui::test]
    async fn not_saving_closes_without_writing(cx: &mut TestAppContext) {
        let harness = TestHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        four_tabs(&harness, &mut cx);
        let dir = tempfile::tempdir().unwrap();
        let path = unsaved_file_on_c(&harness, &dir, &mut cx);

        harness
            .editor_panel
            .update_in(&mut cx, |panel, window, cx| {
                let ids = [panel.tabs[2].view_id()];
                panel.answer_close(&ids, &ids, None, false, window, cx);
            });
        assert_eq!(titles(&harness, &cx), ["a", "b", "d"]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }
}
