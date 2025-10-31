use gpui::{
    actions, div, prelude::FluentBuilder, px, Action, App, AppContext, Context, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, Menu, MenuItem, ParentElement, Render,
    Styled, Window, Subscription,
};
use gpui_component::{
    button::Button, h_flex, menu::AppMenuBar, v_flex, ActiveTheme, ContextModal as _, Root,
    TitleBar,
};
use log::{debug, error, info};
use serde::Deserialize;

use blanco_ui::{Icon, IconName};

use crate::{
    app_events::AppEvent,
    connection_modal::NewConnectionModal,
    db_service::DbService,
    editor_panel::EditorPanel,
    sidebar::ConnectionSidebar,
};

actions!(
    blanco_app,
    [
        Quit,
        About,
        NewQuery,
        OpenConnection,
        OpenSettings,
        OpenNewConnectionModal,
        RunQuery,
        CommitChanges,
        RollbackChanges
    ]
);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = blanco_app, no_json)]
pub struct ToggleSidebar;

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct NewQueryForUnifiedConnection {
    pub connection_key: String,
    pub connection_string: String,
    pub display_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct NewQueryForUnifiedSchema {
    pub connection_key: String,
    pub schema_name: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
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
                                    "Tab '{}' (db_id: {:?}, content_len: {})",
                                    tab.title,
                                    tab.id,
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
        let subscription = cx.subscribe_in(
            &sidebar,
            window,
            move |app, sidebar, event, window, cx| {
                match event {
                    AppEvent::ConnectionEstablished { .. } => {
                        // Refresh sidebar connections when a new connection is established
                        sidebar.update(cx, |sidebar, cx| {
                            sidebar.load_database_connections(cx);
                        });

                        // Optionally refresh editor panel connection options
                        editor_panel_clone.update(cx, |editor_panel, cx| {
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
                            if let Err(e) = editor_panel.restore_saved_tabs_with_connections_sync(window, cx) {
                                log::error!("Failed to restore saved tabs: {}", e);
                            }
                        });
                    }
                    _ => {}
                }
            }
        );
        subscriptions.push(subscription);

        // Subscribe to editor panel events to update other components
        let sidebar_clone = sidebar.clone();
        let subscription = cx.subscribe(&editor_panel, move |app, editor_panel, event, cx| {
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
        // Save tabs before quitting
        self.editor_panel.update(cx, |panel, cx| {
            panel.save_tabs(cx);
        });

        
        // Give a moment for operations to complete
        std::thread::sleep(std::time::Duration::from_millis(200));
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
        self.sidebar_collapsed = !self.sidebar_collapsed;

        // Update sidebar's collapse state
        self.sidebar.update(cx, |sidebar, cx| {
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

                            cx.spawn(async move |cx| {
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

    fn on_run_query(&mut self, _: &RunQuery, window: &mut Window, cx: &mut Context<Self>) {
        // Delegate query execution to the editor panel
        self.editor_panel.update(cx, |panel, cx| {
            panel.execute_current_query(window, cx);
        });
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

    fn on_new_query_for_unified_connection(
        &mut self,
        action: &NewQueryForUnifiedConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "on_new_query_for_unified_connection called: {}",
            action.display_name
        );
        // Create a new query tab for the specified unified connection
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_new_tab_with_unified_connection(
                window,
                action.display_name.clone(),
                action.connection_string.clone(),
                None,
                cx,
            );
        });
        cx.notify();
    }

    fn on_new_query_for_unified_schema(
        &mut self,
        action: &NewQueryForUnifiedSchema,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "on_new_query_for_unified_schema called: {}",
            action.schema_name
        );
        // Create a new query tab for the specified unified schema
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_new_tab_with_unified_connection(
                window,
                format!("Unified ({})", action.schema_name),
                action.connection_key.clone(),
                Some(action.schema_name.clone()),
                cx,
            );
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
        let drawer_layer = Root::render_drawer_layer(window, cx);
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        let blanco_icon = Icon::new(IconName::Cat);

        v_flex()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_about))
            .on_action(cx.listener(Self::on_new_query_for_unified_connection))
            .on_action(cx.listener(Self::on_new_query_for_unified_schema))
            .on_action(cx.listener(Self::on_open_connection))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .on_action(cx.listener(Self::on_new_connection_modal))
            .on_action(cx.listener(Self::on_run_query))
            .on_action(cx.listener(Self::on_commit_changes))
            .on_action(cx.listener(Self::on_rollback_changes))
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
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    // Sidebar (always visible, handles its own collapsed state)
                    .child(
                        div()
                            .h_full()
                            .when(self.sidebar_collapsed, |div| {
                                div.w(px(48.)) // Collapsed width
                            })
                            .when(!self.sidebar_collapsed, |div| {
                                div.w(px(280.)) // Expanded width
                            })
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .child(self.sidebar.clone()),
                    )
                    // Main panel
                    .child(
                        // Editor panel (now contains everything - tabs, editor, results)
                        self.editor_panel.clone(),
                    ),
            )
            .children(drawer_layer)
            .children(modal_layer)
            .children(notification_layer)
    }
}

fn init_menus(cx: &mut App) {
    // Register keyboard shortcut for settings (Ctrl/Cmd + ,)
    cx.bind_keys([
        gpui::KeyBinding::new("cmd-,", OpenSettings, None),
        gpui::KeyBinding::new("ctrl-,", OpenSettings, None),
        // Register keyboard shortcuts for query execution and commit operations
        gpui::KeyBinding::new("cmd-enter", RunQuery, None),
        gpui::KeyBinding::new("ctrl-enter", RunQuery, None),
        gpui::KeyBinding::new("cmd-shift-c", CommitChanges, None),
        gpui::KeyBinding::new("ctrl-shift-c", CommitChanges, None),
        gpui::KeyBinding::new("cmd-shift-r", RollbackChanges, None),
        gpui::KeyBinding::new("ctrl-shift-r", RollbackChanges, None),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Query", NewQuery),
                MenuItem::Separator,
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
        Menu {
            name: "Query".into(),
            items: vec![
                MenuItem::action("Run Query", RunQuery),
                MenuItem::separator(),
                MenuItem::action("Commit Changes", CommitChanges),
                MenuItem::action("Rollback Changes", RollbackChanges),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![MenuItem::action("Toggle Sidebar", ToggleSidebar)],
        },
    ]);
}
