use blanco_ui::{Tab, TabBar};
use gpui::{
    Action, App, AppContext, BorrowAppContext, Context, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, Menu, MenuItem, ParentElement, Render, Styled, Subscription,
    Task, Window, actions, div, prelude::FluentBuilder, px, svg,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Root, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogFooter},
    global_state::GlobalState,
    h_flex,
    menu::AppMenuBar,
    notification::NotificationType,
    resizable::{ResizableState, h_resizable, resizable_panel},
    spinner::Spinner,
    status_bar::StatusBar,
};
use serde::Deserialize;
use smol::channel;
use tracing::{debug, error, info};

use crate::{
    app_database::{AppDatabase, ConnectionData, EnvironmentType},
    app_settings::AppSettings,
    command_palette::CommandPalette,
    connection_modal::NewConnectionModal,
    connections::{ConnectionsPanel, ConnectionsPanelEvent},
    editor::{EditorPanel, ObjectDdlParams, TabCreationParams, TableStructureParams},
    result_ext::ResultExt,
    snippets_panel::{RefreshSnippets, SnippetsPanel, SnippetsPanelEvent},
    status_bar::{ActivityMessage, ActivityReporter, ActivityResult, StatusBarState, StatusKind},
};

actions!(
    blanco_app,
    [
        Quit,
        OpenConnection,
        OpenSettings,
        OpenNewConnectionModal,
        NewSnippet,
        CommitChanges,
        RollbackChanges,
        CopyAsCSV,
        CopyAsTSV,
        CopyAsJSON,
        CopyAsSQL,
        CopyAsVALUES,
        CopyAsMarkdown,
        ExportData,
        ExportAsCSV,
        ExportAsTSV,
        ExportAsJSON,
        ExportAsSQL,
        ExportAsMarkdown,
        ClearSelection,
        ToggleRenderWhitespace,
        ToggleWordWrap,
        RunQuery,
        ExplainQuery,
        ToggleCommandPalette,
    ]
);

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct ConnectToConnection {
    pub connection_id: i64,
}

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = blanco_app, no_json)]
pub struct ToggleSidebar;

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct NewQuery {
    pub connection_id: i64,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub content: Option<String>,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CopyAsFormat {
    pub format: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct AddRow;

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DuplicateRow {
    pub row: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DeleteRow {
    pub row: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SetCellNull {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct EditCellInPopover {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct OpenSnippetEditor {
    pub snippet_id: Option<i64>,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct RenameTab {
    pub tab_index: usize,
    pub new_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct ExecuteSubstitutedQuery {
    pub query: String,
    pub connection_id: i64,
    pub database_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CreateNewQueryTab {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub environment_type: Option<EnvironmentType>,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct OpenObjectDdl {
    pub kind: blanco_core::RoutineKind,
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub object_name: String,
    pub environment_type: Option<EnvironmentType>,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct OpenTableStructure {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub table_name: String,
    pub environment_type: Option<EnvironmentType>,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct EditConnection {
    pub connection_id: i64,
}

// Action for database connection state
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DatabaseConnected {
    pub connection_id: i64,
    pub database_name: String,
}

// Action for formatting the current SQL query
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct FormatQuery;

impl From<database::DatabaseConnectedMessage> for DatabaseConnected {
    fn from(msg: database::DatabaseConnectedMessage) -> Self {
        Self {
            connection_id: msg.connection_id,
            database_name: msg.database_name,
        }
    }
}

// Action dispatched when the database service detects that a connection has
// dropped (e.g. the server closed an idle connection overnight, or an SSH
// tunnel died). Lets the sidebar reflect the disconnected state.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DatabaseDisconnected {
    pub connection_id: i64,
    pub database_name: String,
}

impl From<database::DatabaseDisconnectedMessage> for DatabaseDisconnected {
    fn from(msg: database::DatabaseDisconnectedMessage) -> Self {
        Self {
            connection_id: msg.connection_id,
            database_name: msg.database_name,
        }
    }
}

/// Which view is active in the sidebar's segmented tab bar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Connections,
    Snippets,
}

pub struct BlancoApp {
    focus_handle: FocusHandle,
    sidebar: Entity<ConnectionsPanel>,
    snippets_panel: Entity<SnippetsPanel>,
    editor_panel: Entity<EditorPanel>,
    command_palette: Entity<CommandPalette>,
    sidebar_collapsed: bool,
    sidebar_tab: SidebarTab,
    app_menu_bar: Entity<AppMenuBar>,
    main_resize_state: Entity<ResizableState>,
    status_bar: Entity<StatusBarState>,
    _subscriptions: Vec<Subscription>,
    _action_task: Task<()>,
    _activity_task: Task<()>,
}

impl BlancoApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        init_menus(cx);

        // Create channel for background action dispatch
        let (action_sender, action_receiver) =
            channel::unbounded::<database::DatabaseServiceMessage>();

        // Set sender on DatabaseService using update_global
        cx.update_global::<database::DatabaseService, _>(
            |db_service: &mut database::DatabaseService, _cx| {
                db_service.set_action_sender(action_sender);
            },
        );

        // Handle database service messages
        let action_task = cx.spawn(async move |_weak_handle, cx| {
            info!("DatabaseServiceMessage listener task started");
            while let Ok(msg) = action_receiver.recv().await {
                match msg {
                    database::DatabaseServiceMessage::Connected(conn_msg) => {
                        info!(
                            "DatabaseServiceMessage::Connected received for connection_id: {}, database_name: {}",
                            conn_msg.connection_id, conn_msg.database_name
                        );
                        _weak_handle.update(cx, |_, cx| {
                            info!("Dispatching DatabaseConnected action to app");
                            cx.dispatch_action(&DatabaseConnected::from(conn_msg));
                        }).log_err();
                    }
                    database::DatabaseServiceMessage::Disconnected(disconn_msg) => {
                        info!(
                            "DatabaseServiceMessage::Disconnected received for connection_id: {}, database_name: {}",
                            disconn_msg.connection_id, disconn_msg.database_name
                        );
                        _weak_handle.update(cx, |_, cx| {
                            cx.dispatch_action(&DatabaseDisconnected::from(disconn_msg));
                        }).log_err();
                    }
                }
            }
            info!("DatabaseServiceMessage listener task ended");
        });

        // App-wide activity channel feeding the bottom status bar. Any context
        // (including background tokio tasks without a `cx`) reports work through
        // the global ActivityReporter; this single task folds messages into the
        // status entity.
        let status_bar = cx.new(|_| StatusBarState::default());
        let (activity_sender, activity_receiver) = channel::unbounded::<ActivityMessage>();
        cx.set_global(ActivityReporter::new(activity_sender));
        let activity_task = cx.spawn({
            let status_bar = status_bar.downgrade();
            async move |_weak_handle, cx| {
                while let Ok(message) = activity_receiver.recv().await {
                    status_bar
                        .update(cx, |state, cx| state.apply(message, cx))
                        .log_err();
                }
            }
        });

        let sidebar = cx.new(|cx| ConnectionsPanel::new(window, cx));
        let snippets_panel = cx.new(|cx| SnippetsPanel::new(window, cx));
        let main_resize_state = cx.new(|_| ResizableState::default());

        // Load saved tabs from database
        info!("Loading saved tabs from database");

        // Synchronously load tabs from database
        let app_database = AppDatabase::global(cx);
        let saved_tabs = gpui_tokio::Tokio::handle(cx).block_on(async {
            // Database should already be initialized synchronously{
            match app_database.load_query_tabs().await {
                Ok(tabs) => {
                    info!("Loaded {} tabs from database", tabs.len());
                    for tab in &tabs {
                        debug!(
                            "Tab '{}' (db_id: {:?}, connection_id: {:?}, content_len: {})",
                            tab.title,
                            tab.id,
                            tab.connection_id,
                            tab.content.len()
                        );
                    }
                    tabs
                }
                Err(e) => {
                    error!("Failed to load tabs: {}", e);
                    Vec::new()
                }
            }
        });

        let editor_panel =
            cx.new(|cx| EditorPanel::new_with_saved_tabs(window, cx, false, saved_tabs));
        let command_palette = cx.new(|cx| CommandPalette::new(sidebar.downgrade(), window, cx));
        let app_menu_bar = AppMenuBar::new(cx);

        // Set up event subscriptions using subscribe_in pattern
        let mut subscriptions = Vec::new();

        // Subscribe to sidebar events
        let subscription = cx.subscribe_in(
            &sidebar,
            window,
            move |app, _sidebar, event, window, cx| match event {
                ConnectionsPanelEvent::EditConnection {
                    connection_data, ..
                } => {
                    app.open_edit_connection_modal(*connection_data.clone(), window, cx);
                }
            },
        );
        subscriptions.push(subscription);

        // Subscribe to snippets panel events
        let subscription = cx.subscribe_in(
            &snippets_panel,
            window,
            move |_app, _snippets_panel, event, _window, _cx| {
                match event {
                    SnippetsPanelEvent::SnippetDeleted => {
                        // Snippets panel already refreshed itself
                    }
                }
            },
        );
        subscriptions.push(subscription);

        // Re-render the status bar whenever the activity state changes.
        subscriptions.push(cx.observe(&status_bar, |_, _, cx| cx.notify()));

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        Self {
            focus_handle,
            sidebar,
            snippets_panel,
            editor_panel,
            command_palette,
            sidebar_collapsed: false,
            sidebar_tab: SidebarTab::Connections,
            app_menu_bar,
            main_resize_state,
            status_bar,
            _subscriptions: subscriptions,
            _action_task: action_task,
            _activity_task: activity_task,
        }
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> StatusBar {
        let line = self.status_bar.read(cx).display();
        let leading = match line.kind {
            StatusKind::Busy => Spinner::new().xsmall().into_any_element(),
            StatusKind::Ok => Icon::new(IconName::CircleCheck)
                .xsmall()
                .text_color(cx.theme().success)
                .into_any_element(),
            StatusKind::Err => Icon::new(IconName::CircleX)
                .xsmall()
                .text_color(cx.theme().danger)
                .into_any_element(),
            StatusKind::Idle => Icon::new(IconName::Check)
                .xsmall()
                .text_color(cx.theme().muted_foreground)
                .into_any_element(),
        };

        StatusBar::new().left(
            h_flex()
                .items_center()
                .gap_1()
                .child(leading)
                .child(line.text),
        )
    }

    #[cfg(test)]
    pub fn command_palette(&self) -> &Entity<CommandPalette> {
        &self.command_palette
    }

    #[cfg(test)]
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;

        // Update editor panel's sidebar state
        self.editor_panel.update(cx, |panel, cx| {
            panel.set_sidebar_collapsed(self.sidebar_collapsed, cx);
        });

        cx.notify();
    }

    fn on_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_settings_tab(window, cx);
        });
    }

    fn on_new_snippet(&mut self, _: &NewSnippet, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.create_snippet_tab(window, cx);
        });
    }

    fn on_open_snippet_editor(
        &mut self,
        action: &OpenSnippetEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = action.snippet_id {
            self.editor_panel.update(cx, |panel, cx| {
                panel.open_snippet_tab(id, window, cx);
            });
        } else {
            self.editor_panel.update(cx, |panel, cx| {
                panel.create_snippet_tab(window, cx);
            });
        }
    }

    fn on_refresh_snippets(
        &mut self,
        _: &RefreshSnippets,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snippets_panel.update(cx, |panel, cx| {
            panel.refresh_snippets(cx);
        });
    }

    fn on_database_connected(
        &mut self,
        action: &DatabaseConnected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.update(cx, |sidebar, cx| {
            // Mark the connection as connected and refresh the sidebar view
            sidebar.validate_connection_as_connected(action.connection_id, cx);
        });
        self.status_bar.update(cx, |state, cx| {
            state.flash(
                ActivityResult::Ok(format!("connected to {}", action.database_name).into()),
                cx,
            );
        });
    }

    fn on_database_disconnected(
        &mut self,
        action: &DatabaseDisconnected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.mark_connection_disconnected(action.connection_id, cx);
        });
        self.status_bar.update(cx, |state, cx| {
            state.flash(
                ActivityResult::Err(format!("disconnected from {}", action.database_name).into()),
                cx,
            );
        });
    }

    fn on_create_new_query_tab(
        &mut self,
        action: &CreateNewQueryTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = match (&action.schema_name, &action.table_name) {
            (None, None) => action.database_name.clone(),
            (Some(schema), None) => format!("{}.{}", action.database_name, schema),
            (Some(schema), Some(table)) => {
                format!("{}.{}.{}", action.database_name, schema, table)
            }
            (None, Some(table)) => table.to_string(),
        };

        let content = action
            .table_name
            .as_ref()
            .map(|table| match &action.schema_name {
                Some(schema) => format!("SELECT * FROM {}.{} LIMIT 100;", schema, table),
                None => format!("SELECT * FROM {} LIMIT 100;", table),
            });

        self.editor_panel.update(cx, |panel, cx| {
            panel.create_and_add_tab_with_connection(
                _window,
                TabCreationParams {
                    title,
                    content,
                    db_id: None,
                    connection_id: action.connection_id,
                    db_type: action.db_type,
                    connection_name: Some(action.connection_name.clone()),
                    database_name: action.database_name.clone(),
                    schema_name: action.schema_name.clone(),
                    environment_type: action.environment_type,
                },
                cx,
            );
        });
        cx.notify();
    }

    fn on_open_table_structure(
        &mut self,
        action: &OpenTableStructure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.create_table_structure_tab(
                TableStructureParams {
                    connection_id: action.connection_id,
                    connection_name: action.connection_name.clone(),
                    db_type: action.db_type,
                    database_name: action.database_name.clone(),
                    schema_name: action.schema_name.clone(),
                    table_name: action.table_name.clone(),
                    environment_type: action.environment_type,
                },
                window,
                cx,
            );
        });

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = action.connection_id;
        let database_name = action.database_name.clone();
        let schema_name = action.schema_name.clone();
        let table_name = action.table_name.clone();
        let editor_panel = self.editor_panel.clone();
        let activity = ActivityReporter::global(cx)
            .begin(format!("{}: loading structure", action.connection_name));

        cx.spawn_in(window, async move |_, window| {
            let _activity = activity;
            use database::DatabaseServiceTrait;
            let columns_result = db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await;

            if let Ok(connection) = columns_result {
                let columns = connection
                    .get_columns_for_table(&table_name, schema_name.as_deref())
                    .await
                    .unwrap_or_default();
                let indexes = connection
                    .get_indexes_for_table(&table_name, schema_name.as_deref())
                    .await
                    .unwrap_or_default();

                window
                    .update(|window, cx| {
                        editor_panel.update(cx, |panel, cx| {
                            panel.update_last_table_structure_tab(columns, indexes, window, cx);
                        });
                    })
                    .log_err();
            }
        })
        .detach();

        cx.notify();
    }

    fn on_open_object_ddl(
        &mut self,
        action: &OpenObjectDdl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_tab = self.editor_panel.update(cx, |panel, cx| {
            panel.create_object_ddl_tab(
                ObjectDdlParams {
                    kind: action.kind,
                    connection_id: action.connection_id,
                    connection_name: action.connection_name.clone(),
                    db_type: action.db_type,
                    database_name: action.database_name.clone(),
                    schema_name: action.schema_name.clone(),
                    object_name: action.object_name.clone(),
                    environment_type: action.environment_type,
                },
                window,
                cx,
            )
        });

        let db_service = database::DatabaseService::global(cx).clone();
        let connection_id = action.connection_id;
        let database_name = action.database_name.clone();
        let schema_name = action.schema_name.clone();
        let object_name = action.object_name.clone();
        let kind = action.kind;
        let activity =
            ActivityReporter::global(cx).begin(format!("{}: loading DDL", action.connection_name));

        cx.spawn_in(window, async move |_, window| {
            let _activity = activity;
            use database::DatabaseServiceTrait;
            let result = match db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await
            {
                Ok(connection) => {
                    connection
                        .object_ddl(kind, schema_name.as_deref(), &object_name)
                        .await
                }
                Err(e) => Err(e),
            };

            window
                .update(|window, cx| {
                    if let Some(tab) = weak_tab.upgrade() {
                        tab.update(cx, |tab, cx| match result {
                            Ok(ddl) => tab.set_ddl(ddl, cx),
                            Err(e) => tab.set_error(e.to_string(), cx),
                        });
                    }
                    let _ = window;
                })
                .log_err();
        })
        .detach();

        cx.notify();
    }

    fn on_new_connection_modal(
        &mut self,
        _: &OpenNewConnectionModal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Create the modal content outside the builder so we can access it
        let modal_content = cx.new(|cx| NewConnectionModal::new(window, cx));
        let content_for_focus = modal_content.clone();

        let sidebar = self.sidebar.clone();
        window.open_dialog(cx, move |modal, _window, _cx| {
            let content_clone = modal_content.clone();

            modal
                .title("New Connection")
                .h(gpui::px(700.))
                .w(gpui::px(650.))
                .child(modal_content.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("test-connection")
                                .label("Test Connection")
                                .on_click({
                                    let content = content_clone.clone();
                                    move |_, window, cx| {
                                        content.update(cx, |modal, cx| {
                                            modal.test_connection(window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("cancel").label("Cancel").outline().on_click(
                                    |_, window, cx| {
                                        window.close_dialog(cx);
                                    },
                                ))
                                .child(
                                    DialogAction::new()
                                        .child(Button::new("ok").primary().label("Save")),
                                ),
                        ),
                )
                .on_ok({
                    let content = content_clone;
                    let sidebar = sidebar.clone();
                    move |_, window, cx| {
                        if let Some(conn_data) = content.read(cx).get_connection_data(cx) {
                            let app_database = AppDatabase::global(cx).clone();
                            let db_service = database::DatabaseService::global(cx).clone();
                            let sidebar = sidebar.clone();
                            cx.spawn(async move |cx| {
                                match app_database.save_connection(&conn_data).await {
                                    Ok(connection_id) => {
                                        tracing::info!(
                                            "Connection saved with ID: {}",
                                            connection_id
                                        );
                                        let mut conn_data_with_id = conn_data;
                                        conn_data_with_id.id = Some(connection_id);
                                        if let Some(config) =
                                            conn_data_with_id.to_connection_config()
                                        {
                                            db_service.add_connection_config(config).await;
                                        }
                                        cx.update(|cx| {
                                            sidebar.update(cx, |panel, cx| {
                                                panel.reload_connections(cx);
                                            });
                                        });
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to save connection: {}", e);
                                    }
                                }
                            })
                            .detach();

                            window.push_notification(
                                (NotificationType::Success, "Connection saved successfully"),
                                cx,
                            );
                            true
                        } else {
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    "Please fill in all required fields",
                                ),
                                cx,
                            );
                            false
                        }
                    }
                })
        });

        // Focus the first input field after the modal opens
        content_for_focus
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    fn open_edit_connection_modal(
        &mut self,
        connection_data: ConnectionData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Create the modal content with existing connection data
        let modal_content = cx.new(|cx| {
            NewConnectionModal::with_connection_data(window, cx, Some(connection_data.clone()))
        });
        let content_for_focus = modal_content.clone();

        let connection_data_clone = connection_data.clone();
        let sidebar = self.sidebar.clone();

        window.open_dialog(cx, move |modal, _window, _cx| {
            let content_clone = modal_content.clone();

            modal
                .title("Edit Connection")
                .h(gpui::px(700.))
                .w(gpui::px(650.))
                .child(modal_content.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("test-connection")
                                .label("Test Connection")
                                .on_click({
                                    let content = content_clone.clone();
                                    move |_, window, cx| {
                                        content.update(cx, |modal, cx| {
                                            modal.test_connection(window, cx);
                                        });
                                    }
                                }),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(Button::new("cancel").label("Cancel").outline().on_click(
                                    |_, window, cx| {
                                        window.close_dialog(cx);
                                    },
                                ))
                                .child(
                                    DialogAction::new()
                                        .child(Button::new("ok").primary().label("Save")),
                                ),
                        ),
                )
                .on_ok({
                    let content = content_clone;
                    let conn_data_ref = connection_data_clone.clone();
                    let sidebar = sidebar.clone();
                    move |_, window, cx| {
                        if let Some(mut conn_data) = content.read(cx).get_connection_data(cx) {
                            let original_id = conn_data_ref.id;
                            conn_data.id = original_id;

                            let app_database = AppDatabase::global(cx).clone();
                            let db_service = database::DatabaseService::global(cx).clone();
                            let sidebar = sidebar.clone();
                            cx.spawn(async move |cx| {
                                match app_database.save_connection(&conn_data).await {
                                    Ok(connection_id) => {
                                        tracing::info!(
                                            "Connection updated with ID: {}",
                                            connection_id
                                        );
                                        let mut conn_data_with_id = conn_data;
                                        conn_data_with_id.id = Some(connection_id);
                                        if let Some(config) =
                                            conn_data_with_id.to_connection_config()
                                        {
                                            db_service.add_connection_config(config).await;
                                        }
                                        cx.update(|cx| {
                                            sidebar.update(cx, |panel, cx| {
                                                panel.reload_connections(cx);
                                            });
                                        });
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to update connection: {}", e);
                                    }
                                }
                            })
                            .detach();

                            window.push_notification(
                                (NotificationType::Success, "Connection updated successfully"),
                                cx,
                            );
                            true
                        } else {
                            window.push_notification(
                                (
                                    NotificationType::Error,
                                    "Please fill in all required fields",
                                ),
                                cx,
                            );
                            false
                        }
                    }
                })
        });

        // Focus the first input field after the modal opens
        content_for_focus
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    fn on_commit_changes(
        &mut self,
        _: &CommitChanges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Delegate commit to the editor panel (which will forward to results panel)
        self.editor_panel.update(cx, |panel, cx| {
            panel.commit_current_changes(window, cx);
        });
    }

    fn on_rollback_changes(
        &mut self,
        _: &RollbackChanges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Delegate rollback to the editor panel (which will forward to results panel)
        self.editor_panel.update(cx, |panel, cx| {
            panel.rollback_current_changes(window, cx);
        });
    }

    fn on_rename_tab(&mut self, action: &RenameTab, _window: &mut Window, cx: &mut Context<Self>) {
        // Delegate tab renaming to the editor panel
        self.editor_panel.update(cx, |panel, cx| {
            panel.rename_tab(action.tab_index, &action.new_name, cx);
        });
        cx.notify();
    }

    fn on_toggle_render_whitespace(
        &mut self,
        _: &ToggleRenderWhitespace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_value = !AppSettings::global(cx).settings.editor.show_whitespace;
        AppSettings::global_mut(cx).settings.editor.show_whitespace = new_value;

        // Save to database
        let db = AppDatabase::global(cx).clone();
        let new_value_str = new_value.to_string();
        cx.spawn(async move |_, _| async move {
            if let Err(e) = db
                .save_setting("editor.show_whitespace", &new_value_str, false)
                .await
            {
                tracing::error!("Failed to save show_whitespace setting: {}", e);
            }
        })
        .detach();

        // Update all query tab editors
        self.editor_panel.update(cx, |panel, cx| {
            panel.set_all_editors_show_whitespace(new_value, window, cx);
        });

        cx.notify();
    }

    fn on_toggle_word_wrap(
        &mut self,
        _: &ToggleWordWrap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_value = !AppSettings::global(cx).settings.editor.word_wrap;
        AppSettings::global_mut(cx).settings.editor.word_wrap = new_value;

        // Save to database
        let db = AppDatabase::global(cx).clone();
        let new_value_str = new_value.to_string();
        cx.spawn(async move |_, _| async move {
            if let Err(e) = db
                .save_setting("editor.word_wrap", &new_value_str, false)
                .await
            {
                tracing::error!("Failed to save word_wrap setting: {}", e);
            }
        })
        .detach();

        // Update all query tab editors
        self.editor_panel.update(cx, |panel, cx| {
            panel.set_all_editors_soft_wrap(new_value, window, cx);
        });

        cx.notify();
    }

    fn on_run_query(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.on_run_query(window, cx);
        });
    }

    fn on_explain_query(&mut self, _: &ExplainQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.on_explain_query(window, cx);
        });
    }

    fn on_connect_to_connection(
        &mut self,
        action: &ConnectToConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connection_id = action.connection_id;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.expand_connection(connection_id, window, cx);
        });
    }

    fn on_toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_palette.update(cx, |palette, cx| {
            palette.toggle(window, cx);
        });
    }
}

impl Focusable for BlancoApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BlancoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet_layer = Root::render_sheet_layer(window, cx);
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .track_focus(&self.focus_handle)
            .key_context("BroquestApp")
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .on_action(cx.listener(Self::on_create_new_query_tab))
            .on_action(cx.listener(Self::on_open_table_structure))
            .on_action(cx.listener(Self::on_open_object_ddl))
            .on_action(cx.listener(Self::on_new_connection_modal))
            .on_action(cx.listener(Self::on_new_snippet))
            .on_action(cx.listener(Self::on_commit_changes))
            .on_action(cx.listener(Self::on_rollback_changes))
            .on_action(cx.listener(Self::on_rename_tab))
            .on_action(cx.listener(Self::on_database_connected))
            .on_action(cx.listener(Self::on_database_disconnected))
            .on_action(cx.listener(Self::on_open_snippet_editor))
            .on_action(cx.listener(Self::on_refresh_snippets))
            .on_action(cx.listener(Self::on_toggle_render_whitespace))
            .on_action(cx.listener(Self::on_toggle_word_wrap))
            .on_action(cx.listener(Self::on_run_query))
            .on_action(cx.listener(Self::on_explain_query))
            .on_action(cx.listener(Self::on_connect_to_connection))
            .on_action(cx.listener(Self::on_toggle_command_palette))
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Title bar
            .child(
                TitleBar::new().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x_3()
                        .bg(cx.theme().title_bar)
                        .child(
                            svg()
                                .h(px(40.))
                                .w(px(128.))
                                .text_color(window.text_style().color)
                                .path("images/blanco.svg"),
                        )
                        .child(self.app_menu_bar.clone()),
                ),
            )
            // Main content area
            .child(
                div().flex().flex_1().min_h_0().overflow_hidden().child(
                    h_resizable("main-layout")
                        .with_state(&self.main_resize_state)
                        // Left side: Connections panel sidebar
                        .when(!self.sidebar_collapsed, |this| {
                            this.child(
                                resizable_panel()
                                    .size(px(256.))
                                    .size_range(px(200.)..px(500.))
                                    .child(
                                        div()
                                            .w_full()
                                            .h_full()
                                            .flex()
                                            .flex_col()
                                            .pb_6()
                                            .overflow_hidden()
                                            .child(
                                                div().flex_none().px_2().py_1p5().child(
                                                    TabBar::new("sidebar-tabs")
                                                        .segmented()
                                                        .w_full()
                                                        .selected_index(match self.sidebar_tab {
                                                            SidebarTab::Connections => 0,
                                                            SidebarTab::Snippets => 1,
                                                        })
                                                        .on_click({
                                                            let view = cx.entity().downgrade();
                                                            move |ix: &usize, _, _, cx| {
                                                                let tab = match ix {
                                                                    1 => SidebarTab::Snippets,
                                                                    _ => SidebarTab::Connections,
                                                                };
                                                                view.update(cx, |this, cx| {
                                                                    this.sidebar_tab = tab;
                                                                    cx.notify();
                                                                })
                                                                .log_err();
                                                            }
                                                        })
                                                        .child(
                                                            Tab::new()
                                                                .label("Connections")
                                                                .flex_1(),
                                                        )
                                                        .child(
                                                            Tab::new().label("Snippets").flex_1(),
                                                        ),
                                                ),
                                            )
                                            .child(div().flex_1().min_h_0().overflow_hidden().map(
                                                |this| match self.sidebar_tab {
                                                    SidebarTab::Connections => {
                                                        this.child(self.sidebar.clone())
                                                    }
                                                    SidebarTab::Snippets => {
                                                        this.child(self.snippets_panel.clone())
                                                    }
                                                },
                                            )),
                                    ),
                            )
                        })
                        // Main panel
                        .child(
                            resizable_panel().child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .h_full()
                                    .overflow_hidden()
                                    .child(self.editor_panel.clone()),
                            ),
                        ),
                ),
            )
            // Status bar pinned to the bottom, spanning the full window width.
            .child(self.render_status_bar(cx))
            .child(self.command_palette.clone())
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

fn init_menus(cx: &mut App) {
    // Register keyboard shortcut for settings
    cx.bind_keys([
        gpui::KeyBinding::new("super-,", OpenSettings, None),
        // Register keyboard shortcuts for commit operations
        gpui::KeyBinding::new("super-shift-c", CommitChanges, None),
        gpui::KeyBinding::new("super-shift-r", RollbackChanges, None),
        // Register keyboard shortcut for formatting SQL
        gpui::KeyBinding::new("shift-alt-f", FormatQuery, None),
        // Register keyboard shortcut for the command palette
        gpui::KeyBinding::new("secondary-k", ToggleCommandPalette, None),
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-q", Quit, None),
        #[cfg(not(target_os = "macos"))]
        gpui::KeyBinding::new("alt-f4", Quit, None),
    ]);

    cx.set_menus(build_menu());

    let menu = build_menu().into_iter().map(|menu| menu.owned()).collect();
    GlobalState::global_mut(cx).set_app_menus(menu);
}

fn build_menu() -> Vec<Menu> {
    vec![
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Connection", OpenNewConnectionModal),
                MenuItem::action("New Snippet", NewSnippet),
                MenuItem::action("Settings", OpenSettings),
                MenuItem::Separator,
                MenuItem::action("Quit", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", gpui_component::input::Undo),
                MenuItem::action("Redo", gpui_component::input::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", gpui_component::input::Cut),
                MenuItem::action("Copy", gpui_component::input::Copy),
                MenuItem::action("Paste", gpui_component::input::Paste),
                MenuItem::separator(),
                MenuItem::action("Select All", gpui_component::input::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Render Whitespace", ToggleRenderWhitespace),
                MenuItem::action("Word Wrap", ToggleWordWrap),
            ],
            disabled: false,
        },
    ]
}
