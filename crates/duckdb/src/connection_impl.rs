use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    BatchFailure, BatchOutcome, ColumnInfo, Connection, DatabaseType, EntityType,
    ForeignKeyConstraint, ForeignKeyInfo, IndexInfo, ObjectComments, QueryResult, QueryableEntity,
    WriteOperation,
};

use super::{
    DuckdbConnection, Location, Session, SharedDatabase, run_script, run_statement,
    text_parameters, use_database,
};

/// The schema a lookup works in when the caller names none.
fn schema_or_main(schema: Option<&str>) -> String {
    schema
        .filter(|schema| !schema.is_empty())
        .unwrap_or("main")
        .to_string()
}

fn first_column(rows: Vec<Vec<Option<String>>>) -> Vec<String> {
    rows.into_iter()
        .filter_map(|row| row.into_iter().next().flatten())
        .collect()
}

#[async_trait]
impl Connection for DuckdbConnection {
    fn database_type(&self) -> DatabaseType {
        DatabaseType::DuckDb
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        let location = Location::parse(connection_string)?;
        let opened = tokio::task::spawn_blocking(move || {
            let shared = SharedDatabase::open(&location)?;
            let session = shared.connect()?;
            use_database(&session, location.database.as_deref())?;
            anyhow::Ok((location, shared, session))
        })
        .await??;
        let (location, shared, session) = opened;
        self.display_name = super::display_name(&location.path);
        self.location = location;
        self.session = Some(Session {
            interrupt: session.interrupt_handle(),
            connection: Arc::new(Mutex::new(session)),
        });
        self.shared = Some(shared);
        Ok(())
    }

    async fn ping(&self) -> Result<()> {
        self.with_fresh_connection(|connection| {
            connection.execute_batch("SELECT 1")?;
            Ok(())
        })
        .await
    }

    async fn execute_query(
        &self,
        query: &str,
        _database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let query = query.to_string();
        let parameters = parameters.map(<[String]>::to_vec).unwrap_or_default();
        self.with_session(move |connection| {
            run_statement(connection, &query, &text_parameters(&parameters))
        })
        .await
    }

    async fn execute_script(
        &self,
        query: &str,
        _database_name: Option<&str>,
    ) -> Result<Vec<QueryResult>> {
        let query = query.to_string();
        self.with_session(move |connection| run_script(connection, &query))
            .await
    }

    async fn execute_write(
        &self,
        query: &str,
        _database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64> {
        let query = query.to_string();
        let parameters = parameters.to_vec();
        self.with_session(move |connection| {
            let parameters: Vec<Option<&str>> = parameters
                .iter()
                .map(|parameter| parameter.as_deref())
                .collect();
            Ok(run_statement(connection, &query, &parameters)?.rows_affected)
        })
        .await
    }

    async fn execute_operations_transactional(
        &self,
        operations: &[WriteOperation],
        _database_name: Option<&str>,
    ) -> Result<BatchOutcome, BatchFailure> {
        let operations = operations.to_vec();
        self.with_session(move |connection| {
            let transaction = connection.transaction()?;
            let mut outcome = BatchOutcome::default();
            for operation in &operations {
                let parameters: Vec<Option<&str>> = operation
                    .parameters
                    .iter()
                    .map(|parameter| parameter.as_deref())
                    .collect();
                // Dropping the transaction on the error path rolls it back.
                outcome.rows_affected +=
                    run_statement(&transaction, &operation.sql, &parameters)?.rows_affected;
                outcome.operations_executed += 1;
            }
            transaction.commit()?;
            Ok(outcome)
        })
        .await
        .map_err(BatchFailure::atomic)
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        let rows = self
            .catalog_rows(
                "SELECT database_name FROM duckdb_databases() WHERE NOT internal ORDER BY database_name",
                Vec::new(),
            )
            .await?;
        Ok(first_column(rows))
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        let rows = self
            .catalog_rows(
                "SELECT schema_name FROM duckdb_schemas()
                 WHERE database_name = current_database()
                   AND schema_name NOT IN ('information_schema', 'pg_catalog')
                 ORDER BY schema_name",
                Vec::new(),
            )
            .await?;
        Ok(first_column(rows))
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let rows = self
            .catalog_rows(
                "SELECT table_name FROM duckdb_tables()
                 WHERE database_name = current_database() AND schema_name = ?
                 ORDER BY table_name",
                vec![schema_or_main(schema)],
            )
            .await?;
        Ok(first_column(rows))
    }

    async fn get_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let rows = self
            .catalog_rows(
                "SELECT view_name FROM duckdb_views()
                 WHERE NOT internal AND database_name = current_database() AND schema_name = ?
                 ORDER BY view_name",
                vec![schema_or_main(schema)],
            )
            .await?;
        Ok(first_column(rows))
    }

    async fn list_functions(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let rows = self
            .catalog_rows(
                "SELECT DISTINCT function_name FROM duckdb_functions()
                 WHERE NOT internal AND database_name = current_database() AND schema_name = ?
                   AND function_type IN ('macro', 'table_macro')
                 ORDER BY function_name",
                vec![schema_or_main(schema)],
            )
            .await?;
        Ok(first_column(rows))
    }

    async fn get_database_sizes(&self) -> Result<HashMap<String, u64>> {
        let rows = self
            .catalog_rows(
                "SELECT database_name, total_blocks * block_size + COALESCE(TRY_CAST(wal_size AS BIGINT), 0)
                 FROM pragma_database_size()",
                Vec::new(),
            )
            .await?;
        Ok(blanco_core::sizes_from_rows(rows))
    }

    async fn get_object_comments(&self, schema: Option<&str>) -> Result<ObjectComments> {
        let rows = self
            .catalog_rows(
                "SELECT 'r', table_name, comment FROM duckdb_tables()
                 WHERE database_name = current_database() AND schema_name = ?1
                 UNION ALL
                 SELECT 'r', view_name, comment FROM duckdb_views()
                 WHERE NOT internal AND database_name = current_database() AND schema_name = ?1
                 UNION ALL
                 SELECT 'f', function_name, comment FROM duckdb_functions()
                 WHERE NOT internal AND database_name = current_database() AND schema_name = ?1",
                vec![schema_or_main(schema)],
            )
            .await?;
        let mut comments = ObjectComments::default();
        for row in rows {
            if let [Some(kind), Some(name), Some(comment)] = row.as_slice() {
                comments.insert_row(kind, name.clone(), comment.clone());
            }
        }
        Ok(comments)
    }

    async fn table_info(
        &self,
        schema: Option<&str>,
        table_name: &str,
    ) -> Result<blanco_core::TableInfo> {
        let rows = self
            .catalog_rows(
                "SELECT comment, estimated_size::VARCHAR FROM duckdb_tables()
                 WHERE database_name = current_database() AND schema_name = ? AND table_name = ?",
                vec![schema_or_main(schema), table_name.to_string()],
            )
            .await?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Table '{table_name}' not found"))?;
        let get = |ix: usize| {
            row.get(ix)
                .cloned()
                .flatten()
                .filter(|value| !value.is_empty())
        };
        Ok(blanco_core::TableInfo {
            comment: get(0),
            estimated_rows: get(1).and_then(|value| value.parse().ok()),
            ..Default::default()
        })
    }

    async fn table_ddl(&self, schema: Option<&str>, table_name: &str) -> Result<String> {
        let rows = self
            .catalog_rows(
                "SELECT sql FROM (
                     SELECT sql, 0 AS position FROM duckdb_tables()
                     WHERE database_name = current_database() AND schema_name = ?1 AND table_name = ?2
                     UNION ALL
                     SELECT sql, 1 FROM duckdb_indexes()
                     WHERE database_name = current_database() AND schema_name = ?1 AND table_name = ?2
                       AND sql IS NOT NULL
                 ) ORDER BY position",
                vec![schema_or_main(schema), table_name.to_string()],
            )
            .await?;
        let statements: Vec<String> = first_column(rows)
            .into_iter()
            .map(|statement| statement.trim_end().trim_end_matches(';').to_string())
            .collect();
        if statements.is_empty() {
            anyhow::bail!("Table '{table_name}' not found");
        }
        Ok(format!("{};", statements.join(";\n")))
    }

    async fn get_queryable_entities(&self, schema: Option<&str>) -> Result<Vec<QueryableEntity>> {
        let rows = self
            .catalog_rows(
                "SELECT table_name, 'TABLE', comment FROM duckdb_tables()
                 WHERE database_name = current_database() AND schema_name = ?1
                 UNION ALL
                 SELECT view_name, 'VIEW', comment FROM duckdb_views()
                 WHERE NOT internal AND database_name = current_database() AND schema_name = ?1
                 ORDER BY 1",
                vec![schema_or_main(schema)],
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let mut cells = row.into_iter();
                let name = cells.next().flatten()?;
                let entity_type = match cells.next().flatten().as_deref() {
                    Some("VIEW") => EntityType::View,
                    _ => EntityType::Table,
                };
                Some(QueryableEntity {
                    name,
                    entity_type,
                    comment: cells.next().flatten().filter(|comment| !comment.is_empty()),
                })
            })
            .collect())
    }

    fn supports_schemas(&self) -> bool {
        true
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        let schema = schema_or_main(schema);
        let columns = self
            .catalog_rows(
                "SELECT column_name, data_type, is_nullable, column_default,
                        character_maximum_length, numeric_precision, numeric_scale, comment
                 FROM duckdb_columns()
                 WHERE database_name = current_database() AND schema_name = ? AND table_name = ?
                 ORDER BY column_index",
                vec![schema.clone(), table_name.to_string()],
            )
            .await?;
        let keys = self.key_columns(&schema, table_name).await?;

        let integer =
            |value: &Option<String>| value.as_deref().and_then(|value| value.parse().ok());
        Ok(columns
            .into_iter()
            .filter_map(|row| {
                let name = row.first().cloned().flatten()?;
                let foreign_key = keys
                    .iter()
                    .find(|key| key.kind == "FOREIGN KEY" && key.column == name)
                    .map(|key| ForeignKeyInfo {
                        foreign_table_name: key.referenced_table.clone().unwrap_or_default(),
                        foreign_column_name: key.referenced_column.clone().unwrap_or_default(),
                        constraint_name: Some(key.name.clone()),
                    });
                Some(ColumnInfo {
                    is_primary_key: keys
                        .iter()
                        .any(|key| key.kind == "PRIMARY KEY" && key.column == name),
                    data_type: row.get(1).cloned().flatten().unwrap_or_default(),
                    is_nullable: row.get(2).cloned().flatten().as_deref() != Some("false"),
                    default_value: row.get(3).cloned().flatten(),
                    character_maximum_length: row.get(4).and_then(integer),
                    numeric_precision: row.get(5).and_then(integer),
                    numeric_scale: row.get(6).and_then(integer),
                    comment: row
                        .get(7)
                        .cloned()
                        .flatten()
                        .filter(|comment| !comment.is_empty()),
                    foreign_key,
                    name,
                    enum_labels: Vec::new(),
                })
            })
            .collect())
    }

    async fn get_indexes_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>> {
        let rows = self
            .catalog_rows(
                "SELECT index_name, is_unique, expressions FROM duckdb_indexes()
                 WHERE database_name = current_database() AND schema_name = ? AND table_name = ?
                 ORDER BY index_name",
                vec![schema_or_main(schema), table_name.to_string()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(IndexInfo {
                    name: row.first().cloned().flatten()?,
                    algorithm: "art".to_string(),
                    is_unique: row.get(1).cloned().flatten().as_deref() == Some("true"),
                    column_names: index_columns(row.get(2).cloned().flatten().as_deref()),
                    condition: None,
                    comment: None,
                    nulls_not_distinct: false,
                })
            })
            .collect())
    }

    async fn get_foreign_keys_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ForeignKeyConstraint>> {
        // DuckDB enforces only NO ACTION, so the actions are left to the
        // default `from_column_rows` fills in.
        let rows: Vec<Vec<Option<String>>> = self
            .key_columns(&schema_or_main(schema), table_name)
            .await?
            .into_iter()
            .filter(|key| key.kind == "FOREIGN KEY")
            .map(|key| {
                vec![
                    Some(key.name),
                    None,
                    key.referenced_table,
                    Some(key.column),
                    key.referenced_column,
                ]
            })
            .collect();
        Ok(ForeignKeyConstraint::from_column_rows(&rows))
    }

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult> {
        let query = DatabaseType::DuckDb.dialect().referenced_row_query(
            table_name,
            column_name,
            reference_value,
            false,
            false,
        );
        self.with_fresh_connection(move |connection| run_statement(connection, &query, &[]))
            .await
    }
}

/// One column of a primary or foreign key, as `duckdb_constraints()` lists
/// it once its column lists are unnested.
struct KeyColumn {
    kind: String,
    name: String,
    column: String,
    referenced_table: Option<String>,
    referenced_column: Option<String>,
}

impl DuckdbConnection {
    async fn key_columns(&self, schema: &str, table_name: &str) -> Result<Vec<KeyColumn>> {
        // Unnesting both lists in one select pairs them by position, and a
        // primary key's empty referenced list pads with NULL.
        let rows = self
            .catalog_rows(
                "SELECT constraint_type, constraint_name,
                        unnest(constraint_column_names), referenced_table,
                        unnest(referenced_column_names)
                 FROM duckdb_constraints()
                 WHERE database_name = current_database() AND schema_name = ? AND table_name = ?
                   AND constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY')
                 ORDER BY constraint_index",
                vec![schema.to_string(), table_name.to_string()],
            )
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let get = |index: usize| row.get(index).cloned().flatten();
                Some(KeyColumn {
                    kind: get(0)?,
                    name: get(1).unwrap_or_default(),
                    column: get(2)?,
                    referenced_table: get(3),
                    referenced_column: get(4),
                })
            })
            .collect())
    }
}

/// The columns of an index from `duckdb_indexes().expressions`, which lists
/// them as `[a, "b c"]`.
fn index_columns(expressions: Option<&str>) -> Vec<String> {
    let Some(expressions) = expressions else {
        return Vec::new();
    };
    let inner = expressions
        .trim()
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(expressions);
    inner
        .split(',')
        .map(|expression| {
            let expression = expression.trim().trim_matches('\'');
            expression
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .map(|name| name.replace("\"\"", "\""))
                .unwrap_or_else(|| expression.to_string())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_string;

    async fn open(path: &std::path::Path, database: Option<&str>) -> DuckdbConnection {
        let connection_string = connection_string(&path.to_string_lossy(), false, None, database);
        let mut connection = DuckdbConnection::new(&connection_string).expect("new");
        connection
            .connect(&connection_string)
            .await
            .expect("connect");
        connection
    }

    fn cells(result: &QueryResult) -> Vec<Option<&str>> {
        result
            .rows
            .first()
            .map(|row| row.iter().map(Option::as_deref).collect())
            .unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn values_of_every_kind_read_as_text() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection = open(&dir.path().join("types.duckdb"), None).await;
        let result = connection
            .execute_query(
                "SELECT 42::INTEGER AS i, 12345678901234567890::HUGEINT AS huge, 3.14::DECIMAL(5, 2) AS d,
                        'text' AS t, true AS b, DATE '2024-03-01' AS day,
                        TIMESTAMP '2024-03-01 10:20:30' AS ts,
                        TIMESTAMP '2024-03-01 10:20:30.5' AS ts_fraction,
                        TIME '08:15:00' AS clock,
                        INTERVAL 3 DAY AS span, [1, 2, 3] AS list, {'a': 1, 'b': 'x'} AS s,
                        MAP {'k': 1} AS m, '\\xCA\\xFE'::BLOB AS bytes,
                        '6f1c2c3e-5d4b-4a1e-9b1a-1c2d3e4f5a6b'::UUID AS id, NULL AS nothing",
                None,
                None,
            )
            .await
            .expect("query");
        assert_eq!(
            result.columns,
            [
                "i",
                "huge",
                "d",
                "t",
                "b",
                "day",
                "ts",
                "ts_fraction",
                "clock",
                "span",
                "list",
                "s",
                "m",
                "bytes",
                "id",
                "nothing"
            ]
        );
        assert_eq!(
            cells(&result),
            [
                Some("42"),
                Some("12345678901234567890"),
                Some("3.14"),
                Some("text"),
                Some("true"),
                Some("2024-03-01"),
                Some("2024-03-01 10:20:30"),
                Some("2024-03-01 10:20:30.500"),
                Some("08:15:00"),
                Some("3 days"),
                Some("[1, 2, 3]"),
                Some("{a: 1, b: x}"),
                Some("{k: 1}"),
                Some("0xcafe"),
                Some("6f1c2c3e-5d4b-4a1e-9b1a-1c2d3e4f5a6b"),
                None,
            ]
        );
        assert_eq!(result.column_types[0], blanco_core::ColumnType::Integer);
        assert_eq!(result.column_types[13], blanco_core::ColumnType::Binary);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn scripts_give_one_result_per_statement_and_writes_count_rows() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection = open(&dir.path().join("script.duckdb"), None).await;
        let results = connection
            .execute_script(
                "CREATE TABLE t (id INTEGER PRIMARY KEY, name VARCHAR);
                 INSERT INTO t VALUES (1, 'a'), (2, 'b');
                 SELECT * FROM t WHERE id > 5;
                 UPDATE t SET name = 'c' WHERE id = 1 RETURNING id, name",
                None,
            )
            .await
            .expect("script");
        assert_eq!(results.len(), 4);
        assert_eq!(results[1].rows_affected, 2);
        assert!(results[1].columns.is_empty(), "{:?}", results[1].columns);
        assert_eq!(
            results[2].columns,
            ["id", "name"],
            "an empty result keeps its header"
        );
        assert!(results[2].rows.is_empty());
        assert_eq!(cells(&results[3]), [Some("1"), Some("c")]);
        assert!(results.iter().all(|result| result.timing.is_some()));

        let changed = connection
            .execute_write(
                "UPDATE t SET name = ? WHERE id = ?",
                None,
                &[None, Some("2".to_string())],
            )
            .await
            .expect("write with text parameters");
        assert_eq!(changed, 1);
        let result = connection
            .execute_query(
                "SELECT name IS NULL FROM t WHERE id = ?",
                None,
                Some(&["2".to_string()]),
            )
            .await
            .expect("query with a parameter");
        assert_eq!(cells(&result), [Some("true")]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn session_state_carries_over_between_runs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection = open(&dir.path().join("session.duckdb"), None).await;
        connection
            .execute_script("CREATE TEMP TABLE scratch AS SELECT 1 AS n", None)
            .await
            .expect("temp table");
        let result = connection
            .execute_query("SELECT n FROM scratch", None, None)
            .await
            .expect("temp table is still there");
        assert_eq!(cells(&result), [Some("1")]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_batch_rolls_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection = open(&dir.path().join("batch.duckdb"), None).await;
        connection
            .execute_script("CREATE TABLE t (id INTEGER PRIMARY KEY)", None)
            .await
            .expect("create");
        let operation = |sql: &str, parameters: Vec<Option<String>>| WriteOperation {
            sql: sql.to_string(),
            parameters,
        };
        let failure = connection
            .execute_operations_transactional(
                &[
                    operation("INSERT INTO t VALUES (?)", vec![Some("1".to_string())]),
                    operation("INSERT INTO t VALUES (?)", vec![Some("1".to_string())]),
                ],
                None,
            )
            .await
            .expect_err("the duplicate key fails");
        assert_eq!(failure.applied, 0);
        let result = connection
            .execute_query("SELECT count(*) FROM t", None, None)
            .await
            .expect("count");
        assert_eq!(cells(&result), [Some("0")]);

        let outcome = connection
            .execute_operations_transactional(
                &[operation(
                    "INSERT INTO t VALUES (?)",
                    vec![Some("1".to_string())],
                )],
                None,
            )
            .await
            .expect("insert");
        assert_eq!(outcome.rows_affected, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_catalog_describes_tables_keys_and_databases() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("shop.duckdb");
        let connection = open(&path, None).await;
        connection
            .execute_script(
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, name VARCHAR NOT NULL DEFAULT 'x');
                 CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers (id), total DECIMAL(10, 2));
                 CREATE INDEX orders_total ON orders (total);
                 CREATE VIEW big_orders AS SELECT * FROM orders WHERE total > 100;
                 COMMENT ON TABLE orders IS 'Every order';
                 CREATE SCHEMA archive;
                 CREATE TABLE archive.old_orders (id INTEGER);
                 CREATE MACRO add_one(x) AS x + 1;
                 INSERT INTO customers (id) SELECT range FROM range(1000);",
                None,
            )
            .await
            .expect("setup");

        assert_eq!(
            connection.get_databases().await.expect("databases"),
            ["shop"]
        );
        assert_eq!(
            connection.get_schemas().await.expect("schemas"),
            ["archive", "main"]
        );
        assert_eq!(
            connection.get_tables(None).await.expect("tables"),
            ["customers", "orders"]
        );
        assert_eq!(
            connection
                .get_tables(Some("archive"))
                .await
                .expect("tables"),
            ["old_orders"]
        );
        assert_eq!(
            connection.get_views(None).await.expect("views"),
            ["big_orders"]
        );
        assert_eq!(
            connection.list_functions(None).await.expect("macros"),
            ["add_one"]
        );

        let columns = connection
            .get_columns_for_table("orders", None)
            .await
            .expect("columns");
        let summary: Vec<(&str, &str, bool, bool)> = columns
            .iter()
            .map(|column| {
                (
                    column.name.as_str(),
                    column.data_type.as_str(),
                    column.is_nullable,
                    column.is_primary_key,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("id", "INTEGER", false, true),
                ("customer_id", "INTEGER", true, false),
                ("total", "DECIMAL(10,2)", true, false),
            ]
        );
        let reference = columns[1].foreign_key.as_ref().expect("foreign key");
        assert_eq!(reference.foreign_table_name, "customers");
        assert_eq!(reference.foreign_column_name, "id");
        let name = connection
            .get_columns_for_table("customers", None)
            .await
            .expect("columns");
        assert_eq!(name[1].default_value.as_deref(), Some("'x'"));

        let keys = connection
            .get_foreign_keys_for_table("orders", None)
            .await
            .expect("foreign keys");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].columns, ["customer_id"]);
        assert_eq!(keys[0].referenced_columns, ["id"]);

        let indexes = connection
            .get_indexes_for_table("orders", None)
            .await
            .expect("indexes");
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].name, "orders_total");
        assert_eq!(indexes[0].column_names, ["total"]);

        let ddl = connection.table_ddl(None, "orders").await.expect("ddl");
        assert!(ddl.starts_with("CREATE TABLE orders"), "{ddl}");
        assert!(ddl.contains("CREATE INDEX orders_total"), "{ddl}");

        let comments = connection
            .get_object_comments(None)
            .await
            .expect("comments");
        assert_eq!(
            comments.relations.get("orders").map(String::as_str),
            Some("Every order")
        );

        let entities = connection
            .get_queryable_entities(None)
            .await
            .expect("entities");
        assert_eq!(entities.len(), 3);

        connection
            .execute_script("CHECKPOINT", None)
            .await
            .expect("checkpoint");
        let sizes = connection.get_database_sizes().await.expect("sizes");
        assert!(sizes.get("shop").is_some_and(|size| *size > 0), "{sizes:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_attached_database_is_listed_and_opened_by_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("sales.duckdb");
        let other = dir.path().join("other.duckdb");
        let connection = open(&path, None).await;
        connection
            .execute_script(
                &format!(
                    "ATTACH '{}' AS other; CREATE TABLE other.things (id INTEGER);",
                    other.to_string_lossy()
                ),
                None,
            )
            .await
            .expect("attach");
        assert_eq!(
            connection.get_databases().await.expect("databases"),
            ["other", "sales"]
        );

        // The service opens one connection per database. They share the
        // instance, so the attachment is visible and the file opens once.
        let in_other = open(&path, Some("other")).await;
        assert_eq!(in_other.get_tables(None).await.expect("tables"), ["things"]);
        let result = in_other
            .execute_query("SELECT current_database()", None, None)
            .await
            .expect("query");
        assert_eq!(cells(&result), [Some("other")]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_only_connections_refuse_writes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("frozen.duckdb");
        {
            let connection = open(&path, None).await;
            connection
                .execute_script("CREATE TABLE t (id INTEGER)", None)
                .await
                .expect("create");
        }
        let connection_string = connection_string(&path.to_string_lossy(), true, None, None);
        let mut connection = DuckdbConnection::new(&connection_string).expect("new");
        connection
            .connect(&connection_string)
            .await
            .expect("connect");
        let error = connection
            .execute_script("INSERT INTO t VALUES (1)", None)
            .await
            .expect_err("read only");
        assert!(format!("{error:#}").contains("read-only"), "{error:#}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_a_running_query_interrupts_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let connection = open(&dir.path().join("slow.duckdb"), None).await;
        let slow = connection.execute_query(
            "SELECT count(*) FROM range(1000000000000) AS a(x) WHERE x % 7 = 3",
            None,
            None,
        );
        let timed_out = tokio::time::timeout(std::time::Duration::from_millis(300), slow).await;
        assert!(timed_out.is_err(), "the slow query should still be running");

        // The interrupted query releases the session promptly.
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            connection.execute_query("SELECT 1", None, None),
        )
        .await
        .expect("the session is free again")
        .expect("query");
        assert_eq!(cells(&next), [Some("1")]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn in_memory_databases_are_private_to_their_connection() {
        let first = connection_string(crate::MEMORY_PATH, false, Some("1"), None);
        let second = connection_string(crate::MEMORY_PATH, false, Some("2"), None);
        let mut one = DuckdbConnection::new(&first).expect("new");
        one.connect(&first).await.expect("connect");
        let mut two = DuckdbConnection::new(&second).expect("new");
        two.connect(&second).await.expect("connect");
        one.execute_script("CREATE TABLE t (id INTEGER)", None)
            .await
            .expect("create");
        assert_eq!(one.get_tables(None).await.expect("tables"), ["t"]);
        assert!(two.get_tables(None).await.expect("tables").is_empty());
    }

    /// The deferred read and the binary filters build SQL from blanco-core's
    /// dialect, which DuckDB has to accept: its text functions take no BLOB.
    #[tokio::test(flavor = "multi_thread")]
    async fn deferred_values_and_binary_filters_run_on_duckdb() {
        use blanco_core::{FilterCondition, FilterOperator};

        let dir = tempfile::tempdir().expect("temp dir");
        let connection = open(&dir.path().join("large.duckdb"), None).await;
        connection
            .execute_script(
                "CREATE TABLE docs (id INTEGER PRIMARY KEY, body VARCHAR, data BLOB);
                 INSERT INTO docs VALUES
                     (1, repeat('é', 2000), from_hex(repeat('CAFE', 2000))),
                     (2, 'short', from_hex('01CAFE'))",
                None,
            )
            .await
            .expect("setup");

        let dialect = DatabaseType::DuckDb.dialect();
        let columns = connection
            .get_columns_for_table("docs", None)
            .await
            .expect("columns");
        let plan = dialect
            .deferred_read("SELECT * FROM docs", &columns, 1024)
            .expect("the large columns are deferred");
        let mut result = connection
            .execute_query(&plan.query, None, None)
            .await
            .expect("deferred read runs");
        let deferred = plan.apply(&mut result, 1024);
        assert_eq!(result.columns, ["id", "body", "data"]);
        let sizes: Vec<(usize, usize, u64)> = deferred
            .iter()
            .map(|cell| (cell.row, cell.column, cell.size))
            .collect();
        assert_eq!(sizes, [(0, 1, 4000), (0, 2, 4000)]);
        assert_eq!(result.rows[1][1].as_deref(), Some("short"));

        let matching = |operator: FilterOperator, value: &str| {
            let predicate = dialect.filter_predicate(&FilterCondition {
                column: "data".to_string(),
                operator,
                value: value.to_string(),
                numeric: false,
                binary: true,
            });
            let connection = &connection;
            async move {
                let result = connection
                    .execute_query(
                        &format!("SELECT id FROM docs WHERE {predicate} ORDER BY id"),
                        None,
                        None,
                    )
                    .await
                    .unwrap_or_else(|error| panic!("{predicate}: {error:#}"));
                result
                    .rows
                    .into_iter()
                    .filter_map(|row| row.into_iter().next().flatten())
                    .collect::<Vec<String>>()
            }
        };
        assert_eq!(matching(FilterOperator::Equals, "0x01cafe").await, ["2"]);
        assert_eq!(matching(FilterOperator::StartsWith, "cafe").await, ["1"]);
        assert_eq!(
            matching(FilterOperator::EndsWith, "0xcafe").await,
            ["1", "2"]
        );
        assert_eq!(matching(FilterOperator::Contains, "01ca").await, ["2"]);
        // 1CAF appears in the digits of row 2 (01 CA FE) only across two
        // bytes, so it is not a match.
        assert!(matching(FilterOperator::Contains, "1caf").await.is_empty());
        assert_eq!(matching(FilterOperator::NotContains, "01").await, ["1"]);
    }

    #[test]
    fn index_expressions_name_their_columns() {
        assert_eq!(index_columns(Some("[total]")), ["total"]);
        assert_eq!(index_columns(Some("[a, \"b c\"]")), ["a", "b c"]);
        assert!(index_columns(None).is_empty());
    }
}
