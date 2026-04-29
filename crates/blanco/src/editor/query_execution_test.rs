use gpui::{TestAppContext, VisualTestContext};

use crate::test_harness::{
    TestHarness, result_row_count, run_query, set_editor_text, wait_for_query,
};

#[gpui::test]
async fn test_execute_simple_query(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 1 as value, 'hello' as name", &mut cx);

    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let rows = result_row_count(&harness, &cx).expect("should have a results panel with rows");
    assert_eq!(rows, 1, "SELECT 1 should return exactly 1 row");
}

#[gpui::test]
async fn test_execute_query_multiple_rows(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "SELECT 1 AS n UNION ALL SELECT 2 UNION ALL SELECT 3",
        &mut cx,
    );

    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let rows = result_row_count(&harness, &cx).expect("should have a results panel with rows");
    assert_eq!(rows, 3, "Should return 3 rows");
}

#[gpui::test]
async fn test_execute_write_query(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "CREATE TABLE test_items (id INTEGER PRIMARY KEY, v TEXT)",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    assert!(
        !crate::test_harness::is_loading(&harness, &cx),
        "Query should have finished"
    );

    set_editor_text(
        &harness,
        "SELECT 1 AS id, 'hello' AS v WHERE 1 = 1",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let rows = result_row_count(&harness, &cx).expect("should have results");
    assert_eq!(rows, 1, "Should return 1 row");
}
