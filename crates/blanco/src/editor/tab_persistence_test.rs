use gpui::{TestAppContext, VisualTestContext};

use app_database::{AppDatabase, ConnectionData, QueryTabData};
use blanco_core::ConnectionContext;

use super::TabCreationParams;
use crate::test_harness::TestHarness;

/// Tabs are stored against a saved connection (they are loaded with a join on
/// it), so these tests save one first and open their tabs on it.
fn saved_connection_context(cx: &mut VisualTestContext) -> ConnectionContext {
    let connection_id = cx.update(|_, cx| {
        let db = AppDatabase::global(cx).clone();
        gpui_tokio::Tokio::handle(cx)
            .block_on(async move {
                db.save_connection(&ConnectionData::new_sqlite(
                    "saved".to_string(),
                    "/tmp/saved.db".to_string(),
                ))
                .await
            })
            .expect("save connection")
    });
    ConnectionContext {
        connection_id,
        connection_name: "saved".to_string(),
        db_type: database::DatabaseType::SQLite,
        database_name: "main".to_string(),
        schema_name: None,
        environment_type: None,
    }
}

fn open_tab(
    harness: &TestHarness,
    title: &str,
    context: ConnectionContext,
    cx: &mut VisualTestContext,
) {
    let params = TabCreationParams {
        title: title.to_string(),
        content: None,
        db_id: None,
        last_run_at: None,
        context,
    };
    harness.editor_panel.update_in(cx, |panel, window, cx| {
        panel.create_and_add_tab_with_connection(window, params, cx);
    });
}

fn type_into_active_tab(harness: &TestHarness, text: &str, cx: &mut VisualTestContext) {
    let text = text.to_string();
    harness.editor_panel.update_in(cx, |panel, window, cx| {
        let editor = panel.active_query_tab().expect("query tab").editor.clone();
        editor.update(cx, |state, cx| state.set_value(&text, window, cx));
    });
}

/// Let the autosave debounce pass and its write land.
async fn settle(cx: &mut VisualTestContext) {
    for _ in 0..10 {
        cx.executor().advance_clock(super::AUTOSAVE_DELAY);
        cx.run_until_parked();
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
}

/// The stored tabs titled `title`. The harness opens a tab of its own, which
/// is left out.
fn stored_tabs(title: &str, cx: &mut VisualTestContext) -> Vec<QueryTabData> {
    let tabs = cx.update(|_, cx| {
        let db = AppDatabase::global(cx).clone();
        gpui_tokio::Tokio::handle(cx)
            .block_on(async move { db.load_query_tabs().await })
            .expect("load tabs")
    });
    tabs.into_iter().filter(|tab| tab.title == title).collect()
}

#[gpui::test]
async fn test_typed_text_is_saved_without_running(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let context = saved_connection_context(&mut cx);

    open_tab(&harness, "Draft", context, &mut cx);
    type_into_active_tab(&harness, "SELECT 'first'", &mut cx);
    type_into_active_tab(&harness, "SELECT 'typed since'", &mut cx);
    settle(&mut cx).await;

    let tabs = stored_tabs("Draft", &mut cx);
    assert_eq!(tabs.len(), 1, "one row per tab, however often it is saved");
    assert_eq!(tabs[0].title, "Draft");
    assert_eq!(tabs[0].content, "SELECT 'typed since'");
}

#[gpui::test]
async fn test_closed_tab_reopens_with_its_text(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let context = saved_connection_context(&mut cx);

    open_tab(&harness, "Keep me", context, &mut cx);
    type_into_active_tab(&harness, "SELECT 42", &mut cx);
    settle(&mut cx).await;
    assert_eq!(stored_tabs("Keep me", &mut cx).len(), 1);

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.close_active_tab(window, cx)
        });
    settle(&mut cx).await;
    assert!(
        stored_tabs("Keep me", &mut cx).is_empty(),
        "a closed tab is not restored"
    );

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.reopen_closed_tab(window, cx)
        });
    settle(&mut cx).await;

    let reopened = harness.editor_panel.read_with(&cx, |panel, cx| {
        let tab = panel.active_query_tab().expect("reopened tab");
        (tab.title.clone(), tab.editor.read(cx).text().to_string())
    });
    assert_eq!(reopened, ("Keep me".to_string(), "SELECT 42".to_string()));
    let tabs = stored_tabs("Keep me", &mut cx);
    assert_eq!(tabs.len(), 1, "the reopened tab is persisted again");
    assert_eq!(tabs[0].content, "SELECT 42");
}
