mod delegate;

pub use delegate::{SnippetItemMetadata, SnippetsTreeDelegate};

use std::future::Future;

use crate::app::{NewSnippet, OpenSnippetEditor};
use crate::result_ext::ResultExt;
use app_database::{AppDatabase, EditorKind, SnippetData};
use blanco_ui::draggable_tree::{DraggableTree, DraggableTreeState, TreeItem};
use gpui::{
    AppContext, ClipboardItem, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Styled, Task, Window, actions,
    prelude::FluentBuilder,
};
use gpui_component::input::Input;
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, h_flex, input::InputState, v_flex};

actions!(snippets, [CreateGroup, RefreshSnippets]);

#[derive(Clone, Debug)]
pub enum SnippetsPanelEvent {
    SnippetDeleted,
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
    tree_state: Entity<DraggableTreeState<SnippetsTreeDelegate>>,
    creating_group: bool,
    group_name_input: Option<Entity<InputState>>,
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
            tree_state,
            creating_group: false,
            group_name_input: None,
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
            this.update(cx, |this, cx| {
                this.snippets = snippets;
                this.refresh_tree(cx);
                cx.notify();
            })
            .log_err();
        }));
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

    pub fn create_new_group(&mut self, cx: &mut Context<Self>) {
        let name = if let Some(input) = &self.group_name_input {
            input.read(cx).value().to_string()
        } else {
            String::new()
        };

        if name.trim().is_empty() {
            self.creating_group = false;
            self.group_name_input = None;
            cx.notify();
            return;
        }

        let app_database = AppDatabase::global(cx).clone();
        let new_group = SnippetData {
            id: None,
            name: name.clone(),
            content: String::new(),
            kind: EditorKind::default(),
            parent_id: None,
            is_group: true,
            position: 0,
        };

        self.creating_group = false;
        self.group_name_input = None;
        self.mutate_then_refresh(
            async move {
                let id = app_database.save_snippet(&new_group).await?;
                tracing::info!("Created new group '{}' with id {}", name, id);
                Ok(())
            },
            cx,
        );
        cx.notify();
    }

    pub fn start_creating_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.creating_group = true;
        self.group_name_input =
            Some(cx.new(|cx| InputState::new(window, cx).placeholder("Group name...")));
        cx.notify();
    }

    fn handle_group_key_event(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "enter" => {
                self.create_new_group(cx);
            }
            "escape" => {
                self.creating_group = false;
                self.group_name_input = None;
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for SnippetsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let group_input = self.group_name_input.clone();
        let creating_group = self.creating_group;

        v_flex()
            .id("snippets-panel")
            .size_full()
            .flex_col()
            .gap_2()
            .on_action(cx.listener(|_this, _: &NewSnippet, window, cx| {
                window.dispatch_action(Box::new(OpenSnippetEditor { snippet_id: None }), cx);
            }))
            .on_action(cx.listener(|this, _: &CreateGroup, window, cx| {
                this.start_creating_group(window, cx);
            }))
            .child(
                v_flex()
                    .id("snippets-tree-container")
                    .flex_1()
                    .min_h_0()
                    .pb_6()
                    .when_some(group_input, |this, input| {
                        this.child(
                            h_flex()
                                .px_3()
                                .py_1()
                                .gap_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().background)
                                .child(Input::new(&input))
                                .when(creating_group, |div| {
                                    div.on_key_down(cx.listener(Self::handle_group_key_event))
                                }),
                        )
                    })
                    .child(DraggableTree::new(&self.tree_state))
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
}
