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

use std::env;

use database::{DatabaseService, DatabaseType};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

use crate::app_database::AppDatabase;
use crate::connection_modal::NewConnectionModal;
use crate::editor::{EditorPanel, TabCreationParams};
use crate::test_harness::FullAppHarness;

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
        cx.new(|cx| NewConnectionModal::new(window, cx))
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
            connection_id,
            db_type: case.db_type,
            connection_name: Some(case.name.into()),
            database_name: case.database.clone(),
            schema_name: case.schema_name.map(|s| s.to_string()),
            environment_type: None,
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

#[gpui::test]
async fn test_sqlite_connect_and_query(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.run_until_parked();

    // SQLite has no password, just exercise create -> test -> save -> query
    // against a fresh temp file.
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let db_path = temp_dir.path().join("integration.db");
    let db_path = db_path.to_string_lossy().to_string();

    let modal = harness.app.update_in(&mut cx, |_app, window, cx| {
        cx.new(|cx| NewConnectionModal::new(window, cx))
    });
    modal.update_in(&mut cx, |modal, window, cx| {
        modal.set_name("it-sqlite", window, cx);
        modal.select_db_type("SQLite", window, cx);
        modal.set_sqlite_path(&db_path, window, cx);
    });

    let result = run_test_connection(&modal, &mut cx).await;
    assert!(
        result.is_ok(),
        "sqlite test connection failed: {:?}",
        result
    );

    let mut data = modal
        .read_with(&cx, |modal, cx| modal.get_connection_data(cx))
        .expect("sqlite connection data");
    data.name = "it-sqlite".to_string();

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
            title: "it-sqlite".into(),
            content: None,
            db_id: Some(connection_id),
            last_run_at: None,
            connection_id,
            db_type: DatabaseType::SQLite,
            connection_name: Some("it-sqlite".into()),
            database_name: "main".into(),
            schema_name: None,
            environment_type: None,
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
