use gpui::{
    Action, App, AppContext, BorrowAppContext, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, Menu, MenuItem, ParentElement, Render, Styled,
    Subscription, Task, Window, actions, div, prelude::FluentBuilder, px, svg,
};
use gpui_component::{
    ActiveTheme, Root, TITLE_BAR_HEIGHT, TitleBar, WindowExt as _, button::Button,
    menu::AppMenuBar, notification::NotificationType,
};
use serde::Deserialize;
use smol::channel;
use tracing::{debug, error, info};

use crate::{
    app_database::AppDatabase,
    app_events::AppEvent,
    connection_modal::NewConnectionModal,
    connections_panel::ConnectionsPanel,
    editor_panel::{EditorPanel, TabCreationParams},
    snippets_panel::{RefreshSnippets, SnippetsPanel},
};

actions!(
    blanco_app,
    [
        Quit,
        About,
        OpenConnection,
        OpenSettings,
        OpenNewConnectionModal,
        NewSnippet,
        CommitChanges,
        RollbackChanges,
        CopyAsCSV,
        CopyAsJSON,
        CopyAsSQL,
        CopyAsMarkdown,
        ExportData,
        ClearSelection
    ]
);

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
pub struct SelectRow {
    pub row: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SelectCell {
    pub row: usize,
    pub col: usize,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DoubleClickCell {
    pub row: usize,
    pub col: usize,
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

// Action for database connection state
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct DatabaseConnected {
    pub connection_id: i64,
    pub database_name: String,
}

impl From<database::DatabaseConnectedMessage> for DatabaseConnected {
    fn from(msg: database::DatabaseConnectedMessage) -> Self {
        Self {
            connection_id: msg.connection_id,
            database_name: msg.database_name,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum ConnectionType {
    SQLite,
    PostgreSQL,
}

pub struct BlancoApp {
    focus_handle: FocusHandle,
    sidebar: Entity<ConnectionsPanel>,
    snippets_panel: Entity<SnippetsPanel>,
    editor_panel: Entity<EditorPanel>,
    sidebar_collapsed: bool,
    app_menu_bar: Entity<AppMenuBar>,
    _subscriptions: Vec<Subscription>,
    _action_task: Task<()>,
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
                        let _ = _weak_handle.update(cx, |_, cx| {
                            info!("Dispatching DatabaseConnected action to app");
                            cx.dispatch_action(&DatabaseConnected::from(conn_msg));
                        });
                    }
                    database::DatabaseServiceMessage::Disconnected(disconn_msg) => {
                        info!(
                            "DatabaseServiceMessage::Disconnected received for connection_id: {}, database_name: {}",
                            disconn_msg.connection_id, disconn_msg.database_name
                        );
                        // TODO: Handle disconnection - dispatch action or update state
                    }
                }
            }
            info!("DatabaseServiceMessage listener task ended");
        });

        let sidebar = cx.new(|cx| ConnectionsPanel::new(window, cx));
        let snippets_panel = cx.new(|cx| SnippetsPanel::new(window, cx));

        // Load saved tabs from database
        info!("Loading saved tabs from database");

        // Synchronously load tabs from database - wait for database to be initialized
        let app_database = AppDatabase::global(cx);
        let saved_tabs = smol::block_on(async {
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
        let app_menu_bar = AppMenuBar::new(cx);

        // Set up event subscriptions using subscribe_in pattern
        let mut subscriptions = Vec::new();

        // Subscribe to sidebar events with window access for tab restoration
        let _editor_panel_clone = editor_panel.clone();
        let subscription =
            cx.subscribe_in(&sidebar, window, move |app, _sidebar, event, window, cx| {
                if let AppEvent::CreateNewQueryTab {
                    connection_id,
                    connection_name,
                    database_name,
                    schema_name,
                    table_name,
                    environment_type,
                } = event
                {
                    tracing::info!(
                        "CreateNewQueryTab called: {} (database: {:?}, schema: {:?}, table: {:?})",
                        connection_id,
                        database_name,
                        schema_name,
                        table_name,
                    );

                    // Generate appropriate title based on provided parameters
                    let title = match (&schema_name, &table_name) {
                        (None, None) => database_name.clone(),
                        (Some(schema), None) => format!("{}.{}", database_name, schema),
                        (Some(schema), Some(table)) => {
                            format!("{}.{}.{}", database_name, schema, table)
                        }
                        (None, Some(table)) => table.to_string(),
                    };

                    // Generate content for table queries if not provided
                    let content = table_name.as_ref().map(|table| match &schema_name {
                        Some(schema) => format!("SELECT * FROM {}.{} LIMIT 100;", schema, table),
                        None => format!("SELECT * FROM {} LIMIT 100;", table),
                    });

                    // Create a new query tab with the specified parameters
                    app.editor_panel.update(cx, |panel, cx| {
                        panel.create_and_add_tab_with_connection(
                            window,
                            TabCreationParams {
                                title,
                                content,
                                db_id: None,
                                connection_id: *connection_id,
                                connection_type: "".to_owned(),
                                connection_name: Some(connection_name.clone()),
                                database_name: database_name.clone(),
                                schema_name: schema_name.clone(),
                                environment_type: *environment_type,
                            },
                            cx,
                        );
                    });
                    cx.notify();
                }
            });
        subscriptions.push(subscription);

        // Subscribe to editor panel events to update other components
        let sidebar_clone = sidebar.clone();
        let editor_panel_for_subscription = editor_panel.clone();

        let subscription = cx.subscribe(&editor_panel, move |app, _editor_panel, event, cx| {
            let editor_panel_for_events = editor_panel_for_subscription.clone();
            match event {
                AppEvent::QueryExecutionStarted { .. } => {
                    // Could show loading indicator or update status
                    tracing::info!("Query execution started");
                }
                AppEvent::TableOperationCompleted { table_name, success, rows_affected, operations_executed, .. } => {
                    if *success {
                        tracing::info!(
                            "Table operations completed successfully on '{}': {} operations, {} rows affected",
                            table_name, operations_executed, rows_affected.unwrap_or(0)
                        );
                    } else {
                        tracing::info!(
                            "Table operations failed on '{}': {} operations attempted",
                            table_name, operations_executed
                        );
                    }
                }
                AppEvent::ToggleSidebar => {
                        tracing::info!("🔄 ToggleSidebar event received!");
                        app.sidebar_collapsed = !app.sidebar_collapsed;
                        tracing::info!("🔄 New sidebar_collapsed state: {}", app.sidebar_collapsed);

                        // Update sidebar's collapse state
                        sidebar_clone.update(cx, |_sidebar, _cx| {
                            tracing::info!(
                                "🔄 Calling sidebar.set_collapsed with: {}",
                                app.sidebar_collapsed
                            );
                            // sidebar.set_collapsed(app.sidebar_collapsed, cx);
                        });

                        // Update editor panel's sidebar state
                        editor_panel_for_events.update(cx, |panel, cx| {
                            panel.set_sidebar_collapsed(app.sidebar_collapsed, cx);
                        });
                }
                AppEvent::RenameTabRequested { tab_index, new_name } => {
                        tracing::info!("📝 RenameTabRequested event received: tab_index={}, new_name={}", tab_index, new_name);

                        // Dispatch the RenameTab action to handle the rename
                        cx.dispatch_action(&RenameTab {
                            tab_index: *tab_index,
                            new_name: new_name.clone(),
                        });
                }
                _ => {}
            }
        });
        subscriptions.push(subscription);

        // Subscribe to snippets panel events
        let editor_panel_for_snippets = editor_panel.clone();

        let snippets_panel_for_refresh = snippets_panel.clone();
        let subscription = cx.subscribe_in(
            &snippets_panel,
            window,
            move |app, _snippets_panel, event, window, cx| {
                let _editor_panel = editor_panel_for_snippets.clone();
                let _snippets_panel = snippets_panel_for_refresh.clone();
                match event {
                    AppEvent::OpenSnippetEditor { snippet_id } => {
                        if let Some(id) = snippet_id {
                            app.editor_panel.update(cx, |panel, cx| {
                                panel.open_snippet_tab(*id, window, cx);
                            });
                        } else {
                            app.editor_panel.update(cx, |panel, cx| {
                                panel.create_snippet_tab(window, cx);
                            });
                        }
                    }
                    AppEvent::SnippetDeleted { .. } => {
                        // Snippets panel already refreshed itself
                    }
                    _ => {}
                }
            },
        );
        subscriptions.push(subscription);

        Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            snippets_panel,
            editor_panel,
            sidebar_collapsed: false,
            app_menu_bar,
            _subscriptions: subscriptions,
            _action_task: action_task,
        }
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn on_about(&mut self, _: &About, _: &mut Window, _: &mut Context<Self>) {
        println!("Blanco SQL Editor v0.1.0");
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

        // Capture a weak reference to the app entity for event emission
        // The app implements EventEmitter<AppEvent>, so it can emit events
        let app_entity = cx.entity().downgrade();

        window.open_dialog(cx, move |modal, _window, _cx| {
            let content_clone = modal_content.clone();

            modal
                .title("New Connection")
                .h(gpui::px(600.))
                .w(gpui::px(400.))
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
                    let app_entity_ref = app_entity.clone();
                    move |_, window, cx| {
                        if let Some(conn_data) = content.read(cx).get_connection_data(cx) {
                            // Capture connection data for the event
                            let conn_type = conn_data.db_type.clone();
                            let db_name = conn_data.database_name.clone();

                            // Start the async save operation
                            let app_database = AppDatabase::global(cx).clone();
                            cx.spawn(async move |_cx| {
                                match app_database.save_connection(&conn_data.clone()).await {
                                    Ok(connection_id) => {
                                        tracing::info!(
                                            "Connection saved with ID: {}",
                                            connection_id
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to save connection: {}", e);
                                    }
                                }
                            })
                            .detach();

                            // Emit the event immediately after starting the save
                            // We'll emit optimistically since the modal was validated
                            if let Some(app) = app_entity_ref.upgrade() {
                                app.update(cx, |_app, cx| {
                                    cx.emit(AppEvent::ConnectionEstablished {
                                        connection_id: None, // We don't know the ID yet
                                        connection_type: conn_type,
                                        database_name: db_name,
                                    });
                                });
                            }

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
        tracing::info!(
            "on_rename_tab called: tab_index={}, new_name={}",
            action.tab_index,
            action.new_name
        );
        // Delegate tab renaming to the editor panel
        self.editor_panel.update(cx, |panel, cx| {
            panel.rename_tab(action.tab_index, &action.new_name, cx);
        });
        cx.notify();
    }
}

impl EventEmitter<AppEvent> for BlancoApp {}

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

        let window_bounds = window.bounds();

        div()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_about))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .on_action(cx.listener(Self::on_new_connection_modal))
            .on_action(cx.listener(Self::on_new_snippet))
            .on_action(cx.listener(Self::on_commit_changes))
            .on_action(cx.listener(Self::on_rollback_changes))
            .on_action(cx.listener(Self::on_rename_tab))
            .on_action(cx.listener(Self::on_database_connected))
            .on_action(cx.listener(Self::on_refresh_snippets))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Title bar
            .child(
                TitleBar::new().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_x_3()
                        .child(
                            svg()
                                .h(px(32.))
                                .w(px(103.))
                                .text_color(window.text_style().color)
                                .path("images/blanco.svg"),
                        )
                        .child(self.app_menu_bar.clone()),
                ),
            )
            // Main content area
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .items_start()
                    // Left side: Connections panel sidebar
                    .when(!self.sidebar_collapsed, |this| {
                        let window_height = window_bounds.size.height;
                        this.child(
                            div()
                                .h(window_height - TITLE_BAR_HEIGHT - px(25.))
                                .w(px(256.))
                                .pb_6()
                                .overflow_hidden()
                                .border_r_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .size_full()
                                        .flex()
                                        .flex_col()
                                        .child(self.sidebar.clone())
                                        .child(self.snippets_panel.clone()),
                                ),
                        )
                    })
                    // Main panel
                    .child({
                        let window_height = window_bounds.size.height;
                        div()
                            .flex()
                            .flex_1()
                            .h(window_height - TITLE_BAR_HEIGHT - px(25.))
                            .overflow_hidden()
                            .child(
                                // Editor panel (now contains everything - tabs, editor, results)
                                self.editor_panel.clone(),
                            )
                    }),
            )
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
    ]);
    cx.set_menus(vec![
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Connection", OpenNewConnectionModal),
                MenuItem::action("New Snippet", NewSnippet),
                MenuItem::action("Settings", OpenSettings),
                MenuItem::Separator,
                MenuItem::action("Quit", Quit),
            ],
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
        },
    ]);
}
