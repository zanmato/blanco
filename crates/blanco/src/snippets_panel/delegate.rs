use crate::app::NewSnippet;
use crate::snippets_panel::{CreateGroup, DropPlacement, SnippetsPanel};
use gpui::ClickEvent;
use gpui::{
    App, Entity, InteractiveElement, ParentElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{ActiveTheme, Icon, h_flex};
use gpui_component::{
    label::Label,
    list::ListItem,
    menu::{PopupMenu, PopupMenuItem},
};

use blanco_ui::draggable_tree::{
    DraggableTreeDelegate, DraggedTreeItem, InsertPosition, TreeEntry,
};

/// Metadata for snippet items in the tree
#[derive(Clone, Debug)]
pub struct SnippetItemMetadata {
    pub id: i64,
    pub name: String,
    pub is_group: bool,
    pub parent_id: Option<i64>,
}

/// Delegate for handling the snippets tree rendering and drag and drop
pub struct SnippetsTreeDelegate {
    parent: Entity<SnippetsPanel>,
}

impl SnippetsTreeDelegate {
    pub fn new(parent: &Entity<SnippetsPanel>) -> Self {
        Self {
            parent: parent.clone(),
        }
    }

    fn get_snippet_metadata(&self, item_id: &str, cx: &App) -> Option<SnippetItemMetadata> {
        if let Some(id_str) = item_id.strip_prefix("snippet:")
            && let Ok(id) = id_str.parse::<i64>()
        {
            let panel = self.parent.read(cx);
            if let Some(snippet) = panel.snippets.iter().find(|s| s.id == Some(id)) {
                return Some(SnippetItemMetadata {
                    id: snippet.id.unwrap_or(0),
                    name: snippet.name.clone(),
                    is_group: snippet.is_group,
                    parent_id: snippet.parent_id,
                });
            }
        }
        None
    }

    /// Whether `candidate_id` sits somewhere under `ancestor_id`.
    fn is_descendant_of(&self, candidate_id: i64, ancestor_id: i64, cx: &App) -> bool {
        let snippets = &self.parent.read(cx).snippets;
        let mut current = Some(candidate_id);

        // A malformed parent chain (a cycle already in the data) would other-
        // wise spin here, so bound the walk by the number of snippets.
        for _ in 0..snippets.len() {
            let Some(id) = current else {
                return false;
            };
            if id == ancestor_id {
                return true;
            }
            current = snippets
                .iter()
                .find(|snippet| snippet.id == Some(id))
                .and_then(|snippet| snippet.parent_id);
        }

        false
    }
}

impl DraggableTreeDelegate for SnippetsTreeDelegate {
    fn render_item(
        &self,
        _ix: usize,
        entry: &TreeEntry,
        selected: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> ListItem {
        let item = entry.item();
        let item_id = item.id.as_ref();
        let depth = entry.depth();

        let metadata = self.get_snippet_metadata(item_id, cx);
        let (icon_name, is_group, has_parent) = if let Some(m) = metadata {
            (
                if m.is_group {
                    if entry.is_expanded() {
                        gpui_component::IconName::FolderOpen
                    } else {
                        gpui_component::IconName::Folder
                    }
                } else {
                    gpui_component::IconName::File
                },
                m.is_group,
                m.parent_id.is_some(),
            )
        } else {
            (gpui_component::IconName::File, false, false)
        };

        let item_id_for_click = item.id.to_string();
        let icon_color = if is_group {
            cx.theme().blue
        } else {
            cx.theme().green
        };

        ListItem::new(_ix)
            .selected(selected)
            .w_full()
            .px_3()
            .pl(px(12.) * depth as f32 + px(12.)) // Indent based on depth
            .child(
                h_flex()
                    .id(("snippet-item", _ix))
                    .gap_2()
                    .items_center()
                    .relative()
                    .when(has_parent, |this| {
                        this.child(
                            div()
                                .absolute()
                                .border_l_1()
                                .border_color(cx.theme().border)
                                .w(px(10.))
                                .h(px(28.))
                                .left(px(-6.)) // Half of the indent
                                .top(px(-3.)),
                        )
                    })
                    .child(Icon::new(icon_name).text_color(icon_color))
                    .child(Label::new(item.label.clone()).text_sm())
                    .child(div().flex_1()),
            )
            .on_click(window.listener_for(
                &self.parent,
                move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() == 2 {
                        this.handle_snippet_double_click(&item_id_for_click, window, cx);
                    }
                },
            ))
    }

    fn context_menu(
        &self,
        _ix: Option<usize>,
        entry: Option<&TreeEntry>,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut App,
    ) -> PopupMenu {
        let mut context_menu = menu
            .menu("New Snippet", Box::new(NewSnippet))
            .menu("New Group", Box::new(CreateGroup));

        let Some(entry) = entry else {
            return context_menu;
        };

        let item_id = entry.item().id.as_ref();
        let metadata = self.get_snippet_metadata(item_id, cx);

        context_menu = context_menu.separator();
        if let Some(metadata) = metadata {
            if !metadata.is_group {
                context_menu
                    .item(
                        PopupMenuItem::new("Copy Snippet").on_click(window.listener_for(
                            &self.parent,
                            move |this, _event, _window, cx| {
                                this.copy_snippet_to_clipboard(metadata.id, cx);
                            },
                        )),
                    )
                    .item(PopupMenuItem::new("Delete").on_click(window.listener_for(
                        &self.parent,
                        move |this, _event, window, cx| {
                            this.delete_snippet(metadata.id, metadata.name.clone(), window, cx);
                        },
                    )))
            } else {
                context_menu.item(
                    PopupMenuItem::new("Delete Group").on_click(window.listener_for(
                        &self.parent,
                        move |this, _event, window, cx| {
                            this.delete_snippet(metadata.id, metadata.name.clone(), window, cx);
                        },
                    )),
                )
            }
        } else {
            context_menu
        }
    }

    fn can_drag(&self, _item_id: &str, _entry: &TreeEntry, _cx: &App) -> bool {
        true
    }

    fn create_drag_data(
        &self,
        _item_id: &str,
        entry: &TreeEntry,
        _cx: &App,
    ) -> Option<DraggedTreeItem> {
        Some(DraggedTreeItem {
            item_id: entry.item().id.clone(),
            label: entry.item().label.clone(),
            collection_path: "snippets".into(),
            icon: None,
        })
    }

    fn can_drop_on(
        &self,
        dragged_item: &DraggedTreeItem,
        target_entry: &TreeEntry,
        position: InsertPosition,
        cx: &App,
    ) -> bool {
        let Some(dragged_id) = snippet_id(dragged_item.item_id.as_ref()) else {
            return false;
        };
        let Some(target) = self.get_snippet_metadata(target_entry.item().id.as_ref(), cx) else {
            return false;
        };

        if target.id == dragged_id {
            return false;
        }
        // Moving a group next to or into one of its own descendants would
        // detach that subtree from the root.
        if self.is_descendant_of(target.id, dragged_id, cx) {
            return false;
        }

        match position {
            InsertPosition::Inside => target.is_group,
            InsertPosition::Before | InsertPosition::After => true,
        }
    }

    fn can_drop_on_root(&self, _dragged_item: &DraggedTreeItem) -> bool {
        true
    }

    fn on_drop(
        &mut self,
        dragged_item: &DraggedTreeItem,
        target_entry_id: Option<&str>,
        position: InsertPosition,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(dragged_id) = snippet_id(dragged_item.item_id.as_ref()) else {
            return;
        };

        let placement = match target_entry_id.and_then(snippet_id) {
            None => DropPlacement::Root,
            Some(target_id) => match position {
                InsertPosition::Inside => DropPlacement::IntoGroup(target_id),
                InsertPosition::Before => DropPlacement::Beside {
                    sibling_id: target_id,
                    after: false,
                },
                InsertPosition::After => DropPlacement::Beside {
                    sibling_id: target_id,
                    after: true,
                },
            },
        };

        self.parent.update(cx, |this, cx| {
            this.handle_drop(dragged_id, placement, window, cx);
        });
    }
}

/// Parse the `snippet:<id>` item id used for tree items.
fn snippet_id(item_id: &str) -> Option<i64> {
    item_id.strip_prefix("snippet:")?.parse().ok()
}
