use gpui::{Focusable as _, TestAppContext, VisualTestContext};

use crate::app::CopyAsCSV;
use crate::app_settings::AppSettings;
use crate::settings::CsvDelimiter;
use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

async fn copy_as_csv(harness: &TestHarness, cx: &mut VisualTestContext) -> String {
    let table_state = harness.editor_panel.read_with(cx, |panel, cx| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .read(cx)
            .table_state()
            .clone()
    });
    table_state.update_in(cx, |state, window, cx| {
        state.set_selected_cell(0, 0, cx);
        state.focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();

    cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
    cx.dispatch_action(CopyAsCSV);
    cx.run_until_parked();
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

/// Copy as CSV writes with the options from the CSV settings.
#[gpui::test]
async fn test_copy_as_csv_follows_settings(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "SELECT 1 AS id, 'a,b' AS name, NULL AS note",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    assert_eq!(
        copy_as_csv(&harness, &mut cx).await,
        "id;name;note\n1;a,b;\n"
    );

    cx.update(|_, cx| {
        let csv = &mut AppSettings::global_mut(cx).settings.csv;
        csv.delimiter = CsvDelimiter::Comma;
        csv.header = false;
        csv.null_text = "NULL".into();
    });
    assert_eq!(copy_as_csv(&harness, &mut cx).await, "1,\"a,b\",NULL\n");
}
