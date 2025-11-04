use crate::connection_sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui::{
    div, App, AppContext, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Window,
};
use gpui_component::{
    button::Button, h_flex, v_flex, ActiveTheme, ContextModal as _, IconName as GCIconName, Side,
};
use std::collections::HashMap;
use std::sync::Arc;

use crate::app_events::AppEvent;
use crate::connection_modal::NewConnectionModal;
use crate::db_service::DbService;
use blanco_ui::IconName;
use log::info;

/// Simple schema node for sidebar display
#[derive(Clone, Debug)]
pub struct SchemaNode {
    pub name: String,
    pub expanded: bool,
    pub tables: Vec<String>,
}

impl SchemaNode {
    pub fn new(name: String) -> Self {
        let expanded = name == "public"; // Default expand public schema
        Self {
            name,
            expanded,
            tables: Vec::new(),
        }
    }
}

/// Unified connection information stored in the sidebar
#[derive(Clone)]
pub struct UnifiedConnectionInfo {
    pub connection: Arc<dyn blanco_core::Connection>,
    pub connection_string: String,
    pub expanded: bool,
    pub schemas: Vec<SchemaNode>,
    pub display_name: String,
    pub tables: Vec<String>, // For SQLite connections
}

pub struct ConnectionSidebar {
    focus_handle: FocusHandle,
    collapsed: bool,
    // Unified connections cache (connection_string -> connection info)
    unified_connections: HashMap<String, UnifiedConnectionInfo>,
}

impl ConnectionSidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut sidebar = Self {
            focus_handle: cx.focus_handle(),
            collapsed: false,
            unified_connections: HashMap::new(),
        };

        // Load real connections from database first
        sidebar.load_database_connections(cx);

        // Subscribe to connection events to refresh when connections are added
        sidebar.subscribe_to_connection_events(cx);

        // Debug: Log initial state
        log::info!("Sidebar initialized with unified connection system");

        // Note: We no longer add mock connections automatically
        // Mock connections can still be added manually for testing if needed

        sidebar
    }

    /// Subscribe to connection events to refresh when connections are added
    // TODO: Re-implement with GPUI EventEmitter system
    fn subscribe_to_connection_events(&mut self, _cx: &mut Context<Self>) {
        // This function will be re-implemented using GPUI's EventEmitter system
        // For now, the sidebar refreshes through other mechanisms
        log::info!(
            "Connection event subscription temporarily disabled during EventEmitter migration"
        );
    }

    /// Load real connections from the app database
    pub fn load_database_connections(&mut self, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();

        // Load connections asynchronously and update UI
        log::info!("Starting to load database connections...");
        cx.spawn(async move |sidebar_handle, mut cx| {
            let connections = match *app_db.read().await {
                Some(ref app_db) => match app_db.load_connections().await {
                    Ok(connections) => {
                        log::info!("Loaded {} connections from database", connections.len());
                        for conn in &connections {
                            log::info!("Processing connection: {} ({})", conn.name, conn.db_type);
                            log::info!("Connection details - name: '{}', db_type: '{}', db_path: {:?}, host: {:?}, port: {:?}, database: {:?}, username: {:?}",
                                         conn.name,
                                         conn.db_type,
                                         conn.database_path,
                                         conn.host,
                                         conn.port,
                                         conn.database_name,
                                         conn.username);
                        }
                        connections
                    }
                    Err(e) => {
                        log::error!("Failed to load connections from database: {}", e);
                        return;
                    }
                },
                None => {
                    log::warn!("App database not initialized");
                    return;
                }
            };

            if let Some(sidebar) = sidebar_handle.upgrade() {
                let _ = sidebar.update(cx, |sidebar, cx| {
                    // Process loaded connections and add them to unified connection system
                    for conn in &connections {
                        // Build connection string for unified system
                        let connection_string = if conn.db_type == "PostgreSQL" {
                            if let (Some(host), Some(port), Some(database), Some(username)) =
                                (&conn.host, conn.port, &conn.database_name, &conn.username) {

                                log::info!("Processing PostgreSQL connection: {} ({}:{}/{})",
                                            conn.name, host, port, database);

                                // Build PostgreSQL connection string
                                let built_conn_string = format!("postgresql://{}:{}@{}:{}/{}",
                                    username,
                                    conn.password.as_ref().unwrap_or(&"".to_string()),
                                    host, port, database);
                                log::info!("Built connection string for {}: {}", conn.name, built_conn_string);
                                Some(built_conn_string)
                            } else {
                                log::warn!("PostgreSQL connection missing required fields: {}", conn.name);
                                None
                            }
                        } else if conn.db_type == "SQLite" {
                            if let Some(database_path) = &conn.database_path {
                                log::info!("Processing SQLite connection: {} -> {}", conn.name, database_path);

                                // Build SQLite connection string
                                Some(format!("sqlite:{}", database_path))
                            } else {
                                log::warn!("SQLite connection has no database_path: {}", conn.name);
                                None
                            }
                        } else {
                            log::warn!("Unsupported connection type: {} for connection {}", conn.db_type, conn.name);
                            None
                        };

                        // Add to unified connections system
                        if let Some(conn_str) = connection_string {
                            let unified_manager = DbService::global(cx).unified_manager_handle();
                            let conn_name = conn.name.clone();
                            let conn_str_clone = conn_str.clone();

                            // Add connection to unified system
                            cx.spawn(async move |sidebar_handle, mut cx| {
                                if let Ok(connection) = unified_manager.read().await.get_or_create_connection(&conn_str_clone).await {
                                    if let Some(sidebar) = sidebar_handle.upgrade() {
                                        let _ = sidebar.update(cx, |sidebar, cx| {
                                            let connection_key = connection.get_connection_key_str();
                                            let unified_info = UnifiedConnectionInfo {
                                                connection: connection.clone(),
                                                connection_string: conn_str.clone(),
                                                expanded: false,
                                                schemas: Vec::new(),
                                                display_name: conn_name.clone(),
                                                tables: Vec::new(),
                                            };
                                            sidebar.unified_connections.insert(connection_key, unified_info);
                                            log::info!("Added connection to unified system: {} -> {}", conn_name, conn_str);
                                            cx.notify();
                                        });
                                    }
                                } else {
                                    log::error!("Failed to create unified connection for: {}", conn_name);
                                }
                            }).detach();
                        }
                    }

                    // Emit ConnectionsLoaded event to trigger tab restoration
                    cx.emit(crate::app_events::AppEvent::ConnectionsLoaded {
                        count: connections.len(),
                    });

                    cx.notify();
                });
            }
        }).detach();
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        cx.notify();
    }

    /// Load tables using the unified connection interface
    pub async fn load_unified_tables(
        &mut self,
        connection_string: &str,
        _cx: &mut Context<'_, Self>,
    ) -> Result<Vec<String>, anyhow::Error> {
        info!(
            "Loading tables using unified interface for: {}",
            connection_string
        );

        // Get or create connection using unified manager
        let db_service = DbService::global(_cx);
        let connection = db_service
            .get_or_create_unified_connection(connection_string)
            .await?;

        // Load tables using the unified trait
        let tables = connection.get_tables(None).await?;

        info!("Loaded {} tables using unified interface", tables.len());
        Ok(tables)
    }

    /// Load schemas using the unified connection interface
    pub async fn load_unified_schemas(
        &mut self,
        connection_string: &str,
        _cx: &mut Context<'_, Self>,
    ) -> Result<Vec<String>, anyhow::Error> {
        info!(
            "Loading schemas using unified interface for: {}",
            connection_string
        );

        // Get or create connection using unified manager
        let db_service = DbService::global(_cx);
        let connection = db_service
            .get_or_create_unified_connection(connection_string)
            .await?;

        // Load schemas using the unified trait
        let schemas = connection.get_schemas().await?;

        info!("Loaded {} schemas using unified interface", schemas.len());
        Ok(schemas)
    }

    /// Add a connection using the unified interface
    pub async fn add_unified_connection(
        &mut self,
        connection_string: &str,
        display_name: &str,
        _cx: &mut Context<'_, Self>,
    ) -> Result<(), anyhow::Error> {
        info!(
            "Adding unified connection: {} ({})",
            display_name, connection_string
        );

        // Create connection using unified manager
        let db_service = DbService::global(_cx);
        let connection = db_service
            .get_or_create_unified_connection(connection_string)
            .await?;

        // Cache the connection with proper info
        let connection_info = UnifiedConnectionInfo {
            connection: connection.clone(),
            connection_string: connection_string.to_string(),
            expanded: false,
            schemas: Vec::new(),
            display_name: display_name.to_string(),
            tables: Vec::new(),
        };
        self.unified_connections
            .insert(connection_string.to_string(), connection_info);

        // TODO: Add the connection to the appropriate legacy structure for rendering
        // For now, just log the success
        info!("Successfully added unified connection: {}", display_name);

        Ok(())
    }

    /// Load tables for a connection using unified interface
    pub fn load_tables_unified_async(
        &mut self,
        connection_string: &str,
        schema: Option<&str>,
        ui_update_callback: impl Fn(&mut Self, Vec<String>) + Send + Sync + 'static,
        error_callback: impl Fn(&mut Self, String) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        let unified_manager = DbService::global(cx).unified_manager_handle();
        let connection_string_clone = connection_string.to_string();
        let connection_string_for_logging = connection_string_clone.clone();
        let schema_clone = schema.map(|s| s.to_string());

        cx.spawn(async move |sidebar_handle, mut cx| {
            // Get connection and tables in sequence using unified interface
            match unified_manager
                .read()
                .await
                .get_or_create_connection(&connection_string_clone)
                .await
            {
                Ok(connection) => match connection.get_tables(schema_clone.as_deref()).await {
                    Ok(tables) => {
                        if let Some(sidebar) = sidebar_handle.upgrade() {
                            let _ = sidebar.update(cx, |sidebar, cx| {
                                ui_update_callback(sidebar, tables);
                                cx.notify();
                            });
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to load tables for connection '{}': {}",
                            connection_string_for_logging,
                            e
                        );
                        if let Some(sidebar) = sidebar_handle.upgrade() {
                            let _ = sidebar.update(cx, |sidebar, cx| {
                                error_callback(sidebar, format!("Error loading tables: {}", e));
                                cx.notify();
                            });
                        }
                    }
                },
                Err(e) => {
                    log::error!(
                        "Failed to get connection for '{}': {}",
                        connection_string_for_logging,
                        e
                    );
                }
            }
        })
        .detach();
    }

    /// Load schemas for a connection using unified interface
    pub fn load_schemas_unified_async(
        &mut self,
        connection_string: &str,
        ui_update_callback: impl Fn(&mut Self, Vec<String>) + Send + Sync + 'static,
        error_callback: impl Fn(&mut Self, String) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        let unified_manager = DbService::global(cx).unified_manager_handle();
        let connection_string_clone = connection_string.to_string();
        let connection_string_for_logging = connection_string_clone.clone();

        cx.spawn(async move |sidebar_handle, mut cx| {
            // Get connection and schemas in sequence using unified interface
            match unified_manager
                .read()
                .await
                .get_or_create_connection(&connection_string_clone)
                .await
            {
                Ok(connection) => match connection.get_schemas().await {
                    Ok(schemas) => {
                        if let Some(sidebar) = sidebar_handle.upgrade() {
                            let _ = sidebar.update(cx, |sidebar, cx| {
                                ui_update_callback(sidebar, schemas);
                                cx.notify();
                            });
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to load schemas for connection '{}': {}",
                            connection_string_for_logging,
                            e
                        );
                        if let Some(sidebar) = sidebar_handle.upgrade() {
                            let _ = sidebar.update(cx, |sidebar, cx| {
                                error_callback(sidebar, format!("Error loading schemas: {}", e));
                                cx.notify();
                            });
                        }
                    }
                },
                Err(e) => {
                    log::error!(
                        "Failed to get connection for schemas '{}': {}",
                        connection_string_for_logging,
                        e
                    );
                }
            }
        })
        .detach();
    }

    /// Toggle expansion of a unified connection and load schemas/tables as needed
    pub fn toggle_unified_connection(&mut self, connection_key: &str, cx: &mut Context<Self>) {
        if let Some(connection_info) = self.unified_connections.get_mut(connection_key) {
            let was_expanded = connection_info.expanded;
            connection_info.expanded = !connection_info.expanded;

            if !was_expanded && connection_info.expanded {
                // Connection is being expanded - load schemas/tables
                let connection_string = connection_info.connection_string.clone();
                let connection_key_clone = connection_key.to_string();
                let connection_key_for_schemas_callbacks = connection_key_clone.clone();
                let connection_key_for_schemas_logging =
                    connection_key_for_schemas_callbacks.clone();
                if connection_info.connection.supports_schemas() {
                    // Load schemas for connections that support them (e.g., PostgreSQL)
                    self.load_schemas_unified_async(
                        &connection_string,
                        move |sidebar, schemas| {
                            // Success callback - update schemas
                            if let Some(conn_info) =
                                sidebar.unified_connections.get_mut(&connection_key_clone)
                            {
                                conn_info.schemas.clear();
                                for schema_name in &schemas {
                                    let mut schema = SchemaNode::new(schema_name.clone());
                                    // Expand "public" schema by default
                                    schema.expanded = schema_name == "public";
                                    conn_info.schemas.push(schema);
                                }
                            }
                            log::info!(
                                "Loaded {} schemas for PostgreSQL connection: {}",
                                schemas.len(),
                                connection_key_for_schemas_logging
                            );
                        },
                        move |sidebar, error_msg| {
                            // Error callback - show error
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_schemas_callbacks)
                            {
                                conn_info.schemas.clear();
                                let mut error_schema = SchemaNode::new("Error".to_string());
                                error_schema.tables = vec![error_msg.clone()];
                                conn_info.schemas.push(error_schema);
                            }
                            log::error!(
                                "Failed to load schemas for connection '{}': {}",
                                connection_key_for_schemas_callbacks,
                                error_msg
                            );
                        },
                        cx,
                    );
                } else {
                    // Load tables for flat structure connections (e.g., SQLite)
                    let connection_key_for_tables_callbacks = connection_key_clone.clone();
                    let connection_key_for_tables_logging =
                        connection_key_for_tables_callbacks.clone();
                    let connection_key_for_tables_error =
                        connection_key_for_tables_callbacks.clone();
                    self.load_tables_unified_async(
                        &connection_string,
                        None,
                        move |sidebar, tables| {
                            // Success callback - update tables
                            let tables_count = tables.len();
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_tables_callbacks)
                            {
                                conn_info.tables = tables;
                            }
                            log::info!(
                                "Loaded {} tables for connection: {}",
                                tables_count,
                                connection_key_for_tables_logging
                            );
                        },
                        move |sidebar, error_msg| {
                            // Error callback - show error
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_tables_error)
                            {
                                conn_info.tables = vec![format!("Error: {}", error_msg)];
                            }
                            log::error!(
                                "Failed to load tables for connection '{}': {}",
                                connection_key_for_tables_error,
                                error_msg
                            );
                        },
                        cx,
                    );
                }
            }

            cx.notify();
        }
    }

    /// Toggle expansion of a unified schema and load tables as needed
    pub fn toggle_unified_schema(
        &mut self,
        connection_key: &str,
        schema_name: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(connection_info) = self.unified_connections.get_mut(connection_key) {
            if let Some(schema) = connection_info
                .schemas
                .iter_mut()
                .find(|s| s.name == schema_name)
            {
                let was_expanded = schema.expanded;
                schema.expanded = !schema.expanded;

                if !was_expanded && schema.expanded && schema.tables.is_empty() {
                    // Schema is being expanded - load tables
                    let connection_string = connection_info.connection_string.clone();
                    let connection_key_for_schema_tables_callbacks = connection_key.to_string();
                    let connection_key_for_schema_tables_logging =
                        connection_key_for_schema_tables_callbacks.clone();
                    let connection_key_for_schema_tables_error =
                        connection_key_for_schema_tables_callbacks.clone();
                    let schema_name_for_callbacks = schema_name.to_string();
                    let schema_name_for_logging = schema_name_for_callbacks.clone();
                    let schema_name_for_error = schema_name_for_callbacks.clone();

                    self.load_tables_unified_async(
                        &connection_string,
                        Some(schema_name),
                        move |sidebar, tables| {
                            // Success callback - update tables
                            let tables_count = tables.len();
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_schema_tables_callbacks)
                            {
                                if let Some(schema) = conn_info
                                    .schemas
                                    .iter_mut()
                                    .find(|s| s.name == schema_name_for_callbacks)
                                {
                                    schema.tables = tables;
                                }
                            }
                            log::info!(
                                "Loaded {} tables for PostgreSQL schema '{}': {}",
                                tables_count,
                                schema_name_for_logging,
                                connection_key_for_schema_tables_logging
                            );
                        },
                        move |sidebar, error_msg| {
                            // Error callback - show error
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_schema_tables_error)
                            {
                                if let Some(schema) = conn_info
                                    .schemas
                                    .iter_mut()
                                    .find(|s| s.name == schema_name_for_error)
                                {
                                    schema.tables = vec![format!("Error: {}", error_msg)];
                                }
                            }
                            log::error!(
                                "Failed to load tables for PostgreSQL schema '{}': {}",
                                schema_name_for_error,
                                error_msg
                            );
                        },
                        cx,
                    );
                }

                cx.notify();
            }
        }
    }
}

impl EventEmitter<AppEvent> for ConnectionSidebar {}
impl Focusable for ConnectionSidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConnectionSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().h_full().track_focus(&self.focus_handle).child(
            div().flex_1().min_h_0().overflow_hidden().child(
                Sidebar::new(Side::Left).collapsed(self.collapsed).child(
                    SidebarGroup::new("Databases").child(SidebarMenu::new().children({
                        let mut menu_items = Vec::new();

                        // Add unified connections
                        for (connection_key, connection_info) in &self.unified_connections {
                            let connection_key_clone = connection_key.clone();
                            let display_name = connection_info.display_name.clone();
                            let _connection_string = connection_info.connection_string.clone();
                            let was_expanded = connection_info.expanded;
                            let _connection_type = connection_info.connection.get_connection_type();

                            // Create menu item for connection
                            let menu_item =
                                SidebarMenuItem::new(SharedString::from(display_name.clone()))
                                    .icon(match connection_info.connection.get_icon_name() {
                                        blanco_core::IconName::Sqlite => IconName::Sqlite,
                                        blanco_core::IconName::Postgres => IconName::Postgresql,
                                        blanco_core::IconName::Database => IconName::SquareTerminal,
                                        blanco_core::IconName::Table => IconName::SquareTerminal,
                                        blanco_core::IconName::Column => IconName::SquareTerminal,
                                        _ => IconName::SquareTerminal,
                                    })
                                    .active(was_expanded)
                                    .id(("unified-connection", connection_key.len() as u64))
                                    .context_menu({
                                        let display_name_for_menu = display_name.clone();
                                        let connection_key_for_menu = connection_key_clone.clone();
                                        let connection_string_for_menu = _connection_string.clone();
                                        move |menu, _window, _cx| {
                                            log::info!(
                                                "Creating context menu for unified connection: {}",
                                                display_name_for_menu
                                            );
                                            menu.menu(
                                                "New Query",
                                                Box::new(
                                                    crate::app::NewQueryForUnifiedConnection {
                                                        display_name: display_name_for_menu.clone(),
                                                        connection_key: connection_key_for_menu
                                                            .clone(),
                                                        connection_string:
                                                            connection_string_for_menu.clone(),
                                                    },
                                                ),
                                            )
                                        }
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        log::info!(
                                            "Clicked on unified connection: {}",
                                            display_name
                                        );
                                        this.toggle_unified_connection(&connection_key_clone, cx);
                                    }));

                            menu_items.push(menu_item);

                            // Add child items (schemas/tables) if expanded
                            if was_expanded {
                                if connection_info.connection.supports_schemas() {
                                    // Add schemas for connections that support them (e.g., PostgreSQL)
                                    for schema in &connection_info.schemas {
                                        let schema_name = schema.name.clone();
                                        let schema_expanded = schema.expanded;
                                        let schema_key =
                                            format!("{}:{}", connection_key, schema_name);

                                        let schema_item = SidebarMenuItem::new(SharedString::from(
                                            schema_name.clone(),
                                        ))
                                        .icon(IconName::Folder)
                                        .active(schema_expanded)
                                        .id(("unified-schema", schema_key.len() as u64))
                                        .context_menu({
                                            let schema_name_for_menu = schema_name.clone();
                                            let connection_key_for_menu = connection_key.clone();
                                            move |menu, _window, _cx| {
                                                menu.menu(
                                                    "New Query",
                                                    Box::new(
                                                        crate::app::NewQueryForUnifiedSchema {
                                                            connection_key: connection_key_for_menu
                                                                .clone(),
                                                            schema_name: schema_name_for_menu
                                                                .clone(),
                                                        },
                                                    ),
                                                )
                                            }
                                        })
                                        .on_click(cx.listener({
                                            let connection_key_for_click = connection_key.clone();
                                            let schema_name_for_click = schema_name.clone();
                                            move |this, _, _, cx| {
                                                log::info!(
                                                    "Clicked on unified schema: {}",
                                                    schema_name_for_click
                                                );
                                                this.toggle_unified_schema(
                                                    &connection_key_for_click,
                                                    &schema_name_for_click,
                                                    cx,
                                                );
                                            }
                                        }));

                                        menu_items.push(schema_item);

                                        // Add tables if schema is expanded
                                        if schema_expanded && !schema.tables.is_empty() {
                                            for table in &schema.tables {
                                                let table_item = SidebarMenuItem::new(
                                                    SharedString::from(table.clone()),
                                                )
                                                .icon(IconName::Folder)
                                                .id((
                                                    "unified-table",
                                                    format!(
                                                        "{}:{}:{}",
                                                        connection_key, schema_name, table
                                                    )
                                                    .len()
                                                        as u64,
                                                ));

                                                menu_items.push(table_item);
                                            }
                                        }
                                    }
                                } else {
                                    // Flat structure - show tables directly (e.g., SQLite)
                                    for table in &connection_info.tables {
                                        let table_item =
                                            SidebarMenuItem::new(SharedString::from(table.clone()))
                                                .icon(IconName::Folder)
                                                .id((
                                                    "unified-table",
                                                    format!("{}:{}", connection_key, table).len()
                                                        as u64,
                                                ));

                                        menu_items.push(table_item);
                                    }
                                }
                            }
                        }

                        menu_items
                    })),
                ),
            ),
        )
    }
}
