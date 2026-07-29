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

use super::data_loading::{DatabaseItemType, DatabaseTable};
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
            icon: self.get_connection_icon(connection_id, db_type, cx),
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
                        // Each database has its own connection in the pool, so
                        // the color tracks that database specifically. Coloring
                        // by the parent connection's state (as this once did)
                        // lit up every database as soon as any one of them was
                        // opened.
                        icon: TreeItemIcon {
                            icon: IconName::Database,
                            color: if self.is_database_connected(connection_id, &database.name) {
                                cx.theme().primary.into()
                            } else {
                                cx.theme().foreground.into()
                            },
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

                            let category_items: Vec<TreeItem<TreeItemMetadata>> = if schema
                                .is_expanded
                            {
                                let children = schema
                                    .tables
                                    .iter()
                                    .map(|table| {
                                        let table_key = format!(
                                            "table:{}:{}:{}:{}",
                                            connection_id, database.name, schema.name, table.name
                                        );
                                        build_object_item(
                                            connection,
                                            connection_id,
                                            db_type,
                                            Some(database.name.clone()),
                                            Some(schema.name.clone()),
                                            table_key,
                                            table,
                                            cx,
                                        )
                                    })
                                    .collect();
                                self.group_into_categories(
                                    connection,
                                    connection_id,
                                    db_type,
                                    Some(database.name.clone()),
                                    Some(schema.name.clone()),
                                    &format!(
                                        "category:{}:{}:{}",
                                        connection_id, database.name, schema.name
                                    ),
                                    children,
                                    cx,
                                )
                            } else {
                                Vec::new()
                            };

                            TreeItem::new(schema_key, schema.name.clone(), schema_metadata)
                                .expanded(schema.is_expanded && !schema.tables.is_empty())
                                .children(category_items)
                        })
                        .collect();

                    TreeItem::new(database_key, database.name.clone(), database_metadata)
                        .expanded(database.is_expanded && !database.schemas.is_empty())
                        .children(schema_items)
                })
                .collect();
            base_item.children(database_items)
        } else {
            // MySQL/SQLite: connection -> category folders -> objects (no
            // schema level). Objects across all schemas are merged, then split
            // into per-type category folders.
            let children = metadata
                .schemas
                .iter()
                .flat_map(|schema| {
                    schema
                        .tables
                        .iter()
                        .map(|table| {
                            let table_key =
                                format!("table:{}:{}:{}", connection_id, schema.name, table.name);
                            build_object_item(
                                connection,
                                connection_id,
                                db_type,
                                Some(schema.name.clone()),
                                None,
                                table_key,
                                table,
                                cx,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            let category_items = self.group_into_categories(
                connection,
                connection_id,
                db_type,
                None,
                None,
                &format!("category:{}", connection_id),
                children,
                cx,
            );
            base_item.children(category_items)
        }
    }

    /// Group leaf object items into per-type category folders (Tables, Views,
    /// ...). Each leaf is paired with its `DatabaseItemType` so it can be sorted
    /// into the matching folder. Empty categories are omitted.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn group_into_categories(
        &self,
        connection: &ConnectionData,
        connection_id: i64,
        db_type: database::DatabaseType,
        database_name: Option<String>,
        schema_name: Option<String>,
        category_key_prefix: &str,
        children: Vec<(DatabaseItemType, TreeItem<TreeItemMetadata>)>,
        cx: &Context<Self>,
    ) -> Vec<TreeItem<TreeItemMetadata>> {
        CATEGORY_ORDER
            .iter()
            .copied()
            .filter_map(|(item_type, label, slug)| {
                let category_children: Vec<TreeItem<TreeItemMetadata>> = children
                    .iter()
                    .filter(|(kind, _)| *kind == item_type)
                    .map(|(_, item)| item.clone())
                    .collect();
                if category_children.is_empty() {
                    return None;
                }

                // The "Tables" folder is relabeled per backend (e.g. "Keys" for
                // Redis); other categories keep their static label.
                let label = if item_type == DatabaseItemType::Table {
                    db_type.primary_object_category_label()
                } else {
                    label
                };

                let category_key = format!("{category_key_prefix}:{slug}");
                // Tables are expanded by default; everything else is collapsed.
                // `expanded_categories` records categories the user has toggled
                // away from that default, so membership XORs the default.
                let default_expanded = item_type == DatabaseItemType::Table;
                let is_expanded =
                    default_expanded ^ self.expanded_categories.contains(&category_key);

                // Match the folder's icon and color to the objects it holds so
                // the grouping reads as "more of these".
                let (icon, color, _) = icon_for_item_type(item_type, cx);
                let category_metadata = TreeItemMetadata {
                    connection_id,
                    connection_name: connection.display_name(),
                    kind: TreeItemKind::Category,
                    db_type,
                    database_name: database_name.clone(),
                    schema_name: schema_name.clone(),
                    table_name: None,
                    icon: TreeItemIcon { icon, color },
                    environment_type: Some(connection.environment_type),
                    loading: false,
                    size_bytes: None,
                    relative_size: None,
                };

                // Backends that namespace object names with a separator (e.g.
                // Redis `user:1:name`) fold the flat leaf list into nested
                // folders. SQL backends return `None` and keep a flat list.
                let category_children = match db_type.key_namespace_separator() {
                    Some(separator) => fold_keys_into_namespaces(
                        separator,
                        &category_key,
                        &self.expanded_categories,
                        &category_metadata,
                        TreeItemIcon {
                            icon: IconName::Folder,
                            color: cx.theme().yellow.into(),
                        },
                        category_children,
                    ),
                    None => category_children,
                };

                Some(
                    TreeItem::new(category_key, label, category_metadata)
                        .expanded(is_expanded)
                        .children(category_children),
                )
            })
            .collect()
    }
}

/// Object types grouped into category folders, in display order. The third
/// field is a stable slug used to build the category tree item id.
const CATEGORY_ORDER: &[(DatabaseItemType, &str, &str)] = &[
    (DatabaseItemType::Table, "Tables", "tables"),
    (DatabaseItemType::View, "Views", "views"),
    (
        DatabaseItemType::MaterializedView,
        "Materialized Views",
        "matviews",
    ),
    (DatabaseItemType::Function, "Functions", "functions"),
    (DatabaseItemType::Procedure, "Procedures", "procedures"),
    (DatabaseItemType::Trigger, "Triggers", "triggers"),
];

/// Build a leaf tree item for a single database object, paired with its type so
/// it can later be grouped into the matching category folder.
#[allow(clippy::too_many_arguments)]
fn build_object_item(
    connection: &ConnectionData,
    connection_id: i64,
    db_type: database::DatabaseType,
    database_name: Option<String>,
    schema_name: Option<String>,
    item_key: String,
    table: &DatabaseTable,
    cx: &Context<ConnectionsPanel>,
) -> (DatabaseItemType, TreeItem<TreeItemMetadata>) {
    let (icon, color, kind) = icon_for_item_type(table.item_type, cx);

    let metadata = TreeItemMetadata {
        connection_id,
        connection_name: connection.display_name(),
        kind,
        db_type,
        database_name,
        schema_name,
        table_name: Some(table.name.clone()),
        icon: TreeItemIcon { icon, color },
        environment_type: Some(connection.environment_type),
        loading: false,
        size_bytes: table.size_bytes,
        relative_size: table.relative_size,
    };

    (
        table.item_type,
        TreeItem::new(item_key, table.name.clone(), metadata),
    )
}

/// Fold a flat list of namespaced leaf items into a nested folder tree by
/// splitting each item's label on `separator`. Leaves keep their original
/// `metadata` (so a click still resolves the full key) and are relabeled to
/// their trailing segment. Intermediate folders clone `base_metadata` (with
/// `folder_icon` swapped in) and derive a stable id from the cumulative path
/// under `id_prefix`, so their expansion survives tree rebuilds via
/// `expanded_categories`.
fn fold_keys_into_namespaces(
    separator: char,
    id_prefix: &str,
    expanded_categories: &std::collections::HashSet<String>,
    base_metadata: &TreeItemMetadata,
    folder_icon: TreeItemIcon,
    leaves: Vec<TreeItem<TreeItemMetadata>>,
) -> Vec<TreeItem<TreeItemMetadata>> {
    let entries = leaves
        .into_iter()
        .map(|leaf| {
            let segments: Vec<String> = leaf.label.split(separator).map(str::to_string).collect();
            (segments, leaf)
        })
        .collect();
    fold_namespace_level(
        id_prefix,
        expanded_categories,
        base_metadata,
        &folder_icon,
        entries,
    )
}

/// One level of the namespace fold: group entries by their leading segment,
/// recursing into deeper segments. A segment can be both a leaf (`foo`) and a
/// folder (`foo:bar`); when both occur the direct leaf is listed first inside
/// the folder. Folders sort before plain leaves, each group alphabetically.
fn fold_namespace_level(
    id_prefix: &str,
    expanded_categories: &std::collections::HashSet<String>,
    base_metadata: &TreeItemMetadata,
    folder_icon: &TreeItemIcon,
    entries: Vec<(Vec<String>, TreeItem<TreeItemMetadata>)>,
) -> Vec<TreeItem<TreeItemMetadata>> {
    // BTreeMap keeps segments in stable alphabetical order. Each group holds an
    // optional direct leaf and the deeper entries below that segment.
    type Group = (
        Option<TreeItem<TreeItemMetadata>>,
        Vec<(Vec<String>, TreeItem<TreeItemMetadata>)>,
    );
    let mut groups: std::collections::BTreeMap<String, Group> = std::collections::BTreeMap::new();

    for (mut segments, leaf) in entries {
        if segments.is_empty() {
            continue;
        }
        let head = segments.remove(0);
        let group = groups.entry(head).or_default();
        if segments.is_empty() {
            group.0 = Some(leaf);
        } else {
            group.1.push((segments, leaf));
        }
    }

    let mut folders = Vec::new();
    let mut plain_leaves = Vec::new();
    for (segment, (direct_leaf, deeper)) in groups {
        if deeper.is_empty() {
            if let Some(mut leaf) = direct_leaf {
                leaf.label = segment.into();
                plain_leaves.push(leaf);
            }
            continue;
        }

        let folder_id = format!("{id_prefix}|{segment}");
        let mut children = fold_namespace_level(
            &folder_id,
            expanded_categories,
            base_metadata,
            folder_icon,
            deeper,
        );
        // A key that is both a value and a prefix (`foo` and `foo:bar`) keeps
        // its own leaf at the top of the folder it heads.
        if let Some(mut leaf) = direct_leaf {
            leaf.label = segment.clone().into();
            children.insert(0, leaf);
        }

        // Namespace folders default to collapsed and are recorded in
        // `expanded_categories` only once opened. This matches the default
        // assumed by `record_category_expansion` (only `:tables` ids default
        // open), so a folder's state round-trips across tree rebuilds.
        let is_expanded = expanded_categories.contains(&folder_id);
        let mut folder_metadata = base_metadata.clone();
        folder_metadata.icon = folder_icon.clone();
        folders.push(
            TreeItem::new(folder_id, segment, folder_metadata)
                .expanded(is_expanded)
                .children(children),
        );
    }

    folders.extend(plain_leaves);
    folders
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
        DatabaseItemType::Trigger => (IconName::Zap, cx.theme().red.into(), TreeItemKind::Trigger),
    }
}
