use blanco_ui::{Tab, TabBar};
use gpui::{
    Action, App, AppContext, BorrowAppContext, Context, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, Menu, MenuItem, ParentElement, Render, SharedString, Styled,
    Subscription, Task, Window, actions, div, prelude::FluentBuilder, px, svg,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Root, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogFooter},
    global_state::GlobalState,
    h_flex,
    menu::AppMenuBar,
    notification::NotificationType,
    resizable::{ResizablePanel, ResizableState, h_resizable, resizable_panel},
    spinner::Spinner,
    status_bar::StatusBar,
};
use serde::Deserialize;
use smol::channel;
use tracing::{debug, error, info};

use crate::{
    app_settings::AppSettings,
    command_palette::CommandPalette,
    connection_credentials,
    connection_modal::NewConnectionModal,
    connections::{ConnectionsPanel, ConnectionsPanelEvent},
    editor::{
        EditorPanel, EditorPanelEvent, ObjectDdlParams, TabCreationParams, TableStructureParams,
    },
    history_panel::{HistoryPanel, HistoryPanelEvent},
    result_ext::ResultExt,
    snippets_panel::{RefreshSnippets, SnippetsPanel, SnippetsPanelEvent},
    status_bar::{ActivityMessage, ActivityReporter, ActivityResult, StatusBarState, StatusKind},
};
use app_database::{AppDatabase, ConnectionData, EnvironmentType};

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
        CloseActiveTab,
        StartCellEdit,
        EditNextCell,
        EditPrevCell,
    ]
);

#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct ConnectToConnection {
    pub connection_id: i64,
}

/// Bring an editor tab to the front by its position in the tab strip.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct ActivateEditorTab {
    pub index: usize,
}

/// Switch to a theme registered in the `ThemeRegistry` by name.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct SwitchTheme {
    pub name: String,
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
pub struct SetCellDefault {
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
    /// When set, the new tab immediately inspects `table_name` as a key (used by
    /// the "Inspect Key" action on key/value backends). Plain "New Query" leaves
    /// this false and just opens an editor tab.
    pub inspect_key: bool,
}

/// Open a JavaScript script tab on a connection. Scripts are connection-scoped
/// rather than object-scoped, so there is no table to scaffold from.
#[derive(Action, Clone, PartialEq, Eq)]
#[action(namespace = blanco_app, no_json)]
pub struct CreateNewScriptTab {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
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
pub struct OpenSchemaGraph {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
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

/// Corner radius of the sidebar and editor cards. Deliberately separate from
/// `theme.radius`, which stays smaller for the controls inside the cards.
pub(crate) const PANEL_RADIUS: gpui::Pixels = px(8.);

/// Gap between the cards and the window edges. Half of it is applied as padding
/// on each side of the split so the two cards end up `PANEL_GAP` apart.
pub(crate) const PANEL_GAP: gpui::Pixels = px(6.);

/// Which view is active in the sidebar's segmented tab bar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarTab {
    Connections,
    Snippets,
    History,
}

pub struct BlancoApp {
    focus_handle: FocusHandle,
    sidebar: Entity<ConnectionsPanel>,
    snippets_panel: Entity<SnippetsPanel>,
    history_panel: Entity<HistoryPanel>,
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
    _save_window_bounds_task: Task<()>,
}

impl BlancoApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        init_menus(cx);

        let action_task = Self::spawn_service_listener(cx);
        let (status_bar, activity_task) = Self::spawn_activity_status(cx);

        let sidebar = cx.new(|cx| ConnectionsPanel::new(window, cx));
        let snippets_panel = cx.new(|cx| SnippetsPanel::new(window, cx));
        let history_panel = cx.new(|cx| HistoryPanel::new(window, cx));
        let main_resize_state = cx.new(|_| ResizableState::default());

        let saved_tabs = Self::load_saved_tabs(cx);
        let editor_panel = cx.new(|cx| EditorPanel::new_with_saved_tabs(window, cx, saved_tabs));
        let command_palette = cx.new(|cx| {
            CommandPalette::new(sidebar.downgrade(), editor_panel.downgrade(), window, cx)
        });
        let app_menu_bar = AppMenuBar::new(cx);

        let subscriptions = Self::wire_subscriptions(
            &sidebar,
            &snippets_panel,
            &history_panel,
            &editor_panel,
            &status_bar,
            window,
            cx,
        );

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        Self {
            focus_handle,
            sidebar,
            snippets_panel,
            history_panel,
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
            _save_window_bounds_task: Task::ready(()),
        }
    }

    /// Spawn the background task that translates `DatabaseServiceMessage`s
    /// (emitted from any context, including tokio tasks without a `cx`) into
    /// dispatched `DatabaseConnected`/`DatabaseDisconnected` actions. Installs
    /// the message sender on the global `DatabaseService`.
    fn spawn_service_listener(cx: &mut Context<Self>) -> Task<()> {
        let (action_sender, action_receiver) =
            channel::unbounded::<database::DatabaseServiceMessage>();

        cx.update_global::<database::DatabaseService, _>(
            |db_service: &mut database::DatabaseService, _cx| {
                db_service.set_action_sender(action_sender);
            },
        );

        cx.spawn(async move |_weak_handle, cx| {
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
        })
    }

    /// Create the status-bar entity and the background task that folds
    /// app-wide `ActivityMessage`s (reported through the global
    /// `ActivityReporter`) into it.
    fn spawn_activity_status(cx: &mut Context<Self>) -> (Entity<StatusBarState>, Task<()>) {
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
        (status_bar, activity_task)
    }

    /// Synchronously load the persisted query tabs from the app database. Blocks
    /// on the tokio runtime because the app cannot render until it knows which
    /// tabs to restore. Load failures degrade to an empty tab set.
    fn load_saved_tabs(cx: &mut Context<Self>) -> Vec<app_database::QueryTabData> {
        info!("Loading saved tabs from database");
        let app_database = AppDatabase::global(cx);
        gpui_tokio::Tokio::handle(cx).block_on(async {
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
        })
    }

    /// Wire the cross-panel event subscriptions and window observers. Returns
    /// the subscription guards to be stored on the app so they live as long as
    /// it does.
    fn wire_subscriptions(
        sidebar: &Entity<ConnectionsPanel>,
        snippets_panel: &Entity<SnippetsPanel>,
        history_panel: &Entity<HistoryPanel>,
        editor_panel: &Entity<EditorPanel>,
        status_bar: &Entity<StatusBarState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        let mut subscriptions = Vec::new();

        // Sidebar: open the edit-connection modal, and the first-run dialog.
        subscriptions.push(cx.subscribe_in(
            sidebar,
            window,
            move |app, sidebar, event, window, cx| match event {
                ConnectionsPanelEvent::EditConnection {
                    connection_data, ..
                } => {
                    app.open_edit_connection_modal(*connection_data.clone(), window, cx);
                }
                // First-run experience: if the initial load found no saved connections,
                // greet the user with the new connection dialog instead of an empty window.
                ConnectionsPanelEvent::FirstLoadCompleted => {
                    if sidebar.read(cx).connections.is_empty() {
                        app.on_new_connection_modal(&OpenNewConnectionModal, window, cx);
                    }
                }
            },
        ));

        // Snippets panel: deletion already refreshes the panel itself.
        subscriptions.push(cx.subscribe_in(
            snippets_panel,
            window,
            move |_app, _snippets_panel, event, _window, _cx| match event {
                SnippetsPanelEvent::SnippetDeleted => {}
            },
        ));

        // History panel: drop a saved query into the active editor tab.
        subscriptions.push(cx.subscribe_in(
            history_panel,
            window,
            move |app, _history_panel, event, window, cx| match event {
                HistoryPanelEvent::InsertQuery(query) => {
                    let query = query.clone();
                    app.editor_panel.update(cx, |editor_panel, cx| {
                        editor_panel.insert_into_active_query(&query, window, cx);
                    });
                }
            },
        ));

        // Editor panel: keep the history panel in sync as queries run.
        subscriptions.push(cx.subscribe_in(
            editor_panel,
            window,
            move |app, _editor_panel, event, _window, cx| match event {
                EditorPanelEvent::QueryRecorded => {
                    app.history_panel.update(cx, |panel, cx| {
                        panel.reload(cx);
                    });
                }
            },
        ));

        // Re-render the status bar whenever the activity state changes.
        subscriptions.push(cx.observe(status_bar, |_, _, cx| cx.notify()));

        // Persist window geometry as it changes (debounced in the handler).
        subscriptions.push(cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_window_bounds(window, cx);
        }));

        subscriptions
    }

    #[cfg(test)]
    pub fn editor_panel(&self) -> &Entity<EditorPanel> {
        &self.editor_panel
    }

    /// Persist the current window geometry to the app settings database so it
    /// can be restored on the next launch. Called on every resize/move, so the
    /// database write is debounced to avoid hammering it during a drag.
    fn persist_window_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (maximized, bounds) = match window.window_bounds() {
            gpui::WindowBounds::Maximized(bounds) => (true, bounds),
            gpui::WindowBounds::Fullscreen(bounds) => (true, bounds),
            gpui::WindowBounds::Windowed(bounds) => (false, bounds),
        };

        let x = f32::from(bounds.origin.x);
        let y = f32::from(bounds.origin.y);
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);

        cx.update_global::<AppSettings, _>(|app_settings, _| {
            let window = &mut app_settings.settings.window;
            window.x = Some(x);
            window.y = Some(y);
            window.width = Some(width);
            window.height = Some(height);
            window.maximized = maximized;
        });

        let db = AppDatabase::global(cx).clone();
        self._save_window_bounds_task = cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let values = [
                ("window.x", x.to_string()),
                ("window.y", y.to_string()),
                ("window.width", width.to_string()),
                ("window.height", height.to_string()),
                ("window.maximized", maximized.to_string()),
            ];
            for (key, value) in values {
                if let Err(e) = db.save_setting(key, &value, false).await {
                    error!("Failed to save {}: {}", key, e);
                }
            }
        });
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

    /// The window title bar: app logo and menu bar on the left, the sidebar
    /// toggle on the right. `TitleBar` lays its children out left of the window
    /// controls, so a trailing child lands next to minimize/maximize/close.
    fn render_title_bar(&self, window: &mut Window, cx: &Context<Self>) -> impl IntoElement {
        TitleBar::new().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .w_full()
                .child(
                    h_flex()
                        .items_center()
                        .gap_x_3()
                        .child(
                            svg()
                                .h(px(40.))
                                .w(px(128.))
                                .text_color(window.text_style().color)
                                .path("images/blanco.svg"),
                        )
                        .child(self.app_menu_bar.clone()),
                )
                .child(self.render_sidebar_toggle(cx)),
        )
    }

    fn render_sidebar_toggle(&self, _cx: &Context<Self>) -> impl IntoElement {
        Button::new("toggle-sidebar")
            .ghost()
            .small()
            .icon(if self.sidebar_collapsed {
                Icon::new(IconName::PanelLeftOpen).size_4()
            } else {
                Icon::new(IconName::PanelLeftClose).size_4()
            })
            .tooltip("Toggle Sidebar")
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(ToggleSidebar), cx);
            })
    }

    /// The left sidebar: a segmented tab strip (Connections / Snippets /
    /// History) over the currently selected panel.
    fn render_sidebar_panel(&self, cx: &Context<Self>) -> ResizablePanel {
        resizable_panel()
            // Kept in the tree while collapsed rather than omitted, so the
            // group's state retains the width across a collapse/re-open.
            .visible(!self.sidebar_collapsed)
            .size(px(288.))
            .size_range(px(288.)..px(500.))
            .flex_none()
            .child(
                div().size_full().pr(PANEL_GAP / 2.).child(
                    div()
                        .w_full()
                        .h_full()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .bg(cx.theme().sidebar)
                        .rounded(PANEL_RADIUS)
                        .border_1()
                        .border_color(cx.theme().border)
                        // The segmented trough paints its own background, so it
                        // is inset by this wrapper rather than padding the bar
                        // itself, which would otherwise square the card's top
                        // corners.
                        .child(
                            div().flex_none().p(PANEL_GAP).child(
                                TabBar::new("sidebar-tabs")
                                    .segmented()
                                    .w_full()
                                    .selected_index(match self.sidebar_tab {
                                        SidebarTab::Connections => 0,
                                        SidebarTab::Snippets => 1,
                                        SidebarTab::History => 2,
                                    })
                                    .on_click({
                                        let view = cx.entity().downgrade();
                                        move |ix: &usize, _, _, cx| {
                                            let tab = match ix {
                                                1 => SidebarTab::Snippets,
                                                2 => SidebarTab::History,
                                                _ => SidebarTab::Connections,
                                            };
                                            view.update(cx, |this, cx| {
                                                this.sidebar_tab = tab;
                                                // Pick up queries run since this
                                                // panel was last shown.
                                                if tab == SidebarTab::History {
                                                    this.history_panel.update(cx, |panel, cx| {
                                                        panel.reload(cx);
                                                    });
                                                }
                                                cx.notify();
                                            })
                                            .log_err();
                                        }
                                    })
                                    .child(Tab::new().label("Connections").flex_1())
                                    .child(Tab::new().label("Snippets").flex_1())
                                    .child(Tab::new().label("History").flex_1()),
                            ),
                        )
                        .child(div().flex_1().min_h_0().overflow_hidden().map(|this| {
                            match self.sidebar_tab {
                                SidebarTab::Connections => this.child(self.sidebar.clone()),
                                SidebarTab::Snippets => this.child(self.snippets_panel.clone()),
                                SidebarTab::History => this.child(self.history_panel.clone()),
                            }
                        })),
                ),
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
        cx.notify();
    }

    fn on_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.add_settings_tab(window, cx);
        });
    }

    fn on_activate_editor_tab(
        &mut self,
        action: &ActivateEditorTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = action.index;
        self.editor_panel.update(cx, |panel, cx| {
            panel.activate_tab(index, window, cx);
        });
    }

    fn on_close_active_tab(
        &mut self,
        _: &CloseActiveTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_panel.update(cx, |panel, cx| {
            panel.close_active_tab(cx);
        });
    }

    fn on_switch_theme(
        &mut self,
        action: &SwitchTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::settings::apply_theme_by_name(&action.name, cx);
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
        window: &mut Window,
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

        // Non-SQL backends (Redis) don't get a SELECT scaffold; a clicked key
        // opens the key inspector instead (handled below).
        let content = if action.db_type.supports_sql() {
            action
                .table_name
                .as_ref()
                .map(|table| match &action.schema_name {
                    Some(schema) => format!("SELECT * FROM {}.{} LIMIT 100;", schema, table),
                    None => format!("SELECT * FROM {} LIMIT 100;", table),
                })
        } else {
            None
        };

        self.editor_panel.update(cx, |panel, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title,
                    content,
                    db_id: None,
                    last_run_at: None,
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

        // The "Inspect Key" action drills the freshly opened tab straight into
        // the key inspector; plain "New Query" leaves the editor tab empty.
        if action.inspect_key
            && let Some(key) = action.table_name.clone()
        {
            self.open_redis_key(
                action.connection_id,
                action.database_name.clone(),
                key,
                window,
                cx,
            );
        }

        cx.notify();
    }

    fn on_create_new_script_tab(
        &mut self,
        action: &CreateNewScriptTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = match &action.schema_name {
            Some(schema) => format!("{}.{} script", action.database_name, schema),
            None => format!("{} script", action.database_name),
        };

        self.editor_panel.update(cx, |panel, cx| {
            panel.create_and_add_script_tab(
                window,
                TabCreationParams {
                    title,
                    content: None,
                    db_id: None,
                    last_run_at: None,
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

    /// Inspect a Redis key in a background task and route the decoded value to
    /// the (freshly created, now active) query tab's results panel.
    fn open_redis_key(
        &mut self,
        connection_id: i64,
        database_name: String,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(results_panel) = self.editor_panel.read(cx).active_results_panel() else {
            return;
        };
        let db_service = database::DatabaseService::global(cx).clone();

        cx.spawn_in(window, async move |_, cx| {
            use database::DatabaseServiceTrait;
            let connection = match db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await
            {
                Ok(connection) => connection,
                Err(e) => {
                    tracing::error!("Failed to open Redis connection: {e}");
                    return;
                }
            };
            let result = match connection.inspect_key(Some(&database_name), &key).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Failed to inspect Redis key {key}: {e}");
                    return;
                }
            };
            results_panel
                .update_in(cx, |panel, window, cx| {
                    panel.set_key_value_result(result, window, cx);
                })
                .log_err();
        })
        .detach();
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
                            Err(e) => tab.set_error(format!("{e:#}"), cx),
                        });
                    }
                    let _ = window;
                })
                .log_err();
        })
        .detach();

        cx.notify();
    }

    fn on_open_schema_graph(
        &mut self,
        action: &OpenSchemaGraph,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::editor::schema_graph::SchemaGraphParams;

        self.editor_panel.update(cx, |panel, cx| {
            panel.create_schema_graph_tab(
                SchemaGraphParams {
                    connection_id: action.connection_id,
                    connection_name: action.connection_name.clone(),
                    db_type: action.db_type,
                    database_name: action.database_name.clone(),
                    schema_name: action.schema_name.clone(),
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
        let editor_panel = self.editor_panel.clone();

        cx.spawn_in(window, async move |_, window| {
            use database::DatabaseServiceTrait;
            let connection_result = db_service
                .get_or_create_connection_by_id(connection_id, Some(&database_name))
                .await;

            match connection_result {
                Ok(connection) => {
                    let tables_result = connection
                        .get_tables(schema_name.as_deref())
                        .await
                        .unwrap_or_default();

                    let mut all_table_info = Vec::new();
                    for table_name in &tables_result {
                        let columns = connection
                            .get_columns_for_table(table_name, schema_name.as_deref())
                            .await
                            .unwrap_or_default();

                        all_table_info.push(blanco_core::connection_trait::TableSchemaInfo {
                            name: table_name.clone(),
                            schema: schema_name.clone().unwrap_or_default(),
                            object_type: "table".to_string(),
                            columns,
                            column_count: 0,
                            referenced_by: Vec::new(),
                        });
                    }

                    window
                        .update(|window, cx| {
                            editor_panel.update(cx, |panel, cx| {
                                panel.update_last_schema_graph_tab(all_table_info, window, cx);
                            });
                        })
                        .log_err();
                }
                Err(e) => {
                    window
                        .update(|_window, cx| {
                            editor_panel.update(cx, |panel, cx| {
                                panel.set_schema_graph_error(
                                    format!("Failed to connect: {e}"),
                                    _window,
                                    cx,
                                );
                            });
                        })
                        .log_err();
                }
            }
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
                            window
                                .spawn(cx, async move |cx| {
                                    let result: anyhow::Result<()> = async {
                                        let connection_id =
                                            app_database.save_connection(&conn_data).await?;
                                        tracing::info!("Connection saved with ID: {connection_id}");
                                        let mut connection_data = conn_data;
                                        connection_data.id = Some(connection_id);
                                        let credential_tasks = cx.update(|_, cx| {
                                            connection_credentials::start_writing_connection(
                                                connection_id,
                                                &connection_data,
                                                cx,
                                            )
                                        })?;
                                        if let Err(error) =
                                            connection_credentials::finish_writing(credential_tasks)
                                                .await
                                        {
                                            if let Err(cleanup_error) = app_database
                                                .delete_connection(connection_id)
                                                .await
                                            {
                                                tracing::error!(
                                                    "Failed to remove connection metadata after credential storage failed: {cleanup_error}"
                                                );
                                            }
                                            let cleanup_tasks = cx.update(|_, cx| {
                                                connection_credentials::start_deleting_connection(
                                                    connection_id,
                                                    cx,
                                                )
                                            })?;
                                            if let Err(cleanup_error) =
                                                connection_credentials::finish_writing(cleanup_tasks)
                                                    .await
                                            {
                                                tracing::error!(
                                                    "Failed to remove partially saved credentials: {cleanup_error}"
                                                );
                                            }
                                            return Err(error);
                                        }
                                        if let Some(config) = connection_data.to_connection_config()
                                        {
                                            db_service.add_connection_config(config).await;
                                        }
                                        sidebar.update_in(cx, |panel, _window, cx| {
                                            panel.reload_connections(cx);
                                        })?;
                                        Ok(())
                                    }
                                    .await;

                                    cx.update(|window, cx| match result {
                                        Ok(()) => window.push_notification(
                                            (
                                                NotificationType::Success,
                                                "Connection saved successfully",
                                            ),
                                            cx,
                                        ),
                                        Err(error) => {
                                            tracing::error!("Failed to save connection: {error:#}");
                                            window.push_notification(
                                                (
                                                    NotificationType::Error,
                                                    SharedString::from(format!(
                                                        "Failed to save connection: {error:#}"
                                                    )),
                                                ),
                                                cx,
                                            );
                                        }
                                    })
                                    .log_err();
                                })
                                .detach();
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
                            window
                                .spawn(cx, async move |cx| {
                                    let result: anyhow::Result<()> = async {
                                        let connection_id =
                                            app_database.save_connection(&conn_data).await?;
                                        tracing::info!(
                                            "Connection updated with ID: {connection_id}"
                                        );
                                        let mut connection_data = conn_data;
                                        connection_data.id = Some(connection_id);
                                        let credential_tasks = cx.update(|_, cx| {
                                            connection_credentials::start_writing_connection(
                                                connection_id,
                                                &connection_data,
                                                cx,
                                            )
                                        })?;
                                        connection_credentials::finish_writing(credential_tasks)
                                            .await?;
                                        if let Some(config) = connection_data.to_connection_config()
                                        {
                                            db_service.add_connection_config(config).await;
                                        }
                                        sidebar.update_in(cx, |panel, _window, cx| {
                                            panel.reload_connections(cx);
                                        })?;
                                        Ok(())
                                    }
                                    .await;

                                    cx.update(|window, cx| match result {
                                        Ok(()) => window.push_notification(
                                            (
                                                NotificationType::Success,
                                                "Connection updated successfully",
                                            ),
                                            cx,
                                        ),
                                        Err(error) => {
                                            tracing::error!(
                                                "Failed to update connection: {error:#}"
                                            );
                                            window.push_notification(
                                                (
                                                    NotificationType::Error,
                                                    SharedString::from(format!(
                                                        "Failed to update connection: {error:#}"
                                                    )),
                                                ),
                                                cx,
                                            );
                                        }
                                    })
                                    .log_err();
                                })
                                .detach();
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
            .key_context("BlancoApp")
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::on_settings))
            .on_action(cx.listener(Self::on_create_new_query_tab))
            .on_action(cx.listener(Self::on_create_new_script_tab))
            .on_action(cx.listener(Self::on_open_table_structure))
            .on_action(cx.listener(Self::on_open_object_ddl))
            .on_action(cx.listener(Self::on_open_schema_graph))
            .on_action(cx.listener(Self::on_new_connection_modal))
            .on_action(cx.listener(Self::on_new_snippet))
            .on_action(cx.listener(Self::on_commit_changes))
            .on_action(cx.listener(Self::on_rollback_changes))
            .on_action(cx.listener(Self::on_rename_tab))
            .on_action(cx.listener(Self::on_database_connected))
            .on_action(cx.listener(Self::on_database_disconnected))
            .on_action(cx.listener(Self::on_open_snippet_editor))
            .on_action(cx.listener(Self::on_refresh_snippets))
            .on_action(cx.listener(Self::on_activate_editor_tab))
            .on_action(cx.listener(Self::on_close_active_tab))
            .on_action(cx.listener(Self::on_switch_theme))
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
            .child(self.render_title_bar(window, cx))
            // Main content area: the cards, inset from the window edges. The
            // vertical insets are kept minimal so the cards sit close under the
            // title bar and just above the status bar; only the sides and the
            // split between the cards get the full gap.
            .child(
                div()
                    .flex()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .overflow_hidden()
                    .px(PANEL_GAP)
                    .pb(px(2.))
                    .pt(px(1.))
                    .child(
                        h_resizable("main-layout")
                            .with_state(&self.main_resize_state)
                            // The cards draw their own borders, so the handle
                            // only needs to stay draggable; its line would
                            // otherwise float in the gutter and overshoot the
                            // rounded corners.
                            .invisible_handles()
                            // Left side: Connections panel sidebar
                            .child(self.render_sidebar_panel(cx))
                            // Main panel
                            .child(
                                resizable_panel().child(
                                    div()
                                        .size_full()
                                        .when(!self.sidebar_collapsed, |this| {
                                            this.pl(PANEL_GAP / 2.)
                                        })
                                        .child(
                                            div()
                                                .flex()
                                                .flex_1()
                                                // Let this shrink below its
                                                // content width, or the panel
                                                // grows to fit the widest tab
                                                // and the tab bar never scrolls.
                                                .min_w_0()
                                                .h_full()
                                                .overflow_hidden()
                                                .bg(cx.theme().background)
                                                .rounded(PANEL_RADIUS)
                                                // This card shares the shell
                                                // colour, so the border is what
                                                // makes its outline readable.
                                                .border_1()
                                                .border_color(cx.theme().border)
                                                .child(self.editor_panel.clone()),
                                        ),
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
    // User-customizable shortcuts, resolved from settings (defaults overlaid
    // with any user overrides).
    let settings = AppSettings::global(cx).settings.clone();
    cx.bind_keys(crate::keybindings::customizable_key_bindings(&settings));

    // Quit is platform-fixed and not user-configurable.
    cx.bind_keys([
        #[cfg(target_os = "macos")]
        gpui::KeyBinding::new("cmd-q", Quit, None),
        #[cfg(not(target_os = "macos"))]
        gpui::KeyBinding::new("alt-f4", Quit, None),
    ]);

    // Results-table cell editing. "DataTable" is the table's own key context;
    // "CellEditor" wraps the inline cell input, so its bindings win over the
    // table's tab -> SelectNextColumn while an edit is in flight (the input's
    // own tab -> IndentInline binding matches first but is unhandled for
    // single-line inputs, so the keystroke falls through to these).
    cx.bind_keys([
        gpui::KeyBinding::new("enter", StartCellEdit, Some("DataTable")),
        gpui::KeyBinding::new("f2", StartCellEdit, Some("DataTable")),
        gpui::KeyBinding::new("tab", EditNextCell, Some("CellEditor")),
        gpui::KeyBinding::new("shift-tab", EditPrevCell, Some("CellEditor")),
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
