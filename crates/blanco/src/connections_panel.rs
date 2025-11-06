use gpui::{
    div, prelude::FluentBuilder, Axis, Context, Entity, EventEmitter,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, Styled,
    Window,
};
use gpui_component::{
    h_flex,
    label::Label,
    v_flex, ActiveTheme as _, StyledExt,
};
use blanco_ui::IconName;

use crate::app_database::ConnectionData;
use crate::app_events::AppEvent;
use crate::db_service::DbService;

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

/// Metadata about a database's schemas and tables
#[derive(Debug, Clone)]
pub struct DatabaseMetadata {
    pub connection_id: Option<i64>,
    pub schemas: Vec<DatabaseSchema>,
    pub supports_schemas: bool,
}

pub struct ConnectionsPanel {
    connections: Vec<ConnectionData>,
    selected_connection_id: Option<i64>,
    database_metadata: Option<DatabaseMetadata>,
    expanded_schemas: std::collections::HashSet<String>,
    expanded_connections: std::collections::HashSet<i64>,
}

impl ConnectionsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let panel = Self {
            connections: Vec::new(),
            selected_connection_id: None,
            database_metadata: None,
            expanded_schemas: std::collections::HashSet::new(),
            expanded_connections: std::collections::HashSet::new(),
        };

        // Load initial connections
        let db_service = DbService::global(cx);
        let app_db_handle = db_service.app_db_handle();
        cx.spawn(async move |this_handle, cx| {
            let connections = match *app_db_handle.read().await {
                Some(ref app_db) => match app_db.load_connections().await {
                    Ok(connections) => connections,
                    Err(e) => {
                        log::error!("Failed to load connections: {}", e);
                        vec![]
                    }
                },
                None => {
                    log::warn!("App database not initialized");
                    vec![]
                }
            };

            let _ = this_handle.update(cx, |this, cx| {
                this.connections = connections;
                cx.notify();
            });
        })
        .detach();

        panel
    }

    /// Get connection by ID
    fn get_connection_by_id(&self, connection_id: i64) -> Option<&ConnectionData> {
        self.connections.iter().find(|conn| conn.id == Some(connection_id))
    }

    /// Select a connection and load its schemas and tables
    fn select_connection_and_load_metadata(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        self.selected_connection_id = Some(connection_id);
        self.database_metadata = None; // Clear existing metadata
        cx.notify();

        // Load schemas and tables asynchronously
        let db_service = DbService::global(cx).clone();

        cx.spawn(async move |this_handle, cx| {
            match db_service.get_or_create_connection(connection_id).await {
                Ok(connection) => {
                    log::debug!("Connected to database: {}", connection.get_display_name());

                    // Check if the connection supports schemas
                    let supports_schemas = connection.supports_schemas();

                    let schemas = if supports_schemas {
                        // Load schemas for databases like PostgreSQL
                        match connection.get_schemas().await {
                            Ok(schema_list) => schema_list,
                            Err(e) => {
                                log::error!(
                                    "Failed to load schemas for connection {}: {}",
                                    connection_id,
                                    e
                                );
                                Vec::new()
                            }
                        }
                    } else {
                        // For SQLite, create a single schema entry
                        vec!["main".to_string()]
                    };

                    let mut database_schemas = Vec::new();
                    for schema_name in schemas {
                        let tables = match connection.get_tables(Some(&schema_name)).await {
                            Ok(table_list) => table_list
                                .into_iter()
                                .map(|table_name| DatabaseTable {
                                    name: table_name,
                                    schema: Some(schema_name.clone()),
                                })
                                .collect(),
                            Err(e) => {
                                log::error!(
                                    "Failed to load tables for schema {} in connection {}: {}",
                                    schema_name,
                                    connection_id,
                                    e
                                );
                                Vec::new()
                            }
                        };

                        database_schemas.push(DatabaseSchema {
                            name: schema_name,
                            tables,
                            is_expanded: false,
                        });
                    }

                    let metadata = DatabaseMetadata {
                        connection_id: Some(connection_id),
                        schemas: database_schemas,
                        supports_schemas,
                    };

                    // Update the panel with loaded metadata
                    let schemas = metadata.schemas.iter().map(|s| s.name.clone()).collect();
                    let _ = this_handle.update(cx, |this, cx| {
                        this.database_metadata = Some(metadata);
                        cx.emit(AppEvent::SchemasLoaded {
                            connection_id: Some(connection_id),
                            schemas,
                        });
                        cx.notify();
                    });
                }
                Err(e) => {
                    log::error!("Failed to get connection {}: {}", connection_id, e);
                    let _ = this_handle.update(cx, |this, cx| {
                        this.database_metadata = None;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Toggle the expansion state of a connection
    fn toggle_connection_expansion(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        if self.expanded_connections.contains(&connection_id) {
            self.expanded_connections.remove(&connection_id);
        } else {
            self.expanded_connections.insert(connection_id);
            // Load schemas and tables for this connection when expanding
            self.select_connection_and_load_metadata(connection_id, cx);
        }
        cx.notify();
    }

    /// Check if a connection is expanded
    fn is_connection_expanded(&self, connection_id: i64) -> bool {
        self.expanded_connections.contains(&connection_id)
    }

    /// Toggle the expansion state of a schema
    fn toggle_schema_expansion(&mut self, schema_name: &str, cx: &mut Context<Self>) {
        if self.expanded_schemas.contains(schema_name) {
            self.expanded_schemas.remove(schema_name);
        } else {
            self.expanded_schemas.insert(schema_name.to_string());
        }
        cx.notify();
    }

    /// Check if a schema is expanded
    fn is_schema_expanded(&self, schema_name: &str) -> bool {
        self.expanded_schemas.contains(schema_name)
    }

    fn render_header_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(Label::new("Saved Connections").font_bold().text_sm())
    }

    fn render_connections_tree(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .p_3()
            .flex_1()
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .scrollable(Axis::Vertical)
                    .children(
                        self.connections.iter().enumerate().map(|(index, connection)| {
                            self.render_connection_tree_node(connection, index, cx)
                        })
                    ),
            )
    }

    fn render_connection_tree_node(
        &self,
        connection: &ConnectionData,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = self.selected_connection_id == connection.id;
        let is_expanded = self.is_connection_expanded(connection.id.unwrap_or(0));
        let connection_id = connection.id.unwrap_or(0);

        let display_name = connection.display_name();
        let subtitle = match connection.db_type.as_str() {
            "SQLite" => connection
                .database_path
                .as_ref()
                .map(|path| {
                    std::path::Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(path)
                        .to_string()
                })
                .unwrap_or_else(|| "In-memory".to_string()),
            "PostgreSQL" => format!(
                "{}@{}:{}/{}",
                connection.username.as_deref().unwrap_or(""),
                connection.host.as_deref().unwrap_or("localhost"),
                connection.port.unwrap_or(5432),
                connection.database_name.as_deref().unwrap_or("")
            ),
            _ => connection.db_type.clone(),
        };

        v_flex()
            .w_full()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .w_full()
                    .bg(if is_selected {
                        cx.theme().list_active.opacity(0.2)
                    } else if index % 2 == 0 {
                        cx.theme().list
                    } else {
                        cx.theme().list_even
                    })
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener({
                        let display_name = display_name.clone();
                        move |this, _event, _window, cx| {
                            log::debug!("Connection clicked: {}", display_name);
                            this.toggle_connection_expansion(connection_id, cx);
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener({
                        let display_name = display_name.clone();
                        move |_this, _event, _window, cx| {
                            log::debug!("Create new query tab for connection: {}", display_name);
                            cx.emit(AppEvent::CreateNewQueryTab { connection_id });
                        }
                    }))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .text_color(if is_selected {
                                cx.theme().accent_foreground
                            } else {
                                cx.theme().foreground
                            })
                            .child(
                                // Database icon - connected if metadata is loaded (indicating successful connection)
                                if self.database_metadata.as_ref().map(|m| m.connection_id) == Some(Some(connection_id)) {
                                    IconName::DatabaseConnected
                                } else {
                                    IconName::Database
                                }.view(cx)
                            )
                            .child(
                                Label::new(format!(
                                    "{} {}",
                                    if is_expanded { "▼" } else { "▶" },
                                    display_name
                                ))
                                .text_sm()
                                .font_semibold(),
                            )
                            .child(
                                Label::new(subtitle)
                                    .text_xs()
                                    .text_color(if is_selected {
                                        cx.theme().accent_foreground.opacity(0.7)
                                    } else {
                                        cx.theme().foreground.opacity(0.6)
                                    })
                                    .whitespace_nowrap(),
                            ),
                    ),
            )
            .when(is_expanded, |this| {
                // Render schemas and tables for this connection if metadata is available
                if let Some(metadata) = &self.database_metadata {
                    if metadata.connection_id == Some(connection_id) {
                        this.children(
                            metadata
                                .schemas
                                .iter()
                                .map(|schema| self.render_schema_node(schema, cx)),
                        )
                    } else {
                        this.child(
                            div()
                                .px_6()
                                .py_2()
                                .w_full()
                                .child(
                                    Label::new("Loading schemas...")
                                        .text_xs()
                                        .text_color(cx.theme().foreground.opacity(0.6)),
                                ),
                        )
                    }
                } else {
                    this.child(
                        div()
                            .px_6()
                            .py_2()
                            .w_full()
                            .child(
                                Label::new("Loading schemas...")
                                    .text_xs()
                                    .text_color(cx.theme().foreground.opacity(0.6)),
                            ),
                    )
                }
            })
    }

    
    fn render_schema_node(
        &self,
        schema: &DatabaseSchema,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_expanded = self.is_schema_expanded(&schema.name);

        v_flex()
            .w_full()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .w_full()
                    .bg(if is_expanded {
                        cx.theme().background
                    } else {
                        cx.theme().list
                    })
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let schema_name = schema.name.clone();
                            move |this, _event, _window, cx| {
                                log::debug!("Schema clicked: {}", schema_name);
                                // Toggle schema expansion
                                this.toggle_schema_expansion(&schema_name, cx);

                                let supports_schemas = this
                                    .database_metadata
                                    .as_ref()
                                    .map(|m| m.supports_schemas)
                                    .unwrap_or(false);
                                if supports_schemas {
                                    cx.emit(AppEvent::SchemaSelected {
                                        connection_id: None, // Will be filled by parent
                                        schema: schema_name.clone(),
                                    });
                                }
                            }
                        }),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Label::new(format!(
                                    "{} {}",
                                    if is_expanded { "▼" } else { "▶" },
                                    schema.name
                                ))
                                .text_sm()
                                .font_semibold(),
                            )
                            .child(
                                Label::new(format!("{} tables", schema.tables.len()))
                                    .text_xs()
                                    .text_color(cx.theme().foreground.opacity(0.6)),
                            ),
                    ),
            )
            .when(is_expanded, |this| {
                this.children(
                    schema
                        .tables
                        .iter()
                        .map(|table| self.render_table_node(table, cx)),
                )
            })
    }

    fn render_table_node(&self, table: &DatabaseTable, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_6()
            .py_1()
            .ml_4()
            .w_full()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let table_name = table.name.clone();
                    let schema_name = table.schema.clone();
                    move |_this, _event, _window, cx| {
                        log::debug!(
                            "Table clicked: {} ({})",
                            table_name,
                            schema_name.as_deref().unwrap_or("no schema")
                        );
                        cx.emit(AppEvent::TableSelected {
                            connection_id: None, // Will be filled by parent
                            schema: schema_name.clone(),
                            table: table_name.clone(),
                        });
                    }
                }),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Label::new(&table.name)
                            .text_sm()
                            .text_color(cx.theme().foreground),
                    ),
            )
    }
}

impl EventEmitter<AppEvent> for ConnectionsPanel {}

impl Render for ConnectionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar_primary_foreground)
            .child(self.render_header_section(cx))
            .child(self.render_connections_tree(cx))
    }
}

// Add display_name method to ConnectionData
impl ConnectionData {
    pub fn display_name(&self) -> String {
        self.name.clone()
    }
}