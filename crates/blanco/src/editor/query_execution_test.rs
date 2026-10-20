use gpui::{AppContext, TestAppContext, VisualTestContext};
use std::collections::HashMap;

use crate::status_bar::{ActivityReporter, ActivityResult, StatusKind};
use crate::test_harness::{
    TestHarness, result_cell, result_columns, result_row_count, run_query, set_editor_text,
    status_line, wait_for_query,
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

/// After a query completes, the status bar should flash an `Ok` outcome that
/// names the row count, then linger (the transient timer keeps it for a few
/// seconds before returning to `Ready`).
#[gpui::test]
async fn test_status_bar_reports_query_outcome(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 1 AS n UNION ALL SELECT 2", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    cx.run_until_parked();

    let line = status_line(&harness, &cx);
    assert_eq!(line.kind, StatusKind::Ok, "got text {:?}", line.text);
    assert!(
        line.text.starts_with("Query OK · 2 rows ·"),
        "unexpected status text: {:?}",
        line.text
    );
}

/// Drive the global reporter directly to exercise the display() folding logic:
/// the latest activity is shown with a `(+N)` overflow count, finishing one
/// flashes its outcome only once the bar is idle, and the line returns to
/// `Ready` once every guard is gone (transient aside).
#[gpui::test]
async fn test_status_bar_tracks_concurrent_activities(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let reporter = cx.update(|_, cx| ActivityReporter::global(cx));

    let first = reporter.begin("alpha: executing query");
    cx.run_until_parked();
    let line = status_line(&harness, &cx);
    assert_eq!(line.kind, StatusKind::Busy);
    assert_eq!(line.text.as_ref(), "alpha: executing query");

    let second = reporter.begin("beta: listing schema");
    cx.run_until_parked();
    let line = status_line(&harness, &cx);
    assert_eq!(line.kind, StatusKind::Busy);
    assert_eq!(
        line.text.as_ref(),
        "beta: listing schema (+1)",
        "newest activity wins and the older one shows as overflow"
    );

    // Finishing the foreground activity flashes its outcome, but the still-busy
    // `first` keeps the bar in the Busy state.
    second.finish(ActivityResult::Ok("done".into()));
    cx.run_until_parked();
    let line = status_line(&harness, &cx);
    assert_eq!(line.kind, StatusKind::Busy);
    assert_eq!(line.text.as_ref(), "alpha: executing query");

    // Dropping the last guard clears active work; the lingering transient from
    // `second` is what the bar shows next.
    drop(first);
    cx.run_until_parked();
    let line = status_line(&harness, &cx);
    assert_eq!(line.kind, StatusKind::Ok);
    assert_eq!(line.text.as_ref(), "done");
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
    // Row numbers live in the table's row header column now, so the delegate
    // exposes only the user's columns starting at index 0.
    assert!(columns.len() >= 3, "got {columns:?}");
    assert_eq!(columns[0], "id");
    assert_eq!(columns[1], "greeting");
    assert_eq!(columns[2], "missing");

    assert_eq!(
        result_cell(&harness, 0, 0, &cx).expect("cell present"),
        Some("7".to_string())
    );
    assert_eq!(
        result_cell(&harness, 0, 1, &cx).expect("cell present"),
        Some("hello".to_string())
    );
    assert_eq!(
        result_cell(&harness, 0, 2, &cx).expect("cell present"),
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
        result_cell(&harness, 0, 0, &cx).expect("cell"),
        Some("1".to_string())
    );
    assert_eq!(
        result_cell(&harness, 0, 1, &cx).expect("cell"),
        Some("one".to_string())
    );
    assert_eq!(
        result_cell(&harness, 1, 0, &cx).expect("cell"),
        Some("2".to_string())
    );
    assert_eq!(
        result_cell(&harness, 1, 1, &cx).expect("cell"),
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
    // The driver only learns the header from the first row, so the editor
    // rebuilds it from the statement description when nothing matched.
    let columns = result_columns(&harness, &cx).expect("columns vec should be present");
    assert_eq!(columns, vec!["id".to_string(), "label".to_string()]);
}

/// Build a ParameterForm directly and confirm the substituted query is
/// produced by walking byte offsets in descending order. Skips the modal UI
/// since that goes through gpui_component's dialog system which is awkward to
/// drive headlessly; this targets the substitution logic itself.
#[gpui::test]
async fn test_parameter_form_substitutes_named_params(cx: &mut TestAppContext) {
    use crate::editor::parameter_form::ParameterForm;
    use sql_parser::statement_parser::{ParameterStyle, QueryParameter};

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
        (":uid".to_string(), typed("42")),
        (":st".to_string(), typed("active")),
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
    assert_eq!(values.get(":uid"), Some(&typed("42")));
    assert_eq!(values.get(":st"), Some(&typed("active")));
}

/// Same parameter appearing twice in a query must be substituted at both
/// offsets, not just one. Regression guard for the byte-offset walk.
#[gpui::test]
async fn test_parameter_form_substitutes_repeated_param(cx: &mut TestAppContext) {
    use crate::editor::parameter_form::ParameterForm;
    use sql_parser::statement_parser::{ParameterStyle, QueryParameter};

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

    let initial = HashMap::from([(":x".to_string(), typed("7"))]);
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
    use sql_parser::statement_parser::{ParameterStyle, QueryParameter};

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
        ("$1".to_string(), typed("'alice'")),
        ("$2".to_string(), typed("100")),
    ]);

    let form = harness
        .editor_panel
        .update_in(&mut cx, |_panel, window, cx| {
            cx.new(|cx| ParameterForm::new(query.to_string(), params, &initial, window, cx))
        });

    let substituted = form.read_with(&cx, |f, cx| f.get_substituted_query(cx));
    assert_eq!(substituted, "SELECT 'alice', 100");
}

fn typed(text: &str) -> crate::editor::parameter_form::ParameterValue {
    crate::editor::parameter_form::ParameterValue {
        text: text.to_string(),
        empty_string: false,
    }
}

/// An empty field binds NULL unless its toggle asks for an empty string, and
/// Clear empties every field and turns the toggles off again.
#[gpui::test]
async fn test_parameter_form_binds_null_or_empty_string_and_clears(cx: &mut TestAppContext) {
    use crate::editor::parameter_form::{ParameterForm, ParameterValue};
    use sql_parser::statement_parser::{ParameterStyle, QueryParameter};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let query = "SELECT $1, $2, $3";
    let params = (1..=3)
        .map(|n| {
            let raw_text = format!("${n}");
            QueryParameter {
                style: ParameterStyle::Positional(n),
                byte_offset: query.find(&raw_text).unwrap(),
                raw_text,
            }
        })
        .collect();
    let deliberate_empty = ParameterValue {
        text: String::new(),
        empty_string: true,
    };
    let initial = HashMap::from([
        ("$1".to_string(), typed("1")),
        ("$2".to_string(), deliberate_empty.clone()),
    ]);

    let form = harness
        .editor_panel
        .update_in(&mut cx, |_panel, window, cx| {
            cx.new(|cx| ParameterForm::new(query.to_string(), params, &initial, window, cx))
        });

    let substituted = form.read_with(&cx, |f, cx| f.get_substituted_query(cx));
    assert_eq!(substituted, "SELECT 1, '', NULL");
    let values = form.read_with(&cx, |f, cx| f.current_values(cx));
    assert_eq!(
        values.get("$2"),
        Some(&deliberate_empty),
        "the empty string toggle is remembered with the value"
    );
    assert_eq!(values.get("$3"), Some(&ParameterValue::default()));

    form.update_in(&mut cx, |f, window, cx| f.clear(window, cx));
    let substituted = form.read_with(&cx, |f, cx| f.get_substituted_query(cx));
    assert_eq!(substituted, "SELECT NULL, NULL, NULL");
    let values = form.read_with(&cx, |f, cx| f.current_values(cx));
    assert!(
        values
            .values()
            .all(|value| *value == ParameterValue::default())
    );
}

/// Poll the persisted query history until it reaches the expected length. The
/// recording insert runs just after the results land, so a short retry loop
/// avoids racing the background write.
async fn wait_for_history(
    cx: &mut VisualTestContext,
    expected_len: usize,
) -> Vec<app_database::QueryHistoryData> {
    use app_database::AppDatabase;

    for _ in 0..50 {
        let history = cx.update(|_window, cx| {
            let db = AppDatabase::global(cx).clone();
            gpui_tokio::Tokio::handle(cx)
                .block_on(async move { db.load_query_history(100, None).await })
        });
        if let Ok(history) = history
            && history.len() >= expected_len
        {
            return history;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("query history did not reach {expected_len} entries within timeout");
}

/// A successful execution is recorded in the history log with its outcome.
#[gpui::test]
async fn test_successful_query_records_history(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 42 AS answer", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let history = wait_for_history(&mut cx, 1).await;
    assert_eq!(history.len(), 1);
    assert!(history[0].success, "expected a successful entry");
    assert!(
        history[0].query_text.contains("SELECT 42"),
        "unexpected query text: {:?}",
        history[0].query_text
    );
    assert_eq!(history[0].row_count, Some(1));
}

/// A failed execution is recorded with `success = false` and the error message.
#[gpui::test]
async fn test_failed_query_records_history(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT * FROM no_such_table_xyz", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let history = wait_for_history(&mut cx, 1).await;
    assert_eq!(history.len(), 1);
    assert!(!history[0].success, "expected a failed entry");
    assert!(
        history[0].error_message.is_some(),
        "failed entry should carry an error message"
    );
}

/// Pruning keeps only the newest N entries and treats 0 as unlimited.
#[gpui::test]
async fn test_prune_query_history_keeps_newest(cx: &mut TestAppContext) {
    use app_database::{AppDatabase, QueryHistoryData};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let record = |query: &str, ts: i64| QueryHistoryData {
        id: None,
        query_text: query.to_string(),
        executed_at: ts,
        duration_ms: Some(1),
        execution_us: Some(400),
        fetch_us: None,
        rows_affected: Some(0),
        row_count: Some(0),
        success: true,
        error_message: None,
        connection_id: None,
        connection_name: None,
        database_name: None,
    };

    let remaining = cx.update(|_window, cx| {
        let db = AppDatabase::global(cx).clone();
        gpui_tokio::Tokio::handle(cx).block_on(async move {
            for n in 0..5 {
                db.record_query_history(&record(&format!("SELECT {n}"), 1000 + n))
                    .await
                    .unwrap();
            }
            // 0 means unlimited: nothing is removed.
            db.prune_query_history(0).await.unwrap();
            assert_eq!(db.load_query_history(100, None).await.unwrap().len(), 5);

            db.prune_query_history(2).await.unwrap();
            db.load_query_history(100, None).await.unwrap()
        })
    });

    assert_eq!(remaining.len(), 2, "prune should keep exactly 2 entries");
    assert_eq!(remaining[0].query_text, "SELECT 4", "newest first");
    assert_eq!(remaining[1].query_text, "SELECT 3");
}

/// History search filters by query text and is case-insensitive.
#[gpui::test]
async fn test_history_search_filters_entries(cx: &mut TestAppContext) {
    use app_database::AppDatabase;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 1 AS apple", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(&harness, "SELECT 2 AS banana", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    wait_for_history(&mut cx, 2).await;

    let matches = cx.update(|_window, cx| {
        let db = AppDatabase::global(cx).clone();
        gpui_tokio::Tokio::handle(cx).block_on(async move {
            db.load_query_history(100, Some("BANANA".to_string()))
                .await
                .unwrap()
        })
    });

    assert_eq!(matches.len(), 1, "only the banana query should match");
    assert!(matches[0].query_text.contains("banana"));
}

/// A run logs the driver's execution and fetch times and keeps them in the
/// history, and a statement without rows has no fetch time.
#[gpui::test]
async fn test_run_keeps_execution_and_fetch_time(cx: &mut TestAppContext) {
    use app_database::AppDatabase;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "SELECT 1 AS apple", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(&harness, "CREATE TABLE timed (id INTEGER)", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let log = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .map(|tab| tab.sql_view.read(cx).all_text())
            .unwrap_or_default()
    });
    let summaries: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(" rows in "))
        .collect();
    let [select, create] = summaries.as_slice() else {
        panic!("expected two run summaries in the log:\n{log}");
    };
    assert!(
        select.contains("(execution ") && select.contains(" · fetch "),
        "a read should log both times: {select}"
    );
    assert!(
        create.contains("(execution ") && !create.contains("fetch"),
        "a write should log only its execution time: {create}"
    );

    wait_for_history(&mut cx, 2).await;
    let history = cx.update(|_window, cx| {
        let db = AppDatabase::global(cx).clone();
        gpui_tokio::Tokio::handle(cx)
            .block_on(async move { db.load_query_history(100, None).await.unwrap() })
    });
    let [create, select] = history.as_slice() else {
        panic!("expected two history entries, got {}", history.len());
    };
    assert!(create.execution_us.is_some());
    assert_eq!(create.fetch_us, None, "a write has no fetch time");
    assert!(select.execution_us.is_some());
    assert!(select.fetch_us.is_some());
}

/// Writes on a PROD-tagged tab must wait for confirmation: running one opens a
/// dialog instead of executing, so the statement never reaches the database.
#[gpui::test]
async fn test_prod_write_requires_confirmation(cx: &mut TestAppContext) {
    let harness = TestHarness::new_with_environment(cx, app_database::EnvironmentType::PROD);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(&harness, "CREATE TABLE guarded (id INTEGER)", &mut cx);
    run_query(&harness, &mut cx);
    cx.run_until_parked();
    assert!(
        !crate::test_harness::is_loading(&harness, &cx),
        "a guarded write must not start executing"
    );

    // Reads are unaffected and prove the table was never created.
    set_editor_text(
        &harness,
        "SELECT name FROM sqlite_master WHERE name = 'guarded'",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    assert_eq!(result_row_count(&harness, &cx), Some(0));
}

/// An EXPLAIN whose output the backend returns in structured form lands as a
/// plan tree and the tab opens in plan view, with the raw rows still loaded
/// for the table toggle.
#[gpui::test]
async fn test_explain_produces_plan_tree(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    set_editor_text(
        &harness,
        "CREATE TABLE plans (id INTEGER PRIMARY KEY)",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(&harness, "SELECT * FROM plans WHERE id = 1", &mut cx);
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.on_explain_query(blanco_core::ExplainMode::Estimated, window, cx);
        });
    wait_for_query(&harness, &mut cx).await;

    let (plan, view_mode) = harness.editor_panel.read_with(&cx, |panel, cx| {
        let tab = panel.active_query_tab().expect("query tab");
        tab.results_panel.read_with(cx, |results, _| {
            (results.active_plan(), results.active_view_mode())
        })
    });
    let plan = plan.expect("EXPLAIN QUERY PLAN should parse into a tree");
    assert_eq!(view_mode, crate::results_panel::ResultViewMode::Plan);
    assert!(
        plan.roots[0].label.contains("plans"),
        "root node should name the table: {}",
        plan.roots[0].label
    );
    assert!(
        result_row_count(&harness, &cx).unwrap_or(0) > 0,
        "raw rows stay available"
    );
}

/// A role created from the editor keeps its password out of the history and
/// the SQL log, while the server gets it as typed: the new role logs in with
/// it. Runs against the compose PostgreSQL, and is skipped when it is down
/// unless `BLANCO_RUN_DB_TESTS=1`.
#[gpui::test]
async fn test_passwords_are_masked_in_history_and_log_but_not_sent_masked(cx: &mut TestAppContext) {
    use blanco_core::{ConnectionContext, DatabaseService as _};
    use database::{ConnectionConfig, DatabaseType};

    const CONFIG_ID: i64 = 9501;
    const ROLE_CONFIG_ID: i64 = 9502;
    const PASSWORD: &str = "hunter2-mask";
    let config = |id: i64, user: &str, password: &str| {
        ConnectionConfig::new(
            id,
            format!("it-{user}"),
            DatabaseType::PostgreSQL,
            "localhost".to_string(),
            5488,
            "blanco".to_string(),
            user.to_string(),
            Some(password.to_string()),
        )
    };

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let (service, handle) = cx.update(|_, cx| {
        (
            database::DatabaseService::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let reachable = handle.block_on(async {
        service
            .add_connection_config(config(CONFIG_ID, "blanco", "blanco"))
            .await;
        service
            .execute_script(
                CONFIG_ID,
                Some("blanco"),
                "DROP ROLE IF EXISTS blanco_mask_test",
            )
            .await
    });
    if let Err(error) = reachable {
        assert!(
            std::env::var("BLANCO_RUN_DB_TESTS").as_deref() != Ok("1"),
            "PostgreSQL unreachable: {error:#}"
        );
        return;
    }

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                crate::editor::TabCreationParams {
                    title: "roles".into(),
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context: ConnectionContext {
                        connection_id: CONFIG_ID,
                        db_type: DatabaseType::PostgreSQL,
                        connection_name: "it-blanco".into(),
                        database_name: "blanco".into(),
                        schema_name: None,
                        environment_type: None,
                    },
                },
                cx,
            );
        });
    let script = format!(
        "SELECT 'PASSWORD' AS kept;\nCREATE ROLE blanco_mask_test LOGIN PASSWORD '{PASSWORD}';\nSELECT 1"
    );
    set_editor_text(&harness, &script, &mut cx);
    // Run the whole script, as a selection does.
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            let tab = panel.active_query_tab().expect("query tab");
            tab.editor.update(cx, |editor, cx| {
                editor.set_selected_range(0..script.len(), window, cx);
            });
        });
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let masked = "SELECT 'PASSWORD' AS kept;\nCREATE ROLE blanco_mask_test LOGIN PASSWORD '********';\nSELECT 1";
    let history = wait_for_history(&mut cx, 1).await;
    assert_eq!(history[0].query_text, masked);
    let log = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .map(|tab| tab.sql_view.read(cx).all_text())
            .unwrap_or_default()
    });
    assert!(
        log.contains(masked),
        "the log should hold the masked text: {log}"
    );
    assert!(!log.contains(PASSWORD), "the log holds the password: {log}");

    let login = handle.block_on(async {
        service
            .add_connection_config(config(ROLE_CONFIG_ID, "blanco_mask_test", PASSWORD))
            .await;
        service
            .execute_script(ROLE_CONFIG_ID, Some("blanco"), "SELECT current_user")
            .await
    });
    handle
        .block_on(service.disconnect(ROLE_CONFIG_ID, None))
        .expect("close the role's connection");
    let current_user = login
        .unwrap_or_else(|error| panic!("the role should log in with its password: {error:#}"))
        .first()
        .and_then(|result| result.rows.first())
        .and_then(|row| row.first().cloned().flatten());
    assert_eq!(current_user.as_deref(), Some("blanco_mask_test"));

    handle
        .block_on(service.execute_script(CONFIG_ID, Some("blanco"), "DROP ROLE blanco_mask_test"))
        .expect("drop the role");
}
