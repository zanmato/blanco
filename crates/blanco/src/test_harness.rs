use database::{ConnectionConfig, DatabaseService};
use gpui::{AppContext, TestAppContext, VisualTestContext};
use gpui_component::Root;

use crate::app_database::AppDatabase;
use crate::app_settings::AppSettings;
use crate::editor::{EditorPanel, TabCreationParams};
use crate::settings::Settings;
use crate::status_bar::{ActivityMessage, ActivityReporter, StatusBarState, StatusLine};

use gpui::Task;
use smol::channel;
use std::sync::atomic::{AtomicI64, Ordering};

static NEXT_CONNECTION_ID: AtomicI64 = AtomicI64::new(1);

fn next_connection_id() -> i64 {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct TestHarness {
    pub editor_panel: gpui::Entity<EditorPanel>,
    pub window_handle: gpui::WindowHandle<Root>,
    pub status_bar: gpui::Entity<StatusBarState>,
    _activity_task: Task<()>,
}

impl TestHarness {
    pub fn new(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();

        let mut editor_panel: Option<gpui::Entity<EditorPanel>> = None;
        let mut status_bar: Option<gpui::Entity<StatusBarState>> = None;
        let mut activity_task: Option<Task<()>> = None;
        let window_handle = cx.update(|cx| {
            gpui_component::init(cx);
            gpui_tokio::init(cx);

            // Mirror app.rs: an activity channel feeding the status bar entity.
            let status_bar_entity = cx.new(|_| StatusBarState::default());
            let (activity_sender, activity_receiver) = channel::unbounded::<ActivityMessage>();
            cx.set_global(ActivityReporter::new(activity_sender));
            activity_task = Some(cx.spawn({
                let status_bar = status_bar_entity.downgrade();
                async move |cx| {
                    while let Ok(message) = activity_receiver.recv().await {
                        status_bar
                            .update(cx, |state, cx| state.apply(message, cx))
                            .ok();
                    }
                }
            }));
            status_bar = Some(status_bar_entity);

            let runtime_handle = gpui_tokio::Tokio::handle(cx);

            let app_database = runtime_handle
                .block_on(AppDatabase::new_in_memory(runtime_handle.clone()))
                .expect("failed to create in-memory database");
            cx.set_global(app_database);

            let db_service = DatabaseService::new(runtime_handle.clone());
            cx.set_global(db_service);

            let settings = AppSettings::new(cx, Settings::default());
            cx.set_global(settings);

            let connection_id = next_connection_id();
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            let db_path = temp_dir.path().join("test.db");
            let db_path_str = db_path.to_string_lossy().to_string();
            std::mem::forget(temp_dir);

            let sqlite_config =
                ConnectionConfig::new_sqlite(connection_id, "test".into(), db_path_str);
            let db_service = DatabaseService::global(cx).clone();
            runtime_handle.block_on(async {
                db_service.add_connection_config(sqlite_config).await;
            });

            cx.open_window(Default::default(), |window, cx| {
                let panel =
                    cx.new(|cx| EditorPanel::new_with_saved_tabs(window, cx, false, vec![]));

                let params = TabCreationParams {
                    title: "Test Query".into(),
                    content: None,
                    db_id: None,
                    connection_id,
                    db_type: database::DatabaseType::SQLite,
                    connection_name: Some("test".into()),
                    database_name: "main".into(),
                    schema_name: None,
                    environment_type: None,
                };
                panel.update(cx, |panel: &mut EditorPanel, cx| {
                    panel.create_and_add_tab_with_connection(window, params, cx);
                });

                editor_panel = Some(panel.clone());
                cx.new(|cx| Root::new(panel, window, cx))
            })
            .expect("failed to open window")
        });

        Self {
            editor_panel: editor_panel.expect("editor_panel should be set"),
            window_handle,
            status_bar: status_bar.expect("status_bar should be set"),
            _activity_task: activity_task.expect("activity_task should be set"),
        }
    }
}

/// A harness that opens the *full* `BlancoApp` (sidebar + main resizable
/// layout), not just the `EditorPanel`. Use this to reproduce layout issues
/// that depend on the app-level chrome.
pub struct FullAppHarness {
    pub app: gpui::Entity<crate::app::BlancoApp>,
    pub window_handle: gpui::WindowHandle<Root>,
    _activity_task: Task<()>,
}

impl FullAppHarness {
    pub fn new(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();

        let mut app: Option<gpui::Entity<crate::app::BlancoApp>> = None;
        let mut activity_task: Option<Task<()>> = None;

        let window_handle = cx.update(|cx| {
            gpui_component::init(cx);
            gpui_tokio::init(cx);

            let status_bar_entity = cx.new(|_| StatusBarState::default());
            let (activity_sender, activity_receiver) = channel::unbounded::<ActivityMessage>();
            cx.set_global(ActivityReporter::new(activity_sender));
            activity_task = Some(cx.spawn({
                let status_bar = status_bar_entity.downgrade();
                async move |cx| {
                    while let Ok(message) = activity_receiver.recv().await {
                        status_bar
                            .update(cx, |state, cx| state.apply(message, cx))
                            .ok();
                    }
                }
            }));

            let runtime_handle = gpui_tokio::Tokio::handle(cx);

            let app_database = runtime_handle
                .block_on(AppDatabase::new_in_memory(runtime_handle.clone()))
                .expect("failed to create in-memory database");
            cx.set_global(app_database);

            let db_service = DatabaseService::new(runtime_handle.clone());
            cx.set_global(db_service);

            let settings = AppSettings::new(cx, Settings::default());
            cx.set_global(settings);

            let connection_id = next_connection_id();
            let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
            let db_path = temp_dir.path().join("test.db");
            let db_path_str = db_path.to_string_lossy().to_string();
            std::mem::forget(temp_dir);

            let sqlite_config =
                ConnectionConfig::new_sqlite(connection_id, "test".into(), db_path_str);
            let db_service = DatabaseService::global(cx).clone();
            runtime_handle.block_on(async {
                db_service.add_connection_config(sqlite_config).await;
            });

            cx.open_window(Default::default(), |window, cx| {
                let blanco_app = cx.new(|cx| crate::app::BlancoApp::new(window, cx));

                let params = TabCreationParams {
                    title: "Test Query".into(),
                    content: None,
                    db_id: None,
                    connection_id,
                    db_type: database::DatabaseType::SQLite,
                    connection_name: Some("test".into()),
                    database_name: "main".into(),
                    schema_name: None,
                    environment_type: None,
                };
                blanco_app.update(cx, |app, cx| {
                    app.editor_panel().clone().update(cx, |panel, cx| {
                        panel.create_and_add_tab_with_connection(window, params, cx);
                    });
                });

                app = Some(blanco_app.clone());
                cx.new(|cx| Root::new(blanco_app, window, cx))
            })
            .expect("failed to open window")
        });

        Self {
            app: app.expect("app should be set"),
            window_handle,
            _activity_task: activity_task.expect("activity_task should be set"),
        }
    }

    pub fn editor_panel(&self, cx: &VisualTestContext) -> gpui::Entity<EditorPanel> {
        self.app.read_with(cx, |app, _| app.editor_panel().clone())
    }
}

pub fn status_line(harness: &TestHarness, cx: &VisualTestContext) -> StatusLine {
    harness
        .status_bar
        .read_with(cx, |state, _cx| state.display())
}

pub fn set_editor_text(harness: &TestHarness, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_string();
    harness.editor_panel.update_in(cx, |panel, window, cx| {
        let tab = panel
            .active_query_tab()
            .expect("no query tab at active index");
        tab.editor.update(cx, |state, cx| {
            state.set_value(&text, window, cx);
        });
    });
}

pub fn run_query(harness: &TestHarness, cx: &mut VisualTestContext) {
    harness.editor_panel.update_in(cx, |panel, window, cx| {
        panel.on_run_query(window, cx);
    });
}

pub fn result_row_count(harness: &TestHarness, cx: &VisualTestContext) -> Option<usize> {
    harness.editor_panel.read_with(cx, |panel, cx| {
        let tab = panel.active_query_tab()?;
        Some(tab.results_panel.read_with(cx, |results, cx| {
            results
                .table_state()
                .read_with(cx, |state, _cx| state.delegate().rows.len())
        }))
    })
}

pub fn result_columns(harness: &TestHarness, cx: &VisualTestContext) -> Option<Vec<String>> {
    harness.editor_panel.read_with(cx, |panel, cx| {
        let tab = panel.active_query_tab()?;
        Some(tab.results_panel.read_with(cx, |results, cx| {
            results.table_state().read_with(cx, |state, _cx| {
                state
                    .delegate()
                    .columns
                    .iter()
                    .map(|c| c.name.to_string())
                    .collect()
            })
        }))
    })
}

pub fn result_cell(
    harness: &TestHarness,
    row: usize,
    col: usize,
    cx: &VisualTestContext,
) -> Option<Option<String>> {
    harness.editor_panel.read_with(cx, |panel, cx| {
        let tab = panel.active_query_tab()?;
        Some(tab.results_panel.read_with(cx, |results, cx| {
            results.table_state().read_with(cx, |state, _cx| {
                state
                    .delegate()
                    .rows
                    .get(row)
                    .and_then(|r| r.get(col))
                    .cloned()
                    .unwrap_or(None)
            })
        }))
    })
}

pub fn is_loading(harness: &TestHarness, cx: &VisualTestContext) -> bool {
    harness
        .editor_panel
        .read_with(cx, |panel, _cx| panel.is_loading())
}

pub async fn wait_for_query(harness: &TestHarness, cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        if !is_loading(harness, cx) {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("Query did not complete within timeout");
}
