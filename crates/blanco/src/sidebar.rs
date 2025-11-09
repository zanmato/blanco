use crate::connection_sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui::{
    div, App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Window,
};
use gpui_component::{
    v_flex, Side,
};
use std::collections::HashMap;
use std::sync::Arc;

use crate::app_events::AppEvent;
use crate::db_service::DbService;
use blanco_ui::IconName;
use log::info;

/// Database node for sidebar display
#[derive(Clone, Debug)]
pub struct DatabaseNode {
    pub name: String,
    pub expanded: bool,
    pub schemas: Vec<SchemaNode>,
}

/// Simple schema node for sidebar display
#[derive(Clone, Debug)]
pub struct SchemaNode {
    pub name: String,
    pub expanded: bool,
    pub tables: Vec<String>,
}

impl DatabaseNode {
    pub fn new(name: String) -> Self {
        Self {
            name,
            expanded: false,
            schemas: Vec::new(),
        }
    }
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
    pub connection_id: i64,
    pub expanded: bool,
    pub databases: Vec<DatabaseNode>,
    pub display_name: String,
    pub default_database: Option<String>,
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
        cx.spawn(async move |sidebar_handle, cx| {
            let connections = match *app_db.read().await {
                Some(ref app_db) => match app_db.load_connections().await {
                    Ok(connections) => {
                        log::info!("Loaded {} connections from database", connections.len());
                        for conn in &connections {
                            log::info!("Processing connection: {} ({})", conn.name, conn.db_type);
                            log::info!("Connection details - id: '{}', name: '{}', db_type: '{}', db_path: {:?}, host: {:?}, port: {:?}, database: {:?}, username: {:?}",
                                        conn.id.unwrap_or(0),
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
                let _ = sidebar.update(cx, |_sidebar, cx| {
                    // Process loaded connections and add them to unified connection system
                    for conn in &connections {
                        // Add to unified connections system
                        if let Some(conn_id) = conn.id {
                            let db_service = DbService::global(cx).clone();
                            let conn_name = conn.name.clone();

                            // Add connection to unified system
                            cx.spawn(async move |sidebar_handle, cx| {
                                match db_service.get_or_create_connection(conn_id).await {
                                    Ok(connection) => {
                                        if let Some(sidebar) = sidebar_handle.upgrade() {
                                            let _ = sidebar.update(cx, |sidebar, cx| {
                                                let connection_key = connection.get_connection_key_str();
                                                let unified_info = UnifiedConnectionInfo {
                                                    connection: connection.clone(),
                                                    connection_id: conn_id,
                                                    expanded: false,
                                                    databases: Vec::new(),
                                                    display_name: conn_name.clone(),
                                                    default_database: None,
                                                };
                                                sidebar.unified_connections.insert(connection_key, unified_info);
                                                log::info!("Added connection to unified system: {} -> {}", conn_name, conn_id);
                                                cx.notify();
                                            });
                                        }
                                    },
                                    Err(e) => {
                                        log::error!("Failed to create unified connection for: {}, {}", conn_name, e);
                                    }
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

    /// Load databases using the connection interface
    pub async fn load_databases(
        &mut self,
        connection_id: i64,
        _cx: &mut Context<'_, Self>,
    ) -> Result<Vec<String>, anyhow::Error> {
        info!("Loading databases for connection ID: {}", connection_id);

        // Get or create connection using unified manager
        let db_service = DbService::global(_cx);
        let connection = db_service.get_or_create_connection(connection_id).await?;

        // Load databases using the unified trait
        let databases = connection.get_databases().await?;

        info!("Loaded {} databases", databases.len());
        Ok(databases)
    }

    /// Load schemas for a specific database
    pub async fn load_schemas(
        &mut self,
        connection_id: i64,
        database_name: Option<String>,
        _cx: &mut Context<'_, Self>,
    ) -> Result<Vec<String>, anyhow::Error> {
        info!(
            "Loading schemas for database '{}' in connection ID: {}",
            database_name.as_ref().unwrap_or(&"default".to_string()),
            connection_id
        );

        // Get or create connection using unified manager
        let db_service = DbService::global(_cx);
        let connection = db_service.get_or_create_connection(connection_id).await?;

        // Load schemas using the unified trait
        let schemas = connection.get_schemas().await?;

        info!(
            "Loaded {} schemas for database '{}'",
            schemas.len(),
            database_name.as_ref().unwrap_or(&"default".to_string())
        );
        Ok(schemas)
    }

    /// Load tables for a specific database and schema
    pub async fn load_tables(
        &mut self,
        connection_id: i64,
        database_name: Option<String>,
        schema: Option<String>,
        _cx: &mut Context<'_, Self>,
    ) -> Result<Vec<String>, anyhow::Error> {
        info!(
            "Loading tables for database '{}' schema '{}' in connection ID: {}",
            database_name.as_ref().unwrap_or(&"default".to_string()),
            schema.as_ref().unwrap_or(&"default".to_string()),
            connection_id
        );

        // Get or create connection using unified manager
        let db_service = DbService::global(_cx);
        let connection = db_service.get_or_create_connection(connection_id).await?;

        // Load tables using the unified trait
        let tables = connection.get_tables(schema.as_deref()).await?;

        info!(
            "Loaded {} tables for database '{}' schema '{}'",
            tables.len(),
            database_name.as_ref().unwrap_or(&"default".to_string()),
            schema.as_ref().unwrap_or(&"default".to_string())
        );
        Ok(tables)
    }

    /// Load databases for a connection
    pub fn load_databases_async(
        &mut self,
        connection_id: i64,
        ui_update_callback: impl Fn(&mut Self, Vec<String>) + Send + Sync + 'static,
        error_callback: impl Fn(&mut Self, String) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        let db_service = DbService::global(cx).clone();

        cx.spawn(async move |sidebar_handle, cx| {
            match db_service.get_or_create_connection(connection_id).await {
                Ok(connection) => match connection.get_databases().await {
                    Ok(databases) => {
                        if let Some(sidebar) = sidebar_handle.upgrade() {
                            let _ = sidebar.update(cx, |sidebar, cx| {
                                ui_update_callback(sidebar, databases);
                                cx.notify();
                            });
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to load databases for connection '{}': {}",
                            connection_id,
                            e
                        );
                        if let Some(sidebar) = sidebar_handle.upgrade() {
                            let _ = sidebar.update(cx, |sidebar, cx| {
                                error_callback(sidebar, format!("Error loading databases: {}", e));
                                cx.notify();
                            });
                        }
                    }
                },
                Err(e) => {
                    log::error!(
                        "Failed to get connection for databases '{}': {}",
                        connection_id,
                        e
                    );
                }
            }
        })
        .detach();
    }

    /// Load schemas for a specific database
    pub fn load_schemas_async(
        &mut self,
        connection_id: i64,
        database_name: Option<String>,
        ui_update_callback: impl Fn(&mut Self, Vec<String>) + Send + Sync + 'static,
        error_callback: impl Fn(&mut Self, String) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        let db_service = DbService::global(cx).clone();
        let database_name_clone = database_name.clone();

        cx.spawn(async move |sidebar_handle, cx| {
            match db_service.get_or_create_connection(connection_id).await {
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
                            "Failed to load schemas for database '{}' in connection '{}': {}",
                            database_name_clone
                                .as_ref()
                                .unwrap_or(&"default".to_string()),
                            connection_id,
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
                        connection_id,
                        e
                    );
                }
            }
        })
        .detach();
    }

    /// Load tables for a specific database and schema
    pub fn load_tables_async(
        &mut self,
        connection_id: i64,
        database_name: Option<String>,
        schema: Option<String>,
        ui_update_callback: impl Fn(&mut Self, Vec<String>) + Send + Sync + 'static,
        error_callback: impl Fn(&mut Self, String) + Send + Sync + 'static,
        cx: &mut Context<Self>,
    ) {
        let db_service = DbService::global(cx).clone();
        let database_name_clone = database_name.clone();
        let schema_clone = schema.clone();

        cx.spawn(async move |sidebar_handle, cx| {
            match db_service.get_or_create_connection(connection_id).await {
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
                            "Failed to load tables for database '{}' schema '{}' in connection '{}': {}",
                            database_name_clone.as_ref().unwrap_or(&"default".to_string()),
                            schema_clone.as_ref().unwrap_or(&"default".to_string()),
                            connection_id,
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
                    log::error!("Failed to get connection for tables '{}': {}", connection_id, e);
                }
            }
        })
        .detach();
    }

    /// Toggle expansion of a connection and load databases as needed
    pub fn toggle_connection(&mut self, connection_key: &str, cx: &mut Context<Self>) {
        if let Some(connection_info) = self.unified_connections.get_mut(connection_key) {
            let was_expanded = connection_info.expanded;
            connection_info.expanded = !connection_info.expanded;

            if !was_expanded && connection_info.expanded {
                // Connection is being expanded - load databases
                let connection_id = connection_info.connection_id;
                let connection_key_clone = connection_key.to_string();
                let connection_key_for_logging = connection_key_clone.clone();

                let connection_key_for_success = connection_key_clone.clone();
                let connection_key_for_error = connection_key_clone.clone();

                self.load_databases_async(
                    connection_id,
                    move |sidebar, databases| {
                        // Success callback - update databases
                        if let Some(conn_info) = sidebar
                            .unified_connections
                            .get_mut(&connection_key_for_success)
                        {
                            conn_info.databases.clear();
                            for database_name in &databases {
                                let database = DatabaseNode::new(database_name.clone());
                                conn_info.databases.push(database);
                            }
                        }
                        log::info!(
                            "Loaded {} databases for connection: {}",
                            databases.len(),
                            connection_key_for_logging
                        );
                    },
                    move |sidebar, error_msg| {
                        // Error callback - show error
                        if let Some(conn_info) = sidebar
                            .unified_connections
                            .get_mut(&connection_key_for_error)
                        {
                            conn_info.databases.clear();
                            let mut error_database = DatabaseNode::new("Error".to_string());
                            error_database.schemas = vec![SchemaNode::new(error_msg.clone())];
                            conn_info.databases.push(error_database);
                        }
                        log::error!(
                            "Failed to load databases for connection '{}': {}",
                            connection_key_for_error,
                            error_msg
                        );
                    },
                    cx,
                );
            }

            cx.notify();
        }
    }

    /// Toggle expansion of a database and load schemas as needed
    pub fn toggle_database(
        &mut self,
        connection_key: &str,
        database_name: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(connection_info) = self.unified_connections.get_mut(connection_key) {
            if let Some(database) = connection_info
                .databases
                .iter_mut()
                .find(|d| d.name == database_name)
            {
                let was_expanded = database.expanded;
                database.expanded = !database.expanded;

                if !was_expanded && database.expanded && database.schemas.is_empty() {
                    // Database is being expanded - load schemas
                    let connection_id = connection_info.connection_id;
                    let connection_key_for_success = connection_key.to_string();
                    let connection_key_for_error = connection_key.to_string();
                    let connection_key_for_logging = connection_key.to_string();
                    let database_name_for_success = database_name.to_string();
                    let database_name_for_error = database_name.to_string();
                    let database_name_for_logging = database_name.to_string();

                    self.load_schemas_async(
                        connection_id,
                        Some(database_name_for_success.clone()),
                        move |sidebar, schemas| {
                            let schemas_count = schemas.len();
                            let database_name_for_success_clone = database_name_for_success.clone();
                            // Success callback - update schemas
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_success)
                            {
                                if let Some(database) = conn_info
                                    .databases
                                    .iter_mut()
                                    .find(|d| d.name == database_name_for_success_clone)
                                {
                                    database.schemas.clear();
                                    for schema_name in &schemas {
                                        let mut schema = SchemaNode::new(schema_name.clone());
                                        // Expand "public" schema by default
                                        schema.expanded = schema_name == "public";
                                        database.schemas.push(schema);
                                    }
                                }
                            }
                            log::info!(
                                "Loaded {} schemas for database '{}': {}",
                                schemas_count,
                                database_name_for_logging,
                                connection_key_for_logging
                            );
                        },
                        move |sidebar, error_msg| {
                            // Error callback - show error
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_error)
                            {
                                if let Some(database) = conn_info
                                    .databases
                                    .iter_mut()
                                    .find(|d| d.name == database_name_for_error)
                                {
                                    database.schemas.clear();
                                    let mut error_schema = SchemaNode::new("Error".to_string());
                                    error_schema.tables = vec![error_msg.clone()];
                                    database.schemas.push(error_schema);
                                }
                            }
                            log::error!(
                                "Failed to load schemas for database '{}': {}",
                                database_name_for_error,
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

    /// Toggle expansion of a schema and load tables as needed
    pub fn toggle_schema(
        &mut self,
        connection_key: &str,
        database_name: &str,
        schema_name: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(connection_info) = self.unified_connections.get_mut(connection_key) {
            if let Some(database) = connection_info
                .databases
                .iter_mut()
                .find(|d| d.name == database_name)
            {
                if let Some(schema) = database.schemas.iter_mut().find(|s| s.name == schema_name) {
                    let was_expanded = schema.expanded;
                    schema.expanded = !schema.expanded;

                    if !was_expanded && schema.expanded && schema.tables.is_empty() {
                        // Schema is being expanded - load tables
                        let connection_id = connection_info.connection_id;
                        let connection_key_for_success = connection_key.to_string();
                        let connection_key_for_error = connection_key.to_string();
                        let connection_key_for_logging = connection_key.to_string();
                        let database_name_for_success = database_name.to_string();
                        let database_name_for_error = database_name.to_string();
                        let database_name_for_logging = database_name.to_string();
                        let schema_name_for_success = schema_name.to_string();
                        let schema_name_for_error = schema_name.to_string();
                        let schema_name_for_logging = schema_name.to_string();

                        self.load_tables_async(
                            connection_id,
                            Some(database_name_for_success.clone()),
                            Some(schema_name_for_success.clone()),
                            move |sidebar, tables| {
                                let tables_count = tables.len();
                                let database_name_for_success_clone =
                                    database_name_for_success.clone();
                                let schema_name_for_success_clone = schema_name_for_success.clone();
                                // Success callback - update tables
                                if let Some(conn_info) = sidebar
                                    .unified_connections
                                    .get_mut(&connection_key_for_success)
                                {
                                    if let Some(database) = conn_info
                                        .databases
                                        .iter_mut()
                                        .find(|d| d.name == database_name_for_success_clone)
                                    {
                                        if let Some(schema) = database
                                            .schemas
                                            .iter_mut()
                                            .find(|s| s.name == schema_name_for_success_clone)
                                        {
                                            schema.tables = tables;
                                        }
                                    }
                                }
                                log::info!(
                                    "Loaded {} tables for schema '{}' in database '{}': {}",
                                    tables_count,
                                    schema_name_for_logging,
                                    database_name_for_logging,
                                    connection_key_for_logging
                                );
                            },
                            move |sidebar, error_msg| {
                                // Error callback - show error
                                if let Some(conn_info) = sidebar
                                    .unified_connections
                                    .get_mut(&connection_key_for_error)
                                {
                                    if let Some(database) = conn_info
                                        .databases
                                        .iter_mut()
                                        .find(|d| d.name == database_name_for_error)
                                    {
                                        if let Some(schema) = database
                                            .schemas
                                            .iter_mut()
                                            .find(|s| s.name == schema_name_for_error)
                                        {
                                            schema.tables = vec![format!("Error: {}", error_msg)];
                                        }
                                    }
                                }
                                log::error!(
                                    "Failed to load tables for schema '{}' in database '{}': {}",
                                    schema_name_for_error,
                                    database_name_for_error,
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
                    SidebarGroup::new("Connections").child(SidebarMenu::new().children({
                        let mut menu_items = Vec::new();

                        // Add connections
                        for (connection_key, connection_info) in &self.unified_connections {
                            let connection_key_clone = connection_key.clone();
                            let display_name = connection_info.display_name.clone();
                            let connection_id = connection_info.connection_id;
                            let was_expanded = connection_info.expanded;

                            // Create menu item for connection (no query creation at connection level)
                            let menu_item =
                                SidebarMenuItem::new(SharedString::from(display_name.clone()))
                                    .icon(match connection_info.connection.get_icon_name() {
                                        blanco_core::IconName::Sqlite => IconName::Sqlite,
                                        blanco_core::IconName::Postgres => IconName::Postgresql,
                                        blanco_core::IconName::Database => IconName::Database,
                                        blanco_core::IconName::Table => IconName::Sheet,
                                        blanco_core::IconName::Column => IconName::SquareTerminal,
                                        _ => IconName::SquareTerminal,
                                    })
                                    .active(was_expanded)
                                    .id(("connection", connection_key.len() as u64))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        log::info!("Clicked on connection: {}", display_name);
                                        this.toggle_connection(&connection_key_clone, cx);
                                    }));

                            menu_items.push(menu_item);

                            // Add databases if connection is expanded
                            if was_expanded {
                                for database in &connection_info.databases {
                                    let database_name = database.name.clone();
                                    let database_expanded = database.expanded;
                                    let database_key =
                                        format!("{}:{}", connection_key, database_name);

                                    let database_item = SidebarMenuItem::new(SharedString::from(
                                        database_name.clone(),
                                    ))
                                    .icon(IconName::Database)
                                    .active(database_expanded)
                                    .id(("database", database_key.len() as u64))
                                    .context_menu({
                                        let database_name_for_menu = database_name.clone();
                                        move |menu, _window, _cx| {
                                            menu.menu(
                                                "New Query",
                                                Box::new(crate::app::NewQueryForDatabase {
                                                    connection_id: connection_id,
                                                    database_name: database_name_for_menu.clone(),
                                                }),
                                            )
                                        }
                                    })
                                    .on_click(cx.listener({
                                        let connection_key_for_click = connection_key.clone();
                                        let database_name_for_click = database_name.clone();
                                        move |this, _, _, cx| {
                                            log::info!(
                                                "Clicked on database: {}",
                                                database_name_for_click
                                            );
                                            this.toggle_database(
                                                &connection_key_for_click,
                                                &database_name_for_click,
                                                cx,
                                            );
                                        }
                                    }));

                                    menu_items.push(database_item);

                                    // Add schemas if database is expanded
                                    if database_expanded {
                                        for schema in &database.schemas {
                                            let schema_name = schema.name.clone();
                                            let schema_expanded = schema.expanded;
                                            let schema_key =
                                                format!("{}:{}", database_key, schema_name);

                                            let schema_item = SidebarMenuItem::new(
                                                SharedString::from(schema_name.clone()),
                                            )
                                            .icon(IconName::Folder)
                                            .active(schema_expanded)
                                            .id(("schema", schema_key.len() as u64))
                                            .context_menu({
                                                let database_name_for_menu = database_name.clone();
                                                let schema_name_for_menu = schema_name.clone();
                                                move |menu, _window, _cx| {
                                                    menu.menu(
                                                        "New Query",
                                                        Box::new(crate::app::NewQueryForSchema {
                                                            connection_id: connection_id,
                                                            database_name: database_name_for_menu
                                                                .clone(),
                                                            schema_name: schema_name_for_menu
                                                                .clone(),
                                                        }),
                                                    )
                                                }
                                            })
                                            .on_click(cx.listener({
                                                let connection_key_for_click =
                                                    connection_key.clone();
                                                let database_name_for_click = database_name.clone();
                                                let schema_name_for_click = schema_name.clone();
                                                move |this, _, _, cx| {
                                                    log::info!(
                                                        "Clicked on schema: {}.{}",
                                                        database_name_for_click,
                                                        schema_name_for_click
                                                    );
                                                    this.toggle_schema(
                                                        &connection_key_for_click,
                                                        &database_name_for_click,
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
                                                    .icon(IconName::Sheet)
                                                    .id((
                                                        "table",
                                                        format!(
                                                            "{}:{}:{}",
                                                            database_key, schema_name, table
                                                        )
                                                        .len()
                                                            as u64,
                                                    ))
                                                    .context_menu({
                                                        let database_name_for_menu =
                                                            database_name.clone();
                                                        let schema_name_for_menu =
                                                            schema_name.clone();
                                                        let table_name_for_menu = table.clone();
                                                        move |menu, _window, _cx| {
                                                            menu.menu(
                                                                "New Query",
                                                                Box::new(
                                                                    crate::app::NewQueryForTable {
                                                                        connection_id:
                                                                            connection_id,
                                                                        database_name:
                                                                            database_name_for_menu
                                                                                .clone(),
                                                                        schema_name:
                                                                            schema_name_for_menu
                                                                                .clone(),
                                                                        table_name:
                                                                            table_name_for_menu
                                                                                .clone(),
                                                                    },
                                                                ),
                                                            )
                                                        }
                                                    });

                                                    menu_items.push(table_item);
                                                }
                                            }
                                        }
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
