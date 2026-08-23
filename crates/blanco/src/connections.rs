mod data_loading;
mod delegate;
mod tree_builder;

pub use data_loading::DatabaseMetadata;
pub use delegate::ConnectionsTreeDelegate;

use crate::app::{CreateNewQueryTab, CreateNewScriptTab};
use crate::connection_credentials;
use crate::export::modal::ExportModal;
use crate::import::modal::ImportModal;
use crate::result_ext::ResultExt;
use app_database::{AppDatabase, ConnectionData, EnvironmentType};
use blanco_core::DatabaseService as DatabaseServiceTrait;
use blanco_ui::IconName;
use blanco_ui::tree::{Tree, TreeItem, TreeState};
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled,
    Subscription, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, WindowExt,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputEvent, InputState},
    notification::NotificationType,
};

#[derive(Clone, Debug)]
pub enum ConnectionsPanelEvent {
    EditConnection {
        connection_data: Box<ConnectionData>,
    },
    /// The initial async connection load finished. Connections load off the foreground
    /// thread, so anything that depends on whether the user has connections (like the
    /// first-run dialog) must wait for this instead of reading `connections` at startup.
    FirstLoadCompleted,
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
    /// Free-text filter over the tree. Only already loaded children are
    /// searched, so a connection has to be expanded before its tables show up.
    filter_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    pub loaded_connections: std::collections::HashSet<i64>,
    expanded_connections: std::collections::HashSet<i64>, // Track which connections are expanded
    // Track which object-type category folders (Tables, Views, ...) are
    // expanded, keyed by the category tree item id.
    expanded_categories: std::collections::HashSet<String>,
    // Live per-database connection status, keyed by connection id. This is the
    // single source of truth for the "connected" icon color: it is read while
    // *building* tree items so that a rebuild (or the tree widget
    // re-materializing children on expand/collapse) always produces the same
    // colors, instead of being patched onto the flattened entries afterwards.
    connected_databases: std::collections::HashMap<i64, std::collections::HashSet<String>>,
    initial_load_completed: bool,
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

    /// Script tabs are scoped to the connection/database, not to the clicked
    /// object, so this is available on every node that carries a database.
    fn create_new_script_tab_action(&self) -> CreateNewScriptTab;
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
                inspect_key: false,
            }),
        }
    }

    fn create_new_script_tab_action(&self) -> CreateNewScriptTab {
        CreateNewScriptTab {
            connection_id: self.connection_id,
            connection_name: self.connection_name.clone(),
            db_type: self.db_type,
            database_name: self.database_name.clone().unwrap_or_default(),
            schema_name: self.schema_name.clone(),
            environment_type: self.environment_type,
        }
    }
}

impl ConnectionsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter..."));
        let filter_subscription =
            cx.subscribe(&filter_input, |this, _input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.rebuild_tree(cx);
                    cx.notify();
                }
            });
        let database_metadata = std::collections::HashMap::new();
        let loaded_connections = std::collections::HashSet::new();
        let expanded_connections = std::collections::HashSet::new();
        let expanded_categories = std::collections::HashSet::new();
        let connected_databases = std::collections::HashMap::new();

        // Create delegate with parent reference
        let panel_entity = cx.entity();
        let delegate = ConnectionsTreeDelegate::new(&panel_entity);

        let tree_state = cx.new(|cx| TreeState::new(delegate, cx));

        let mut panel = Self {
            connections: Vec::new(),
            _selected_connection_id: None,
            database_metadata,
            tree_state,
            filter_input,
            _subscriptions: vec![filter_subscription],
            loaded_connections,
            expanded_connections,
            expanded_categories,
            connected_databases,
            initial_load_completed: false,
        };

        panel.reload_connections(cx);
        panel
    }

    pub fn reload_connections(&mut self, cx: &mut Context<Self>) {
        let app_database = AppDatabase::global(cx).clone();
        let load = gpui_tokio::Tokio::spawn_result(cx, async move {
            app_database
                .load_connections()
                .await
                .map_err(anyhow::Error::from)
        });
        cx.spawn(async move |this_handle, cx| {
            let Some(connections) = load.await.log_err() else {
                return;
            };
            let hydrate =
                cx.update(|cx| connection_credentials::hydrate_connections(connections, cx));
            let connections = match hydrate.await {
                Ok(connections) => connections,
                Err(error) => {
                    tracing::error!("Failed to load connection credentials: {error}");
                    return;
                }
            };
            this_handle
                .update(cx, |this, cx| {
                    this.connections = connections;
                    this.update_tree_items(cx);
                    if !this.initial_load_completed {
                        this.initial_load_completed = true;
                        cx.emit(ConnectionsPanelEvent::FirstLoadCompleted);
                    }
                    cx.notify();
                })
                .log_err();
        })
        .detach();
    }

    /// Refresh the live connection statuses, then rebuild the tree from them.
    ///
    /// Statuses have to be fetched asynchronously, so they are cached on the
    /// panel and the tree is only rebuilt once they land. Rebuilding first and
    /// patching the flattened entries afterwards (as this used to do) both
    /// produced a visible flash of stale colors and left the *nested* child
    /// items unpatched, so the tree widget resurrected the stale colors the
    /// next time it re-materialized children on expand/collapse.
    fn update_tree_items(&mut self, cx: &mut Context<Self>) {
        let db_service = DatabaseService::global(cx).clone();

        cx.spawn(async move |this_handle, cx| {
            let connection_statuses = db_service.get_active_connection_statuses().await;

            this_handle
                .update(cx, |this, cx| {
                    match connection_statuses {
                        Ok(statuses) => {
                            this.connected_databases.clear();
                            for ((connection_id, database_name), status) in statuses {
                                if status.is_connected {
                                    this.connected_databases
                                        .entry(connection_id)
                                        .or_default()
                                        .insert(database_name);
                                }
                            }
                        }
                        // Keep the last known statuses rather than blanking
                        // every icon on a transient failure.
                        Err(e) => tracing::error!("Failed to get connection statuses: {}", e),
                    }

                    this.rebuild_tree(cx);
                    cx.notify();
                })
                .log_err();
        })
        .detach();
    }

    /// Rebuild the tree items from the panel's current state. All expansion and
    /// connection state is read from the panel here, so the result is fully
    /// determined by that state and rebuilding is idempotent.
    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let tree_items: Vec<TreeItem<TreeItemMetadata>> = self
            .connections
            .iter()
            .map(|connection| {
                self.build_connection_tree_item_with_expand(
                    connection,
                    self.expanded_connections
                        .contains(&connection.id.unwrap_or(0)),
                    cx,
                )
            })
            .collect();

        let filter = self.filter_input.read(cx).value().trim().to_lowercase();
        let tree_items = if filter.is_empty() {
            tree_items
        } else {
            filter_tree_items(&tree_items, &filter)
        };

        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items, cx);
        });
    }

    /// Whether a specific database within a connection currently has a live
    /// connection. Connections themselves are tracked under the `"default"`
    /// database name by `DatabaseService`.
    fn is_database_connected(&self, connection_id: i64, database_name: &str) -> bool {
        self.connected_databases
            .get(&connection_id)
            .is_some_and(|databases| databases.contains(database_name))
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

    fn get_connection_icon(
        &self,
        connection_id: i64,
        db_type: database::DatabaseType,
        cx: &Context<Self>,
    ) -> TreeItemIcon {
        // The icon shows the driver type; connection state is conveyed purely
        // through color. A loaded connection is by definition connected, and
        // `DatabaseService` files connection-level handles under the "default"
        // database name.
        let connected = self.loaded_connections.contains(&connection_id)
            || self.is_database_connected(connection_id, "default");
        let color = if connected {
            cx.theme().primary.into()
        } else {
            cx.theme().foreground.into()
        };
        TreeItemIcon {
            icon: connection_type_icon(db_type),
            color,
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
                        // Already loaded. The tree widget toggled this node
                        // itself on mouse-down (before this click handler), so
                        // we only mirror the resulting state; toggling again
                        // here would fight it, and rebuilding would re-apply
                        // stored state to every other node.
                        self.record_connection_expansion(connection_id, item_id, cx);
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
                            self.record_database_expansion(
                                connection_id,
                                database_name,
                                item_id,
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
                            self.record_schema_expansion(
                                connection_id,
                                database_name,
                                schema_name,
                                item_id,
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

    /// Read back the expansion state the tree widget applied to `item_id` when
    /// it handled the mouse-down that preceded the current click.
    ///
    /// The widget owns the actual toggle; every `record_*_expansion` below
    /// mirrors its result into the panel's own state so it survives the next
    /// tree rebuild. Mirroring instead of independently toggling is what keeps
    /// the two in sync: if the panel toggled on its own, any click the widget
    /// swallowed (or handled for a node the panel didn't expect to be a folder)
    /// would leave the panel one toggle out of phase, and the next rebuild
    /// would snap the node back open.
    fn widget_expansion(&self, item_id: &str, cx: &Context<Self>) -> bool {
        self.tree_state
            .read(cx)
            .entries()
            .iter()
            .find(|entry| entry.item().id == item_id)
            .is_some_and(|entry| entry.is_expanded())
    }

    /// Record a connection's expansion after the tree widget toggled it.
    fn record_connection_expansion(
        &mut self,
        connection_id: i64,
        item_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.widget_expansion(item_id, cx) {
            self.expanded_connections.insert(connection_id);
        } else {
            self.expanded_connections.remove(&connection_id);
        }
    }

    /// Record a database's expansion after the tree widget toggled it.
    fn record_database_expansion(
        &mut self,
        connection_id: i64,
        database_name: &str,
        item_id: &str,
        cx: &mut Context<Self>,
    ) {
        let expanded = self.widget_expansion(item_id, cx);
        if let Some(metadata) = self.database_metadata.get_mut(&connection_id)
            && let Some(database) = metadata
                .databases
                .iter_mut()
                .find(|db| db.name == database_name)
        {
            database.is_expanded = expanded;
        }
    }

    /// Record a schema's expansion after the tree widget toggled it.
    fn record_schema_expansion(
        &mut self,
        connection_id: i64,
        database_name: &str,
        schema_name: &str,
        item_id: &str,
        cx: &mut Context<Self>,
    ) {
        let expanded = self.widget_expansion(item_id, cx);
        if let Some(metadata) = self.database_metadata.get_mut(&connection_id)
            && let Some(database) = metadata
                .databases
                .iter_mut()
                .find(|db| db.name == database_name)
            && let Some(schema) = database.schemas.iter_mut().find(|s| s.name == schema_name)
        {
            schema.is_expanded = expanded;
        }
    }

    /// Record an object-type category folder's expansion after the tree widget
    /// has toggled it, so the state survives future tree rebuilds. The set
    /// stores only categories toggled *away* from their default (Tables open,
    /// the rest closed), matching the XOR used when building the tree.
    fn record_category_expansion(&mut self, category_key: &str, cx: &mut Context<Self>) {
        let expanded = self.widget_expansion(category_key, cx);
        let default_expanded = category_key.ends_with(":tables");
        if expanded == default_expanded {
            self.expanded_categories.remove(category_key);
        } else {
            self.expanded_categories.insert(category_key.to_string());
        }
    }

    /// Refresh a connection by clearing cached metadata and reloading from the server
    /// Ask for confirmation (and a new name for renames), then run a DDL
    /// operation on a table or view and refresh the connection's tree. The
    /// read-only flag is enforced by the connection itself; this dialog is the
    /// confirmation step, so PROD connections get it like everyone else.
    pub fn confirm_table_operation(
        &mut self,
        metadata: TreeItemMetadata,
        operation: blanco_core::ddl::TableOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use blanco_core::ddl::TableOperation;

        let Some(table_name) = metadata.table_name.clone() else {
            return;
        };
        let entity = match metadata.kind {
            TreeItemKind::View => blanco_core::EntityType::View,
            TreeItemKind::MaterializedView => blanco_core::EntityType::MaterializedView,
            _ => blanco_core::EntityType::Table,
        };
        let is_rename = matches!(operation, TableOperation::Rename { .. });
        let new_name_input = is_rename.then(|| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("New name")
                    .default_value(&table_name)
            })
        });
        let is_prod = metadata.environment_type == Some(EnvironmentType::Prod);
        let prod_note = if is_prod { " on a PROD connection" } else { "" };
        let title = format!(
            "{} {}?",
            operation.label(),
            entity.display_name().to_lowercase()
        );
        let description = match &operation {
            TableOperation::Drop => format!(
                "\"{table_name}\" and all of its data will be permanently deleted{prod_note}."
            ),
            TableOperation::Truncate => {
                format!("All rows in \"{table_name}\" will be permanently deleted{prod_note}.")
            }
            TableOperation::Rename { .. } => {
                format!("Enter the new name for \"{table_name}\"{prod_note}.")
            }
        };
        let confirm_label = operation.label();
        let this_handle = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let metadata = metadata.clone();
            let operation = operation.clone();
            let new_name_input = new_name_input.clone();
            let this_handle = this_handle.clone();
            let mut dialog = dialog.title(title.clone()).child(description.clone());
            if let Some(input) = &new_name_input {
                dialog = dialog.child(Input::new(input));
            }
            dialog.footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new()
                            .child(Button::new("table-op-cancel").label("Cancel").outline()),
                    )
                    .child(
                        DialogAction::new().child(
                            Button::new("table-op-confirm")
                                .danger()
                                .label(confirm_label)
                                .on_click(move |_, window, cx| {
                                    let operation = match (&operation, &new_name_input) {
                                        (TableOperation::Rename { .. }, Some(input)) => {
                                            TableOperation::Rename {
                                                new_name: input.read(cx).value().to_string(),
                                            }
                                        }
                                        _ => operation.clone(),
                                    };
                                    this_handle
                                        .update(cx, |this, cx| {
                                            this.run_table_operation(
                                                metadata.clone(),
                                                entity,
                                                operation,
                                                window,
                                                cx,
                                            );
                                        })
                                        .log_err();
                                }),
                        ),
                    ),
            )
        });
    }

    fn run_table_operation(
        &mut self,
        metadata: TreeItemMetadata,
        entity: blanco_core::EntityType,
        operation: blanco_core::ddl::TableOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table_name) = metadata.table_name.clone() else {
            return;
        };
        let Some(sql) = blanco_core::ddl::table_operation_sql(
            metadata.db_type,
            entity,
            metadata.schema_name.as_deref(),
            &table_name,
            &operation,
        ) else {
            window.push_notification(
                (
                    NotificationType::Error,
                    format!("{} is not supported for this object", operation.label()),
                ),
                cx,
            );
            return;
        };

        let db_service = DatabaseService::global(cx).clone();
        let connection_id = metadata.connection_id;
        let database_name = metadata.database_name;
        let label = operation.label();
        cx.spawn_in(window, async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let connection = db_service
                    .get_or_create_connection(connection_id, database_name.as_deref())
                    .await?;
                connection
                    .execute_write(&sql, database_name.as_deref(), &[])
                    .await?;
                Ok(())
            })
            .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    window.push_notification(
                        (
                            NotificationType::Success,
                            format!("{label} {table_name} succeeded"),
                        ),
                        cx,
                    );
                    this.refresh_connection(connection_id, cx);
                }
                Err(error) => {
                    tracing::error!("{label} {table_name} failed: {error:#}");
                    window.push_notification(
                        (
                            NotificationType::Error,
                            format!("{label} {table_name} failed: {error:#}"),
                        ),
                        cx,
                    );
                }
            })
            .log_err();
        })
        .detach();
    }

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
                    this.forget_connection(connection_id);
                    this.update_tree_items(cx);
                    cx.notify();
                })
                .log_err();
        })
        .detach();
    }

    /// Drop all cached state for a connection that is no longer live. The
    /// cached object tree has to go with it: leaving it behind keeps the node
    /// rendering as an expandable folder, so the tree widget would keep
    /// toggling it open on click while the panel treated the click as a
    /// reconnect request.
    fn forget_connection(&mut self, connection_id: i64) {
        self.loaded_connections.remove(&connection_id);
        self.expanded_connections.remove(&connection_id);
        self.database_metadata.remove(&connection_id);
        self.connected_databases.remove(&connection_id);
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
        if self.loaded_connections.contains(&connection_id) {
            // Children are already loaded, but the query may have opened a
            // database that wasn't connected before, so refresh live status.
            self.update_tree_items(cx);
        } else {
            self.load_connection_children(connection_id, false, cx);
        }
        cx.notify();
    }

    /// Mark a connection as disconnected after the database service detected a
    /// dropped connection. Unlike `disconnect_connection`, this only updates UI
    /// state: the service has already evicted the underlying connection.
    pub fn mark_connection_disconnected(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        if !self.loaded_connections.contains(&connection_id) {
            return;
        }
        tracing::info!("Marking connection {} as disconnected", connection_id);
        self.forget_connection(connection_id);
        self.update_tree_items(cx);
        cx.notify();
    }

    fn render_connections_tree(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        Tree::new(&self.tree_state)
    }

    fn render_filter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(Input::new(&self.filter_input).small())
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
                                    let credential_tasks =
                                        connection_credentials::start_deleting_connection(
                                            connection_id,
                                            cx,
                                        );
                                    cx.spawn(async move |cx| {
                                        if let Err(e) =
                                            app_database.delete_connection(connection_id).await
                                        {
                                            tracing::error!("Failed to delete connection: {}", e);
                                            return;
                                        }
                                        if let Err(error) =
                                            connection_credentials::finish_writing(credential_tasks)
                                                .await
                                        {
                                            tracing::error!(
                                                "Failed to delete connection credentials: {error}"
                                            );
                                        }

                                        this_handle
                                            .update(cx, |this, cx| {
                                                this.connections
                                                    .retain(|c| c.id != Some(connection_id));
                                                this.forget_connection(connection_id);
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
            .child(self.render_filter(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_connections_tree(cx)),
            )
    }
}

/// Keep items whose label matches `needle` (lowercased) or that have a matching
/// descendant. Ancestors of a match are expanded so the match is visible. The user's expansion sets are untouched, so clearing
/// the filter restores the previous layout.
fn filter_tree_items(
    items: &[TreeItem<TreeItemMetadata>],
    needle: &str,
) -> Vec<TreeItem<TreeItemMetadata>> {
    items
        .iter()
        .filter_map(|item| {
            let matching_children = filter_tree_items(&item.children, needle);
            let label_matches = item.label.to_lowercase().contains(needle);
            if !label_matches && matching_children.is_empty() {
                return None;
            }
            // A matching item keeps its full subtree so the user can still
            // browse into it; otherwise only the matching branch survives and
            // is expanded so the match is visible.
            let (children, expanded) = if matching_children.is_empty() {
                (item.children.as_ref().clone(), item.is_expanded())
            } else {
                (matching_children, true)
            };
            let filtered =
                TreeItem::new(item.id.clone(), item.label.clone(), item.metadata.clone())
                    .disabled(item.is_disabled())
                    .expanded(expanded)
                    .children(children);
            Some(filtered)
        })
        .collect()
}

impl EventEmitter<ConnectionsPanelEvent> for ConnectionsPanel {}

/// Map a connection's driver to its brand icon. Each icon renders as a single
/// color silhouette (the SVG is tinted by the tree item's color), so the same
/// icon serves both the connected and disconnected states, distinguished only
/// by color.
fn connection_type_icon(db_type: database::DatabaseType) -> IconName {
    match db_type {
        database::DatabaseType::SQLite => IconName::Sqlite,
        database::DatabaseType::PostgreSQL => IconName::Postgresql,
        database::DatabaseType::MySQL => IconName::Mysql,
        database::DatabaseType::ClickHouse => IconName::Clickhouse,
        database::DatabaseType::MsSql => IconName::MsSql,
        database::DatabaseType::Redis => IconName::Redis,
    }
}

/// Theme color for an environment chip. Lives here rather than on
/// `EnvironmentType` so the app-database crate stays free of UI dependencies.
pub fn environment_color(environment_type: EnvironmentType, cx: &App) -> gpui::Rgba {
    match environment_type {
        EnvironmentType::Dev => cx.theme().blue.into(),
        EnvironmentType::Test => cx.theme().green.into(),
        EnvironmentType::Prod => cx.theme().red.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> TreeItemMetadata {
        TreeItemMetadata {
            connection_id: 1,
            connection_name: "test".to_string(),
            kind: TreeItemKind::Table,
            db_type: database::DatabaseType::SQLite,
            database_name: None,
            schema_name: None,
            table_name: None,
            icon: TreeItemIcon {
                icon: IconName::Database,
                color: gpui::rgba(0),
            },
            environment_type: None,
            loading: false,
            size_bytes: None,
            relative_size: None,
        }
    }

    fn item(id: &str, children: Vec<TreeItem<TreeItemMetadata>>) -> TreeItem<TreeItemMetadata> {
        TreeItem::new(id.to_string(), id.to_string(), metadata()).children(children)
    }

    #[test]
    fn filter_keeps_matching_descendants_and_expands_their_ancestors() {
        let tree = vec![
            item(
                "local",
                vec![item(
                    "public",
                    vec![item("orders", vec![]), item("users", vec![])],
                )],
            ),
            item(
                "staging",
                vec![item("public", vec![item("invoices", vec![])])],
            ),
        ];

        let filtered = filter_tree_items(&tree, "user");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id.as_ref(), "local");
        assert!(filtered[0].is_expanded());
        let schema = &filtered[0].children[0];
        assert!(schema.is_expanded());
        assert_eq!(schema.children.len(), 1);
        assert_eq!(schema.children[0].id.as_ref(), "users");
    }

    #[test]
    fn filter_keeps_a_matching_parent_with_all_its_children() {
        let tree = vec![item(
            "staging",
            vec![item("public", vec![item("invoices", vec![])])],
        )];
        let filtered = filter_tree_items(&tree, "stag");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].children.len(), 1);
        assert!(
            !filtered[0].is_expanded(),
            "a match without matching children keeps its own state"
        );
    }
}
