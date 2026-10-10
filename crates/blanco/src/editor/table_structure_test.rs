use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use super::{ForeignKeyForm, TableStructureTab};
use crate::editor::TableStructureParams;
use crate::test_harness::TestHarness;
use blanco_core::ConnectionContext;
use blanco_core::ddl::{ForeignKeyOperation, ReferentialAction};
use database::DatabaseType;

fn context(harness: &TestHarness) -> ConnectionContext {
    ConnectionContext {
        connection_id: harness.connection_id,
        db_type: DatabaseType::SQLite,
        connection_name: "test".into(),
        database_name: "main".into(),
        schema_name: None,
        environment_type: None,
    }
}

fn execute(harness: &TestHarness, sql: &str, cx: &mut VisualTestContext) {
    let (db_service, handle) = cx.update(|_, cx| {
        (
            database::DatabaseService::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let connection_id = harness.connection_id;
    handle
        .block_on(async {
            let connection = db_service
                .get_or_create_connection(connection_id, Some("main"))
                .await?;
            connection.execute_write(sql, Some("main"), &[]).await
        })
        .unwrap_or_else(|error| panic!("{sql}: {error:#}"));
}

/// Poll until `done` holds; the loads run on the tokio runtime, not the
/// test executor, so parking alone does not wait for them.
async fn wait_until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    for _ in 0..100 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("timed out waiting for {what}");
}

#[gpui::test]
async fn structure_tab_lists_foreign_keys_and_prefills_the_form(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    execute(
        &harness,
        "CREATE TABLE parent (id integer NOT NULL, region integer NOT NULL, label text, PRIMARY KEY (id, region))",
        &mut cx,
    );
    execute(
        &harness,
        "CREATE TABLE child (parent_id integer, parent_region integer, FOREIGN KEY (parent_id, parent_region) REFERENCES parent (id, region) ON DELETE CASCADE)",
        &mut cx,
    );

    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_table_structure_tab(
                TableStructureParams {
                    context: context(&harness),
                    table_name: "child".to_string(),
                },
                window,
                cx,
            )
        });
    let tab = tab.upgrade().expect("structure tab");
    tab.update_in(&mut cx, |tab, window, cx| tab.reload(window, cx));
    wait_until(&mut cx, "structure", |cx| {
        tab.read_with(cx, |tab, _| !tab.loading && !tab.columns.is_empty())
    })
    .await;

    tab.read_with(&cx, |tab: &TableStructureTab, _| {
        assert_eq!(tab.error, None);
        assert_eq!(tab.foreign_keys.len(), 1);
        let foreign_key = &tab.foreign_keys[0];
        assert_eq!(foreign_key.columns, vec!["parent_id", "parent_region"]);
        assert_eq!(foreign_key.referenced_table, "parent");
        assert_eq!(foreign_key.referenced_columns, vec!["id", "region"]);
        assert_eq!(foreign_key.on_delete, "CASCADE");
    });

    let form: Entity<ForeignKeyForm> = tab.update_in(&mut cx, |tab, window, cx| {
        let columns = tab.columns.iter().map(|c| c.name.clone()).collect();
        let context = tab.context.clone();
        cx.new(|cx| ForeignKeyForm::new(context, columns, window, cx))
    });
    form.update_in(&mut cx, |form, window, cx| form.add_pair(window, cx));
    // The table list loads asynchronously; `set_selected_value` only finds
    // "parent" once it has.
    wait_until(&mut cx, "tables", |cx| {
        form.update_in(cx, |form, window, cx| {
            form.table.update(cx, |select, cx| {
                select.set_selected_value(&"parent".to_string(), window, cx);
                select.selected_value().is_some()
            })
        })
    })
    .await;

    // Confirming a table loads its columns and prefills its primary key.
    form.update_in(&mut cx, |form, window, cx| {
        form.load_referenced_columns("parent".to_string(), window, cx);
    });
    wait_until(&mut cx, "referenced columns", |cx| {
        form.read_with(cx, |form, _| !form.referenced_columns.is_empty())
    })
    .await;

    form.update_in(&mut cx, |form, window, cx| {
        assert_eq!(form.referenced_primary_key, vec!["id", "region"]);
        for (pair, column) in form.pairs.iter().zip(["parent_id", "parent_region"]) {
            pair.column.update(cx, |select, cx| {
                select.set_selected_value(&column.to_string(), window, cx)
            });
        }
        form.on_delete.update(cx, |select, cx| {
            select.set_selected_value(&"SET NULL".to_string(), window, cx)
        });
    });

    let operation = form.read_with(&cx, |form, cx| form.operation(cx));
    assert_eq!(
        operation,
        ForeignKeyOperation::Create {
            name: None,
            columns: vec!["parent_id".to_string(), "parent_region".to_string()],
            referenced_schema: None,
            referenced_table: "parent".to_string(),
            referenced_columns: vec!["id".to_string(), "region".to_string()],
            on_update: ReferentialAction::NoAction,
            on_delete: ReferentialAction::SetNull,
        }
    );
}
