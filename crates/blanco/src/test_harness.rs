use blanco_core::ConnectionContext;
use database::{ConnectionConfig, DatabaseService};
use gpui::{AppContext, TestAppContext, VisualTestContext};
use gpui_component::Root;

use crate::app_settings::AppSettings;
use crate::editor::{EditorPanel, TabCreationParams};
use crate::settings::Settings;
use crate::status_bar::{StatusBarState, StatusLine};
use app_database::AppDatabase;

use std::sync::atomic::{AtomicI64, Ordering};

static NEXT_CONNECTION_ID: AtomicI64 = AtomicI64::new(1);

fn next_connection_id() -> i64 {
    NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct TestHarness {
    pub editor_panel: gpui::Entity<EditorPanel>,
    pub window_handle: gpui::WindowHandle<Root>,
    pub status_bar: gpui::Entity<StatusBarState>,
    /// The temp-SQLite connection every tab in this harness is bound to.
    pub connection_id: i64,
    /// The `windows` row the panel's tabs are stored under.
    pub window_id: i64,
    pub db_type: database::DatabaseType,
}

impl TestHarness {
    pub fn new(cx: &mut TestAppContext) -> Self {
        Self::new_with_db_type(cx, database::DatabaseType::SQLite)
    }

    /// Open a harness whose single query tab is backed by `db_type`. Registers
    /// the editor highlighting languages first so the tab's `code_editor`
    /// resolves a real grammar (and loads its highlights query, which panics on
    /// drift). The connection itself is always an in-memory SQLite file, so this
    /// is only meaningful for render/highlighting tests, not query execution.
    pub fn new_with_db_type(cx: &mut TestAppContext, db_type: database::DatabaseType) -> Self {
        Self::new_with(cx, db_type, None)
    }

    /// Open a SQLite-backed harness whose query tab carries `environment_type`,
    /// for exercising the PROD write confirmation.
    pub fn new_with_environment(
        cx: &mut TestAppContext,
        environment_type: app_database::EnvironmentType,
    ) -> Self {
        Self::new_with(cx, database::DatabaseType::SQLite, Some(environment_type))
    }

    fn new_with(
        cx: &mut TestAppContext,
        db_type: database::DatabaseType,
        environment_type: Option<app_database::EnvironmentType>,
    ) -> Self {
        cx.executor().allow_parking();

        crate::sql::register_languages();
        crate::redis_syntax::register_language();

        let mut editor_panel: Option<gpui::Entity<EditorPanel>> = None;
        let mut status_bar: Option<gpui::Entity<StatusBarState>> = None;
        let mut harness_connection_id: Option<i64> = None;
        let mut harness_window_id: Option<i64> = None;
        let window_handle = cx.update(|cx| {
            gpui_component::init(cx);
            gpui_tokio::init(cx);

            status_bar = Some(StatusBarState::shared(cx));

            let runtime_handle = gpui_tokio::Tokio::handle(cx);

            let app_database = runtime_handle
                .block_on(AppDatabase::new_in_memory(runtime_handle.clone()))
                .expect("failed to create in-memory database");
            cx.set_global(app_database);

            let db_service = DatabaseService::new(runtime_handle.clone());
            cx.set_global(db_service);

            let settings = AppSettings::new(Settings::default());
            cx.set_global(settings);

            let connection_id = next_connection_id();
            harness_connection_id = Some(connection_id);
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

            let app_database = AppDatabase::global(cx).clone();
            let window_id = runtime_handle
                .block_on(async move { app_database.create_window(None, None).await })
                .expect("failed to store the window");
            harness_window_id = Some(window_id);

            cx.open_window(Default::default(), |window, cx| {
                let panel = cx.new(|cx| {
                    EditorPanel::new_with_saved_tabs(window, cx, window_id, None, vec![])
                });

                let params = TabCreationParams {
                    title: "Test Query".into(),

                    content: None,

                    db_id: None,

                    last_run_at: None,

                    context: ConnectionContext {
                        connection_id,
                        db_type,
                        connection_name: "test".into(),
                        database_name: "main".into(),
                        schema_name: None,
                        environment_type,
                    },
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
            connection_id: harness_connection_id.expect("connection_id should be set"),
            window_id: harness_window_id.expect("window_id should be set"),
            db_type,
        }
    }

    /// Add a JavaScript script tab on the harness's connection and make it
    /// active. `content` of `None` uses the default scaffold.
    pub fn add_script_tab(&self, content: Option<&str>, cx: &mut VisualTestContext) {
        let params = TabCreationParams {
            title: "Test Script".into(),

            content: content.map(str::to_string),

            db_id: None,

            last_run_at: None,

            context: ConnectionContext {
                connection_id: self.connection_id,
                db_type: self.db_type,
                connection_name: "test".into(),
                database_name: "main".into(),
                schema_name: None,
                environment_type: None,
            },
        };
        self.editor_panel.update_in(cx, |panel, window, cx| {
            panel.create_and_add_script_tab(window, params, cx);
        });
    }
}

/// A harness that opens the *full* `BlancoApp` (sidebar + main resizable
/// layout), not just the `EditorPanel`. Use this to reproduce layout issues
/// that depend on the app-level chrome.
pub struct FullAppHarness {
    pub app: gpui::Entity<crate::app::BlancoApp>,
    pub window_handle: gpui::WindowHandle<Root>,
}

impl FullAppHarness {
    pub fn new(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();

        let (window_handle, app) = cx.update(|cx| {
            gpui_component::init(cx);
            gpui_tokio::init(cx);

            let runtime_handle = gpui_tokio::Tokio::handle(cx);

            let app_database = runtime_handle
                .block_on(AppDatabase::new_in_memory(runtime_handle.clone()))
                .expect("failed to create in-memory database");
            cx.set_global(app_database);

            let db_service = DatabaseService::new(runtime_handle.clone());
            cx.set_global(db_service);
            crate::mcp::McpService::init(cx);

            let settings = AppSettings::new(Settings::default());
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

            crate::app::init_menus(cx);
            crate::windows::init(cx);
            let (window_handle, app) =
                crate::windows::open_new(None, cx).expect("failed to open window");

            let params = TabCreationParams {
                title: "Test Query".into(),
                content: None,
                db_id: None,
                last_run_at: None,
                context: ConnectionContext {
                    connection_id,
                    db_type: database::DatabaseType::SQLite,
                    connection_name: "test".into(),
                    database_name: "main".into(),
                    schema_name: None,
                    environment_type: None,
                },
            };
            window_handle
                .update(cx, |_, window, cx| {
                    app.read(cx).editor_panel().clone().update(cx, |panel, cx| {
                        panel.create_and_add_tab_with_connection(window, params, cx);
                    });
                })
                .expect("failed to add a tab");
            (window_handle, app)
        });

        Self { app, window_handle }
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

pub fn set_script_text(harness: &TestHarness, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_string();
    harness.editor_panel.update_in(cx, |panel, window, cx| {
        let tab = panel
            .active_script_tab()
            .expect("no script tab at active index");
        tab.editor.update(cx, |state, cx| {
            state.set_value(&text, window, cx);
        });
    });
}

/// Everything written to the active script tab's console log so far.
pub fn script_log_text(harness: &TestHarness, cx: &VisualTestContext) -> String {
    harness.editor_panel.read_with(cx, |panel, cx| {
        panel
            .active_script_tab()
            .expect("no script tab at active index")
            .log_view
            .read_with(cx, |log, _cx| log.all_text())
    })
}

pub fn script_result_row_count(harness: &TestHarness, cx: &VisualTestContext) -> Option<usize> {
    harness.editor_panel.read_with(cx, |panel, cx| {
        let tab = panel.active_script_tab()?;
        Some(tab.results_panel.read_with(cx, |results, cx| {
            results
                .table_state()
                .read_with(cx, |state, _cx| state.delegate().rows.len())
        }))
    })
}

pub fn script_result_cell(
    harness: &TestHarness,
    row: usize,
    col: usize,
    cx: &VisualTestContext,
) -> Option<Option<String>> {
    harness.editor_panel.read_with(cx, |panel, cx| {
        let tab = panel.active_script_tab()?;
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

/// Scripts run on their own OS thread and can do many round trips, so they get
/// a much longer budget than a single query.
pub async fn wait_for_script(harness: &TestHarness, cx: &mut VisualTestContext) {
    for _ in 0..500 {
        cx.run_until_parked();
        if !is_loading(harness, cx) {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("Script did not complete within timeout");
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
