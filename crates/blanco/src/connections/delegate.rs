use gpui::{
    App, Entity, InteractiveElement, ParentElement, StatefulInteractiveElement as _, Styled,
    Window, div, prelude::FluentBuilder, px, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, h_flex,
    list::ListItem,
    menu::{PopupMenu, PopupMenuItem},
    spinner::Spinner,
    tooltip::Tooltip,
};

use crate::app_events::AppEvent;
use crate::connections::{CreateNewQueryTabParams, TreeItemIcon, TreeItemKind, TreeItemMetadata};

use blanco_ui::{
    IconName,
    tree::{TreeDelegate, TreeEntry},
};

/// Delegate for handling the connections tree rendering and data loading
pub struct ConnectionsTreeDelegate {
    parent: Entity<crate::connections::ConnectionsPanel>,
}

impl ConnectionsTreeDelegate {
    pub fn new(parent: &Entity<crate::connections::ConnectionsPanel>) -> Self {
        Self {
            parent: parent.clone(),
        }
    }
}

impl TreeDelegate for ConnectionsTreeDelegate {
    type Metadata = TreeItemMetadata;

    fn render_item(
        &self,
        ix: usize,
        entry: &TreeEntry<TreeItemMetadata>,
        selected: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> ListItem {
        let item = entry.item();
        let depth = entry.depth();

        // Access metadata directly - no HashMap lookups!
        let metadata = &item.metadata;
        let mut tree_item_icon = metadata.icon.clone();

        // The icon is now updated directly in the tree metadata, so we just use it as-is

        // Add environment label for connections
        let environment_label = if metadata.kind == TreeItemKind::Connection {
            // Get panel data for environment labels
            let panel = self.parent.read(cx);
            if let Some(connection) = panel
                .connections
                .iter()
                .find(|c| c.id == Some(metadata.connection_id))
            {
                let env_type = connection.environment_type;
                Some(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .justify_center()
                        .text_size(rems(0.55))
                        .font_family(cx.theme().mono_font_family.clone())
                        .w_9()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(env_type.get_color(cx))
                        .text_color(env_type.get_color(cx))
                        .child(env_type.display_name()),
                )
            } else {
                None
            }
        } else {
            None
        };

        // Update schema icons based on expansion state
        if metadata.kind == TreeItemKind::Schema && entry.is_expanded() {
            tree_item_icon = TreeItemIcon {
                icon: IconName::FolderOpen,
                color: tree_item_icon.color,
            };
        }

        let item_id: gpui::SharedString = item.id.clone();
        let tooltip_label = item.label.clone();

        ListItem::new(ix)
            .selected(selected)
            .w_full()
            .px_3()
            .pl(px(12.) * depth as f32 + px(12.)) // Indent based on depth
            .child(
                h_flex()
                    .id(("tree-item", ix))
                    .gap_2()
                    .items_center()
                    .tooltip(move |window, cx| {
                        Tooltip::new(tooltip_label.clone()).build(window, cx)
                    })
                    .child(Icon::new(tree_item_icon.icon).text_color(tree_item_icon.color))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_sm()
                            .text_ellipsis()
                            .child(item.label.clone()),
                    )
                    .when_some(environment_label, |this, label| this.child(label))
                    .when(
                        (entry.is_folder() || metadata.kind == TreeItemKind::Connection)
                            && !metadata.loading,
                        |this| {
                            this.child(if entry.is_expanded() {
                                IconName::ChevronDown.view(cx)
                            } else {
                                IconName::ChevronRight.view(cx)
                            })
                        },
                    )
                    .when(
                        (entry.is_folder()
                            || metadata.kind == TreeItemKind::Connection
                            || metadata.kind == TreeItemKind::Database)
                            && metadata.loading,
                        |this| {
                            this.child(
                                Spinner::new()
                                    .icon(IconName::LoaderCircle)
                                    .color(cx.theme().muted_foreground),
                            )
                        },
                    ),
            )
            .when(!metadata.loading, |this| {
                // Only allow clicks when not loading
                this.on_click(
                    window.listener_for(&self.parent, move |this, _event, window, cx| {
                        this.handle_tree_item_click(&item_id, window, cx);
                    }),
                )
            })
    }

    fn context_menu(
        &self,
        _ix: usize,
        entry: &TreeEntry<TreeItemMetadata>,
        menu: PopupMenu,
        window: &mut Window,
        _cx: &mut App,
    ) -> PopupMenu {
        let metadata = &entry.item().metadata;

        match metadata.kind {
            TreeItemKind::Connection => {
                let connection_id = metadata.connection_id;
                let connection_name = metadata.connection_name.clone();
                menu.item(PopupMenuItem::new("Refresh"))
                    .item(PopupMenuItem::new("Edit").on_click(window.listener_for(
                        &self.parent,
                        move |this, _event, _window, cx| {
                            this.edit_connection(connection_id, cx);
                        },
                    )))
                    .item(
                        PopupMenuItem::new("Disconnect").on_click(window.listener_for(
                            &self.parent,
                            move |this, _event, _window, cx| {
                                this.disconnect_connection(connection_id, cx);
                            },
                        )),
                    )
                    .separator()
                    .item(PopupMenuItem::new("Remove").on_click(window.listener_for(
                        &self.parent,
                        move |this, _event, window, cx| {
                            this.confirm_remove_connection(
                                connection_id,
                                connection_name.clone(),
                                window,
                                cx,
                            );
                        },
                    )))
            }
            TreeItemKind::Database => {
                let connection_id = metadata.connection_id;
                let database_name = metadata.database_name.clone().unwrap_or_default();

                // Add "New Query" if applicable
                let mut menu = if let Some(event) = metadata.create_new_query_tab_event() {
                    menu.item(
                        PopupMenuItem::new("New Query").on_click(window.listener_for(
                            &self.parent,
                            move |_this, _event, _window, cx| {
                                cx.emit(event.clone());
                            },
                        )),
                    )
                } else {
                    menu
                };

                // Add "Disconnect" option
                menu = menu.item(
                    PopupMenuItem::new("Disconnect").on_click(window.listener_for(
                        &self.parent,
                        move |this, _event, _window, cx| {
                            this.disconnect_database(connection_id, database_name.clone(), cx);
                        },
                    )),
                );

                menu
            }
            TreeItemKind::Schema
            | TreeItemKind::Table
            | TreeItemKind::View
            | TreeItemKind::MaterializedView => {
                // Use the trait to create the query tab event
                if let Some(event) = metadata.create_new_query_tab_event() {
                    let connection_id = metadata.connection_id;
                    let connection_name = metadata.connection_name.clone();
                    let database_name = metadata.database_name.clone().unwrap_or_default();
                    let schema_name = metadata.schema_name.clone();
                    let table_name = metadata.table_name.clone();
                    let db_type = metadata.db_type;
                    let environment_type = metadata.environment_type;

                    // Clone values for the "Open Structure" closure
                    let connection_name_for_structure = connection_name.clone();
                    let database_name_for_structure = database_name.clone();
                    let schema_name_for_structure = schema_name.clone();
                    let table_name_for_structure = table_name.clone().unwrap_or_default();

                    let mut menu = menu
                        .item(
                            PopupMenuItem::new("New Query").on_click(window.listener_for(
                                &self.parent,
                                move |_this, _event, _window, cx| {
                                    cx.emit(event.clone());
                                },
                            )),
                        )
                        .item(
                            PopupMenuItem::new("Export Data").on_click(window.listener_for(
                                &self.parent,
                                move |this, _event, window, cx| {
                                    this.export_table_data(
                                        connection_id,
                                        connection_name.clone(),
                                        database_name.clone(),
                                        schema_name.clone(),
                                        table_name.clone(),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        );

                    // Add "Open Structure" option for tables, views, and materialized views
                    if matches!(
                        metadata.kind,
                        TreeItemKind::Table | TreeItemKind::View | TreeItemKind::MaterializedView
                    ) {
                        menu = menu
                            .separator()
                            .item(PopupMenuItem::new("Open Structure").on_click(
                                window.listener_for(&self.parent, move |_this, _event, _window, cx| {
                                    cx.emit(AppEvent::OpenTableStructure {
                                        connection_id,
                                        connection_name: connection_name_for_structure.clone(),
                                        db_type,
                                        database_name: database_name_for_structure.clone(),
                                        schema_name: schema_name_for_structure.clone(),
                                        table_name: table_name_for_structure.clone(),
                                        environment_type,
                                    });
                                }),
                            ));
                    }

                    menu
                } else {
                    menu
                }
            }
        }
    }
}
