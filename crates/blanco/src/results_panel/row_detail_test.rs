use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};
use gpui_component::input::TextareaState;

use crate::app::RowDetailNextRow;
use crate::results_panel::ResultsPanel;
use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

async fn run(harness: &TestHarness, sql: &str, cx: &mut VisualTestContext) {
    set_editor_text(harness, sql, cx);
    run_query(harness, cx);
    wait_for_query(harness, cx).await;
}

fn field(
    panel: &Entity<ResultsPanel>,
    col: usize,
    cx: &VisualTestContext,
) -> Entity<TextareaState> {
    panel.read_with(cx, |panel, _| {
        panel
            .row_detail
            .as_ref()
            .and_then(|detail| detail.field_input(col))
            .expect("row detail field")
    })
}

fn text(input: &Entity<TextareaState>, cx: &VisualTestContext) -> String {
    input.read_with(cx, |input, _| input.value().to_string())
}

#[gpui::test]
async fn row_detail_follows_selection_and_edits_through_pending_changes(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    // Focus and blur events are only delivered to the active window.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    run(
        &harness,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, v TEXT, note TEXT)",
        &mut cx,
    )
    .await;
    run(
        &harness,
        "INSERT INTO items (id, v, note) VALUES (1, 'first', NULL), (2, 'second', 'x')",
        &mut cx,
    )
    .await;
    run(&harness, "SELECT * FROM items ORDER BY id", &mut cx).await;

    let panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    });
    let table_state = panel.read_with(&cx, |panel, _| panel.table_state().clone());

    panel.update_in(&mut cx, |panel, window, cx| {
        panel.toggle_row_detail(window, cx)
    });
    table_state.update(&mut cx, |state, cx| state.set_selected_cell(0, 1, cx));
    cx.run_until_parked();

    panel.read_with(&cx, |panel, _| {
        assert_eq!(panel.row_detail.as_ref().and_then(|d| d.row()), Some(0));
    });
    let value = field(&panel, 1, &cx);
    let note = field(&panel, 2, &cx);
    assert_eq!(text(&value, &cx), "first");
    assert_eq!(text(&note, &cx), "", "NULL shows as an empty field");

    // Type into one field, then click into the next: the first applies as a
    // pending edit and the second keeps focus.
    value.update_in(&mut cx, |input, window, cx| {
        input.focus_handle(cx).focus(window, cx);
        input.replace_all("edited", window, cx);
    });
    cx.run_until_parked();
    note.update_in(&mut cx, |input, window, cx| {
        input.focus_handle(cx).focus(window, cx);
    });
    cx.run_until_parked();

    let statements = panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(statements.len(), 1, "{statements:?}");
    assert!(statements[0].contains("edited"), "{}", statements[0]);
    let grid_value =
        table_state.read_with(&cx, |state, _| state.delegate().stored_cell_value(0, 1));
    assert_eq!(grid_value.as_deref(), Some("edited"));
    assert_eq!(
        field(&panel, 2, &cx).entity_id(),
        note.entity_id(),
        "refilling after the edit must not rebuild the focused field"
    );
    cx.update(|window, cx| {
        assert!(note.focus_handle(cx).is_focused(window));
    });

    // The next row shortcut moves the grid and the panel, focus staying on
    // the same field.
    cx.dispatch_action(RowDetailNextRow);
    cx.run_until_parked();

    panel.read_with(&cx, |panel, _| {
        assert_eq!(panel.row_detail.as_ref().and_then(|d| d.row()), Some(1));
    });
    let selected = table_state.read_with(&cx, |state, _| state.selected_cell());
    assert_eq!(selected, Some((1, 1)));
    let note = field(&panel, 2, &cx);
    assert_eq!(text(&note, &cx), "x");
    cx.update(|window, cx| {
        assert!(note.focus_handle(cx).is_focused(window));
    });
    // The edit of the first row is still pending.
    let statements = panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(statements.len(), 1, "{statements:?}");
}
