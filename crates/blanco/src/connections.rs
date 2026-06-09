mod data_loading;
mod delegate;
mod tree_builder;

pub use data_loading::DatabaseMetadata;
pub use delegate::ConnectionsTreeDelegate;

use crate::app::CreateNewQueryTab;
use crate::app_database::{AppDatabase, ConnectionData, EnvironmentType};
use crate::export::modal::ExportModal;
use crate::import::modal::ImportModal;
use crate::result_ext::ResultExt;
use blanco_core::DatabaseService as DatabaseServiceTrait;
use blanco_ui::IconName;
use blanco_ui::tree::{Tree, TreeItem, TreeState};
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled,
    Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, WindowExt,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
};

#[derive(Clone, Debug)]
pub enum ConnectionsPanelEvent {
    EditConnection {
        connection_data: Box<ConnectionData>,
    },
}

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

pub struct ConnectionsPanel {
    pub connections: Vec<ConnectionData>,
    _selected_connection_id: Option<i64>,
    database_metadata: std::collections::HashMap<i64, DatabaseMetadata>, // Store metadata per connection
    tree_state: Entity<TreeState<ConnectionsTreeDelegate>>,
    pub loaded_connections: std::collections::HashSet<i64>,
    expanded_connections: std::collections::HashSet<i64>, // Track which connections are expanded
    // Track which object-type category folders (Tables, Views, ...) are
    // expanded, keyed by the category tree item id.
    expanded_categories: std::collections::HashSet<String>,
}

/// Type of tree item in the metadata context
#[derive(Clone, Debug, PartialEq)]
pub enum TreeItemKind {
    Connection,
    Database,
    Schema,
    Table,
    View,
    MaterializedView,
    Procedure,
    Function,
    Trigger,
    /// A grouping folder for one object type (Tables, Views, Functions, ...)
    /// inserted between a schema/connection and its objects.
    Category,
}

/// Metadata for tree items to enable proper context menu actions
#[derive(Clone, Debug)]
pub struct TreeItemMetadata {
    pub connection_id: i64,
    pub connection_name: String,
    pub kind: TreeItemKind,
    pub db_type: database::DatabaseType,
    pub database_name: Option<String>,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub icon: TreeItemIcon,
    pub environment_type: Option<EnvironmentType>,
    pub loading: bool, // Whether this item is currently loading
    /// Reported physical size in bytes; `None` if the backend did not report a
    /// size for this object.
    pub size_bytes: Option<u64>,
    /// Log-normalized size against schema siblings in `[0.0, 1.0]`.
    pub relative_size: Option<f32>,
}

/// Trait to convert metadata into CreateNewQueryTab actions
pub trait CreateNewQueryTabParams {
    fn create_new_query_tab_action(&self) -> Option<CreateNewQueryTab>;
}

impl CreateNewQueryTabParams for TreeItemMetadata {
    fn create_new_query_tab_action(&self) -> Option<CreateNewQueryTab> {
        match self.kind {
            TreeItemKind::Connection | TreeItemKind::Category => None,
            // Procedures/Functions/Triggers don't open a query tab when
            // double-clicked; they get a dedicated DDL tab instead, opened by
            // the panel itself.
            TreeItemKind::Procedure | TreeItemKind::Function | TreeItemKind::Trigger => None,
            TreeItemKind::Database
            | TreeItemKind::Schema
            | TreeItemKind::Table
            | TreeItemKind::View
            | TreeItemKind::MaterializedView => Some(CreateNewQueryTab {
                connection_id: self.connection_id,
                connection_name: self.connection_name.clone(),
                db_type: self.db_type,
                database_name: self.database_name.clone().unwrap_or_default(),
                schema_name: self.schema_name.clone(),
                table_name: self.table_name.clone(),
                environment_type: self.environment_type,
            }),
        }
    }
}

impl ConnectionsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Load initial connections
        let app_database = AppDatabase::global(cx);
        let connections = gpui_tokio::Tokio::handle(cx).block_on(async {
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
        let expanded_categories = std::collections::HashSet::new();

        // Create delegate with parent reference
        let panel_entity = cx.entity();
        let delegate = ConnectionsTreeDelegate::new(&panel_entity);

        let tree_state = cx.new(|cx| TreeState::new(delegate, cx));

        let panel = Self {
            connections,
            _selected_connection_id: None,
            database_metadata,
            tree_state,
            loaded_connections,
            expanded_connections,
            expanded_categories,
        };

        cx.spawn(async |this_handle, cx| {
            this_handle
                .update(cx, |this, cx| {
                    this.update_tree_items(cx);
                    cx.notify();
                })
                .log_err();
        })
        .detach();

        panel
    }

    pub fn reload_connections(&mut self, cx: &mut Context<Self>) {
        let app_database = AppDatabase::global(cx).clone();
        cx.spawn(async move |this_handle, cx| {
            let connections = app_database.load_connections().await;
            this_handle
                .update(cx, |this, cx| match connections {
                    Ok(connections) => {
                        this.connections = connections;
                        this.update_tree_items(cx);
                        cx.notify();
                    }
                    Err(e) => {
                        tracing::error!("Failed to reload connections: {}", e);
                    }
                })
                .log_err();
        })
        .detach();
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
            this_handle
                .update(cx, |this, cx| {
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
                })
                .log_err();
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

    fn get_connection_icon(&self, connection_id: i64, cx: &Context<Self>) -> TreeItemIcon {
        // Reflect the known connected state at build time so rebuilding the tree
        // (on every expand/collapse) doesn't momentarily flip a connected node
        // back to the disconnected icon before `update_connection_status_in_tree`
        // reconciles it. A loaded connection is, by definition, connected.
        if self.loaded_connections.contains(&connection_id) {
            TreeItemIcon {
                icon: IconName::DatabaseConnected,
                color: cx.theme().primary.into(),
            }
        } else {
            TreeItemIcon {
                icon: IconName::Database,
                color: cx.theme().foreground.into(),
            }
        }
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
                }
                TreeItemKind::Table | TreeItemKind::View | TreeItemKind::MaterializedView => {}
                TreeItemKind::Category => {
                    // The tree widget toggles the clicked folder's expansion
                    // itself (on mouse-down, before this click handler). We only
                    // persist the resulting state so it survives later tree
                    // rebuilds. We must NOT rebuild here: a full rebuild
                    // re-applies stored state to every folder and would reopen
                    // unrelated ones.
                    self.record_category_expansion(item_id, cx);
                }
                TreeItemKind::Procedure | TreeItemKind::Function | TreeItemKind::Trigger => {
                    let routine_kind = match metadata.kind {
                        TreeItemKind::Procedure => blanco_core::RoutineKind::Procedure,
                        TreeItemKind::Function => blanco_core::RoutineKind::Function,
                        TreeItemKind::Trigger => blanco_core::RoutineKind::Trigger,
                        _ => unreachable!(),
                    };
                    if let Some(name) = metadata.table_name.clone() {
                        window.dispatch_action(
                            Box::new(crate::app::OpenObjectDdl {
                                kind: routine_kind,
                                connection_id: metadata.connection_id,
                                connection_name: metadata.connection_name.clone(),
                                db_type: metadata.db_type,
                                database_name: metadata.database_name.clone().unwrap_or_default(),
                                schema_name: metadata.schema_name.clone(),
                                object_name: name,
                                environment_type: metadata.environment_type,
                            }),
                            cx,
                        );
                    }
                }
            }
        }
    }

    /// Expand (and connect) a connection idempotently, mirroring the click path
    /// used when a connection node is activated in the tree. Loads the
    /// connection's children on first use (which establishes the DB connection),
    /// and otherwise just ensures the node is expanded.
    pub fn expand_connection(
        &mut self,
        connection_id: i64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.loaded_connections.contains(&connection_id) {
            self.set_item_loading(&format!("connection:{}", connection_id), true, cx);
            self.load_connection_children(connection_id, true, cx);
        } else if !self.expanded_connections.contains(&connection_id) {
            self.expanded_connections.insert(connection_id);
            self.update_tree_items(cx);
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

    /// Record an object-type category folder's expansion after the tree widget
    /// has toggled it, so the state survives future tree rebuilds. The set
    /// stores only categories toggled *away* from their default (Tables open,
    /// the rest closed), matching the XOR used when building the tree.
    fn record_category_expansion(&mut self, category_key: &str, cx: &mut Context<Self>) {
        let expanded = self
            .tree_state
            .read(cx)
            .entries()
            .iter()
            .find(|entry| entry.item().id == category_key)
            .map(|entry| entry.is_expanded())
            .unwrap_or(false);

        let default_expanded = category_key.ends_with(":tables");
        if expanded == default_expanded {
            self.expanded_categories.remove(category_key);
        } else {
            self.expanded_categories.insert(category_key.to_string());
        }
    }

    /// Refresh a connection by clearing cached metadata and reloading from the server
    pub fn refresh_connection(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        tracing::info!("Refreshing connection {}", connection_id);

        self.set_item_loading(&format!("connection:{}", connection_id), true, cx);
        self.loaded_connections.remove(&connection_id);
        self.database_metadata.remove(&connection_id);
        self.update_tree_items(cx);
        cx.notify();

        self.load_connection_children(connection_id, true, cx);
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
            this_handle
                .update(cx, |this, cx| {
                    this.loaded_connections.remove(&connection_id);
                    this.expanded_connections.remove(&connection_id);
                    this.update_tree_items(cx);
                    cx.notify();
                })
                .log_err();
        })
        .detach();
    }

    /// Disconnect a specific database within a connection
    pub fn disconnect_database(
        &mut self,
        connection_id: i64,
        database_name: String,
        cx: &mut Context<Self>,
    ) {
        tracing::info!(
            "Disconnecting database '{}' on connection {}",
            database_name,
            connection_id
        );

        let db_service = DatabaseService::global(cx).clone();

        cx.spawn(async move |this_handle, cx| {
            // Disconnect the specific database
            if let Err(e) = db_service
                .disconnect(connection_id, Some(&database_name))
                .await
            {
                tracing::error!(
                    "Failed to disconnect database '{}' on connection {}: {}",
                    database_name,
                    connection_id,
                    e
                );
            }

            // Update the UI
            this_handle
                .update(cx, |this, cx| {
                    this.update_tree_items(cx);
                    cx.notify();
                })
                .log_err();
        })
        .detach();
    }

    /// Validate and mark a connection as connected after successful query execution
    pub fn validate_connection_as_connected(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        self.load_connection_children(connection_id, false, cx);
        cx.notify();
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
        db_type: database::DatabaseType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(table_name) = table_name {
            // Create the export modal content
            let modal_content = cx.new(|cx| {
                ExportModal::new(
                    connection_id,
                    database_name,
                    schema_name,
                    table_name,
                    db_type,
                    window,
                    cx,
                )
            });

            window.open_dialog(cx, move |dialog, _window, cx| {
                let is_exporting = modal_content.read(cx).is_exporting();
                let modal_clone = modal_content.clone();
                dialog
                    .title("Export Table Data")
                    .h(px(450.0))
                    .child(modal_content.clone())
                    .footer(
                        DialogFooter::new()
                            .child(
                                DialogClose::new()
                                    .child(Button::new("export-cancel").label("Cancel").outline()),
                            )
                            .child(
                                Button::new("export-submit")
                                    .primary()
                                    .label("Export")
                                    .loading(is_exporting)
                                    .on_click({
                                        let modal_for_button = modal_clone;
                                        move |_, window, cx| {
                                            modal_for_button.update(cx, |modal, cx| {
                                                modal.start_export(window, cx);
                                            });
                                        }
                                    }),
                            ),
                    )
            })
        } else {
            tracing::error!("Cannot export: No table name provided");
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn import_table_data(
        &mut self,
        connection_id: i64,
        _connection_name: String,
        database_name: String,
        schema_name: Option<String>,
        table_name: Option<String>,
        db_type: database::DatabaseType,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table_name) = table_name else {
            tracing::error!("Cannot import: No table name provided");
            return;
        };

        let modal_content = cx.new(|cx| {
            ImportModal::new(
                connection_id,
                database_name,
                schema_name,
                table_name,
                db_type,
                window,
                cx,
            )
        });

        window.open_dialog(cx, move |dialog, _window, _cx| {
            dialog
                .title("Import CSV")
                .w(px(780.0))
                .h(px(780.0))
                .overlay_closable(false)
                .child(modal_content.clone())
        })
    }

    /// Set loading state for a tree item by updating the tree entry directly
    pub fn set_item_loading(&mut self, item_id: &str, loading: bool, cx: &mut Context<Self>) {
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
                .title("Confirm Remove Connection")
                .child(format!(
                    "Are you sure you want to remove the connection \"{}\"?",
                    connection_name
                ))
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(Button::new("cancel").label("No").outline()),
                        )
                        .child(DialogAction::new().child(
                            Button::new("confirm").danger().label("Yes").on_click({
                                let this_handle = this_handle.clone();
                                move |_, window, cx| {
                                    let this_handle = this_handle.clone();
                                    let app_database = AppDatabase::global(cx).clone();
                                    cx.spawn(async move |cx| {
                                        if let Err(e) =
                                            app_database.delete_connection(connection_id).await
                                        {
                                            tracing::error!("Failed to delete connection: {}", e);
                                            return;
                                        }

                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.connections
                                                    .retain(|c| c.id != Some(connection_id));
                                                this.loaded_connections.remove(&connection_id);
                                                this.expanded_connections.remove(&connection_id);
                                                this.database_metadata.remove(&connection_id);
                                                this.update_tree_items(cx);
                                                cx.notify();
                                            })
                                            .log_err();
                                    })
                                    .detach();
                                    window.close_dialog(cx);
                                }
                            }),
                        )),
                )
        });
    }

    /// Edit an existing connection by opening the modal with pre-populated data
    pub fn edit_connection(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        // Find the connection data
        let connection_data = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .cloned();

        if let Some(conn_data) = connection_data {
            cx.emit(ConnectionsPanelEvent::EditConnection {
                connection_data: Box::new(conn_data),
            });
        } else {
            tracing::error!("Connection with ID {} not found", connection_id);
        }
    }
}

impl Render for ConnectionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap_2()
            .child(self.render_connections_tree(cx))
    }
}

impl EventEmitter<ConnectionsPanelEvent> for ConnectionsPanel {}

// Add display_name method to ConnectionData
impl ConnectionData {
    pub fn display_name(&self) -> String {
        self.name.clone()
    }
}
