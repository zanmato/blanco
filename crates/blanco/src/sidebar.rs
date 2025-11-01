use crate::connection_sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui::Subscription;
use gpui::{
    div, App, AppContext, Axis, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_component::{
    button::Button, h_flex, v_flex, ActiveTheme, ContextModal as _, IconName as GCIconName, Side,
    StyledExt,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app_events::AppEvent;
use crate::connection_modal::NewConnectionModal;
use crate::db_service::DbService;
use blanco_ui::IconName;
use log::info;

// Constants for lazy loading and pagination
const TABLES_PER_PAGE: usize = 100; // Load tables in chunks of 100
const LARGE_SCHEMA_THRESHOLD: usize = 500; // Consider schema large if it has more than 500 tables

/// Simple schema node for sidebar display
#[derive(Clone, Debug)]
pub struct SchemaNode {
    pub name: String,
    pub expanded: bool,
    pub tables: Vec<String>,
    pub loading: bool,
    pub has_more_tables: bool,
    pub tables_loaded: usize,
}

impl SchemaNode {
    pub fn new(name: String) -> Self {
        let expanded = name == "public"; // Default expand public schema
        Self {
            name,
            expanded,
            tables: Vec::new(),
            loading: false,
            has_more_tables: false,
            tables_loaded: 0,
        }
    }
}

/// Cache entry to avoid rebuilding menu items on every render
struct CacheEntry {
    last_updated: Instant,
}

impl CacheEntry {
    fn new() -> Self {
        Self {
            last_updated: Instant::now(),
        }
    }

    fn is_valid(&self, max_age: Duration) -> bool {
        self.last_updated.elapsed() < max_age
    }
}

/// Menu cache for performance optimization
struct MenuCache {
    // Cache for connection menu items (stores timestamps)
    connection_items: HashMap<String, CacheEntry>,
    // Cache for schema menu items (stores timestamps)
    schema_items: HashMap<String, CacheEntry>,
    // Cache for table menu items (stores timestamps)
    table_items: HashMap<String, CacheEntry>,
    // Last cache invalidation time
    last_invalidated: Instant,
}

impl Default for MenuCache {
    fn default() -> Self {
        Self {
            connection_items: HashMap::new(),
            schema_items: HashMap::new(),
            table_items: HashMap::new(),
            last_invalidated: Instant::now(),
        }
    }
}

impl MenuCache {
    fn new() -> Self {
        Self::default()
    }

    fn invalidate(&mut self) {
        self.connection_items.clear();
        self.schema_items.clear();
        self.table_items.clear();
        self.last_invalidated = Instant::now();
    }

    fn is_valid(&self, max_age: Duration) -> bool {
        self.last_invalidated.elapsed() < max_age
    }

    fn get_cache_key(connection_key: &str, item_type: &str, name: &str) -> String {
        format!("{}:{}:{}", connection_key, item_type, name)
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
    pub loading: bool,
    pub schemas_loaded: bool,
}

pub struct ConnectionSidebar {
    focus_handle: FocusHandle,
    collapsed: bool,
    // Unified connections cache (connection_string -> connection info)
    unified_connections: HashMap<String, UnifiedConnectionInfo>,
    // Cached menu items to avoid rebuilding on every render
    cached_menu_items: Option<Vec<SidebarMenuItem>>,
    // Track when we need to rebuild the menu
    menu_needs_rebuild: bool,
    // Subscriptions for reactive updates
    _subscriptions: Vec<Subscription>,
}

impl ConnectionSidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut sidebar = Self {
            focus_handle: cx.focus_handle(),
            collapsed: false,
            unified_connections: HashMap::new(),
            cached_menu_items: None,
            menu_needs_rebuild: true, // Initial build needed
            _subscriptions: Vec::new(),
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
    fn subscribe_to_connection_events(&mut self, cx: &mut Context<Self>) {
        // For now, we'll use a simpler approach with cache invalidation in async callbacks
        // The event-driven system can be improved later with proper GPUI event patterns
        log::info!(
            "Connection event subscription system initialized with cache invalidation in callbacks"
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
                                                loading: false,
                                                schemas_loaded: false,
                                            };
                                            sidebar.unified_connections.insert(connection_key, unified_info);
                                            // Invalidate cache since we actually added a connection
                                            sidebar.invalidate_menu_cache();
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
        log::info!(
            "Sidebar set_collapsed called with: {} (was: {})",
            collapsed,
            self.collapsed
        );
        self.collapsed = collapsed;
        cx.notify();
    }

    /// Build connection menu item
    fn build_connection_menu_item(
        &mut self,
        connection_key: &str,
        connection_info: &UnifiedConnectionInfo,
        cx: &mut Context<Self>,
    ) -> SidebarMenuItem {
        // Build new menu item
        let display_name = connection_info.display_name.clone();
        let connection_key_clone = connection_key.to_string();
        let was_expanded = connection_info.expanded;

        let menu_item = SidebarMenuItem::new(SharedString::from(display_name.clone()))
            .icon(match connection_info.connection.get_icon_name() {
                blanco_core::IconName::Sqlite => IconName::Database,
                blanco_core::IconName::Postgres => IconName::Database,
                blanco_core::IconName::Database => IconName::Database,
                blanco_core::IconName::DatabaseConnected => IconName::DatabaseConnected,
                blanco_core::IconName::Table => IconName::DatabaseConnected,
                blanco_core::IconName::Column => IconName::DatabaseConnected,
                _ => IconName::DatabaseConnected,
            })
            .active(was_expanded)
            .id(("unified-connection", connection_key.len() as u64))
            .context_menu({
                let display_name_for_menu = display_name.clone();
                let connection_key_for_menu = connection_key.to_string();
                let connection_string_for_menu = connection_info.connection_string.clone();
                move |menu, _window, _cx| {
                    log::info!(
                        "Creating context menu for unified connection: {}",
                        display_name_for_menu
                    );
                    menu.menu(
                        "New Query",
                        Box::new(crate::app::NewQueryForUnifiedConnection {
                            connection_key: connection_key_for_menu.clone(),
                            connection_string: connection_string_for_menu.clone(),
                            display_name: display_name_for_menu.clone(),
                        }),
                    )
                }
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                log::info!("Clicked on unified connection: {}", display_name);
                this.toggle_unified_connection(&connection_key_clone, cx);
            }));

        menu_item
    }

    /// Build schema menu item
    fn build_schema_menu_item(
        &mut self,
        connection_key: &str,
        schema: &SchemaNode,
        cx: &mut Context<Self>,
    ) -> SidebarMenuItem {
        // Build new menu item
        let schema_name = schema.name.clone();
        let schema_expanded = schema.expanded;
        let connection_key_for_click = connection_key.to_string();
        let schema_name_for_click = schema_name.clone();

        let menu_item = SidebarMenuItem::new(SharedString::from(schema_name.clone()))
            .icon(IconName::Folder)
            .active(schema_expanded)
            .id((
                "unified-schema",
                format!("{}:{}", connection_key, schema_name).len() as u64,
            ))
            .context_menu({
                let connection_key_for_menu = connection_key.to_string();
                let schema_name_for_menu = schema_name.clone();
                move |menu, _window, _cx| {
                    menu.menu(
                        "New Query",
                        Box::new(crate::app::NewQueryForUnifiedSchema {
                            connection_key: connection_key_for_menu.clone(),
                            schema_name: schema_name_for_menu.clone(),
                        }),
                    )
                }
            })
            .on_click(cx.listener({
                move |this, _, _, cx| {
                    log::info!("Clicked on unified schema: {}", schema_name_for_click);
                    this.toggle_unified_schema(
                        &connection_key_for_click,
                        &schema_name_for_click,
                        cx,
                    );
                }
            }));

        menu_item
    }

    /// Build table menu item
    fn build_table_menu_item(
        &mut self,
        connection_key: &str,
        schema_name: Option<&str>,
        table_name: &str,
    ) -> SidebarMenuItem {
        // Build new menu item
        let id_base = if let Some(schema) = schema_name {
            format!("{}:{}:{}", connection_key, schema, table_name)
        } else {
            format!("{}:{}", connection_key, table_name)
        };

        let menu_item = SidebarMenuItem::new(SharedString::from(table_name.to_string()))
            .icon(IconName::Sheet)
            .id(("unified-table", id_base.len() as u64));

        menu_item
    }

    /// Mark menu as needing rebuild
    fn invalidate_menu_cache(&mut self) {
        self.menu_needs_rebuild = true;
        // Don't clear cached_menu_items here - let the next render handle it
        log::info!("Menu cache invalidated due to data changes");
    }

    /// Get cached menu items or rebuild if necessary
    fn get_cached_menu_items(&mut self, cx: &mut Context<Self>) -> &Vec<SidebarMenuItem> {
        if self.menu_needs_rebuild || self.cached_menu_items.is_none() {
            log::info!(
                "Rebuilding sidebar menu items... (collapsed: {})",
                self.collapsed
            );
            let menu_items = self.build_menu_items(cx);
            self.cached_menu_items = Some(menu_items);
            self.menu_needs_rebuild = false;
            log::info!(
                "Menu items cached, {} items total, collapsed state: {}",
                self.cached_menu_items.as_ref().unwrap().len(),
                self.collapsed
            );
        }

        self.cached_menu_items.as_ref().unwrap()
    }

    /// Build menu items using optimized rendering
    fn build_menu_items(&mut self, cx: &mut Context<Self>) -> Vec<SidebarMenuItem> {
        let mut menu_items = Vec::new();

        // Use a more straightforward approach to avoid borrowing issues
        let connections: Vec<(String, UnifiedConnectionInfo)> = self
            .unified_connections
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (connection_key, connection_info) in connections {
            // Build connection menu item
            let connection_item =
                self.build_connection_menu_item(&connection_key, &connection_info, cx);
            menu_items.push(connection_item);

            // Add child items (schemas/tables) if expanded
            if connection_info.expanded {
                if connection_info.loading {
                    // Show loading indicator for schemas
                    let loading_item =
                        SidebarMenuItem::new(SharedString::from("Loading schemas..."))
                            .icon(IconName::SquareTerminal)
                            .id(("loading-schemas", connection_key.len() as u64));
                    menu_items.push(loading_item);
                } else if connection_info.connection.supports_schemas() {
                    // Add schemas for connections that support them (e.g., PostgreSQL)
                    for schema in &connection_info.schemas {
                        // Build schema menu item
                        let schema_item = self.build_schema_menu_item(&connection_key, schema, cx);
                        menu_items.push(schema_item);

                        // Add tables if schema is expanded
                        if schema.expanded {
                            if schema.loading {
                                // Show loading indicator for tables
                                let loading_item =
                                    SidebarMenuItem::new(SharedString::from("Loading tables..."))
                                        .icon(IconName::SquareTerminal)
                                        .id((
                                            "loading-tables",
                                            format!("{}:{}", connection_key, schema.name).len()
                                                as u64,
                                        ));
                                menu_items.push(loading_item);
                            } else if !schema.tables.is_empty() {
                                for table in &schema.tables {
                                    // Build table menu item
                                    let table_item = self.build_table_menu_item(
                                        &connection_key,
                                        Some(&schema.name),
                                        table,
                                    );
                                    menu_items.push(table_item);
                                }

                                // Show "Load more" indicator if there are more tables
                                if schema.has_more_tables {
                                    let schema_name = schema.name.clone();
                                    let connection_key_clone = connection_key.clone();
                                    let load_more_item = SidebarMenuItem::new(SharedString::from(
                                        "Load more tables...",
                                    ))
                                    .icon(IconName::SquareTerminal)
                                    .id((
                                        "load-more",
                                        format!("{}:{}_loadmore", connection_key, schema_name).len()
                                            as u64,
                                    ))
                                    .on_click(cx.listener({
                                        let connection_key_for_load = connection_key_clone.clone();
                                        let schema_name_for_load = schema_name.clone();
                                        move |this, _, _, cx| {
                                            this.load_more_tables(
                                                &connection_key_for_load,
                                                &schema_name_for_load,
                                                cx,
                                            );
                                        }
                                    }));
                                    menu_items.push(load_more_item);
                                }
                            }
                        }
                    }
                } else {
                    // Flat structure - show tables directly (e.g., SQLite)
                    for table in &connection_info.tables {
                        // Build table menu item (flat structure)
                        let table_item = self.build_table_menu_item(&connection_key, None, table);
                        menu_items.push(table_item);
                    }
                }
            }
        }

        menu_items
    }

    /// Load more tables for a schema with pagination
    fn load_more_tables(
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
                if schema.loading || !schema.has_more_tables {
                    return; // Already loading or no more tables
                }

                schema.loading = true;
                let connection_string = connection_info.connection_string.clone();
                let connection_key_for_callback = connection_key.to_string();
                let schema_name_for_callback = schema_name.to_string();
                let current_table_count = schema.tables_loaded;
                let connection_key_for_error = connection_key_for_callback.clone();
                let schema_name_for_error = schema_name_for_callback.clone();

                self.load_tables_unified_async(
                    &connection_string,
                    Some(schema_name),
                    move |sidebar, mut new_tables| {
                        // Get only the new tables (paginated)
                        if new_tables.len() > TABLES_PER_PAGE {
                            let connection_key_clone = connection_key_for_callback.clone();
                            let schema_name_clone = schema_name_for_callback.clone();

                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_callback)
                            {
                                if let Some(schema) = conn_info
                                    .schemas
                                    .iter_mut()
                                    .find(|s| s.name == schema_name_for_callback)
                                {
                                    // Keep only the new tables beyond what we already have
                                    if schema.tables.len() < current_table_count + TABLES_PER_PAGE {
                                        let start_index = schema.tables.len();
                                        let end_index = std::cmp::min(
                                            start_index + TABLES_PER_PAGE,
                                            new_tables.len(),
                                        );
                                        let additional_tables: Vec<String> =
                                            new_tables.drain(start_index..end_index).collect();

                                        schema.tables.extend(additional_tables.clone());
                                        schema.tables_loaded = schema.tables.len();
                                        schema.has_more_tables = end_index < new_tables.len();
                                        schema.loading = false;
                                        // Invalidate cache to show loaded tables
                                        sidebar.invalidate_menu_cache();
                                    }
                                }
                            }
                        } else {
                            // All tables fit in one page
                            if let Some(conn_info) = sidebar
                                .unified_connections
                                .get_mut(&connection_key_for_callback)
                            {
                                if let Some(schema) = conn_info
                                    .schemas
                                    .iter_mut()
                                    .find(|s| s.name == schema_name_for_callback)
                                {
                                    schema.tables = new_tables;
                                    schema.tables_loaded = schema.tables.len();
                                    schema.has_more_tables = false;
                                    schema.loading = false;
                                }
                            }
                        }
                    },
                    move |sidebar, error_msg| {
                        if let Some(connection_info) = sidebar
                            .unified_connections
                            .get_mut(&connection_key_for_error)
                        {
                            if let Some(schema) = connection_info
                                .schemas
                                .iter_mut()
                                .find(|s| s.name == schema_name_for_error)
                            {
                                schema.loading = false;
                                schema.tables = vec![format!("Error: {}", error_msg)];
                            }
                        }
                    },
                    cx,
                );
            }
        }
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
            loading: false,
            schemas_loaded: false,
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
        // Invalidate cache when loading new tables
        self.invalidate_menu_cache();
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
        // Invalidate cache when loading new schemas
        self.invalidate_menu_cache();
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
                // Connection is being expanded - set loading state and load schemas/tables
                connection_info.loading = true;
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
                                conn_info.loading = false;
                                conn_info.schemas_loaded = true;
                                conn_info.schemas.clear();
                                for schema_name in &schemas {
                                    let mut schema = SchemaNode::new(schema_name.clone());
                                    // Expand "public" schema by default
                                    schema.expanded = schema_name == "public";
                                    // Mark as large schema if it exceeds threshold
                                    // We'll determine this when we load tables
                                    conn_info.schemas.push(schema);
                                }
                            }
                            // Invalidate cache to show loaded schemas
                            sidebar.invalidate_menu_cache();
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
                                conn_info.loading = false;
                                conn_info.schemas.clear();
                                let mut error_schema = SchemaNode::new("Error".to_string());
                                error_schema.tables = vec![error_msg.clone()];
                                conn_info.schemas.push(error_schema);
                            }
                            // Invalidate cache to show error state
                            sidebar.invalidate_menu_cache();
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
                                conn_info.loading = false;
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
                                conn_info.loading = false;
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
                    // Schema is being expanded - set loading state and load tables
                    schema.loading = true;
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
                        move |sidebar, mut tables| {
                            // Success callback - update tables with pagination
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
                                    schema.loading = false;

                                    // Implement pagination for large schemas
                                    if tables.len() > TABLES_PER_PAGE {
                                        // Take only the first page of tables
                                        let first_page: Vec<String> = tables
                                            .drain(0..TABLES_PER_PAGE)
                                            .collect();
                                        schema.tables = first_page;
                                        schema.tables_loaded = schema.tables.len();
                                        schema.has_more_tables = true;
                                        log::info!(
                                            "Loaded first {} tables for large PostgreSQL schema '{}': {} ({} total)",
                                            schema.tables.len(),
                                            schema_name_for_logging,
                                            connection_key_for_schema_tables_logging,
                                            tables_count
                                        );
                                    } else {
                                        // All tables fit in one page
                                        schema.tables = tables;
                                        schema.tables_loaded = schema.tables.len();
                                        schema.has_more_tables = false;
                                        schema.loading = false;
                                        // Invalidate cache to show loaded tables
                                        sidebar.invalidate_menu_cache();
                                        log::info!(
                                            "Loaded {} tables for PostgreSQL schema '{}': {}",
                                            tables_count,
                                            schema_name_for_logging,
                                            connection_key_for_schema_tables_logging
                                        );
                                    }
                                }
                            }
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
                                    schema.loading = false;
                                    schema.tables = vec![format!("Error: {}", error_msg)];
                                    schema.has_more_tables = false;
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

impl EventEmitter<()> for ConnectionSidebar {}
impl EventEmitter<AppEvent> for ConnectionSidebar {}
impl Focusable for ConnectionSidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConnectionSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Extract needed values before borrowing self for get_cached_menu_items
        let collapsed = self.collapsed;

        // Get cached menu items - they will only be rebuilt when necessary
        let menu_items = self.get_cached_menu_items(cx);

        Sidebar::new(Side::Left).collapsed(collapsed).child(
            SidebarGroup::new("Databases").child(SidebarMenu::new().children(menu_items.clone())),
        )

        /* .child(
            h_flex()
                .p_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    Button::new("new-connection")
                        .w_full()
                        .outline()
                        .icon(GCIconName::Plus)
                        .label("New Connection")
                        .on_click(cx.listener(move |_this, _event, window, cx| {
                            log::info!("New Connection button clicked");

                            // Create the modal content outside the builder so we can access it
                            let modal_content = cx.new(|cx| NewConnectionModal::new(window, cx));
                            let content_for_focus = modal_content.clone();

                            window.open_modal(cx, move |modal, _window, _cx| {
                                let content_clone = modal_content.clone();

                                modal
                                    .title("New Connection")
                                    .w(gpui::px(500.))
                                    .child(modal_content.clone())
                                    .footer({
                                        let content = content_clone.clone();
                                        move |ok, cancel, window, cx| {
                                            let test_btn = Button::new("test-connection")
                                                .label("Test Connection")
                                                .on_click({
                                                    let content = content.clone();
                                                    move |_, window, cx| {
                                                        content.update(cx, |modal, cx| {
                                                            modal.test_connection(window, cx);
                                                        });
                                                    }
                                                })
                                                .into_any_element();

                                            vec![test_btn, cancel(window, cx), ok(window, cx)]
                                        }
                                    })
                                    .on_ok({
                                        let content = content_clone.clone();
                                        move |_, window, cx| {
                                            if let Some(conn_data) = content.read(cx).get_connection_data(cx) {
                                                // Save connection to database
                                                let db_service = DbService::global(cx).clone();
                                                let app_db = db_service.app_db_handle();

                                                // For PostgreSQL connections, we need to construct the connection string
                                                // and add it to the sidebar
                                                if conn_data.db_type == "PostgreSQL" {
                                                    if let (Some(host), Some(port), Some(database), Some(username), Some(password)) = (
                                                        conn_data.host.as_ref(),
                                                        conn_data.port,
                                                        conn_data.database_name.as_ref(),
                                                        conn_data.username.as_ref(),
                                                        conn_data.password.as_ref()
                                                    ) {
                                                        let connection_string = format!(
                                                            "postgresql://{}:{}@{}:{}/{}",
                                                            username, password, host, port, database
                                                        );

                                                        log::info!("Attempting to save PostgreSQL connection: {} (name: {})", connection_string, conn_data.name);
                                                        log::info!("Connection data: db_type={}, host={:?}, port={:?}, database={:?}, username={:?}",
                                                            conn_data.db_type, conn_data.host, conn_data.port, conn_data.database_name, conn_data.username);

                                                        // Save to database
                                                        log::info!("Attempting to save PostgreSQL connection to database...");
                                                        let connection_string_clone = connection_string.clone();
                                                        let conn_name_clone = conn_data.name.clone();
                                                        cx.spawn(async move |cx| {
                                                            let db_result = if let Some(db) = app_db.read().await.as_ref() {
                                                                db.save_connection(&conn_data).await
                                                            } else {
                                                                Err(sqlx::Error::Configuration("App database not initialized".into()))
                                                            };

                                                            match db_result {
                                                                Ok(saved_id) => {
                                                                    log::info!("✅ PostgreSQL connection saved to database with ID: {}", saved_id);

                                                                    // We'll emit the event from the main context after the async task completes
                                                                    // For now, just log that the connection was saved
                                                                }
                                                                Err(e) => {
                                                                    log::error!("❌ Failed to save PostgreSQL connection to database: {}", e);
                                                                }
                                                            }
                                                        })
                                                        .detach();

                                                        // Event emission temporarily disabled during EventEmitter migration
                                                        // The sidebar refreshes through other mechanisms when connections are saved
                                                        log::info!("Connection saved, sidebar will refresh through database polling");

                                                        // Always show notification (user will get success/failure details from logs)
                                                        window.push_notification("Saving PostgreSQL connection...", cx);

                                                        // Connection is automatically added to unified system
                                                        // The user will see the new connection after a brief moment
                                                    } else {
                                                        window.push_notification("Missing PostgreSQL connection details", cx);
                                                        return false;
                                                    }
                                                } else {
                                                    // SQLite or other database types
                                                    cx.spawn(async move |cx| {
                                                        if let Some(db) = app_db.read().await.as_ref() {
                                                            db.save_connection(&conn_data).await.map_err(|e| {
                                                                anyhow::anyhow!("Failed to save connection: {}", e)
                                                            })
                                                        } else {
                                                            Err(anyhow::anyhow!("App database not initialized"))
                                                        }
                                                    })
                                                    .detach();

                                                    window.push_notification("Connection saved successfully", cx);
                                                }

                                                true
                                            } else {
                                                window.push_notification("Please fill in all required fields", cx);
                                                false
                                            }
                                        }
                                    })
                            });

                            // Focus the first input field after the modal opens
                            content_for_focus.read(cx).focus_handle(cx).focus(window);
                        }))
                ),
        )*/
    }
}
