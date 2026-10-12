use blanco_core::ColumnType;
use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_component::table::TableState;

use super::{ResultsPanel, ResultsTableDelegate};
use crate::app::DuplicateRow;
use crate::app_settings::AppSettings;
use crate::test_harness::{TestHarness, result_cell, run_query, set_editor_text, wait_for_query};

const BODY_LENGTH: usize = 5000;
const DATA_LENGTH: usize = 3000;

async fn run(harness: &TestHarness, sql: &str, cx: &mut VisualTestContext) {
    set_editor_text(harness, sql, cx);
    run_query(harness, cx);
    wait_for_query(harness, cx).await;
}

/// A table with one small row and one row holding large text and binary,
/// read with values over 1 KB deferred.
async fn open_documents(
    harness: &TestHarness,
    cx: &mut VisualTestContext,
) -> (
    Entity<ResultsPanel>,
    Entity<TableState<ResultsTableDelegate>>,
) {
    cx.update(|_, cx| {
        AppSettings::global_mut(cx)
            .settings
            .results
            .defer_values_over_kb = 1;
    });
    run(
        harness,
        "CREATE TABLE documents (id INTEGER PRIMARY KEY, title TEXT, body TEXT, data BLOB)",
        cx,
    )
    .await;
    run(
        harness,
        &format!(
            "INSERT INTO documents VALUES (1, 'small', 'short', X'0102'), \
             (2, 'large', replace(hex(zeroblob({})), '00', 'ab'), zeroblob({DATA_LENGTH}))",
            BODY_LENGTH / 2
        ),
        cx,
    )
    .await;
    run(harness, "SELECT * FROM documents", cx).await;
    let panel = harness.editor_panel.read_with(cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    });
    let table_state = panel.read_with(cx, |panel, _| panel.table_state().clone());
    (panel, table_state)
}

async fn wait_for_deferred_count(
    table_state: &Entity<TableState<ResultsTableDelegate>>,
    expected: usize,
    cx: &mut VisualTestContext,
) {
    for _ in 0..200 {
        cx.run_until_parked();
        if table_state.read_with(cx, |state, _| state.delegate().deferred_values().len())
            == expected
        {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    panic!("deferred values did not reach {expected}");
}

fn cell_length(harness: &TestHarness, row: usize, col: usize, cx: &VisualTestContext) -> usize {
    result_cell(harness, row, col, cx)
        .flatten()
        .map_or(0, |value| value.chars().count())
}

/// Opening a table leaves its large values on the server: the grid holds a
/// preview and the size, keeps the columns' types, and leaves small values
/// and other queries alone.
#[gpui::test]
async fn large_values_are_read_as_previews(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();
    let (_panel, table_state) = open_documents(&harness, &mut cx).await;

    table_state.read_with(&cx, |state, _| {
        let delegate = state.delegate();
        assert_eq!(delegate.deferred_values().len(), 2);
        assert_eq!(delegate.deferred_size(1, 2), Some(BODY_LENGTH as u64));
        assert_eq!(delegate.deferred_size(1, 3), Some(DATA_LENGTH as u64));
        assert_eq!(delegate.deferred_size(0, 2), None);
        assert_eq!(delegate.deferred_size(1, 1), None);
        assert_eq!(
            delegate.column_types,
            vec![
                ColumnType::Integer,
                ColumnType::Text,
                ColumnType::Text,
                ColumnType::Binary
            ]
        );
        assert_eq!(delegate.columns.len(), 4, "helper columns are not shown");
        assert!(delegate.is_editable());
    });
    assert_eq!(cell_length(&harness, 1, 2, &cx), 256);
    // `0x` and two digits for each of the 256 preview bytes.
    assert_eq!(cell_length(&harness, 1, 3, &cx), 2 + 512);
    assert_eq!(
        result_cell(&harness, 0, 2, &cx).flatten().as_deref(),
        Some("short")
    );

    // A query naming its columns is the user's and is read in full.
    run(&harness, "SELECT id, body FROM documents", &mut cx).await;
    assert_eq!(cell_length(&harness, 1, 1, &cx), BODY_LENGTH);
}

/// Opening a deferred cell fetches that value and nothing else, and the
/// editor opens on the full text.
#[gpui::test]
async fn opening_a_deferred_cell_loads_its_value(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();
    let (panel, table_state) = open_documents(&harness, &mut cx).await;

    panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(1, 2, window, cx);
    });
    wait_for_deferred_count(&table_state, 1, &mut cx).await;
    assert_eq!(cell_length(&harness, 1, 2, &cx), BODY_LENGTH);
    table_state.read_with(&cx, |state, cx| {
        let delegate = state.delegate();
        assert_eq!(delegate.deferred_size(1, 2), None);
        assert_eq!(delegate.deferred_size(1, 3), Some(DATA_LENGTH as u64));
        let input = delegate
            .edit_state
            .get_editing_input()
            .expect("the cell is being edited once its value is here");
        assert_eq!(input.text(cx).chars().count(), BODY_LENGTH);
    });
    // Looking is not editing.
    panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    assert!(panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx).is_empty()));

    // The binary value opens in the viewer, complete.
    panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(1, 3, window, cx);
    });
    wait_for_deferred_count(&table_state, 0, &mut cx).await;
    assert_eq!(cell_length(&harness, 1, 3, &cx), 2 + DATA_LENGTH * 2);
    assert!(table_state.read_with(&cx, |state, _| {
        state.delegate().edit_state.binary_viewer.is_some()
    }));
}

/// A duplicated row is inserted with its values, not with the previews the
/// grid was showing.
#[gpui::test]
async fn duplicating_a_row_loads_its_values_first(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();
    let (panel, table_state) = open_documents(&harness, &mut cx).await;

    panel.update_in(&mut cx, |panel, window, cx| {
        panel.on_duplicate_row(&DuplicateRow { row: 1 }, window, cx);
    });
    wait_for_deferred_count(&table_state, 0, &mut cx).await;

    let statements = panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    let [insert] = statements.as_slice() else {
        panic!("one INSERT expected, got {statements:?}");
    };
    assert!(insert.starts_with(r#"INSERT INTO "documents""#), "{insert}");
    assert!(insert.contains(&"ab".repeat(BODY_LENGTH / 2)));
    assert!(insert.contains(&format!("X'{}'", "00".repeat(DATA_LENGTH))));
}
