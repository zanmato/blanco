use gpui::{TestAppContext, VisualTestContext, px};

use crate::app::{HideResultColumn, PinResultColumn};
use crate::results_panel::ResultsPanel;
use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};

async fn run(harness: &TestHarness, sql: &str, cx: &mut VisualTestContext) {
    set_editor_text(harness, sql, cx);
    run_query(harness, cx);
    wait_for_query(harness, cx).await;
}

fn panel(harness: &TestHarness, cx: &VisualTestContext) -> gpui::Entity<ResultsPanel> {
    harness.editor_panel.read_with(cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    })
}

fn shown_columns(panel: &gpui::Entity<ResultsPanel>, cx: &VisualTestContext) -> Vec<String> {
    panel.read_with(cx, |panel, cx| {
        let delegate = panel.table_state().read(cx).delegate();
        delegate.columns[..delegate.visible_column_count()]
            .iter()
            .map(|column| column.name.to_string())
            .collect()
    })
}

#[gpui::test]
async fn a_whole_table_layout_is_kept_across_reruns_and_saved(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    run(
        &harness,
        "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT, email TEXT, notes TEXT)",
        &mut cx,
    )
    .await;
    run(
        &harness,
        "INSERT INTO people VALUES (1, 'Ada', 'ada@example.com', 'first')",
        &mut cx,
    )
    .await;
    run(&harness, "SELECT * FROM people", &mut cx).await;

    let results = panel(&harness, &cx);
    assert_eq!(
        shown_columns(&results, &cx),
        ["id", "name", "email", "notes"]
    );

    // Hide `notes` from its context menu action, pin `email`, widen `name`.
    cx.update(|window, cx| {
        results.update(cx, |panel, cx| {
            panel.on_hide_result_column(&HideResultColumn { col: 3 }, window, cx);
            panel.on_pin_result_column(
                &PinResultColumn {
                    col: 2,
                    pinned: true,
                },
                window,
                cx,
            );
        })
    });
    let table_state = results.read_with(&cx, |panel, _| panel.table_state().clone());
    table_state.update(&mut cx, |state, _| {
        let widths: Vec<_> = state
            .delegate()
            .columns
            .iter()
            .map(|column| {
                if column.name.as_str() == "name" {
                    px(333.)
                } else {
                    column.width
                }
            })
            .collect();
        state.delegate_mut().set_column_widths(&widths);
    });
    cx.update(|_, cx| {
        results.update(cx, |panel, cx| {
            let table_state = panel.table_state().clone();
            panel.remember_column_layout(&table_state, cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(shown_columns(&results, &cx), ["email", "id", "name"]);

    // Copies leave the hidden column out and follow the order.
    let copied = results.read_with(&cx, |panel, cx| {
        let delegate = panel.table_state().read(cx).delegate();
        panel.get_selected_data_for_rows(&[0].into_iter().collect(), delegate)
    });
    assert_eq!(copied.columns, ["email", "id", "name"]);
    assert_eq!(
        copied.selected_rows[0].cells[0].value.as_deref(),
        Some("ada@example.com")
    );

    // A rerun gets the same arrangement and width back.
    run(&harness, "SELECT * FROM people", &mut cx).await;
    let results = panel(&harness, &cx);
    assert_eq!(shown_columns(&results, &cx), ["email", "id", "name"]);
    results.read_with(&cx, |panel, cx| {
        let delegate = panel.table_state().read(cx).delegate();
        assert_eq!(delegate.pinned_column_count(), 1);
        let name = delegate
            .columns
            .iter()
            .find(|column| column.name.as_str() == "name")
            .unwrap();
        assert_eq!(name.width, px(333.));
        // The row still carries the hidden value, for edits keyed on it.
        assert_eq!(delegate.rows[0][3].as_deref(), Some("first"));
    });

    // The table's layout was saved for the next session.
    let database = cx.update(|_, cx| app_database::AppDatabase::global(cx).clone());
    let handle = cx.update(|_, cx| gpui_tokio::Tokio::handle(cx));
    let saved = handle
        .block_on(database.load_all_settings())
        .expect("settings");
    let (_, value) = saved
        .iter()
        .find(|(key, _)| key.starts_with("column_layout.") && key.ends_with(".people"))
        .expect("saved layout");
    assert!(value.contains("\"hidden\":[\"notes\"]"), "{value}");
}
