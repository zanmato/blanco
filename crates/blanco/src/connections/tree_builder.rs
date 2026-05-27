//! Tree construction logic for the connections sidebar.
//!
//! Splits the deeply-nested connection -> database -> schema -> table
//! rendering out of `connections.rs` (which previously held a 250-line method
//! among everything else). All state still lives on `ConnectionsPanel`; this
//! file just hosts an additional inherent impl block.

use blanco_ui::IconName;
use blanco_ui::tree::TreeItem;
use gpui::Context;
use gpui_component::ActiveTheme as _;

use crate::app_database::ConnectionData;

use super::data_loading::DatabaseItemType;
use super::{ConnectionsPanel, TreeItemIcon, TreeItemKind, TreeItemMetadata};

impl ConnectionsPanel {
    /// Build a TreeItem for a connection including its children if loaded.
    pub(super) fn build_connection_tree_item_with_expand(
        &self,
        connection: &ConnectionData,
        should_expand: bool,
        cx: &Context<Self>,
    ) -> TreeItem<TreeItemMetadata> {
        let connection_id = connection.id.unwrap_or(0);

        let db_type = connection.db_type;

        let connection_metadata = TreeItemMetadata {
            connection_id,
            connection_name: connection.display_name(),
            kind: TreeItemKind::Connection,
            db_type,
            database_name: None,
            schema_name: None,
            table_name: None,
            icon: self.get_connection_icon(connection_id, cx),
            environment_type: Some(connection.environment_type),
            loading: false,
            size_bytes: None,
            relative_size: None,
        };

        let base_item = TreeItem::new(
            format!("connection:{}", connection_id),
            connection.display_name(),
            connection_metadata,
        )
        .expanded(should_expand);

        let Some(metadata) = self.database_metadata.get(&connection_id) else {
            return base_item;
        };

        if metadata.supports_schemas {
            // PostgreSQL: connection -> databases -> schemas -> tables
            let database_items: Vec<TreeItem<TreeItemMetadata>> = metadata
                .databases
                .iter()
                .map(|database| {
                    let database_key = format!("database:{}:{}", connection_id, database.name);

                    let database_metadata = TreeItemMetadata {
                        connection_id,
                        connection_name: connection.display_name(),
                        kind: TreeItemKind::Database,
                        db_type,
                        database_name: Some(database.name.clone()),
                        schema_name: None,
                        table_name: None,
                        icon: TreeItemIcon {
                            icon: IconName::Database,
                            color: cx.theme().foreground.into(),
                        },
                        environment_type: Some(connection.environment_type),
                        loading: false,
                        size_bytes: None,
                        relative_size: None,
                    };

                    let schema_items: Vec<TreeItem<TreeItemMetadata>> = database
                        .schemas
                        .iter()
                        .map(|schema| {
                            let schema_key = format!(
                                "schema:{}:{}:{}",
                                connection_id, database.name, schema.name
                            );

                            let schema_metadata = TreeItemMetadata {
                                connection_id,
                                connection_name: connection.display_name(),
                                kind: TreeItemKind::Schema,
                                db_type,
                                database_name: Some(database.name.clone()),
                                schema_name: Some(schema.name.clone()),
                                table_name: None,
                                icon: TreeItemIcon {
                                    icon: IconName::Folder,
                                    color: cx.theme().yellow.into(),
                                },
                                environment_type: Some(connection.environment_type),
                                loading: false,
                                size_bytes: None,
                                relative_size: None,
                            };

                            let table_items: Vec<TreeItem<TreeItemMetadata>> = if schema.is_expanded
                            {
                                schema
                                    .tables
                                    .iter()
                                    .map(|table| {
                                        let table_key = format!(
                                            "table:{}:{}:{}:{}",
                                            connection_id,
                                            database.name,
                                            schema.name,
                                            table.name
                                        );

                                        let (icon, color, kind) =
                                            icon_for_item_type(table.item_type, cx);

                                        let table_metadata = TreeItemMetadata {
                                            connection_id,
                                            connection_name: connection.display_name(),
                                            kind,
                                            db_type,
                                            database_name: Some(database.name.clone()),
                                            schema_name: Some(schema.name.clone()),
                                            table_name: Some(table.name.clone()),
                                            icon: TreeItemIcon { icon, color },
                                            environment_type: Some(connection.environment_type),
                                            loading: false,
                                            size_bytes: table.size_bytes,
                                            relative_size: table.relative_size,
                                        };

                                        TreeItem::new(
                                            table_key,
                                            table.name.clone(),
                                            table_metadata,
                                        )
                                    })
                                    .collect()
                            } else {
                                Vec::new()
                            };

                            TreeItem::new(schema_key, schema.name.clone(), schema_metadata)
                                .expanded(schema.is_expanded && !schema.tables.is_empty())
                                .children(table_items)
                        })
                        .collect();

                    TreeItem::new(database_key, database.name.clone(), database_metadata)
                        .expanded(database.is_expanded && !database.schemas.is_empty())
                        .children(schema_items)
                })
                .collect();
            base_item.children(database_items)
        } else {
            // MySQL/SQLite: connection -> tables (no schema level)
            let table_items: Vec<TreeItem<TreeItemMetadata>> = metadata
                .schemas
                .iter()
                .flat_map(|schema| {
                    schema
                        .tables
                        .iter()
                        .map(|table| {
                            let table_key = format!(
                                "table:{}:{}:{}",
                                connection_id, schema.name, table.name
                            );

                            let (icon, color, kind) = icon_for_item_type(table.item_type, cx);

                            let table_metadata = TreeItemMetadata {
                                connection_id,
                                connection_name: connection.display_name(),
                                kind,
                                db_type,
                                database_name: Some(schema.name.clone()),
                                schema_name: None,
                                table_name: Some(table.name.clone()),
                                icon: TreeItemIcon { icon, color },
                                environment_type: Some(connection.environment_type),
                                loading: false,
                                size_bytes: table.size_bytes,
                                relative_size: table.relative_size,
                            };

                            TreeItem::new(table_key, table.name.clone(), table_metadata)
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            base_item.children(table_items)
        }
    }
}

/// Pick the icon, color, and metadata kind to render for a given object kind.
fn icon_for_item_type(
    item_type: DatabaseItemType,
    cx: &Context<ConnectionsPanel>,
) -> (IconName, gpui::Rgba, TreeItemKind) {
    match item_type {
        DatabaseItemType::Table => (
            IconName::Sheet,
            cx.theme().green.into(),
            TreeItemKind::Table,
        ),
        DatabaseItemType::View => (IconName::Eye, cx.theme().cyan.into(), TreeItemKind::View),
        DatabaseItemType::MaterializedView => (
            IconName::LayoutDashboard,
            cx.theme().magenta.into(),
            TreeItemKind::MaterializedView,
        ),
        DatabaseItemType::Procedure => (
            IconName::SquareTerminal,
            cx.theme().magenta.into(),
            TreeItemKind::Procedure,
        ),
        DatabaseItemType::Function => (
            IconName::Braces,
            cx.theme().cyan.into(),
            TreeItemKind::Function,
        ),
        DatabaseItemType::Trigger => (
            IconName::DatabaseConnected,
            cx.theme().yellow.into(),
            TreeItemKind::Trigger,
        ),
    }
}
