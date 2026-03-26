use database::DatabaseService;
use gpui::Context;

use crate::result_ext::ResultExt;

use super::ConnectionsPanel;

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
    pub _schema: Option<String>,
    pub item_type: DatabaseItemType,
}

/// Type of database item (table, view, or materialized view)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseItemType {
    Table,
    View,
    MaterializedView,
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
    pub _connection_id: Option<i64>,
    pub databases: Vec<Database>, // For PostgreSQL: database -> schema -> table
    pub schemas: Vec<DatabaseSchema>, // For SQLite: direct schema -> table
    pub supports_schemas: bool,
}

impl ConnectionsPanel {
    /// Load connection children (databases for PostgreSQL, schemas for SQLite) on demand
    pub(super) fn load_connection_children(
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
                            _connection_id: Some(connection_id),
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

                        // Load tables and views for all schemas (SQLite needs this for proper tree display)
                        let mut schema_tables: Vec<(String, Vec<DatabaseTable>)> = Vec::new();
                        for schema_name in &schemas {
                            let mut all_items: Vec<DatabaseTable> = Vec::new();

                            // Load tables
                            if let Ok(table_list) = connection.get_tables(Some(schema_name)).await {
                                for table_name in table_list {
                                    all_items.push(DatabaseTable {
                                        name: table_name,
                                        _schema: Some(schema_name.clone()),
                                        item_type: DatabaseItemType::Table,
                                    });
                                }
                            }

                            // Load views
                            if let Ok(view_list) = connection.get_views(Some(schema_name)).await {
                                for view_name in view_list {
                                    all_items.push(DatabaseTable {
                                        name: view_name,
                                        _schema: Some(schema_name.clone()),
                                        item_type: DatabaseItemType::View,
                                    });
                                }
                            }

                            // Sort all items alphabetically by name
                            all_items.sort_by(|a, b| a.name.cmp(&b.name));

                            schema_tables.push((schema_name.clone(), all_items));
                        }

                        DatabaseMetadata {
                            _connection_id: Some(connection_id),
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
                    this_handle
                        .update(cx, |this, cx| {
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
                        })
                        .log_err();
                }
                Err(e) => {
                    tracing::error!("Failed to get connection {}: {}", connection_id, e);
                    // Clear loading state on error
                    this_handle
                        .update(cx, |this, cx| {
                            let item_id = format!("connection:{}", connection_id);
                            this.set_item_loading(&item_id, false, cx);
                            cx.notify();
                        })
                        .log_err();
                }
            }
        })
        .detach();
    }

    /// Load schemas for a specific database (PostgreSQL only)
    pub(super) fn load_database_children(
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

                    // Load tables and views for all schemas (without lazy loading for now)
                    let mut schema_tables: Vec<(String, Vec<DatabaseTable>)> = Vec::new();
                    for schema_name in &schemas {
                        let all_items: Vec<DatabaseTable> = Vec::new();

                        /*
                        Don't perform any initial loading here - load on schema expand instead
                        // Load tables
                        if let Ok(table_list) = connection.get_tables(Some(schema_name)).await {
                            for table_name in table_list {
                                all_items.push(DatabaseTable {
                                    name: table_name,
                                    _schema: Some(schema_name.clone()),
                                    item_type: DatabaseItemType::Table,
                                });
                            }
                        }

                        // Load views
                        if let Ok(view_list) = connection.get_views(Some(schema_name)).await {
                            for view_name in view_list {
                                all_items.push(DatabaseTable {
                                    name: view_name,
                                    _schema: Some(schema_name.clone()),
                                    item_type: DatabaseItemType::View,
                                });
                            }
                        }

                        // Sort all items alphabetically by name
                        all_items.sort_by(|a, b| a.name.cmp(&b.name));
                        */

                        schema_tables.push((schema_name.clone(), all_items));
                    }

                    // Update the panel with loaded schemas
                    this_handle
                        .update(cx, |this, cx| {
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

                                this.update_tree_items(cx);
                                cx.notify();
                            }
                        })
                        .log_err();
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to get connection for database {} on connection {}: {}",
                        database_name,
                        connection_id,
                        e
                    );
                    // Clear loading state on error
                    this_handle
                        .update(cx, |this, cx| {
                            let item_id = format!("database:{}:{}", connection_id, database_name);
                            this.set_item_loading(&item_id, false, cx);
                            cx.notify();
                        })
                        .log_err();
                }
            }
        })
        .detach();
    }

    /// Load tables for a specific schema (PostgreSQL only)
    pub(super) fn load_schema_children(
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
                    tracing::debug!("Loading tables, views for schema: {} in database: {} on connection {}", schema_name, database_name, connection_id);

                    // Load tables, views, and materialized views
                    let tables_result = connection.get_tables(Some(&schema_name)).await;
                    let views_result = connection.get_views(Some(&schema_name)).await;
                    let matviews_result = connection.get_materialized_views(Some(&schema_name)).await;

                    let mut all_items: Vec<DatabaseTable> = Vec::new();

                    // Process tables
                    if let Ok(table_list) = tables_result {
                        let table_count: usize = table_list.len();
                        tracing::info!(
                            "Loaded {} tables for schema {} in database {} on connection {}",
                            table_count,
                            schema_name,
                            database_name,
                            connection_id
                        );
                        for table_name in table_list {
                            all_items.push(DatabaseTable {
                                name: table_name,
                                _schema: Some(schema_name.clone()),
                                item_type: DatabaseItemType::Table,
                            });
                        }
                    }

                    // Process views
                    if let Ok(view_list) = views_result {
                        let view_count: usize = view_list.len();
                        tracing::info!(
                            "Loaded {} views for schema {} in database {} on connection {}",
                            view_count,
                            schema_name,
                            database_name,
                            connection_id
                        );
                        for view_name in view_list {
                            all_items.push(DatabaseTable {
                                name: view_name,
                                _schema: Some(schema_name.clone()),
                                item_type: DatabaseItemType::View,
                            });
                        }
                    }

                    // Process materialized views
                    if let Ok(matview_list) = matviews_result {
                        let matview_count: usize = matview_list.len();
                        tracing::info!(
                            "Loaded {} materialized views for schema {} in database {} on connection {}",
                            matview_count,
                            schema_name,
                            database_name,
                            connection_id
                        );
                        for matview_name in matview_list {
                            all_items.push(DatabaseTable {
                                name: matview_name,
                                _schema: Some(schema_name.clone()),
                                item_type: DatabaseItemType::MaterializedView,
                            });
                        }
                    }

                    // Sort all items alphabetically by name
                    all_items.sort_by(|a, b| a.name.cmp(&b.name));

                    // Update the panel with loaded items
                    this_handle.update(cx, |this, cx| {
                        if let Some(metadata) = this.database_metadata.get_mut(&connection_id) {
                            // Find the database and schema, then update its tables
                            if let Some(database) = metadata.databases.iter_mut().find(|db| db.name == database_name)
                                && let Some(schema) = database.schemas.iter_mut().find(|s| s.name == schema_name) {
                                    schema.tables = all_items;
                                    schema.is_expanded = true; // Mark as expanded
                                }

                            // Rebuild tree to show loaded tables
                            this.update_tree_items(cx);
                            cx.notify();
                        }
                    }).log_err();
                }
                Err(e) => {
                    tracing::error!("Failed to get connection for schema {} in database {} on connection {}: {}", schema_name, database_name, connection_id, e);
                }
            }
        })
        .detach();
    }
}
