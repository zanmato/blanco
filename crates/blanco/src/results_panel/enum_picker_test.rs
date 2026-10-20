//! Picking an enum label for a cell against the compose PostgreSQL server.
//! Skipped when it is down, unless `BLANCO_RUN_DB_TESTS=1`.

use database::DatabaseType;
use gpui::{TestAppContext, VisualTestContext};

use super::ResultsTableDelegate;
use crate::test_harness::{LiveServer, TestHarness, run_for_results};

#[gpui::test]
async fn enum_cells_offer_their_labels_and_save_the_picked_one(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let Some(server) = LiveServer::connect(
        (DatabaseType::PostgreSQL, 5488),
        9601,
        "DROP SCHEMA IF EXISTS blanco_enum_edit CASCADE;
         CREATE SCHEMA blanco_enum_edit;
         CREATE TYPE blanco_enum_edit.status AS ENUM ('open', 'on hold', 'closed');
         CREATE TABLE blanco_enum_edit.tickets
             (id integer PRIMARY KEY, status blanco_enum_edit.status, note text);
         INSERT INTO blanco_enum_edit.tickets VALUES (1, 'open', 'n');",
        &mut cx,
    ) else {
        return;
    };
    server.open_tab(&harness, &mut cx);

    let (panel, table_state) =
        run_for_results(&harness, "SELECT * FROM blanco_enum_edit.tickets", &mut cx).await;
    table_state.read_with(&cx, |state, _| {
        let delegate = state.delegate();
        assert_eq!(
            delegate.enum_labels(1),
            Some(
                &[
                    "open".to_string(),
                    "on hold".to_string(),
                    "closed".to_string()
                ][..]
            )
        );
        assert_eq!(delegate.enum_labels(2), None, "text has no labels");
    });

    panel.update_in(&mut cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    cx.run_until_parked();
    table_state.update(&mut cx, |state, cx| {
        ResultsTableDelegate::set_enum_picker_open(state, true, cx);
    });
    assert!(table_state.read_with(&cx, |state, _| state.delegate().edit_state.enum_picker_open));
    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::pick_enum_label(state, (0, 1), "on hold", window, cx);
    });
    cx.run_until_parked();
    table_state.read_with(&cx, |state, cx| {
        let edit_state = &state.delegate().edit_state;
        assert!(!edit_state.enum_picker_open, "picking closes the list");
        let text = edit_state
            .editing_input
            .as_ref()
            .map(|input| input.text(cx))
            .unwrap_or_default();
        assert_eq!(text, "on hold", "the editor holds the picked label");
    });

    panel.update(&mut cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.commit_current_changes(window, cx)
        });
    let check = "SELECT status::text FROM blanco_enum_edit.tickets WHERE id = 1";
    let mut saved = None;
    for _ in 0..200 {
        cx.run_until_parked();
        saved = server.value(check);
        if saved.as_deref() == Some("on hold") {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    assert_eq!(saved.as_deref(), Some("on hold"));

    server.script("DROP SCHEMA blanco_enum_edit CASCADE");
}
