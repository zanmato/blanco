mod delegate;

pub use delegate::{SnippetItemMetadata, SnippetsTreeDelegate};

use std::future::Future;

use crate::app::{NewSnippet, OpenSnippetEditor};
use crate::result_ext::ResultExt;
use app_database::{AppDatabase, EditorKind, SnippetData};
use blanco_ui::draggable_tree::{DraggableTree, DraggableTreeState, TreeItem};
use gpui::{
    AppContext, ClipboardItem, Context, Entity, EventEmitter, Focusable as _, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Task, Window, actions,
    prelude::FluentBuilder, px,
};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{
    ActiveTheme as _,
    input::{InputEvent, InputState},
    v_flex,
};

actions!(
    snippets,
    [
        CreateGroup,
        RefreshSnippets,
        RenameSnippet,
        DuplicateSnippet
    ]
);

#[derive(Clone, Debug)]
pub enum SnippetsPanelEvent {
    SnippetDeleted,
    /// A snippet or group was renamed. Open editors of it take the name.
    SnippetRenamed {
        id: i64,
        name: String,
    },
    /// A new snippet was stored, to be opened in its editor.
    OpenSnippet(i64),
}

/// The name field shown in place of a snippet's label while it is renamed.
struct Renaming {
    id: i64,
    input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// Where a dragged snippet should land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropPlacement {
    /// Append to the top level.
    Root,
    /// Append as the last child of a group.
    IntoGroup(i64),
    /// Reorder next to an existing snippet, becoming its sibling.
    Beside { sibling_id: i64, after: bool },
}

impl EventEmitter<SnippetsPanelEvent> for SnippetsPanel {}

pub struct SnippetsPanel {
    pub snippets: Vec<SnippetData>,
    /// False until the first load lands, so the empty placeholder does not
    /// flash while the snippets are still being read.
    loaded: bool,
    tree_state: Entity<DraggableTreeState<SnippetsTreeDelegate>>,
    renaming: Option<Renaming>,
    /// In-flight snippet load. Replaced on every refresh so overlapping
    /// refreshes cannot apply a stale snapshot over a newer one.
    load_task: Option<Task<()>>,
}

impl SnippetsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel_entity = cx.entity();
        let delegate = SnippetsTreeDelegate::new(&panel_entity);
        let tree_state = cx.new(|cx| DraggableTreeState::new(delegate, cx));

        let mut panel = Self {
            snippets: Vec::new(),
            loaded: false,
            tree_state,
            renaming: None,
            load_task: None,
        };

        panel.refresh_snippets(cx);
        panel
    }

    pub fn refresh_snippets(&mut self, cx: &mut Context<Self>) {
        let app_database = AppDatabase::global(cx).clone();
        let load = gpui_tokio::Tokio::spawn_result(cx, async move {
            app_database
                .load_snippets()
                .await
                .map_err(anyhow::Error::from)
        });
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let Some(snippets) = load.await.log_err() else {
                return;
            };
            this.update(cx, |this, cx| this.apply_snippets(snippets, cx))
                .log_err();
        }));
    }

    fn apply_snippets(&mut self, snippets: Vec<SnippetData>, cx: &mut Context<Self>) {
        crate::snippet_completion::SnippetTriggers::set(&snippets, cx);
        self.snippets = snippets;
        self.loaded = true;
        self.refresh_tree(cx);
        cx.notify();
    }

    /// Run a snippet mutation on tokio, then reload the panel once it finishes.
    fn mutate_then_refresh(
        &mut self,
        mutation: impl Future<Output = anyhow::Result<()>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let task = gpui_tokio::Tokio::spawn_result(cx, mutation);
        cx.spawn(async move |this, cx| {
            task.await.log_err();
            this.update(cx, |this, cx| this.refresh_snippets(cx))
                .log_err();
        })
        .detach();
    }

    fn refresh_tree(&mut self, cx: &mut Context<Self>) {
        let tree_items = self.build_tree_items();
        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items, cx);
        });
    }

    fn build_tree_items(&self) -> Vec<TreeItem> {
        let root_items: Vec<&SnippetData> = self
            .snippets
            .iter()
            .filter(|s| s.parent_id.is_none())
            .collect();

        root_items
            .into_iter()
            .map(|snippet| self.build_tree_item(snippet, &self.snippets))
            .collect()
    }

    fn build_tree_item(&self, snippet: &SnippetData, all_snippets: &[SnippetData]) -> TreeItem {
        let id = SharedString::from(format!("snippet:{}", snippet.id.unwrap_or(0)));
        let label = SharedString::from(snippet.name.clone());
        let _metadata = SnippetItemMetadata {
            id: snippet.id.unwrap_or(0),
            name: snippet.name.clone(),
            is_group: snippet.is_group,
            parent_id: snippet.parent_id,
        };

        let mut item = TreeItem::new(id, label).expanded(true);

        if snippet.is_group {
            let children: Vec<TreeItem> = all_snippets
                .iter()
                .filter(|s| s.parent_id == snippet.id)
                .map(|child| self.build_tree_item(child, all_snippets))
                .collect();

            for child in children {
                item = item.child(child);
            }
        }

        item
    }

    pub fn handle_snippet_double_click(
        &mut self,
        item_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id_str) = item_id.strip_prefix("snippet:")
            && let Ok(id) = id_str.parse::<i64>()
            && let Some(snippet) = self.snippets.iter().find(|s| s.id == Some(id))
        {
            if snippet.is_group {
                // The draggable tree handles this automatically
            } else {
                window.dispatch_action(
                    Box::new(OpenSnippetEditor {
                        snippet_id: Some(id),
                    }),
                    cx,
                );
            }
        }
    }

    pub fn copy_snippet_to_clipboard(&mut self, snippet_id: i64, cx: &mut Context<Self>) {
        if let Some(snippet) = self.snippets.iter().find(|s| s.id == Some(snippet_id)) {
            cx.write_to_clipboard(ClipboardItem::new_string(snippet.content.clone()));
        }
    }

    pub fn delete_snippet(
        &mut self,
        snippet_id: i64,
        _snippet_name: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app_database = AppDatabase::global(cx).clone();
        self.mutate_then_refresh(
            async move {
                app_database
                    .delete_snippet(snippet_id)
                    .await
                    .map(|()| tracing::info!("Deleted snippet: {}", snippet_id))
                    .map_err(anyhow::Error::from)
            },
            cx,
        );
        cx.emit(SnippetsPanelEvent::SnippetDeleted);
    }

    pub fn handle_drop(
        &mut self,
        snippet_id: i64,
        placement: DropPlacement,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app_database = AppDatabase::global(cx).clone();
        let (new_parent_id, ordered_siblings) = plan_drop(&self.snippets, snippet_id, placement);

        let updates: Vec<(i64, Option<i64>, i32)> = ordered_siblings
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, new_parent_id, index as i32))
            .collect();

        cx.spawn(async move |weak_panel, cx: &mut gpui::AsyncApp| {
            match app_database.reorder_snippets(updates).await {
                Ok(_) => {
                    tracing::info!("Moved snippet {} to parent {:?}", snippet_id, new_parent_id);
                }
                Err(e) => {
                    tracing::error!("Failed to move snippet: {}", e);
                }
            }

            // Defer the tree refresh to avoid nested update panic
            weak_panel
                .update(cx, |this, cx| {
                    this.refresh_snippets(cx);
                })
                .log_err();
        })
        .detach();
    }

    /// `base`, or `base 2`, `base 3` and so on: the first name no other item
    /// in the group (or at the top level) has.
    fn unique_name(&self, base: &str, parent_id: Option<i64>) -> String {
        let taken = |name: &str| {
            self.snippets
                .iter()
                .any(|snippet| snippet.parent_id == parent_id && snippet.name == name)
        };
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !taken(name))
            .expect("an unused name always exists")
    }

    /// The position after the last item in the group (or the top level).
    fn next_position(&self, parent_id: Option<i64>) -> i32 {
        self.snippets
            .iter()
            .filter(|snippet| snippet.parent_id == parent_id)
            .map(|snippet| snippet.position + 1)
            .max()
            .unwrap_or(0)
    }

    /// Add a snippet or group named "New Snippet" or "New Group" at the end
    /// of `parent_id` (or the top level) and start renaming it. A snippet
    /// opens in its editor too.
    pub fn create_item(
        &mut self,
        is_group: bool,
        parent_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let base = if is_group { "New Group" } else { "New Snippet" };
        let snippet = SnippetData {
            id: None,
            name: self.unique_name(base, parent_id),
            content: String::new(),
            kind: EditorKind::default(),
            parent_id,
            is_group,
            position: self.next_position(parent_id),
            trigger: None,
        };
        self.insert_then_rename(snippet, !is_group, window, cx);
    }

    /// Store a copy of the snippet, named "<name> copy", at the end of its
    /// group, and start renaming it. The copy has no trigger, since two
    /// snippets can't share one.
    pub fn duplicate_snippet(
        &mut self,
        snippet_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(original) = self
            .snippets
            .iter()
            .find(|snippet| snippet.id == Some(snippet_id) && !snippet.is_group)
            .cloned()
        else {
            return;
        };
        let copy = SnippetData {
            id: None,
            name: self.unique_name(&format!("{} copy", original.name), original.parent_id),
            position: self.next_position(original.parent_id),
            trigger: None,
            ..original
        };
        self.insert_then_rename(copy, false, window, cx);
    }

    fn insert_then_rename(
        &mut self,
        snippet: SnippetData,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app_database = AppDatabase::global(cx).clone();
        let insert = gpui_tokio::Tokio::spawn_result(cx, async move {
            let id = app_database.save_snippet(&snippet).await?;
            let snippets = app_database.load_snippets().await?;
            Ok((id, snippets))
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some((id, snippets)) = insert.await.log_err() else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.apply_snippets(snippets, cx);
                if open {
                    cx.emit(SnippetsPanelEvent::OpenSnippet(id));
                }
                this.start_rename(id, window, cx);
            })
            .log_err();
        })
        .detach();
    }

    /// The id of the selected snippet or group.
    fn selected_id(&self, cx: &gpui::App) -> Option<i64> {
        let entry = self.tree_state.read(cx).selected_entry()?;
        entry.item().id.strip_prefix("snippet:")?.parse().ok()
    }

    fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected_id(cx) {
            self.start_rename(id, window, cx);
        }
    }

    fn duplicate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected_id(cx) {
            self.duplicate_snippet(id, window, cx);
        }
    }

    /// The name field of the snippet `id`, while it is being renamed.
    pub(crate) fn rename_input(&self, id: i64) -> Option<Entity<InputState>> {
        self.renaming
            .as_ref()
            .filter(|renaming| renaming.id == id)
            .map(|renaming| renaming.input.clone())
    }

    /// Start renaming once the menu that asked for it has closed, since
    /// closing it gives focus back to where it was.
    pub(crate) fn rename_after_menu(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.defer_in(window, move |this, window, cx| {
            this.start_rename(id, window, cx)
        });
    }

    /// Show a name field in place of the snippet's label. Enter or leaving the
    /// field renames it, and Escape leaves it as it was.
    pub fn start_rename(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .snippets
            .iter()
            .find(|snippet| snippet.id == Some(id))
            .map(|snippet| snippet.name.clone())
        else {
            return;
        };
        let item_id = format!("snippet:{id}");
        self.tree_state
            .update(cx, |tree, cx| tree.select_item(&item_id, cx));

        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.commit_rename(true, window, cx),
                    InputEvent::Blur => this.commit_rename(false, window, cx),
                    InputEvent::Change | InputEvent::Focus => {}
                },
            );
        // Escape belongs to the field itself, so it is caught before the
        // field handles it.
        let panel = cx.entity().downgrade();
        let escape = cx.intercept_keystrokes(move |event, window, cx| {
            if event.keystroke.key != "escape" || event.keystroke.modifiers.modified() {
                return;
            }
            panel
                .update(cx, |panel, cx| {
                    let focused = panel
                        .renaming
                        .as_ref()
                        .is_some_and(|renaming| renaming.input.focus_handle(cx).is_focused(window));
                    if focused {
                        cx.stop_propagation();
                        panel.renaming = None;
                        panel.focus_tree(window, cx);
                        cx.notify();
                    }
                })
                .log_err();
        });
        self.renaming = Some(Renaming {
            id,
            input,
            _subscriptions: vec![events, escape],
        });
        cx.notify();
    }

    fn focus_tree(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_state
            .update(cx, |tree, cx| tree.focus(window, cx));
    }

    /// Rename the snippet to what the name field holds. An empty name leaves
    /// it as it was.
    fn commit_rename(&mut self, from_enter: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let name = renaming.input.read(cx).value().trim().to_string();
        let id = renaming.id;
        if from_enter {
            self.focus_tree(window, cx);
        }
        let Some(snippet) = self.snippets.iter_mut().find(|s| s.id == Some(id)) else {
            cx.notify();
            return;
        };
        if name.is_empty() || name == snippet.name {
            cx.notify();
            return;
        }
        // Shown at once, while the database catches up.
        snippet.name = name.clone();
        self.refresh_tree(cx);
        cx.emit(SnippetsPanelEvent::SnippetRenamed {
            id,
            name: name.clone(),
        });
        let app_database = AppDatabase::global(cx).clone();
        self.mutate_then_refresh(
            async move {
                app_database
                    .rename_snippet(id, &name)
                    .await
                    .map_err(anyhow::Error::from)
            },
            cx,
        );
        cx.notify();
    }
}

impl Render for SnippetsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_placeholder = self.loaded && self.snippets.is_empty();

        v_flex()
            .id("snippets-panel")
            .size_full()
            .flex_col()
            .gap_2()
            .on_action(cx.listener(|this, _: &NewSnippet, window, cx| {
                this.create_item(false, None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CreateGroup, window, cx| {
                this.create_item(true, None, window, cx);
            }))
            .on_action(cx.listener(|this, _: &RenameSnippet, window, cx| {
                this.rename_selected(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DuplicateSnippet, window, cx| {
                this.duplicate_selected(window, cx);
            }))
            .child(
                v_flex()
                    .id("snippets-tree-container")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .pb_6()
                    .child(DraggableTree::new(&self.tree_state))
                    // Laid over the tree rather than replacing it: the tree's
                    // context menu is how the first snippet or group is added.
                    .when(show_placeholder, |this| {
                        this.child(
                            v_flex()
                                .absolute()
                                .inset_0()
                                .items_center()
                                .justify_center()
                                .gap_1()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child("No snippets yet")
                                .child("Right-click to add a snippet or group"),
                        )
                    })
                    .overflow_y_scrollbar(),
            )
    }
}

/// Work out where a dropped snippet lands: its new parent, and the order of
/// that parent's children once it has been inserted.
///
/// Returns the new parent and the full ordered list of child ids, so the caller
/// can renumber the whole sibling group instead of guessing a position that
/// might collide with an existing one.
fn plan_drop(
    snippets: &[SnippetData],
    snippet_id: i64,
    placement: DropPlacement,
) -> (Option<i64>, Vec<i64>) {
    let (new_parent_id, insert_index) = match placement {
        DropPlacement::Root => (None, usize::MAX),
        DropPlacement::IntoGroup(group_id) => (Some(group_id), usize::MAX),
        DropPlacement::Beside { sibling_id, after } => {
            let sibling_parent = snippets
                .iter()
                .find(|snippet| snippet.id == Some(sibling_id))
                .and_then(|snippet| snippet.parent_id);

            let index = ordered_children(snippets, sibling_parent, Some(snippet_id))
                .iter()
                .position(|id| *id == sibling_id)
                .map(|index| if after { index + 1 } else { index })
                .unwrap_or(usize::MAX);

            (sibling_parent, index)
        }
    };

    let mut siblings = ordered_children(snippets, new_parent_id, Some(snippet_id));
    let insert_index = insert_index.min(siblings.len());
    siblings.insert(insert_index, snippet_id);

    (new_parent_id, siblings)
}

/// The ids of `parent_id`'s children in display order, optionally skipping one
/// (the snippet being moved, which is about to be re-inserted).
fn ordered_children(
    snippets: &[SnippetData],
    parent_id: Option<i64>,
    exclude: Option<i64>,
) -> Vec<i64> {
    let mut children: Vec<&SnippetData> = snippets
        .iter()
        .filter(|snippet| snippet.parent_id == parent_id && snippet.id != exclude)
        .collect();
    children.sort_by(|a, b| {
        a.position
            .cmp(&b.position)
            .then_with(|| a.name.cmp(&b.name))
    });
    children
        .into_iter()
        .filter_map(|snippet| snippet.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two root snippets (1, 2), a root group (3) holding snippets 4 and 5.
    fn snippets() -> Vec<SnippetData> {
        let snippet =
            |id: i64, parent_id: Option<i64>, position: i32, is_group: bool| SnippetData {
                id: Some(id),
                name: format!("snippet-{id}"),
                content: String::new(),
                kind: EditorKind::default(),
                parent_id,
                is_group,
                position,
                trigger: None,
            };

        vec![
            snippet(1, None, 0, false),
            snippet(2, None, 1, false),
            snippet(3, None, 2, true),
            snippet(4, Some(3), 0, false),
            snippet(5, Some(3), 1, false),
        ]
    }

    #[test]
    fn dropping_after_a_sibling_reorders_within_the_parent() {
        let (parent, order) = plan_drop(
            &snippets(),
            1,
            DropPlacement::Beside {
                sibling_id: 2,
                after: true,
            },
        );

        assert_eq!(parent, None);
        assert_eq!(order, vec![2, 1, 3]);
    }

    #[test]
    fn dropping_before_a_sibling_reorders_within_the_parent() {
        let (parent, order) = plan_drop(
            &snippets(),
            3,
            DropPlacement::Beside {
                sibling_id: 1,
                after: false,
            },
        );

        assert_eq!(parent, None);
        assert_eq!(order, vec![3, 1, 2]);
    }

    #[test]
    fn dropping_beside_a_group_child_moves_into_that_group() {
        let (parent, order) = plan_drop(
            &snippets(),
            1,
            DropPlacement::Beside {
                sibling_id: 5,
                after: false,
            },
        );

        assert_eq!(parent, Some(3));
        assert_eq!(order, vec![4, 1, 5]);
    }

    #[test]
    fn dropping_into_a_group_appends() {
        let (parent, order) = plan_drop(&snippets(), 1, DropPlacement::IntoGroup(3));

        assert_eq!(parent, Some(3));
        assert_eq!(order, vec![4, 5, 1]);
    }

    #[test]
    fn dropping_on_the_background_appends_to_the_root() {
        let (parent, order) = plan_drop(&snippets(), 4, DropPlacement::Root);

        assert_eq!(parent, None);
        assert_eq!(order, vec![1, 2, 3, 4]);
    }

    mod naming {
        use crate::test_harness::FullAppHarness;
        use app_database::AppDatabase;
        use gpui::{TestAppContext, VisualTestContext};

        /// Wait for the database, which runs on tokio, until `done` holds.
        async fn wait_until(
            cx: &mut VisualTestContext,
            mut done: impl FnMut(&mut VisualTestContext) -> bool,
        ) {
            for _ in 0..200 {
                cx.run_until_parked();
                if done(cx) {
                    return;
                }
                cx.executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
            }
            panic!("timed out");
        }

        /// A new snippet is stored at once and starts renaming in the panel,
        /// its editor opens, and a rename reaches the database and the editor.
        /// A duplicate is stored next and starts renaming too.
        #[gpui::test]
        async fn new_snippets_are_named_in_the_panel(cx: &mut TestAppContext) {
            let harness = FullAppHarness::new(cx);
            let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
            let editor_panel = harness.editor_panel(&cx);
            let panel = harness
                .app
                .read_with(&cx, |app, _| app.snippets_panel().clone());

            panel.update_in(&mut cx, |panel, window, cx| {
                panel.create_item(false, None, window, cx)
            });
            wait_until(&mut cx, |cx| {
                let renaming = panel.read_with(cx, |panel, _| panel.renaming.is_some());
                let open = editor_panel.read_with(cx, |p, _| p.active_snippet_editor().is_some());
                renaming && open
            })
            .await;
            let id = panel.read_with(&cx, |panel, _| {
                let id = panel.renaming.as_ref().unwrap().id;
                let snippet = panel.snippets.iter().find(|s| s.id == Some(id)).unwrap();
                assert_eq!(snippet.name, "New Snippet");
                id
            });
            let editor = editor_panel
                .read_with(&cx, |panel, _| panel.active_snippet_editor())
                .unwrap();
            assert_eq!(editor.read_with(&cx, |e, _| e.snippet_id), Some(id));

            panel.update_in(&mut cx, |panel, window, cx| {
                let input = panel.renaming.as_ref().unwrap().input.clone();
                input.update(cx, |input, cx| input.set_value("Row counts", window, cx));
                panel.commit_rename(true, window, cx);
            });
            assert_eq!(editor.read_with(&cx, |e, _| e.name.clone()), "Row counts");
            let database = cx.update(|_, cx| AppDatabase::global(cx).clone());
            let runtime = cx.update(|_, cx| gpui_tokio::Tokio::handle(cx));
            wait_until(&mut cx, |_| {
                runtime
                    .block_on(database.get_snippet_by_id(id))
                    .unwrap()
                    .is_some_and(|stored| stored.name == "Row counts")
            })
            .await;

            panel.update_in(&mut cx, |panel, window, cx| {
                panel.duplicate_snippet(id, window, cx)
            });
            wait_until(&mut cx, |cx| {
                panel.read_with(cx, |panel, _| {
                    panel.snippets.iter().any(|s| s.name == "Row counts copy")
                })
            })
            .await;
            panel.read_with(&cx, |panel, _| {
                let copy = panel
                    .snippets
                    .iter()
                    .find(|s| s.name == "Row counts copy")
                    .unwrap();
                assert_eq!(panel.renaming.as_ref().map(|r| r.id), copy.id);
                assert!(copy.trigger.is_none());
            });
        }
    }
}
