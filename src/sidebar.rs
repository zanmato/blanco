use crate::connection_sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui::{
    div, prelude::FluentBuilder as _, Action, App, Context, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Window,
};
use gpui_component::{button::Button, h_flex, v_flex, ActiveTheme, IconName, Side};
use std::collections::HashMap;

use crate::app::OpenNewConnectionModal;
use crate::connection::Connection;
use crate::db_service::{DbService, PgConnectionKey};
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
}

impl ConnectionSidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = Self {
            focus_handle: cx.focus_handle(),
            connections: Connection::new_mock(),
            test_db_tables: Vec::new(),
            collapsed: false,
            test_db_expanded: true, // Start expanded
            pg_connections: HashMap::new(),
        };

        // Load test database tables asynchronously
        let db_service = DbService::global(cx).clone();
        let user_db = db_service.user_db_handle();

        let task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            // Give the database a moment to initialize
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            let db = user_db.read().await;
            info!(
                "Sidebar: Loading SQLite tables, connected: {}",
                db.is_connected()
            );

            match db.get_tables().await {
                Ok(tables) => {
                    info!("Sidebar: Loaded {} SQLite tables", tables.len());
                    Ok(tables)
                }
                Err(e) => {
                    error!("Sidebar: Failed to load SQLite tables: {}", e);
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

        // Load existing PostgreSQL connections asynchronously
        let db_service_clone = db_service.clone();
        let pg_task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            // Wait for connections to initialize
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

            let connections = db_service_clone.get_all_pg_connections().await;
            let mut connection_data = Vec::new();

            for (key, manager) in connections {
                let display_name = format!("{}@{}:{}/{}", key.username, key.host, key.port, key.database);
                
                info!("Sidebar: Loading schemas for PostgreSQL connection: {}", display_name);
                match manager.get_schemas().await {
                    Ok(schemas) => {
                        info!("Sidebar: Found {} schemas for connection: {}", schemas.len(), display_name);
                        let mut schema_nodes = Vec::new();

                        for schema_name in schemas {
                            debug!("Loading tables for schema: {}", schema_name);
                            match manager.get_tables(&schema_name).await {
                                Ok(tables) => {
                                    debug!("Schema '{}' has {} tables", schema_name, tables.len());
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
                        connection_data.push((key, (display_name, schema_nodes, true)));
                    }
                    Err(e) => {
                        error!("Sidebar: Failed to load schemas for connection {}: {}", display_name, e);
                    }
                }
            }
            
            Ok(connection_data)
        });

        cx.spawn(async move |handle, cx| {
            if let Ok(connection_data) = pg_task.await {
                if let Some(sidebar) = handle.upgrade() {
                    let _ = sidebar.update(cx, |sidebar, cx| {
                        for (key, data) in connection_data {
                            sidebar.pg_connections.insert(key, data);
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();

        sidebar
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.collapsed = collapsed;
        cx.notify();
    }

    /// Add a new PostgreSQL connection and load its schemas
    pub fn add_postgres_connection(&mut self, connection_string: &str, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let connection_string = connection_string.to_string();
        
        let task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            match PgConnectionKey::from_connection_string(&connection_string) {
                Ok(key) => {
                    let display_name = format!("{}@{}:{}/{}", key.username, key.host, key.port, key.database);
                    
                    // Get or create connection
                    let manager = db_service.get_or_create_pg_connection(&connection_string).await?;
                    
                    // Load schemas
                    match manager.get_schemas().await {
                        Ok(schemas) => {
                            info!("Loaded {} schemas for connection: {}", schemas.len(), display_name);
                            let mut schema_nodes = Vec::new();

                            for schema_name in schemas {
                                debug!("Loading tables for schema: {}", schema_name);
                                match manager.get_tables(&schema_name).await {
                                    Ok(tables) => {
                                        debug!("Schema '{}' has {} tables", schema_name, tables.len());
                                        let mut node = SchemaNode::new(schema_name.clone());
                                        node.tables = tables;
                                        node.expanded = schema_name == "public"; // Expand public by default
                                        schema_nodes.push(node);
                                    }
                                    Err(e) => {
                                        error!("Failed to load tables for schema '{}': {}", schema_name, e);
                                    }
                                }
                            }
                            
                            Ok(Some((key, (display_name, schema_nodes, true))))
                        }
                        Err(e) => {
                            error!("Failed to load schemas for connection {}: {}", display_name, e);
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
            if let Ok(result) = task.await {
                if let Some((key, data)) = result {
                    if let Some(sidebar) = handle.upgrade() {
                        let _ = sidebar.update(cx, |sidebar, cx| {
                            sidebar.pg_connections.insert(key, data);
                            cx.notify();
                        });
                    }
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
                                    .child(
                                        SidebarMenuItem::new(SharedString::from("Test Database"))
                                            .icon(IconName::Building2)
                                            .active(self.test_db_expanded)
                                            .id("test-database")  // Unique ID for Test Database
                                            .context_menu({
                                                let table_count = self.test_db_tables.len();
                                                log::info!("SETTING UP context menu for Test Database with {} tables", table_count);
                                                move |menu, window, cx| {
                                                    log::info!("BUILDING context menu for Test Database with {} tables", table_count);
                                                    let result = menu.menu("New Query", Box::new(crate::app::NewQueryForConnection {
                                                        connection_name: "Test Database".to_string(),
                                                        connection_type: crate::app::ConnectionType::SQLite,
                                                    }));
                                                    log::info!("Context menu for Test Database BUILT");
                                                    result
                                                }
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                log::info!("Test Database clicked");
                                                this.test_db_expanded = !this.test_db_expanded;
                                                cx.notify();
                                            }))
                                            .children(self.test_db_tables.iter().enumerate().map(|(ix, table)| {
                                                SidebarMenuItem::new(SharedString::from(table.clone()))
                                                    .icon(IconName::SquareTerminal)
                                                    .id(("test-db-table", ix))  // Unique ID for each table
                                            })),
                                    )
                                    // PostgreSQL Connections
                                    .children({
                                        let mut items = Vec::new();
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
                                                                move |menu, window, cx| {
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
                                                    .icon(IconName::Globe)
                                                    .active(*expanded)
                                                    .id(("pg-connection", conn_ix))
                                                    .context_menu({
                                                        let display_name = display_name_for_menu.clone();
                                                        let key = key_for_menu.clone();
                                                        move |menu, window, cx| {
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
                                                        if let Some((_, _, ref mut expanded)) = this.pg_connections.get_mut(&key_for_click) {
                                                            *expanded = !*expanded;
                                                        }
                                                        cx.notify();
                                                    }))
                                                    .children(schema_items)
                                            );
                                        }
                                        items
                                    })
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
                            .icon(IconName::Plus)
                            .label("New Connection")
                            .on_click(cx.listener(|_this, _event, window, cx| {
                                // Dispatch the action to open the connection modal
                                let action = OpenNewConnectionModal;
                                window.dispatch_action(action.boxed_clone(), cx);
                            }))
                    ),
            )
    }
}
