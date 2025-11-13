use gpui::{
    Action, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, Menu, MenuItem, ParentElement, Render, Styled, Subscription,
    Window, actions, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Root, TITLE_BAR_HEIGHT, TitleBar, WindowExt as _, button::Button, menu::AppMenuBar,
};
use log::{debug, error, info};
use serde::Deserialize;

use blanco_ui::IconName;
use gpui_component::Icon;

use crate::{
    app_events::AppEvent,
    connection_modal::NewConnectionModal,
    db_service::DbService,
    editor_panel::{EditorPanel, TabCreationParams},
    sidebar::ConnectionSidebar,
};

actions!(
    blanco_app,
    [
        Quit,
        About,
        OpenConnection,
        OpenSettings,
        OpenNewConnectionModal,
        CommitChanges,
        RollbackChanges,
        CopyAsCSV,
        CopyAsJSON,
        CopyAsSQL,
        CopyAsMarkdown,
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
pub struct RenameTab {
    pub tab_index: usize,
    pub new_name: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum ConnectionType {
    SQLite,
    PostgreSQL,
}

pub struct BlancoApp {
    focus_handle: FocusHandle,
    sidebar: Entity<ConnectionSidebar>,
    editor_panel: Entity<EditorPanel>,
    sidebar_collapsed: bool,
    app_menu_bar: Entity<AppMenuBar>,
    _subscriptions: Vec<Subscription>,
}

impl BlancoApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        init_menus(cx);

        let sidebar = cx.new(|cx| ConnectionSidebar::new(window, cx));

        // Load saved tabs from database
        info!("Loading saved tabs from database");
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();

        // Synchronously load tabs from database - wait for database to be initialized
        let saved_tabs = std::thread::spawn(move || {
            async_std::task::block_on(async {
                // Database should already be initialized synchronously
                if let Some(db) = app_db.read().await.as_ref() {
                    match db.load_query_tabs().await {
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
                } else {
                    error!("App database not initialized");
                    Vec::new()
                }
            })
        })
        .join()
        .unwrap_or_else(|_| Vec::new());

        let editor_panel =
            cx.new(|cx| EditorPanel::new_with_saved_tabs(window, cx, false, saved_tabs));
        let app_menu_bar = AppMenuBar::new(window, cx);

        // Set up event subscriptions using subscribe_in pattern
        let mut subscriptions = Vec::new();

        // Subscribe to sidebar events with window access for tab restoration
        let editor_panel_clone = editor_panel.clone();
        let subscription =
            cx.subscribe_in(&sidebar, window, move |_app, sidebar, event, window, cx| {
                match event {
                    AppEvent::ConnectionEstablished { .. } => {
                        // Refresh sidebar connections when a new connection is established
                        sidebar.update(cx, |sidebar, cx| {
                            sidebar.load_database_connections(cx);
                        });

                        // Optionally refresh editor panel connection options
                        editor_panel_clone.update(cx, |_editor_panel, _cx| {
                            // TODO: Refresh connection options in editor if needed
                            log::info!("Connection established, refreshing components");
                        });
                    }
                    AppEvent::ConnectionLost { .. } => {
                        // Handle connection loss
                        log::info!("Connection lost, updating UI components");
                    }
                    AppEvent::ConnectionsLoaded { .. } => {
                        // This is the key event for tab restoration with window access!
                        log::info!("Connections loaded, restoring saved tabs with window access");
                        editor_panel_clone.update(cx, |editor_panel, cx| {
                            if let Err(e) =
                                editor_panel.restore_saved_tabs_with_connections_sync(window, cx)
                            {
                                log::error!("Failed to restore saved tabs: {}", e);
                            }
                        });
                    }
                    _ => {}
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
                    log::info!("Query execution started");
                }
                AppEvent::QueryExecutionCompleted { .. } => {
                    // Could update status or refresh data
                    log::info!("Query execution completed");
                }
                AppEvent::TableOperationCompleted { table_name, success, rows_affected, operations_executed, .. } => {
                    if *success {
                        log::info!(
                            "Table operations completed successfully on '{}': {} operations, {} rows affected",
                            table_name, operations_executed, rows_affected.unwrap_or(0)
                        );
                    } else {
                        log::info!(
                            "Table operations failed on '{}': {} operations attempted",
                            table_name, operations_executed
                        );
                    }
                }
                AppEvent::ToggleSidebar => {
                        log::info!("🔄 ToggleSidebar event received!");
                        app.sidebar_collapsed = !app.sidebar_collapsed;
                        log::info!("🔄 New sidebar_collapsed state: {}", app.sidebar_collapsed);

                        // Update sidebar's collapse state
                        sidebar_clone.update(cx, |sidebar, cx| {
                            log::info!(
                                "🔄 Calling sidebar.set_collapsed with: {}",
                                app.sidebar_collapsed
                            );
                            sidebar.set_collapsed(app.sidebar_collapsed, cx);
                        });

                        // Update editor panel's sidebar state
                        editor_panel_for_events.update(cx, |panel, cx| {
                            panel.set_sidebar_collapsed(app.sidebar_collapsed, cx);
                        });
                }
                AppEvent::RenameTabRequested { tab_index, new_name } => {
                        log::info!("📝 RenameTabRequested event received: tab_index={}, new_name={}", tab_index, new_name);

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

        Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            editor_panel,
            sidebar_collapsed: false,
            app_menu_bar,
            _subscriptions: subscriptions,
        }
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn on_about(&mut self, _: &About, _: &mut Window, _: &mut Context<Self>) {
        println!("Blanco SQL Editor v0.1.0");
    }

    fn on_open_connection(&mut self, _: &OpenConnection, _: &mut Window, cx: &mut Context<Self>) {
        // TODO: Open connection dialog
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        log::info!("🔄 ToggleSidebar action triggered!");
        self.sidebar_collapsed = !self.sidebar_collapsed;
        log::info!("🔄 New sidebar_collapsed state: {}", self.sidebar_collapsed);

        // Update sidebar's collapse state
        self.sidebar.update(cx, |sidebar, cx| {
            log::info!(
                "🔄 Calling sidebar.set_collapsed with: {}",
                self.sidebar_collapsed
            );
            sidebar.set_collapsed(self.sidebar_collapsed, cx);
        });

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
                    let app_entity_ref = app_entity.clone();
                    move |_, window, cx| {
                        if let Some(conn_data) = content.read(cx).get_connection_data(cx) {
                            // Capture connection data for the event
                            let conn_type = conn_data.db_type.clone();
                            let db_name = conn_data.database_name.clone();

                            // Save connection to database
                            let db_service = DbService::global(cx).clone();
                            let app_db = db_service.app_db_handle();

                            // Start the async save operation
                            cx.spawn(async move |_cx| {
                                if let Some(db) = app_db.read().await.as_ref() {
                                    match db.save_connection(&conn_data).await {
                                        Ok(connection_id) => {
                                            log::info!(
                                                "Connection saved with ID: {}",
                                                connection_id
                                            );
                                        }
                                        Err(e) => {
                                            log::error!("Failed to save connection: {}", e);
                                        }
                                    }
                                } else {
                                    log::error!("App database not initialized");
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

                            window.push_notification("Connection saved successfully", cx);
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

    fn on_new_query(&mut self, action: &NewQuery, window: &mut Window, cx: &mut Context<Self>) {
        // Generate appropriate title based on provided parameters
        let title = match (&action.schema_name, &action.table_name) {
            (None, None) => action.database_name.clone(),
            (Some(schema), None) => format!("{}.{}", action.database_name, schema),
            (Some(schema), Some(table)) => format!("{}.{}.{}", action.database_name, schema, table),
            (None, Some(_)) => {
                // This shouldn't happen in normal usage, but handle gracefully
                action.database_name.clone()
            }
        };

        // Generate content for table queries if not provided
        let content = action.content.clone().or_else(|| {
            action
                .table_name
                .as_ref()
                .map(|table| match &action.schema_name {
                    Some(schema) => format!("SELECT * FROM {}.{}", schema, table),
                    None => format!("SELECT * FROM {}", table),
                })
        });

        log::info!(
            "on_new_query called: {} (schema: {:?}, table: {:?})",
            title,
            action.schema_name,
            action.table_name
        );

        // Create a new query tab with the specified parameters
        self.editor_panel.update(cx, |panel, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title,
                    content,
                    db_id: None,
                    connection_id: action.connection_id,
                    connection_type: "".to_owned(),
                    connection_name: None,
                    database_name: action.database_name.clone(),
                    schema_name: action.schema_name.clone(),
                },
                cx,
            );
        });
        cx.notify();
    }

    fn on_rename_tab(&mut self, action: &RenameTab, _window: &mut Window, cx: &mut Context<Self>) {
        log::info!(
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
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        let blanco_icon = Icon::new(IconName::Cat);

        let window_bounds = window.bounds();

        div()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_about))
            .on_action(cx.listener(Self::on_new_query))
            .on_action(cx.listener(Self::on_open_connection))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .on_action(cx.listener(Self::on_new_connection_modal))
            .on_action(cx.listener(Self::on_commit_changes))
            .on_action(cx.listener(Self::on_rollback_changes))
            .on_action(cx.listener(Self::on_rename_tab))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Title bar
            .child(
                TitleBar::new().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(blanco_icon)
                        .child(self.app_menu_bar.clone()),
                ),
            )
            // Main content area
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    // Sidebar (always visible, handles its own collapsed state)
                    .items_start()
                    .child({
                        let window_height = window_bounds.size.height;

                        div()
                            .h(window_height - TITLE_BAR_HEIGHT - px(25.))
                            .overflow_hidden()
                            .when(self.sidebar_collapsed, |div| {
                                div.w(px(48.)) // Collapsed width
                            })
                            .when(!self.sidebar_collapsed, |div| {
                                div.w(px(256.)) // Expanded width
                            })
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(self.sidebar.clone())
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
            .children(modal_layer)
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
                MenuItem::action("Open Connection", OpenConnection),
                MenuItem::action("Preferences...", OpenSettings),
                MenuItem::Separator,
                MenuItem::action("About Blanco", About),
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
