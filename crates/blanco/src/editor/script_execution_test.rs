use gpui::{TestAppContext, VisualTestContext};

use crate::test_harness::{
    TestHarness, is_loading, script_log_text, script_result_cell, script_result_row_count,
    set_script_text, wait_for_script,
};

#[gpui::test]
async fn test_script_runs_queries_and_displays_results(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    harness.add_script_tab(Some(""), &mut cx);

    set_script_text(
        &harness,
        r#"
        db.execute("CREATE TABLE script_items (id INTEGER PRIMARY KEY, name TEXT)");
        for (let i = 1; i <= 5; i++) {
            db.execute("INSERT INTO script_items (id, name) VALUES (?, ?)", [i, "row-" + i]);
        }
        const counted = db.query("SELECT COUNT(*) AS total FROM script_items");
        console.log("inserted " + counted.rows[0].total + " rows");
        db.display(counted);
        "#,
        &mut cx,
    );

    crate::test_harness::run_query(&harness, &mut cx);
    wait_for_script(&harness, &mut cx).await;

    assert_eq!(
        script_result_row_count(&harness, &cx),
        Some(1),
        "db.display should have populated the grid"
    );
    assert_eq!(
        script_result_cell(&harness, 0, 0, &cx),
        Some(Some("5".to_string()))
    );

    let log = script_log_text(&harness, &cx);
    assert!(
        log.contains("inserted 5 rows"),
        "console output should reach the log, got: {log}"
    );
    assert!(
        log.contains("script finished"),
        "log should record completion, got: {log}"
    );
}

#[gpui::test]
async fn test_script_can_be_stopped(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    harness.add_script_tab(Some(""), &mut cx);

    set_script_text(&harness, "while (true) {}", &mut cx);
    crate::test_harness::run_query(&harness, &mut cx);
    cx.run_until_parked();

    assert!(
        is_loading(&harness, &cx),
        "a runaway script should leave the panel in the loading state"
    );

    harness.editor_panel.update(&mut cx, |panel, cx| {
        panel.cancel_running_script(cx);
    });
    wait_for_script(&harness, &mut cx).await;

    assert!(!is_loading(&harness, &cx), "Stop should clear loading");
    let log = script_log_text(&harness, &cx);
    assert!(
        log.contains("cancelled by user"),
        "cancellation should be logged, got: {log}"
    );
}

#[gpui::test]
async fn test_script_error_is_reported_without_stalling(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    harness.add_script_tab(Some(""), &mut cx);

    set_script_text(&harness, "throw new Error('script blew up');", &mut cx);
    crate::test_harness::run_query(&harness, &mut cx);
    wait_for_script(&harness, &mut cx).await;

    assert!(!is_loading(&harness, &cx));
    let log = script_log_text(&harness, &cx);
    assert!(
        log.contains("script blew up"),
        "the failure should be logged, got: {log}"
    );
}
