//! HTTP-level tests: the bearer gate and a real rmcp client talking to the
//! server over the streamable HTTP transport, with the foreground bridge
//! answered by a test task.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use app_database::AppDatabase;
use axum::http::{HeaderName, HeaderValue};
use database::{ConnectionConfig, DatabaseService};
use rmcp::ServiceExt as _;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use tokio_util::sync::CancellationToken;

use super::bridge::{McpBridgeClient, McpRequest, TabSummary, WriteAnswer, send_reply};
use super::server::{AGENT_TAB_HEADER, BlancoMcpServer, bind, serve};

struct TestServer {
    port: u16,
    token: String,
    /// `rememberable_kinds` of every write confirmation the fake foreground
    /// was asked for, in order. It answers each with `RunAndRemember`.
    confirmations: Arc<Mutex<Vec<Vec<String>>>>,
    /// The tab id every foreground request named, in order.
    requested_tabs: Arc<Mutex<Vec<Option<u64>>>>,
    cancellation: CancellationToken,
    _bridge_task: tokio::task::JoinHandle<()>,
    _serve_task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

/// Start a server on a free port, backed by an in-memory app database holding
/// one SQLite connection, with a fake foreground that answers `ListTabs`.
async fn start_test_server() -> TestServer {
    let confirmations = Arc::new(Mutex::new(Vec::new()));
    let requested_tabs = Arc::new(Mutex::new(Vec::new()));
    let runtime_handle = tokio::runtime::Handle::current();
    let app_database = AppDatabase::new_in_memory(runtime_handle.clone())
        .await
        .expect("in-memory app database");
    let db_service = DatabaseService::new(runtime_handle);
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("test.db");
    std::mem::forget(temp_dir);
    db_service
        .add_connection_config(ConnectionConfig::new_sqlite(
            1,
            "test".into(),
            db_path.to_string_lossy().to_string(),
        ))
        .await;

    let (client, receiver) = McpBridgeClient::channel();
    let bridge_task = tokio::spawn({
        let confirmations = confirmations.clone();
        let requested_tabs = requested_tabs.clone();
        async move {
            while let Ok(request) = receiver.recv().await {
                requested_tabs
                    .lock()
                    .expect("requested tabs lock")
                    .push(request.tab_id());
                match request {
                    McpRequest::ListTabs { reply } => {
                        send_reply(&reply, Ok(vec![fake_tab_summary()]))
                    }
                    McpRequest::CreateQueryTab { reply, .. }
                    | McpRequest::SetActiveTab { reply, .. } => {
                        send_reply(&reply, Ok(fake_tab_summary()))
                    }
                    McpRequest::PrepareRunTab { reply, .. } => send_reply(
                        &reply,
                        Ok(super::bridge::TabRun {
                            tab_id: 42,
                            connection_id: 1,
                            database: "main".to_string(),
                            sql: "CREATE TABLE from_tab (id INTEGER)".to_string(),
                        }),
                    ),
                    McpRequest::RunTab { reply, .. } => {
                        send_reply(&reply, Ok(serde_json::json!({ "rows": [] })))
                    }
                    McpRequest::ReadTab { reply, .. } => {
                        send_reply(&reply, Err("no tabs in this test".to_string()))
                    }
                    McpRequest::TabConnection { reply, .. } => send_reply(
                        &reply,
                        Ok(super::bridge::TabConnection {
                            connection_id: 1,
                            connection_name: "test".to_string(),
                            db_type: "SQLite".to_string(),
                            database: "main".to_string(),
                            schema: None,
                            environment: None,
                        }),
                    ),
                    McpRequest::ConfirmWrite {
                        confirmation,
                        reply,
                    } => {
                        confirmations
                            .lock()
                            .expect("confirmations lock")
                            .push(confirmation.rememberable_kinds);
                        reply
                            .try_send(WriteAnswer::RunAndRemember)
                            .expect("answer the confirmation");
                    }
                    _ => {}
                }
            }
        }
    });

    let token = "test-token-0123456789".to_string();
    let listener = bind(0).expect("bind a free port");
    let port = listener.local_addr().expect("local addr").port();
    let server = BlancoMcpServer::new(
        client,
        Arc::new(db_service),
        app_database,
        Arc::new(AtomicBool::new(false)),
    );
    let cancellation = CancellationToken::new();
    let serve_task = tokio::spawn(serve(listener, token.clone(), server, cancellation.clone()));

    TestServer {
        port,
        token,
        confirmations,
        requested_tabs,
        cancellation,
        _bridge_task: bridge_task,
        _serve_task: serve_task,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_requests_without_the_token() {
    let server = start_test_server().await;
    let url = format!("http://127.0.0.1:{}/mcp", server.port);
    let http = reqwest::Client::new();

    let missing = http
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await
        .expect("request without token");
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer")
    );

    let wrong = http
        .post(&url)
        .header("Authorization", "Bearer not-the-token")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await
        .expect("request with wrong token");
    assert_eq!(wrong.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_tools_and_calls_them_over_http() {
    let server = start_test_server().await;
    let url = format!("http://127.0.0.1:{}/mcp", server.port);

    // rmcp's `auth_header` is sent as `Authorization: Bearer <value>`.
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url).auth_header(server.token.clone()),
    );
    let client = ().serve(transport).await.expect("initialize against the server");

    let tools = client.list_all_tools().await.expect("list tools");
    let mut names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    names.sort_unstable();
    for expected in [
        "create_query_tab",
        "describe_table",
        "explain_query",
        "explore_schema",
        "get_table_ddl",
        "list_connections",
        "list_schemas",
        "list_tables",
        "read_tab",
        "run_sql",
        "run_tab",
        "set_active_tab",
        "write_tab",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected} in {names:?}"
        );
    }

    let tabs = client
        .call_tool(CallToolRequestParams::new("list_tabs"))
        .await
        .expect("call list_tabs");
    assert_ne!(tabs.is_error, Some(true));
    let text = tabs
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .expect("text content");
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("json tabs");
    assert_eq!(parsed[0]["title"], "Fake tab");
    assert_eq!(parsed[0]["id"], 42);

    let result = client
        .call_tool(CallToolRequestParams::new("run_sql").with_arguments(
            serde_json::Map::from_iter([
                ("connection_id".to_string(), serde_json::json!(1)),
                (
                    "sql".to_string(),
                    serde_json::json!("SELECT 1 AS one, 'two' AS two"),
                ),
            ]),
        ))
        .await
        .expect("call run_sql");
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .expect("text content");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("json result");
    assert_eq!(parsed["columns"], serde_json::json!(["one", "two"]));
    assert_eq!(parsed["rows"], serde_json::json!([["1", "two"]]));

    let from_active_tab = client
        .call_tool(CallToolRequestParams::new("run_sql").with_arguments(
            serde_json::Map::from_iter([(
                "sql".to_string(),
                serde_json::json!("SELECT 7 AS seven"),
            )]),
        ))
        .await
        .expect("call run_sql without connection_id");
    assert_ne!(from_active_tab.is_error, Some(true), "{from_active_tab:?}");
    let text = from_active_tab
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .expect("text content");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("json result");
    assert_eq!(parsed["rows"], serde_json::json!([["7"]]));

    let missing_connection = client
        .call_tool(CallToolRequestParams::new("list_schemas").with_arguments(
            serde_json::Map::from_iter([("connection_id".to_string(), serde_json::json!(999))]),
        ))
        .await
        .expect("call list_schemas");
    assert_eq!(missing_connection.is_error, Some(true));

    client.cancel().await.expect("close client");
}

fn fake_tab_summary() -> TabSummary {
    TabSummary {
        index: 0,
        id: Some(42),
        title: "Fake tab".to_string(),
        kind: "query",
        active: true,
        connection: None,
    }
}

async fn connect(server: &TestServer) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    connect_with_headers(server, HashMap::new()).await
}

async fn connect_with_headers(
    server: &TestServer,
    headers: HashMap<HeaderName, HeaderValue>,
) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let url = format!("http://127.0.0.1:{}/mcp", server.port);
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url)
            .auth_header(server.token.clone())
            .custom_headers(headers),
    );
    ().serve(transport)
        .await
        .expect("initialize against the server")
}

async fn run_sql(client: &rmcp::service::RunningService<rmcp::RoleClient, ()>, sql: &str) {
    let result = client
        .call_tool(CallToolRequestParams::new("run_sql").with_arguments(
            serde_json::Map::from_iter([
                ("connection_id".to_string(), serde_json::json!(1)),
                ("sql".to_string(), serde_json::json!(sql)),
            ]),
        ))
        .await
        .expect("call run_sql");
    assert_ne!(result.is_error, Some(true), "{sql}: {result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remembered_write_kinds_skip_the_dialog_for_that_session_only() {
    let server = start_test_server().await;
    let asked = || server.confirmations.lock().expect("lock").clone();

    let first = connect(&server).await;
    run_sql(&first, "CREATE TABLE items (id INTEGER)").await;
    run_sql(&first, "INSERT INTO items VALUES (1)").await;
    run_sql(&first, "INSERT INTO items VALUES (2)").await;
    assert_eq!(asked(), [vec!["CREATE"], vec!["INSERT"]]);

    // A mix still asks for the kind that was never granted.
    run_sql(
        &first,
        "INSERT INTO items VALUES (3); UPDATE items SET id = 4",
    )
    .await;
    assert_eq!(asked().len(), 3);
    assert_eq!(asked()[2], ["INSERT", "UPDATE"]);

    // Destructive DDL is never rememberable, even once asked.
    run_sql(&first, "DROP TABLE items").await;
    assert_eq!(asked()[3], Vec::<String>::new());

    let second = connect(&server).await;
    run_sql(&second, "CREATE TABLE other (id INTEGER)").await;
    assert_eq!(asked().len(), 5, "a new session starts without grants");

    first.cancel().await.expect("close first client");
    second.cancel().await.expect("close second client");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_in_a_terminal_pane_stays_in_its_tab() {
    let server = start_test_server().await;
    let call = |name: &'static str, arguments: serde_json::Value| {
        let arguments = match arguments {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        CallToolRequestParams::new(name).with_arguments(arguments)
    };

    let outside = connect(&server).await;
    let created = outside
        .call_tool(call(
            "create_query_tab",
            serde_json::json!({ "connection_id": 1 }),
        ))
        .await
        .expect("call create_query_tab");
    assert_ne!(created.is_error, Some(true), "{created:?}");

    let in_pane = connect_with_headers(
        &server,
        HashMap::from([(
            HeaderName::from_static(AGENT_TAB_HEADER),
            HeaderValue::from_static("42"),
        )]),
    )
    .await;
    let refused = in_pane
        .call_tool(call(
            "create_query_tab",
            serde_json::json!({ "connection_id": 1 }),
        ))
        .await
        .expect("call create_query_tab");
    assert_eq!(refused.is_error, Some(true));
    let switched_away = in_pane
        .call_tool(call(
            "set_active_tab",
            serde_json::json!({ "tab": { "id": 7 } }),
        ))
        .await
        .expect("call set_active_tab");
    assert_eq!(switched_away.is_error, Some(true));
    let own_tab = in_pane
        .call_tool(call(
            "set_active_tab",
            serde_json::json!({ "tab": { "id": 42 } }),
        ))
        .await
        .expect("call set_active_tab");
    assert_ne!(own_tab.is_error, Some(true), "{own_tab:?}");

    // Omitted tabs mean the agent's own, wherever the user has moved to.
    server.requested_tabs.lock().expect("lock").clear();
    in_pane
        .call_tool(call("read_tab", serde_json::json!({})))
        .await
        .expect("call read_tab");
    in_pane
        .call_tool(call("run_sql", serde_json::json!({ "sql": "SELECT 1" })))
        .await
        .expect("call run_sql");
    let unnamed = in_pane
        .call_tool(call("set_active_tab", serde_json::json!({})))
        .await
        .expect("call set_active_tab");
    assert_ne!(unnamed.is_error, Some(true), "{unnamed:?}");
    outside
        .call_tool(call("read_tab", serde_json::json!({})))
        .await
        .expect("call read_tab");
    assert_eq!(
        *server.requested_tabs.lock().expect("lock"),
        [Some(42), Some(42), Some(42), None]
    );

    // Outside a pane `${BLANCO_TAB_ID:-}` expands to nothing.
    let empty_header = connect_with_headers(
        &server,
        HashMap::from([(
            HeaderName::from_static(AGENT_TAB_HEADER),
            HeaderValue::from_static(""),
        )]),
    )
    .await;
    let created = empty_header
        .call_tool(call(
            "create_query_tab",
            serde_json::json!({ "connection_id": 1 }),
        ))
        .await
        .expect("call create_query_tab");
    assert_ne!(created.is_error, Some(true), "{created:?}");

    outside.cancel().await.expect("close client");
    in_pane.cancel().await.expect("close client");
    empty_header.cancel().await.expect("close client");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_tab_writes_go_through_the_write_policy() {
    let server = start_test_server().await;
    let client = connect(&server).await;
    let result = client
        .call_tool(CallToolRequestParams::new("run_tab"))
        .await
        .expect("call run_tab");
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(
        *server.confirmations.lock().expect("lock"),
        [vec!["CREATE".to_string()]]
    );
    client.cancel().await.expect("close client");
}
