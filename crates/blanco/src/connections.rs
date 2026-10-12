mod data_loading;
mod delegate;
mod reveal;
mod tree_builder;

pub use reveal::RevealTarget;

pub use data_loading::DatabaseMetadata;
pub use delegate::ConnectionsTreeDelegate;

use crate::app::{
    CreateNewQueryTab, CreateNewScriptTab, ExportConnections, ImportConnections,
    OpenNewConnectionModal,
};
use crate::connection_credentials;
use crate::export::modal::ExportModal;
use crate::import::modal::ImportModal;
use crate::result_ext::ResultExt;
use app_database::{AppDatabase, ConnectionData, EnvironmentType};
use blanco_core::ConnectionContext;
use blanco_core::DatabaseService as DatabaseServiceTrait;
use blanco_ui::IconName;
use blanco_ui::tree::{Tree, TreeItem, TreeState};
use database::DatabaseService;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, WindowExt,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::NotificationType,
    v_flex,
};

#[derive(Clone, Debug)]
pub enum ConnectionsPanelEvent {
    EditConnection {
        connection_data: Box<ConnectionData>,
    },
    /// Open the new connection dialog filled in from `connection_data`, an
    /// unsaved copy of an existing connection.
    DuplicateConnection {
        connection_data: Box<ConnectionData>,
    },
    /// Open the new connection dialog to save a temporary connection, filled
    /// in from it. `connection_data` keeps the temporary id, so the caller can
    /// replace the temporary connection once the new one is saved.
    SaveTemporaryConnection {
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
    /// Saved connections followed by the temporary ones.
    pub connections: Vec<ConnectionData>,
    /// Connections opened for this session only (a SQLite file opened
    /// directly), kept apart so a reload from the app database keeps them.
    temporary_connections: Vec<ConnectionData>,
    next_temporary_id: i64,
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
    /// An object the tree is opening its way to, see [`Self::reveal`].
    pending_reveal: Option<reveal::PendingReveal>,
    /// Tables with partitions or inheritance children that the user expanded,
    /// keyed by tree item id. They default to collapsed.
    expanded_tables: std::collections::HashSet<String>,
    // Live per-database connection status, keyed by connection id. This is the
    // single source of truth for the "connected" icon color: it is read while
    // *building* tree items so that a rebuild (or the tree widget
    // re-materializing children on expand/collapse) always produces the same
    // colors, instead of being patched onto the flattened entries afterwards.
    connected_databases: std::collections::HashMap<i64, std::collections::HashSet<String>>,
    initial_load_completed: bool,
    /// Show only connections with this environment tag.
    tag_filter: Option<EnvironmentType>,
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

impl TreeItemKind {
    /// The routine kind behind a procedure/function/trigger node, `None` for
    /// every other node.
    pub fn routine_kind(&self) -> Option<blanco_core::RoutineKind> {
        match self {
            TreeItemKind::Procedure => Some(blanco_core::RoutineKind::Procedure),
            TreeItemKind::Function => Some(blanco_core::RoutineKind::Function),
            TreeItemKind::Trigger => Some(blanco_core::RoutineKind::Trigger),
            _ => None,
        }
    }
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
    /// The comment stored on the object, shown in its tooltip.
    pub comment: Option<String>,
    /// The partitions or inheritance children nested under this table, `None`
    /// for every node without them.
    pub child_tables: Option<ChildTables>,
}

/// What a table node holds when other tables are nested under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildTables {
    /// Direct children only: a sub-partitioned partition counts its own.
    pub count: usize,
    /// Partitions rather than inheritance children.
    pub are_partitions: bool,
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
                table_name: self.table_name.clone(),

                inspect_key: false,

                context: ConnectionContext {
                    connection_id: self.connection_id,
                    connection_name: self.connection_name.clone(),
                    db_type: self.db_type,
                    database_name: self.database_name.clone().unwrap_or_default(),
                    schema_name: self.schema_name.clone(),
                    environment_type: self.environment_type,
                },
            }),
        }
    }

    fn create_new_script_tab_action(&self) -> CreateNewScriptTab {
        CreateNewScriptTab {
            context: ConnectionContext {
                connection_id: self.connection_id,
                connection_name: self.connection_name.clone(),
                db_type: self.db_type,
                database_name: self.database_name.clone().unwrap_or_default(),
                schema_name: self.schema_name.clone(),
                environment_type: self.environment_type,
            },
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
        let expanded_tables = std::collections::HashSet::new();
        let connected_databases = std::collections::HashMap::new();

        // Create delegate with parent reference
        let panel_entity = cx.entity();
        let delegate = ConnectionsTreeDelegate::new(&panel_entity);

        let tree_state = cx.new(|cx| TreeState::new(delegate, cx));

        let mut panel = Self {
            connections: Vec::new(),
            temporary_connections: Vec::new(),
            next_temporary_id: -1,
            _selected_connection_id: None,
            database_metadata,
            tree_state,
            filter_input,
            _subscriptions: vec![filter_subscription],
            loaded_connections,
            expanded_connections,
            expanded_categories,
            pending_reveal: None,
            expanded_tables,
            connected_databases,
            initial_load_completed: false,
            tag_filter: None,
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
            this_handle
                .update(cx, |this, cx| {
                    this.connections = connections;
                    this.connections
                        .extend(this.temporary_connections.iter().cloned());
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

    /// The table selected in the tree, with the context to open it in.
    pub fn selected_table_context(&self, cx: &App) -> Option<(ConnectionContext, String)> {
        let entry = self.tree_state.read(cx).selected_entry()?;
        let metadata = &entry.item().metadata;
        if !matches!(metadata.kind, TreeItemKind::Table) {
            return None;
        }
        let action = metadata.create_new_query_tab_action()?;
        Some((action.context, metadata.table_name.clone()?))
    }

    /// The connection of whatever is selected in the tree.
    pub fn selected_connection_id(&self, cx: &App) -> Option<i64> {
        let entry = self.tree_state.read(cx).selected_entry()?;
        Some(entry.item().metadata.connection_id)
    }

    /// The saved or temporary SQLite connection for the database file at
    /// `path`, if there is one.
    pub fn sqlite_connection_for(&self, path: &std::path::Path) -> Option<ConnectionData> {
        let canonical = |path: &std::path::Path| path.canonicalize().ok();
        let wanted = canonical(path)?;
        self.connections
            .iter()
            .filter(|connection| connection.db_type == database::DatabaseType::SQLite)
            .find(|connection| {
                connection
                    .database_path
                    .as_deref()
                    .and_then(|stored| canonical(std::path::Path::new(stored)))
                    .is_some_and(|stored| stored == wanted)
            })
            .cloned()
    }

    /// Add a temporary connection to the SQLite database at `path`, shown in
    /// the sidebar until it is closed or saved.
    pub fn add_temporary_sqlite_connection(
        &mut self,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> ConnectionData {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut connection = ConnectionData::new_sqlite(name, path.to_string_lossy().into_owned());
        let id = self.next_temporary_id;
        self.next_temporary_id -= 1;
        connection.id = Some(id);
        self.temporary_connections.push(connection.clone());
        self.connections.push(connection.clone());
        self.update_tree_items(cx);
        connection
    }

    /// Drop a temporary connection from the sidebar and close it.
    pub fn close_temporary_connection(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        self.temporary_connections
            .retain(|connection| connection.id != Some(connection_id));
        self.connections
            .retain(|connection| connection.id != Some(connection_id));
        self.disconnect_connection(connection_id, cx);
    }

    /// Ask the app to save a temporary connection through the connection
    /// dialog.
    pub fn save_temporary_connection(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        if let Some(connection) = self
            .temporary_connections
            .iter()
            .find(|connection| connection.id == Some(connection_id))
        {
            cx.emit(ConnectionsPanelEvent::SaveTemporaryConnection {
                connection_data: Box::new(connection.clone()),
            });
        }
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
            .filter(|connection| {
                self.tag_filter
                    .is_none_or(|tag| connection.environment_type == tag)
            })
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

        // A load the reveal waited on has landed; take the next step.
        if self.pending_reveal.is_some() {
            self.advance_reveal(cx);
        }
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
                TreeItemKind::Table => self.record_table_expansion(item_id, cx),
                TreeItemKind::View | TreeItemKind::MaterializedView => {}
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
                    if let (Some(routine_kind), Some(name)) =
                        (metadata.kind.routine_kind(), metadata.table_name.clone())
                    {
                        window.dispatch_action(
                            Box::new(crate::app::OpenObjectDdl {
                                kind: routine_kind,

                                object_name: name,

                                context: ConnectionContext {
                                    connection_id: metadata.connection_id,
                                    connection_name: metadata.connection_name.clone(),
                                    db_type: metadata.db_type,
                                    database_name: metadata
                                        .database_name
                                        .clone()
                                        .unwrap_or_default(),
                                    schema_name: metadata.schema_name.clone(),
                                    environment_type: metadata.environment_type,
                                },
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

    /// Record a table's expansion after the tree widget toggled it. Only
    /// tables with partitions or inheritance children are folders; for any
    /// other table the widget leaves it collapsed and nothing is recorded.
    fn record_table_expansion(&mut self, item_id: &str, cx: &mut Context<Self>) {
        if self.widget_expansion(item_id, cx) {
            self.expanded_tables.insert(item_id.to_string());
        } else {
            self.expanded_tables.remove(item_id);
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
        let prod_note = match metadata.environment_type {
            Some(environment) if environment.confirms_writes() => {
                format!(" on a {environment} connection")
            }
            _ => String::new(),
        };
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
            // Secrets typed at a prompt last until the user disconnects.
            db_service.forget_session_secrets(connection_id).await;

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
        let tag_filter = self.tag_filter;
        let panel = cx.entity().downgrade();
        h_flex()
            .px_2()
            .pb_2()
            .gap_1()
            .child(div().flex_1().child(Input::new(&self.filter_input).small()))
            .child(
                Button::new("connections-tag-filter")
                    .small()
                    .ghost()
                    .map(|button| match tag_filter {
                        Some(tag) => button
                            .label(tag.display_name())
                            .text_color(environment_color(tag, cx)),
                        None => button.icon(IconName::Funnel),
                    })
                    .tooltip("Filter by environment")
                    .dropdown_menu(move |menu, _, _| {
                        let set_filter = |tag: Option<EnvironmentType>| {
                            let panel = panel.clone();
                            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                                panel
                                    .update(cx, |panel, cx| {
                                        panel.tag_filter = tag;
                                        panel.rebuild_tree(cx);
                                        cx.notify();
                                    })
                                    .ok();
                            }
                        };
                        blanco_core::environment_tags().into_iter().fold(
                            menu.item(
                                PopupMenuItem::new("All environments")
                                    .checked(tag_filter.is_none())
                                    .on_click(set_filter(None)),
                            )
                            .separator(),
                            |menu, tag| {
                                menu.item(
                                    PopupMenuItem::new(tag.name.clone())
                                        .checked(tag_filter == Some(tag.id))
                                        .on_click(set_filter(Some(tag.id))),
                                )
                            },
                        )
                    }),
            )
            .child(
                Button::new("connections-menu")
                    .icon(IconName::Ellipsis)
                    .ghost()
                    .small()
                    .tooltip("Connections")
                    .dropdown_menu(|menu, _, _| {
                        menu.menu("New Connection…", Box::new(OpenNewConnectionModal))
                            .separator()
                            .menu("Import Connections…", Box::new(ImportConnections))
                            .menu("Export Connections…", Box::new(ExportConnections))
                    }),
            )
    }

    /// Write saved connections to a JSON file the user picks: all of them, or
    /// only `connection_id`. Secrets are never written.
    pub fn export_connections(
        &mut self,
        connection_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connections: Vec<ConnectionData> = self
            .connections
            .iter()
            .filter(|connection| connection.id.is_some_and(|id| id >= 0))
            .filter(|connection| connection_id.is_none_or(|only| connection.id == Some(only)))
            .cloned()
            .collect();
        if connections.is_empty() {
            return;
        }
        let file_name = match connections.as_slice() {
            [single] => format!("{}.blanco-connections.json", single.name),
            _ => "blanco-connections.json".to_string(),
        };
        let document = app_database::connection_transfer::export_connections(&connections);
        let directory = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let path = cx.prompt_for_new_path(&directory, Some(&file_name));
        let count = connections.len();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let result = smol::fs::write(&path, document).await;
            cx.update(|window, cx| {
                let note = match result {
                    Ok(()) => (
                        NotificationType::Success,
                        SharedString::from(format!(
                            "Exported {count} connection{} to {}.",
                            if count == 1 { "" } else { "s" },
                            path.display()
                        )),
                    ),
                    Err(error) => (
                        NotificationType::Error,
                        SharedString::from(format!("Export failed: {error}")),
                    ),
                };
                window.push_notification(note, cx);
            })
            .log_err();
        })
        .detach();
    }

    /// Read connections from a file the user picks and show what importing it
    /// would add before anything is saved.
    pub fn import_connections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import connections".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let parsed = smol::fs::read_to_string(&path)
                .await
                .map_err(anyhow::Error::from)
                .and_then(|text| app_database::connection_transfer::parse_connections(&text));
            this.update_in(cx, |this, window, cx| match parsed {
                Ok(incoming) => {
                    let plan =
                        app_database::connection_transfer::plan_import(incoming, &this.connections);
                    this.confirm_import(plan, window, cx);
                }
                Err(error) => window.push_notification(
                    (
                        NotificationType::Error,
                        SharedString::from(format!("Import failed: {error:#}")),
                    ),
                    cx,
                ),
            })
            .log_err();
        })
        .detach();
    }

    fn confirm_import(
        &mut self,
        plan: app_database::connection_transfer::ImportPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if plan.add.is_empty() {
            let message = if plan.skipped.is_empty() {
                "The file holds no connections.".to_string()
            } else {
                format!(
                    "Nothing to import: {} already saved.",
                    plan.skipped.join(", ")
                )
            };
            window.push_notification((NotificationType::Info, SharedString::from(message)), cx);
            return;
        }
        let this_handle = cx.entity().downgrade();
        let plan = std::rc::Rc::new(plan);
        window.open_dialog(cx, move |dialog, _, cx| {
            let add_lines = plan.add.iter().map(|(connection, renamed_from)| {
                let line = match renamed_from {
                    Some(original) => format!("{} (renamed from {original})", connection.name),
                    None => connection.name.clone(),
                };
                div().text_sm().child(line)
            });
            dialog
                .title("Import Connections")
                .w(px(480.))
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child(format!(
                            "Add {} connection{}:",
                            plan.add.len(),
                            if plan.add.len() == 1 { "" } else { "s" }
                        )))
                        .children(add_lines)
                        .when(!plan.skipped.is_empty(), |this| {
                            this.child(div().text_sm().text_color(cx.theme().muted_foreground).child(
                                format!("Skip, already saved: {}", plan.skipped.join(", ")),
                            ))
                        })
                        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(
                            "Passwords are not part of the file. Add them by editing each connection.",
                        )),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("import-cancel").label("Cancel").outline()),
                        )
                        .child(DialogAction::new().child(
                            Button::new("import-confirm").primary().label("Import").on_click({
                                let this_handle = this_handle.clone();
                                let plan = plan.clone();
                                move |_, window, cx| {
                                    let connections: Vec<ConnectionData> = plan
                                        .add
                                        .iter()
                                        .map(|(connection, _)| connection.clone())
                                        .collect();
                                    this_handle
                                        .update(cx, |this, cx| {
                                            this.save_imported_connections(connections, window, cx)
                                        })
                                        .log_err();
                                    window.close_dialog(cx);
                                }
                            }),
                        )),
                )
        });
    }

    fn save_imported_connections(
        &mut self,
        connections: Vec<ConnectionData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app_database = AppDatabase::global(cx).clone();
        let db_service = DatabaseService::global(cx).clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut saved = 0usize;
            let mut failures = Vec::new();
            for mut connection in connections {
                match app_database.save_connection(&connection).await {
                    Ok(id) => {
                        connection.id = Some(id);
                        if let Some(config) = connection.to_connection_config() {
                            db_service.add_connection_config(config).await;
                        }
                        saved += 1;
                    }
                    Err(error) => failures.push(format!("{}: {error}", connection.name)),
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.reload_connections(cx);
                let note = if failures.is_empty() {
                    (
                        NotificationType::Success,
                        SharedString::from(format!(
                            "Imported {saved} connection{}",
                            if saved == 1 { "" } else { "s" }
                        )),
                    )
                } else {
                    (
                        NotificationType::Error,
                        SharedString::from(format!(
                            "Imported {saved}, failed: {}",
                            failures.join("; ")
                        )),
                    )
                };
                window.push_notification(note, cx);
            })
            .log_err();
        })
        .detach();
    }

    pub fn export_table_data(
        &mut self,
        context: ConnectionContext,
        table_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(table_name) = table_name {
            // Create the export modal content
            let modal_content = cx.new(|cx| ExportModal::new(context, table_name, window, cx));

            crate::export::modal::open_export_dialog(modal_content, window, cx);
        } else {
            tracing::error!("Cannot export: No table name provided");
        }
    }

    pub fn import_table_data(
        &mut self,
        context: ConnectionContext,
        table_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table_name) = table_name else {
            tracing::error!("Cannot import: No table name provided");
            return;
        };

        let modal_content = cx.new(|cx| ImportModal::new(context, table_name, window, cx));

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
                                        if let Err(error) =
                                            connection_credentials::delete_connection(
                                                connection_id,
                                                cx,
                                            )
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

    /// Duplicate an existing connection by opening the new connection modal
    /// pre-populated with everything but its secrets
    pub fn duplicate_connection(&mut self, connection_id: i64, cx: &mut Context<Self>) {
        let Some(connection) = self
            .connections
            .iter()
            .find(|connection| connection.id == Some(connection_id))
        else {
            tracing::error!("Connection with ID {} not found", connection_id);
            return;
        };

        let name = duplicate_name(
            &connection.name,
            self.connections
                .iter()
                .map(|connection| connection.name.as_str()),
        );
        cx.emit(ConnectionsPanelEvent::DuplicateConnection {
            connection_data: Box::new(connection.duplicate(name)),
        });
    }
}

/// Connection names are unique in the app database, so a duplicate gets the
/// first free "name copy", "name copy 2", ... name.
fn duplicate_name<'a>(name: &str, existing_names: impl Iterator<Item = &'a str>) -> String {
    let existing_names: std::collections::HashSet<&str> = existing_names.collect();
    let first = format!("{name} copy");
    if !existing_names.contains(first.as_str()) {
        return first;
    }
    (2usize..)
        .map(|number| format!("{name} copy {number}"))
        .find(|candidate| !existing_names.contains(candidate.as_str()))
        .unwrap_or(first)
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
/// `EnvironmentType` so blanco-core stays free of UI dependencies.
pub fn environment_color(environment_type: EnvironmentType, cx: &App) -> gpui::Rgba {
    tag_color(&environment_type.tag().color, cx)
}

/// The color a tag is drawn in: a hex value such as `#ff8800` as it is, or
/// the theme color one of `TAG_COLORS` names, which follows light and dark
/// themes.
pub fn tag_color(color: &str, cx: &App) -> gpui::Rgba {
    use gpui_component::Colorize as _;

    if color.starts_with('#')
        && let Ok(custom) = gpui::Hsla::parse_hex(color)
    {
        return custom.into();
    }
    let theme = cx.theme();
    match color {
        "gray" => theme.muted_foreground,
        "cyan" => theme.cyan,
        "green" => theme.green,
        "yellow" => theme.yellow,
        "red" => theme.red,
        "magenta" => theme.magenta,
        _ => theme.blue,
    }
    .into()
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
            comment: None,
            child_tables: None,
        }
    }

    fn item(id: &str, children: Vec<TreeItem<TreeItemMetadata>>) -> TreeItem<TreeItemMetadata> {
        TreeItem::new(id.to_string(), id.to_string(), metadata()).children(children)
    }

    #[test]
    fn duplicate_name_skips_names_already_taken() {
        assert_eq!(duplicate_name("local", ["local"].into_iter()), "local copy");
        assert_eq!(
            duplicate_name("local", ["local", "local copy", "local copy 2"].into_iter()),
            "local copy 3"
        );
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
