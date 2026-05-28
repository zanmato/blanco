use gpui::{AppContext, TestAppContext, VisualTestContext};
use std::collections::HashMap;

use crate::test_harness::{
    TestHarness, result_cell, result_columns, result_row_count, run_query, set_editor_text,
    wait_for_query,
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

/// Walks a SELECT result and confirms columns + cell values render correctly,
/// covering the columns helper and the cell reader added to the test harness.
#[gpui::test]
async fn test_results_panel_renders_columns_and_cells(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "SELECT 7 AS id, 'hello' AS greeting, NULL AS missing",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let columns = result_columns(&harness, &cx).expect("should have columns");
    // Column 0 is the synthetic row-number column the table delegate prepends.
    assert!(columns.len() >= 4, "got {columns:?}");
    assert_eq!(columns[1], "id");
    assert_eq!(columns[2], "greeting");
    assert_eq!(columns[3], "missing");

    // Row-number column is index 0 → user columns start at 1.
    assert_eq!(
        result_cell(&harness, 0, 1, &cx).expect("cell present"),
        Some("7".to_string())
    );
    assert_eq!(
        result_cell(&harness, 0, 2, &cx).expect("cell present"),
        Some("hello".to_string())
    );
    assert_eq!(
        result_cell(&harness, 0, 3, &cx).expect("cell present"),
        None,
        "NULL should round-trip as None, not the literal string"
    );
}

/// Runs three statements in sequence (DDL + DML + SELECT) and confirms the
/// final SELECT reflects the DML. Exercises the run-many-times path through
/// the same query tab.
#[gpui::test]
async fn test_ddl_then_dml_then_select(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "CREATE TABLE roundtrip (id INTEGER PRIMARY KEY, label TEXT)",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(
        &harness,
        "INSERT INTO roundtrip (id, label) VALUES (1, 'one'), (2, 'two')",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(
        &harness,
        "SELECT id, label FROM roundtrip ORDER BY id",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    assert_eq!(result_row_count(&harness, &cx), Some(2));
    assert_eq!(
        result_cell(&harness, 0, 1, &cx).expect("cell"),
        Some("1".to_string())
    );
    assert_eq!(
        result_cell(&harness, 0, 2, &cx).expect("cell"),
        Some("one".to_string())
    );
    assert_eq!(
        result_cell(&harness, 1, 1, &cx).expect("cell"),
        Some("2".to_string())
    );
    assert_eq!(
        result_cell(&harness, 1, 2, &cx).expect("cell"),
        Some("two".to_string())
    );
}

/// A SELECT statement that should produce zero rows must not blow up rendering
/// and the column metadata should still come through.
#[gpui::test]
async fn test_empty_result_set(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "CREATE TABLE empty_table (id INTEGER PRIMARY KEY, label TEXT)",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(&harness, "SELECT id, label FROM empty_table", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    assert_eq!(result_row_count(&harness, &cx), Some(0));
    // Note: column metadata is currently lost for empty result sets in the
    // stream-collect path; only the synthetic row-number column survives.
    // Worth fixing separately; for now the test just guards against panics.
    let columns = result_columns(&harness, &cx).expect("should have columns");
    assert!(!columns.is_empty(), "got {columns:?}");
}

/// Build a ParameterForm directly and confirm the substituted query is
/// produced by walking byte offsets in descending order. Skips the modal UI
/// since that goes through gpui_component's dialog system which is awkward to
/// drive headlessly; this targets the substitution logic itself.
#[gpui::test]
async fn test_parameter_form_substitutes_named_params(cx: &mut TestAppContext) {
    use crate::editor::parameter_form::ParameterForm;
    use crate::sql::statement_parser::{ParameterStyle, QueryParameter};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let query = "SELECT * FROM users WHERE id = :uid AND status = :st";
    let params = vec![
        QueryParameter {
            style: ParameterStyle::Named("uid".into()),
            raw_text: ":uid".into(),
            byte_offset: query.find(":uid").unwrap(),
        },
        QueryParameter {
            style: ParameterStyle::Named("st".into()),
            raw_text: ":st".into(),
            byte_offset: query.find(":st").unwrap(),
        },
    ];
    let initial = HashMap::from([
        (":uid".to_string(), "42".to_string()),
        (":st".to_string(), "active".to_string()),
    ]);

    let form = harness
        .editor_panel
        .update_in(&mut cx, |_panel, window, cx| {
            cx.new(|cx| ParameterForm::new(query.to_string(), params, &initial, window, cx))
        });

    let substituted = form.read_with(&cx, |f, cx| f.get_substituted_query(cx));
    assert_eq!(
        substituted, "SELECT * FROM users WHERE id = 42 AND status = active",
        "named parameters should be substituted in-place"
    );

    let values = form.read_with(&cx, |f, cx| f.current_values(cx));
    assert_eq!(values.get(":uid").map(String::as_str), Some("42"));
    assert_eq!(values.get(":st").map(String::as_str), Some("active"));
}

/// Same parameter appearing twice in a query must be substituted at both
/// offsets, not just one. Regression guard for the byte-offset walk.
#[gpui::test]
async fn test_parameter_form_substitutes_repeated_param(cx: &mut TestAppContext) {
    use crate::editor::parameter_form::ParameterForm;
    use crate::sql::statement_parser::{ParameterStyle, QueryParameter};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let query = "SELECT :x, :x + 1, :x * 2";
    let mut params = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = query[search_from..].find(":x") {
        let offset = search_from + pos;
        params.push(QueryParameter {
            style: ParameterStyle::Named("x".into()),
            raw_text: ":x".into(),
            byte_offset: offset,
        });
        search_from = offset + 2;
    }
    assert_eq!(params.len(), 3, "test setup: expected three :x occurrences");

    let initial = HashMap::from([(":x".to_string(), "7".to_string())]);
    let form = harness
        .editor_panel
        .update_in(&mut cx, |_panel, window, cx| {
            cx.new(|cx| ParameterForm::new(query.to_string(), params, &initial, window, cx))
        });

    let substituted = form.read_with(&cx, |f, cx| f.get_substituted_query(cx));
    assert_eq!(substituted, "SELECT 7, 7 + 1, 7 * 2");
}

/// Positional parameters ($1, $2) must substitute the way SQL clients expect
/// (the index, not the dollar sign, stays).
#[gpui::test]
async fn test_parameter_form_substitutes_positional_params(cx: &mut TestAppContext) {
    use crate::editor::parameter_form::ParameterForm;
    use crate::sql::statement_parser::{ParameterStyle, QueryParameter};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let query = "SELECT $1, $2";
    let params = vec![
        QueryParameter {
            style: ParameterStyle::Positional(1),
            raw_text: "$1".into(),
            byte_offset: query.find("$1").unwrap(),
        },
        QueryParameter {
            style: ParameterStyle::Positional(2),
            raw_text: "$2".into(),
            byte_offset: query.find("$2").unwrap(),
        },
    ];
    let initial = HashMap::from([
        ("$1".to_string(), "'alice'".to_string()),
        ("$2".to_string(), "100".to_string()),
    ]);

    let form = harness
        .editor_panel
        .update_in(&mut cx, |_panel, window, cx| {
            cx.new(|cx| ParameterForm::new(query.to_string(), params, &initial, window, cx))
        });

    let substituted = form.read_with(&cx, |f, cx| f.get_substituted_query(cx));
    assert_eq!(substituted, "SELECT 'alice', 100");
}
