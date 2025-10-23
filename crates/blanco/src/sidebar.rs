use crate::connection_sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui::{
    div, App, AppContext, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, Styled, Window,
};
use gpui_component::{
    button::Button, h_flex, v_flex, ActiveTheme, ContextModal as _, IconName as GCIconName, Side,
};
use std::collections::HashMap;

use crate::connection::Connection;
use crate::connection_modal::NewConnectionModal;
use crate::db_service::{DbService, PgConnectionKey};
use crate::icon::IconName;
use crate::postgres::SchemaNode;
use log::{debug, error, info};

pub struct ConnectionSidebar {
    focus_handle: FocusHandle,
    connections: Vec<Connection>,
    test_db_tables: Vec<String>,
    collapsed: bool,
    test_db_expanded: bool,
    // PostgreSQL connections - now supports multiple connections
    pg_connections: HashMap<PgConnectionKey, (String, Vec<SchemaNode>, bool)>, // key -> (display_name, schemas, expanded)
    // SQLite connections from database
    sqlite_connections: Vec<(String, String, bool)>, // (name, database_path, expanded)
}

impl ConnectionSidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut sidebar = Self {
            focus_handle: cx.focus_handle(),
            connections: Connection::new_mock(),
            test_db_tables: Vec::new(),
            collapsed: false,
            test_db_expanded: false, // Start collapsed - lazy load on expansion
            pg_connections: HashMap::new(),
            sqlite_connections: Vec::new(),
        };

        // Load real connections from database first
        sidebar.load_database_connections(cx);

        // Debug: Log initial state
        log::info!(
            "Sidebar initialized with {} SQLite connections",
            sidebar.sqlite_connections.len()
        );

        // Note: We no longer add mock connections automatically
        // Mock connections can still be added manually for testing if needed

        sidebar
    }

    /// Load real connections from the app database
    fn load_database_connections(&mut self, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();

        // Load connections asynchronously and update UI
        log::info!("Starting to load database connections...");
        let task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            match *app_db.read().await {
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
                        Ok(connections)
                    }
                    Err(e) => {
                        log::error!("Failed to load connections from database: {}", e);
                        Err(anyhow::anyhow!("Failed to load connections: {}", e))
                    }
                },
                None => {
                    log::warn!("App database not initialized");
                    Err(anyhow::anyhow!("App database not initialized"))
                }
            }
        });

        cx.spawn(async move |handle, cx| {
            if let Ok(connections) = task.await {
                if let Some(sidebar) = handle.upgrade() {
                    let _ = sidebar.update(cx, |sidebar, cx| {
                        // Process loaded connections and add them to sidebar
                        for conn in &connections {
                            if conn.db_type == "PostgreSQL" {
                                if let (Some(host), Some(port), Some(database), Some(username)) =
                                    (&conn.host, conn.port, &conn.database_name, &conn.username) {

                                    let pg_key = PgConnectionKey {
                                        host: host.clone(),
                                        port: port as u16,
                                        database: database.clone(),
                                        username: username.clone(),
                                        password: conn.password.clone(),
                                    };

                                    // Add connection to sidebar (start collapsed, will load schemas on expand)
                                    sidebar.pg_connections.insert(
                                        pg_key.clone(),
                                        (conn.name.clone(), Vec::new(), false)
                                    );

                                    log::info!("Added PostgreSQL connection to sidebar: {} ({}:{}/{})",
                                             conn.name, host, port, database);
                                }
                            } else if conn.db_type == "SQLite" {
                                log::info!("Found SQLite connection: {} -> {} (db_type: {}, db_path: {:?})",
                                         conn.name,
                                         conn.database_path.as_ref().unwrap_or(&"None".to_string()),
                                         conn.db_type,
                                         conn.database_path);
                                if let Some(database_path) = &conn.database_path {
                                    log::info!("Adding SQLite connection to sidebar: {} -> {}", conn.name, database_path);
                                    // Add SQLite connection to sidebar (start collapsed, will load tables on expand)
                                    sidebar.sqlite_connections.push((conn.name.clone(), database_path.clone(), false));
                                    log::info!("Sidebar now has {} SQLite connections after adding", sidebar.sqlite_connections.len());
                                } else {
                                    log::warn!("SQLite connection has no database_path: {}", conn.name);
                                }
                            }
                        }
                        cx.notify();
                    });
                }
            }
        }).detach();
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        cx.notify();
    }

    /// Load SQLite tables lazily
    fn load_sqlite_tables(&mut self, cx: &mut Context<Self>) {
        log::info!("Sidebar: Loading SQLite tables lazily");
        let db_service = DbService::global(cx).clone();
        let user_db = db_service.user_db_handle();

        let task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            let db = user_db.read().await;
            log::info!(
                "Sidebar: Loading SQLite tables, connected: {}",
                db.is_connected()
            );

            match db.get_tables().await {
                Ok(tables) => {
                    log::info!("Sidebar: Loaded {} SQLite tables", tables.len());
                    Ok(tables)
                }
                Err(e) => {
                    log::error!("Sidebar: Failed to load SQLite tables: {}", e);
                    Err(anyhow::anyhow!("{}", e))
                }
            }
        });

        cx.spawn(async move |handle, cx| {
            if let Ok(tables) = task.await {
                if let Some(sidebar) = handle.upgrade() {
                    let _ = sidebar.update(cx, |sidebar, cx| {
                        sidebar.test_db_tables = tables;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Load PostgreSQL schemas and tables lazily
    fn load_postgres_schemas(
        &mut self,
        key: PgConnectionKey,
        display_name: String,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "Sidebar: Loading PostgreSQL schemas lazily for: {}",
            display_name
        );
        let db_service = DbService::global(cx).clone();
        let key_clone = key.clone();

        let task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            // Clone the connection details we need for the connection string
            let username = key_clone.username.clone();
            let password = key_clone.password.clone();
            let host = key_clone.host.clone();
            let port = key_clone.port;
            let database = key_clone.database.clone();

            // Get or create connection
            let connection_string = if let Some(ref password) = password {
                format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    username, password, host, port, database
                )
            } else {
                format!("postgresql://{}@{}:{}/{}", username, host, port, database)
            };

            match db_service
                .get_or_create_pg_connection(&connection_string)
                .await
            {
                Ok(manager) => {
                    log::info!(
                        "Loading schemas for PostgreSQL connection: {}",
                        display_name
                    );
                    match manager.get_schemas().await {
                        Ok(schemas) => {
                            log::info!(
                                "Found {} schemas for connection: {}",
                                schemas.len(),
                                display_name
                            );
                            let mut schema_nodes = Vec::new();

                            for schema_name in schemas {
                                log::debug!("Loading tables for schema: {}", schema_name);
                                match manager.get_tables(&schema_name).await {
                                    Ok(tables) => {
                                        log::debug!(
                                            "Schema '{}' has {} tables",
                                            schema_name,
                                            tables.len()
                                        );
                                        let mut node = SchemaNode::new(schema_name.clone());
                                        node.tables = tables;
                                        node.expanded = schema_name == "public"; // Expand public by default
                                        schema_nodes.push(node);
                                    }
                                    Err(e) => {
                                        log::error!(
                                            "Failed to load tables for schema '{}': {}",
                                            schema_name,
                                            e
                                        );
                                    }
                                }
                            }

                            Ok((display_name, schema_nodes, false)) // Start collapsed
                        }
                        Err(e) => {
                            log::error!(
                                "Failed to load schemas for connection '{}': {}",
                                display_name,
                                e
                            );
                            Err(anyhow::anyhow!("Failed to load schemas: {}", e))
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to connect to PostgreSQL: {}", e);
                    Err(anyhow::anyhow!("Failed to connect: {}", e))
                }
            }
        });

        cx.spawn(async move |handle, cx| {
            if let Ok(schema_data) = task.await {
                if let Some(sidebar) = handle.upgrade() {
                    let _ = sidebar.update(cx, |sidebar, cx| {
                        sidebar.pg_connections.insert(key, schema_data);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Add a new PostgreSQL connection and load its schemas
    pub fn add_postgres_connection(&mut self, connection_string: &str, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let connection_string = connection_string.to_string();

        let task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            match PgConnectionKey::from_connection_string(&connection_string) {
                Ok(key) => {
                    let display_name = format!(
                        "{}@{}:{}/{}",
                        key.username, key.host, key.port, key.database
                    );

                    // Get or create connection
                    let manager = db_service
                        .get_or_create_pg_connection(&connection_string)
                        .await?;

                    // Load schemas
                    match manager.get_schemas().await {
                        Ok(schemas) => {
                            info!(
                                "Loaded {} schemas for connection: {}",
                                schemas.len(),
                                display_name
                            );
                            let mut schema_nodes = Vec::new();

                            for schema_name in schemas {
                                debug!("Loading tables for schema: {}", schema_name);
                                match manager.get_tables(&schema_name).await {
                                    Ok(tables) => {
                                        debug!(
                                            "Schema '{}' has {} tables",
                                            schema_name,
                                            tables.len()
                                        );
                                        let mut node = SchemaNode::new(schema_name.clone());
                                        node.tables = tables;
                                        node.expanded = schema_name == "public"; // Expand public by default
                                        schema_nodes.push(node);
                                    }
                                    Err(e) => {
                                        error!(
                                            "Failed to load tables for schema '{}': {}",
                                            schema_name, e
                                        );
                                    }
                                }
                            }

                            Ok(Some((key, (display_name, schema_nodes, true))))
                        }
                        Err(e) => {
                            error!(
                                "Failed to load schemas for connection {}: {}",
                                display_name, e
                            );
                            Err(anyhow::anyhow!("{}", e))
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to parse connection string: {}", e);
                    Err(anyhow::anyhow!("{}", e))
                }
            }
        });

        cx.spawn(async move |handle, cx| {
            if let Ok(Some((key, data))) = task.await {
                if let Some(sidebar) = handle.upgrade() {
                    let _ = sidebar.update(cx, |sidebar, cx| {
                        sidebar.pg_connections.insert(key, data);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

impl Focusable for ConnectionSidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConnectionSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .h_full()
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        Sidebar::new(Side::Left).collapsed(self.collapsed).child(
                            SidebarGroup::new("Databases").child(
                                SidebarMenu::new()
                                    // Test Database (actual connection)
                                    .children({
                                        let mut items = Vec::new();
                                        for (conn_ix, (name, database_path, expanded)) in self.sqlite_connections.iter().enumerate() {
                                            let name_for_click = name.clone();
                                            let name_for_menu = name.clone();
                                            let database_path_for_click = database_path.clone();
                                            let expanded_for_children = *expanded;

                                            let mut table_items = Vec::new();
                                            if expanded_for_children {
                                                // Load tables lazily when expanded
                                                if !self.test_db_tables.iter().any(|t| t.contains(&format!("sqlite_conn_{}", conn_ix))) {
                                                    // This is a placeholder - we'd need proper SQLite table loading logic
                                                    // For now, add a placeholder table
                                                    table_items.push(
                                                        SidebarMenuItem::new(SharedString::from("Loading..."))
                                                            .icon(IconName::SquareTerminal)
                                                            .id(("sqlite-table", conn_ix))
                                                    );
                                                }
                                            }

                                            items.push(
                                                SidebarMenuItem::new(SharedString::from(name_for_click.clone()))
                                                    .icon(IconName::Sqlite)
                                                    .active(*expanded)
                                                    .id(("sqlite-connection", conn_ix))
                                                    .context_menu({
                                                        let name = name_for_menu.clone();
                                                        let database_path = database_path.clone();
                                                        move |menu, _window, _cx| {
                                                            log::info!("Creating context menu for SQLite connection: {}", name);
                                                            menu.menu("New Query", Box::new(crate::app::NewQueryForConnection {
                                                                connection_name: name.clone(),
                                                                connection_type: crate::app::ConnectionType::SQLite,
                                                            }))
                                                        }
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        log::info!("Clicked on SQLite connection: {}", name_for_click);
                                                        // Toggle expansion for this connection
                                                        if let Some((_, _, expanded)) = this.sqlite_connections.get_mut(conn_ix) {
                                                            *expanded = !*expanded;

                                                            // Load tables when expanding
                                                            if *expanded {
                                                                // TODO: Load actual SQLite tables for this connection
                                                                log::info!("Would load tables for SQLite connection: {}", name_for_click);
                                                            }
                                                        }
                                                        cx.notify();
                                                    }))
                                                    .children(table_items)
                                            );
                                        }

                                        // PostgreSQL Connections
                                        for (conn_ix, (key, (display_name, schemas, expanded))) in self.pg_connections.iter().enumerate() {
                                            let schemas_clone = schemas.clone();
                                            let key_for_click = key.clone();
                                            let key_for_menu = key.clone();
                                            let display_name_for_click = display_name.clone();
                                            let display_name_for_menu = display_name.clone();
                                            let expanded_for_children = *expanded;

                                            let mut schema_items = Vec::new();
                                            if expanded_for_children {
                                                for (schema_ix, schema) in schemas_clone.iter().enumerate() {
                                                    let schema_name_for_click = schema.name.clone();
                                                    let schema_name_for_menu = schema.name.clone();
                                                    let key_for_schema_click = key_for_click.clone();
                                                    let key_for_schema_menu = key_for_menu.clone();
                                                    let schema_expanded_for_children = schema.expanded;

                                                    let mut table_items = Vec::new();
                                                    if schema_expanded_for_children {
                                                        for (table_ix, table) in schema.tables.iter().enumerate() {
                                                            table_items.push(
                                                                SidebarMenuItem::new(SharedString::from(table.clone()))
                                                                    .icon(IconName::SquareTerminal)
                                                                    .id(("pg-table", conn_ix * 10000 + schema_ix * 100 + table_ix))
                                                            );
                                                        }
                                                    }

                                                    schema_items.push(
                                                        SidebarMenuItem::new(SharedString::from(schema_name_for_click.clone()))
                                                            .icon(IconName::Folder)
                                                            .active(schema.expanded)
                                                            .id(("pg-schema", conn_ix * 100 + schema_ix))
                                                            .context_menu({
                                                                let schema_name = schema_name_for_menu.clone();
                                                                let key = key_for_schema_menu.clone();
                                                                move |menu, _window, _cx| {
                                                                    log::info!("Creating context menu for PostgreSQL schema: {}", schema_name);
                                                                    menu.menu("New Query", Box::new(crate::app::NewQueryForPostgresSchema {
                                                                        connection_key: key.clone(),
                                                                        schema_name: schema_name.clone(),
                                                                    }))
                                                                }
                                                            })
                                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                                log::info!("Clicked on PostgreSQL schema: {}", schema_name_for_click);
                                                                // Find and toggle the schema expansion
                                                                if let Some((_, schemas, _)) = this.pg_connections.get_mut(&key_for_schema_click) {
                                                                    if let Some(schema) = schemas.iter_mut().find(|s| s.name == schema_name_for_click) {
                                                                        schema.expanded = !schema.expanded;
                                                                    }
                                                                }
                                                                cx.notify();
                                                            }))
                                                            .children(table_items)
                                                    );
                                                }
                                            }

                                            items.push(
                                                SidebarMenuItem::new(SharedString::from(display_name_for_click.clone()))
                                                    .icon(IconName::Postgresql)
                                                    .active(*expanded)
                                                    .id(("pg-connection", conn_ix))
                                                    .context_menu({
                                                        let display_name = display_name_for_menu.clone();
                                                        let key = key_for_menu.clone();
                                                        move |menu, _window, _cx| {
                                                            log::info!("Creating context menu for PostgreSQL connection: {}", display_name);
                                                            menu.menu("New Query", Box::new(crate::app::NewQueryForPostgresConnection {
                                                                connection_key: key.clone(),
                                                                display_name: display_name.clone(),
                                                            }))
                                                        }
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        log::info!("Clicked on PostgreSQL connection: {}", display_name_for_click);
                                                        // Toggle expansion for this connection
                                                        if let Some((_display_name, schemas, expanded)) = this.pg_connections.get_mut(&key_for_click) {
                                                            let was_expanded = *expanded;
                                                            *expanded = !*expanded;

                                                            // Load schemas lazily when expanding for the first time
                                                            if !was_expanded && *expanded && schemas.is_empty() {
                                                                this.load_postgres_schemas(key_for_click.clone(), display_name_for_click.clone(), cx);
                                                            }
                                                        }
                                                        cx.notify();
                                                    }))
                                                    .children(schema_items)
                                            );
                                        }
                                        items
                                    }),
                            ),
                        ),
                    ),
            )
            .child(
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
                                                            let _connection_string = format!(
                                                                "postgresql://{}:{}@{}:{}/{}",
                                                                username, password, host, port, database
                                                            );

                                                            // Save to database
                                                            crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                                                                if let Some(db) = app_db.read().await.as_ref() {
                                                                    db.save_connection(&conn_data).await.map_err(|e| {
                                                                        anyhow::anyhow!("Failed to save connection: {}", e)
                                                                    })
                                                                } else {
                                                                    Err(anyhow::anyhow!("App database not initialized"))
                                                                }
                                                            })
                                                            .detach();

                                                            // TODO: Add the connection to sidebar's pg_connections
                                                            // This would require passing a handle to the sidebar
                                                            // For now, the user can restart the app to see the new connection

                                                            window.push_notification("PostgreSQL connection saved successfully", cx);
                                                        } else {
                                                            window.push_notification("Missing PostgreSQL connection details", cx);
                                                            return false;
                                                        }
                                                    } else {
                                                        // SQLite or other database types
                                                        crate::gpui_tokio::Tokio::spawn_result(cx, async move {
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
            )
    }
}
