use crate::app_database::{AppDatabase, ConnectionData, EnvironmentType};
use crate::app_events::{AppEvent, TreeItemType};
use blanco_ui::IconName;
use database::{DatabaseService, DatabaseServiceTrait};
use gpui::rems;
use gpui::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::menu::ContextMenuExt;
use gpui_component::{
    ActiveTheme as _, Icon, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    label::Label,
    list::ListItem,
    menu::PopupMenuItem,
    tree::{TreeEntry, TreeItem, TreeState, tree},
    v_flex,
};
use std::sync::Arc;

/// Icon and color combination for tree items
#[derive(Clone)]
pub struct TreeItemIcon {
    pub icon: IconName,
    pub color: gpui::Rgba,
}

impl std::fmt::Debug for TreeItemIcon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TreeItemIcon {{ icon: IconName({:?}), color: rgba(...) }}",
            std::mem::discriminant(&self.icon)
        )
    }
}

/// Represents a database schema with its tables
#[derive(Debug, Clone)]
pub struct DatabaseSchema {
    pub name: String,
    pub tables: Vec<DatabaseTable>,
    pub is_expanded: bool,
}

/// Represents a database table
#[derive(Debug, Clone)]
pub struct DatabaseTable {
    pub name: String,
    pub schema: Option<String>,
}

/// Represents a database with its schemas
#[derive(Debug, Clone)]
pub struct Database {
    pub name: String,
    pub schemas: Vec<DatabaseSchema>,
    pub is_expanded: bool,
}

/// Metadata about a database's schemas and tables
#[derive(Debug, Clone)]
pub struct DatabaseMetadata {
    pub connection_id: Option<i64>,
    pub databases: Vec<Database>, // For PostgreSQL: database -> schema -> table
    pub schemas: Vec<DatabaseSchema>, // For SQLite: direct schema -> table
    pub supports_schemas: bool,
}

pub struct ConnectionsPanel {
    connections: Vec<ConnectionData>,
    selected_connection_id: Option<i64>,
    database_metadata: std::collections::HashMap<i64, DatabaseMetadata>, // Store metadata per connection
    tree_state: Entity<TreeState>,
    loaded_connections: std::collections::HashSet<i64>,
    expanded_connections: std::collections::HashSet<i64>, // Track which connections are expanded
    tree_item_metadata: std::collections::HashMap<String, TreeItemMetadata>, // Map hierarchical key -> metadata
    next_item_id: u32, // Serial ID for tree items within each connection
    connection_status: std::collections::HashMap<String, bool>, // Track connection status for each connection and database
}

/// Type of tree item in the metadata context
#[derive(Clone, Debug, PartialEq)]
pub enum TreeItemKind {
    Connection,
    Database,
    Schema,
    Table,
}

/// Metadata for tree items to enable proper context menu actions
#[derive(Clone, Debug)]
pub struct TreeItemMetadata {
    pub connection_id: i64,
    pub connection_name: String,
    pub kind: TreeItemKind,
    pub database_name: Option<String>,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub icon: TreeItemIcon,
    pub environment_type: Option<EnvironmentType>,
}

impl ConnectionsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));

        // Load initial connections
        let app_database = AppDatabase::global(cx);
        let connections = async_std::task::block_on(async {
            match app_database.load_connections().await {
                Ok(connections) => connections,
                Err(e) => {
                    tracing::error!("Failed to load connections: {}", e);
                    vec![]
                }
            }
        });

        let panel = Self {
            connections,
            selected_connection_id: None,
            database_metadata: std::collections::HashMap::new(),
            tree_state,
            loaded_connections: std::collections::HashSet::new(),
            expanded_connections: std::collections::HashSet::new(),
            tree_item_metadata: std::collections::HashMap::new(),
            next_item_id: 1, // Start with 1 to avoid potential issues with 0
            connection_status: std::collections::HashMap::new(),
        };

        cx.spawn(async |this_handle, cx| {
            let _ = this_handle.update(cx, |this, cx| {
                this.update_tree_items(cx);
                cx.notify();
            });
        })
        .detach();

        panel
    }

    /// Update tree items from connections data
    fn update_tree_items(&mut self, cx: &mut Context<Self>) {
        // Clear existing metadata
        self.tree_item_metadata.clear();

        // Collect connection IDs first to avoid borrowing issues
        let connection_ids: Vec<i64> = self.connections.iter().filter_map(|conn| conn.id).collect();

        // Populate metadata for all connections
        for connection_id in connection_ids {
            self.populate_tree_item_metadata(connection_id, cx);
        }

        let tree_items: Vec<TreeItem> = self
            .connections
            .iter()
            .map(|conn| self.build_connection_tree_item(conn))
            .collect();

        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items, cx);
        });
    }

    /// Build a TreeItem for a connection including its children if loaded
    fn build_connection_tree_item(&self, connection: &ConnectionData) -> TreeItem {
        let connection_id = connection.id.unwrap_or(0);
        let should_expand = self.expanded_connections.contains(&connection_id);
        self.build_connection_tree_item_with_expand(connection, should_expand)
    }

    /// Build a TreeItem for a connection including its children if loaded, with specified expand state
    fn build_connection_tree_item_with_expand(
        &self,
        connection: &ConnectionData,
        should_expand: bool,
    ) -> TreeItem {
        let connection_id = connection.id.unwrap_or(0);

        // Use hierarchical key: "connection:123" for connections
        let base_item = TreeItem::new(
            format!("connection:{}", connection_id),
            connection.display_name(),
        )
        .expanded(should_expand);

        // If this connection's metadata is loaded, add children
        if let Some(metadata) = self.database_metadata.get(&connection_id) {
            if metadata.supports_schemas {
                // PostgreSQL: connection -> databases -> schemas -> tables
                let database_items: Vec<TreeItem> = metadata
                    .databases
                    .iter()
                    .map(|database| {
                        let database_key = format!("database:{}:{}", connection_id, database.name);

                        let schema_items: Vec<TreeItem> = database
                            .schemas
                            .iter()
                            .map(|schema| {
                                let schema_key = format!(
                                    "schema:{}:{}:{}",
                                    connection_id, database.name, schema.name
                                );

                                let table_items: Vec<TreeItem> = if schema.is_expanded {
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
                                            TreeItem::new(table_key, table.name.clone())
                                        })
                                        .collect()
                                } else {
                                    Vec::new() // Tables not loaded or collapsed
                                };

                                TreeItem::new(schema_key, schema.name.clone())
                                    .expanded(schema.is_expanded && !schema.tables.is_empty())
                                    .children(table_items)
                            })
                            .collect();

                        TreeItem::new(database_key, database.name.clone())
                            .expanded(database.is_expanded && !database.schemas.is_empty())
                            .children(schema_items)
                    })
                    .collect();
                base_item.children(database_items)
            } else {
                // SQLite: connection -> tables (no schema level for cleaner UI)
                let table_items: Vec<TreeItem> = metadata
                    .schemas
                    .iter()
                    .flat_map(|schema| {
                        // For SQLite, show tables directly under connection
                        schema
                            .tables
                            .iter()
                            .map(|table| {
                                let table_key = format!(
                                    "table:{}:{}:{}",
                                    connection_id, schema.name, table.name
                                );
                                TreeItem::new(table_key, table.name.clone())
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                base_item.children(table_items)
            }
        } else {
            base_item
        }
    }

    /// Load connection children (databases for PostgreSQL, schemas for SQLite) on demand
    fn load_connection_children(
        &mut self,
        connection_id: i64,
        expand: bool,
        cx: &mut Context<Self>,
    ) {
        if self.loaded_connections.contains(&connection_id) {
            return; // Already loaded
        }

        let db_service = DatabaseService::global(cx).clone();

        cx.spawn(async move |this_handle, cx| {
            match db_service
                .get_or_create_connection(connection_id, None)
                .await
            {
                Ok(connection) => {
                    tracing::debug!("Connected to database: {}", connection.get_display_name());

                    // Check if the connection supports schemas
                    let supports_schemas = connection.supports_schemas();

                    let metadata = if supports_schemas {
                        // For PostgreSQL: Load databases only (schemas will be loaded lazily)
                        let databases = match connection.get_databases().await {
                            Ok(db_list) => {
                                tracing::info!(
                                    "Connection {} supports schemas. Loaded {} databases: {:?}",
                                    connection_id,
                                    db_list.len(),
                                    db_list
                                );
                                db_list
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to load databases for connection {}: {}",
                                    connection_id,
                                    e
                                );
                                Vec::new()
                            }
                        };

                        DatabaseMetadata {
                            connection_id: Some(connection_id),
                            databases: databases
                                .into_iter()
                                .map(|db_name| Database {
                                    name: db_name,
                                    schemas: Vec::new(), // Will be loaded lazily
                                    is_expanded: false,
                                })
                                .collect(),
                            schemas: Vec::new(), // Not used for PostgreSQL
                            supports_schemas,
                        }
                    } else {
                        // For SQLite: Load schemas and their tables (no databases level)
                        let schemas = match connection.get_schemas().await {
                            Ok(schema_list) => {
                                tracing::info!(
                                    "Connection {} loaded {} schemas: {:?}",
                                    connection_id,
                                    schema_list.len(),
                                    schema_list
                                );
                                schema_list
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to load schemas for connection {}: {}",
                                    connection_id,
                                    e
                                );
                                Vec::new()
                            }
                        };

                        // Load tables for all schemas (SQLite needs this for proper tree display)
                        let mut schema_tables: Vec<(String, Vec<DatabaseTable>)> = Vec::new();
                        for schema_name in &schemas {
                            let tables = match connection.get_tables(Some(schema_name)).await {
                                Ok(table_list) => table_list
                                    .into_iter()
                                    .map(|table_name| DatabaseTable {
                                        name: table_name,
                                        schema: Some(schema_name.clone()),
                                    })
                                    .collect(),
                                Err(e) => {
                                    tracing::error!(
                                        "Failed to load tables for schema {}: {}",
                                        schema_name,
                                        e
                                    );
                                    Vec::new()
                                }
                            };
                            schema_tables.push((schema_name.clone(), tables));
                        }

                        DatabaseMetadata {
                            connection_id: Some(connection_id),
                            databases: Vec::new(), // Not used for SQLite
                            schemas: schema_tables
                                .into_iter()
                                .map(|(schema_name, tables)| DatabaseSchema {
                                    name: schema_name,
                                    tables,
                                    is_expanded: false,
                                })
                                .collect(),
                            supports_schemas,
                        }
                    };

                    // Update the panel with loaded metadata
                    let _ = this_handle.update(cx, |this, cx| {
                        this.database_metadata.insert(connection_id, metadata);
                        this.loaded_connections.insert(connection_id);
                        if expand {
                            this.expanded_connections.insert(connection_id);
                        }

                        // Build tree items with all loaded connections visible
                        this.update_tree_items(cx);

                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to get connection {}: {}", connection_id, e);
                }
            }
        })
        .detach();
    }

    /// Load schemas for a specific database (PostgreSQL only)
    fn load_database_children(
        &mut self,
        connection_id: i64,
        database_name: String,
        cx: &mut Context<Self>,
    ) {
        let db_service = DatabaseService::global(cx).clone();

        cx.spawn(async move |this_handle, cx| {
            match db_service
                .get_or_create_connection(connection_id, Some(&database_name))
                .await
            {
                Ok(connection) => {
                    tracing::debug!(
                        "Loading schemas for database: {} on connection {}",
                        database_name,
                        connection_id
                    );

                    // Load schemas for this specific database
                    let schemas = match connection.get_schemas().await {
                        Ok(schema_list) => {
                            tracing::info!(
                                "Loaded {} schemas for database {} on connection {}: {:?}",
                                schema_list.len(),
                                database_name,
                                connection_id,
                                schema_list
                            );
                            schema_list
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to load schemas for database {} on connection {}: {}",
                                database_name,
                                connection_id,
                                e
                            );
                            Vec::new()
                        }
                    };

                    // Load tables for all schemas (without lazy loading for now)
                    let mut schema_tables: Vec<(String, Vec<DatabaseTable>)> = Vec::new();
                    for schema_name in &schemas {
                        let tables = match connection.get_tables(Some(schema_name)).await {
                            Ok(table_list) => table_list
                                .into_iter()
                                .map(|table_name| DatabaseTable {
                                    name: table_name,
                                    schema: Some(schema_name.clone()),
                                })
                                .collect(),
                            Err(e) => {
                                tracing::error!(
                                    "Failed to load tables for schema {} in database {}: {}",
                                    schema_name,
                                    database_name,
                                    e
                                );
                                Vec::new()
                            }
                        };
                        schema_tables.push((schema_name.clone(), tables));
                    }

                    // Update the panel with loaded schemas
                    let _ = this_handle.update(cx, |this, cx| {
                        if let Some(metadata) = this.database_metadata.get_mut(&connection_id) {
                            // Find the database and update its schemas
                            if let Some(database) = metadata
                                .databases
                                .iter_mut()
                                .find(|db| db.name == database_name)
                            {
                                database.schemas = schema_tables
                                    .into_iter()
                                    .map(|(schema_name, tables)| DatabaseSchema {
                                        name: schema_name,
                                        tables,
                                        is_expanded: false,
                                    })
                                    .collect();
                                database.is_expanded = true; // Mark as expanded
                            }

                            // Rebuild tree to show loaded schemas
                            this.update_tree_items(cx);

                            cx.emit(AppEvent::SchemasLoaded {
                                connection_id: Some(connection_id),
                                database_name: Some(database_name),
                                schemas: schemas.clone(),
                            });
                            cx.notify();
                        }
                    });
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to get connection for database {} on connection {}: {}",
                        database_name,
                        connection_id,
                        e
                    );
                }
            }
        })
        .detach();
    }

    /// Load tables for a specific schema (PostgreSQL only)
    fn load_schema_children(
        &mut self,
        connection_id: i64,
        database_name: String,
        schema_name: String,
        cx: &mut Context<Self>,
    ) {
        let db_service = DatabaseService::global(cx).clone();

        cx.spawn(async move |this_handle, cx| {
            match db_service.get_or_create_connection(connection_id, Some(&database_name)).await {
                Ok(connection) => {
                    tracing::debug!("Loading tables for schema: {} in database: {} on connection {}", schema_name, database_name, connection_id);

                    // Load tables for this specific schema
                    let tables = match connection.get_tables(Some(&schema_name)).await {
                        Ok(table_list) => {
                            tracing::info!(
                                "Loaded {} tables for schema {} in database {} on connection {}: {:?}",
                                table_list.len(),
                                schema_name,
                                database_name,
                                connection_id,
                                table_list
                            );
                            table_list
                                .into_iter()
                                .map(|table_name| DatabaseTable {
                                    name: table_name,
                                    schema: Some(schema_name.clone()),
                                })
                                .collect()
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to load tables for schema {} in database {} on connection {}: {}",
                                schema_name,
                                database_name,
                                connection_id,
                                e
                            );
                            Vec::new()
                        }
                    };

                    // Update the panel with loaded tables
                    let _ = this_handle.update(cx, |this, cx| {
                        if let Some(metadata) = this.database_metadata.get_mut(&connection_id) {
                            // Find the database and schema, then update its tables
                            if let Some(database) = metadata.databases.iter_mut().find(|db| db.name == database_name) {
                                if let Some(schema) = database.schemas.iter_mut().find(|s| s.name == schema_name) {
                                    schema.tables = tables;
                                    schema.is_expanded = true; // Mark as expanded
                                }
                            }

                            // Rebuild tree to show loaded tables
                            this.update_tree_items(cx);
                            cx.notify();
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to get connection for schema {} in database {} on connection {}: {}", schema_name, database_name, connection_id, e);
                }
            }
        })
        .detach();
    }

    /// Handle tree item click and expand/collapse
    fn handle_tree_item_click(&mut self, item_id: &str, cx: &mut Context<Self>) {
        // Use tree_item_metadata directly instead of parsing the key
        if let Some(metadata) = self.get_tree_item_metadata(item_id).cloned() {
            let connection_id = metadata.connection_id;
            match metadata.kind {
                TreeItemKind::Connection => {
                    // Check if already loaded, if not load first
                    if !self.loaded_connections.contains(&connection_id) {
                        tracing::debug!("Loading connection {} for first time", connection_id);
                        self.load_connection_children(connection_id, true, cx);
                    } else {
                        // Already loaded, toggle expand/collapse the tree
                        tracing::debug!("Toggling expansion for connection {}", connection_id);
                        self.toggle_connection_expansion(connection_id, cx);
                    }
                    cx.emit(AppEvent::TreeItemExpanded {
                        item_id: item_id.to_string(),
                        item_type: TreeItemType::Connection,
                        connection_id: Some(connection_id),
                    });
                }
                TreeItemKind::Database => {
                    if let Some(database_name) = &metadata.database_name {
                        if let Some(connection_metadata) =
                            self.database_metadata.get(&connection_id)
                        {
                            if let Some(database) = connection_metadata
                                .databases
                                .iter()
                                .find(|db| db.name == *database_name)
                            {
                                if database.schemas.is_empty() {
                                    tracing::debug!(
                                        "Loading schemas for database: {}",
                                        database_name
                                    );
                                    self.load_database_children(
                                        connection_id,
                                        database_name.clone(),
                                        cx,
                                    );
                                } else {
                                    // Already loaded, toggle expansion
                                    tracing::debug!(
                                        "Toggling expansion for database: {}",
                                        database_name
                                    );
                                    self.toggle_database_expansion(
                                        connection_id,
                                        database_name,
                                        cx,
                                    );
                                }
                            }
                        }
                    }
                    cx.emit(AppEvent::TreeItemExpanded {
                        item_id: item_id.to_string(),
                        item_type: TreeItemType::Database,
                        connection_id: Some(connection_id),
                    });
                }
                TreeItemKind::Schema => {
                    if let (Some(database_name), Some(schema_name)) =
                        (&metadata.database_name, &metadata.schema_name)
                    {
                        if let Some(connection_metadata) =
                            self.database_metadata.get(&connection_id)
                        {
                            if let Some(database) = connection_metadata
                                .databases
                                .iter()
                                .find(|db| db.name == *database_name)
                            {
                                if let Some(schema) =
                                    database.schemas.iter().find(|s| s.name == *schema_name)
                                {
                                    if schema.tables.is_empty() {
                                        tracing::debug!(
                                            "Loading tables for schema: {} in database: {}",
                                            schema_name,
                                            database_name
                                        );
                                        self.load_schema_children(
                                            connection_id,
                                            database_name.clone(),
                                            schema_name.clone(),
                                            cx,
                                        );
                                    } else {
                                        // Already loaded, toggle expansion
                                        tracing::debug!(
                                            "Toggling expansion for schema: {} in database: {}",
                                            schema_name,
                                            database_name
                                        );
                                        self.toggle_schema_expansion(
                                            connection_id,
                                            database_name,
                                            schema_name,
                                            cx,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    cx.emit(AppEvent::TreeItemExpanded {
                        item_id: item_id.to_string(),
                        item_type: TreeItemType::Schema,
                        connection_id: Some(connection_id),
                    });
                }
                TreeItemKind::Table => {
                    cx.emit(AppEvent::TreeItemSelected {
                        item_id: item_id.to_string(),
                        item_type: TreeItemType::Table,
                        connection_id: Some(connection_id),
                        database_name: metadata.database_name,
                        schema_name: metadata.schema_name,
                        table_name: metadata.table_name,
                    });
                }
            }
        }
    }

    /// Toggle connection expansion in the tree
    fn toggle_connection_expansion(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        // Toggle the expansion state
        if self.expanded_connections.contains(&connection_id) {
            tracing::debug!("Collapsing connection {}", connection_id);
            self.expanded_connections.remove(&connection_id);
        } else {
            tracing::debug!("Expanding connection {}", connection_id);
            self.expanded_connections.insert(connection_id);
        }

        // Rebuild tree with updated expansion state
        self.update_tree_items(cx);
    }

    /// Toggle database expansion in the tree
    fn toggle_database_expansion(
        &mut self,
        connection_id: i64,
        database_name: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(metadata) = self.database_metadata.get_mut(&connection_id) {
            if let Some(database) = metadata
                .databases
                .iter_mut()
                .find(|db| db.name == database_name)
            {
                database.is_expanded = !database.is_expanded;
                tracing::debug!(
                    "Toggling database '{}' expansion to: {}",
                    database_name,
                    database.is_expanded
                );

                // Rebuild tree with updated expansion state
                self.update_tree_items(cx);
            }
        }
    }

    /// Toggle schema expansion in the tree
    fn toggle_schema_expansion(
        &mut self,
        connection_id: i64,
        database_name: &str,
        schema_name: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(metadata) = self.database_metadata.get_mut(&connection_id) {
            if let Some(database) = metadata
                .databases
                .iter_mut()
                .find(|db| db.name == database_name)
            {
                if let Some(schema) = database.schemas.iter_mut().find(|s| s.name == schema_name) {
                    schema.is_expanded = !schema.is_expanded;
                    tracing::debug!(
                        "Toggling schema '{}' in database '{}' expansion to: {}",
                        schema_name,
                        database_name,
                        schema.is_expanded
                    );

                    // Rebuild tree with updated expansion state
                    self.update_tree_items(cx);
                }
            }
        }
    }

    /// Build context menu for a tree item based on its type
    fn build_context_menu(
        &self,
        item: &TreeItem,
        weak_panel: gpui::WeakEntity<Self>,
    ) -> impl Fn(
        gpui_component::menu::PopupMenu,
        &mut Window,
        &mut gpui::Context<gpui_component::menu::PopupMenu>,
    ) -> gpui_component::menu::PopupMenu
    + 'static {
        let item = item.clone();
        let metadata = self.get_tree_item_metadata(&item.id).cloned();

        move |this, _window, _cx| {
            // Build context menu based on item type using metadata
            if metadata.as_ref().map(|m| &m.kind) == Some(&TreeItemKind::Connection) {
                // Connection level
                // Connection context menu - clone for each closure
                let refresh_label_refresh = item.label.clone();
                let disconnect_label = item.label.clone();
                let edit_label = item.label.clone();
                let weak_panel_clone = weak_panel.clone();
                let weak_panel_disconnect = weak_panel.clone();
                let connection_id = metadata
                    .as_ref()
                    .map(|m| m.connection_id)
                    .unwrap_or_default();

                this.item(
                    PopupMenuItem::new("Refresh").on_click(move |_, _window, cx| {
                        tracing::info!("Refresh connection: {}", refresh_label_refresh);
                        if let Some(panel) = weak_panel_clone.upgrade() {
                            panel.update(cx, |_this, _cx| {
                                // cx.emit(AppEvent::CreateNewQueryTab {})
                            });
                        }
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Disconnect").on_click(move |_, _window, cx| {
                        tracing::info!("Disconnect connection: {}", disconnect_label);
                        if let Some(panel) = weak_panel_disconnect.upgrade() {
                            panel.update(cx, |this, cx| {
                                this.disconnect_connection(connection_id, cx);
                            });
                        }
                    }),
                )
                .item(
                    PopupMenuItem::new("Edit Connection").on_click(move |_, _window, _cx| {
                        tracing::info!("Edit connection: {}", edit_label);
                        // TODO: Implement edit connection functionality
                    }),
                )
            } else if metadata.as_ref().map(|m| &m.kind) == Some(&TreeItemKind::Database) {
                // Database level (PostgreSQL)
                // Database context menu - clone for each closure
                let new_query_label = item.label.clone();
                let refresh_label = item.label.clone();
                let weak_panel_clone = weak_panel.clone();
                let connection_id = metadata
                    .as_ref()
                    .map(|m| m.connection_id)
                    .unwrap_or_default();
                let database_name = metadata.as_ref().and_then(|m| m.database_name.clone());
                let connection_name = metadata
                    .as_ref()
                    .map(|m| m.connection_name.clone())
                    .unwrap_or_default();

                // Get environment type from metadata
                let environment_type = metadata.as_ref().and_then(|m| m.environment_type);

                this.item(PopupMenuItem::new("New Query").on_click({
                    let environment_type = environment_type;
                    move |_, _window, cx| {
                        tracing::info!("New Query for database: {}", new_query_label);
                        if let Some(panel) = weak_panel_clone.upgrade() {
                            panel.update(cx, |_this, cx| {
                                cx.emit(AppEvent::CreateNewQueryTab {
                                    connection_id,
                                    connection_name: connection_name.clone(),
                                    database_name: database_name.clone().unwrap_or_default(),
                                    schema_name: None,
                                    table_name: None,
                                    environment_type,
                                });
                            });
                        }
                    }
                }))
                .item(
                    PopupMenuItem::new("Refresh Database").on_click(move |_, _window, _cx| {
                        tracing::info!("Refresh database: {}", refresh_label);
                        // TODO: Implement database refresh functionality
                    }),
                )
            } else if metadata.as_ref().map(|m| &m.kind) == Some(&TreeItemKind::Schema) {
                // Schema level
                // Schema context menu - clone for each closure
                let new_query_label = item.label.clone();
                let refresh_label = item.label.clone();
                let weak_panel_clone = weak_panel.clone();
                let schema_name = metadata.as_ref().and_then(|m| m.schema_name.clone());
                let connection_id = metadata
                    .as_ref()
                    .map(|m| m.connection_id)
                    .unwrap_or_default();
                let database_name = metadata
                    .as_ref()
                    .and_then(|m| m.database_name.clone())
                    .unwrap_or_default();
                let connection_name = metadata
                    .as_ref()
                    .map(|m| m.connection_name.clone())
                    .unwrap_or_default();

                // Get environment type from metadata
                let environment_type = metadata.as_ref().and_then(|m| m.environment_type);

                this.item(PopupMenuItem::new("New Query").on_click({
                    let environment_type = environment_type;
                    move |_, _window, cx| {
                        tracing::info!("New Query for schema: {}", new_query_label);
                        if let Some(panel) = weak_panel_clone.upgrade() {
                            panel.update(cx, |_, cx| {
                                cx.emit(AppEvent::CreateNewQueryTab {
                                    connection_id,
                                    connection_name: connection_name.clone(),
                                    database_name: database_name.clone(),
                                    schema_name: schema_name.clone(),
                                    table_name: None,
                                    environment_type,
                                });
                            });
                        }
                    }
                }))
                .item(
                    PopupMenuItem::new("Refresh Schema").on_click(move |_, _window, _cx| {
                        tracing::info!("Refresh schema: {}", refresh_label);
                        // TODO: Implement refresh schema functionality
                    }),
                )
            } else if metadata.as_ref().map(|m| &m.kind) == Some(&TreeItemKind::Table) {
                // Table level
                // Table context menu - clone for each closure
                let select_label_new_query = item.label.clone();
                let weak_panel_new_query = weak_panel.clone();
                let table_schema_name_new_query =
                    metadata.as_ref().and_then(|m| m.schema_name.clone());
                let table_name_new_query = metadata.as_ref().and_then(|m| m.table_name.clone());
                let connection_id_new_query = metadata
                    .as_ref()
                    .map(|m| m.connection_id)
                    .unwrap_or_default();
                let database_name_new_query = metadata
                    .as_ref()
                    .and_then(|m| m.database_name.clone())
                    .unwrap_or_default();
                let connection_name_new_query = metadata
                    .as_ref()
                    .map(|m| m.connection_name.clone())
                    .unwrap_or_default();

                // Get environment type from metadata
                let environment_type = metadata.as_ref().and_then(|m| m.environment_type);

                let select_label_export = item.label.clone();
                let weak_panel_export = weak_panel.clone();
                let table_schema_name_export =
                    metadata.as_ref().and_then(|m| m.schema_name.clone());
                let table_name_export = metadata.as_ref().and_then(|m| m.table_name.clone());
                let connection_id_export = metadata
                    .as_ref()
                    .map(|m| m.connection_id)
                    .unwrap_or_default();
                let database_name_export = metadata
                    .as_ref()
                    .and_then(|m| m.database_name.clone())
                    .unwrap_or_default();
                let connection_name_export = metadata
                    .as_ref()
                    .map(|m| m.connection_name.clone())
                    .unwrap_or_default();

                this.item(PopupMenuItem::new("New Query").on_click({
                    let environment_type = environment_type;
                    move |_, _window, cx| {
                        tracing::info!("New Query for table: {}", select_label_new_query);
                        if let Some(panel) = weak_panel_new_query.upgrade() {
                            panel.update(cx, |_, cx| {
                                cx.emit(AppEvent::CreateNewQueryTab {
                                    connection_id: connection_id_new_query,
                                    connection_name: connection_name_new_query.clone(),
                                    database_name: database_name_new_query.clone(),
                                    schema_name: table_schema_name_new_query.clone(),
                                    table_name: table_name_new_query.clone(),
                                    environment_type,
                                });
                            });
                        }
                    }
                }))
                .item(
                    PopupMenuItem::new("Export Data").on_click(move |_, window, cx| {
                        tracing::info!("Export data for table: {}", select_label_export);
                        if let Some(panel) = weak_panel_export.upgrade() {
                            panel.update(cx, |panel, cx| {
                                panel.export_table_data(
                                    connection_id_export,
                                    connection_name_export.clone(),
                                    database_name_export.clone(),
                                    table_schema_name_export.clone(),
                                    table_name_export.clone(),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
                )
            } else {
                // Default context menu
                this.label(item.label.clone())
            }
        }
    }

    /// Get tree item metadata by item_id
    fn get_tree_item_metadata(&self, item_id: &str) -> Option<&TreeItemMetadata> {
        self.tree_item_metadata.get(item_id)
    }

    /// Populate tree item metadata when building tree items
    fn populate_tree_item_metadata(&mut self, connection_id: i64, cx: &Context<Self>) {
        // Get the connection data
        let connection_data = self
            .connections
            .iter()
            .find(|conn| conn.id == Some(connection_id));

        // Get the connection name and environment type
        let connection_name = connection_data
            .map(|conn| conn.display_name())
            .unwrap_or_else(|| format!("Connection {}", connection_id));

        let connection_environment_type = connection_data.map(|conn| conn.environment_type);

        // Check if the connection is actually established using the database service
        // Initially assume not connected, we'll update this asynchronously
        let connection_icon = TreeItemIcon {
            icon: IconName::Database,
            color: cx.theme().foreground.into(),
        };

        // Clone the database service before entering async context
        let db_service = DatabaseService::global(cx).clone();

        // Spawn a task to check connection status
        cx.spawn(async move |this_handle, cx| {
            let is_connected = db_service.is_connected(connection_id, None).await;

            // Store the connection status
            let _ = this_handle.update(cx, |this, cx| {
                this.connection_status
                    .insert(format!("connection:{}", connection_id), is_connected);
                cx.notify();
            });
        })
        .detach();

        // Add connection metadata (using hierarchical key)
        // Check if we already know the connection status
        let initial_icon = self
            .connection_status
            .get(&format!("connection:{}", connection_id))
            .map(|&is_connected| {
                if is_connected {
                    TreeItemIcon {
                        icon: IconName::DatabaseConnected,
                        color: cx.theme().primary.into(),
                    }
                } else {
                    TreeItemIcon {
                        icon: IconName::Database,
                        color: cx.theme().foreground.into(),
                    }
                }
            })
            .unwrap_or(connection_icon);

        self.tree_item_metadata.insert(
            format!("connection:{}", connection_id),
            TreeItemMetadata {
                connection_id,
                connection_name: connection_name.clone(),
                kind: TreeItemKind::Connection,
                database_name: None, // Connection level doesn't have a specific database
                schema_name: None,
                table_name: None,
                icon: initial_icon,
                environment_type: connection_environment_type,
            },
        );

        // Add database, schema and table metadata if connection is loaded
        if let Some(metadata) = self.database_metadata.get(&connection_id) {
            if !metadata.databases.is_empty() && metadata.supports_schemas {
                // PostgreSQL: connection -> databases -> schemas -> tables
                for database in &metadata.databases {
                    let database_key = format!("database:{}:{}", connection_id, database.name);

                    // Check if we already know the database connection status
                    let database_icon = self
                        .connection_status
                        .get(&format!("database:{}:{}", connection_id, database.name))
                        .map(|&is_connected| {
                            if is_connected {
                                TreeItemIcon {
                                    icon: IconName::DatabaseConnected,
                                    color: cx.theme().primary.into(),
                                }
                            } else {
                                TreeItemIcon {
                                    icon: IconName::Database,
                                    color: cx.theme().foreground.into(),
                                }
                            }
                        })
                        .unwrap_or_else(|| TreeItemIcon {
                            icon: IconName::Database,
                            color: cx.theme().foreground.into(),
                        });

                    self.tree_item_metadata.insert(
                        database_key.clone(),
                        TreeItemMetadata {
                            connection_id,
                            connection_name: connection_name.clone(),
                            kind: TreeItemKind::Database,
                            database_name: Some(database.name.clone()),
                            schema_name: None,
                            table_name: None,
                            icon: database_icon,
                            environment_type: connection_environment_type,
                        },
                    );

                    // Clone the database service before entering async context
                    let db_service = DatabaseService::global(cx).clone();
                    let db_name = database.name.clone();

                    // Spawn a task to check database connection status
                    cx.spawn(async move |this_handle, cx| {
                        let is_connected =
                            db_service.is_connected(connection_id, Some(&db_name)).await;

                        // Store the database connection status
                        let _ = this_handle.update(cx, |this, cx| {
                            this.connection_status.insert(
                                format!("database:{}:{}", connection_id, db_name),
                                is_connected,
                            );
                            cx.notify();
                        });
                    })
                    .detach();

                    // Add schemas for this database
                    for schema in &database.schemas {
                        let schema_key =
                            format!("schema:{}:{}:{}", connection_id, database.name, schema.name);
                        self.tree_item_metadata.insert(
                            schema_key,
                            TreeItemMetadata {
                                connection_id,
                                connection_name: connection_name.clone(),
                                kind: TreeItemKind::Schema,
                                database_name: Some(database.name.clone()),
                                schema_name: Some(schema.name.clone()),
                                table_name: None,
                                icon: TreeItemIcon {
                                    icon: IconName::Folder,
                                    color: cx.theme().foreground.into(),
                                },
                                environment_type: connection_environment_type,
                            },
                        );

                        // Add table metadata for each schema
                        for table in &schema.tables {
                            let table_key = format!(
                                "table:{}:{}:{}:{}",
                                connection_id, database.name, schema.name, table.name
                            );
                            self.tree_item_metadata.insert(
                                table_key,
                                TreeItemMetadata {
                                    connection_id,
                                    connection_name: connection_name.clone(),
                                    kind: TreeItemKind::Table,
                                    database_name: Some(database.name.clone()),
                                    schema_name: Some(schema.name.clone()),
                                    table_name: Some(table.name.clone()),
                                    icon: TreeItemIcon {
                                        icon: IconName::Sheet,
                                        color: cx.theme().foreground.into(),
                                    },
                                    environment_type: connection_environment_type,
                                },
                            );
                        }
                    }
                }
            } else {
                // SQLite: connection -> tables (no schema level in UI)
                for schema in &metadata.schemas {
                    // Add table metadata for each schema (store schema internally but don't show in UI)
                    for table in &schema.tables {
                        let table_key =
                            format!("table:{}:{}:{}", connection_id, schema.name, table.name);
                        self.tree_item_metadata.insert(
                            table_key,
                            TreeItemMetadata {
                                connection_id,
                                connection_name: connection_name.clone(),
                                kind: TreeItemKind::Table,
                                database_name: None,
                                schema_name: Some(schema.name.clone()),
                                table_name: Some(table.name.clone()),
                                icon: TreeItemIcon {
                                    icon: IconName::Sheet,
                                    color: cx.theme().foreground.into(),
                                },
                                environment_type: connection_environment_type,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Handle connection established event
    pub fn handle_connection_established(&mut self, _connection_id: i64, cx: &mut Context<Self>) {
        // Refresh the tree to show all connections as connected
        self.update_tree_items(cx);
        cx.notify();
    }

    /// Handle connection lost event
    pub fn handle_connection_lost(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        // Remove the metadata for this connection and refresh tree
        if self.database_metadata.remove(&connection_id).is_some() {
            self.loaded_connections.remove(&connection_id);
            self.update_tree_items(cx);
        }
        cx.notify();
    }

    /// Disconnect and remove a connection
    pub fn disconnect_connection(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        tracing::info!("Disconnecting connection {}", connection_id);

        // Remove connection from the connections list
        self.connections
            .retain(|conn| conn.id != Some(connection_id));

        // Remove metadata and tracking
        self.database_metadata.remove(&connection_id);
        self.loaded_connections.remove(&connection_id);
        self.expanded_connections.remove(&connection_id);

        // Remove any tree item metadata for this connection
        let keys_to_remove: Vec<String> = self
            .tree_item_metadata
            .keys()
            .filter(|key| {
                key.starts_with(&format!("connection:{}", connection_id))
                    || key.starts_with(&format!("database:{}:", connection_id))
                    || key.starts_with(&format!("schema:{}:", connection_id))
                    || key.starts_with(&format!("table:{}:", connection_id))
            })
            .cloned()
            .collect();

        for key in keys_to_remove {
            self.tree_item_metadata.remove(&key);
        }

        // Update the tree to reflect the removal
        self.update_tree_items(cx);

        // Emit connection lost event for any listeners
        cx.emit(AppEvent::ConnectionLost {
            connection_id: Some(connection_id),
            error: "Disconnected by user".to_string(),
        });

        cx.notify();
    }

    /// Validate and mark a connection as connected after successful query execution
    pub fn validate_connection_as_connected(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        self.load_connection_children(connection_id, false, cx);

        tracing::info!(
            "Validated and marked connection {} as connected",
            connection_id
        );
        cx.notify();
    }

    fn render_header_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .gap_2()
            .px_3()
            .py(px(6.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Label::new("Connections")
                    .font_bold()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground),
            )
    }

    fn render_connections_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();

        tree(&self.tree_state, move |ix, entry, selected, window, cx| {
            view.update(cx, |this, cx| {
                this.render_tree_item(ix, entry, selected, window, cx)
            })
        })
    }

    fn render_tree_item(
        &self,
        ix: usize,
        entry: &TreeEntry,
        selected: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ListItem {
        let item = entry.item();
        let depth = entry.depth();
        let weak_panel = cx.weak_entity();

        // Get icon from metadata
        let mut tree_item_icon = self
            .get_tree_item_metadata(&item.id)
            .map(|metadata| metadata.icon.clone())
            .unwrap_or_else(|| TreeItemIcon {
                icon: IconName::File,
                color: cx.theme().foreground.into(),
            });

        // Update icon based on connection status if we have it stored
        let environment_label = if let Some(metadata) = self.get_tree_item_metadata(&item.id) {
            let status_key = match metadata.kind {
                TreeItemKind::Connection => format!("connection:{}", metadata.connection_id),
                TreeItemKind::Database => {
                    if let Some(ref db_name) = metadata.database_name {
                        format!("database:{}:{}", metadata.connection_id, db_name)
                    } else {
                        format!("connection:{}", metadata.connection_id)
                    }
                }
                _ => String::new(),
            };

            if !status_key.is_empty() {
                if let Some(&is_connected) = self.connection_status.get(&status_key) {
                    tree_item_icon = if is_connected {
                        TreeItemIcon {
                            icon: IconName::DatabaseConnected,
                            color: cx.theme().primary.into(),
                        }
                    } else {
                        match metadata.kind {
                            TreeItemKind::Connection => TreeItemIcon {
                                icon: IconName::Database,
                                color: cx.theme().foreground.into(),
                            },
                            TreeItemKind::Database => TreeItemIcon {
                                icon: IconName::Database,
                                color: cx.theme().foreground.into(),
                            },
                            _ => tree_item_icon,
                        }
                    };
                }
            }

            // Add environment label for connections
            if metadata.kind == TreeItemKind::Connection {
                if let Some(connection) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(metadata.connection_id))
                {
                    let env_type = connection.environment_type;
                    Some(
                        div()
                            .text_size(rems(0.55))
                            .font_family(cx.theme().mono_font_family.clone())
                            .px_2()
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
            }
        } else {
            None
        };

        // Update schema icons based on expansion state (check if it's a schema by looking at metadata)
        if let Some(metadata) = self.get_tree_item_metadata(&item.id) {
            if metadata.kind == TreeItemKind::Schema && entry.is_expanded() {
                // This is an expanded schema, change icon to FolderOpen
                tree_item_icon = TreeItemIcon {
                    icon: IconName::FolderOpen,
                    color: tree_item_icon.color,
                };
            }
        }

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
                    .child(Icon::new(tree_item_icon.icon).text_color(tree_item_icon.color))
                    .child(Label::new(item.label.clone()).text_sm())
                    .when_some(environment_label, |this, label| this.child(label))
                    .context_menu(self.build_context_menu(&item, weak_panel))
                    .when(entry.is_folder(), |this| {
                        this.child(if entry.is_expanded() {
                            IconName::ChevronDown.view(cx)
                        } else {
                            IconName::ChevronRight.view(cx)
                        })
                    }),
            )
            .on_click(cx.listener({
                let item = item.clone();
                move |this, _event: &ClickEvent, _window, cx| {
                    this.handle_tree_item_click(&item.id, cx);
                }
            }))
    }

    pub fn export_table_data(
        &mut self,
        connection_id: i64,
        _connection_name: String,
        database_name: String,
        schema_name: Option<String>,
        table_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(table_name) = table_name {
            // Create the export modal content
            let modal_content = cx.new(|cx| {
                crate::export_modal::ExportModal::new(
                    connection_id,
                    database_name,
                    schema_name,
                    table_name,
                    window,
                    cx,
                )
            });

            window.open_dialog(cx, move |dialog, _window, _cx| {
                let modal_clone = modal_content.clone();
                dialog
                    .title("Export Table Data")
                    .h(px(450.0))
                    .child(modal_content.clone())
                    .footer({
                        move |_ok, _cancel, _window, _cx| {
                            vec![
                                Button::new("export-cancel").label("Cancel").on_click(
                                    |_, window, cx| {
                                        window.close_dialog(cx);
                                    },
                                ),
                                Button::new("export-submit")
                                    .primary()
                                    .label("Export")
                                    .on_click({
                                        let modal_for_button = modal_clone.clone();
                                        move |_, window, cx| {
                                            modal_for_button.update(cx, |modal, cx| {
                                                modal.start_export(window, cx);
                                            });
                                            window.close_dialog(cx);
                                        }
                                    }),
                            ]
                        }
                    })
            })
        } else {
            tracing::error!("Cannot export: No table name provided");
        }
    }
}

impl Render for ConnectionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_2()
            .bg(cx.theme().sidebar_primary_foreground)
            .child(self.render_header_section(cx))
            .child(self.render_connections_tree(cx))
    }
}

impl EventEmitter<AppEvent> for ConnectionsPanel {}

// Add display_name method to ConnectionData
impl ConnectionData {
    pub fn display_name(&self) -> String {
        self.name.clone()
    }
}
