use crate::connection_sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui::{
    div, prelude::FluentBuilder as _, Action, App, Context, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Window,
};
use gpui_component::{button::Button, h_flex, v_flex, ActiveTheme, IconName, Side};

use crate::app::OpenNewConnectionModal;
use crate::connection::Connection;
use crate::db_service::DbService;
use crate::postgres::SchemaNode;
use log::{debug, error, info};

pub struct ConnectionSidebar {
    focus_handle: FocusHandle,
    connections: Vec<Connection>,
    test_db_tables: Vec<String>,
    collapsed: bool,
    test_db_expanded: bool,
    // PostgreSQL hierarchy
    pg_connection_name: String,
    pg_database_name: String,
    pg_schemas: Vec<SchemaNode>,
    pg_expanded: bool,
}

impl ConnectionSidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = Self {
            focus_handle: cx.focus_handle(),
            connections: Connection::new_mock(),
            test_db_tables: Vec::new(),
            collapsed: false,
            test_db_expanded: true, // Start expanded
            pg_connection_name: "PostgreSQL".to_string(),
            pg_database_name: "bylyngamanager".to_string(),
            pg_schemas: Vec::new(),
            pg_expanded: true,
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

        // Load PostgreSQL hierarchy asynchronously
        let pg_db = db_service.pg_db_handle();
        let pg_task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            // Wait for PostgreSQL to initialize
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

            let pg_db_ref = pg_db.read().await;

            if let Some(pg_manager) = pg_db_ref.as_ref() {
                info!("Sidebar: Loading PostgreSQL schemas");
                match pg_manager.get_schemas().await {
                    Ok(schemas) => {
                        info!("Sidebar: Found {} PostgreSQL schemas", schemas.len());
                        let mut schema_nodes = Vec::new();

                        for schema_name in schemas {
                            debug!("Loading tables for schema: {}", schema_name);
                            match pg_manager.get_tables(&schema_name).await {
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
                        Ok(schema_nodes)
                    }
                    Err(e) => {
                        error!("Sidebar: Failed to load PostgreSQL schemas: {}", e);
                        Err(anyhow::anyhow!("{}", e))
                    }
                }
            } else {
                debug!("PostgreSQL not connected");
                Ok(Vec::new())
            }
        });

        cx.spawn(async move |handle, cx| {
            if let Ok(schemas) = pg_task.await {
                if let Some(sidebar) = handle.upgrade() {
                    let _ = sidebar.update(cx, |sidebar, cx| {
                        sidebar.pg_schemas = schemas;
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
                                            .children(self.test_db_tables.iter().map(|table| {
                                                SidebarMenuItem::new(SharedString::from(table.clone()))
                                                    .icon(IconName::SquareTerminal)
                                            })),
                                    )
                                    // PostgreSQL Database
                                    .child(
                                        SidebarMenuItem::new(SharedString::from(format!("{} ({})", self.pg_connection_name, self.pg_database_name)))
                                            .icon(IconName::Globe)
                                            .active(self.pg_expanded)
                                            .context_menu({
                                                let pg_connection_name = self.pg_connection_name.clone();
                                                let pg_database_name = self.pg_database_name.clone();
                                                move |menu, window, cx| {
                                                    log::info!("Creating context menu for PostgreSQL: {} ({})", pg_connection_name, pg_database_name);
                                                    menu.menu("New Query", Box::new(crate::app::NewQueryForConnection {
                                                        connection_name: format!("{} ({})", pg_connection_name, pg_database_name),
                                                        connection_type: crate::app::ConnectionType::PostgreSQL,
                                                    }))
                                                }
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                log::info!("Clicked on sidebar menu item");
                                                this.pg_expanded = !this.pg_expanded;
                                                cx.notify();
                                            }))
                                            .children(self.pg_schemas.iter().map(|schema| {
                                                let schema_name = schema.name.clone();
                                                SidebarMenuItem::new(SharedString::from(schema_name.clone()))
                                                    .icon(IconName::Folder)
                                                    .active(schema.expanded)
                                                    .context_menu({
                                                        let schema_name = schema_name.clone();
                                                        move |menu, window, cx| {
                                                            log::info!("Creating context menu for PostgreSQL schema: {}", schema_name);
                                                            menu.menu("New Query", Box::new(crate::app::NewQueryForPostgresSchema {
                                                                schema_name: schema_name.clone(),
                                                            }))
                                                        }
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        // Find and toggle the schema expansion
                                                        if let Some(schema) = this.pg_schemas.iter_mut().find(|s| s.name == schema_name) {
                                                            schema.expanded = !schema.expanded;
                                                        }
                                                        cx.notify();
                                                    }))
                                                    .children(if schema.expanded {
                                                        schema.tables.iter().map(|table| {
                                                            SidebarMenuItem::new(SharedString::from(table.clone()))
                                                                .icon(IconName::SquareTerminal)
                                                        }).collect()
                                                    } else {
                                                        Vec::new()
                                                    })
                                            }))
                                    )
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
