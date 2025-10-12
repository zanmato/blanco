use gpui::{
    actions, div, prelude::FluentBuilder, px, Action, App, AppContext, Context, Entity,
    FocusHandle, Focusable, InteractiveElement, IntoElement, Menu, MenuItem, MouseButton,
    ParentElement, Render, Styled, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    menu::AppMenuBar,
    v_flex, ActiveTheme, ContextModal as _, IconName, Root, Sizable, TitleBar,
};
use log::{debug, error, info};
use serde::Deserialize;

use crate::{
    connection_modal::NewConnectionModal,
    db_service::{DbService, PgConnectionKey},
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
        OpenNewConnectionModal
    ]
);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = blanco_app, no_json)]
pub struct ToggleSidebar;

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct NewQueryForConnection {
    pub connection_name: String,
    pub connection_type: ConnectionType,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct NewQueryForPostgresConnection {
    pub connection_key: PgConnectionKey,
    pub display_name: String,
}

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct NewQueryForPostgresSchema {
    pub connection_key: PgConnectionKey,
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
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                // Wait up to 1 second for database to be initialized
                let mut attempts = 0;
                while attempts < 10 {
                    if app_db.read().await.is_some() {
                        break;
                    }
                    debug!(
                        "Waiting for app database to initialize... (attempt {})",
                        attempts + 1
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    attempts += 1;
                }

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
                    error!("App database not initialized after waiting");
                    Vec::new()
                }
            })
        })
        .join()
        .unwrap_or_else(|_| Vec::new());

        let editor_panel =
            cx.new(|cx| EditorPanel::new_with_saved_tabs(window, cx, false, saved_tabs));
        let app_menu_bar = AppMenuBar::new(window, cx);

        Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            editor_panel,
            sidebar_collapsed: false,
            app_menu_bar,
        }
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        // Save tabs before quitting
        self.editor_panel.update(cx, |panel, cx| {
            panel.save_tabs(cx);
        });

        // Give a moment for the save to complete
        std::thread::sleep(std::time::Duration::from_millis(100));
        cx.quit();
    }

    fn on_about(&mut self, _: &About, _: &mut Window, _: &mut Context<Self>) {
        println!("Blanco SQL Editor v0.1.0");
    }

    fn on_new_query(&mut self, _: &NewQuery, window: &mut Window, cx: &mut Context<Self>) {
        // Create a new query tab with default connection
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_new_tab_with_connection(
                window,
                "Test Database".to_string(),
                ConnectionType::SQLite,
                None,
                cx,
            );
        });
        cx.notify();
    }

    fn on_new_query_for_connection(
        &mut self,
        action: &NewQueryForConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "on_new_query_for_connection called: {}",
            action.connection_name
        );
        // Create a new query tab for the specified connection
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_new_tab_with_connection(
                window,
                action.connection_name.clone(),
                action.connection_type.clone(),
                None,
                cx,
            );
        });
        cx.notify();
    }

    fn on_new_query_for_postgres_connection(
        &mut self,
        action: &NewQueryForPostgresConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "on_new_query_for_postgres_connection called: {}",
            action.display_name
        );
        // Create a new query tab for the specified PostgreSQL connection
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_new_tab_with_postgres_connection(
                window,
                action.display_name.clone(),
                action.connection_key.clone(),
                None,
                cx,
            );
        });
        cx.notify();
    }

    fn on_new_query_for_postgres_schema(
        &mut self,
        action: &NewQueryForPostgresSchema,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!(
            "on_new_query_for_postgres_schema called: {}",
            action.schema_name
        );
        // Create a new query tab for the specified PostgreSQL schema
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_new_tab_with_postgres_connection(
                window,
                format!("PostgreSQL ({})", action.schema_name),
                action.connection_key.clone(),
                Some(action.schema_name.clone()),
                cx,
            );
        });
        cx.notify();
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

        window.open_modal(cx, move |modal, window, cx| {
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

                            crate::gpui_tokio::Tokio::spawn_result(cx, async move {
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
        content_for_focus.read(cx).first_input_focus_handle(cx).focus(window);
    }
}

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

        v_flex()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_about))
            .on_action(cx.listener(Self::on_new_query))
            .on_action(cx.listener(Self::on_new_query_for_connection))
            .on_action(cx.listener(Self::on_new_query_for_postgres_connection))
            .on_action(cx.listener(Self::on_new_query_for_postgres_schema))
            .on_action(cx.listener(Self::on_open_connection))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .on_action(cx.listener(Self::on_new_connection_modal))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Title bar
            .child(
                TitleBar::new()
                    .child(div().flex().items_center().child(self.app_menu_bar.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .px_2()
                            .gap_2()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new("github")
                                    .icon(IconName::GitHub)
                                    .small()
                                    .ghost()
                                    .on_click(|_, _, cx| {
                                        cx.open_url("https://github.com/yourusername/blanco")
                                    }),
                            ),
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
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Blanco".into(),
            items: vec![
                MenuItem::action("Preferences...", OpenSettings),
                MenuItem::Separator,
                MenuItem::action("About Blanco", About),
                MenuItem::Separator,
                MenuItem::action("Quit", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Query", NewQuery),
                MenuItem::Separator,
                MenuItem::action("New Connection", OpenNewConnectionModal),
                MenuItem::action("Open Connection", OpenConnection),
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
            name: "View".into(),
            items: vec![MenuItem::action("Toggle Sidebar", ToggleSidebar)],
        },
    ]);
}
