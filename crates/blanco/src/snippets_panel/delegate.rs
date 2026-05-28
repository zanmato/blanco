use crate::app::NewSnippet;
use crate::snippets_panel::{CreateGroup, SnippetsPanel};
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

use blanco_ui::draggable_tree::{DraggableTreeDelegate, DraggedTreeItem, TreeEntry};

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
        _dragged_item: &DraggedTreeItem,
        target_entry: &TreeEntry,
        cx: &App,
    ) -> bool {
        let item_id = target_entry.item().id.as_ref();
        if let Some(metadata) = self.get_snippet_metadata(item_id, cx) {
            metadata.is_group
        } else {
            false
        }
    }

    fn can_drop_on_root(&self, _dragged_item: &DraggedTreeItem) -> bool {
        true
    }

    fn on_drop(
        &mut self,
        dragged_item: &DraggedTreeItem,
        target_entry_id: Option<&str>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let snippet_id =
            if let Some(id_str) = dragged_item.item_id.as_ref().strip_prefix("snippet:") {
                if let Ok(id) = id_str.parse::<i64>() {
                    id
                } else {
                    return;
                }
            } else {
                return;
            };

        let parent_id = if let Some(target_id) = target_entry_id {
            if let Some(id_str) = target_id.strip_prefix("snippet:") {
                id_str.parse::<i64>().ok()
            } else {
                None
            }
        } else {
            None
        };

        self.parent.update(cx, |this, cx| {
            this.handle_drop(snippet_id, parent_id, window, cx);
        });
    }
}
