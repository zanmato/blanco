//! The grid against tables outside the `public` schema, on the compose
//! PostgreSQL and CockroachDB servers. Each table has a namesake in `public`
//! with other values and columns, so a lookup or a write that loses the
//! schema lands on the wrong table instead of passing by chance. A server
//! that is down is skipped, unless `BLANCO_RUN_DB_TESTS=1`.

use blanco_core::{ConnectionContext, DatabaseService as _};
use database::{ConnectionConfig, DatabaseService, DatabaseType};
use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_component::table::TableState;

use super::{ResultsPanel, ResultsTableDelegate};
use crate::app_settings::AppSettings;
use crate::editor::TabCreationParams;
use crate::results_panel::cell_edit_state::CellInput;
use crate::test_harness::{TestHarness, result_cell, run_query, set_editor_text, wait_for_query};

/// The compose servers with schemas, and their ports.
const BACKENDS: [(DatabaseType, u16); 2] = [
    (DatabaseType::PostgreSQL, 5488),
    (DatabaseType::CockroachDb, 26258),
];

struct Server {
    service: DatabaseService,
    handle: tokio::runtime::Handle,
    config_id: i64,
    db_type: DatabaseType,
}

impl Server {
    /// Connect to the compose server and run `setup`. `None` when the server
    /// is down and the database tests are not required.
    fn connect(
        (db_type, port): (DatabaseType, u16),
        config_id: i64,
        setup: &str,
        cx: &mut VisualTestContext,
    ) -> Option<Self> {
        let (service, handle) = cx.update(|_, cx| {
            (
                DatabaseService::global(cx).clone(),
                gpui_tokio::Tokio::handle(cx),
            )
        });
        let config = ConnectionConfig::new(
            config_id,
            "it-schemas".to_string(),
            db_type,
            "localhost".to_string(),
            port,
            "blanco".to_string(),
            "blanco".to_string(),
            Some("blanco".to_string()),
        );
        let server = Self {
            service,
            handle,
            config_id,
            db_type,
        };
        server
            .handle
            .block_on(server.service.add_connection_config(config));
        match server.try_script(setup) {
            Ok(_) => Some(server),
            Err(error) => {
                assert!(
                    std::env::var("BLANCO_RUN_DB_TESTS").as_deref() != Ok("1"),
                    "{db_type:?} unreachable: {error:#}"
                );
                None
            }
        }
    }

    /// Run `sql`, again when CockroachDB asks for it: schema changes that
    /// other tests make at the same time make it refuse a transaction with
    /// "restart transaction", which its clients are meant to retry.
    fn try_script(&self, sql: &str) -> anyhow::Result<Vec<blanco_core::QueryResult>> {
        let mut attempts = 0;
        loop {
            let result = self.handle.block_on(self.service.execute_script(
                self.config_id,
                Some("blanco"),
                sql,
            ));
            match result {
                Err(error)
                    if attempts < 20 && format!("{error:#}").contains("restart transaction") =>
                {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                result => return result,
            }
        }
    }

    fn script(&self, sql: &str) {
        self.try_script(sql)
            .unwrap_or_else(|error| panic!("{sql}: {error:#}"));
    }

    /// The first column of the first row `sql` returns.
    fn value(&self, sql: &str) -> Option<String> {
        self.try_script(sql)
            .unwrap_or_else(|error| panic!("{sql}: {error:#}"))
            .first()
            .and_then(|result| result.rows.first())
            .and_then(|row| row.first().cloned().flatten())
    }

    fn open_tab(&self, harness: &TestHarness, cx: &mut VisualTestContext) {
        let context = ConnectionContext {
            connection_id: self.config_id,
            db_type: self.db_type,
            connection_name: "it-schemas".into(),
            database_name: "blanco".into(),
            schema_name: None,
            environment_type: None,
        };
        harness.editor_panel.update_in(cx, |panel, window, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title: "schemas".into(),
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context,
                },
                cx,
            );
        });
    }
}

async fn run(
    harness: &TestHarness,
    sql: &str,
    cx: &mut VisualTestContext,
) -> (
    Entity<ResultsPanel>,
    Entity<TableState<ResultsTableDelegate>>,
) {
    set_editor_text(harness, sql, cx);
    run_query(harness, cx);
    wait_for_query(harness, cx).await;
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

/// Type `value` into a cell and commit it, waiting until `check` on the
/// server reads it back.
async fn edit_and_commit(
    harness: &TestHarness,
    server: &Server,
    (row, col): (usize, usize),
    value: &str,
    check: &str,
    cx: &mut VisualTestContext,
) {
    let panel = harness.editor_panel.read_with(cx, |panel, _| {
        panel
            .active_query_tab()
            .expect("query tab")
            .results_panel
            .clone()
    });
    let table_state = panel.read_with(cx, |panel, _| panel.table_state().clone());
    panel.update_in(cx, |panel, window, cx| {
        panel.start_cell_edit(row, col, window, cx);
    });
    cx.run_until_parked();
    let input = table_state.read_with(cx, |state, _| {
        match state
            .delegate()
            .edit_state
            .get_editing_input()
            .expect("the cell should be editable")
        {
            CellInput::Inline(input) => input,
            CellInput::Expanded(_) => panic!("a fresh cell edit is inline"),
        }
    });
    input.update_in(cx, |input, window, cx| {
        input.replace_all(value, window, cx);
    });
    cx.run_until_parked();
    panel.update(cx, |panel, cx| panel.finalize_active_cell_edit(cx));
    cx.run_until_parked();
    harness.editor_panel.update_in(cx, |panel, window, cx| {
        panel.commit_current_changes(window, cx)
    });

    let mut saved = None;
    for _ in 0..200 {
        cx.run_until_parked();
        saved = server.value(check);
        if saved.as_deref() == Some(value) {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    let statements = panel.read_with(cx, |panel, cx| panel.preview_pending_sql(cx));
    panic!("{check} read {saved:?}, expected {value:?}; pending: {statements:?}");
}

/// A table read by a schema qualified name, in lower case, in mixed case
/// (which PostgreSQL folds) and quoted, is editable with its own columns,
/// and an edit is written to it. An unqualified name still means the table
/// the search path finds.
#[gpui::test]
async fn tables_outside_public_are_edited_in_their_own_schema(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    for (index, backend) in BACKENDS.into_iter().enumerate() {
        edit_in_own_schema(&harness, backend, 9402 + index as i64 * 10, &mut cx).await;
    }
}

async fn edit_in_own_schema(
    harness: &TestHarness,
    backend: (DatabaseType, u16),
    config_id: i64,
    cx: &mut VisualTestContext,
) {
    let Some(server) = Server::connect(
        backend,
        config_id,
        r#"DROP SCHEMA IF EXISTS blanco_schema_edit CASCADE;
           DROP SCHEMA IF EXISTS "Blanco Schema Edit" CASCADE;
           DROP TABLE IF EXISTS public.schema_edit_items;
           CREATE SCHEMA blanco_schema_edit;
           CREATE SCHEMA "Blanco Schema Edit";
           CREATE TABLE public.schema_edit_items (id integer PRIMARY KEY, name text);
           CREATE TABLE blanco_schema_edit.schema_edit_items
               (id integer PRIMARY KEY, name text, note text);
           CREATE TABLE "Blanco Schema Edit"."Schema Edit Items"
               ("Id" integer PRIMARY KEY, "Name" text, "Note" text);
           INSERT INTO public.schema_edit_items VALUES (1, 'public');
           INSERT INTO blanco_schema_edit.schema_edit_items VALUES (1, 'lower', 'n');
           INSERT INTO "Blanco Schema Edit"."Schema Edit Items" VALUES (1, 'quoted', 'n');"#,
        cx,
    ) else {
        return;
    };
    server.open_tab(harness, cx);

    let lower = "SELECT name FROM blanco_schema_edit.schema_edit_items WHERE id = 1";
    let quoted = r#"SELECT "Name" FROM "Blanco Schema Edit"."Schema Edit Items" WHERE "Id" = 1"#;
    let public = "SELECT name FROM public.schema_edit_items WHERE id = 1";
    let cases: [(&str, &[&str], &str); 4] = [
        (
            "SELECT * FROM blanco_schema_edit.schema_edit_items ORDER BY id",
            &["id", "name", "note"],
            lower,
        ),
        (
            "SELECT * FROM Blanco_Schema_Edit.Schema_Edit_Items ORDER BY id",
            &["id", "name", "note"],
            lower,
        ),
        (
            r#"SELECT * FROM "Blanco Schema Edit"."Schema Edit Items" ORDER BY "Id""#,
            &["Id", "Name", "Note"],
            quoted,
        ),
        (
            "SELECT * FROM schema_edit_items ORDER BY id",
            &["id", "name"],
            public,
        ),
    ];
    for (index, (sql, columns, check)) in cases.into_iter().enumerate() {
        let (_panel, table_state) = run(harness, sql, cx).await;
        table_state.read_with(cx, |state, _| {
            let delegate = state.delegate();
            let names: Vec<&str> = delegate
                .table_columns()
                .iter()
                .map(|column| column.name.as_str())
                .collect();
            assert_eq!(
                names, columns,
                "{:?} {sql}: the table's columns",
                server.db_type
            );
            assert!(
                delegate.is_editable(),
                "{:?} {sql}: should be editable",
                server.db_type
            );
        });
        let value = format!("edited {index}");
        edit_and_commit(harness, &server, (0, 1), &value, check, cx).await;
    }

    assert_eq!(server.value(lower).as_deref(), Some("edited 1"));
    assert_eq!(server.value(quoted).as_deref(), Some("edited 2"));
    assert_eq!(server.value(public).as_deref(), Some("edited 3"));

    server.script(
        r#"DROP SCHEMA blanco_schema_edit CASCADE;
           DROP SCHEMA "Blanco Schema Edit" CASCADE;
           DROP TABLE public.schema_edit_items;"#,
    );
}

/// Large values of a table outside `public` are fetched from that table
/// when a deferred cell opens.
#[gpui::test]
async fn deferred_values_are_fetched_from_their_own_schema(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    cx.update(|_, cx| {
        AppSettings::global_mut(cx)
            .settings
            .results
            .defer_values_over_kb = 1;
    });
    for (index, backend) in BACKENDS.into_iter().enumerate() {
        fetch_deferred_from_own_schema(&harness, backend, 9403 + index as i64 * 10, &mut cx).await;
    }
}

async fn fetch_deferred_from_own_schema(
    harness: &TestHarness,
    backend: (DatabaseType, u16),
    config_id: i64,
    cx: &mut VisualTestContext,
) {
    let Some(server) = Server::connect(
        backend,
        config_id,
        "DROP SCHEMA IF EXISTS blanco_schema_deferred CASCADE;
         DROP TABLE IF EXISTS public.schema_deferred_items;
         CREATE SCHEMA blanco_schema_deferred;
         CREATE TABLE public.schema_deferred_items (id integer PRIMARY KEY, body text);
         CREATE TABLE blanco_schema_deferred.schema_deferred_items
             (id integer PRIMARY KEY, body text);
         INSERT INTO public.schema_deferred_items VALUES (1, 'public');
         INSERT INTO blanco_schema_deferred.schema_deferred_items
             VALUES (1, repeat('x', 3000));",
        cx,
    ) else {
        return;
    };
    server.open_tab(harness, cx);

    let (panel, table_state) = run(
        harness,
        "SELECT * FROM blanco_schema_deferred.schema_deferred_items",
        cx,
    )
    .await;
    assert_eq!(
        table_state.read_with(cx, |state, _| state.delegate().deferred_size(0, 1)),
        Some(3000),
        "{:?}: the body should be deferred",
        server.db_type
    );

    panel.update_in(cx, |panel, window, cx| {
        panel.start_cell_edit(0, 1, window, cx);
    });
    let mut body = None;
    for _ in 0..200 {
        cx.run_until_parked();
        body = result_cell(harness, 0, 1, cx).flatten();
        if table_state.read_with(cx, |state, _| state.delegate().deferred_values().is_empty()) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    assert_eq!(body, Some("x".repeat(3000)), "{:?}", server.db_type);

    server.script(
        "DROP SCHEMA blanco_schema_deferred CASCADE;
         DROP TABLE public.schema_deferred_items;",
    );
}

/// A foreign key into another schema opens the referenced row there, both
/// in the popover and in a new tab.
#[gpui::test]
async fn foreign_keys_into_another_schema_find_their_row(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    for (index, backend) in BACKENDS.into_iter().enumerate() {
        follow_foreign_key_into_schema(&harness, backend, 9404 + index as i64 * 10, &mut cx).await;
    }
}

async fn follow_foreign_key_into_schema(
    harness: &TestHarness,
    backend: (DatabaseType, u16),
    config_id: i64,
    cx: &mut VisualTestContext,
) {
    let Some(server) = Server::connect(
        backend,
        config_id,
        r#"DROP SCHEMA IF EXISTS blanco_schema_orders CASCADE;
           DROP SCHEMA IF EXISTS "Blanco Schema Customers" CASCADE;
           DROP TABLE IF EXISTS public."Customers";
           CREATE SCHEMA blanco_schema_orders;
           CREATE SCHEMA "Blanco Schema Customers";
           CREATE TABLE public."Customers" (id integer PRIMARY KEY, name text);
           CREATE TABLE "Blanco Schema Customers"."Customers" (id integer PRIMARY KEY, name text);
           CREATE TABLE blanco_schema_orders.orders (
               id integer PRIMARY KEY,
               customer_id integer REFERENCES "Blanco Schema Customers"."Customers" (id)
           );"#,
        cx,
    ) else {
        return;
    };
    // Apart from the tables: CockroachDB refuses rows for a table whose
    // foreign key is still being added in the same batch.
    server.script(
        r#"INSERT INTO public."Customers" VALUES (7, 'public');
           INSERT INTO "Blanco Schema Customers"."Customers" VALUES (7, 'referenced');
           INSERT INTO blanco_schema_orders.orders VALUES (1, 7);"#,
    );
    server.open_tab(harness, cx);

    let (_panel, table_state) = run(harness, "SELECT * FROM blanco_schema_orders.orders", cx).await;
    let (foreign_table, request) = table_state.read_with(cx, |state, _| {
        let delegate = state.delegate();
        let foreign_key = delegate
            .table_columns()
            .iter()
            .find(|column| column.name == "customer_id")
            .and_then(|column| column.foreign_key.clone())
            .expect("customer_id is a foreign key");
        (
            foreign_key.foreign_table_name,
            delegate
                .referenced_row_request(0, 1)
                .expect("the cell references a row"),
        )
    });
    assert_eq!(
        server.value(&format!("SELECT name FROM ({}) AS referenced", request.sql)),
        Some("referenced".to_string()),
        "{:?} {}",
        server.db_type,
        request.sql
    );

    let lookup = server
        .handle
        .block_on(server.service.foreign_key_lookup(
            server.config_id,
            Some("blanco"),
            &foreign_table,
            "id",
            "7",
        ))
        .unwrap_or_else(|error| panic!("lookup in {foreign_table}: {error:#}"));
    let name_column = lookup
        .columns
        .iter()
        .position(|column| column == "name")
        .expect("the lookup returns the name");
    let names: Vec<Option<String>> = lookup
        .rows
        .iter()
        .map(|row| row.get(name_column).cloned().flatten())
        .collect();
    assert_eq!(
        names,
        [Some("referenced".to_string())],
        "{:?} lookup in {foreign_table}",
        server.db_type
    );

    server.script(
        r#"DROP SCHEMA blanco_schema_orders CASCADE;
           DROP SCHEMA "Blanco Schema Customers" CASCADE;
           DROP TABLE public."Customers";"#,
    );
}
