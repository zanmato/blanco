use gpui::{Entity, TestAppContext, VisualTestContext};

use super::ResultsPanel;
use super::filter_bar::FilterBar;
use crate::app::FilterByCellValue;
use crate::test_harness::{
    TestHarness, result_cell, result_row_count, run_query, set_editor_text, wait_for_query,
};

fn results_panel(harness: &TestHarness, cx: &VisualTestContext) -> Entity<ResultsPanel> {
    harness.editor_panel.read_with(cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    })
}

async fn wait_for_filter(bar: &Entity<FilterBar>, cx: &mut VisualTestContext) {
    for _ in 0..100 {
        cx.run_until_parked();
        if !bar.read_with(cx, |bar, _| bar.is_running()) {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("filter did not finish");
}

async fn run(harness: &TestHarness, sql: &str, cx: &mut VisualTestContext) {
    set_editor_text(harness, sql, cx);
    run_query(harness, cx);
    wait_for_query(harness, cx).await;
}

#[gpui::test]
async fn filter_bar_runs_conditions_on_the_server(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    run(
        &harness,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, qty INTEGER)",
        &mut cx,
    )
    .await;
    for (id, name, qty) in [(1, "apple", 5), (2, "Banana", 12), (3, "cherry", 7)] {
        run(
            &harness,
            &format!("INSERT INTO items VALUES ({id}, '{name}', {qty})"),
            &mut cx,
        )
        .await;
    }
    run(
        &harness,
        "SELECT * FROM items ORDER BY id LIMIT 100",
        &mut cx,
    )
    .await;
    assert_eq!(result_row_count(&harness, &cx), Some(3));

    let panel = results_panel(&harness, &cx);
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.toggle_filter_bar(window, cx)));
    cx.run_until_parked();
    let bar = panel
        .read_with(&cx, |panel, _| panel.active_filter_bar())
        .expect("the bar opens for a table result");

    // A typed WHERE keeps the query's own ordering.
    cx.update(|window, cx| bar.update(cx, |bar, cx| bar.apply_where("qty > 6", window, cx)));
    wait_for_filter(&bar, &mut cx).await;
    assert_eq!(bar.read_with(&cx, |bar, _| bar.error()), None);
    assert_eq!(result_row_count(&harness, &cx), Some(2));
    assert_eq!(
        result_cell(&harness, 0, 1, &cx),
        Some(Some("Banana".into()))
    );

    // The next filter starts from the unfiltered query, not the filtered one.
    cx.update(|window, cx| bar.update(cx, |bar, cx| bar.apply_any_column("AN", window, cx)));
    wait_for_filter(&bar, &mut cx).await;
    assert_eq!(result_row_count(&harness, &cx), Some(1));
    assert_eq!(
        result_cell(&harness, 0, 1, &cx),
        Some(Some("Banana".into()))
    );

    // Filter by a cell's value from the context menu.
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.on_filter_by_cell_value(&FilterByCellValue { row: 0, col: 2 }, window, cx)
        })
    });
    wait_for_filter(&bar, &mut cx).await;
    assert_eq!(result_row_count(&harness, &cx), Some(1));
    assert_eq!(result_cell(&harness, 0, 2, &cx), Some(Some("12".into())));

    // A broken condition reports an error and leaves the rows alone.
    cx.update(|window, cx| bar.update(cx, |bar, cx| bar.apply_where("qty >", window, cx)));
    wait_for_filter(&bar, &mut cx).await;
    let error = bar.read_with(&cx, |bar, _| bar.error()).expect("error");
    assert!(error.starts_with("Invalid filter"), "{error}");
    assert_eq!(result_row_count(&harness, &cx), Some(1));

    // The rows stay editable: the table identity carries over.
    let editable = panel.read_with(&cx, |panel, cx| {
        panel.table_state().read(cx).delegate().is_editable()
    });
    assert!(editable);
}
