//! End-to-end integration tests that walk the real connection-creation flow
//! for every database driver.
//!
//! For each driver the test:
//!   1. builds a `NewConnectionModal` and selects the driver,
//!   2. (non-sqlite) asserts "Test Connection" is *rejected on auth* when no
//!      password is supplied,
//!   3. supplies the password and asserts "Test Connection" *succeeds*,
//!   4. saves the connection (persist + register with `DatabaseService`) and
//!      asserts it round-trips back out of `AppDatabase`,
//!   5. opens a tab bound to it and asserts `SELECT 1` returns the value `1`.
#![allow(clippy::print_stderr)]

use blanco_core::ConnectionContext;
use std::env;

use database::{DatabaseService, DatabaseType};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

use crate::connection_modal::NewConnectionModal;
use crate::editor::{EditorPanel, TabCreationParams};
use crate::test_harness::FullAppHarness;
use app_database::AppDatabase;

fn strict() -> bool {
    env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1")
}

/// A single driver scenario. `password` is the *correct* password; the
/// no-password attempt always uses an empty string.
struct DriverCase {
    label: &'static str,
    db_type: DatabaseType,
    name: &'static str,
    host: String,
    port: String,
    database: String,
    username: String,
    password: String,
    schema_name: Option<&'static str>,
    /// Substrings that identify an authentication/permission rejection (as
    /// opposed to the server being unreachable).
    auth_markers: &'static [&'static str],
    /// Query/command to run once connected, with the single-cell value it is
    /// expected to return. SQL backends run `SELECT 1`; Redis runs `PING`.
    query: &'static str,
    expected_cell: &'static str,
    /// When set, the connection is made through an SSH tunnel. The `host`/`port`
    /// above must then be the database's address *as seen from the bastion*
    /// (the docker-internal service name + port).
    ssh: Option<SshCase>,
}

/// SSH bastion coordinates for a tunneled `DriverCase`.
struct SshCase {
    host: String,
    port: String,
    user: String,
    password: String,
}

/// Reads `BLANCO_<PREFIX>_<FIELD>` if set, otherwise the docker-compose default.
fn env_or(prefix: &str, field: &str, default: &str) -> String {
    env::var(format!("BLANCO_{prefix}_{field}")).unwrap_or_else(|_| default.to_string())
}

fn postgres_case() -> DriverCase {
    DriverCase {
        label: "PostgreSQL",
        db_type: DatabaseType::PostgreSQL,
        name: "it-postgres",
        host: env_or("POSTGRES", "HOST", "localhost"),
        port: env_or("POSTGRES", "PORT", "5488"),
        database: env_or("POSTGRES", "DB", "blanco"),
        username: env_or("POSTGRES", "USER", "blanco"),
        password: env_or("POSTGRES", "PASSWORD", "blanco"),
        schema_name: Some("public"),
        auth_markers: &["password authentication failed", "authentication"],
        query: "SELECT 1",
        expected_cell: "1",
        ssh: None,
    }
}

fn cockroach_case() -> DriverCase {
    DriverCase {
        label: "CockroachDB",
        db_type: DatabaseType::CockroachDb,
        name: "it-cockroachdb",
        host: env_or("COCKROACH", "HOST", "localhost"),
        port: env_or("COCKROACH", "PORT", "26258"),
        database: env_or("COCKROACH", "DB", "blanco"),
        username: env_or("COCKROACH", "USER", "blanco"),
        password: env_or("COCKROACH", "PASSWORD", "blanco"),
        schema_name: Some("public"),
        auth_markers: &["password authentication failed", "no password supplied"],
        query: "SELECT 1",
        expected_cell: "1",
        ssh: None,
    }
}

fn mysql_case() -> DriverCase {
    DriverCase {
        label: "MySQL",
        db_type: DatabaseType::MySQL,
        name: "it-mysql",
        host: env_or("MYSQL", "HOST", "localhost"),
        port: env_or("MYSQL", "PORT", "3306"),
        database: env_or("MYSQL", "DB", "blanco"),
        username: env_or("MYSQL", "USER", "blanco"),
        password: env_or("MYSQL", "PASSWORD", "blanco"),
        schema_name: None,
        auth_markers: &["Access denied", "access denied"],
        query: "SELECT 1",
        expected_cell: "1",
        ssh: None,
    }
}

fn clickhouse_case() -> DriverCase {
    DriverCase {
        label: "ClickHouse",
        db_type: DatabaseType::ClickHouse,
        name: "it-clickhouse",
        host: env_or("CLICKHOUSE", "HOST", "localhost"),
        port: env_or("CLICKHOUSE", "PORT", "8124"),
        database: env_or("CLICKHOUSE", "DB", "blanco"),
        username: env_or("CLICKHOUSE", "USER", "blanco"),
        password: env_or("CLICKHOUSE", "PASSWORD", "blanco"),
        schema_name: None,
        auth_markers: &[
            "Authentication failed",
            "AUTHENTICATION_FAILED",
            "authentication",
            "password is incorrect",
        ],
        query: "SELECT 1",
        expected_cell: "1",
        ssh: None,
    }
}

fn mssql_case() -> DriverCase {
    DriverCase {
        label: "SQL Server",
        db_type: DatabaseType::MsSql,
        name: "it-mssql",
        host: env_or("MSSQL", "HOST", "localhost"),
        port: env_or("MSSQL", "PORT", "1433"),
        database: env_or("MSSQL", "DB", "master"),
        username: env_or("MSSQL", "USER", "sa"),
        password: env_or("MSSQL", "PASSWORD", "Blanco_Passw0rd!"),
        schema_name: None,
        auth_markers: &["Login failed", "login failed"],
        query: "SELECT 1",
        expected_cell: "1",
        ssh: None,
    }
}

fn redis_case() -> DriverCase {
    DriverCase {
        label: "Redis",
        db_type: DatabaseType::Redis,
        name: "it-redis",
        host: env_or("REDIS", "HOST", "localhost"),
        port: env_or("REDIS", "PORT", "6400"),
        // Redis "database" is a numeric index; db 0 is always present.
        database: env_or("REDIS", "DB", "0"),
        // `requirepass` authenticates the default user, so no username is sent;
        // a connection string of `redis://:<password>@host/db` is generated.
        username: env_or("REDIS", "USER", ""),
        password: env_or("REDIS", "PASSWORD", "blanco"),
        schema_name: None,
        auth_markers: &[
            "NOAUTH",
            "Authentication required",
            "WRONGPASS",
            "invalid password",
        ],
        // Redis is not SQL: run a plain command. PING replies with PONG, which
        // the console shapes into a single `value` cell.
        query: "PING",
        expected_cell: "PONG",
        ssh: None,
    }
}

/// Bastion coordinates shared by every SSH case (see the `sshbastion` service).
fn ssh_case() -> SshCase {
    SshCase {
        host: env_or("SSH", "HOST", "localhost"),
        port: env_or("SSH", "PORT", "2222"),
        user: env_or("SSH", "USER", "blanco"),
        password: env_or("SSH", "PASSWORD", "blanco"),
    }
}

/// Turn a direct `DriverCase` into one tunneled through the bastion: the
/// database is now addressed by its docker-internal `service:port` (only
/// reachable from the bastion), and SSH coordinates are attached.
fn over_ssh(
    mut case: DriverCase,
    name: &'static str,
    internal_host: &str,
    internal_port: &str,
) -> DriverCase {
    case.name = name;
    case.host = internal_host.to_string();
    case.port = internal_port.to_string();
    case.ssh = Some(ssh_case());
    case
}

fn is_auth_error(case: &DriverCase, message: &str) -> bool {
    case.auth_markers
        .iter()
        .any(|marker| message.contains(marker))
}

/// Build the modal, select the driver, and populate the network fields with the
/// given password (empty string for the no-password probe).
fn build_modal(
    harness: &FullAppHarness,
    case: &DriverCase,
    password: &str,
    cx: &mut VisualTestContext,
) -> Entity<NewConnectionModal> {
    let modal = harness.app.update_in(cx, |_app, window, cx| {
        cx.new(|cx| NewConnectionModal::with_connection_data(window, cx, None))
    });
    modal.update_in(cx, |modal, window, cx| {
        modal.set_name(case.name, window, cx);
        modal.select_db_type(case.label, window, cx);
        modal.set_network_credentials(
            &case.host,
            &case.port,
            &case.database,
            &case.username,
            password,
            window,
            cx,
        );
        if let Some(ssh) = &case.ssh {
            modal.set_ssh_credentials(&ssh.host, &ssh.port, &ssh.user, &ssh.password, window, cx);
        }
    });
    modal
}

/// Drive `test_connection` to completion and return its recorded outcome.
async fn run_test_connection(
    modal: &Entity<NewConnectionModal>,
    cx: &mut VisualTestContext,
) -> Result<(), String> {
    modal.update_in(cx, |modal, window, cx| {
        modal.test_connection(window, cx);
    });
    // mssql login (TLS handshake) can take several seconds; allow up to ~30s.
    for _ in 0..1200 {
        cx.run_until_parked();
        let done = modal.read_with(cx, |modal, _| !modal.is_testing());
        if done {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(25))
            .await;
    }
    modal
        .update(cx, |modal, _| modal.take_test_result())
        .expect("test_connection should have recorded a result")
}

fn editor_panel(harness: &FullAppHarness, cx: &VisualTestContext) -> Entity<EditorPanel> {
    harness.editor_panel(cx)
}

async fn settle(panel: &Entity<EditorPanel>, cx: &mut VisualTestContext) {
    for _ in 0..200 {
        cx.run_until_parked();
        let loading = panel.read_with(cx, |panel, _| panel.is_loading());
        if !loading {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(25))
            .await;
    }
    for _ in 0..5 {
        cx.run_until_parked();
    }
}

fn first_row_count(panel: &Entity<EditorPanel>, cx: &VisualTestContext) -> Option<usize> {
    panel.read_with(cx, |panel, cx| {
        let tab = panel.active_query_tab()?;
        Some(tab.results_panel.read_with(cx, |results, cx| {
            results
                .table_state()
                .read_with(cx, |state, _| state.delegate().rows.len())
        }))
    })
}

fn first_cell(panel: &Entity<EditorPanel>, cx: &VisualTestContext) -> Option<Option<String>> {
    panel.read_with(cx, |panel, cx| {
        let tab = panel.active_query_tab()?;
        Some(tab.results_panel.read_with(cx, |results, cx| {
            results.table_state().read_with(cx, |state, _| {
                state
                    .delegate()
                    .rows
                    .first()
                    .and_then(|row| row.first())
                    .cloned()
                    .unwrap_or(None)
            })
        }))
    })
}

/// Runs the full flow for one driver. Returns `false` if the test was skipped
/// because the server was unreachable (and `BLANCO_RUN_DB_TESTS` is not set).
async fn run_driver_case(
    harness: &FullAppHarness,
    case: DriverCase,
    cx: &mut VisualTestContext,
) -> bool {
    // 1. No password: must be rejected on auth, not silently accepted.
    let modal = build_modal(harness, &case, "", cx);
    let no_password = run_test_connection(&modal, cx).await;
    match no_password {
        Err(message) if is_auth_error(&case, &message) => {
            // expected negative outcome, continue
        }
        Err(message) => {
            // Not an auth error -> almost certainly unreachable / server down.
            if strict() {
                panic!("{}: server unreachable: {message}", case.label);
            }
            eprintln!(
                "skip {}: server unreachable ({message}). Set BLANCO_RUN_DB_TESTS=1 to require.",
                case.label
            );
            return false;
        }
        Ok(()) => {
            // Server accepted an empty password; we cannot validate the
            // negative case against such a server.
            if strict() {
                panic!(
                    "{}: expected an auth failure with no password, but the connection succeeded",
                    case.label
                );
            }
            eprintln!(
                "skip {}: server did not require a password; cannot validate negative case.",
                case.label
            );
            return false;
        }
    }

    // 2. Correct password: must succeed.
    let modal = build_modal(harness, &case, &case.password, cx);
    match run_test_connection(&modal, cx).await {
        Ok(()) => {}
        Err(message) => {
            panic!(
                "{}: test connection with correct password failed: {message}",
                case.label
            );
        }
    }

    // 3. Save: persist + register with the DatabaseService, then read back.
    let mut data = modal
        .read_with(cx, |modal, cx| modal.get_connection_data(cx))
        .expect("get_connection_data should return data");
    data.name = case.name.to_string();

    let connection_id = cx.update(|_window, cx| {
        let app_database = AppDatabase::global(cx).clone();
        let db_service = DatabaseService::global(cx).clone();
        let handle = gpui_tokio::Tokio::handle(cx);
        handle.block_on(async move {
            let id = app_database
                .save_connection(&data)
                .await
                .expect("save_connection failed");
            let mut saved = data;
            saved.id = Some(id);
            let config = saved
                .to_connection_config()
                .expect("to_connection_config returned None");
            db_service.add_connection_config(config).await;
            id
        })
    });

    let saved = cx.update(|_window, cx| {
        let app_database = AppDatabase::global(cx).clone();
        let handle = gpui_tokio::Tokio::handle(cx);
        handle.block_on(async move {
            app_database
                .load_connections()
                .await
                .expect("load_connections failed")
        })
    });
    let saved = saved
        .into_iter()
        .find(|c| c.id == Some(connection_id))
        .expect("saved connection not found after reload");
    assert_eq!(saved.db_type, case.db_type, "{}: db_type", case.label);
    assert_eq!(
        saved.host.as_deref(),
        Some(case.host.as_str()),
        "{}: host did not round-trip",
        case.label
    );
    assert_eq!(
        saved.database_name.as_deref(),
        Some(case.database.as_str()),
        "{}: database did not round-trip",
        case.label
    );

    // 4. Open a tab bound to the saved connection and run SELECT 1.
    let panel = editor_panel(harness, cx);
    panel.update_in(cx, |panel, window, cx| {
        let params = TabCreationParams {
            title: case.name.into(),

            content: None,

            db_id: Some(connection_id),

            last_run_at: None,

            context: ConnectionContext {
                connection_id,
                db_type: case.db_type,
                connection_name: case.name.into(),
                database_name: case.database.clone(),
                schema_name: case.schema_name.map(|s| s.to_string()),
                environment_type: None,
            },
        };
        panel.create_and_add_tab_with_connection(window, params, cx);
    });

    panel.update_in(cx, |panel, window, cx| {
        let tab = panel.active_query_tab().expect("active query tab");
        tab.editor.update(cx, |state, cx| {
            state.set_value(case.query, window, cx);
        });
        panel.on_run_query(window, cx);
    });
    settle(&panel, cx).await;

    assert_eq!(
        first_row_count(&panel, cx),
        Some(1),
        "{}: `{}` should return exactly one row",
        case.label,
        case.query
    );
    assert_eq!(
        first_cell(&panel, cx),
        Some(Some(case.expected_cell.to_string())),
        "{}: `{}` should return the value {}",
        case.label,
        case.query,
        case.expected_cell
    );

    true
}

/// Duplicating fills the new connection dialog from an existing connection.
/// What it would save is a new connection (no id) with the same settings and
/// none of the original's secrets.
#[gpui::test]
async fn test_duplicate_prefills_everything_but_secrets(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();

    let mut original = app_database::ConnectionData::new_server(
        database::DatabaseType::PostgreSQL,
        "staging".to_string(),
        "db.internal".to_string(),
        6543,
        "orders".to_string(),
        "reporting".to_string(),
        "hunter2".to_string(),
    );
    original.id = Some(7);
    original.read_only = true;
    original.ssh_host = Some("bastion.internal".to_string());
    original.ssh_user = Some("deploy".to_string());
    original.ssh_password = Some("ssh-secret".to_string());
    original.ssh_private_key_password = Some("key-secret".to_string());

    let duplicate = original.duplicate("staging copy".to_string());
    let modal = harness.app.update_in(&mut cx, |_app, window, cx| {
        cx.new(|cx| NewConnectionModal::with_connection_data(window, cx, Some(duplicate)))
    });
    let saved = modal
        .read_with(&cx, |modal, cx| modal.get_connection_data(cx))
        .expect("the prefilled dialog is complete enough to save");

    assert_eq!(saved.id, None);
    assert_eq!(saved.name, "staging copy");
    assert_eq!(saved.db_type, database::DatabaseType::PostgreSQL);
    assert_eq!(saved.host.as_deref(), Some("db.internal"));
    assert_eq!(saved.port, Some(6543));
    assert_eq!(saved.database_name.as_deref(), Some("orders"));
    assert_eq!(saved.username.as_deref(), Some("reporting"));
    assert_eq!(saved.ssh_host.as_deref(), Some("bastion.internal"));
    assert_eq!(saved.ssh_user.as_deref(), Some("deploy"));
    assert!(saved.read_only);
    for secret in [
        &saved.password,
        &saved.ssh_password,
        &saved.ssh_private_key_password,
    ] {
        assert!(secret.as_deref().is_none_or(str::is_empty));
    }
}

/// The dialog's close button dispatches `Cancel` along the focused element's
/// path. The modal takes focus when the dialog opens, so its focus handle has
/// to be attached to a rendered element or the dialog host never sees the
/// action and the dialog cannot be closed that way.
#[gpui::test]
async fn test_new_connection_dialog_closes_on_cancel(cx: &mut TestAppContext) {
    use gpui_component::WindowExt as _;

    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();

    // A fresh app database has no connections, so the first-run flow has
    // already opened the new connection dialog.
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));

    // Same dispatch as the dialog's close button.
    cx.update(|window, cx| {
        let cancel = cx
            .build_action("ui::Cancel", None)
            .expect("the dialog cancel action is registered");
        window.dispatch_action(cancel, cx);
    });
    cx.run_until_parked();
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "cancel should close the new connection dialog"
    );
}

/// File, New creates a SQLite or DuckDB file that is recognised as a
/// database when opened again, adds it to the sidebar and opens a query tab
/// on it. Opening the file again reuses that connection.
#[gpui::test]
async fn test_new_database_files_open_as_connections(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    let temp_dir = tempfile::tempdir().expect("temp dir");

    for (db_type, file_name) in [
        (DatabaseType::SQLite, "created.db"),
        (DatabaseType::DuckDb, "created.duckdb"),
    ] {
        let path = temp_dir.path().join(file_name);
        harness.app.update_in(&mut cx, |app, window, cx| {
            app.open_file_connection(db_type, &path, true, window, cx);
        });
        let panel = harness.editor_panel(&cx);
        let mut opened = false;
        for _ in 0..200 {
            cx.run_until_parked();
            opened = panel.read_with(&cx, |panel, _| {
                panel.active_query_tab().is_some_and(|tab| {
                    tab.context.db_type == db_type && tab.context.connection_name == file_name
                })
            });
            if opened {
                break;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
        assert!(
            opened,
            "{db_type:?}: a query tab should open on the new file"
        );
        assert_eq!(
            crate::database_file::database_file_type(&path),
            Some(db_type),
            "{db_type:?}: the new file should be recognised as a database"
        );

        panel.update_in(&mut cx, |panel, window, cx| {
            let tab = panel.active_query_tab().expect("query tab");
            tab.editor.update(cx, |state, cx| {
                state.set_value("SELECT 40 + 2 AS answer", window, cx);
            });
            panel.on_run_query(window, cx);
        });
        for _ in 0..200 {
            cx.run_until_parked();
            if !panel.read_with(&cx, |panel, _| panel.is_loading()) {
                break;
            }
            cx.executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
        }
        let answer = panel.read_with(&cx, |panel, cx| {
            let tab = panel.active_query_tab().expect("query tab");
            tab.results_panel.read_with(cx, |results, cx| {
                results.table_state().read_with(cx, |state, _| {
                    state
                        .delegate()
                        .rows
                        .first()
                        .and_then(|row| row.last().cloned().flatten())
                })
            })
        });
        assert_eq!(answer.as_deref(), Some("42"), "{db_type:?}");

        let temporary = |cx: &mut VisualTestContext| {
            harness.app.read_with(cx, |app, cx| {
                app.sidebar()
                    .read(cx)
                    .temporary_connections()
                    .iter()
                    .filter(|connection| connection.db_type == db_type)
                    .count()
            })
        };
        assert_eq!(temporary(&mut cx), 1, "{db_type:?}");
        harness.app.update_in(&mut cx, |app, window, cx| {
            app.open_database_file(&path, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            temporary(&mut cx),
            1,
            "{db_type:?}: reopening reuses the connection"
        );
    }
}

#[gpui::test]
async fn test_sqlite_connect_and_query(cx: &mut TestAppContext) {
    run_file_case(DatabaseType::SQLite, "integration.db", cx).await;
}

#[gpui::test]
async fn test_duckdb_connect_and_query(cx: &mut TestAppContext) {
    run_file_case(DatabaseType::DuckDb, "integration.duckdb", cx).await;
}

/// A database file has no password, so this exercises create -> test ->
/// save -> query against a fresh temp file.
async fn run_file_case(db_type: DatabaseType, file_name: &str, cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();

    let label = db_type.as_str();
    let name = format!("it-{}", label.to_lowercase());
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let db_path = temp_dir.path().join(file_name);
    let db_path = db_path.to_string_lossy().to_string();

    let modal = harness.app.update_in(&mut cx, |_app, window, cx| {
        cx.new(|cx| NewConnectionModal::with_connection_data(window, cx, None))
    });
    modal.update_in(&mut cx, |modal, window, cx| {
        modal.set_name(&name, window, cx);
        modal.select_db_type(label, window, cx);
        modal.set_file_path(&db_path, window, cx);
    });

    let result = run_test_connection(&modal, &mut cx).await;
    assert!(
        result.is_ok(),
        "{label} test connection failed: {:?}",
        result
    );

    let mut data = modal
        .read_with(&cx, |modal, cx| modal.get_connection_data(cx))
        .expect("connection data");
    assert_eq!(data.db_type, db_type);
    data.name = name.clone();

    let connection_id = cx.update(|_window, cx| {
        let app_database = AppDatabase::global(cx).clone();
        let db_service = DatabaseService::global(cx).clone();
        let handle = gpui_tokio::Tokio::handle(cx);
        handle.block_on(async move {
            let id = app_database
                .save_connection(&data)
                .await
                .expect("save_connection failed");
            let mut saved = data;
            saved.id = Some(id);
            let config = saved
                .to_connection_config()
                .expect("to_connection_config returned None");
            db_service.add_connection_config(config).await;
            id
        })
    });

    let panel = editor_panel(&harness, &cx);
    panel.update_in(&mut cx, |panel, window, cx| {
        let params = TabCreationParams {
            title: name.clone(),

            content: None,

            db_id: Some(connection_id),

            last_run_at: None,

            context: ConnectionContext {
                connection_id,
                db_type,
                connection_name: name.clone(),
                database_name: db_type.dialect().file_database_name().to_string(),
                schema_name: None,
                environment_type: None,
            },
        };
        panel.create_and_add_tab_with_connection(window, params, cx);
    });
    panel.update_in(&mut cx, |panel, window, cx| {
        let tab = panel.active_query_tab().expect("active query tab");
        tab.editor.update(cx, |state, cx| {
            state.set_value("SELECT 1", window, cx);
        });
        panel.on_run_query(window, cx);
    });
    settle(&panel, &mut cx).await;

    assert_eq!(first_row_count(&panel, &cx), Some(1));
    assert_eq!(first_cell(&panel, &cx), Some(Some("1".to_string())));
}

#[gpui::test]
async fn test_postgres_connect_and_query(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    run_driver_case(&harness, postgres_case(), &mut cx).await;
}

#[gpui::test]
async fn test_cockroachdb_connect_and_query(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    run_driver_case(&harness, cockroach_case(), &mut cx).await;
}

#[gpui::test]
async fn test_mysql_connect_and_query(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    run_driver_case(&harness, mysql_case(), &mut cx).await;
}

#[gpui::test]
async fn test_clickhouse_connect_and_query(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    run_driver_case(&harness, clickhouse_case(), &mut cx).await;
}

#[gpui::test]
async fn test_mssql_connect_and_query(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    run_driver_case(&harness, mssql_case(), &mut cx).await;
}

#[gpui::test]
async fn test_redis_connect_and_query(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    run_driver_case(&harness, redis_case(), &mut cx).await;
}

// SSH-tunneled variants: identical to the direct cases but routed through the
// `sshbastion` service, which reaches each database over the compose-internal
// network by its service name. These exercise the full SSH path end to end:
// modal SSH fields -> `requires_ssh_tunnel` -> `SshTunnel` -> driver connect.

#[gpui::test]
async fn test_postgres_connect_and_query_over_ssh(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    let case = over_ssh(
        postgres_case(),
        "it-postgres-ssh",
        "postgrestestdb_ssl",
        "5432",
    );
    run_driver_case(&harness, case, &mut cx).await;
}

#[gpui::test]
async fn test_mysql_connect_and_query_over_ssh(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    let case = over_ssh(mysql_case(), "it-mysql-ssh", "mysqltestdb", "3306");
    run_driver_case(&harness, case, &mut cx).await;
}

#[gpui::test]
async fn test_clickhouse_connect_and_query_over_ssh(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    let case = over_ssh(
        clickhouse_case(),
        "it-clickhouse-ssh",
        "clickhousetestdb",
        "8123",
    );
    run_driver_case(&harness, case, &mut cx).await;
}

#[gpui::test]
async fn test_mssql_connect_and_query_over_ssh(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    let case = over_ssh(mssql_case(), "it-mssql-ssh", "mssqltestdb", "1433");
    run_driver_case(&harness, case, &mut cx).await;
}

#[gpui::test]
async fn test_redis_connect_and_query_over_ssh(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();
    let case = over_ssh(redis_case(), "it-redis-ssh", "redis", "6379");
    run_driver_case(&harness, case, &mut cx).await;
}

/// Exercises the key-inspection path the sidebar uses: a connection fetched via
/// `DatabaseService` (so it is wrapped in `TokioConnection`) must forward
/// `inspect_key` to the Redis driver rather than fall back to the trait
/// default. Writes a string key, then inspects it.
#[gpui::test]
async fn test_redis_inspect_key(cx: &mut TestAppContext) {
    use database::DatabaseServiceTrait;

    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();

    let case = redis_case();

    // Register the connection with the service (no modal needed here).
    let modal = build_modal(&harness, &case, &case.password, &mut cx);
    let mut data = modal
        .read_with(&cx, |modal, cx| modal.get_connection_data(cx))
        .expect("get_connection_data should return data");
    data.name = case.name.to_string();

    let connection_id = cx.update(|_window, cx| {
        let app_database = AppDatabase::global(cx).clone();
        let db_service = DatabaseService::global(cx).clone();
        let handle = gpui_tokio::Tokio::handle(cx);
        handle.block_on(async move {
            let id = app_database
                .save_connection(&data)
                .await
                .expect("save_connection failed");
            let mut saved = data;
            saved.id = Some(id);
            let config = saved
                .to_connection_config()
                .expect("to_connection_config returned None");
            db_service.add_connection_config(config).await;
            id
        })
    });

    let database = case.database.clone();
    let inspected = cx.update(|_window, cx| {
        let db_service = DatabaseService::global(cx).clone();
        let handle = gpui_tokio::Tokio::handle(cx);
        handle.block_on(async move {
            let connection = match db_service
                .get_or_create_connection_by_id(connection_id, Some(&database))
                .await
            {
                Ok(connection) => connection,
                Err(e) => return Err(format!("connect failed: {e}")),
            };
            // Write a known key, then inspect it through the wrapper.
            connection
                .execute_write("SET blanco:it:greeting hello", Some(&database), &[])
                .await
                .map_err(|e| format!("SET failed: {e}"))?;
            connection
                .inspect_key(Some(&database), "blanco:it:greeting")
                .await
                .map_err(|e| format!("inspect_key failed: {e}"))
        })
    });

    let inspected = match inspected {
        Ok(result) => result,
        Err(message) => {
            if strict() {
                panic!("Redis inspect_key: {message}");
            }
            eprintln!("skip Redis inspect_key: {message}. Set BLANCO_RUN_DB_TESTS=1 to require.");
            return;
        }
    };

    assert_eq!(inspected.key, "blanco:it:greeting");
    assert_eq!(inspected.key_type, blanco_core::RedisType::String);
    assert_eq!(
        inspected.value,
        blanco_core::RedisValue::Str("hello".to_string())
    );
}

/// Every driver times each statement of a script on its own, and reports a
/// fetch time only for a statement that returned rows.
#[test]
fn test_statement_timing_against_every_backend() {
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir
        .path()
        .join("timing.db")
        .to_string_lossy()
        .to_string();
    let duckdb_path = temp_dir
        .path()
        .join("timing.duckdb")
        .to_string_lossy()
        .to_string();

    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let cockroach = cockroach_case();
    let mysql = mysql_case();
    let clickhouse = clickhouse_case();
    let mssql = mssql_case();
    // Each script and whether each of its results returned rows.
    let cases = vec![
        (
            "SQLite",
            ConnectionConfig::new_sqlite(9101, "timing-sqlite".to_string(), sqlite_path),
            "main".to_string(),
            "CREATE TABLE blanco_timing (id INTEGER); SELECT 1 AS n",
            vec![false, true],
        ),
        (
            "DuckDB",
            ConnectionConfig::new_duckdb(9106, "timing-duckdb".to_string(), duckdb_path),
            String::new(),
            "CREATE TABLE blanco_timing (id INTEGER); SELECT 1 AS n",
            vec![false, true],
        ),
        (
            postgres.label,
            server(&postgres, 9102),
            postgres.database.clone(),
            "CREATE TEMP TABLE blanco_timing (id integer); SELECT 1 AS n",
            vec![false, true],
        ),
        (
            cockroach.label,
            server(&cockroach, 9107),
            cockroach.database.clone(),
            "CREATE TABLE IF NOT EXISTS blanco_timing (id integer); SELECT 1 AS n",
            vec![false, true],
        ),
        (
            mysql.label,
            server(&mysql, 9103),
            mysql.database.clone(),
            "CREATE TEMPORARY TABLE blanco_timing (id integer); SELECT 1 AS n",
            vec![false, true],
        ),
        (
            clickhouse.label,
            server(&clickhouse, 9104),
            clickhouse.database.clone(),
            "SELECT 1 AS n; SELECT 1 AS n WHERE 0",
            vec![true, false],
        ),
        (
            mssql.label,
            server(&mssql, 9105).with_trust_server_certificate(true),
            mssql.database.clone(),
            "SELECT 1 AS n; SELECT 1 AS n WHERE 1 = 0",
            vec![true, false],
        ),
    ];

    let service = DatabaseService::new(runtime.handle().clone());
    for (label, config, database, script, returns_rows) in cases {
        let database = Some(database.as_str());
        let results = runtime.block_on(async {
            service.add_connection_config(config.clone()).await;
            let connection = service
                .get_or_create_connection(config.id, database)
                .await?;
            connection.ping().await?;
            connection.execute_script(script, database).await
        });
        let results = match results {
            Ok(results) => results,
            Err(error) if strict() => panic!("{label}: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };

        let fetched: Vec<bool> = results
            .iter()
            .map(|result| {
                result
                    .timing
                    .unwrap_or_else(|| panic!("{label}: every result should be timed"))
                    .fetch
                    .is_some()
            })
            .collect();
        assert_eq!(fetched, returns_rows, "{label}: fetch time per statement");
    }
}

/// Every driver reports the size of the database the connection is on, keyed
/// by the name the sidebar lists it under.
#[test]
fn test_database_sizes_against_every_backend() {
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir
        .path()
        .join("sizes.db")
        .to_string_lossy()
        .to_string();
    let duckdb_path = temp_dir
        .path()
        .join("sizes.duckdb")
        .to_string_lossy()
        .to_string();

    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let mysql = mysql_case();
    let clickhouse = clickhouse_case();
    let mssql = mssql_case();
    // Each connection, the name its database's size is under, and a table to
    // create first where the database may be empty. ClickHouse's database may
    // hold no parts, so only its presence is checked there.
    let cases = vec![
        (
            "SQLite",
            ConnectionConfig::new_sqlite(9121, "sizes-sqlite".to_string(), sqlite_path),
            "main".to_string(),
            Some(("CREATE TABLE blanco_size (id INTEGER)", None)),
            true,
        ),
        (
            "DuckDB",
            ConnectionConfig::new_duckdb(9126, "sizes-duckdb".to_string(), duckdb_path),
            "sizes".to_string(),
            Some((
                "CREATE TABLE blanco_size AS SELECT range AS id FROM range(100000); CHECKPOINT",
                None,
            )),
            true,
        ),
        (
            postgres.label,
            server(&postgres, 9122),
            postgres.database.clone(),
            None,
            true,
        ),
        (
            mysql.label,
            server(&mysql, 9123),
            mysql.database.clone(),
            Some((
                "CREATE TABLE IF NOT EXISTS blanco_size (id integer)",
                Some("DROP TABLE blanco_size"),
            )),
            true,
        ),
        (
            clickhouse.label,
            server(&clickhouse, 9124),
            clickhouse.database.clone(),
            None,
            false,
        ),
        (
            mssql.label,
            server(&mssql, 9125).with_trust_server_certificate(true),
            mssql.database.clone(),
            None,
            true,
        ),
    ];

    let service = DatabaseService::new(runtime.handle().clone());
    for (label, config, database, scratch_table, has_data) in cases {
        let sizes = runtime.block_on(async {
            service.add_connection_config(config.clone()).await;
            let connection = service.get_or_create_connection(config.id, None).await?;
            connection.ping().await?;
            let Some((create, drop)) = scratch_table else {
                return connection.get_database_sizes().await;
            };
            connection
                .execute_query(create, Some(&database), None)
                .await?;
            let sizes = connection.get_database_sizes().await;
            if let Some(drop) = drop {
                connection
                    .execute_query(drop, Some(&database), None)
                    .await?;
            }
            sizes
        });
        let sizes = match sizes {
            Ok(sizes) => sizes,
            Err(error) if strict() => panic!("{label}: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };
        let size = sizes
            .get(&database)
            .copied()
            .unwrap_or_else(|| panic!("{label}: no size for {database} in {sizes:?}"));
        if has_data {
            assert!(size > 0, "{label}: {database} should take space");
        }
    }
}

/// The sqlx pools hand out a connection without pinging it first, so a
/// connection the server dropped while idle fails one query. That failure must
/// count as a lost connection, so the service evicts it and the next run
/// reconnects.
#[test]
fn test_dropped_pool_connection_reconnects_on_next_run() {
    use blanco_core::DatabaseService as _;
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let mysql = mysql_case();
    let cases = [
        (
            &postgres,
            9111,
            "SELECT pg_backend_pid()",
            "SELECT pg_terminate_backend({id})",
        ),
        (&mysql, 9113, "SELECT CONNECTION_ID()", "KILL {id}"),
    ];

    let service = DatabaseService::new(runtime.handle().clone());
    for (case, config_id, backend_id_sql, kill_sql) in cases {
        let label = case.label;
        let database = Some(case.database.as_str());
        // A second connection does the killing, since a session cannot end
        // itself and still report back.
        let killer_id = config_id + 1;
        let backend_id = runtime.block_on(async {
            service.add_connection_config(server(case, config_id)).await;
            service.add_connection_config(server(case, killer_id)).await;
            let results = service
                .execute_script(config_id, database, backend_id_sql)
                .await?;
            anyhow::Ok(
                results
                    .first()
                    .and_then(|result| result.rows.first())
                    .and_then(|row| row.first().cloned().flatten())
                    .unwrap_or_else(|| panic!("{label}: backend id")),
            )
        });
        let backend_id = match backend_id {
            Ok(backend_id) => backend_id,
            Err(error) if strict() => panic!("{label}: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };

        runtime.block_on(async {
            service
                .execute_script(killer_id, database, &kill_sql.replace("{id}", &backend_id))
                .await
                .unwrap_or_else(|error| panic!("{label}: kill: {error:#}"));
            // The server closes the socket after answering the kill.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            let error = service
                .execute_script(config_id, database, "SELECT 1")
                .await
                .expect_err("the dropped connection should fail its query");
            assert!(
                blanco_core::is_connection_lost(&error),
                "{label}: should be a lost connection: {error:#}"
            );

            let results = service
                .execute_script(config_id, database, "SELECT 1")
                .await
                .unwrap_or_else(|error| panic!("{label}: should reconnect: {error:#}"));
            assert_eq!(
                results
                    .first()
                    .and_then(|result| result.rows.first())
                    .and_then(|row| row.first().cloned().flatten())
                    .as_deref(),
                Some("1"),
                "{label}"
            );
        });
    }
}

/// The process list against every server backend: a marked sleep running
/// on the shared connection shows up in the listing read through a
/// dedicated connection, even where the shared one holds the only pool slot,
/// and the backend's cancel and kill statements stop it.
#[test]
fn test_process_list_finds_cancels_and_kills_sessions() {
    use blanco_core::{DatabaseService as _, ServerSession};
    use database::ConnectionConfig;
    use std::time::{Duration, Instant};

    const MARKER: &str = "blanco_process_list_test";
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
        // The compose SQL Server presents a self-signed certificate.
        .with_trust_server_certificate(case.db_type == DatabaseType::MsSql)
    };
    let cases = [
        (
            postgres_case(),
            9201,
            "SELECT pg_sleep(30) AS {marker}".to_string(),
        ),
        (
            cockroach_case(),
            9205,
            "SELECT pg_sleep(30) AS {marker}".to_string(),
        ),
        (
            mysql_case(),
            9202,
            "SELECT SLEEP(30) AS {marker}".to_string(),
        ),
        (
            mssql_case(),
            9203,
            "WAITFOR DELAY '00:00:30'; SELECT 1 AS {marker}".to_string(),
        ),
        (
            clickhouse_case(),
            9204,
            "SELECT sleepEachRow(1) AS {marker} FROM numbers(30) SETTINGS max_block_size = 1"
                .to_string(),
        ),
    ];

    let service = DatabaseService::new(runtime.handle().clone());
    for (case, config_id, sleep_sql) in cases {
        let label = case.label;
        let dialect = case.db_type.dialect();
        let database = case.database.clone();
        let sleep_sql = sleep_sql.replace("{marker}", MARKER);
        let reachable = runtime.block_on(async {
            service
                .add_connection_config(server(&case, config_id))
                .await;
            service
                .execute_script(config_id, Some(&database), "SELECT 1")
                .await
                .map(|_| ())
        });
        if let Err(error) = reachable {
            if strict() {
                panic!("{label}: {error:#}");
            }
            eprintln!(
                "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            continue;
        }
        let dedicated = runtime
            .block_on(service.open_dedicated_connection(config_id, Some(&database)))
            .unwrap_or_else(|error| panic!("{label}: dedicated connection: {error:#}"));

        // Start the marked sleep on the shared connection, find it, run
        // `statement_for` on its id and check the sleep stops early.
        let stop_with = |statement_for: &dyn Fn(&str) -> Option<String>, what: &str| {
            let sleeper = runtime.spawn({
                let service = service.clone();
                let database = database.clone();
                let sleep_sql = sleep_sql.clone();
                async move {
                    service
                        .execute_script(config_id, Some(&database), &sleep_sql)
                        .await
                }
            });
            let started = Instant::now();
            let session = runtime.block_on(async {
                loop {
                    let result = dedicated
                        .execute_query(
                            dialect.process_list_query().expect("process list"),
                            Some(&database),
                            None,
                        )
                        .await
                        .unwrap_or_else(|error| panic!("{label}: listing: {error:#}"));
                    let found = result
                        .rows
                        .iter()
                        .filter_map(|row| ServerSession::from_row(row))
                        .find(|session| {
                            session
                                .query
                                .as_deref()
                                .is_some_and(|query| query.contains(MARKER))
                        });
                    if let Some(session) = found {
                        return session;
                    }
                    assert!(
                        started.elapsed() < Duration::from_secs(10),
                        "{label}: the sleeping session never showed up"
                    );
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            });
            let statement = statement_for(&session.id)
                .unwrap_or_else(|| panic!("{label}: no {what} statement for {}", session.id));
            runtime
                .block_on(dedicated.execute_query(&statement, Some(&database), None))
                .unwrap_or_else(|error| panic!("{label}: {what}: {error:#}"));
            let outcome = runtime
                .block_on(async { tokio::time::timeout(Duration::from_secs(15), sleeper).await })
                .unwrap_or_else(|_| panic!("{label}: the sleep outlived its {what}"))
                .expect("sleeper task");
            assert!(
                outcome.is_err() || started.elapsed() < Duration::from_secs(25),
                "{label}: the sleep ran to its end despite the {what}"
            );
        };

        if dialect.can_cancel_queries() {
            stop_with(&|id| dialect.cancel_query_statement(id), "cancel");
        }
        if dialect.can_kill_sessions() {
            stop_with(&|id| dialect.kill_session_statement(id), "kill");
        }
    }
}

/// Both explain modes against every backend: an estimate leaves the data
/// alone, Explain Analyze runs the statement, and the plans of the backends
/// with a parser come back as trees.
#[test]
fn test_explain_estimates_and_analyzes_per_backend() {
    use blanco_core::{ExplainMode, ServerVariant};
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let file = |name: &str| temp_dir.path().join(name).to_string_lossy().to_string();
    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
        .with_trust_server_certificate(case.db_type == DatabaseType::MsSql)
    };
    let postgres = postgres_case();
    let cockroach = cockroach_case();
    let mysql = mysql_case();
    let clickhouse = clickhouse_case();
    let mssql = mssql_case();
    // ClickHouse has no UPDATE statement to explain.
    let cases = [
        (
            "SQLite",
            ConnectionConfig::new_sqlite(9301, "explain-sqlite".to_string(), file("explain.db")),
            "main".to_string(),
            true,
        ),
        (
            "DuckDB",
            ConnectionConfig::new_duckdb(
                9302,
                "explain-duckdb".to_string(),
                file("explain.duckdb"),
            ),
            String::new(),
            true,
        ),
        (
            postgres.label,
            server(&postgres, 9303),
            postgres.database.clone(),
            true,
        ),
        (
            cockroach.label,
            server(&cockroach, 9307),
            cockroach.database.clone(),
            true,
        ),
        (
            mysql.label,
            server(&mysql, 9304),
            mysql.database.clone(),
            true,
        ),
        (
            clickhouse.label,
            server(&clickhouse, 9305),
            clickhouse.database.clone(),
            false,
        ),
        (
            mssql.label,
            server(&mssql, 9306),
            mssql.database.clone(),
            true,
        ),
    ];

    let service = DatabaseService::new(runtime.handle().clone());
    for (label, config, database, explains_update) in cases {
        let db_type = config.db_type;
        let dialect = db_type.dialect();
        let database = Some(database.as_str());
        let connection = runtime.block_on(async {
            service.add_connection_config(config.clone()).await;
            let connection = service
                .get_or_create_connection(config.id, database)
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        });
        let connection = match connection {
            Ok(connection) => connection,
            Err(error) if strict() => panic!("{label}: server unreachable: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };
        let run = |sql: &str| {
            runtime
                .block_on(connection.execute_script(sql, database))
                .unwrap_or_else(|error| panic!("{label}: {sql}: {error:#}"))
        };
        let counter = || -> String {
            run("SELECT n FROM blanco_explain_test WHERE id = 1")
                .first()
                .and_then(|result| result.rows.first())
                .and_then(|row| row.first().cloned().flatten())
                .unwrap_or_else(|| panic!("{label}: no counter row"))
        };
        let variant = runtime
            .block_on(connection.server_variant())
            .unwrap_or_else(|error| panic!("{label}: server variant: {error:#}"));
        assert_eq!(
            variant == ServerVariant::MariaDb,
            db_type == DatabaseType::MySQL,
            "{label}: the compose MySQL server is MariaDB"
        );

        runtime
            .block_on(
                connection.execute_script("DROP TABLE IF EXISTS blanco_explain_test", database),
            )
            .ok();
        run(if db_type == DatabaseType::ClickHouse {
            "CREATE TABLE blanco_explain_test (id Int32, n Int32) ENGINE = MergeTree ORDER BY id"
        } else {
            "CREATE TABLE blanco_explain_test (id integer PRIMARY KEY, n integer)"
        });
        run("INSERT INTO blanco_explain_test (id, n) VALUES (1, 0)");

        let select = "SELECT * FROM blanco_explain_test WHERE id = 1";
        for mode in [ExplainMode::Estimated, ExplainMode::Analyze] {
            let Some(explain) = dialect.explain(select, mode, variant) else {
                assert!(
                    mode == ExplainMode::Analyze && !dialect.supports_explain_analyze(),
                    "{label}: no {mode:?} explain"
                );
                continue;
            };
            let results = run(&explain);
            let parses = matches!(
                db_type,
                DatabaseType::PostgreSQL | DatabaseType::CockroachDb | DatabaseType::SQLite
            ) || (db_type == DatabaseType::MySQL
                && variant == ServerVariant::Standard);
            if parses {
                let plan = results
                    .iter()
                    .find_map(|result| dialect.parse_explain_plan(result))
                    .unwrap_or_else(|| panic!("{label}: {mode:?} plan did not parse"));
                assert_eq!(
                    plan.has_timings(),
                    mode == ExplainMode::Analyze,
                    "{label}: only an analyzed plan has timings"
                );
            }
        }

        if explains_update {
            let update = "UPDATE blanco_explain_test SET n = n + 1 WHERE id = 1";
            let estimate = dialect
                .explain(update, ExplainMode::Estimated, variant)
                .expect("estimate");
            run(&estimate);
            assert_eq!(
                counter(),
                "0",
                "{label}: an estimate must not run the update"
            );
            if let Some(analyze) = dialect.explain(update, ExplainMode::Analyze, variant) {
                run(&analyze);
                assert_eq!(counter(), "1", "{label}: Explain Analyze runs the update");
            }
        }
        run("DROP TABLE blanco_explain_test");
    }
}

/// The column/index DDL builders against every live backend: create a
/// scratch table, add a column and an index, alter the column, drop both,
/// and check the driver's metadata between steps.
#[test]
fn test_column_and_index_ddl_round_trip() {
    use blanco_core::ddl::{
        ColumnOperation, ColumnPosition, ColumnSpec, DdlError, ForeignKeyOperation, IndexOperation,
        IndexSpec, NewTable, ReferentialAction, TableChange, column_operation_sql,
        create_table_sql, foreign_key_operation_sql, index_operation_sql, table_change_sql,
    };
    use blanco_core::{ColumnInfo, IndexInfo, WriteOperation};
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir.path().join("ddl.db").to_string_lossy().to_string();
    let duckdb_path = temp_dir
        .path()
        .join("ddl.duckdb")
        .to_string_lossy()
        .to_string();

    struct DdlCase {
        label: &'static str,
        config: ConnectionConfig,
        database: String,
        schema: Option<&'static str>,
        create_table: &'static str,
        alter_supported: bool,
        /// SQL Server stores defaults as named constraints, which block a later
        /// DROP COLUMN, so the added column gets no default there.
        add_default: Option<&'static str>,
    }

    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let cockroach = cockroach_case();
    let mysql = mysql_case();
    let clickhouse = clickhouse_case();
    let mssql = mssql_case();
    let cases = vec![
        DdlCase {
            label: "SQLite",
            config: ConnectionConfig::new_sqlite(9001, "ddl-sqlite".to_string(), sqlite_path),
            database: "main".to_string(),
            schema: None,
            create_table: "CREATE TABLE blanco_ddl_test (id INTEGER PRIMARY KEY)",
            alter_supported: false,
            add_default: Some("0"),
        },
        DdlCase {
            label: "DuckDB",
            config: ConnectionConfig::new_duckdb(9006, "ddl-duckdb".to_string(), duckdb_path),
            database: String::new(),
            schema: Some("main"),
            create_table: "CREATE TABLE blanco_ddl_test (id INTEGER PRIMARY KEY)",
            alter_supported: true,
            add_default: Some("0"),
        },
        DdlCase {
            label: postgres.label,
            config: server(&postgres, 9002),
            database: postgres.database.clone(),
            schema: Some("public"),
            create_table: "CREATE TABLE blanco_ddl_test (id integer PRIMARY KEY)",
            alter_supported: true,
            add_default: Some("0"),
        },
        DdlCase {
            label: cockroach.label,
            config: server(&cockroach, 9007),
            database: cockroach.database.clone(),
            schema: Some("public"),
            create_table: "CREATE TABLE blanco_ddl_test (id integer PRIMARY KEY)",
            alter_supported: true,
            add_default: Some("0"),
        },
        DdlCase {
            label: mysql.label,
            config: server(&mysql, 9003),
            database: mysql.database.clone(),
            schema: None,
            create_table: "CREATE TABLE blanco_ddl_test (id integer PRIMARY KEY)",
            alter_supported: true,
            add_default: Some("0"),
        },
        DdlCase {
            label: clickhouse.label,
            config: server(&clickhouse, 9004),
            database: clickhouse.database.clone(),
            schema: None,
            create_table: "CREATE TABLE blanco_ddl_test (id Int32) ENGINE = MergeTree ORDER BY id",
            alter_supported: true,
            add_default: Some("0"),
        },
        DdlCase {
            label: mssql.label,
            config: server(&mssql, 9005).with_trust_server_certificate(true),
            database: mssql.database.clone(),
            schema: Some("dbo"),
            create_table: "CREATE TABLE blanco_ddl_test (id integer PRIMARY KEY)",
            alter_supported: true,
            add_default: None,
        },
    ];

    let service = DatabaseService::new(runtime.handle().clone());

    for case in cases {
        let label = case.label;
        let db_type = case.config.db_type;
        let database = Some(case.database.as_str());
        // `get_or_create_connection` only builds the driver; pools open on
        // first use, so ping to learn whether the server is up before the
        // first statement turns a down server into a pool timeout panic.
        let connection = runtime.block_on(async {
            service.add_connection_config(case.config.clone()).await;
            let connection = service
                .get_or_create_connection(case.config.id, database)
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        });
        let connection = match connection {
            Ok(connection) => connection,
            Err(error) if strict() => panic!("{label}: server unreachable: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };

        let run = |statements: Vec<String>| {
            let operations: Vec<WriteOperation> =
                statements.iter().map(|sql| sql.as_str().into()).collect();
            runtime
                .block_on(connection.execute_operations_transactional(&operations, database))
                .map_err(|failure| failure.error)
        };
        let columns = || -> Vec<ColumnInfo> {
            runtime
                .block_on(connection.get_columns_for_table("blanco_ddl_test", case.schema))
                .unwrap_or_else(|error| panic!("{label}: columns: {error:#}"))
        };
        let indexes = || -> Vec<IndexInfo> {
            runtime
                .block_on(connection.get_indexes_for_table("blanco_ddl_test", case.schema))
                .unwrap_or_else(|error| panic!("{label}: indexes: {error:#}"))
        };
        let build_column = |operation: &ColumnOperation| {
            column_operation_sql(db_type, case.schema, "blanco_ddl_test", operation)
        };
        let build_index = |operation: &IndexOperation| {
            index_operation_sql(db_type, case.schema, "blanco_ddl_test", operation)
        };

        runtime
            .block_on(connection.execute_write(
                "DROP TABLE IF EXISTS blanco_ddl_test",
                database,
                &[],
            ))
            .unwrap_or_else(|error| panic!("{label}: drop leftover: {error:#}"));
        runtime
            .block_on(connection.execute_write(case.create_table, database, &[]))
            .unwrap_or_else(|error| panic!("{label}: create: {error:#}"));

        // Add a column.
        let add = ColumnOperation::Add {
            column: ColumnSpec {
                name: "score".to_string(),
                data_type: if db_type == DatabaseType::ClickHouse {
                    "Int32".to_string()
                } else {
                    "integer".to_string()
                },
                nullable: false,
                default: case.add_default.map(str::to_string),
                comment: None,
                ..ColumnSpec::default()
            },
            position: None,
        };
        run(build_column(&add).unwrap_or_else(|error| panic!("{label}: {error}")))
            .unwrap_or_else(|error| panic!("{label}: add column: {error:#}"));
        let added = columns()
            .into_iter()
            .find(|column| column.name == "score")
            .unwrap_or_else(|| panic!("{label}: added column missing"));
        assert!(!added.is_nullable, "{label}: NOT NULL should stick");
        if case.add_default.is_some() {
            assert!(
                added.default_value.is_some(),
                "{label}: default should be reported"
            );
        }

        // Create an index on it.
        let create_index = IndexOperation::Create(IndexSpec {
            name: "blanco_ddl_test_score_idx".to_string(),
            columns: vec!["score".to_string()],
            ..IndexSpec::default()
        });
        run(build_index(&create_index).unwrap_or_else(|error| panic!("{label}: {error}")))
            .unwrap_or_else(|error| panic!("{label}: create index: {error:#}"));
        assert!(
            indexes()
                .iter()
                .any(|index| index.name == "blanco_ddl_test_score_idx"),
            "{label}: created index missing from {:?}",
            indexes()
        );

        // Comments on the column and the table, read back through the
        // column listing and the table information.
        let table_info = || {
            runtime
                .block_on(connection.table_info(case.schema, "blanco_ddl_test"))
                .unwrap_or_else(|error| panic!("{label}: table info: {error:#}"))
        };
        let score_spec = || {
            let added = columns()
                .into_iter()
                .find(|column| column.name == "score")
                .unwrap_or_else(|| panic!("{label}: added column missing"));
            ColumnSpec {
                name: added.name.clone(),
                data_type: added.display_type(),
                nullable: added.is_nullable,
                default: added.default_value.clone(),
                comment: added.comment,
                ..ColumnSpec::default()
            }
        };
        if db_type.dialect().supports_comments() {
            let uncommented = score_spec();
            let comment = ColumnOperation::Alter {
                from: uncommented.clone(),
                to: ColumnSpec {
                    comment: Some("Points it's worth".to_string()),
                    ..uncommented
                },
            };
            run(build_column(&comment).unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: comment column: {error:#}"));
            assert_eq!(
                score_spec().comment.as_deref(),
                Some("Points it's worth"),
                "{label}: column comment"
            );
            run(table_change_sql(
                db_type,
                case.schema,
                "blanco_ddl_test",
                &TableChange::Comment(Some("Scratch table".to_string())),
            )
            .unwrap_or_else(|error| panic!("{label}: {error}")))
            .unwrap_or_else(|error| panic!("{label}: comment table: {error:#}"));
            assert_eq!(
                table_info().comment.as_deref(),
                Some("Scratch table"),
                "{label}: table comment"
            );
        } else {
            assert_eq!(table_info().comment, None, "{label}");
        }

        // MySQL restates a column to comment it, which must keep what its
        // reported type alone would lose.
        if db_type == DatabaseType::MySQL {
            runtime
                .block_on(connection.execute_write(
                    "CREATE TABLE blanco_ddl_attrs (id int unsigned NOT NULL AUTO_INCREMENT PRIMARY KEY)",
                    database,
                    &[],
                ))
                .unwrap_or_else(|error| panic!("{label}: create: {error:#}"));
            let definition = || {
                runtime
                    .block_on(connection.table_info(case.schema, "blanco_ddl_attrs"))
                    .unwrap_or_else(|error| panic!("{label}: table info: {error:#}"))
                    .column_definitions
                    .into_iter()
                    .find(|definition| definition.name == "id")
                    .unwrap_or_else(|| panic!("{label}: no definition of id"))
            };
            let before = definition();
            let from = ColumnSpec {
                name: "id".to_string(),
                data_type: before.column_type.clone(),
                nullable: false,
                attributes: before.attributes.clone(),
                ..ColumnSpec::default()
            };
            let comment = ColumnOperation::Alter {
                to: ColumnSpec {
                    comment: Some("Row id".to_string()),
                    ..from.clone()
                },
                from,
            };
            run(
                column_operation_sql(db_type, case.schema, "blanco_ddl_attrs", &comment)
                    .unwrap_or_else(|error| panic!("{label}: {error}")),
            )
            .unwrap_or_else(|error| panic!("{label}: comment id: {error:#}"));
            let after = definition();
            assert_eq!(after.column_type, before.column_type, "{label}");
            assert!(after.column_type.contains("unsigned"), "{label}: {after:?}");
            assert_eq!(
                after.attributes.as_deref(),
                Some("auto_increment"),
                "{label}"
            );
            runtime
                .block_on(connection.execute_write("DROP TABLE blanco_ddl_attrs", database, &[]))
                .unwrap_or_else(|error| panic!("{label}: drop: {error:#}"));
        }

        // Place a column first, then move it and another one around.
        if db_type.dialect().supports_column_positions() {
            let names =
                || -> Vec<String> { columns().into_iter().map(|column| column.name).collect() };
            let rank = ColumnSpec {
                name: "rank".to_string(),
                data_type: if db_type == DatabaseType::ClickHouse {
                    "Int32".to_string()
                } else {
                    "integer".to_string()
                },
                nullable: true,
                ..ColumnSpec::default()
            };
            let add_first = ColumnOperation::Add {
                column: rank.clone(),
                position: Some(ColumnPosition::First),
            };
            run(build_column(&add_first).unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: add first: {error:#}"));
            assert_eq!(names(), ["rank", "id", "score"], "{label}");

            let move_rank = ColumnOperation::Move {
                column: rank,
                position: ColumnPosition::After("score".to_string()),
            };
            run(build_column(&move_rank).unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: move after: {error:#}"));
            assert_eq!(names(), ["id", "score", "rank"], "{label}");

            // Moving restates the column, which must keep its default and
            // comment.
            let before = score_spec();
            let move_score = ColumnOperation::Move {
                column: before.clone(),
                position: ColumnPosition::First,
            };
            run(build_column(&move_score).unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: move first: {error:#}"));
            assert_eq!(names(), ["score", "id", "rank"], "{label}");
            let after = score_spec();
            assert_eq!(after.comment, before.comment, "{label}");
            assert_eq!(after.default, before.default, "{label}");
            assert!(!after.nullable, "{label}");

            run(build_column(&ColumnOperation::Drop {
                name: "rank".to_string(),
            })
            .unwrap_or_else(|error| panic!("{label}: {error}")))
            .unwrap_or_else(|error| panic!("{label}: drop rank: {error:#}"));
        } else {
            assert!(
                matches!(
                    build_column(&ColumnOperation::Move {
                        column: score_spec(),
                        position: ColumnPosition::First,
                    }),
                    Err(DdlError::Unsupported(_))
                ),
                "{label}"
            );
        }

        // A unique index that counts NULLs as equal, where the server has it.
        let server_version_num = runtime
            .block_on(connection.server_version_num())
            .unwrap_or_else(|error| panic!("{label}: server version: {error:#}"));
        if db_type
            .dialect()
            .supports_nulls_not_distinct(server_version_num)
        {
            let unique = IndexOperation::Create(IndexSpec {
                name: "blanco_ddl_test_score_key".to_string(),
                columns: vec!["score".to_string()],
                unique: true,
                nulls_not_distinct: true,
                ..IndexSpec::default()
            });
            run(build_index(&unique).unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: create unique index: {error:#}"));
            let listed = indexes()
                .into_iter()
                .find(|index| index.name == "blanco_ddl_test_score_key")
                .unwrap_or_else(|| panic!("{label}: unique index missing"));
            assert!(
                listed.is_unique && listed.nulls_not_distinct,
                "{label}: {listed:?}"
            );
            run(build_index(&IndexOperation::Drop {
                name: "blanco_ddl_test_score_key".to_string(),
            })
            .unwrap_or_else(|error| panic!("{label}: {error}")))
            .unwrap_or_else(|error| panic!("{label}: drop unique index: {error:#}"));
        }

        // Alter: rename and widen.
        let added = columns()
            .into_iter()
            .find(|column| column.name == "score")
            .unwrap_or_else(|| panic!("{label}: added column missing"));
        let added_spec = ColumnSpec {
            name: added.name.clone(),
            data_type: added.display_type(),
            nullable: added.is_nullable,
            default: added.default_value.clone(),
            comment: added.comment.clone(),
            ..ColumnSpec::default()
        };
        let alter = ColumnOperation::Alter {
            from: added_spec,
            to: ColumnSpec {
                name: "points".to_string(),
                data_type: if db_type == DatabaseType::ClickHouse {
                    "Int64".to_string()
                } else {
                    "bigint".to_string()
                },
                nullable: true,
                default: None,
                comment: None,
                ..ColumnSpec::default()
            },
        };
        match build_column(&alter) {
            Ok(statements) => {
                assert!(
                    case.alter_supported,
                    "{label}: alter unexpectedly supported"
                );
                // An index on the column blocks the type change on some
                // backends; drop it first as a user would.
                run(build_index(&IndexOperation::Drop {
                    name: "blanco_ddl_test_score_idx".to_string(),
                })
                .unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: drop index: {error:#}"));
                run(statements).unwrap_or_else(|error| panic!("{label}: alter column: {error:#}"));
                let altered = columns()
                    .into_iter()
                    .find(|column| column.name == "points")
                    .unwrap_or_else(|| panic!("{label}: renamed column missing"));
                assert!(altered.is_nullable, "{label}: DROP NOT NULL should stick");
                assert!(
                    altered.data_type.to_lowercase().contains("int"),
                    "{label}: type after alter was {}",
                    altered.data_type
                );
                run(build_column(&ColumnOperation::Drop {
                    name: "points".to_string(),
                })
                .unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: drop column: {error:#}"));
            }
            Err(DdlError::Unsupported(_)) => {
                assert!(!case.alter_supported, "{label}: alter should be supported");
                run(build_index(&IndexOperation::Drop {
                    name: "blanco_ddl_test_score_idx".to_string(),
                })
                .unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: drop index: {error:#}"));
                run(build_column(&ColumnOperation::Drop {
                    name: "score".to_string(),
                })
                .unwrap_or_else(|error| panic!("{label}: {error}")))
                .unwrap_or_else(|error| panic!("{label}: drop column: {error:#}"));
            }
            Err(error) => panic!("{label}: {error}"),
        }
        assert!(
            indexes()
                .iter()
                .all(|index| index.name != "blanco_ddl_test_score_idx"),
            "{label}: index should be gone"
        );
        assert!(
            columns()
                .iter()
                .all(|column| column.name != "score" && column.name != "points"),
            "{label}: column should be gone"
        );

        if db_type.dialect().supports_foreign_keys() {
            let foreign_keys = |table: &str| {
                runtime
                    .block_on(connection.get_foreign_keys_for_table(table, case.schema))
                    .unwrap_or_else(|error| panic!("{label}: foreign keys: {error:#}"))
            };
            runtime
                .block_on(connection.execute_write(
                    "DROP TABLE IF EXISTS blanco_ddl_child",
                    database,
                    &[],
                ))
                .unwrap_or_else(|error| panic!("{label}: drop leftover child: {error:#}"));
            runtime
                .block_on(connection.execute_write(
                    "DROP TABLE IF EXISTS blanco_ddl_parent",
                    database,
                    &[],
                ))
                .unwrap_or_else(|error| panic!("{label}: drop leftover parent: {error:#}"));
            runtime
                .block_on(connection.execute_write(
                    "CREATE TABLE blanco_ddl_parent (id integer NOT NULL, region integer NOT NULL, PRIMARY KEY (id, region))",
                    database,
                    &[],
                ))
                .unwrap_or_else(|error| panic!("{label}: create parent: {error:#}"));

            let create_fk = ForeignKeyOperation::Create {
                name: Some("blanco_ddl_child_parent_fk".to_string()),
                columns: vec!["parent_id".to_string(), "parent_region".to_string()],
                referenced_schema: None,
                referenced_table: "blanco_ddl_parent".to_string(),
                referenced_columns: vec!["id".to_string(), "region".to_string()],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::Cascade,
            };
            match foreign_key_operation_sql(db_type, case.schema, "blanco_ddl_child", &create_fk) {
                Ok(statements) => {
                    runtime
                        .block_on(connection.execute_write(
                            "CREATE TABLE blanco_ddl_child (parent_id integer, parent_region integer)",
                            database,
                            &[],
                        ))
                        .unwrap_or_else(|error| panic!("{label}: create child: {error:#}"));
                    run(statements)
                        .unwrap_or_else(|error| panic!("{label}: create foreign key: {error:#}"));
                    let created = foreign_keys("blanco_ddl_child");
                    assert_eq!(created.len(), 1, "{label}: {created:?}");
                    let fk = &created[0];
                    assert_eq!(fk.name, "blanco_ddl_child_parent_fk", "{label}");
                    assert_eq!(fk.columns, vec!["parent_id", "parent_region"], "{label}");
                    assert_eq!(fk.referenced_table, "blanco_ddl_parent", "{label}");
                    assert_eq!(fk.referenced_columns, vec!["id", "region"], "{label}");
                    assert_eq!(fk.on_delete, "CASCADE", "{label}");
                    // MySQL treats NO ACTION as RESTRICT, and MariaDB reports
                    // the omitted clause that way.
                    assert!(
                        fk.on_update == "NO ACTION"
                            || (db_type == DatabaseType::MySQL && fk.on_update == "RESTRICT"),
                        "{label}: on update was {}",
                        fk.on_update
                    );

                    let drop_fk = ForeignKeyOperation::Drop {
                        name: fk.name.clone(),
                    };
                    run(foreign_key_operation_sql(
                        db_type,
                        case.schema,
                        "blanco_ddl_child",
                        &drop_fk,
                    )
                    .unwrap_or_else(|error| panic!("{label}: {error}")))
                    .unwrap_or_else(|error| panic!("{label}: drop foreign key: {error:#}"));
                    assert!(
                        foreign_keys("blanco_ddl_child").is_empty(),
                        "{label}: foreign key should be gone"
                    );
                }
                Err(DdlError::Unsupported(_)) => {
                    assert!(
                        !db_type.dialect().supports_foreign_key_ddl(),
                        "{label}: foreign key DDL should be supported"
                    );
                    // The list still reads keys declared in CREATE TABLE.
                    // DuckDB enforces no referential actions but NO ACTION.
                    let on_delete = if db_type == DatabaseType::DuckDb {
                        "NO ACTION"
                    } else {
                        "CASCADE"
                    };
                    let action = if db_type == DatabaseType::DuckDb {
                        String::new()
                    } else {
                        format!(" ON DELETE {on_delete}")
                    };
                    runtime
                        .block_on(connection.execute_write(
                            &format!("CREATE TABLE blanco_ddl_child (parent_id integer, parent_region integer, FOREIGN KEY (parent_id, parent_region) REFERENCES blanco_ddl_parent (id, region){action})"),
                            database,
                            &[],
                        ))
                        .unwrap_or_else(|error| panic!("{label}: create child: {error:#}"));
                    let declared = foreign_keys("blanco_ddl_child");
                    assert_eq!(declared.len(), 1, "{label}: {declared:?}");
                    assert_eq!(declared[0].columns, vec!["parent_id", "parent_region"]);
                    assert_eq!(declared[0].referenced_columns, vec!["id", "region"]);
                    assert_eq!(declared[0].on_delete, on_delete);
                }
                Err(error) => panic!("{label}: {error}"),
            }
            for table in ["blanco_ddl_child", "blanco_ddl_parent"] {
                runtime
                    .block_on(connection.execute_write(
                        &format!("DROP TABLE {table}"),
                        database,
                        &[],
                    ))
                    .unwrap_or_else(|error| panic!("{label}: drop {table}: {error:#}"));
            }
        }

        runtime
            .block_on(connection.execute_write("DROP TABLE blanco_ddl_test", database, &[]))
            .unwrap_or_else(|error| panic!("{label}: drop: {error:#}"));

        // A table created in one statement, as the new table tab does: the
        // backend's numbered key, a commented column and an index.
        runtime
            .block_on(connection.execute_write(
                "DROP TABLE IF EXISTS blanco_ddl_created",
                database,
                &[],
            ))
            .unwrap_or_else(|error| panic!("{label}: drop leftover: {error:#}"));
        let (key_type, attributes) = db_type.dialect().new_table_key();
        let new_table = NewTable {
            columns: vec![
                ColumnSpec {
                    name: "id".to_string(),
                    data_type: key_type.to_string(),
                    attributes: attributes.map(str::to_string),
                    primary_key: true,
                    ..ColumnSpec::default()
                },
                ColumnSpec {
                    name: "label".to_string(),
                    data_type: if db_type == DatabaseType::ClickHouse {
                        "String".to_string()
                    } else {
                        "varchar(40)".to_string()
                    },
                    nullable: true,
                    comment: db_type
                        .dialect()
                        .supports_comments()
                        .then(|| "Shown to people".to_string()),
                    ..ColumnSpec::default()
                },
            ],
            indexes: vec![IndexSpec {
                name: "blanco_ddl_created_label_idx".to_string(),
                columns: vec!["label".to_string()],
                ..IndexSpec::default()
            }],
            foreign_keys: Vec::new(),
        };
        run(
            create_table_sql(db_type, case.schema, "blanco_ddl_created", &new_table)
                .unwrap_or_else(|error| panic!("{label}: {error}")),
        )
        .unwrap_or_else(|error| panic!("{label}: create table: {error:#}"));
        let created = runtime
            .block_on(connection.get_columns_for_table("blanco_ddl_created", case.schema))
            .unwrap_or_else(|error| panic!("{label}: columns: {error:#}"));
        let names: Vec<&str> = created.iter().map(|column| column.name.as_str()).collect();
        assert_eq!(names, ["id", "label"], "{label}");
        assert!(created[0].is_primary_key, "{label}: id should be the key");
        if db_type.dialect().supports_comments() {
            assert_eq!(
                created[1].comment.as_deref(),
                Some("Shown to people"),
                "{label}"
            );
        }
        assert!(
            runtime
                .block_on(connection.get_indexes_for_table("blanco_ddl_created", case.schema))
                .unwrap_or_else(|error| panic!("{label}: indexes: {error:#}"))
                .iter()
                .any(|index| index.name == "blanco_ddl_created_label_idx"),
            "{label}: index missing"
        );
        runtime
            .block_on(connection.execute_write("DROP TABLE blanco_ddl_created", database, &[]))
            .unwrap_or_else(|error| panic!("{label}: drop created: {error:#}"));
    }
}

/// `describe_query_columns` must return the header of a read that matches no
/// rows, since the result-set path only learns columns from the first row.
#[test]
fn test_describe_query_columns_on_empty_result() {
    use blanco_core::ColumnType;
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir
        .path()
        .join("describe.db")
        .to_string_lossy()
        .to_string();

    struct DescribeCase {
        label: &'static str,
        config: ConnectionConfig,
        database: String,
        create_table: &'static str,
    }

    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let cockroach = cockroach_case();
    let mysql = mysql_case();
    let mssql = mssql_case();
    let cases = vec![
        DescribeCase {
            label: "SQLite",
            config: ConnectionConfig::new_sqlite(9101, "describe-sqlite".to_string(), sqlite_path),
            database: "main".to_string(),
            create_table: "CREATE TABLE blanco_describe_test (id INTEGER PRIMARY KEY, label TEXT)",
        },
        DescribeCase {
            label: postgres.label,
            config: server(&postgres, 9102),
            database: postgres.database.clone(),
            create_table: "CREATE TABLE blanco_describe_test (id integer PRIMARY KEY, label text)",
        },
        DescribeCase {
            label: cockroach.label,
            config: server(&cockroach, 9105),
            database: cockroach.database.clone(),
            create_table: "CREATE TABLE blanco_describe_test (id integer PRIMARY KEY, label text)",
        },
        DescribeCase {
            label: mysql.label,
            config: server(&mysql, 9103),
            database: mysql.database.clone(),
            create_table: "CREATE TABLE blanco_describe_test (id integer PRIMARY KEY, label text)",
        },
        DescribeCase {
            label: mssql.label,
            config: server(&mssql, 9104).with_trust_server_certificate(true),
            database: mssql.database.clone(),
            create_table: "CREATE TABLE blanco_describe_test (id integer PRIMARY KEY, label nvarchar(50))",
        },
    ];

    let service = DatabaseService::new(runtime.handle().clone());

    for case in cases {
        let label = case.label;
        let database = Some(case.database.as_str());
        let connection = runtime.block_on(async {
            service.add_connection_config(case.config.clone()).await;
            let connection = service
                .get_or_create_connection(case.config.id, database)
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        });
        let connection = match connection {
            Ok(connection) => connection,
            Err(error) if strict() => panic!("{label}: server unreachable: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };

        runtime
            .block_on(connection.execute_write(
                "DROP TABLE IF EXISTS blanco_describe_test",
                database,
                &[],
            ))
            .unwrap_or_else(|error| panic!("{label}: drop leftover: {error:#}"));
        runtime
            .block_on(connection.execute_write(case.create_table, database, &[]))
            .unwrap_or_else(|error| panic!("{label}: create: {error:#}"));

        let query = "SELECT * FROM blanco_describe_test WHERE id = -1";
        let result = runtime
            .block_on(connection.execute_script(query, database))
            .unwrap_or_else(|error| panic!("{label}: query: {error:#}"));
        assert_eq!(result.len(), 1, "{label}: one result set");
        assert!(result[0].rows.is_empty(), "{label}: no rows");

        let (columns, column_types) = runtime
            .block_on(connection.describe_query_columns(query, database))
            .unwrap_or_else(|error| panic!("{label}: describe: {error:#}"));
        assert_eq!(
            columns,
            vec!["id".to_string(), "label".to_string()],
            "{label}: column names"
        );
        assert_eq!(
            column_types,
            vec![ColumnType::Integer, ColumnType::Text],
            "{label}: column types"
        );

        runtime
            .block_on(connection.execute_write("DROP TABLE blanco_describe_test", database, &[]))
            .unwrap_or_else(|error| panic!("{label}: drop: {error:#}"));
    }
}

/// A column in several foreign keys came back once per key, and the grid
/// then compared a primary key column twice in the WHERE of its edits.
#[test]
fn test_column_in_several_foreign_keys_is_listed_once() {
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let file = |name: &str| temp_dir.path().join(name).to_string_lossy().to_string();

    struct ForeignKeyCase {
        label: &'static str,
        config: ConnectionConfig,
        database: String,
        schema: Option<&'static str>,
    }

    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let cockroach = cockroach_case();
    let mysql = mysql_case();
    let mssql = mssql_case();
    let cases = vec![
        ForeignKeyCase {
            label: "SQLite",
            config: ConnectionConfig::new_sqlite(9201, "fk-sqlite".to_string(), file("fk.db")),
            database: "main".to_string(),
            schema: None,
        },
        ForeignKeyCase {
            label: "DuckDB",
            config: ConnectionConfig::new_duckdb(9202, "fk-duckdb".to_string(), file("fk.duckdb")),
            database: String::new(),
            schema: Some("main"),
        },
        ForeignKeyCase {
            label: postgres.label,
            config: server(&postgres, 9203),
            database: postgres.database.clone(),
            schema: Some("public"),
        },
        ForeignKeyCase {
            label: cockroach.label,
            config: server(&cockroach, 9206),
            database: cockroach.database.clone(),
            schema: Some("public"),
        },
        ForeignKeyCase {
            label: mysql.label,
            config: server(&mysql, 9204),
            database: mysql.database.clone(),
            schema: None,
        },
        ForeignKeyCase {
            label: mssql.label,
            config: server(&mssql, 9205).with_trust_server_certificate(true),
            database: mssql.database.clone(),
            schema: Some("dbo"),
        },
    ];
    let drop_tables = [
        "DROP TABLE IF EXISTS blanco_fk_themes",
        "DROP TABLE IF EXISTS blanco_fk_groups",
        "DROP TABLE IF EXISTS blanco_fk_tenants",
    ];
    let create_tables = [
        "CREATE TABLE blanco_fk_tenants (id integer PRIMARY KEY)",
        "CREATE TABLE blanco_fk_groups (tenant_id integer NOT NULL, id integer NOT NULL, \
         PRIMARY KEY (tenant_id, id))",
        "CREATE TABLE blanco_fk_themes (tenant_id integer NOT NULL, id integer NOT NULL, \
         group_id integer NOT NULL, PRIMARY KEY (tenant_id, id), \
         CONSTRAINT blanco_fk_themes_group FOREIGN KEY (tenant_id, group_id) \
         REFERENCES blanco_fk_groups (tenant_id, id), \
         CONSTRAINT blanco_fk_themes_tenant FOREIGN KEY (tenant_id) \
         REFERENCES blanco_fk_tenants (id))",
    ];

    let service = DatabaseService::new(runtime.handle().clone());

    for case in cases {
        let label = case.label;
        let database = Some(case.database.as_str());
        let connection = runtime.block_on(async {
            service.add_connection_config(case.config.clone()).await;
            let connection = service
                .get_or_create_connection(case.config.id, database)
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        });
        let connection = match connection {
            Ok(connection) => connection,
            Err(error) if strict() => panic!("{label}: server unreachable: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };
        let execute = |statement: &str| {
            runtime
                .block_on(connection.execute_write(statement, database, &[]))
                .unwrap_or_else(|error| panic!("{label}: {statement}: {error:#}"));
        };

        drop_tables.iter().for_each(|statement| execute(statement));
        create_tables
            .iter()
            .for_each(|statement| execute(statement));

        let columns = runtime
            .block_on(connection.get_columns_for_table("blanco_fk_themes", case.schema))
            .unwrap_or_else(|error| panic!("{label}: columns: {error:#}"));
        let names: Vec<&str> = columns.iter().map(|column| column.name.as_str()).collect();
        assert_eq!(names, ["tenant_id", "id", "group_id"], "{label}: columns");
        let primary_key: Vec<&str> = columns
            .iter()
            .filter(|column| column.is_primary_key)
            .map(|column| column.name.as_str())
            .collect();
        assert_eq!(primary_key, ["tenant_id", "id"], "{label}: primary key");
        let group = columns
            .iter()
            .find(|column| column.name == "group_id")
            .and_then(|column| column.foreign_key.as_ref())
            .unwrap_or_else(|| panic!("{label}: group_id has a foreign key"));
        assert!(
            group.foreign_table_name.ends_with("blanco_fk_groups")
                && group.foreign_column_name == "id",
            "{label}: group_id references blanco_fk_groups(id), got {group:?}"
        );

        drop_tables.iter().for_each(|statement| execute(statement));
    }
}

/// The sidebar reads table parents through `DatabaseService`, whose
/// connections wrap the driver in `TokioConnection`; a method the wrapper does
/// not forward silently falls back to the trait's empty default.
#[test]
fn test_postgres_table_parents_through_database_service() {
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let postgres = postgres_case();
    let config = ConnectionConfig::new(
        9201,
        postgres.name.to_string(),
        postgres.db_type,
        postgres.host.clone(),
        postgres.port.parse().expect("port"),
        postgres.database.clone(),
        postgres.username.clone(),
        Some(postgres.password.clone()),
    );
    let database = Some(postgres.database.as_str());
    let service = DatabaseService::new(runtime.handle().clone());

    let connection = runtime.block_on(async {
        service.add_connection_config(config.clone()).await;
        let connection = service
            .get_or_create_connection(config.id, database)
            .await?;
        connection.ping().await?;
        anyhow::Ok(connection)
    });
    let connection = match connection {
        Ok(connection) => connection,
        Err(error) if strict() => panic!("PostgreSQL: server unreachable: {error:#}"),
        Err(error) => {
            eprintln!(
                "skip PostgreSQL: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            return;
        }
    };

    for statement in [
        "DROP SCHEMA IF EXISTS blanco_sidebar_partitions CASCADE",
        "CREATE SCHEMA blanco_sidebar_partitions",
        "CREATE TABLE blanco_sidebar_partitions.events (id integer, kind text)
            PARTITION BY LIST (kind)",
        "CREATE TABLE blanco_sidebar_partitions.events_click
            PARTITION OF blanco_sidebar_partitions.events FOR VALUES IN ('click')",
        "CREATE TABLE blanco_sidebar_partitions.events_view
            PARTITION OF blanco_sidebar_partitions.events FOR VALUES IN ('view')",
    ] {
        runtime
            .block_on(connection.execute_write(statement, database, &[]))
            .unwrap_or_else(|error| panic!("setup `{statement}`: {error:#}"));
    }

    let parents = runtime
        .block_on(connection.get_table_parents(Some("blanco_sidebar_partitions")))
        .unwrap_or_else(|error| panic!("table parents: {error:#}"));
    let mut grouped: Vec<(&str, &str, bool)> = parents
        .iter()
        .map(|(child, parent)| (child.as_str(), parent.name.as_str(), parent.is_partition))
        .collect();
    grouped.sort();
    assert_eq!(
        grouped,
        [
            ("events_click", "events", true),
            ("events_view", "events", true),
        ]
    );

    runtime
        .block_on(connection.execute_write(
            "DROP SCHEMA blanco_sidebar_partitions CASCADE",
            database,
            &[],
        ))
        .unwrap_or_else(|error| panic!("drop: {error:#}"));
}

/// Answers secret prompts from a script and records each request.
struct ScriptedPrompt {
    answers: std::sync::Mutex<Vec<Option<String>>>,
    asked: std::sync::Mutex<Vec<database::SecretPromptRequest>>,
}

#[async_trait::async_trait]
impl database::SecretPrompt for ScriptedPrompt {
    async fn ask(&self, request: database::SecretPromptRequest) -> anyhow::Result<Option<String>> {
        self.asked.lock().unwrap().push(request);
        Ok(self.answers.lock().unwrap().pop().flatten())
    }
}

/// A connection saved without a password asks when the server wants one,
/// asks again after a wrong answer, and reuses the right one for a reconnect
/// until it is forgotten. One set to ask on connect asks up front.
#[test]
fn test_password_prompt_against_postgres() {
    password_prompt_round_trip(postgres_case(), 9101);
}

/// MySQL refuses the password while the pool opens rather than at the first
/// query, so it takes the other path through the retry.
#[test]
fn test_password_prompt_against_mysql() {
    password_prompt_round_trip(mysql_case(), 9111);
}

fn password_prompt_round_trip(case: DriverCase, id: i64) {
    use database::{ConnectionConfig, SecretKind};
    use std::sync::Arc;

    let label = case.label;
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let prompt = Arc::new(ScriptedPrompt {
        // Popped from the end: wrong, then right, then right for the
        // ask-on-connect config.
        answers: std::sync::Mutex::new(vec![
            Some(case.password.clone()),
            Some(case.password.clone()),
            Some("wrong".to_string()),
        ]),
        asked: std::sync::Mutex::new(Vec::new()),
    });
    let mut service = DatabaseService::new(runtime.handle().clone());
    service.set_secret_prompt(prompt.clone());
    // No password file: the test is about being asked, whatever is in the
    // `~/.pgpass` of whoever runs it.
    let password_file_dir = tempfile::tempdir().expect("temp dir");
    service.set_password_file(password_file_dir.path().join("pgpass"));

    let config = ConnectionConfig::new(
        id,
        "prompted".to_string(),
        case.db_type,
        case.host.clone(),
        case.port.parse().expect("port"),
        case.database.clone(),
        case.username.clone(),
        None,
    );
    let database = Some(case.database.as_str());
    let connected = runtime.block_on(async {
        service.add_connection_config(config.clone()).await;
        service.get_or_create_connection(config.id, database).await
    });
    let connection = match connected {
        Ok(connection) => connection,
        Err(error) if prompt.asked.lock().unwrap().is_empty() && !strict() => {
            eprintln!("skip {label}: server unreachable ({error:#})");
            return;
        }
        Err(error) => panic!("{label}: {error:#}"),
    };
    runtime
        .block_on(connection.execute_query("SELECT 1", database, None))
        .expect("query with the prompted password");
    {
        let asked = prompt.asked.lock().unwrap();
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert_eq!(asked[0].kind, SecretKind::Password);
        // The first prompt is the server asking; only the refused answer
        // comes with a reason.
        assert_eq!(asked[0].retry_reason, None);
        assert!(
            asked[1]
                .retry_reason
                .as_deref()
                .is_some_and(|reason| reason.to_lowercase().contains(match case.db_type {
                    DatabaseType::MySQL => "access denied",
                    _ => "password authentication failed",
                })),
            "{:?}",
            asked[1].retry_reason
        );
    }

    // A dropped connection reconnects with the remembered password.
    runtime.block_on(async {
        service
            .disconnect(config.id, None)
            .await
            .expect("disconnect");
        service
            .get_or_create_connection(config.id, database)
            .await
            .expect("reconnect");
    });
    assert_eq!(prompt.asked.lock().unwrap().len(), 2);

    // Ask on connect asks before connecting; the script's last answer is
    // right, then it runs out and the next prompt is cancelled.
    let asking = ConnectionConfig {
        id: id + 1,
        ..config.clone()
    }
    .with_ask_secrets(vec![SecretKind::Password]);
    runtime.block_on(async {
        service.add_connection_config(asking.clone()).await;
        service
            .get_or_create_connection(asking.id, database)
            .await
            .expect("ask on connect");
        service
            .disconnect(asking.id, None)
            .await
            .expect("disconnect");
        service.forget_session_secrets(asking.id).await;
        let error = match service.get_or_create_connection(asking.id, database).await {
            Ok(_) => panic!("cancelled prompt should fail the connect"),
            Err(error) => error,
        };
        assert!(
            error
                .downcast_ref::<database::SecretPromptCancelled>()
                .is_some(),
            "{error:#}"
        );
    });
    assert_eq!(prompt.asked.lock().unwrap().len(), 4);
}

/// Binary values against every live backend that has a binary type: bytes
/// written with the dialect's literal come back as the same hex, a binary
/// key addresses its row, the stream export path agrees with the query path
/// and the filter predicates select what they say.
#[test]
fn test_binary_values_round_trip() {
    use blanco_core::binary::encode;
    use blanco_core::{ColumnType, FilterCondition, FilterOperator};
    use database::ConnectionConfig;
    use futures::StreamExt as _;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir
        .path()
        .join("binary.db")
        .to_string_lossy()
        .to_string();

    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let cockroach = cockroach_case();
    let mysql = mysql_case();
    let mssql = mssql_case();
    let cases = vec![
        (
            "SQLite",
            ConnectionConfig::new_sqlite(9101, "binary-sqlite".to_string(), sqlite_path),
            "main".to_string(),
            "CREATE TABLE blanco_binary_test (id BLOB PRIMARY KEY, body BLOB)",
        ),
        (
            postgres.label,
            server(&postgres, 9102),
            postgres.database.clone(),
            "CREATE TABLE blanco_binary_test (id bytea PRIMARY KEY, body bytea)",
        ),
        (
            cockroach.label,
            server(&cockroach, 9105),
            cockroach.database.clone(),
            "CREATE TABLE blanco_binary_test (id bytea PRIMARY KEY, body bytea)",
        ),
        (
            mysql.label,
            server(&mysql, 9103),
            mysql.database.clone(),
            "CREATE TABLE blanco_binary_test (id VARBINARY(16) PRIMARY KEY, body LONGBLOB)",
        ),
        (
            mssql.label,
            server(&mssql, 9104).with_trust_server_certificate(true),
            mssql.database.clone(),
            "CREATE TABLE blanco_binary_test (id VARBINARY(16) PRIMARY KEY, body VARBINARY(MAX))",
        ),
    ];

    let every_byte: Vec<u8> = (0..=255).collect();
    let image_start = b"\x89PNG\r\n".to_vec();
    let service = DatabaseService::new(runtime.handle().clone());

    for (label, config, database, create_table) in cases {
        let dialect = config.db_type.dialect();
        let database = Some(database.as_str());
        let connection = runtime.block_on(async {
            service.add_connection_config(config.clone()).await;
            let connection = service
                .get_or_create_connection(config.id, database)
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        });
        let connection = match connection {
            Ok(connection) => connection,
            Err(error) if strict() => panic!("{label}: server unreachable: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };
        let write = |sql: &str| {
            runtime
                .block_on(connection.execute_write(sql, database, &[]))
                .unwrap_or_else(|error| panic!("{label}: {sql}: {error:#}"))
        };
        let literal = |bytes: &[u8]| dialect.value_literal(&encode(bytes), ColumnType::Binary);

        write("DROP TABLE IF EXISTS blanco_binary_test");
        write(create_table);
        for (key, body) in [
            (1u8, Some(&every_byte)),
            (2, Some(&image_start)),
            (3, Some(&Vec::new())),
            (4, None),
        ] {
            let body = body.map_or("NULL".to_string(), |body| literal(body));
            write(&format!(
                "INSERT INTO blanco_binary_test (id, body) VALUES ({}, {body})",
                literal(&[key])
            ));
        }

        let expected = vec![
            vec![Some("0x01".to_string()), Some(encode(&every_byte))],
            vec![Some("0x02".to_string()), Some(encode(&image_start))],
            vec![Some("0x03".to_string()), Some("0x".to_string())],
            vec![Some("0x04".to_string()), None],
        ];
        let select = "SELECT id, body FROM blanco_binary_test ORDER BY id";
        let result = runtime
            .block_on(connection.execute_query(select, database, None))
            .unwrap_or_else(|error| panic!("{label}: select: {error:#}"));
        assert_eq!(
            result.column_types,
            vec![ColumnType::Binary, ColumnType::Binary],
            "{label}: column types"
        );
        assert_eq!(result.rows, expected, "{label}: queried rows");

        // Exports read through the streaming path.
        let streamed = runtime.block_on(async {
            let (_, column_types, mut rows) = connection
                .execute_query_stream_rows(select, database)
                .await
                .unwrap_or_else(|error| panic!("{label}: stream: {error:#}"));
            assert_eq!(column_types[1], ColumnType::Binary, "{label}: stream types");
            let mut collected = Vec::new();
            while let Some(row) = rows.next().await {
                collected.push(row.unwrap_or_else(|error| panic!("{label}: row: {error:#}")));
            }
            collected
        });
        assert_eq!(streamed, expected, "{label}: streamed rows");

        let matching = |operator: FilterOperator, value: &str| -> Vec<String> {
            let predicate = dialect.filter_predicate(&FilterCondition {
                column: "body".to_string(),
                operator,
                value: value.to_string(),
                numeric: false,
                binary: true,
                geometry: false,
            });
            let sql = format!("SELECT id FROM blanco_binary_test WHERE {predicate} ORDER BY id");
            runtime
                .block_on(connection.execute_query(&sql, database, None))
                .unwrap_or_else(|error| panic!("{label}: {sql}: {error:#}"))
                .rows
                .into_iter()
                .map(|row| row[0].clone().unwrap_or_default())
                .collect()
        };
        assert_eq!(
            matching(FilterOperator::Equals, "0x89504e470d0a"),
            ["0x02"],
            "{label}: equals"
        );
        assert_eq!(
            matching(FilterOperator::NotEquals, "0x89504e470d0a"),
            ["0x01", "0x03"],
            "{label}: not equals"
        );
        assert_eq!(
            matching(FilterOperator::StartsWith, "8950"),
            ["0x02"],
            "{label}: starts with"
        );
        assert_eq!(
            matching(FilterOperator::EndsWith, "0x0d0a"),
            ["0x02"],
            "{label}: ends with"
        );
        assert_eq!(
            matching(FilterOperator::EndsWith, "0xfeff"),
            ["0x01"],
            "{label}: ends with, long value"
        );
        assert_eq!(
            matching(FilterOperator::Contains, "0x4e47"),
            ["0x02"],
            "{label}: contains"
        );
        assert_eq!(
            matching(FilterOperator::NotContains, "0x4e47"),
            ["0x01", "0x03"],
            "{label}: does not contain"
        );
        assert_eq!(
            matching(FilterOperator::In, "0x, 0x89504e470d0a"),
            ["0x02", "0x03"],
            "{label}: in"
        );
        assert_eq!(
            matching(FilterOperator::Greater, "0x88"),
            ["0x02"],
            "{label}: greater"
        );

        // A binary key addresses its row.
        let updated = write(&format!(
            "UPDATE blanco_binary_test SET body = {} WHERE id = {}",
            literal(&[0xca, 0xfe]),
            literal(&[4])
        ));
        assert_eq!(updated, 1, "{label}: update by binary key");
        let result = runtime
            .block_on(connection.execute_query(
                &format!(
                    "SELECT body FROM blanco_binary_test WHERE id = {}",
                    literal(&[4])
                ),
                database,
                None,
            ))
            .unwrap_or_else(|error| panic!("{label}: reselect: {error:#}"));
        assert_eq!(
            result.rows,
            vec![vec![Some("0xcafe".to_string())]],
            "{label}"
        );

        write("DROP TABLE blanco_binary_test");
    }
}

/// PostGIS values against a live PostgreSQL that has the extension: both
/// read paths decode geometry and geography to EWKT, EWKT written back as a
/// string literal is the same value, WKT typed without an SRID keeps the one
/// of the value it replaces, and text filters match WKT. Skipped when the
/// server has no PostGIS; the docker-compose image has it.
#[test]
fn test_postgis_values_round_trip() {
    use blanco_core::geometry::carry_srid;
    use blanco_core::{ColumnType, FilterCondition, FilterOperator};
    use database::ConnectionConfig;
    use futures::StreamExt as _;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let case = postgres_case();
    let config = ConnectionConfig::new(
        9111,
        case.name.to_string(),
        case.db_type,
        case.host.clone(),
        case.port.parse().expect("port"),
        case.database.clone(),
        case.username.clone(),
        Some(case.password.clone()),
    );
    let dialect = config.db_type.dialect();
    let database = Some(case.database.as_str());
    let service = DatabaseService::new(runtime.handle().clone());
    let connection = runtime.block_on(async {
        service.add_connection_config(config.clone()).await;
        let connection = service
            .get_or_create_connection(config.id, database)
            .await?;
        connection.ping().await?;
        anyhow::Ok(connection)
    });
    let connection = match connection {
        Ok(connection) => connection,
        Err(error) if strict() => panic!("PostgreSQL: server unreachable: {error:#}"),
        Err(error) => {
            eprintln!(
                "skip PostgreSQL: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            return;
        }
    };
    let write = |sql: &str| runtime.block_on(connection.execute_write(sql, database, &[]));
    if let Err(error) = write("CREATE EXTENSION IF NOT EXISTS postgis") {
        eprintln!("skip PostGIS: the extension is not available ({error:#})");
        return;
    }
    let write = |sql: &str| write(sql).unwrap_or_else(|error| panic!("{sql}: {error:#}"));
    let literal = |ewkt: &str| dialect.value_literal(ewkt, ColumnType::Geometry);

    write("DROP TABLE IF EXISTS blanco_geometry_test");
    write(
        "CREATE TABLE blanco_geometry_test (id integer PRIMARY KEY, shape geometry, \
         place geography(Point, 4326), located geometry(Point, 4326))",
    );
    let shapes = [
        "SRID=3006;POLYGON((0 0,4 0,4 4,0 4,0 0),(1 1,2 1,2 2,1 1))",
        "LINESTRING Z (0 0 1,1 1 2)",
        "GEOMETRYCOLLECTION(POINT(1 2),MULTIPOINT((0 0),(1 1)))",
        "POINT EMPTY",
    ];
    for (id, shape) in shapes.iter().enumerate() {
        write(&format!(
            "INSERT INTO blanco_geometry_test VALUES ({id}, {}, {}, {})",
            literal(shape),
            literal("SRID=4326;POINT(18.0686 59.3293)"),
            literal("SRID=4326;POINT(-73.9857 40.7484)"),
        ));
    }
    write("INSERT INTO blanco_geometry_test (id) VALUES (9)");

    let mut expected: Vec<Vec<Option<String>>> = shapes
        .iter()
        .map(|shape| {
            vec![
                Some(shape.to_string()),
                Some("SRID=4326;POINT(18.0686 59.3293)".to_string()),
                Some("SRID=4326;POINT(-73.9857 40.7484)".to_string()),
            ]
        })
        .collect();
    expected.push(vec![None, None, None]);
    let select = "SELECT shape, place, located FROM blanco_geometry_test ORDER BY id";
    let result = runtime
        .block_on(connection.execute_query(select, database, None))
        .unwrap_or_else(|error| panic!("select: {error:#}"));
    assert_eq!(result.column_types, vec![ColumnType::Geometry; 3]);
    assert_eq!(result.rows, expected, "queried rows");

    // Exports read through the streaming path, which takes the binary
    // protocol.
    let streamed = runtime.block_on(async {
        let (_, column_types, mut rows) = connection
            .execute_query_stream_rows(select, database)
            .await
            .unwrap_or_else(|error| panic!("stream: {error:#}"));
        assert_eq!(column_types, vec![ColumnType::Geometry; 3]);
        let mut collected = Vec::new();
        while let Some(row) = rows.next().await {
            collected.push(row.unwrap_or_else(|error| panic!("row: {error:#}")));
        }
        collected
    });
    assert_eq!(streamed, expected, "streamed rows");

    // WKT typed over a value in a column without an SRID in its type keeps
    // the value's SRID instead of becoming SRID 0.
    let original = expected[0][0].clone().unwrap_or_default();
    let typed = carry_srid("POINT(1 2)", &original).expect("SRID carried over");
    write(&format!(
        "UPDATE blanco_geometry_test SET shape = {} WHERE id = 0",
        literal(&typed)
    ));
    let reselected = runtime
        .block_on(connection.execute_query(
            "SELECT shape FROM blanco_geometry_test WHERE id = 0",
            database,
            None,
        ))
        .unwrap_or_else(|error| panic!("reselect: {error:#}"));
    assert_eq!(
        reselected.rows,
        vec![vec![Some("SRID=3006;POINT(1 2)".to_string())]]
    );

    let matching = |operator: FilterOperator, value: &str| -> Vec<String> {
        let predicate = dialect.filter_predicate(&FilterCondition {
            column: "shape".to_string(),
            operator,
            value: value.to_string(),
            numeric: false,
            binary: false,
            geometry: true,
        });
        let sql = format!("SELECT id FROM blanco_geometry_test WHERE {predicate} ORDER BY id");
        runtime
            .block_on(connection.execute_query(&sql, database, None))
            .unwrap_or_else(|error| panic!("{sql}: {error:#}"))
            .rows
            .into_iter()
            .map(|row| row[0].clone().unwrap_or_default())
            .collect()
    };
    assert_eq!(matching(FilterOperator::Contains, "linestring z"), ["1"]);
    assert_eq!(matching(FilterOperator::StartsWith, "point(1"), ["0"]);
    assert_eq!(matching(FilterOperator::Like, "%POINT EMPTY"), ["3"]);

    write("DROP TABLE blanco_geometry_test");
}

/// Reading a table with its large values deferred, against every backend
/// that supports it: the rewritten read runs, cuts exactly the values over
/// the threshold, keeps the result's shape and types, and the full values
/// come back by primary key.
#[test]
fn test_deferred_read_round_trip() {
    use blanco_core::ColumnType;
    use blanco_core::binary::encode;
    use blanco_core::deferred::DeferredCell;
    use database::ConnectionConfig;

    const THRESHOLD: u64 = 1024;
    const PREVIEW: usize = blanco_core::deferred::PREVIEW_LENGTH;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir
        .path()
        .join("deferred.db")
        .to_string_lossy()
        .to_string();
    let server = |case: &DriverCase, id: i64| {
        ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            Some(case.password.clone()),
        )
    };
    let postgres = postgres_case();
    let cockroach = cockroach_case();
    let mysql = mysql_case();
    let mssql = mssql_case();
    // The last field is how many bytes a character of the text column takes.
    let cases = vec![
        (
            "SQLite",
            ConnectionConfig::new_sqlite(9201, "deferred-sqlite".to_string(), sqlite_path),
            "main".to_string(),
            "CREATE TABLE blanco_deferred_test (id INTEGER PRIMARY KEY, title VARCHAR(50), body TEXT, data BLOB)",
            1,
        ),
        (
            postgres.label,
            server(&postgres, 9202),
            postgres.database.clone(),
            "CREATE TABLE blanco_deferred_test (id integer PRIMARY KEY, title varchar(50), body text, data bytea)",
            1,
        ),
        (
            cockroach.label,
            server(&cockroach, 9205),
            cockroach.database.clone(),
            "CREATE TABLE blanco_deferred_test (id integer PRIMARY KEY, title varchar(50), body text, data bytea)",
            1,
        ),
        (
            mysql.label,
            server(&mysql, 9203),
            mysql.database.clone(),
            "CREATE TABLE blanco_deferred_test (id integer PRIMARY KEY, title VARCHAR(50), body LONGTEXT, data LONGBLOB)",
            1,
        ),
        (
            mssql.label,
            server(&mssql, 9204).with_trust_server_certificate(true),
            mssql.database.clone(),
            "CREATE TABLE blanco_deferred_test (id integer PRIMARY KEY, title NVARCHAR(50), body NVARCHAR(MAX), data VARBINARY(MAX))",
            2,
        ),
    ];

    let long_text = "ab".repeat(2500);
    let long_bytes = vec![7u8; 3000];
    let service = DatabaseService::new(runtime.handle().clone());

    for (label, config, database, create_table, bytes_per_character) in cases {
        let dialect = config.db_type.dialect();
        let database = Some(database.as_str());
        let connection = runtime.block_on(async {
            service.add_connection_config(config.clone()).await;
            let connection = service
                .get_or_create_connection(config.id, database)
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        });
        let connection = match connection {
            Ok(connection) => connection,
            Err(error) if strict() => panic!("{label}: server unreachable: {error:#}"),
            Err(error) => {
                eprintln!(
                    "skip {label}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
                );
                continue;
            }
        };
        let write = |sql: &str| {
            runtime
                .block_on(connection.execute_write(sql, database, &[]))
                .unwrap_or_else(|error| panic!("{label}: {error:#}"))
        };
        let query = |sql: &str| {
            runtime
                .block_on(connection.execute_query(sql, database, None))
                .unwrap_or_else(|error| panic!("{label}: {sql}: {error:#}"))
        };

        write("DROP TABLE IF EXISTS blanco_deferred_test");
        write(create_table);
        // The NULL row comes first: a driver that types a column from its
        // first value has nothing to go by.
        write(
            "INSERT INTO blanco_deferred_test (id, title, body, data) VALUES (1, NULL, NULL, NULL)",
        );
        write(&format!(
            "INSERT INTO blanco_deferred_test (id, title, body, data) VALUES (2, 'small', 'short', {})",
            dialect.value_literal("0x0102", ColumnType::Binary)
        ));
        write(&format!(
            "INSERT INTO blanco_deferred_test (id, title, body, data) VALUES (3, 'large', {}, {})",
            dialect.string_literal(&long_text),
            dialect.value_literal(&encode(&long_bytes), ColumnType::Binary)
        ));

        let table_columns = runtime
            .block_on(connection.get_columns_for_table("blanco_deferred_test", None))
            .unwrap_or_else(|error| panic!("{label}: columns: {error:#}"));
        let original = dialect.apply_row_limit(
            "SELECT * FROM blanco_deferred_test WHERE id > 0 ORDER BY id",
            100,
        );
        let plan = dialect
            .deferred_read(&original, &table_columns, THRESHOLD)
            .unwrap_or_else(|| {
                panic!("{label}: {original} should be deferrable: {table_columns:?}")
            });

        let full = query(&original);
        let mut result = query(&plan.query);
        let deferred = plan.apply(&mut result, THRESHOLD);

        assert_eq!(result.columns, full.columns, "{label}: columns");
        assert_eq!(
            result.column_types, full.column_types,
            "{label}: column types"
        );
        assert_eq!(
            deferred,
            vec![
                DeferredCell {
                    row: 2,
                    column: 2,
                    size: (long_text.len() * bytes_per_character) as u64
                },
                DeferredCell {
                    row: 2,
                    column: 3,
                    size: long_bytes.len() as u64
                },
            ],
            "{label}: deferred cells"
        );
        // Rows without large values are untouched.
        assert_eq!(result.rows[..2], full.rows[..2], "{label}: small rows");
        assert_eq!(
            result.rows[2][..2],
            full.rows[2][..2],
            "{label}: small cells"
        );
        assert_eq!(
            result.rows[2][2].as_deref(),
            Some(&long_text[..PREVIEW]),
            "{label}: text preview"
        );
        assert_eq!(
            result.rows[2][3],
            Some(encode(&long_bytes[..PREVIEW])),
            "{label}: binary preview"
        );

        let fetched = query(&dialect.deferred_values_query(
            "blanco_deferred_test",
            &[("id".to_string(), ColumnType::Integer)],
            &["body".to_string(), "data".to_string()],
            &[vec!["3".to_string()]],
        ));
        assert_eq!(
            fetched.rows,
            vec![vec![
                Some("3".to_string()),
                Some(long_text.clone()),
                Some(encode(&long_bytes))
            ]],
            "{label}: full values"
        );

        write("DROP TABLE blanco_deferred_test");
    }
}

/// A PostgreSQL connection without a password takes it from the password
/// file the service is pointed at, and is asked for one when the file has no
/// line for it. The file is a temporary one, so neither outcome depends on
/// the `~/.pgpass` of whoever runs the test.
#[test]
fn test_password_file_against_postgres() {
    use database::ConnectionConfig;
    use std::sync::Arc;

    let case = postgres_case();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let directory = tempfile::tempdir().expect("temp dir");
    let password_file = directory.path().join("pgpass");
    let write_password_file = |contents: String| {
        std::fs::write(&password_file, contents).expect("write password file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .expect("chmod");
        }
    };
    let connect = |id: i64| {
        let prompt = Arc::new(ScriptedPrompt {
            answers: std::sync::Mutex::new(vec![None]),
            asked: std::sync::Mutex::new(Vec::new()),
        });
        let mut service = DatabaseService::new(runtime.handle().clone());
        service.set_secret_prompt(prompt.clone());
        service.set_password_file(&password_file);
        let config = ConnectionConfig::new(
            id,
            "from-password-file".to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            case.username.clone(),
            None,
        );
        let database = Some(case.database.as_str());
        let outcome = runtime.block_on(async {
            service.add_connection_config(config.clone()).await;
            let connection = service
                .get_or_create_connection(config.id, database)
                .await?;
            connection.execute_query("SELECT 1", database, None).await
        });
        let asked = prompt.asked.lock().unwrap().len();
        (outcome, asked)
    };

    write_password_file(format!(
        "{}:{}:*:{}:{}\n",
        case.host, case.port, case.username, case.password
    ));
    let (outcome, asked) = connect(9301);
    match outcome {
        Ok(_) => assert_eq!(asked, 0, "the file's password needs no prompt"),
        Err(error) if !strict() && !is_auth_error(&case, &format!("{error:#}")) => {
            eprintln!(
                "skip {}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require.",
                case.label
            );
            return;
        }
        Err(error) => panic!("{}: {error:#}", case.label),
    }

    // No line for this server: nothing is found, so the user is asked.
    write_password_file(format!(
        "elsewhere:{}:*:{}:{}\n",
        case.port, case.username, case.password
    ));
    let (outcome, asked) = connect(9302);
    assert!(outcome.is_err(), "the prompt was dismissed");
    assert_eq!(asked, 1, "a missing password is asked for");
}

/// Where CockroachDB's catalog differs from PostgreSQL's, against the compose
/// node: materialized views are not listed as views too, databases report no
/// size, and a table declared without a primary key is read with its hidden
/// `rowid` so the grid can update a row by it.
#[test]
fn test_cockroachdb_catalog_differences() {
    use crate::results_panel::deferred_values::DeferredRead;
    use crate::results_panel::table_operations::{
        ColumnChange, ColumnValue, OperationType, RowIdentifier, TableChangeOperation,
    };
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let case = cockroach_case();
    let config = ConnectionConfig::new(
        9401,
        case.name.to_string(),
        case.db_type,
        case.host.clone(),
        case.port.parse().expect("port"),
        case.database.clone(),
        case.username.clone(),
        Some(case.password.clone()),
    );
    let database = Some(case.database.as_str());
    let service = DatabaseService::new(runtime.handle().clone());
    let connection = runtime.block_on(async {
        service.add_connection_config(config.clone()).await;
        let connection = service
            .get_or_create_connection(config.id, database)
            .await?;
        connection.ping().await?;
        anyhow::Ok(connection)
    });
    let connection = match connection {
        Ok(connection) => connection,
        Err(error) if strict() => panic!("CockroachDB: server unreachable: {error:#}"),
        Err(error) => {
            eprintln!(
                "skip CockroachDB: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            return;
        }
    };
    let run = |sql: &str| {
        runtime
            .block_on(connection.execute_script(sql, database))
            .unwrap_or_else(|error| panic!("{sql}: {error:#}"))
    };
    run("DROP MATERIALIZED VIEW IF EXISTS blanco_crdb_counts; \
         DROP VIEW IF EXISTS blanco_crdb_names; \
         DROP TABLE IF EXISTS blanco_crdb_keyless");
    run("CREATE TABLE blanco_crdb_keyless (label text, n integer)");
    run("INSERT INTO blanco_crdb_keyless VALUES ('a', 1), ('b', 2)");
    run("CREATE VIEW blanco_crdb_names AS SELECT label FROM blanco_crdb_keyless");
    run("CREATE MATERIALIZED VIEW blanco_crdb_counts AS SELECT count(*) FROM blanco_crdb_keyless");

    let views = runtime
        .block_on(connection.get_views(Some("public")))
        .expect("views");
    assert!(views.contains(&"blanco_crdb_names".to_string()));
    assert!(!views.contains(&"blanco_crdb_counts".to_string()));
    let materialized = runtime
        .block_on(connection.get_materialized_views(Some("public")))
        .expect("materialized views");
    assert!(materialized.contains(&"blanco_crdb_counts".to_string()));
    assert!(
        runtime
            .block_on(connection.get_database_sizes())
            .expect("sizes")
            .is_empty()
    );

    let query = "SELECT * FROM blanco_crdb_keyless";
    let table_columns = runtime
        .block_on(connection.get_columns_for_table("blanco_crdb_keyless", Some("public")))
        .expect("columns");
    assert!(
        table_columns
            .iter()
            .any(|column| column.name == "rowid" && column.is_primary_key),
        "the hidden key is the primary key: {table_columns:?}"
    );
    let read = DeferredRead::plan(case.db_type, query, &table_columns, 0)
        .expect("a keyless table is read with its rowid");
    let mut result = runtime
        .block_on(connection.execute_query(read.query(), database, None))
        .expect("rewritten read");
    read.finish(&mut result);
    let rowid = result
        .columns
        .iter()
        .position(|column| column == "rowid")
        .expect("the result carries rowid");
    let label = result
        .columns
        .iter()
        .position(|column| column == "label")
        .expect("label column");
    let row = result
        .rows
        .iter()
        .find(|row| row[label].as_deref() == Some("a"))
        .expect("row a");

    let update = TableChangeOperation {
        operation_type: OperationType::Update,
        table_name: "blanco_crdb_keyless".to_string(),
        row_identifier: RowIdentifier::PrimaryKey {
            columns: vec![(
                "rowid".to_string(),
                ColumnValue::Literal(row[rowid].clone().expect("rowid value")),
            )],
        },
        changes: vec![ColumnChange {
            column_name: "n".to_string(),
            value: ColumnValue::Literal("10".to_string()),
        }],
    };
    let affected = runtime
        .block_on(connection.execute_write(&update.to_sql_query(case.db_type), database, &[]))
        .expect("update by rowid");
    assert_eq!(affected, 1);
    let updated = run("SELECT n FROM blanco_crdb_keyless WHERE label = 'a'");
    assert_eq!(updated[0].rows[0][0].as_deref(), Some("10"));

    run("DROP MATERIALIZED VIEW blanco_crdb_counts; \
         DROP VIEW blanco_crdb_names; \
         DROP TABLE blanco_crdb_keyless");
}

/// The create statement of a table carries what the CREATE alone leaves
/// out: comments, inheritance, partitioning, storage parameters, triggers
/// and grants. The PostgreSQL DDL is run again in another schema, which must
/// give the same DDL back.
#[test]
fn test_table_ddl_includes_comments_partitions_triggers_and_grants() {
    use blanco_core::ObjectKind;
    use database::ConnectionConfig;

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let service = DatabaseService::new(runtime.handle().clone());
    let connect = |case: &DriverCase, id: i64, username: &str, password: &str| {
        let config = ConnectionConfig::new(
            id,
            case.name.to_string(),
            case.db_type,
            case.host.clone(),
            case.port.parse().expect("port"),
            case.database.clone(),
            username.to_string(),
            Some(password.to_string()),
        );
        let connection = runtime.block_on(async {
            service.add_connection_config(config.clone()).await;
            let connection = service
                .get_or_create_connection(config.id, Some(case.database.as_str()))
                .await?;
            connection.ping().await?;
            anyhow::Ok(connection)
        });
        match connection {
            Ok(connection) => Some(connection),
            Err(error) if strict() => panic!("{}: server unreachable: {error:#}", case.label),
            Err(error) => {
                eprintln!(
                    "skip {}: server unreachable ({error:#}). Set BLANCO_RUN_DB_TESTS=1 to require.",
                    case.label
                );
                None
            }
        }
    };
    let assert_contains = |label: &str, ddl: &str, expected: &str| {
        assert!(
            ddl.contains(expected),
            "{label}: expected {expected:?} in\n{ddl}"
        );
    };
    let assert_lacks = |label: &str, ddl: &str, unexpected: &str| {
        assert!(
            !ddl.contains(unexpected),
            "{label}: did not expect {unexpected:?} in\n{ddl}"
        );
    };

    let postgres = postgres_case();
    if let Some(connection) = connect(&postgres, 9501, &postgres.username, &postgres.password) {
        let database = Some(postgres.database.as_str());
        let run = |sql: &str| {
            runtime
                .block_on(connection.execute_script(sql, database))
                .unwrap_or_else(|error| panic!("{sql}: {error:#}"))
        };
        let table_ddl = |schema: &str, table: &str| {
            runtime
                .block_on(connection.table_ddl(Some(schema), table))
                .unwrap_or_else(|error| panic!("{schema}.{table}: {error:#}"))
        };
        let function_ddl = |schema: &str| {
            runtime
                .block_on(connection.object_ddl(ObjectKind::Function, Some(schema), "touch"))
                .unwrap_or_else(|error| panic!("{schema}.touch: {error:#}"))
        };
        let cleanup = "DROP SCHEMA IF EXISTS blanco_ddl_probe CASCADE; \
             DROP SCHEMA IF EXISTS blanco_ddl_copy CASCADE; \
             DROP ROLE IF EXISTS blanco_ddl_reader";
        run(cleanup);
        run("CREATE ROLE blanco_ddl_reader");
        run("CREATE SCHEMA blanco_ddl_probe");
        run(
            "CREATE FUNCTION blanco_ddl_probe.touch() RETURNS trigger LANGUAGE plpgsql \
             AS $$BEGIN RETURN NEW; END$$",
        );
        run("REVOKE EXECUTE ON FUNCTION blanco_ddl_probe.touch() FROM PUBLIC");
        run("GRANT EXECUTE ON FUNCTION blanco_ddl_probe.touch() TO blanco_ddl_reader");
        run("CREATE TABLE blanco_ddl_probe.base (
                 id integer PRIMARY KEY,
                 label text NOT NULL DEFAULT 'x',
                 CONSTRAINT label_short CHECK (length(label) < 50)
             ) WITH (fillfactor = 70)");
        run("COMMENT ON TABLE blanco_ddl_probe.base IS 'Base''s rows'");
        run("COMMENT ON COLUMN blanco_ddl_probe.base.label IS 'Shown name'");
        run("CREATE INDEX base_label ON blanco_ddl_probe.base (label)");
        run(
            "CREATE TRIGGER base_touch BEFORE UPDATE ON blanco_ddl_probe.base \
             FOR EACH ROW EXECUTE FUNCTION blanco_ddl_probe.touch()",
        );
        run("GRANT SELECT, INSERT ON blanco_ddl_probe.base TO blanco_ddl_reader");
        run("GRANT UPDATE ON blanco_ddl_probe.base TO blanco_ddl_reader WITH GRANT OPTION");
        run(
            "CREATE TABLE blanco_ddl_probe.child (extra integer CHECK (extra > 0)) \
             INHERITS (blanco_ddl_probe.base)",
        );
        run("CREATE TABLE blanco_ddl_probe.measurements (
                 id integer,
                 taken date NOT NULL,
                 PRIMARY KEY (id, taken)
             ) PARTITION BY RANGE (taken)");
        run("CREATE INDEX measurements_taken ON blanco_ddl_probe.measurements (taken)");
        run(
            "CREATE TRIGGER measurements_touch BEFORE UPDATE ON blanco_ddl_probe.measurements \
             FOR EACH ROW EXECUTE FUNCTION blanco_ddl_probe.touch()",
        );
        run("CREATE TABLE blanco_ddl_probe.measurements_2026 \
             PARTITION OF blanco_ddl_probe.measurements \
             FOR VALUES FROM ('2026-01-01') TO ('2027-01-01')");
        run(
            "CREATE VIEW blanco_ddl_probe.base_labels AS SELECT id, label FROM blanco_ddl_probe.base",
        );
        run("COMMENT ON VIEW blanco_ddl_probe.base_labels IS 'Labels only'");

        let label = "PostgreSQL base";
        let base = table_ddl("blanco_ddl_probe", "base");
        assert_contains(label, &base, "WITH (fillfactor=70);");
        assert_contains(
            label,
            &base,
            "COMMENT ON TABLE \"blanco_ddl_probe\".\"base\" IS 'Base''s rows';",
        );
        assert_contains(
            label,
            &base,
            "COMMENT ON COLUMN \"blanco_ddl_probe\".\"base\".\"label\" IS 'Shown name';",
        );
        assert_contains(
            label,
            &base,
            "CREATE INDEX base_label ON blanco_ddl_probe.base",
        );
        assert_contains(
            label,
            &base,
            "CREATE TRIGGER base_touch BEFORE UPDATE ON blanco_ddl_probe.base",
        );
        assert_contains(
            label,
            &base,
            "GRANT INSERT, SELECT ON TABLE \"blanco_ddl_probe\".\"base\" TO blanco_ddl_reader;",
        );
        assert_contains(
            label,
            &base,
            "GRANT UPDATE ON TABLE \"blanco_ddl_probe\".\"base\" TO blanco_ddl_reader WITH GRANT OPTION;",
        );
        assert_lacks(label, &base, &format!("TO {};", postgres.username));

        let label = "PostgreSQL child";
        let child = table_ddl("blanco_ddl_probe", "child");
        assert_contains(label, &child, "INHERITS (blanco_ddl_probe.base);");
        assert_contains(label, &child, "\"extra\" integer");
        assert_lacks(label, &child, "\"label\" text");
        assert_lacks(label, &child, "label_short");

        let label = "PostgreSQL partitioned table";
        let measurements = table_ddl("blanco_ddl_probe", "measurements");
        assert_contains(label, &measurements, "PARTITION BY RANGE (taken);");
        assert_contains(
            label,
            &measurements,
            "CREATE INDEX measurements_taken ON blanco_ddl_probe.measurements",
        );
        assert_contains(label, &measurements, "CREATE TRIGGER measurements_touch");

        let label = "PostgreSQL partition";
        let partition = table_ddl("blanco_ddl_probe", "measurements_2026");
        assert_contains(
            label,
            &partition,
            "CREATE TABLE \"blanco_ddl_probe\".\"measurements_2026\" \
             PARTITION OF blanco_ddl_probe.measurements\n\
             FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');",
        );
        assert_lacks(label, &partition, "\"taken\" date");
        assert_lacks(label, &partition, "CREATE INDEX");
        assert_lacks(label, &partition, "CREATE TRIGGER");

        let label = "PostgreSQL view";
        let view = table_ddl("blanco_ddl_probe", "base_labels");
        assert_contains(
            label,
            &view,
            "CREATE VIEW \"blanco_ddl_probe\".\"base_labels\" AS\n",
        );
        assert_contains(
            label,
            &view,
            "COMMENT ON VIEW \"blanco_ddl_probe\".\"base_labels\" IS 'Labels only';",
        );

        let label = "PostgreSQL function";
        let function = function_ddl("blanco_ddl_probe");
        assert_contains(
            label,
            &function,
            "REVOKE EXECUTE ON FUNCTION blanco_ddl_probe.touch() FROM PUBLIC;",
        );
        assert_contains(
            label,
            &function,
            "GRANT EXECUTE ON FUNCTION blanco_ddl_probe.touch() TO blanco_ddl_reader;",
        );

        // Recreating everything in another schema must give the same DDL.
        let in_copy = |ddl: &str| ddl.replace("blanco_ddl_probe", "blanco_ddl_copy");
        run("CREATE SCHEMA blanco_ddl_copy");
        run(&in_copy(&function));
        for ddl in [&base, &child, &measurements, &partition, &view] {
            run(&in_copy(ddl));
        }
        assert_eq!(function_ddl("blanco_ddl_copy"), in_copy(&function));
        for (table, ddl) in [
            ("base", &base),
            ("child", &child),
            ("measurements", &measurements),
            ("measurements_2026", &partition),
            ("base_labels", &view),
        ] {
            assert_eq!(table_ddl("blanco_ddl_copy", table), in_copy(ddl), "{table}");
        }
        run(cleanup);
    }

    let cockroach = cockroach_case();
    if let Some(connection) = connect(&cockroach, 9502, &cockroach.username, &cockroach.password) {
        let database = Some(cockroach.database.as_str());
        let run = |sql: &str| {
            runtime
                .block_on(connection.execute_script(sql, database))
                .unwrap_or_else(|error| panic!("{sql}: {error:#}"))
        };
        let table_ddl = |table: &str| {
            runtime
                .block_on(connection.table_ddl(Some("public"), table))
                .unwrap_or_else(|error| panic!("CockroachDB {table}: {error:#}"))
        };
        let cleanup = "DROP VIEW IF EXISTS blanco_ddl_labels; \
             DROP TABLE IF EXISTS blanco_ddl_base; \
             DROP FUNCTION IF EXISTS blanco_ddl_touch; \
             DROP ROLE IF EXISTS blanco_ddl_reader";
        run(cleanup);
        run("CREATE ROLE blanco_ddl_reader");
        run(
            "CREATE FUNCTION blanco_ddl_touch() RETURNS trigger LANGUAGE plpgsql \
             AS $$BEGIN RETURN NEW; END$$",
        );
        run("CREATE TABLE blanco_ddl_base (id integer PRIMARY KEY, label text NOT NULL)");
        run("COMMENT ON TABLE blanco_ddl_base IS 'Base rows'");
        run("COMMENT ON COLUMN blanco_ddl_base.label IS 'Shown name'");
        run(
            "CREATE TRIGGER blanco_ddl_base_touch BEFORE UPDATE ON blanco_ddl_base \
             FOR EACH ROW EXECUTE FUNCTION blanco_ddl_touch()",
        );
        run("GRANT SELECT, INSERT ON blanco_ddl_base TO blanco_ddl_reader");
        run("CREATE VIEW blanco_ddl_labels AS SELECT label FROM blanco_ddl_base");

        let label = "CockroachDB table";
        let base = table_ddl("blanco_ddl_base");
        assert_contains(
            label,
            &base,
            "COMMENT ON TABLE \"public\".\"blanco_ddl_base\" IS 'Base rows';",
        );
        assert_contains(
            label,
            &base,
            "COMMENT ON COLUMN \"public\".\"blanco_ddl_base\".\"label\" IS 'Shown name';",
        );
        assert_contains(label, &base, "CREATE TRIGGER blanco_ddl_base_touch");
        assert_contains(
            label,
            &base,
            "GRANT INSERT, SELECT ON TABLE \"public\".\"blanco_ddl_base\" TO blanco_ddl_reader;",
        );
        assert_lacks(label, &base, "TO admin");
        assert_lacks(label, &base, "_not_null");
        assert_lacks(label, &base, "TO root");
        let view = table_ddl("blanco_ddl_labels");
        assert_contains(
            "CockroachDB view",
            &view,
            "CREATE VIEW \"public\".\"blanco_ddl_labels\" AS\n",
        );
        run(cleanup);
    }

    // Granting takes an account with GRANT OPTION, which the compose user
    // lacks, so the grants are made as root. The compose user may not read
    // other accounts' grants, and still gets the CREATE statement.
    let mysql = mysql_case();
    let root_user = env_or("MYSQL", "ROOT_USER", "root");
    let root_password = env_or("MYSQL", "ROOT_PASSWORD", "blanco");
    if let Some(connection) = connect(&mysql, 9503, &root_user, &root_password) {
        let database = Some(mysql.database.as_str());
        let run = |sql: &str| {
            runtime
                .block_on(connection.execute_script(sql, database))
                .unwrap_or_else(|error| panic!("{sql}: {error:#}"))
        };
        let cleanup = "DROP TABLE IF EXISTS blanco_ddl_grants; \
             DROP USER IF EXISTS 'blanco_ddl_reader'@'%'; \
             DROP ROLE IF EXISTS blanco_ddl_role";
        run(cleanup);
        run("CREATE TABLE blanco_ddl_grants (id integer PRIMARY KEY)");
        run("CREATE USER 'blanco_ddl_reader'@'%'");
        run("CREATE ROLE blanco_ddl_role");
        run(
            "GRANT SELECT, INSERT ON blanco_ddl_grants TO 'blanco_ddl_reader'@'%' \
             WITH GRANT OPTION",
        );
        run("GRANT UPDATE ON blanco_ddl_grants TO blanco_ddl_role");

        let label = "MySQL";
        let ddl = runtime
            .block_on(connection.table_ddl(None, "blanco_ddl_grants"))
            .unwrap_or_else(|error| panic!("{label}: {error:#}"));
        let object = format!("`{}`.`blanco_ddl_grants`", mysql.database);
        assert!(ddl.starts_with("CREATE TABLE"), "{label}: {ddl}");
        assert_contains(
            label,
            &ddl,
            &format!(
                "GRANT INSERT, SELECT ON {object} TO 'blanco_ddl_reader'@'%' WITH GRANT OPTION;"
            ),
        );
        assert_contains(
            label,
            &ddl,
            &format!("GRANT UPDATE ON {object} TO 'blanco_ddl_role';"),
        );

        if let Some(unprivileged) = connect(&mysql, 9504, &mysql.username, &mysql.password) {
            let ddl = runtime
                .block_on(unprivileged.table_ddl(None, "blanco_ddl_grants"))
                .unwrap_or_else(|error| panic!("{label} as {}: {error:#}", mysql.username));
            assert!(ddl.starts_with("CREATE TABLE"), "{label}: {ddl}");
        }
        run(cleanup);
    }

    let clickhouse = clickhouse_case();
    if let Some(connection) = connect(
        &clickhouse,
        9505,
        &clickhouse.username,
        &clickhouse.password,
    ) {
        let database = Some(clickhouse.database.as_str());
        let run = |sql: &str| runtime.block_on(connection.execute_script(sql, database));
        let label = "ClickHouse";
        run("DROP TABLE IF EXISTS blanco_ddl_grants")
            .unwrap_or_else(|error| panic!("{label}: {error:#}"));
        run("CREATE TABLE blanco_ddl_grants (id Int32) ENGINE = MergeTree ORDER BY id")
            .unwrap_or_else(|error| panic!("{label}: {error:#}"));
        // The compose user has no access management, so the grant is
        // checked only where roles can be created.
        let granted = match run("CREATE ROLE IF NOT EXISTS blanco_ddl_reader") {
            Ok(_) => {
                run("GRANT SELECT, INSERT ON blanco_ddl_grants TO blanco_ddl_reader")
                    .unwrap_or_else(|error| panic!("{label}: {error:#}"));
                true
            }
            Err(error) => {
                eprintln!("{label}: grants not checked, roles cannot be created ({error:#})");
                false
            }
        };
        let ddl = runtime
            .block_on(connection.table_ddl(None, "blanco_ddl_grants"))
            .unwrap_or_else(|error| panic!("{label}: {error:#}"));
        assert!(ddl.starts_with("CREATE TABLE"), "{label}: {ddl}");
        if granted {
            assert_contains(
                label,
                &ddl,
                &format!(
                    "GRANT INSERT, SELECT ON `{}`.`blanco_ddl_grants` TO `blanco_ddl_reader`;",
                    clickhouse.database
                ),
            );
            run("DROP ROLE IF EXISTS blanco_ddl_reader")
                .unwrap_or_else(|error| panic!("{label}: {error:#}"));
        } else {
            assert_lacks(label, &ddl, "GRANT");
        }
        run("DROP TABLE IF EXISTS blanco_ddl_grants")
            .unwrap_or_else(|error| panic!("{label}: {error:#}"));
    }
}
