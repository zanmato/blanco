use gpui::{
    App, Entity, InteractiveElement, ParentElement, SharedString, StatefulInteractiveElement as _,
    Styled, Window, div, prelude::FluentBuilder, px, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, h_flex,
    list::ListItem,
    menu::{PopupMenu, PopupMenuItem},
    spinner::Spinner,
    tooltip::Tooltip,
};

use crate::app::OpenSchemaGraph;
use crate::app::OpenTableStructure;
use crate::connections::{
    ConnectionsPanel, CreateNewQueryTabParams, TreeItemIcon, TreeItemKind, TreeItemMetadata,
};

use blanco_ui::{
    IconName, SizeIndicator,
    tree::{TreeDelegate, TreeEntry},
};

/// Delegate for handling the connections tree rendering and data loading
pub struct ConnectionsTreeDelegate {
    parent: Entity<ConnectionsPanel>,
}

impl ConnectionsTreeDelegate {
    pub fn new(parent: &Entity<ConnectionsPanel>) -> Self {
        Self {
            parent: parent.clone(),
        }
    }
}

/// Format a byte count as a short human-readable string using IEC base-1024
/// units (KB / MB / GB / TB). Matches the unit naming most DB tools display.
fn format_size_short(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{}{}", bytes, UNITS[0]);
    }
    let decimals = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    let mut formatted = format!("{:.*}", decimals, value);
    if formatted.contains('.') {
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
    }
    format!("{}{}", formatted, UNITS[unit])
}

/// Format a byte count with thousands separators for the dot tooltip.
fn format_bytes_exact(bytes: u64) -> String {
    let s = bytes.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Total width of the bordered size indicator rectangle, including border.
const SIZE_BAR_WIDTH_PX: f32 = 48.0;
const SIZE_BAR_HEIGHT_PX: f32 = 16.0;

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

        let metadata = &item.metadata;
        let mut tree_item_icon = metadata.icon.clone();

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

        let size_indicator = metadata.relative_size.map(|relative| {
            let size_text = metadata
                .size_bytes
                .map(format_size_short)
                .unwrap_or_default();
            let tooltip_text: SharedString = metadata
                .size_bytes
                .map(|b| format!("{} bytes", format_bytes_exact(b)))
                .unwrap_or_else(|| "unknown size".to_string())
                .into();
            let theme = cx.theme();
            let bar = SizeIndicator::new(("tree-item-size", ix), size_text, relative)
                .size(px(SIZE_BAR_WIDTH_PX), px(SIZE_BAR_HEIGHT_PX))
                .fill_color(theme.blue.opacity(0.35))
                .border_color(theme.border)
                .text_color(theme.muted_foreground)
                .fill_text_color(theme.foreground)
                .font(theme.mono_font_family.clone(), px(10.))
                .corner_radius(px(3.));
            h_flex()
                .id(("tree-item-size-wrap", ix))
                .items_center()
                .flex_shrink_0()
                .tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
                .child(bar)
        });

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
                    .when_some(size_indicator, |this, indicator| this.child(indicator))
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
                menu.item(PopupMenuItem::new("Refresh").on_click(window.listener_for(
                    &self.parent,
                    move |this, _event, _window, cx| {
                        this.refresh_connection(connection_id, cx);
                    },
                )))
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
                let connection_name = metadata.connection_name.clone();
                let db_type = metadata.db_type;
                let environment_type = metadata.environment_type;

                let mut menu = if let Some(action) = metadata.create_new_query_tab_action() {
                    menu.item(
                        PopupMenuItem::new("New Query").on_click(window.listener_for(
                            &self.parent,
                            move |_this, _event, window, cx| {
                                window.dispatch_action(Box::new(action.clone()), cx);
                            },
                        )),
                    )
                } else {
                    menu
                };

                let connection_name_for_graph = connection_name;
                let database_name_for_graph = database_name.clone();
                menu =
                    menu.item(PopupMenuItem::new("View Schema Graph").on_click(
                        window.listener_for(&self.parent, move |_this, _event, window, cx| {
                            window.dispatch_action(
                                Box::new(OpenSchemaGraph {
                                    connection_id,
                                    connection_name: connection_name_for_graph.clone(),
                                    db_type,
                                    database_name: database_name_for_graph.clone(),
                                    schema_name: None,
                                    environment_type,
                                }),
                                cx,
                            );
                        }),
                    ))
                    .item(
                        PopupMenuItem::new("Disconnect").on_click(window.listener_for(
                            &self.parent,
                            move |this, _event, _window, cx| {
                                this.disconnect_database(connection_id, database_name.clone(), cx);
                            },
                        )),
                    );

                menu
            }
            TreeItemKind::Procedure | TreeItemKind::Function | TreeItemKind::Trigger => {
                // Routine items have no contextual operations yet; double-
                // click handling (DDL tab) is wired separately in the panel.
                menu
            }
            TreeItemKind::Category => {
                // Grouping folders have no contextual operations.
                menu
            }
            TreeItemKind::Schema
            | TreeItemKind::Table
            | TreeItemKind::View
            | TreeItemKind::MaterializedView => {
                if let Some(action) = metadata.create_new_query_tab_action() {
                    let connection_id = metadata.connection_id;
                    let connection_name = metadata.connection_name.clone();
                    let database_name = metadata.database_name.clone().unwrap_or_default();
                    let schema_name = metadata.schema_name.clone();
                    let table_name = metadata.table_name.clone();
                    let db_type = metadata.db_type;
                    let environment_type = metadata.environment_type;

                    let connection_name_for_structure = connection_name.clone();
                    let database_name_for_structure = database_name.clone();
                    let schema_name_for_structure = schema_name.clone();
                    let table_name_for_structure = table_name.clone().unwrap_or_default();

                    let connection_name_for_import = connection_name.clone();
                    let database_name_for_import = database_name.clone();
                    let schema_name_for_import = schema_name.clone();
                    let table_name_for_import = table_name.clone();

                    let mut menu = menu
                        .item(
                            PopupMenuItem::new("New Query").on_click(window.listener_for(
                                &self.parent,
                                move |_this, _event, window, cx| {
                                    window.dispatch_action(Box::new(action.clone()), cx);
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
                                        db_type,
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        );

                    if matches!(metadata.kind, TreeItemKind::Table) {
                        menu = menu.item(PopupMenuItem::new("Import Data").on_click(
                            window.listener_for(&self.parent, move |this, _event, window, cx| {
                                this.import_table_data(
                                    connection_id,
                                    connection_name_for_import.clone(),
                                    database_name_for_import.clone(),
                                    schema_name_for_import.clone(),
                                    table_name_for_import.clone(),
                                    db_type,
                                    window,
                                    cx,
                                );
                            }),
                        ));
                    }

                    if matches!(
                        metadata.kind,
                        TreeItemKind::Table | TreeItemKind::View | TreeItemKind::MaterializedView
                    ) {
                        menu =
                            menu.separator()
                                .item(PopupMenuItem::new("Open Structure").on_click(
                                    window.listener_for(
                                        &self.parent,
                                        move |_this, _event, window, cx| {
                                            window.dispatch_action(
                                                Box::new(OpenTableStructure {
                                                    connection_id,
                                                    connection_name: connection_name_for_structure
                                                        .clone(),
                                                    db_type,
                                                    database_name: database_name_for_structure
                                                        .clone(),
                                                    schema_name: schema_name_for_structure.clone(),
                                                    table_name: table_name_for_structure.clone(),
                                                    environment_type,
                                                }),
                                                cx,
                                            );
                                        },
                                    ),
                                ));
                    }

                    if matches!(metadata.kind, TreeItemKind::Schema) {
                        let connection_name_for_graph = metadata.connection_name.clone();
                        let database_name_for_graph =
                            metadata.database_name.clone().unwrap_or_default();
                        let schema_name_for_graph = metadata.schema_name.clone();
                        let environment_type_for_graph = metadata.environment_type;

                        menu = menu.separator().item(
                            PopupMenuItem::new("View Schema Graph").on_click(window.listener_for(
                                &self.parent,
                                move |_this, _event, window, cx| {
                                    window.dispatch_action(
                                        Box::new(OpenSchemaGraph {
                                            connection_id,
                                            connection_name: connection_name_for_graph.clone(),
                                            db_type,
                                            database_name: database_name_for_graph.clone(),
                                            schema_name: schema_name_for_graph.clone(),
                                            environment_type: environment_type_for_graph,
                                        }),
                                        cx,
                                    );
                                },
                            )),
                        );
                    }

                    menu
                } else {
                    menu
                }
            }
        }
    }
}
