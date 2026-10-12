use gpui::{TestAppContext, VisualTestContext};

use super::tabs::TabType;
use crate::test_harness::{TestHarness, result_cell, run_query, set_editor_text, wait_for_query};

fn tab_count(harness: &TestHarness, cx: &VisualTestContext) -> usize {
    harness
        .editor_panel
        .read_with(cx, |panel, _| panel.tabs.len())
}

fn active_file(harness: &TestHarness, cx: &VisualTestContext) -> Option<(String, bool, String)> {
    harness.editor_panel.read_with(cx, |panel, cx| {
        let tab = panel.tabs.get(panel.active_tab_ix)?.connection_tab()?;
        let file = tab.file.as_ref()?;
        Some((
            tab.title.clone(),
            file.dirty,
            tab.editor.read(cx).text().to_string(),
        ))
    })
}

#[gpui::test]
async fn open_edit_save_and_reopen_a_sql_file(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("report.sql");
    std::fs::write(&path, "SELECT 1;\n").unwrap();

    let tabs_before = tab_count(&harness, &cx);
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            let context = panel.active_context(cx).expect("query tab context");
            panel.open_sql_files(vec![path.clone()], context, window, cx)
        });
    cx.run_until_parked();
    assert_eq!(tab_count(&harness, &cx), tabs_before + 1);
    assert_eq!(
        active_file(&harness, &cx),
        Some(("report.sql".into(), false, "SELECT 1;\n".into()))
    );

    // Editing marks the tab unsaved; saving writes the file and clears it.
    set_editor_text(&harness, "SELECT 2;\n", &mut cx);
    cx.run_until_parked();
    assert_eq!(active_file(&harness, &cx).map(|file| file.1), Some(true));
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.save_active_tab(window, cx)
        });
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "SELECT 2;\n");
    assert_eq!(active_file(&harness, &cx).map(|file| file.1), Some(false));

    // Typing back to the saved text is not a change.
    set_editor_text(&harness, "SELECT 3;\n", &mut cx);
    set_editor_text(&harness, "SELECT 2;\n", &mut cx);
    cx.run_until_parked();
    assert_eq!(active_file(&harness, &cx).map(|file| file.1), Some(false));

    // Opening the file again brings its tab forward instead of reading it
    // into another one, so the tab's text is never replaced.
    set_editor_text(&harness, "SELECT 'unsaved';\n", &mut cx);
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.activate_tab(0, window, cx);
            let context = panel.active_context(cx).expect("query tab context");
            panel.open_sql_files(vec![path.clone()], context, window, cx);
        });
    cx.run_until_parked();
    assert_eq!(tab_count(&harness, &cx), tabs_before + 1);
    assert_eq!(
        active_file(&harness, &cx).map(|file| file.2),
        Some("SELECT 'unsaved';\n".into())
    );
}

#[gpui::test]
async fn run_sql_file_streams_statements_and_stops_on_error(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("load.sql.gz");
    let mut script = String::from("CREATE TABLE loaded (id INTEGER PRIMARY KEY, note TEXT);\n");
    for id in 0..200 {
        script.push_str(&format!("INSERT INTO loaded VALUES ({id}, 'a;b');\n"));
    }
    script.push_str(
        "INSERT INTO missing_table VALUES (1);\nINSERT INTO loaded VALUES (999, 'never');\n",
    );
    super::sql_files::write_sql_file(&path, &script).unwrap();

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            let ix = panel.active_tab_ix;
            assert!(matches!(panel.tabs[ix], TabType::Query(_)));
            panel.start_file_run(ix, path.clone(), window, cx);
        });
    for _ in 0..200 {
        cx.run_until_parked();
        if harness
            .editor_panel
            .read_with(&cx, |panel, _| panel.file_run.is_none())
        {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert!(
        harness
            .editor_panel
            .read_with(&cx, |panel, _| panel.file_run.is_none())
    );

    let log = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .unwrap()
            .sql_view
            .read(cx)
            .all_text()
    });
    assert!(
        log.contains("load.sql.gz: statement 202 failed after 201 statements ran"),
        "{log}"
    );

    set_editor_text(&harness, "SELECT COUNT(*) FROM loaded", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    assert_eq!(result_cell(&harness, 0, 0, &cx), Some(Some("200".into())));
}
