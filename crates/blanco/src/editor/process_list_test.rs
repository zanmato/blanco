//! The process list tab against the compose PostgreSQL server. Skipped when
//! it is down, unless `BLANCO_RUN_DB_TESTS=1`.

use blanco_core::{ConnectionContext, DatabaseService as _};
use database::{ConnectionConfig, DatabaseType};
use gpui::{TestAppContext, VisualTestContext};
use gpui_component::{WindowExt as _, table::TableSelection};

use super::{ProcessListTab, SessionAction};
use crate::app::CopySelection;
use crate::test_harness::TestHarness;

const MARKER: &str = "blanco_process_list_tab_test";
const CONFIG_ID: i64 = 9301;

fn postgres_config(read_only: bool) -> ConnectionConfig {
    ConnectionConfig::new(
        CONFIG_ID,
        "it-process-list".to_string(),
        DatabaseType::PostgreSQL,
        "localhost".to_string(),
        5488,
        "blanco".to_string(),
        "blanco".to_string(),
        Some("blanco".to_string()),
    )
    .with_read_only(read_only)
}

async fn wait_for_refresh(tab: &gpui::Entity<ProcessListTab>, cx: &mut VisualTestContext) {
    for _ in 0..200 {
        cx.run_until_parked();
        if tab.read_with(cx, |tab, _| !tab.loading) {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("the process list never finished refreshing");
}

#[gpui::test]
async fn process_list_tab_lists_copies_and_guards_read_only(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    let (service, handle) = cx.update(|_, cx| {
        (
            database::DatabaseService::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let reachable = handle.block_on(async {
        service.add_connection_config(postgres_config(false)).await;
        service
            .execute_script(CONFIG_ID, Some("blanco"), "SELECT 1")
            .await
    });
    if let Err(error) = reachable {
        assert!(
            std::env::var("BLANCO_RUN_DB_TESTS").as_deref() != Ok("1"),
            "PostgreSQL unreachable: {error:#}"
        );
        return;
    }
    let sleeper = handle.spawn({
        let service = service.clone();
        async move {
            service
                .execute_script(
                    CONFIG_ID,
                    Some("blanco"),
                    &format!("SELECT pg_sleep(30) AS {MARKER}"),
                )
                .await
        }
    });

    let context = ConnectionContext {
        connection_id: CONFIG_ID,
        db_type: DatabaseType::PostgreSQL,
        connection_name: "it-process-list".into(),
        database_name: "blanco".into(),
        schema_name: None,
        environment_type: None,
    };
    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_process_list_tab(context, window, cx)
        });
    let mut marked_row = None;
    for _ in 0..25 {
        tab.update_in(&mut cx, |tab, window, cx| tab.refresh(window, cx));
        wait_for_refresh(&tab, &mut cx).await;
        marked_row = tab.read_with(&cx, |tab, _| {
            assert_eq!(tab.error, None);
            tab.sessions.iter().position(|session| {
                session
                    .query
                    .as_deref()
                    .is_some_and(|query| query.contains(MARKER))
            })
        });
        if marked_row.is_some() {
            break;
        }
    }
    let row = marked_row.expect("the sleeping session should be listed");

    // The copy shortcut copies the selected row as TSV with its header.
    let table_state = tab.read_with(&cx, |tab, _| tab.table_state.clone());
    table_state.update(&mut cx, |table, cx| {
        table.set_selection(TableSelection::Row(row), cx)
    });
    tab.update_in(&mut cx, |tab, window, cx| {
        tab.copy_selection(&CopySelection, window, cx)
    });
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(copied.starts_with("Session\tUser\tDatabase"), "{copied:?}");
    assert!(copied.contains(MARKER), "{copied:?}");

    // A read-only connection asks nothing and kills nothing.
    handle.block_on(service.add_connection_config(postgres_config(true)));
    tab.update_in(&mut cx, |tab, window, cx| tab.refresh(window, cx));
    wait_for_refresh(&tab, &mut cx).await;
    assert!(tab.read_with(&cx, |tab, _| tab.read_only));
    tab.update_in(&mut cx, |tab, window, cx| {
        tab.confirm_action(row, SessionAction::KillSession, window, cx)
    });
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));

    // A writable one asks first.
    handle.block_on(service.add_connection_config(postgres_config(false)));
    tab.update_in(&mut cx, |tab, window, cx| tab.refresh(window, cx));
    wait_for_refresh(&tab, &mut cx).await;
    let row = tab
        .read_with(&cx, |tab, _| {
            tab.sessions.iter().position(|session| {
                session
                    .query
                    .as_deref()
                    .is_some_and(|query| query.contains(MARKER))
            })
        })
        .expect("still sleeping");
    tab.update_in(&mut cx, |tab, window, cx| {
        tab.confirm_action(row, SessionAction::KillSession, window, cx)
    });
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.update(|window, cx| window.close_dialog(cx));

    // End the sleep rather than leave it running.
    let session_id = tab.read_with(&cx, |tab, _| tab.sessions[row].id.clone());
    let statement = DatabaseType::PostgreSQL
        .dialect()
        .kill_session_statement(&session_id)
        .expect("statement");
    handle
        .block_on(async {
            let connection = service
                .open_dedicated_connection(CONFIG_ID, Some("blanco"))
                .await?;
            connection
                .execute_query(&statement, Some("blanco"), None)
                .await
        })
        .expect("kill the sleep");
    assert!(
        handle.block_on(sleeper).expect("sleeper task").is_err(),
        "the killed sleep should fail"
    );
}
