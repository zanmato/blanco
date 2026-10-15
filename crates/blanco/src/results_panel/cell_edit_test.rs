use gpui::{TestAppContext, VisualTestContext};
use gpui_component::menu::PopupMenu;
use gpui_component::table::{TableDelegate as _, TableSelection};

use crate::app::DeleteRow;
use crate::results_panel::{CellInput, ResultsTableDelegate};
use crate::test_harness::{TestHarness, result_cell, run_query, set_editor_text, wait_for_query};

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

/// A new row in a table with column defaults must omit untouched default
/// columns from the INSERT (so the server default applies), and a successful
/// commit must re-run the original query so server-generated values replace
/// the local placeholders.
#[gpui::test]
async fn test_new_row_omits_default_columns_and_refreshes_after_commit(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "CREATE TABLE defaults_test (id INTEGER PRIMARY KEY, v TEXT, created TEXT DEFAULT 'server-default')",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(
        &harness,
        "INSERT INTO defaults_test (v) VALUES ('first')",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(&harness, "SELECT * FROM defaults_test", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .clone()
    });
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    results_panel.update(&mut cx, |panel, cx| panel.add_new_row(cx));
    cx.run_until_parked();

    // The untouched "created" cell of the new row shows the server default.
    let uses_default =
        table_state.read_with(&cx, |state, _| state.delegate().cell_uses_default(1, 2));
    assert!(
        uses_default,
        "untouched cell with a column default should display DEFAULT"
    );

    // Type into "v" only.
    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(1, 1, window, cx);
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
    input.update_in(&mut cx, |input, window, cx| {
        input.replace_all("second", window, cx);
    });
    cx.run_until_parked();
    results_panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    cx.run_until_parked();

    let statements = results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(
        statements.len(),
        1,
        "expected one INSERT, got {statements:?}"
    );
    assert!(
        statements[0].ends_with(r#"("id", "v") VALUES (NULL, 'second')"#),
        "INSERT should send the SQLite key as NULL and omit the defaulted column, got {statements:?}"
    );

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.commit_current_changes(window, cx);
        });

    // The commit runs on tokio threads and the post-commit refresh is a second
    // round trip, so poll like wait_for_query does.
    for _ in 0..100 {
        cx.run_until_parked();
        if result_cell(&harness, 1, 2, &cx) == Some(Some("server-default".to_string())) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }

    assert_eq!(
        result_cell(&harness, 1, 2, &cx),
        Some(Some("server-default".to_string())),
        "refresh should surface the server-side default"
    );
    assert_eq!(
        result_cell(&harness, 1, 0, &cx),
        Some(Some("2".to_string())),
        "refresh should surface the generated primary key"
    );
    assert_eq!(
        result_cell(&harness, 1, 1, &cx),
        Some(Some("second".to_string()))
    );

    let has_pending = results_panel.read_with(&cx, |panel, cx| panel.has_pending_edits(cx));
    assert!(!has_pending, "commit should leave no pending edits");
}

/// Duplicating a row must not copy its primary key. On SQLite the copy's key
/// is an explicit NULL (which assigns a fresh rowid), the INSERT spells it
/// out, and the post-commit refresh shows the generated key.
#[gpui::test]
async fn test_duplicate_row_leaves_primary_key_to_the_server(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    results_panel.update(&mut cx, |panel, cx| panel.duplicate_row_with_row(0, cx));
    cx.run_until_parked();

    let (key_cell, key_uses_default) = table_state.read_with(&cx, |state, _| {
        let delegate = state.delegate();
        (
            delegate.rows.get(1).and_then(|row| row.first().cloned()),
            delegate.cell_uses_default(1, 0),
        )
    });
    assert_eq!(
        key_cell,
        Some(None),
        "duplicated row must not carry the source key"
    );
    assert!(
        !key_uses_default,
        "SQLite has no key default, the cell shows NULL"
    );

    let statements = results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(
        statements.len(),
        1,
        "expected one INSERT, got {statements:?}"
    );
    assert!(
        statements[0].ends_with(r#"("id", "v") VALUES (NULL, 'orig')"#),
        "INSERT should send the key as NULL, got {statements:?}"
    );

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.commit_current_changes(window, cx);
        });
    for _ in 0..100 {
        cx.run_until_parked();
        if result_cell(&harness, 1, 0, &cx) == Some(Some("2".to_string())) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }

    assert_eq!(
        result_cell(&harness, 1, 0, &cx),
        Some(Some("2".to_string())),
        "refresh should surface the generated primary key"
    );
    assert_eq!(
        result_cell(&harness, 1, 1, &cx),
        Some(Some("orig".to_string()))
    );
    let has_pending = results_panel.read_with(&cx, |panel, cx| panel.has_pending_edits(cx));
    assert!(!has_pending, "commit should leave no pending edits");
}

/// EditNextCell/EditPrevCell (bound to tab/shift-tab in the cell editor) must
/// close the current cell's input, keep its text as a pending edit, and open
/// the neighbouring cell's editor.
#[gpui::test]
async fn test_edit_next_cell_moves_the_editor(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 0, window, cx);
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
    input.update_in(&mut cx, |input, window, cx| {
        input.replace_all("7", window, cx);
    });
    cx.run_until_parked();

    cx.dispatch_action(crate::app::EditNextCell);
    cx.run_until_parked();

    let (editing_cell, edited_values) = table_state.read_with(&cx, |state, _| {
        (
            state.delegate().edit_state.editing_cell,
            state.delegate().edit_state.edited_values.clone(),
        )
    });
    assert_eq!(
        editing_cell,
        Some((0, 1)),
        "tab should move the editor to the next column"
    );
    assert_eq!(
        edited_values.get(&(0, 0)),
        Some(&Some("7".to_string())),
        "the previous cell's text should be kept as a pending edit"
    );

    cx.dispatch_action(crate::app::EditPrevCell);
    cx.run_until_parked();

    let editing_cell =
        table_state.read_with(&cx, |state, _| state.delegate().edit_state.editing_cell);
    assert_eq!(
        editing_cell,
        Some((0, 0)),
        "shift-tab should move the editor back"
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

/// A multi-line value opens in the single-line inline input, which drops the
/// line breaks. Expanding the cell must show the stored lines, not the
/// flattened inline text.
#[gpui::test]
async fn test_maximize_restores_line_breaks_from_inline_input(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    set_editor_text(
        &harness,
        "UPDATE items SET v = 'first' || char(10) || 'second' || char(13) || char(10) || 'third' WHERE id = 1",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    set_editor_text(&harness, "SELECT * FROM items", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let stored = "first\nsecond\r\nthird";
    assert_eq!(
        result_cell(&harness, 0, 1, &cx).flatten().as_deref(),
        Some(stored),
        "fixture row should hold a multi-line value"
    );

    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());
    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();

    let inline_text = table_state.read_with(&cx, |state, cx| {
        state
            .delegate()
            .edit_state
            .get_editing_input()
            .expect("inline editing input should exist")
            .text(cx)
    });
    assert_eq!(
        inline_text, "firstsecondthird",
        "single-line input is expected to drop the line breaks"
    );

    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::handle_maximize(state, 0, 1, false, window, cx);
    });
    cx.run_until_parked();

    let expanded_text = table_state.read_with(&cx, |state, cx| {
        match state
            .delegate()
            .edit_state
            .get_editing_input()
            .expect("expanded editing input should exist")
        {
            CellInput::Expanded(editor) => editor.read(cx).text().to_string(),
            CellInput::Inline(_) => panic!("maximize switches the cell to the expanded editor"),
        }
    });
    assert_eq!(
        expanded_text, stored,
        "expanded editor should show the stored value with its line breaks"
    );
}

/// The toolbar Delete button acts on the whole row selection.
#[gpui::test]
async fn test_delete_marks_every_selected_row(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    set_editor_text(
        &harness,
        "INSERT INTO items (id, v) VALUES (2, 'two'), (3, 'three')",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    set_editor_text(&harness, "SELECT * FROM items", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());
    table_state.update(&mut cx, |state, cx| {
        state.set_selected_row(0, cx);
        state.add_selected_row(1, cx);
        state.add_selected_row(2, cx);
    });
    cx.run_until_parked();

    // The row context menu dispatches the action for the right-clicked row.
    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.on_delete_row(&DeleteRow { row: 1 }, window, cx)
    });
    cx.run_until_parked();

    let (deleted, changes) = table_state.read_with(&cx, |state, _| {
        (
            state.delegate().edit_state.pending_deleted_rows.clone(),
            state.delegate().edit_state.changes.clone(),
        )
    });
    assert_eq!(
        deleted.len(),
        3,
        "all selected rows should be marked deleted, got {:?}",
        deleted
    );
    assert_eq!(
        changes.len(),
        3,
        "one DELETE change per row, got {:?}",
        changes
    );

    // Deleting again from the toolbar must not duplicate the changes.
    results_panel.update(&mut cx, |panel, cx| panel.delete_row(cx));
    cx.run_until_parked();
    let changes =
        table_state.read_with(&cx, |state, _| state.delegate().edit_state.changes.clone());
    assert_eq!(
        changes.len(),
        3,
        "already deleted rows are skipped, got {:?}",
        changes
    );

    let statements = results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(
        statements.len(),
        3,
        "preview should hold three DELETEs, got {:?}",
        statements
    );
}

/// Horizontal cursor-follow inside a table cell: a value wider than the
/// column must scroll the inline input as the cursor walks right, and walking
/// back to the start must land on a zero offset with no leftover shift.
#[gpui::test]
async fn test_inline_edit_scrolls_horizontally_with_cursor(cx: &mut TestAppContext) {
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
    let long_value = "x".repeat(400);
    input.update_in(&mut cx, |input, window, cx| {
        input.replace_all(&long_value, window, cx);
        input.set_selected_range(0..0, window, cx);
    });
    cx.run_until_parked();
    let start_offset = input.read_with(&cx, |input, _| input.scroll_offset().x);
    assert_eq!(start_offset, gpui::px(0.));

    for _ in 0..long_value.len() {
        cx.simulate_keystrokes("right");
    }
    cx.run_until_parked();
    let (cursor, end_offset) =
        input.read_with(&cx, |input, _| (input.cursor(), input.scroll_offset().x));
    assert_eq!(cursor, long_value.len(), "cursor should reach the end");
    assert!(
        end_offset < gpui::px(0.),
        "input should scroll right to follow the cursor, offset {end_offset:?}"
    );

    for _ in 0..long_value.len() {
        cx.simulate_keystrokes("left");
    }
    cx.run_until_parked();
    let (cursor, offset) =
        input.read_with(&cx, |input, _| (input.cursor(), input.scroll_offset().x));
    assert_eq!(cursor, 0);
    assert_eq!(
        offset,
        gpui::px(0.),
        "scrolling back to the start should settle at 0"
    );
}

/// Left/right at the text boundaries must stay inside the cell editor rather
/// than moving the table's column selection underneath it.
#[gpui::test]
async fn test_inline_edit_arrows_do_not_reach_the_table(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();

    let before = table_state.read_with(&cx, |state, _| {
        (state.selected_cell(), state.selected_col())
    });

    cx.simulate_keystrokes("left left right right right");
    cx.run_until_parked();

    let after = table_state.read_with(&cx, |state, _| {
        (state.selected_cell(), state.selected_col())
    });
    let editing_cell =
        table_state.read_with(&cx, |state, _| state.delegate().edit_state.editing_cell);
    assert_eq!(
        after, before,
        "table selection should not move while editing"
    );
    assert_eq!(
        editing_cell,
        Some((0, 1)),
        "the cell should still be in edit mode"
    );
}

#[gpui::test]
async fn compare_cells_opens_a_diff_dialog(cx: &mut TestAppContext) {
    use crate::app::{CompareCellWithSelected, SelectCellForCompare};
    use crate::results_panel::compare::CompareKind;
    use blanco_ui::diff_view::DiffRowKind;
    use gpui_component::WindowExt as _;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    set_editor_text(
        &harness,
        "SELECT 'same' || char(10) || 'old' AS doc UNION ALL SELECT 'same' || char(10) || 'new'",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .clone()
    });

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.on_select_cell_for_compare(&SelectCellForCompare { row: 0, col: 0 }, window, cx);
    });
    cx.run_until_parked();

    let (kind, menu_kind) = results_panel.read_with(&cx, |panel, cx| {
        (
            panel.compare_selection_kind(),
            panel
                .table_state()
                .read_with(cx, |state, _| state.delegate().compare_selection_kind),
        )
    });
    assert_eq!(kind, Some(CompareKind::Cell));
    assert_eq!(
        menu_kind,
        Some(CompareKind::Cell),
        "the delegate mirrors the selection so the context menu can offer comparison"
    );

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.on_compare_cell_with_selected(
            &CompareCellWithSelected { row: 1, col: 0 },
            window,
            cx,
        );
    });
    cx.run_until_parked();

    let has_dialog = cx.update(|window, cx| window.has_active_dialog(cx));
    assert!(has_dialog, "comparing should open the diff dialog");

    let diff_view = results_panel
        .read_with(&cx, |panel, _| panel.last_diff_view())
        .expect("diff view should be stored");
    let kinds = diff_view.read_with(&cx, |view, _| view.row_kinds());
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == DiffRowKind::Modified)
            .count(),
        1,
        "only the second line differs between the two values: {kinds:?}"
    );
    assert_eq!(kinds.first(), Some(&DiffRowKind::Equal));
}

/// A read that matches nothing still yields the table's header (rebuilt from
/// the statement description), so "Add row" works on an empty table.
#[gpui::test]
async fn test_add_row_after_empty_result(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "CREATE TABLE empty_items (id INTEGER PRIMARY KEY, v TEXT)",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(
        &harness,
        "SELECT * FROM empty_items WHERE v = 'first'",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .clone()
    });
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    let (column_names, row_count) = table_state.read_with(&cx, |state, _| {
        let delegate = state.delegate();
        (
            delegate
                .columns
                .iter()
                .map(|column| column.name.to_string())
                .collect::<Vec<_>>(),
            delegate.rows.len(),
        )
    });
    assert_eq!(row_count, 0);
    assert_eq!(column_names, vec!["id".to_string(), "v".to_string()]);
    assert!(
        results_panel.read_with(&cx, |panel, cx| panel.is_editable(cx)),
        "an empty single-table read should still be editable"
    );

    results_panel.update(&mut cx, |panel, cx| panel.add_new_row(cx));
    cx.run_until_parked();

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
    input.update_in(&mut cx, |input, window, cx| {
        input.replace_all("first", window, cx);
    });
    cx.run_until_parked();
    results_panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    cx.run_until_parked();

    let statements = results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(
        statements.len(),
        1,
        "expected one INSERT, got {statements:?}"
    );
    assert!(
        statements[0].ends_with(r#"("id", "v") VALUES (NULL, 'first')"#),
        "INSERT should name the rebuilt columns, got {statements:?}"
    );

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.commit_current_changes(window, cx);
        });

    for _ in 0..100 {
        cx.run_until_parked();
        if result_cell(&harness, 0, 0, &cx) == Some(Some("1".to_string())) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }

    assert_eq!(
        result_cell(&harness, 0, 0, &cx),
        Some(Some("1".to_string())),
        "refresh should surface the generated primary key"
    );
    assert_eq!(
        result_cell(&harness, 0, 1, &cx),
        Some(Some("first".to_string()))
    );
}

/// A cell is data unless its expression toggle is on: the same text is quoted
/// without it and sent verbatim with it. Cancelling a later edit must not
/// change the kind of the value that is still pending.
#[gpui::test]
async fn test_expression_toggle_sends_cell_text_as_sql(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let results_panel = setup_editable_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    let editing_input = |cx: &VisualTestContext| {
        table_state.read_with(cx, |state, _| {
            match state
                .delegate()
                .edit_state
                .get_editing_input()
                .expect("editing input should exist")
            {
                CellInput::Inline(input) => input,
                CellInput::Expanded(_) => panic!("a fresh cell edit is inline"),
            }
        })
    };

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    editing_input(&cx).update_in(&mut cx, |input, window, cx| {
        input.replace_all("upper('a') || 'b'", window, cx);
    });
    cx.run_until_parked();

    let statements = results_panel.update(&mut cx, |panel, cx| {
        panel.finalize_active_cell_edit(cx);
        panel.preview_pending_sql(cx)
    });
    assert_eq!(
        statements,
        vec![r#"UPDATE "items" SET "v" = 'upper(''a'') || ''b''' WHERE "id" = '1'"#.to_string()],
        "without the toggle the text is data"
    );

    // Toggling alone is an edit: the text is unchanged but now means SQL.
    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    cx.dispatch_action(crate::app::ToggleCellExpression);
    cx.run_until_parked();

    let statements = results_panel.update(&mut cx, |panel, cx| {
        panel.finalize_active_cell_edit(cx);
        panel.preview_pending_sql(cx)
    });
    assert_eq!(
        statements,
        vec![r#"UPDATE "items" SET "v" = upper('a') || 'b' WHERE "id" = '1'"#.to_string()],
    );

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    let starts_as_expression = table_state.read_with(&cx, |state, _| {
        state.delegate().edit_state.editing_as_expression
    });
    assert!(
        starts_as_expression,
        "reopening the cell should show its toggle on"
    );
    cx.dispatch_action(crate::app::ToggleCellExpression);
    cx.run_until_parked();
    results_panel.update(&mut cx, |panel, cx| panel.cancel_current_edit(cx));
    cx.run_until_parked();

    let is_expression = table_state.read_with(&cx, |state, _| {
        state.delegate().edit_state.is_expression(0, 1)
    });
    assert!(
        is_expression,
        "a cancelled edit must leave the pending expression as it was"
    );
}

/// A text value holding a JSON object opens formatted in the expanded editor,
/// keeps its key order, reports invalid JSON while typing, and collapses back
/// to its minified form.
#[gpui::test]
async fn test_expanded_text_json_formats_and_minifies(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "CREATE TABLE docs (id INTEGER PRIMARY KEY, body TEXT)",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    set_editor_text(
        &harness,
        r#"INSERT INTO docs (id, body) VALUES (1, '{"zeta":1,"alpha":[1,2]}')"#,
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    set_editor_text(&harness, "SELECT * FROM docs", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    });

    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());
    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::handle_maximize(state, 0, 1, false, window, cx);
    });
    cx.run_until_parked();

    let editor = table_state.read_with(&cx, |state, _| {
        assert!(state.delegate().edit_state.expanded_as_json);
        match state.delegate().edit_state.get_editing_input() {
            Some(CellInput::Expanded(editor)) => editor,
            _ => panic!("maximize opens the expanded editor"),
        }
    });
    let text = editor.read_with(&cx, |editor, _| editor.text().to_string());
    assert_eq!(
        text,
        "{\n  \"zeta\": 1,\n  \"alpha\": [\n    1,\n    2\n  ]\n}"
    );

    editor.update_in(&mut cx, |editor, window, cx| {
        editor.replace_all("{\"zeta\": }", window, cx);
    });
    cx.run_until_parked();
    let error = table_state.read_with(&cx, |state, _| {
        state.delegate().edit_state.json_error.clone()
    });
    assert!(error.is_some(), "invalid JSON is flagged while typing");

    editor.update_in(&mut cx, |editor, window, cx| {
        editor.replace_all("{\n  \"zeta\": 2,\n  \"alpha\": []\n}", window, cx);
    });
    cx.run_until_parked();
    table_state.update_in(&mut cx, |state, window, cx| {
        assert!(state.delegate().edit_state.json_error.is_none());
        ResultsTableDelegate::handle_minimize(state, (1, 0), false, window, cx);
    });
    cx.run_until_parked();
    let inline = table_state.read_with(&cx, |state, cx| {
        state
            .delegate()
            .edit_state
            .get_editing_input()
            .map(|input| input.text(cx))
    });
    assert_eq!(inline.as_deref(), Some(r#"{"zeta":2,"alpha":[]}"#));
}

/// Copying a column as a distinct IN list leaves NULL out and keeps numbers
/// bare; the plain list keeps every value, NULL included.
#[gpui::test]
async fn test_copy_column_values(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    for sql in [
        "CREATE TABLE picks (id INTEGER, v TEXT)",
        "INSERT INTO picks VALUES (1, 'a'), (2, NULL), (2, 'it''s')",
        "SELECT id, v FROM picks",
    ] {
        set_editor_text(&harness, sql, &mut cx);
        run_query(&harness, &mut cx);
        wait_for_query(&harness, &mut cx).await;
    }
    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    });

    let copy = |column, distinct, as_in_list, cx: &mut VisualTestContext| {
        results_panel.update_in(cx, |panel, window, cx| {
            panel.on_copy_column_values(
                &crate::app::CopyColumnValues {
                    column,
                    distinct,
                    as_in_list,
                    selected_only: false,
                },
                window,
                cx,
            );
        });
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    };
    assert_eq!(copy(0, true, true, &mut cx), "(1, 2)");
    assert_eq!(copy(1, false, true, &mut cx), "('a', 'it''s')");
    assert_eq!(copy(1, false, false, &mut cx), "a\nNULL\nit's");
}

/// A result column can be selected from its header, and the header's context
/// menu offers the column copies.
#[gpui::test]
async fn test_column_header_selects_and_has_a_menu(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 1 AS id, 'a' AS v", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let table_state = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .read(cx)
            .table_state()
            .clone()
    });

    assert!(table_state.read_with(&cx, |state, _| state.col_selectable));
    table_state.update(&mut cx, |state, cx| {
        state.set_selection(TableSelection::Column(1), cx);
    });
    assert_eq!(
        table_state.read_with(&cx, |state, _| state.selection()),
        TableSelection::Column(1)
    );

    let menu = cx.update(|window, cx| {
        PopupMenu::build(window, cx, move |menu, window, cx| {
            table_state.update(cx, |state, cx| {
                state
                    .delegate_mut()
                    .column_context_menu(1, menu, window, cx)
            })
        })
    });
    assert!(!menu.read_with(&cx, |menu, _| menu.is_empty()));
}

/// With the setting on, a cell marks a no-break space, a control character
/// and the spaces that end its value, while the stored value is untouched.
#[gpui::test]
async fn test_cells_mark_invisible_characters_when_asked(cx: &mut TestAppContext) {
    use crate::app_settings::AppSettings;
    use crate::results_panel::results_table_delegate::results_settings;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "SELECT 'a' || char(160) || 'b' || char(7) || 'c  ' AS v",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    let table_state = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .read(cx)
            .table_state()
            .clone()
    });
    let stored = "a\u{00A0}b\u{0007}c  ";
    let shown = |cx: &mut VisualTestContext| {
        table_state.read_with(cx, |state, cx| {
            state
                .delegate()
                .display_value(0, stored, &results_settings(cx))
        })
    };
    assert!(shown(&mut cx).markers.is_empty());

    cx.update(|_, cx| {
        AppSettings::global_mut(cx)
            .settings
            .results
            .show_invisible_characters = true;
        cx.refresh_windows();
    });
    cx.run_until_parked();
    let marked = shown(&mut cx);
    assert_eq!(marked.text, "a\u{00A0}bU+0007c  ");
    assert_eq!(marked.markers.len(), 4);
    assert_eq!(
        table_state.read_with(&cx, |state, _| state.delegate().rows[0][0].clone()),
        Some(stored.to_string())
    );
}

/// The copy shortcut on a focused grid copies a selected cell's value, the
/// selected rows as TSV and a selected column's values.
#[gpui::test]
async fn test_copy_shortcut_copies_the_selection(cx: &mut TestAppContext) {
    use gpui::Focusable as _;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.update(|_, cx| {
        cx.bind_keys(crate::keybindings::customizable_key_bindings(
            &crate::settings::Settings::default(),
        ));
    });

    set_editor_text(
        &harness,
        "SELECT 1 AS id, 'first value' AS v UNION ALL SELECT 2, 'second'",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    let table_state = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .read(cx)
            .table_state()
            .clone()
    });
    let copy = |selection: TableSelection, cx: &mut VisualTestContext| {
        table_state.update_in(cx, |state, window, cx| {
            state.set_selection(selection, cx);
            state.focus_handle(cx).focus(window, cx);
        });
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
        cx.simulate_keystrokes("secondary-c");
        cx.run_until_parked();
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default()
    };

    assert_eq!(copy(TableSelection::Cell(0, 1), &mut cx), "first value");
    let rows = copy(TableSelection::Row(1), &mut cx);
    assert!(rows.contains("2\tsecond"), "{rows:?}");
    assert_eq!(copy(TableSelection::Column(0), &mut cx), "1\n2");

    // While a cell is edited, the shortcut copies the editor's selection.
    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    });
    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    let input = table_state.read_with(&cx, |state, _| {
        match state.delegate().edit_state.get_editing_input() {
            Some(CellInput::Inline(input)) => input,
            _ => panic!("a fresh cell edit is inline"),
        }
    });
    input.update_in(&mut cx, |input, window, cx| {
        input.set_selected_range(0..5, window, cx);
    });
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(String::new()));
    cx.simulate_keystrokes("secondary-c");
    cx.run_until_parked();
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("first".to_string())
    );
}

/// Copy as HTML writes the selected row as an escaped table, and the cell
/// menu builds with its Copy as and Export as submenus.
#[gpui::test]
async fn test_copy_as_html_and_cell_menu(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "SELECT 1 AS id, '<b>' AS v, NULL AS note",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    });
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());
    table_state.update(&mut cx, |state, cx| {
        state.set_selection(TableSelection::Cell(0, 1), cx);
    });

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.on_copy_as_html(&crate::app::CopyAsHTML, window, cx);
    });
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        copied.contains("<tr><th>id</th><th>v</th><th>note</th></tr>")
            && copied.contains("<tr><td>1</td><td>&lt;b&gt;</td><td>NULL</td></tr>"),
        "{copied}"
    );

    let menu = cx.update(|window, cx| {
        PopupMenu::build(window, cx, move |menu, window, cx| {
            table_state.update(cx, |state, cx| {
                state.delegate_mut().context_menu((0, 1), menu, window, cx)
            })
        })
    });
    assert!(!menu.read_with(&cx, |menu, _| menu.is_empty()));
}

/// Dates read in the configured display format, but the cell editor, the
/// copy paths and the picker all work with the text the driver returned.
#[gpui::test]
async fn test_date_display_format_and_picker_keep_values_raw(cx: &mut TestAppContext) {
    use chrono::{NaiveDate, NaiveTime};

    use crate::app_settings::AppSettings;
    use crate::results_panel::results_table_delegate::results_settings;
    use crate::results_panel::temporal::{TemporalShape, TemporalValue};
    use crate::settings::DateDisplayFormat;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    for statement in [
        "CREATE TABLE events (id INTEGER PRIMARY KEY, happened DATETIME, day DATE)",
        "INSERT INTO events (id, happened, day) VALUES (1, '2024-01-31 14:05:09', '2024-01-31')",
        "SELECT * FROM events",
    ] {
        set_editor_text(&harness, statement, &mut cx);
        run_query(&harness, &mut cx);
        wait_for_query(&harness, &mut cx).await;
    }
    let results_panel = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .clone()
    });
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    cx.update(|_, cx| {
        AppSettings::global_mut(cx).settings.results.date_format = DateDisplayFormat::DayMonthYear;
        cx.refresh_windows();
    });
    cx.run_until_parked();

    let raw = "2024-01-31 14:05:09";
    assert_eq!(
        result_cell(&harness, 0, 1, &cx),
        Some(Some(raw.to_string()))
    );
    let (shown, shape) = table_state.read_with(&cx, |state, cx| {
        let delegate = state.delegate();
        (
            delegate.display_value(1, raw, &results_settings(cx)).text,
            delegate.temporal_shape(0, 1),
        )
    });
    assert_eq!(shown, "31/01/2024 14:05:09");
    assert_eq!(shape, Some(TemporalShape::Timestamp));

    let copied = results_panel.read_with(&cx, |panel, cx| {
        let delegate = table_state.read(cx).delegate();
        panel.get_selected_data_for_rows(&std::collections::HashSet::from([0]), delegate)
    });
    let copied_value = copied
        .selected_rows
        .first()
        .and_then(|row| row.cells.get(1))
        .and_then(|cell| cell.value.clone());
    assert_eq!(
        copied_value.as_deref(),
        Some(raw),
        "copy uses the stored text"
    );

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    let input_text = |cx: &VisualTestContext| {
        table_state.read_with(cx, |state, cx| {
            state
                .delegate()
                .edit_state
                .get_editing_input()
                .expect("editing input should exist")
                .text(cx)
        })
    };
    assert_eq!(input_text(&cx), raw, "the editor opens on the stored text");

    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::open_date_time_picker(state, TemporalShape::Timestamp, window, cx);
    });
    cx.run_until_parked();
    let picker = table_state
        .read_with(&cx, |state, _| state.delegate().date_time_picker())
        .expect("the picker should be open");
    let still_editing =
        table_state.read_with(&cx, |state, _| state.delegate().edit_state.editing_cell);
    assert_eq!(
        still_editing,
        Some((0, 1)),
        "moving focus into the picker must not end the edit"
    );

    picker.update_in(&mut cx, |picker, window, cx| {
        picker.select(
            NaiveDate::from_ymd_opt(2024, 2, 1).expect("valid date"),
            NaiveTime::from_hms_opt(8, 30, 0).expect("valid time"),
            window,
            cx,
        );
        picker.apply(cx);
    });
    cx.run_until_parked();

    let picker_closed = table_state.read_with(&cx, |state, _| {
        state.delegate().date_time_picker().is_none()
    });
    assert!(picker_closed, "applying closes the picker");
    assert_eq!(input_text(&cx), "2024-02-01 08:30:00");

    let statements = results_panel.update(&mut cx, |panel, cx| {
        panel.finalize_active_cell_edit(cx);
        panel.preview_pending_sql(cx)
    });
    assert_eq!(
        statements,
        vec![
            r#"UPDATE "events" SET "happened" = '2024-02-01 08:30:00' WHERE "id" = '1'"#
                .to_string()
        ],
    );

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 2, window, cx);
    });
    cx.run_until_parked();
    let date_shape = table_state.read_with(&cx, |state, _| state.delegate().temporal_shape(0, 2));
    assert_eq!(date_shape, Some(TemporalShape::Date));
    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::open_date_time_picker(state, TemporalShape::Date, window, cx);
    });
    cx.run_until_parked();
    let picker = table_state
        .read_with(&cx, |state, _| state.delegate().date_time_picker())
        .expect("the picker should be open");
    picker.update(&mut cx, |picker, cx| picker.pick_now(cx));
    cx.run_until_parked();
    let picked = TemporalValue::parse(&input_text(&cx)).map(|value| value.shape());
    assert_eq!(
        picked,
        Some(TemporalShape::Date),
        "Now on a date cell is today"
    );
}

/// A 1x1 PNG, long enough that its cell opens in the viewer.
const PNG_HEX: &str = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415478da636460f85f0f0002870180eb47ba920000000049454e44ae426082";

async fn setup_binary_table(
    harness: &TestHarness,
    cx: &mut VisualTestContext,
) -> gpui::Entity<crate::results_panel::ResultsPanel> {
    for sql in [
        "CREATE TABLE files (id INTEGER PRIMARY KEY, body BLOB)".to_string(),
        format!("INSERT INTO files (id, body) VALUES (1, X'{PNG_HEX}'), (2, X'0102')"),
        "SELECT * FROM files".to_string(),
    ] {
        set_editor_text(harness, &sql, cx);
        run_query(harness, cx);
        wait_for_query(harness, cx).await;
    }
    harness.editor_panel.read_with(cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab should exist")
            .results_panel
            .clone()
    })
}

/// A long binary value opens in the viewer, which recognises and decodes
/// the image. Looking at it is not an edit; loading a file into it is.
#[gpui::test]
async fn test_binary_cell_opens_in_the_viewer(cx: &mut TestAppContext) {
    use crate::results_panel::binary_viewer::{BinaryViewerEvent, ImageKind};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    let results_panel = setup_binary_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());
    assert_eq!(
        result_cell(&harness, 0, 1, &cx).flatten(),
        Some(format!("0x{PNG_HEX}"))
    );

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    let viewer = table_state
        .read_with(&cx, |state, _| {
            let edit_state = &state.delegate().edit_state;
            assert!(edit_state.get_editing_input().is_none());
            edit_state.binary_viewer.clone()
        })
        .expect("a long binary value opens in the viewer");
    assert_eq!(
        viewer.read_with(&cx, |viewer, _| viewer.image_kind()),
        Some(ImageKind::Png)
    );
    // The decode runs off the main thread.
    for _ in 0..200 {
        if viewer.read_with(&cx, |viewer, _| viewer.has_preview()) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
        cx.run_until_parked();
    }
    assert!(viewer.read_with(&cx, |viewer, _| viewer.has_preview()));

    results_panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    cx.run_until_parked();
    assert!(results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx).is_empty()));
    assert!(table_state.read_with(&cx, |state, _| {
        state.delegate().edit_state.binary_viewer.is_none()
    }));

    results_panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    let viewer = table_state
        .read_with(&cx, |state, _| {
            state.delegate().edit_state.binary_viewer.clone()
        })
        .expect("viewer reopens");
    viewer.update(&mut cx, |_, cx| {
        cx.emit(BinaryViewerEvent::Loaded(std::sync::Arc::new(vec![
            0x00, 0xff, 0x10,
        ])));
    });
    cx.run_until_parked();
    results_panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    cx.run_until_parked();
    assert_eq!(
        results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx)),
        vec![r#"UPDATE "files" SET "body" = X'00ff10' WHERE "id" = '1'"#.to_string()]
    );
}

/// A short binary value edits inline as hex, is written as bytes, and hex
/// that is not whole bytes is refused before anything is sent.
#[gpui::test]
async fn test_short_binary_cell_edits_inline_as_hex(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    let results_panel = setup_binary_table(&harness, &mut cx).await;
    let table_state = results_panel.read_with(&cx, |panel, _| panel.table_state().clone());

    let edit = |text: &'static str, cx: &mut VisualTestContext| {
        results_panel.update_in(cx, |panel, window, cx| {
            panel.start_cell_edit(1, 1, window, cx);
        });
        cx.run_until_parked();
        let input = table_state.read_with(cx, |state, _| {
            match state.delegate().edit_state.get_editing_input() {
                Some(CellInput::Inline(input)) => input,
                _ => panic!("a short binary value edits inline"),
            }
        });
        input.update_in(cx, |input, window, cx| {
            input.replace_all(text, window, cx);
        });
        cx.run_until_parked();
        results_panel.update(cx, |panel, cx| panel.finalize_active_cell_edit(cx));
        cx.run_until_parked();
    };

    edit("0xCAFE", &mut cx);
    assert_eq!(
        results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx)),
        vec![r#"UPDATE "files" SET "body" = X'cafe' WHERE "id" = '2'"#.to_string()]
    );
    assert_eq!(
        table_state.read_with(&cx, |state, _| state.delegate().invalid_binary_edit()),
        None
    );

    edit("0xCAF", &mut cx);
    let problem = table_state
        .read_with(&cx, |state, _| state.delegate().invalid_binary_edit())
        .expect("half a byte is refused");
    assert!(problem.contains("Row 2, body"), "{problem}");
}
