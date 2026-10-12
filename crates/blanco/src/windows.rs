//! Blanco's windows. Each shows its own tabs over the connections they all
//! share. A window's place and tabs are stored in the app database's
//! `windows` table, so the next start reopens every window open at quit, on
//! the display it was on. Closing one of several windows forgets it and its
//! tabs; closing the last one quits and keeps it.
//!
//! The work that is not one window's (database service messages, idle
//! disconnects, files the platform asks to open, MCP requests) runs once here
//! and is handed to the window it concerns.

use std::time::Duration;

use app_database::{AppDatabase, WindowData, WindowGeometry};
use gpui::{
    App, AppContext as _, AsyncApp, BorrowAppContext as _, Bounds, Context, Entity, Global, Pixels,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions, point, px, size,
};
use gpui_component::Root;
use smol::channel;
use tracing::{error, info};

use crate::app::{BlancoApp, NewWindow};
use crate::app_settings::AppSettings;
use crate::editor::MovedTab;
use crate::mcp::McpService;
use crate::result_ext::ResultExt as _;
use crate::sqlite_file;
use crate::status_bar::{ActivityResult, StatusBarState};
use crate::update_manager::UpdateManager;

/// Matches `StartupWMClass` in `resources/blanco.desktop`, so the desktop
/// attributes windows and notifications to the installed launcher.
pub const APP_ID: &str = "com.blanco.sql-editor";

/// How far a new window sits from the one it was opened from, so both stay
/// visible.
const CASCADE_OFFSET: f32 = 28.;

const DEFAULT_SIZE: (f32, f32) = (1400., 900.);

/// One open window.
struct OpenWindow {
    handle: WindowHandle<Root>,
    app: WeakEntity<BlancoApp>,
    /// Its row in the `windows` table.
    id: i64,
}

/// The open windows, the most recently active last.
#[derive(Default)]
struct OpenWindows {
    windows: Vec<OpenWindow>,
    /// Set when quitting starts, so the windows closing on the way out are
    /// kept for the next start rather than forgotten.
    quitting: bool,
}

impl Global for OpenWindows {}

/// Start the app-wide services every window relies on. Called once, before
/// the first window opens.
pub fn init(cx: &mut App) {
    cx.set_global(OpenWindows::default());
    StatusBarState::shared(cx);
    cx.on_action(|_: &NewWindow, cx| {
        // The action arrives while its window is updating, and the new window
        // reads that window's bounds.
        cx.defer(|cx| {
            open_new(None, cx).log_err();
        });
    });
    cx.on_window_closed(forget_closed_window).detach();
    cx.on_app_quit(|cx| {
        cx.global_mut::<OpenWindows>().quitting = true;
        async {}
    })
    .detach();
    cx.on_system_notification_response(|response, cx| {
        if response.tag != crate::app::MCP_CONFIRMATION_NOTIFICATION_TAG
            && response.tag != crate::backup::NOTIFICATION_TAG
        {
            return;
        }
        cx.activate(true);
        if let Some(window) = frontmost_window(cx) {
            window
                .handle
                .update(cx, |_, window, _| window.activate_window())
                .log_err();
        }
    });
    spawn_service_listener(cx);
    spawn_idle_disconnect(cx);
    spawn_mcp_listener(cx);
}

/// Reopen the windows open when Blanco last quit, or one new window on the
/// first start, then start answering requests to open database files.
pub fn restore(cx: &mut App) {
    let app_database = AppDatabase::global(cx).clone();
    let saved = gpui_tokio::Tokio::handle(cx)
        .block_on(async move { app_database.load_windows().await })
        .unwrap_or_else(|error| {
            error!("Failed to load the saved windows: {error}");
            Vec::new()
        });
    info!("Restoring {} windows", saved.len());

    let mut opened = 0;
    for window in saved {
        match open(window, None, cx) {
            Ok(_) => opened += 1,
            Err(error) => error!("Failed to reopen a window: {error}"),
        }
    }
    if opened == 0 {
        open_new(None, cx).log_err();
    }

    if let Some(update_manager) = UpdateManager::try_global(cx)
        && update_manager.updated_from.is_some()
        && let Some(window) = frontmost_window(cx)
    {
        update_app(&window, cx, |_, window, cx| {
            BlancoApp::notify_updated(window, cx)
        });
    }

    spawn_open_file_listener(cx);
}

/// Open a new window over the frontmost one, showing `tab` if given.
pub fn open_new(
    tab: Option<MovedTab>,
    cx: &mut App,
) -> anyhow::Result<(WindowHandle<Root>, Entity<BlancoApp>)> {
    let (geometry, display_uuid) = match frontmost_window(cx) {
        Some(window) => window
            .handle
            .update(cx, |_, window, cx| cascaded_geometry(window, cx))?,
        None => (None, None),
    };
    let app_database = AppDatabase::global(cx).clone();
    let id = gpui_tokio::Tokio::handle(cx).block_on({
        let display_uuid = display_uuid.clone();
        async move { app_database.create_window(geometry, display_uuid).await }
    })?;
    open(
        WindowData {
            id,
            geometry,
            display_uuid,
            active_tab_id: None,
        },
        tab,
        cx,
    )
}

/// The geometry of a window opened over `window`: the same size, moved down
/// and right, on the same display.
fn cascaded_geometry(window: &Window, cx: &App) -> (Option<WindowGeometry>, Option<String>) {
    let bounds = window.window_bounds().get_bounds();
    let geometry = WindowGeometry {
        x: f32::from(bounds.origin.x) + CASCADE_OFFSET,
        y: f32::from(bounds.origin.y) + CASCADE_OFFSET,
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        maximized: false,
    };
    (Some(geometry), display_uuid(window, cx))
}

/// The UUID of the display `window` is on, which stays the same across
/// restarts where the platform's display id may not.
pub(crate) fn display_uuid(window: &Window, cx: &App) -> Option<String> {
    window
        .display(cx)
        .and_then(|display| display.uuid().ok())
        .map(|uuid| uuid.to_string())
}

fn open(
    saved: WindowData,
    tab: Option<MovedTab>,
    cx: &mut App,
) -> anyhow::Result<(WindowHandle<Root>, Entity<BlancoApp>)> {
    let options = window_options(&saved, cx);
    let id = saved.id;
    let mut app = None;
    let handle = cx.open_window(options, |window, cx| {
        let blanco_app = cx.new(|cx| BlancoApp::new(&saved, window, cx));
        if let Some(tab) = tab {
            blanco_app.update(cx, |app, cx| app.adopt_tab(tab, window, cx));
        }
        window.on_window_should_close(cx, {
            let blanco_app = blanco_app.downgrade();
            move |window, cx| {
                blanco_app
                    .update(cx, |app, cx| app.may_close_window(window, cx))
                    .unwrap_or(true)
            }
        });
        #[cfg(feature = "screenshots")]
        if let Some(scene) = crate::screenshots::Scene::from_env() {
            crate::screenshots::stage(scene, blanco_app.clone(), window, cx);
        }
        app = Some(blanco_app.clone());
        cx.new(|cx| Root::new(blanco_app, window, cx))
    })?;
    let app = app.ok_or_else(|| anyhow::anyhow!("the window was opened without its app"))?;
    cx.global_mut::<OpenWindows>().windows.push(OpenWindow {
        handle,
        app: app.downgrade(),
        id,
    });
    Ok((handle, app))
}

fn window_options(saved: &WindowData, cx: &App) -> WindowOptions {
    let saved_display = saved.display_uuid.as_ref().map(|uuid| {
        cx.displays().into_iter().find(|display| {
            display
                .uuid()
                .is_ok_and(|display_uuid| display_uuid.to_string() == *uuid)
        })
    });
    let display_id = saved_display.clone().flatten().map(|display| display.id());
    let window_bounds = match saved.geometry {
        // The bounds are relative to the display they were saved on, so they
        // only hold on that display.
        Some(geometry) if !matches!(saved_display, Some(None)) => {
            let bounds = Bounds {
                origin: point(px(geometry.x), px(geometry.y)),
                size: size(px(geometry.width), px(geometry.height)),
            };
            maximized_or_windowed(geometry.maximized, bounds)
        }
        Some(geometry) => maximized_or_windowed(
            geometry.maximized,
            Bounds::centered(None, size(px(geometry.width), px(geometry.height)), cx),
        ),
        None => WindowBounds::Windowed(Bounds::centered(
            display_id,
            size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)),
            cx,
        )),
    };

    WindowOptions {
        window_bounds: Some(window_bounds),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Blanco".into()),
            appears_transparent: true,
            traffic_light_position: Some(gpui::Point {
                x: px(8.0),
                y: px(6.0),
            }),
        }),
        window_decorations: Some(gpui::WindowDecorations::Client),
        window_min_size: Some(size(px(1024.), px(768.))),
        focus: true,
        show: true,
        kind: gpui::WindowKind::Normal,
        is_movable: true,
        app_owns_titlebar_drag: true,
        // Matches gpui's own default: throttle to 30fps while inactive.
        inactive_frame_interval: Some(Duration::from_micros(33_333)),
        is_minimizable: true,
        is_resizable: true,
        tabbing_identifier: None,
        display_id,
        window_background: gpui::WindowBackgroundAppearance::Opaque,
        app_id: Some(APP_ID.into()),
        icon: None,
    }
}

fn maximized_or_windowed(maximized: bool, bounds: Bounds<Pixels>) -> WindowBounds {
    if maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

/// Note that the window stored as `id` came to the front, so requests that
/// name no window go to it, and the next start opens it on top.
pub(crate) fn note_activated(id: i64, cx: &mut App) {
    if !cx.has_global::<OpenWindows>() {
        return;
    }
    let windows = &mut cx.global_mut::<OpenWindows>().windows;
    if let Some(position) = windows.iter().position(|window| window.id == id) {
        let window = windows.remove(position);
        windows.push(window);
    }
    let app_database = AppDatabase::global(cx).clone();
    cx.spawn(async move |_| {
        app_database
            .touch_window_active(id)
            .await
            .map_err(anyhow::Error::from)
            .log_err();
    })
    .detach();
}

/// How many windows are open.
pub(crate) fn count(cx: &App) -> usize {
    cx.try_global::<OpenWindows>()
        .map_or(0, |open| open.windows.len())
}

/// Every open window's app.
pub(crate) fn apps(cx: &App) -> Vec<Entity<BlancoApp>> {
    cx.try_global::<OpenWindows>()
        .map(|open| {
            open.windows
                .iter()
                .filter_map(|window| window.app.upgrade())
                .collect()
        })
        .unwrap_or_default()
}

/// Run `f` on each open window's app with its window.
pub(crate) fn update_each(
    cx: &mut App,
    mut f: impl FnMut(&mut BlancoApp, &mut Window, &mut Context<BlancoApp>),
) {
    for window in window_list(cx) {
        update_app(&window, cx, &mut f);
    }
}

fn window_list(cx: &App) -> Vec<OpenWindow> {
    cx.try_global::<OpenWindows>()
        .map(|open| {
            open.windows
                .iter()
                .map(|window| OpenWindow {
                    handle: window.handle,
                    app: window.app.clone(),
                    id: window.id,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The active window when it is Blanco's, otherwise the one active last.
fn frontmost_window(cx: &App) -> Option<OpenWindow> {
    let windows = window_list(cx);
    let active = cx.active_window();
    let position = windows
        .iter()
        .position(|window| Some(window.handle.into()) == active)
        .or_else(|| windows.len().checked_sub(1))?;
    windows.into_iter().nth(position)
}

fn update_app<R>(
    window: &OpenWindow,
    cx: &mut App,
    f: impl FnOnce(&mut BlancoApp, &mut Window, &mut Context<BlancoApp>) -> R,
) -> Option<R> {
    let app = window.app.upgrade()?;
    // Not `WindowHandle::update`, which holds the `Root` view for the call:
    // the app's handlers read it, for dialogs and notifications.
    cx.update_window(window.handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| f(app, window, cx))
    })
    .log_err()
}

fn forget_closed_window(cx: &mut App, window_id: gpui::WindowId) {
    let open = cx.global_mut::<OpenWindows>();
    let Some(position) = open
        .windows
        .iter()
        .position(|window| window.handle.window_id() == window_id)
    else {
        return;
    };
    let closed = open.windows.remove(position);
    // The last window closing quits Blanco, and is the one the next start
    // shows.
    if open.quitting || open.windows.is_empty() {
        return;
    }
    let app_database = AppDatabase::global(cx).clone();
    cx.spawn(async move |_| {
        app_database
            .delete_window(closed.id)
            .await
            .map_err(anyhow::Error::from)
            .log_err();
    })
    .detach();
}

/// Translate `DatabaseServiceMessage`s, sent from any context including
/// tokio tasks without a `cx`, into each window's connected marks and the
/// status bar.
fn spawn_service_listener(cx: &mut App) {
    let (sender, receiver) = channel::unbounded::<database::DatabaseServiceMessage>();
    cx.update_global::<database::DatabaseService, _>(|db_service, _| {
        db_service.set_action_sender(sender);
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(message) = receiver.recv().await {
            cx.update(|cx| {
                let status_bar = StatusBarState::shared(cx);
                match message {
                    database::DatabaseServiceMessage::Connected(connected) => {
                        info!(
                            "Connected to {} on connection {}",
                            connected.database_name, connected.connection_id
                        );
                        for app in apps(cx) {
                            app.update(cx, |app, cx| {
                                app.on_database_connected(connected.connection_id, cx)
                            });
                        }
                        status_bar.update(cx, |state, cx| {
                            state.flash(
                                ActivityResult::Ok(
                                    format!("connected to {}", connected.database_name).into(),
                                ),
                                cx,
                            );
                        });
                    }
                    database::DatabaseServiceMessage::Disconnected(disconnected) => {
                        info!(
                            "Disconnected from {} on connection {}",
                            disconnected.database_name, disconnected.connection_id
                        );
                        for app in apps(cx) {
                            app.update(cx, |app, cx| {
                                app.on_database_disconnected(disconnected.connection_id, cx)
                            });
                        }
                        let result = if disconnected.idle {
                            ActivityResult::Ok(
                                format!("closed idle connection {}", disconnected.database_name)
                                    .into(),
                            )
                        } else {
                            ActivityResult::Err(
                                format!("disconnected from {}", disconnected.database_name).into(),
                            )
                        };
                        status_bar.update(cx, |state, cx| state.flash(result, cx));
                    }
                }
            });
        }
    })
    .detach();
}

/// Close connections that have sat unused past their idle timeout. A
/// connection with uncommitted grid edits in any window is kept open, and
/// the service itself skips one with a query running.
fn spawn_idle_disconnect(cx: &mut App) {
    const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(SWEEP_INTERVAL).await;
            let (db_service, default_timeout, keep) = cx.update(|cx| {
                let default_timeout = AppSettings::global(cx)
                    .settings
                    .database
                    .idle_disconnect_timeout();
                let keep: std::collections::HashSet<i64> = apps(cx)
                    .iter()
                    .flat_map(|app| {
                        app.read(cx)
                            .editor_panel()
                            .read(cx)
                            .connections_with_pending_edits(cx)
                    })
                    .collect();
                (
                    database::DatabaseService::global(cx).clone(),
                    default_timeout,
                    keep,
                )
            });
            db_service
                .disconnect_idle_connections(default_timeout, &keep)
                .await;
        }
    })
    .detach();
}

/// Open the database files passed at launch and those the platform asks the
/// running app to open, in the frontmost window.
fn spawn_open_file_listener(cx: &mut App) {
    let Some(requests) = cx
        .try_global::<sqlite_file::OpenFileRequests>()
        .map(|requests| requests.receiver.clone())
    else {
        return;
    };
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(paths) = requests.recv().await {
            cx.update(|cx| {
                let window = match frontmost_window(cx) {
                    Some(window) => Some(window),
                    None => open_new(None, cx)
                        .log_err()
                        .and_then(|_| frontmost_window(cx)),
                };
                if let Some(window) = window {
                    update_app(&window, cx, |app, window, cx| {
                        for path in &paths {
                            app.open_database_file(path, window, cx);
                        }
                    });
                }
            });
        }
    })
    .detach();
}

/// Answer MCP requests on the foreground, in the window holding the tab a
/// request names by id, otherwise in the frontmost window.
fn spawn_mcp_listener(cx: &mut App) {
    let Some(receiver) = McpService::take_bridge_receiver(cx) else {
        return;
    };
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(request) = receiver.recv().await {
            cx.update(|cx| {
                let window = request
                    .tab_id()
                    .and_then(|tab_id| {
                        window_list(cx).into_iter().find(|window| {
                            window.app.upgrade().is_some_and(|app| {
                                app.read(cx).editor_panel().read(cx).has_tab_id(tab_id)
                            })
                        })
                    })
                    .or_else(|| frontmost_window(cx));
                match window {
                    Some(window) => {
                        update_app(&window, cx, |app, window, cx| {
                            app.handle_mcp_request(request, window, cx)
                        });
                    }
                    None => error!("No window is open to answer an MCP request"),
                }
            });
        }
        info!("MCP bridge listener ended");
    })
    .detach();
}

/// Reload every window's connection list after a saved connection changed.
/// Deferred, so it can be asked for from inside any panel's update.
pub(crate) fn reload_connections(cx: &mut App) {
    cx.defer(|cx| {
        for app in apps(cx) {
            let sidebar = app.read(cx).sidebar().clone();
            sidebar.update(cx, |sidebar, cx| sidebar.reload_connections(cx));
        }
    });
}

/// Recreate the tabs of the temporary connection `from`, in every window, on
/// the connection it was saved as.
pub(crate) fn move_tabs_to_connection(from: i64, to: blanco_core::ConnectionContext, cx: &mut App) {
    cx.defer(move |cx| {
        update_each(cx, |app, window, cx| {
            app.editor_panel().clone().update(cx, |panel, cx| {
                panel.move_tabs_to_connection(from, &to, window, cx)
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::test_harness::FullAppHarness;

    /// The titles of the tabs stored for `window_id`. The harness's own tab
    /// is left out: whether it is stored depends on whether its connection
    /// id happens to match a stored connection.
    fn stored_tab_titles(window_id: i64, cx: &mut TestAppContext) -> Vec<String> {
        cx.update(|cx| {
            let db = AppDatabase::global(cx).clone();
            gpui_tokio::Tokio::handle(cx)
                .block_on(async move { db.load_query_tabs(window_id).await })
                .expect("load tabs")
                .into_iter()
                .map(|tab| tab.title)
                .filter(|title| title != "Test Query")
                .collect()
        })
    }

    fn stored_window_ids(cx: &mut TestAppContext) -> Vec<i64> {
        cx.update(|cx| {
            let db = AppDatabase::global(cx).clone();
            gpui_tokio::Tokio::handle(cx)
                .block_on(async move { db.load_windows().await })
                .expect("load windows")
                .into_iter()
                .map(|window| window.id)
                .collect()
        })
    }

    fn tab_titles(app: &Entity<BlancoApp>, cx: &VisualTestContext) -> Vec<String> {
        app.read_with(cx, |app, cx| app.editor_panel().read(cx).tab_titles(cx))
    }

    /// Opens two stored tabs, "First" and "Second", after the harness's
    /// own, which is on a connection the app database does not know and so
    /// is never stored.
    async fn two_tabs(harness: &FullAppHarness, cx: &mut VisualTestContext) {
        let connection_id = cx.update(|_, cx| {
            let db = AppDatabase::global(cx).clone();
            gpui_tokio::Tokio::handle(cx)
                .block_on(async move {
                    db.save_connection(&app_database::ConnectionData::new_sqlite(
                        "saved".to_string(),
                        "/tmp/saved.db".to_string(),
                    ))
                    .await
                })
                .expect("save connection")
        });
        harness.editor_panel(cx).update_in(cx, |panel, window, cx| {
            for title in ["First", "Second"] {
                panel.create_and_add_tab_with_connection(
                    window,
                    crate::editor::TabCreationParams {
                        title: title.into(),
                        content: Some(format!("SELECT '{title}'")),
                        db_id: None,
                        last_run_at: None,
                        context: blanco_core::ConnectionContext {
                            connection_id,
                            connection_name: "saved".to_string(),
                            db_type: database::DatabaseType::SQLite,
                            database_name: "main".to_string(),
                            schema_name: None,
                            environment_type: None,
                        },
                    },
                    cx,
                );
            }
        });
        settle(cx).await;
    }

    async fn settle(cx: &mut VisualTestContext) {
        for _ in 0..10 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(800));
            cx.run_until_parked();
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
    }

    #[gpui::test]
    async fn moving_a_tab_gives_it_a_window_and_keeps_its_row(cx: &mut TestAppContext) {
        let harness = FullAppHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        two_tabs(&harness, &mut cx).await;
        let first_window = harness.app.read_with(&cx, |app, _| app.window_id());
        let moved_row = harness
            .editor_panel(&cx)
            .read_with(&cx, |panel, _| panel.tab_db_id(2))
            .expect("the tab is stored");

        harness.editor_panel(&cx).update(&mut cx, |panel, cx| {
            panel.move_tab_to_new_window(2, cx);
        });
        settle(&mut cx).await;

        let apps = cx.update(|_, cx| apps(cx));
        assert_eq!(apps.len(), 2);
        let new_app = apps
            .into_iter()
            .find(|app| app.entity_id() != harness.app.entity_id())
            .expect("a new window");
        assert_eq!(tab_titles(&harness.app, &cx), ["Test Query", "First"]);
        assert_eq!(tab_titles(&new_app, &cx), ["Second"]);
        let (new_window, text, row) = new_app.read_with(&cx, |app, cx| {
            let panel = app.editor_panel().read(cx);
            let tab = panel.active_query_tab().expect("moved query tab");
            (
                app.window_id(),
                tab.editor.read(cx).text().to_string(),
                tab.db_id,
            )
        });
        assert_eq!(text, "SELECT 'Second'");
        assert_eq!(row, Some(moved_row), "the tab keeps its row");
        assert_eq!(stored_tab_titles(first_window, &mut cx), ["First"]);
        assert_eq!(stored_tab_titles(new_window, &mut cx), ["Second"]);

        // The only tab of a window has nowhere to move from.
        new_app.read_with(&cx, |app, cx| {
            assert!(!app.editor_panel().read(cx).can_move_tab_to_new_window(0));
        });
    }

    #[gpui::test]
    async fn closing_one_of_two_windows_forgets_it_and_the_last_is_kept(cx: &mut TestAppContext) {
        let harness = FullAppHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        two_tabs(&harness, &mut cx).await;
        let first_window = harness.app.read_with(&cx, |app, _| app.window_id());
        harness.editor_panel(&cx).update(&mut cx, |panel, cx| {
            panel.move_tab_to_new_window(2, cx);
        });
        settle(&mut cx).await;
        let second_window = cx
            .update(|_, cx| window_list(cx))
            .into_iter()
            .find(|window| window.id != first_window)
            .expect("a second window");
        assert_eq!(stored_window_ids(&mut cx), [first_window, second_window.id]);

        cx.update(|_, cx| {
            cx.update_window(second_window.handle.into(), |_, window, _| {
                window.remove_window()
            })
        })
        .expect("close the second window");
        settle(&mut cx).await;
        assert_eq!(cx.update(|_, cx| count(cx)), 1);
        assert_eq!(stored_window_ids(&mut cx), [first_window]);
        assert!(
            stored_tab_titles(second_window.id, &mut cx).is_empty(),
            "a closed window's tabs go with it"
        );

        cx.update(|window, _| window.remove_window());
        cx.run_until_parked();
        assert_eq!(stored_window_ids(&mut cx), [first_window]);
        assert_eq!(stored_tab_titles(first_window, &mut cx), ["First"]);
    }

    #[gpui::test]
    async fn restore_reopens_every_window_with_its_own_tabs(cx: &mut TestAppContext) {
        let harness = FullAppHarness::new(cx);
        let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
        two_tabs(&harness, &mut cx).await;
        harness.editor_panel(&cx).update(&mut cx, |panel, cx| {
            panel.move_tab_to_new_window(2, cx);
        });
        settle(&mut cx).await;
        let stored = stored_window_ids(&mut cx);
        let open_before = cx.update(|_, cx| apps(cx));

        // What the next start does, without closing these first.
        cx.update(|_, cx| restore(cx));
        settle(&mut cx).await;

        let mut restored: Vec<(i64, Vec<String>)> = cx
            .update(|_, cx| apps(cx))
            .into_iter()
            .filter(|app| !open_before.contains(app))
            .map(|app| {
                app.read_with(&cx, |app, cx| {
                    let mut titles = app.editor_panel().read(cx).tab_titles(cx);
                    titles.retain(|title| title != "Test Query");
                    (app.window_id(), titles)
                })
            })
            .collect();
        restored.sort();
        assert_eq!(
            restored,
            [
                (stored[0], vec!["First".to_string()]),
                (stored[1], vec!["Second".to_string()]),
            ]
        );
    }
}
