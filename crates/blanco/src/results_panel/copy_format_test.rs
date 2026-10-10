use gpui::{Focusable as _, TestAppContext, VisualTestContext};

use crate::app_settings::AppSettings;
use crate::settings::CopyFormat;
use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

/// The plain copy shortcut copies the selected row in the format chosen in
/// settings, TSV until one is chosen.
#[gpui::test]
async fn test_copy_shortcut_uses_the_configured_format(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 1 AS id, 'a' AS name", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let table_state = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .read(cx)
            .table_state()
            .clone()
    });
    table_state.update_in(&mut cx, |state, window, cx| {
        state.set_selected_cell(0, 1, cx);
        state.focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();

    let copy = |cx: &mut VisualTestContext| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
        cx.simulate_keystrokes("secondary-c");
        cx.run_until_parked();
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    };

    assert_eq!(copy(&mut cx).trim_end(), "id\tname\n1\ta");

    cx.update(|_, cx| AppSettings::global_mut(cx).settings.results.copy_format = CopyFormat::Json);
    let json = copy(&mut cx);
    assert!(
        json.trim_start().starts_with('[') && json.contains("\"name\": \"a\""),
        "expected JSON rows, got {json:?}"
    );
}
