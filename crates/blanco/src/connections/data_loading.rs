use anyhow::Context as _;
use database::DatabaseService;
use gpui::Context;

use crate::result_ext::ResultExt;
use crate::status_bar::{ActivityReporter, ActivityResult};

use super::ConnectionsPanel;

/// Populate `DatabaseTable::relative_size` for items in a single schema using
/// linear normalization.
pub(super) fn compute_relative_sizes(items: &mut [DatabaseTable]) {
    let max = items
        .iter()
        .filter_map(|item| item.size_bytes)
        .max()
        .unwrap_or(0);
    if max == 0 {
        return;
    }
    for item in items.iter_mut() {
        if let Some(size) = item.size_bytes {
            item.relative_size = Some((size as f64 / max as f64) as f32);
        }
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
    pub _schema: Option<String>,
    pub item_type: DatabaseItemType,
    /// Physical size in bytes as reported by the database. `None` for object
    /// types that don't have a meaningful size (views, functions, ...) or for
    /// backends that don't report size.
    pub size_bytes: Option<u64>,
    /// Size normalized against siblings within the same schema, on a `log2`
    /// scale in `[0.0, 1.0]`. `None` when no siblings reported a size.
    pub relative_size: Option<f32>,
}

/// Type of database item (table, view, materialized view, or stored
/// routine/trigger). Routines and triggers share the same tree level as
/// tables/views; an icon distinguishes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseItemType {
    Table,
    View,
    MaterializedView,
    Procedure,
    Function,
    Trigger,
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

        let connection_label = self
            .connections
            .iter()
            .find(|connection| connection.id == Some(connection_id))
            .map(|connection| connection.name.clone())
            .unwrap_or_else(|| format!("connection {connection_id}"));
        let activity =
            ActivityReporter::global(cx).begin(format!("{connection_label}: listing schema"));

        cx.spawn(async move |this_handle, cx| {
            let activity = activity;

            // Build the metadata, treating a failed top-level listing (databases
            // for schema-aware backends, schemas otherwise) as a connection
            // failure. Lazy backends like PostgreSQL only open the SSH tunnel
            // eagerly, so bad credentials don't surface from
            // `get_or_create_connection`, only from the first real query here.
            let metadata_result: anyhow::Result<DatabaseMetadata> = async {
                let connection = db_service
                    .get_or_create_connection(connection_id, None)
                    .await?;
                tracing::debug!("Connected to database: {}", connection.get_display_name());

                // Validate the connection up front. For lazy backends like
                // PostgreSQL `get_or_create_connection` only opens the SSH tunnel,
                // so bad credentials would otherwise surface as a slow, opaque
                // pool timeout on the first listing query below.
                connection.ping().await?;

                // Check if the connection supports schemas
                let supports_schemas = connection.supports_schemas();

                if supports_schemas {
                    // For PostgreSQL: Load databases only (schemas loaded lazily)
                    let databases = connection
                        .get_databases()
                        .await
                        .context("Failed to list databases")?;
                    tracing::info!(
                        "Connection {} supports schemas. Loaded {} databases: {:?}",
                        connection_id,
                        databases.len(),
                        databases
                    );

                    Ok(DatabaseMetadata {
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
                    })
                } else {
                    // For SQLite: Load schemas and their tables (no databases level)
                    let schemas = connection
                        .get_schemas()
                        .await
                        .context("Failed to list schemas")?;
                    tracing::info!(
                        "Connection {} loaded {} schemas: {:?}",
                        connection_id,
                        schemas.len(),
                        schemas
                    );

                    // Load tables, views, and routines for all schemas
                    // (SQLite/MySQL need this for proper tree display).
                    let mut schema_tables: Vec<(String, Vec<DatabaseTable>)> = Vec::new();
                    for schema_name in &schemas {
                        let mut all_items: Vec<DatabaseTable> = Vec::new();
                        let sizes = connection
                            .get_object_sizes(Some(schema_name))
                            .await
                            .unwrap_or_default();
                        let push = |items: &mut Vec<DatabaseTable>,
                                    v: Vec<String>,
                                    kind: DatabaseItemType,
                                    sizes: &std::collections::HashMap<String, u64>| {
                            for n in v {
                                let size_bytes = if matches!(
                                    kind,
                                    DatabaseItemType::Table | DatabaseItemType::MaterializedView
                                ) {
                                    sizes.get(&n).copied()
                                } else {
                                    None
                                };
                                items.push(DatabaseTable {
                                    name: n,
                                    _schema: Some(schema_name.clone()),
                                    item_type: kind,
                                    size_bytes,
                                    relative_size: None,
                                });
                            }
                        };
                        if let Ok(v) = connection.get_tables(Some(schema_name)).await {
                            push(&mut all_items, v, DatabaseItemType::Table, &sizes);
                        }
                        if let Ok(v) = connection.get_views(Some(schema_name)).await {
                            push(&mut all_items, v, DatabaseItemType::View, &sizes);
                        }
                        if let Ok(v) = connection.list_procedures(Some(schema_name)).await {
                            push(&mut all_items, v, DatabaseItemType::Procedure, &sizes);
                        }
                        if let Ok(v) = connection.list_functions(Some(schema_name)).await {
                            push(&mut all_items, v, DatabaseItemType::Function, &sizes);
                        }
                        if let Ok(v) = connection.list_triggers(Some(schema_name)).await {
                            push(&mut all_items, v, DatabaseItemType::Trigger, &sizes);
                        }

                        // Sort all items alphabetically by name
                        all_items.sort_by(|a, b| a.name.cmp(&b.name));

                        compute_relative_sizes(&mut all_items);

                        schema_tables.push((schema_name.clone(), all_items));
                    }

                    Ok(DatabaseMetadata {
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
                    })
                }
            }
            .await;

            match metadata_result {
                Ok(metadata) => {
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
                    tracing::error!("Failed to load connection {}: {:#}", connection_id, e);

                    // Evict the half-open connection so it doesn't linger as
                    // "connected" in the sidebar and so the next attempt
                    // reconnects (and tears down its SSH tunnel if unused).
                    db_service.disconnect(connection_id, None).await.log_err();

                    // Surface the failure in the status bar rather than silently
                    // degrading to an empty tree.
                    activity.finish(ActivityResult::Err(
                        format!("{connection_label}: {e}").into(),
                    ));

                    this_handle
                        .update(cx, |this, cx| {
                            let item_id = format!("connection:{}", connection_id);
                            this.set_item_loading(&item_id, false, cx);
                            // Drop any state a racing connect may have recorded so
                            // the tree rebuild reflects the disconnected state.
                            this.loaded_connections.remove(&connection_id);
                            this.expanded_connections.remove(&connection_id);
                            this.update_tree_items(cx);
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

                    // Register each schema with an empty table list. Tables and
                    // views are loaded lazily when the user expands a schema, so
                    // we avoid introspecting every schema up front.
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

                    // Load tables, views, materialized views, and routines.
                    let tables_result = connection.get_tables(Some(&schema_name)).await;
                    let views_result = connection.get_views(Some(&schema_name)).await;
                    let matviews_result = connection.get_materialized_views(Some(&schema_name)).await;
                    let procs_result = connection.list_procedures(Some(&schema_name)).await;
                    let funcs_result = connection.list_functions(Some(&schema_name)).await;
                    let triggers_result = connection.list_triggers(Some(&schema_name)).await;

                    let sizes = connection
                        .get_object_sizes(Some(&schema_name))
                        .await
                        .unwrap_or_default();

                    let mut all_items: Vec<DatabaseTable> = Vec::new();
                    let push_named = |items: &mut Vec<DatabaseTable>,
                                      names: Vec<String>,
                                      kind: DatabaseItemType,
                                      sizes: &std::collections::HashMap<String, u64>| {
                        for n in names {
                            let size_bytes = if matches!(
                                kind,
                                DatabaseItemType::Table | DatabaseItemType::MaterializedView
                            ) {
                                sizes.get(&n).copied()
                            } else {
                                None
                            };
                            items.push(DatabaseTable {
                                name: n,
                                _schema: Some(schema_name.clone()),
                                item_type: kind,
                                size_bytes,
                                relative_size: None,
                            });
                        }
                    };
                    if let Ok(v) = tables_result {
                        push_named(&mut all_items, v, DatabaseItemType::Table, &sizes);
                    }
                    if let Ok(v) = views_result {
                        push_named(&mut all_items, v, DatabaseItemType::View, &sizes);
                    }
                    if let Ok(v) = matviews_result {
                        push_named(&mut all_items, v, DatabaseItemType::MaterializedView, &sizes);
                    }
                    if let Ok(v) = procs_result {
                        push_named(&mut all_items, v, DatabaseItemType::Procedure, &sizes);
                    }
                    if let Ok(v) = funcs_result {
                        push_named(&mut all_items, v, DatabaseItemType::Function, &sizes);
                    }
                    if let Ok(v) = triggers_result {
                        push_named(&mut all_items, v, DatabaseItemType::Trigger, &sizes);
                    }

                    // Sort all items alphabetically by name
                    all_items.sort_by(|a, b| a.name.cmp(&b.name));

                    compute_relative_sizes(&mut all_items);

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
