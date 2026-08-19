use gpui::{TestAppContext, VisualTestContext};

use crate::results_panel::{CellInput, ResultsTableDelegate};
use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

async fn setup_editable_table(
    harness: &TestHarness,
    cx: &mut VisualTestContext,
) -> gpui::Entity<crate::results_panel::ResultsPanel> {
    set_editor_text(
        harness,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, v TEXT)",
        cx,
    );
    run_query(harness, cx);
    wait_for_query(harness, cx).await;

    set_editor_text(harness, "INSERT INTO items (id, v) VALUES (1, 'orig')", cx);
    run_query(harness, cx);
    wait_for_query(harness, cx).await;

    set_editor_text(harness, "SELECT * FROM items", cx);
    run_query(harness, cx);
    wait_for_query(harness, cx).await;

    harness.editor_panel.read_with(cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .clone()
    })
}

/// Plain inline editing: typing then finalizing (what Apply edits does when a
/// cell is still in edit mode) must produce exactly one UPDATE.
#[gpui::test]
async fn test_inline_edit_commits_via_finalize(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();

    let input = table_state.read_with(&cx, |state, _| {
        match state
            .delegate()
            .edit_state
            .get_editing_input()
            .expect("editing input should exist")
        {
            CellInput::Inline(input) => input,
            CellInput::Expanded(_) => panic!("a fresh cell edit is inline"),
        }
    });
    // `replace_all` emits `InputEvent::Change` just like keyboard input
    // (`set_value` suppresses events).
    input.update_in(&mut cx, |input, window, cx| {
        input.replace_all("inline-edited", window, cx);
    });
    cx.run_until_parked();

    results_panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    cx.run_until_parked();

    let statements = results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(
        statements.len(),
        1,
        "inline edit should preview one UPDATE, got {:?}",
        statements
    );
    assert!(
        statements[0].contains("inline-edited"),
        "UPDATE should carry the edited value, got {:?}",
        statements
    );
}

/// Editing a cell through the expanded (maximized) editor, then collapsing it,
/// must keep the edited text and produce an UPDATE statement on Apply. The
/// expanded input entity is dropped on collapse, so its blur commit never
/// fires; the finalize path has to pick the edit up from the live input.
#[gpui::test]
async fn test_expanded_edit_survives_minimize(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    let is_editable = table_state.read_with(&cx, |state, _| state.delegate().is_editable());
    assert!(is_editable, "result table should be editable");

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();

    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::handle_maximize(state, 0, 1, false, window, cx);
    });
    cx.run_until_parked();

    let expanded_input = table_state.read_with(&cx, |state, _| {
        match state
            .delegate()
            .edit_state
            .get_editing_input()
            .expect("expanded editing input should exist")
        {
            CellInput::Expanded(editor) => editor,
            CellInput::Inline(_) => panic!("maximize switches the cell to the expanded editor"),
        }
    });
    expanded_input.update_in(&mut cx, |input, window, cx| {
        input.replace_all("edited-value", window, cx);
    });
    cx.run_until_parked();

    let edited_values = table_state.read_with(&cx, |state, _| {
        state.delegate().edit_state.edited_values.clone()
    });
    assert_eq!(
        edited_values.get(&(0, 1)),
        Some(&Some("edited-value".to_string())),
        "expanded typing should land in edited_values under (row=0, col=1), got {:?}",
        edited_values
    );

    // Collapse the expanded editor (call site passes (col, row)).
    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::handle_minimize(state, (1, 0), false, window, cx);
    });
    cx.run_until_parked();

    let inline_input = table_state.read_with(&cx, |state, _| {
        state
            .delegate()
            .edit_state
            .get_editing_input()
            .expect("collapsed editing input should exist")
    });
    let inline_text = cx.read(|cx| inline_input.text(cx));
    assert_eq!(
        inline_text, "edited-value",
        "collapsed input should keep the text entered while expanded"
    );

    // What the Apply edits popover does before rendering the SQL preview.
    results_panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    cx.run_until_parked();

    let (edited_values, changes) = table_state.read_with(&cx, |state, _| {
        (
            state.delegate().edit_state.edited_values.clone(),
            state.delegate().edit_state.changes.clone(),
        )
    });
    assert_eq!(
        edited_values.get(&(0, 1)),
        Some(&Some("edited-value".to_string())),
        "cell should still show as edited after collapse+finalize, got {:?}",
        edited_values
    );
    assert_eq!(
        changes.len(),
        1,
        "collapse+finalize should have committed exactly one change, got {:?}",
        changes
    );

    let statements = results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(
        statements.len(),
        1,
        "Apply edits preview should contain one UPDATE, got {:?}",
        statements
    );
    assert!(
        statements[0].contains("edited-value"),
        "UPDATE should carry the edited value, got {:?}",
        statements
    );
}
