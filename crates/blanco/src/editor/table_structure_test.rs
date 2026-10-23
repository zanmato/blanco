use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use super::staging::{RowState, StructureChange};
use super::{ForeignKeyForm, TableStructureTab};
use crate::editor::TableStructureParams;
use crate::test_harness::TestHarness;
use blanco_core::ConnectionContext;
use blanco_core::ddl::{
    ColumnOperation, ColumnSpec, ForeignKeyOperation, IndexOperation, IndexSpec, ReferentialAction,
};
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
        let table_name = tab.table_name.clone();
        cx.new(|cx| ForeignKeyForm::new(context, table_name, columns, window, cx))
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

#[gpui::test]
async fn structure_changes_wait_for_apply_and_run_together(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    execute(
        &harness,
        "CREATE TABLE people (id integer PRIMARY KEY, name text, nickname text)",
        &mut cx,
    );
    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_table_structure_tab(
                TableStructureParams {
                    context: context(&harness),
                    table_name: "people".to_string(),
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

    let age = ColumnSpec {
        name: "age".to_string(),
        data_type: "integer".to_string(),
        nullable: true,
        ..ColumnSpec::default()
    };
    tab.update_in(&mut cx, |tab, window, cx| {
        tab.stage(
            StructureChange::Column(ColumnOperation::Add(age.clone())),
            window,
            cx,
        );
        // The index can name the column only staged so far.
        tab.stage(
            StructureChange::Index(IndexOperation::Create(IndexSpec {
                name: "people_age_idx".to_string(),
                columns: vec!["age".to_string()],
                ..IndexSpec::default()
            })),
            window,
            cx,
        );
        tab.stage(
            StructureChange::Column(ColumnOperation::Drop {
                name: "nickname".to_string(),
            }),
            window,
            cx,
        );
        // SQLite cannot change a column's type in place, which is refused
        // when the change is staged.
        tab.stage(
            StructureChange::Column(ColumnOperation::Alter {
                from: ColumnSpec {
                    name: "name".to_string(),
                    data_type: "text".to_string(),
                    nullable: true,
                    ..ColumnSpec::default()
                },
                to: ColumnSpec {
                    name: "name".to_string(),
                    data_type: "varchar(20)".to_string(),
                    nullable: true,
                    ..ColumnSpec::default()
                },
            }),
            window,
            cx,
        );
    });

    tab.read_with(&cx, |tab: &TableStructureTab, cx| {
        assert_eq!(tab.pending.len(), 3);
        assert!(tab.has_pending_changes(cx));
        assert_eq!(tab.staged.live_column_names(), ["id", "name", "age"]);
        let delegate = tab.columns_table_state.read(cx).delegate();
        let states: Vec<RowState> = delegate.marks.iter().map(|mark| mark.state).collect();
        assert_eq!(
            states,
            [
                RowState::Unchanged,
                RowState::Unchanged,
                RowState::Dropped,
                RowState::Added
            ]
        );
        let statements: Vec<&str> = tab
            .pending
            .iter()
            .flat_map(|pending| &pending.statements)
            .map(String::as_str)
            .collect();
        assert_eq!(
            statements,
            [
                "ALTER TABLE \"people\" ADD COLUMN \"age\" integer",
                "CREATE INDEX \"people_age_idx\" ON \"people\" (\"age\")",
                "ALTER TABLE \"people\" DROP COLUMN \"nickname\"",
            ]
        );
    });
    // Nothing has run yet.
    tab.update_in(&mut cx, |tab, window, cx| tab.reload(window, cx));
    wait_until(&mut cx, "reload", |cx| {
        tab.read_with(cx, |tab, _| !tab.loading)
    })
    .await;
    tab.read_with(&cx, |tab, _| {
        assert_eq!(tab.columns.len(), 3);
        assert_eq!(tab.pending.len(), 3);
    });

    // Undoing the new column takes the index on it along.
    tab.update_in(&mut cx, |tab, window, cx| tab.undo_change(0, window, cx));
    tab.read_with(&cx, |tab, _| {
        let changes: Vec<&StructureChange> =
            tab.pending.iter().map(|pending| &pending.change).collect();
        assert_eq!(
            changes,
            [&StructureChange::Column(ColumnOperation::Drop {
                name: "nickname".to_string()
            })]
        );
    });

    // Closing the tab asks first instead of dropping the pending change.
    let tab_count = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            let count = panel.tabs.len();
            panel.request_close_tab(panel.active_tab_index(), window, cx);
            count
        });
    harness.editor_panel.read_with(&cx, |panel, cx| {
        assert_eq!(panel.tabs.len(), tab_count);
        assert_eq!(panel.unsaved_tab_names(cx), ["people"]);
    });

    tab.update_in(&mut cx, |tab, window, cx| {
        tab.stage(
            StructureChange::Column(ColumnOperation::Add(age)),
            window,
            cx,
        );
        tab.apply_changes(window, cx);
    });
    wait_until(&mut cx, "apply", |cx| {
        tab.read_with(cx, |tab, _| {
            !tab.applying && !tab.loading && tab.pending.is_empty()
        })
    })
    .await;
    tab.read_with(&cx, |tab, cx| {
        let names: Vec<&str> = tab.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "age"]);
        assert!(!tab.has_pending_changes(cx));
    });
}

#[gpui::test]
async fn virtual_keys_are_saved_from_the_panel_and_reach_the_grid(cx: &mut TestAppContext) {
    use super::Panel;
    use crate::editor::TabCreationParams;
    use crate::test_harness::{run_query, set_editor_text, wait_for_query};
    use app_database::{AppDatabase, ConnectionData};
    use database::ConnectionConfig;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    // Virtual keys belong to a stored connection.
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let path = temp_dir
        .path()
        .join("shop.db")
        .to_string_lossy()
        .to_string();
    let (db_service, app_database, handle) = cx.update(|_, cx| {
        (
            database::DatabaseService::global(cx).clone(),
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let connection_id = handle
        .block_on(
            app_database.save_connection(&ConnectionData::new_sqlite("shop".into(), path.clone())),
        )
        .expect("stored connection");
    handle
        .block_on(async {
            db_service
                .add_connection_config(ConnectionConfig::new_sqlite(
                    connection_id,
                    "shop".into(),
                    path,
                ))
                .await;
            let connection = db_service
                .get_or_create_connection(connection_id, Some("main"))
                .await?;
            for sql in [
                "CREATE TABLE customers (id integer PRIMARY KEY, name text)",
                "CREATE TABLE orders (id integer PRIMARY KEY, customer_id integer)",
                "CREATE VIEW order_report AS SELECT id, customer_id FROM orders",
                "INSERT INTO customers VALUES (1, 'Ada')",
                "INSERT INTO orders VALUES (10, 1)",
            ] {
                connection.execute_write(sql, Some("main"), &[]).await?;
            }
            anyhow::Ok(())
        })
        .unwrap_or_else(|error| panic!("setup: {error:#}"));
    let shop = ConnectionContext {
        connection_id,
        db_type: DatabaseType::SQLite,
        connection_name: "shop".into(),
        database_name: "main".into(),
        schema_name: None,
        environment_type: None,
    };

    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_table_structure_tab(
                TableStructureParams {
                    context: shop.clone(),
                    table_name: "order_report".to_string(),
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

    tab.update_in(&mut cx, |tab, window, cx| {
        tab.edit_virtual_key(0, window, cx)
    });
    let form = tab.read_with(&cx, |tab, _| match &tab.panel {
        Some(Panel::VirtualKey { form, .. }) => form.clone(),
        _ => panic!("the panel should show a new virtual key"),
    });
    form.update_in(&mut cx, |form, window, cx| {
        form.name
            .update(cx, |name, cx| name.set_value("customer", window, cx));
    });
    // Tables and views load asynchronously.
    wait_until(&mut cx, "referenced tables", |cx| {
        form.update_in(cx, |form, window, cx| {
            form.table.update(cx, |select, cx| {
                select.set_selected_value(&"customers".to_string(), window, cx);
                select.selected_value().is_some()
            })
        })
    })
    .await;
    form.update_in(&mut cx, |form, window, cx| {
        form.load_referenced_columns("customers".to_string(), window, cx);
    });
    wait_until(&mut cx, "referenced columns", |cx| {
        form.read_with(cx, |form, _| !form.referenced_columns.is_empty())
    })
    .await;
    form.update_in(&mut cx, |form, window, cx| {
        form.pairs[0].column.update(cx, |select, cx| {
            select.set_selected_value(&"customer_id".to_string(), window, cx)
        });
    });

    tab.update_in(&mut cx, |tab, window, cx| tab.save_panel(window, cx));
    wait_until(&mut cx, "saved key", |cx| {
        tab.read_with(cx, |tab, _| tab.virtual_keys.len() == 1)
    })
    .await;
    let saved = handle
        .block_on(app_database.load_virtual_foreign_keys(connection_id, "main", None))
        .expect("stored keys");
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].table_name, "order_report");
    assert_eq!(saved[0].columns, ["customer_id"]);
    assert_eq!(saved[0].referenced_table, "customers");
    assert_eq!(saved[0].referenced_columns, ["id"]);

    // A query on the view now knows where customer_id points.
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title: "Report".into(),
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context: shop,
                },
                cx,
            );
        });
    set_editor_text(&harness, "SELECT * FROM order_report", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    let foreign_key = harness.editor_panel.read_with(&cx, |panel, cx| {
        let results = panel.active_results_panel().expect("results panel");
        results.read_with(cx, |results, cx| {
            results.table_state().read_with(cx, |state, _| {
                state
                    .delegate()
                    .table_columns()
                    .iter()
                    .find(|column| column.name == "customer_id")
                    .and_then(|column| column.foreign_key.clone())
            })
        })
    });
    let foreign_key = foreign_key.expect("customer_id should reference customers");
    assert_eq!(foreign_key.foreign_table_name, "customers");
    assert_eq!(foreign_key.foreign_column_name, "id");

    // Deleting it takes it out of the list and the store.
    tab.update_in(&mut cx, |tab, window, cx| {
        tab.delete_virtual_key(saved[0].id, window, cx)
    });
    wait_until(&mut cx, "deleted key", |cx| {
        tab.read_with(cx, |tab, _| tab.virtual_keys.is_empty())
    })
    .await;
    assert!(
        handle
            .block_on(app_database.load_virtual_foreign_keys(connection_id, "main", None))
            .expect("stored keys")
            .is_empty()
    );
}

#[gpui::test]
async fn a_new_table_is_designed_then_created_in_one_statement(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.executor().allow_parking();

    execute(
        &harness,
        "CREATE TABLE customers (id integer PRIMARY KEY, name text)",
        &mut cx,
    );
    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_new_table_tab(context(&harness), window, cx)
        });
    tab.update_in(&mut cx, |tab, window, cx| tab.reload(window, cx));
    wait_until(&mut cx, "connection details", |cx| {
        tab.read_with(cx, |tab, _| !tab.loading)
    })
    .await;

    // It starts with a numbered key and asks nothing when closed untouched.
    tab.read_with(&cx, |tab: &TableStructureTab, cx| {
        assert!(tab.is_creating());
        assert!(!tab.has_pending_changes(cx));
        let definition = tab.new_table_definition();
        assert_eq!(definition.columns.len(), 1);
        assert_eq!(definition.columns[0].name, "id");
        assert!(definition.columns[0].primary_key);
    });

    tab.update_in(&mut cx, |tab, window, cx| {
        if let Some(name) = &tab.creating {
            name.update(cx, |name, cx| name.set_value("orders", window, cx));
        }
        tab.stage(
            StructureChange::Column(ColumnOperation::Add(ColumnSpec {
                name: "customer_id".to_string(),
                data_type: "integer".to_string(),
                ..ColumnSpec::default()
            })),
            window,
            cx,
        );
        tab.stage(
            StructureChange::Index(IndexOperation::Create(IndexSpec {
                name: "orders_customer_idx".to_string(),
                columns: vec!["customer_id".to_string()],
                ..IndexSpec::default()
            })),
            window,
            cx,
        );
        // SQLite declares a foreign key only when the table is created.
        tab.stage(
            StructureChange::ForeignKey(ForeignKeyOperation::Create {
                name: None,
                columns: vec!["customer_id".to_string()],
                referenced_schema: None,
                referenced_table: "customers".to_string(),
                referenced_columns: vec!["id".to_string()],
                on_update: ReferentialAction::NoAction,
                on_delete: ReferentialAction::Cascade,
            }),
            window,
            cx,
        );
    });
    tab.read_with(&cx, |tab: &TableStructureTab, cx| {
        assert!(tab.has_pending_changes(cx));
        let statements = tab.create_statements("orders").expect("statements");
        assert_eq!(statements.len(), 2, "{statements:?}");
        assert!(statements[0].starts_with("CREATE TABLE \"orders\""));
        assert!(statements[0].contains("FOREIGN KEY (\"customer_id\") REFERENCES \"customers\""));
    });

    tab.update_in(&mut cx, |tab, window, cx| tab.apply_changes(window, cx));
    wait_until(&mut cx, "created table", |cx| {
        tab.read_with(cx, |tab, _| {
            !tab.is_creating() && !tab.applying && !tab.loading && !tab.columns.is_empty()
        })
    })
    .await;
    tab.read_with(&cx, |tab: &TableStructureTab, cx| {
        assert_eq!(tab.table_name, "orders");
        assert_eq!(tab.title, "orders");
        assert!(!tab.has_pending_changes(cx));
        let names: Vec<&str> = tab.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "customer_id"]);
        assert!(tab.columns[0].is_primary_key);
        assert!(
            tab.indexes
                .iter()
                .any(|index| index.name == "orders_customer_idx")
        );
        assert_eq!(tab.foreign_keys.len(), 1);
        assert_eq!(tab.foreign_keys[0].referenced_table, "customers");
    });
}
