mod delegate;

pub use delegate::ConnectionsTreeDelegate;
use gpui_component::button::ButtonVariant;
use gpui_component::dialog::DialogButtonProps;

use crate::app_database::{AppDatabase, ConnectionData, EnvironmentType};
use crate::app_events::{AppEvent, TreeItemType};
use blanco_core::DatabaseService as DatabaseServiceTrait;
use blanco_ui::IconName;
use blanco_ui::tree::{Tree, TreeItem, TreeState};
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled,
    Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    label::Label,
};

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
    pub connections: Vec<ConnectionData>,
    selected_connection_id: Option<i64>,
    database_metadata: std::collections::HashMap<i64, DatabaseMetadata>, // Store metadata per connection
    tree_state: Entity<TreeState<ConnectionsTreeDelegate>>,
    pub loaded_connections: std::collections::HashSet<i64>,
    expanded_connections: std::collections::HashSet<i64>, // Track which connections are expanded
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
    pub loading: bool, // Whether this item is currently loading
}

/// Trait to convert metadata into CreateNewQueryTab events
pub trait CreateNewQueryTabParams {
    /// Convert this metadata into a CreateNewQueryTab event, if applicable
    fn create_new_query_tab_event(&self) -> Option<crate::app_events::AppEvent>;
}

impl CreateNewQueryTabParams for TreeItemMetadata {
    fn create_new_query_tab_event(&self) -> Option<crate::app_events::AppEvent> {
        match self.kind {
            TreeItemKind::Connection => None, // Connections don't create queries directly
            TreeItemKind::Database | TreeItemKind::Schema | TreeItemKind::Table => {
                Some(crate::app_events::AppEvent::CreateNewQueryTab {
                    connection_id: self.connection_id,
                    connection_name: self.connection_name.clone(),
                    database_name: self.database_name.clone().unwrap_or_default(),
                    schema_name: self.schema_name.clone(),
                    table_name: self.table_name.clone(),
                    environment_type: self.environment_type,
                })
            }
        }
    }
}

impl ConnectionsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Load initial connections
        let app_database = AppDatabase::global(cx);
        let connections = smol::block_on(async {
            match app_database.load_connections().await {
                Ok(connections) => connections,
                Err(e) => {
                    tracing::error!("Failed to load connections: {}", e);
                    vec![]
                }
            }
        });

        let database_metadata = std::collections::HashMap::new();
        let loaded_connections = std::collections::HashSet::new();
        let expanded_connections = std::collections::HashSet::new();

        // Create delegate with parent reference
        let panel_entity = cx.entity();
        let delegate = ConnectionsTreeDelegate::new(&panel_entity);

        let tree_state = cx.new(|cx| TreeState::new(delegate, cx));

        let panel = Self {
            connections,
            selected_connection_id: None,
            database_metadata,
            tree_state,
            loaded_connections,
            expanded_connections,
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

    /// Update tree items from connections data and update connection status directly
    fn update_tree_items(&mut self, cx: &mut Context<Self>) {
        let db_service = DatabaseService::global(cx).clone();

        // Get active connections and current tree state to update metadata
        cx.spawn(async move |this_handle, cx| {
            // Get active connection statuses from DatabaseService
            let connection_statuses = match db_service.get_active_connection_statuses().await {
                Ok(statuses) => statuses,
                Err(e) => {
                    tracing::error!("Failed to get connection statuses: {}", e);
                    return;
                }
            };

            // Update connection status directly in tree metadata
            let _ = this_handle.update(cx, |this, cx| {
                // Rebuild tree items with updated metadata
                let tree_items: Vec<TreeItem<TreeItemMetadata>> = this
                    .connections
                    .iter()
                    .map(|conn| {
                        this.build_connection_tree_item_with_expand(
                            conn,
                            this.expanded_connections.contains(&conn.id.unwrap_or(0)),
                            cx,
                        )
                    })
                    .collect();

                this.tree_state.update(cx, |state, cx| {
                    state.set_items(tree_items, cx);
                });

                // Update connection status directly in tree entries
                this.update_connection_status_in_tree(&connection_statuses, cx);

                cx.notify();
            });
        })
        .detach();
    }

    /// Update connection status directly in tree entries using ConnectionStatus
    fn update_connection_status_in_tree(
        &mut self,
        connection_statuses: &std::collections::HashMap<
            (i64, String),
            blanco_core::ConnectionStatus,
        >,
        cx: &mut Context<Self>,
    ) {
        self.tree_state.update(cx, |tree_state, cx| {
            // Check each entry and update its status
            for entry in tree_state.entries_mut() {
                let metadata = &entry.item.metadata;
                match metadata.kind {
                    TreeItemKind::Connection => {
                        // Connections are stored with "default" as the database name
                        let is_connected = connection_statuses
                            .get(&(metadata.connection_id, "default".to_string()))
                            .map(|status| status.is_connected)
                            .unwrap_or(false);
                        if is_connected {
                            entry.item.metadata.icon = TreeItemIcon {
                                icon: IconName::DatabaseConnected,
                                color: cx.theme().primary.into(),
                            };
                        } else {
                            entry.item.metadata.icon = TreeItemIcon {
                                icon: IconName::Database,
                                color: cx.theme().foreground.into(),
                            };
                        }
                    }
                    TreeItemKind::Database => {
                        if let Some(ref db_name) = metadata.database_name {
                            let is_connected = connection_statuses
                                .get(&(metadata.connection_id, db_name.clone()))
                                .map(|status| status.is_connected)
                                .unwrap_or(false);
                            if is_connected {
                                entry.item.metadata.icon = TreeItemIcon {
                                    icon: IconName::DatabaseConnected,
                                    color: cx.theme().primary.into(),
                                };
                            } else {
                                entry.item.metadata.icon = TreeItemIcon {
                                    icon: IconName::Database,
                                    color: cx.theme().foreground.into(),
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    /// Build a TreeItem for a connection including its children if loaded, with specified expand state
    fn build_connection_tree_item_with_expand(
        &self,
        connection: &ConnectionData,
        should_expand: bool,
        cx: &Context<Self>,
    ) -> TreeItem<TreeItemMetadata> {
        let connection_id = connection.id.unwrap_or(0);

        // Create metadata for the connection
        let connection_metadata = TreeItemMetadata {
            connection_id,
            connection_name: connection.display_name(),
            kind: TreeItemKind::Connection,
            database_name: None,
            schema_name: None,
            table_name: None,
            icon: self.get_connection_icon(connection_id, cx),
            environment_type: Some(connection.environment_type),
            loading: false,
        };

        // Use hierarchical key: "connection:123" for connections
        let base_item = TreeItem::new(
            format!("connection:{}", connection_id),
            connection.display_name(),
            connection_metadata,
        )
        .expanded(should_expand);

        // If this connection's metadata is loaded, add children
        if let Some(metadata) = self.database_metadata.get(&connection_id) {
            if metadata.supports_schemas {
                // PostgreSQL: connection -> databases -> schemas -> tables
                let database_items: Vec<TreeItem<TreeItemMetadata>> = metadata
                    .databases
                    .iter()
                    .map(|database| {
                        let database_key = format!("database:{}:{}", connection_id, database.name);

                        // Create metadata for the database
                        let database_metadata = TreeItemMetadata {
                            connection_id,
                            connection_name: connection.display_name(),
                            kind: TreeItemKind::Database,
                            database_name: Some(database.name.clone()),
                            schema_name: None,
                            table_name: None,
                            icon: TreeItemIcon {
                                icon: IconName::Database,
                                color: cx.theme().foreground.into(),
                            },
                            environment_type: Some(connection.environment_type),
                            loading: false,
                        };

                        let schema_items: Vec<TreeItem<TreeItemMetadata>> = database
                            .schemas
                            .iter()
                            .map(|schema| {
                                let schema_key = format!(
                                    "schema:{}:{}:{}",
                                    connection_id, database.name, schema.name
                                );

                                // Create metadata for the schema
                                let schema_metadata = TreeItemMetadata {
                                    connection_id,
                                    connection_name: connection.display_name(),
                                    kind: TreeItemKind::Schema,
                                    database_name: Some(database.name.clone()),
                                    schema_name: Some(schema.name.clone()),
                                    table_name: None,
                                    icon: TreeItemIcon {
                                        icon: IconName::Folder,
                                        color: cx.theme().yellow.into(),
                                    },
                                    environment_type: Some(connection.environment_type),
                                    loading: false,
                                };

                                let table_items: Vec<TreeItem<TreeItemMetadata>> = if schema
                                    .is_expanded
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

                                            // Create metadata for the table
                                            let table_metadata = TreeItemMetadata {
                                                connection_id,
                                                connection_name: connection.display_name(),
                                                kind: TreeItemKind::Table,
                                                database_name: Some(database.name.clone()),
                                                schema_name: Some(schema.name.clone()),
                                                table_name: Some(table.name.clone()),
                                                icon: TreeItemIcon {
                                                    icon: IconName::Sheet,
                                                    color: cx.theme().green.into(),
                                                },
                                                environment_type: Some(connection.environment_type),
                                                loading: false,
                                            };

                                            TreeItem::new(
                                                table_key,
                                                table.name.clone(),
                                                table_metadata,
                                            )
                                        })
                                        .collect()
                                } else {
                                    Vec::new() // Tables not loaded or collapsed
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
                // MySQL/SQLite: connection -> tables (no schema level for cleaner UI)
                let table_items: Vec<TreeItem<TreeItemMetadata>> = metadata
                    .schemas
                    .iter()
                    .flat_map(|schema| {
                        // For MySQL/SQLite, show tables directly under connection
                        schema
                            .tables
                            .iter()
                            .map(|table| {
                                let table_key = format!(
                                    "table:{}:{}:{}",
                                    connection_id, schema.name, table.name
                                );

                                // Create metadata for the table
                                let table_metadata = TreeItemMetadata {
                                    connection_id,
                                    connection_name: connection.display_name(),
                                    kind: TreeItemKind::Table,
                                    database_name: Some(schema.name.clone()), // Use schema.name as database_name for MySQL/SQLite
                                    schema_name: None, // MySQL/SQLite don't have schemas in the traditional sense
                                    table_name: Some(table.name.clone()),
                                    icon: TreeItemIcon {
                                        icon: IconName::Sheet,
                                        color: cx.theme().green.into(),
                                    },
                                    environment_type: Some(connection.environment_type),
                                    loading: false,
                                };

                                TreeItem::new(table_key, table.name.clone(), table_metadata)
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

    /// Find tree item metadata by searching through the tree entries
    fn find_tree_item_metadata(&self, item_id: &str, cx: &App) -> Option<TreeItemMetadata> {
        let entries = self.tree_state.read(cx).entries();
        for entry in entries {
            if entry.item().id == item_id {
                return Some(entry.item().metadata.clone());
            }
        }
        None
    }

    fn get_connection_icon(&self, _connection_id: i64, cx: &Context<Self>) -> TreeItemIcon {
        TreeItemIcon {
            icon: IconName::Database,
            color: cx.theme().foreground.into(),
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
                        // Clear loading state for this connection
                        let item_id = format!("connection:{}", connection_id);
                        this.set_item_loading(&item_id, false, cx);

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
                    // Clear loading state on error
                    let _ = this_handle.update(cx, |this, cx| {
                        let item_id = format!("connection:{}", connection_id);
                        this.set_item_loading(&item_id, false, cx);
                        cx.notify();
                    });
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
                        // Clear loading state for this database
                        let item_id = format!("database:{}:{}", connection_id, database_name);
                        this.set_item_loading(&item_id, false, cx);

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
                    // Clear loading state on error
                    let _ = this_handle.update(cx, |this, cx| {
                        let item_id = format!("database:{}:{}", connection_id, database_name);
                        this.set_item_loading(&item_id, false, cx);
                        cx.notify();
                    });
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
                            if let Some(database) = metadata.databases.iter_mut().find(|db| db.name == database_name)
                                && let Some(schema) = database.schemas.iter_mut().find(|s| s.name == schema_name) {
                                    schema.tables = tables;
                                    schema.is_expanded = true; // Mark as expanded
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
    pub fn handle_tree_item_click(
        &mut self,
        item_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Find the item in the tree and get its metadata directly
        if let Some(metadata) = self.find_tree_item_metadata(item_id, cx) {
            let connection_id = metadata.connection_id;
            match metadata.kind {
                TreeItemKind::Connection => {
                    // Check if already loaded, if not load first
                    if !self.loaded_connections.contains(&connection_id) {
                        tracing::debug!("Loading connection {} for first time", connection_id);
                        // Set loading state before starting async operation
                        self.set_item_loading(item_id, true, cx);
                        self.load_connection_children(connection_id, true, cx);
                    } else {
                        // Already loaded, toggle expand/collapse the tree
                        tracing::debug!("Toggling expansion for connection {}", connection_id);
                        self.toggle_connection_expansion(connection_id, window, cx);
                    }
                    cx.emit(AppEvent::TreeItemExpanded {
                        item_id: item_id.to_string(),
                        item_type: TreeItemType::Connection,
                        connection_id: Some(connection_id),
                    });
                }
                TreeItemKind::Database => {
                    if let Some(database_name) = &metadata.database_name
                        && let Some(connection_metadata) =
                            self.database_metadata.get(&connection_id)
                        && let Some(database) = connection_metadata
                            .databases
                            .iter()
                            .find(|db| db.name == *database_name)
                    {
                        if database.schemas.is_empty() {
                            tracing::debug!("Loading schemas for database: {}", database_name);
                            // Set loading state before starting async operation
                            self.set_item_loading(item_id, true, cx);
                            self.load_database_children(connection_id, database_name.clone(), cx);
                        } else {
                            // Already loaded, toggle expansion
                            tracing::debug!("Toggling expansion for database: {}", database_name);
                            self.toggle_database_expansion(
                                connection_id,
                                database_name,
                                window,
                                cx,
                            );
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
                        && let Some(connection_metadata) =
                            self.database_metadata.get(&connection_id)
                        && let Some(database) = connection_metadata
                            .databases
                            .iter()
                            .find(|db| db.name == *database_name)
                        && let Some(schema) =
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
    fn toggle_connection_expansion(
        &mut self,
        connection_id: i64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(metadata) = self.database_metadata.get_mut(&connection_id)
            && let Some(database) = metadata
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

    /// Toggle schema expansion in the tree
    fn toggle_schema_expansion(
        &mut self,
        connection_id: i64,
        database_name: &str,
        schema_name: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(metadata) = self.database_metadata.get_mut(&connection_id)
            && let Some(database) = metadata
                .databases
                .iter_mut()
                .find(|db| db.name == database_name)
            && let Some(schema) = database.schemas.iter_mut().find(|s| s.name == schema_name)
        {
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

    /// Disconnect and remove a connection
    pub fn disconnect_connection(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        tracing::info!("Disconnecting connection {}", connection_id);

        // Disconnect via DatabaseService
        let db_service = DatabaseService::global(cx).clone();

        cx.spawn(async move |this_handle, cx| {
            // Disconnect all databases for this connection
            if let Err(e) = db_service.disconnect(connection_id, None).await {
                tracing::error!("Failed to disconnect connection {}: {}", connection_id, e);
            }

            // Update the UI
            let _ = this_handle.update(cx, |this, cx| {
                this.loaded_connections.remove(&connection_id);
                this.expanded_connections.remove(&connection_id);
                this.update_tree_items(cx);

                // Emit connection lost event for any listeners
                cx.emit(AppEvent::ConnectionLost {
                    connection_id: Some(connection_id),
                    error: "Disconnected by user".to_string(),
                });

                cx.notify();
            });
        })
        .detach();
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
            .pt(px(6.))
            .pb(px(5.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Label::new("Connections")
                    .font_bold()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground),
            )
    }

    fn render_connections_tree(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        Tree::new(&self.tree_state)
    }

    #[allow(clippy::too_many_arguments)]
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

    /// Set loading state for a tree item by updating the tree entry directly
    pub fn set_item_loading(&mut self, item_id: &str, loading: bool, cx: &mut Context<Self>) {
        tracing::info!("Setting item loading {} = {}", item_id, loading);

        self.tree_state.update(cx, |tree_state, cx| {
            if let Some(entry) = tree_state.find_mut(item_id, cx) {
                entry.item.metadata.loading = loading;
            }
        });
    }

    /// Show a confirmation dialog and remove the connection if confirmed
    pub fn confirm_remove_connection(
        &mut self,
        connection_id: i64,
        connection_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .confirm()
                .child(format!(
                    "Are you sure you want to remove the connection \"{}\"?",
                    connection_name
                ))
                .button_props(
                    DialogButtonProps::default()
                        .cancel_text("No")
                        .cancel_variant(ButtonVariant::Secondary)
                        .ok_text("Yes")
                        .ok_variant(ButtonVariant::Danger),
                )
                .on_ok({
                    let this_handle = this_handle.clone();
                    move |_, _, cx| {
                        let this_handle = this_handle.clone();
                        let app_database = AppDatabase::global(cx).clone();
                        cx.spawn(async move |cx| {
                            if let Err(e) = app_database.delete_connection(connection_id).await {
                                tracing::error!("Failed to delete connection: {}", e);
                                return;
                            }

                            let _ = this_handle.update(cx, |this, cx| {
                                this.connections.retain(|c| c.id != Some(connection_id));
                                this.loaded_connections.remove(&connection_id);
                                this.expanded_connections.remove(&connection_id);
                                this.database_metadata.remove(&connection_id);
                                this.update_tree_items(cx);
                                cx.notify();
                            });
                        })
                        .detach();
                        true
                    }
                })
                .on_cancel(|_, _, _| true)
        });
    }
}

impl Render for ConnectionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
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
