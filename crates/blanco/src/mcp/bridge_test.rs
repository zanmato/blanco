//! End-to-end tests of the foreground bridge against a real `BlancoApp`: the
//! requests an MCP tool would send, answered by the actual editor panel, with
//! `run_tab` executing against the harness's SQLite file.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use blanco_core::ConnectionContext;
use gpui::{TestAppContext, VisualTestContext};

use super::McpService;
use super::bridge::{McpBridgeClient, McpRequest, TabSelector, TabSummary};
use crate::editor::tab_access::WriteOperation;
use crate::test_harness::FullAppHarness;

/// Poll `future` to completion while pumping the GPUI executor, so the
/// foreground listener gets to answer. Sleeps between rounds because tokio
/// (database IO) runs on threads the test executor cannot see.
fn drive<T: Send + 'static>(
    cx: &mut VisualTestContext,
    future: impl Future<Output = T> + Send + 'static,
) -> T {
    let task = cx.executor().spawn(future);
    let mut task = pin!(task);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    for _ in 0..4000 {
        cx.run_until_parked();
        if let Poll::Ready(value) = task.as_mut().poll(&mut context) {
            return value;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("bridge request did not finish in time");
}

fn list_tabs(client: &McpBridgeClient, cx: &mut VisualTestContext) -> Vec<TabSummary> {
    let client = client.clone();
    drive(cx, async move {
        client.request(|reply| McpRequest::ListTabs { reply }).await
    })
    .expect("list_tabs")
}

fn read_tab(
    client: &McpBridgeClient,
    tab: TabSelector,
    cx: &mut VisualTestContext,
) -> Result<super::bridge::TabContent, String> {
    let client = client.clone();
    drive(cx, async move {
        client
            .request(|reply| McpRequest::ReadTab {
                tab,
                start_line: None,
                end_line: None,
                reply,
            })
            .await
    })
}

fn write_tab(
    client: &McpBridgeClient,
    tab: TabSelector,
    operation: WriteOperation,
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
    cx: &mut VisualTestContext,
) -> Result<super::bridge::WriteOutcome, String> {
    let client = client.clone();
    let content = content.to_string();
    drive(cx, async move {
        client
            .request(|reply| McpRequest::WriteTab {
                tab,
                edit: super::bridge::TabEdit {
                    operation,
                    content,
                    start_line,
                    end_line,
                },
                reply,
            })
            .await
    })
}

fn run_tab(
    client: &McpBridgeClient,
    tab: TabSelector,
    cx: &mut VisualTestContext,
) -> Result<serde_json::Value, String> {
    let client = client.clone();
    drive(cx, async move {
        client
            .request(|reply| McpRequest::RunTab { tab, reply })
            .await
    })
}

fn by_id(id: u64) -> TabSelector {
    TabSelector {
        index: None,
        id: Some(id),
    }
}

#[gpui::test]
async fn bridge_reads_writes_creates_and_runs_tabs(cx: &mut TestAppContext) {
    let harness = FullAppHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let client = cx.update(|_, cx| McpService::global(cx).client());

    let tabs = list_tabs(&client, &mut cx);
    assert_eq!(tabs.len(), 1);
    let first = &tabs[0];
    assert_eq!(first.title, "Test Query");
    assert_eq!(first.kind, "query");
    assert!(first.active);
    let first_id = first.id.expect("query tabs have ids");
    let connection = first
        .connection
        .clone()
        .expect("query tab has a connection");
    assert_eq!(connection.db_type, "SQLite");

    let outcome = write_tab(
        &client,
        TabSelector::default(),
        WriteOperation::ReplaceAll,
        "SELECT 1 AS one;\nSELECT 2 AS two;",
        None,
        None,
        &mut cx,
    )
    .expect("replace_all");
    assert_eq!(outcome.total_lines, 2);

    let content = read_tab(&client, TabSelector::default(), &mut cx).expect("read_tab");
    assert_eq!(content.content, "1: SELECT 1 AS one;\n2: SELECT 2 AS two;");
    assert_eq!(content.total_lines, 2);
    assert!(!content.truncated);

    let outcome = write_tab(
        &client,
        by_id(first_id),
        WriteOperation::ReplaceLines,
        "SELECT 3 AS three;",
        Some(2),
        Some(2),
        &mut cx,
    )
    .expect("replace_lines");
    assert_eq!(outcome.message, "Replaced lines 2-2");
    let content = read_tab(&client, by_id(first_id), &mut cx).expect("read after replace");
    assert_eq!(
        content.content,
        "1: SELECT 1 AS one;\n2: SELECT 3 AS three;"
    );

    let active_connection = {
        let client = client.clone();
        drive(&mut cx, async move {
            client
                .request(|reply| McpRequest::TabConnection {
                    tab: TabSelector::default(),
                    reply,
                })
                .await
        })
        .expect("active tab has a connection")
    };
    assert_eq!(active_connection.connection_id, connection.connection_id);
    assert_eq!(active_connection.database, "main");

    let missing = read_tab(&client, by_id(999_999), &mut cx).expect_err("unknown id fails");
    assert!(missing.contains("No tab with id"), "{missing}");

    let context = ConnectionContext {
        connection_id: connection.connection_id,
        connection_name: connection.connection_name.clone(),
        db_type: blanco_core::DatabaseType::SQLite,
        database_name: connection.database,
        schema_name: None,
        environment_type: None,
    };
    let created = {
        let client = client.clone();
        drive(&mut cx, async move {
            client
                .request(|reply| McpRequest::CreateQueryTab {
                    context,
                    content: Some("SELECT 42 AS answer".to_string()),
                    title: Some("Answer".to_string()),
                    reply,
                })
                .await
        })
        .expect("create_query_tab")
    };
    assert_eq!(created.index, 1);
    assert_eq!(created.title, "Answer");
    assert!(created.active);
    let created_id = created.id.expect("new tab has an id");
    assert_ne!(created_id, first_id);

    let activated = {
        let client = client.clone();
        drive(&mut cx, async move {
            client
                .request(|reply| McpRequest::SetActiveTab {
                    tab: TabSelector {
                        index: Some(0),
                        id: None,
                    },
                    reply,
                })
                .await
        })
        .expect("set_active_tab")
    };
    assert_eq!(activated.index, 0);
    assert!(activated.active);

    let result = run_tab(&client, by_id(created_id), &mut cx).expect("run_tab succeeds");
    assert_eq!(result["columns"], serde_json::json!(["answer"]));
    assert_eq!(result["rows"], serde_json::json!([["42"]]));
    assert_eq!(result["truncated"], false);
    let tabs = list_tabs(&client, &mut cx);
    assert!(tabs[1].active, "run_tab activates the tab it runs");

    write_tab(
        &client,
        by_id(created_id),
        WriteOperation::ReplaceAll,
        "SELECT * FROM table_that_does_not_exist",
        None,
        None,
        &mut cx,
    )
    .expect("replace_all");
    let failure = run_tab(&client, by_id(created_id), &mut cx).expect_err("bad query fails");
    assert!(failure.contains("table_that_does_not_exist"), "{failure}");

    write_tab(
        &client,
        by_id(created_id),
        WriteOperation::ReplaceAll,
        "",
        None,
        None,
        &mut cx,
    )
    .expect("clear tab");
    let empty = run_tab(&client, by_id(created_id), &mut cx).expect_err("empty tab fails");
    assert!(empty.contains("No query to execute"), "{empty}");
}
