//! Editing an hstore cell in the pair editor against the compose PostgreSQL
//! server. Skipped when it is down, unless `BLANCO_RUN_DB_TESTS=1`.

use std::collections::BTreeMap;

use blanco_core::{ColumnType, ConnectionContext, DatabaseService as _};
use database::{ConnectionConfig, DatabaseType};
use gpui::{TestAppContext, VisualTestContext};

use super::KeyValueEditorEvent;
use crate::editor::TabCreationParams;
use crate::results_panel::ResultsTableDelegate;
use crate::results_panel::results_table_delegate::results_settings;
use crate::test_harness::{TestHarness, result_cell, run_query, set_editor_text, wait_for_query};

const CONFIG_ID: i64 = 9401;

fn as_map(text: &str) -> BTreeMap<String, Option<String>> {
    blanco_core::hstore::parse(text)
        .unwrap_or_else(|error| panic!("{text:?}: {error}"))
        .into_iter()
        .collect()
}

#[gpui::test]
async fn hstore_cells_show_as_pairs_and_save_edited_pairs(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);

    let (service, handle) = cx.update(|_, cx| {
        (
            database::DatabaseService::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let config = ConnectionConfig::new(
        CONFIG_ID,
        "it-hstore".to_string(),
        DatabaseType::PostgreSQL,
        "localhost".to_string(),
        5488,
        "blanco".to_string(),
        "blanco".to_string(),
        Some("blanco".to_string()),
    );
    let setup = handle.block_on(async {
        service.add_connection_config(config).await;
        service
            .execute_script(
                CONFIG_ID,
                Some("blanco"),
                "CREATE EXTENSION IF NOT EXISTS hstore;
                 DROP TABLE IF EXISTS hstore_edit_test;
                 CREATE TABLE hstore_edit_test (id integer PRIMARY KEY, attrs hstore);
                 INSERT INTO hstore_edit_test VALUES (1, 'a=>1, \"b c\"=>NULL');",
            )
            .await
    });
    if let Err(error) = setup {
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
                TabCreationParams {
                    title: "hstore".into(),
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context: ConnectionContext {
                        connection_id: CONFIG_ID,
                        db_type: DatabaseType::PostgreSQL,
                        connection_name: "it-hstore".into(),
                        database_name: "blanco".into(),
                        schema_name: None,
                        environment_type: None,
                    },
                },
                cx,
            );
        });
    set_editor_text(
        &harness,
        "SELECT id, attrs FROM hstore_edit_test ORDER BY id",
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
    let stored = result_cell(&harness, 0, 1, &cx)
        .flatten()
        .expect("hstore value");
    let shown = table_state.read_with(&cx, |state, cx| {
        let delegate = state.delegate();
        assert_eq!(delegate.column_types.get(1), Some(&ColumnType::KeyValue));
        delegate
            .display_value(1, &stored, &results_settings(cx))
            .text
    });
    assert_eq!(shown, "a: 1, b c: NULL");

    // Expanding the cell opens the pair editor on its pairs.
    table_state.update_in(&mut cx, |state, window, cx| {
        ResultsTableDelegate::handle_maximize(state, 0, 1, false, window, cx);
    });
    cx.run_until_parked();
    let editor = table_state
        .read_with(&cx, |state, _| {
            state.delegate().edit_state.key_value_editor.clone()
        })
        .expect("the pair editor should open");
    assert_eq!(
        editor.read_with(&cx, |editor, cx| editor.pairs(cx)),
        vec![
            ("a".to_string(), Some("1".to_string())),
            ("b c".to_string(), None)
        ]
    );

    editor.update_in(&mut cx, |editor, window, cx| {
        let set = |input: &gpui::Entity<gpui_component::input::InputState>,
                   text: &str,
                   window: &mut gpui::Window,
                   cx: &mut gpui::Context<super::KeyValueEditor>| {
            input.update(cx, |input, cx| {
                input.set_value(text.to_string(), window, cx)
            });
        };
        set(
            &editor.rows[0].value.clone(),
            "say \"hi\" \\ ok",
            window,
            cx,
        );
        editor.set_null(1, false, cx);
        set(&editor.rows[1].value.clone(), "x", window, cx);
        editor.add_pair(window, cx);
        set(&editor.rows[2].key.clone(), "new, key", window, cx);
        editor.set_null(2, true, cx);
        editor.changed(cx);
    });
    editor.update(&mut cx, |_, cx| cx.emit(KeyValueEditorEvent::Close));
    cx.run_until_parked();

    let statements = results_panel.read_with(&cx, |panel, cx| panel.preview_pending_sql(cx));
    assert_eq!(statements.len(), 1, "{statements:?}");
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.commit_current_changes(window, cx)
        });

    let expected = as_map(r#""a"=>"say \"hi\" \\ ok", "b c"=>"x", "new, key"=>NULL"#);
    let mut saved = None;
    for _ in 0..100 {
        cx.run_until_parked();
        let on_server = handle
            .block_on(service.execute_script(
                CONFIG_ID,
                Some("blanco"),
                "SELECT attrs FROM hstore_edit_test WHERE id = 1",
            ))
            .expect("read back");
        saved = on_server
            .first()
            .and_then(|result| result.rows.first())
            .and_then(|row| row.first().cloned().flatten());
        if saved.as_deref().map(as_map) == Some(expected.clone()) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert_eq!(saved.as_deref().map(as_map), Some(expected));

    handle
        .block_on(service.execute_script(CONFIG_ID, Some("blanco"), "DROP TABLE hstore_edit_test"))
        .expect("drop the table");
}
