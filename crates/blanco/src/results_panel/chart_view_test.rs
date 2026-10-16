use gpui::{TestAppContext, VisualTestContext};

use super::nice_axis_max;
use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

/// Every result column can be charted, and each select reads the column it
/// names. The grid once had a row number column first, which the chart
/// skipped and offset by long after it was gone.
#[gpui::test]
async fn test_chart_columns_match_the_result(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "SELECT 'Jazz' AS genre, 3 AS orders, 10.5 AS revenue \
         UNION ALL SELECT 'Soul', 2, 4.0",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let chart_view = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .read(cx)
            .active_chart_view()
            .expect("result tab should have a chart")
    });
    chart_view.read_with(&cx, |chart, cx| {
        assert_eq!(chart.axis_column_indices, vec![0, 1, 2]);
        assert_eq!(chart.value_column_indices, vec![1, 2]);
        let rows = chart.extract_raw(cx).expect("chart should have data");
        assert_eq!(
            rows,
            vec![
                ("Jazz".to_string(), 3.0, None),
                ("Soul".to_string(), 2.0, None),
            ]
        );
    });
}

/// Stacked bars end their value axis on a round number with little room to
/// spare, so the tick labels read cleanly.
#[test]
fn test_nice_axis_max_rounds_up_to_a_round_step() {
    assert_eq!(nice_axis_max(109_036.2), 120_000.);
    assert_eq!(nice_axis_max(9.5), 10.);
    assert_eq!(nice_axis_max(4.), 4.);
    assert_eq!(nice_axis_max(0.37), 0.4);
    assert_eq!(nice_axis_max(0.), 4.);
}
