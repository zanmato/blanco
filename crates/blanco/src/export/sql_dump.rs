//! Write several tables, or a whole schema, to one SQL file: structure, data
//! or both per table, in dependency order.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context as _, Result};
use blanco_core::sql_dump::{InsertBatcher, dependency_order};
use blanco_core::{ColumnType, Connection, Dialect};
use futures::StreamExt as _;

/// One table to dump and what to write for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpTable {
    pub schema: Option<String>,
    pub name: String,
    pub structure: bool,
    pub data: bool,
}

impl DumpTable {
    /// `schema.name`, the key tables are ordered by.
    fn key(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DumpOptions {
    /// `DROP TABLE IF EXISTS` before each `CREATE TABLE`.
    pub drop_existing: bool,
    /// Turn foreign key checks off around the load. Also done when the
    /// tables reference each other in a cycle.
    pub disable_foreign_keys: bool,
    /// Rows per INSERT statement.
    pub rows_per_insert: usize,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            drop_existing: false,
            disable_foreign_keys: false,
            rows_per_insert: 100,
        }
    }
}

/// Where a running dump is, for the dialog.
#[derive(Default)]
pub struct DumpProgress {
    pub tables_done: AtomicUsize,
    pub rows: AtomicU64,
    pub cancelled: AtomicBool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DumpSummary {
    pub tables: usize,
    pub rows: u64,
    /// The tables reference each other in a cycle, so they could not all be
    /// ordered after the tables they reference.
    pub cyclic: bool,
}

struct TablePlan {
    table: DumpTable,
    quoted: String,
    /// Columns written by INSERT: all but the generated ones.
    insert_columns: Vec<String>,
    /// Integer primary key columns, whose sequence (if any) is moved past
    /// the loaded rows.
    sequence_columns: Vec<String>,
}

/// Write the dump of `tables` to `out`.
pub async fn write_dump(
    connection: &dyn Connection,
    dialect: Dialect,
    database: &str,
    tables: &[DumpTable],
    options: &DumpOptions,
    out: &mut (dyn Write + Send),
    progress: &DumpProgress,
) -> Result<DumpSummary> {
    let mut plans: BTreeMap<String, TablePlan> = BTreeMap::new();
    let mut references: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for table in tables.iter().filter(|table| table.structure || table.data) {
        let columns = connection
            .get_columns_for_table(&table.name, table.schema.as_deref())
            .await
            .with_context(|| format!("Could not read the columns of {}", table.key()))?;
        let generated: BTreeSet<String> = connection
            .get_generated_columns(&table.name, table.schema.as_deref())
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let key = table.key();
        references.insert(
            key.clone(),
            columns
                .iter()
                .filter_map(|column| column.foreign_key.as_ref())
                .map(|foreign_key| {
                    let referenced = &foreign_key.foreign_table_name;
                    match (&table.schema, referenced.contains('.')) {
                        (Some(schema), false) => format!("{schema}.{referenced}"),
                        _ => referenced.clone(),
                    }
                })
                .collect(),
        );
        let sequence_columns = columns
            .iter()
            .filter(|column| {
                column.is_primary_key
                    && !generated.contains(&column.name)
                    && column.data_type.to_ascii_lowercase().contains("int")
            })
            .map(|column| column.name.clone())
            .collect();
        plans.insert(
            key,
            TablePlan {
                quoted: dialect.quote_qualified(table.schema.as_deref(), &table.name),
                sequence_columns,
                insert_columns: columns
                    .into_iter()
                    .map(|column| column.name)
                    .filter(|name| !generated.contains(name))
                    .collect(),
                table: table.clone(),
            },
        );
    }

    let (order, cyclic) = dependency_order(&references);
    let toggle = (options.disable_foreign_keys || cyclic)
        .then(|| dialect.foreign_key_check_toggle())
        .flatten();

    writeln!(
        out,
        "-- Blanco SQL dump of {database}, {} table(s)\n-- Written {}\n",
        order.len(),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z")
    )?;
    if cyclic && toggle.is_none() {
        writeln!(
            out,
            "-- The tables reference each other in a cycle and this database has no\n\
             -- session switch for foreign key checks; load the data with them off.\n"
        )?;
    }
    if let Some((off, _)) = toggle {
        writeln!(out, "{off}\n")?;
    }

    if options.drop_existing {
        for key in order.iter().rev() {
            let plan = &plans[key];
            if plan.table.structure {
                writeln!(out, "{}", dialect.drop_table_if_exists(&plan.quoted))?;
            }
        }
        writeln!(out)?;
    }

    let mut total_rows = 0u64;
    for key in &order {
        let plan = &plans[key];
        if progress.cancelled.load(Ordering::Relaxed) {
            anyhow::bail!("Export cancelled");
        }
        writeln!(out, "-- {key}")?;
        if plan.table.structure {
            match connection
                .table_ddl(plan.table.schema.as_deref(), &plan.table.name)
                .await
            {
                Ok(ddl) => {
                    let ddl = ddl.trim();
                    let terminator = if ddl.ends_with(';') { "" } else { ";" };
                    writeln!(out, "{ddl}{terminator}\n")?;
                }
                Err(error) => writeln!(out, "-- No CREATE statement for {key}: {error}\n")?,
            }
        }
        if plan.table.data && !plan.insert_columns.is_empty() {
            total_rows += write_rows(connection, dialect, database, plan, options, out, progress)
                .await
                .with_context(|| format!("Could not export the rows of {key}"))?;
            writeln!(out)?;
        }
        progress.tables_done.fetch_add(1, Ordering::Relaxed);
    }

    if let Some((_, on)) = toggle {
        writeln!(out, "{on}")?;
    }
    out.flush()?;
    Ok(DumpSummary {
        tables: order.len(),
        rows: total_rows,
        cyclic,
    })
}

async fn write_rows(
    connection: &dyn Connection,
    dialect: Dialect,
    database: &str,
    plan: &TablePlan,
    options: &DumpOptions,
    out: &mut (dyn Write + Send),
    progress: &DumpProgress,
) -> Result<u64> {
    let quoted_columns: Vec<String> = plan
        .insert_columns
        .iter()
        .map(|column| dialect.quote_identifier(column))
        .collect();
    let query = format!("SELECT {} FROM {}", quoted_columns.join(", "), plan.quoted);
    let (_names, types, mut rows) = connection
        .execute_query_stream_rows(&query, Some(database))
        .await?;
    let mut batcher = InsertBatcher::with_clause(
        &plan.quoted,
        &quoted_columns,
        dialect.insert_override_clause(),
        options.rows_per_insert,
    );
    let mut count = 0u64;
    while let Some(row) = rows.next().await {
        if progress.cancelled.load(Ordering::Relaxed) {
            anyhow::bail!("Export cancelled");
        }
        let row = row?;
        let literals: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(ix, value)| {
                dialect.value_literal(
                    value.as_deref(),
                    types.get(ix).copied().unwrap_or(ColumnType::Unknown),
                )
            })
            .collect();
        if let Some(statement) = batcher.push(&literals) {
            writeln!(out, "{statement}")?;
        }
        count += 1;
        progress.rows.fetch_add(1, Ordering::Relaxed);
    }
    if let Some(statement) = batcher.flush() {
        writeln!(out, "{statement}")?;
    }
    if count > 0 {
        for column in &plan.sequence_columns {
            if let Some(statement) = dialect.sequence_reset(&plan.quoted, column) {
                writeln!(out, "{statement}")?;
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sqlite(dir: &tempfile::TempDir, name: &str) -> Box<dyn Connection> {
        let path = dir.path().join(name).to_string_lossy().into_owned();
        let mut connection = sqlite::SqliteConnection::new(path.clone()).expect("connection");
        connection
            .connect(&format!("sqlite:{path}"))
            .await
            .expect("connect");
        Box::new(connection)
    }

    /// The statements of a dump this module wrote: no value in the tests
    /// holds `;` followed by a newline.
    fn statements(dump: &str) -> Vec<String> {
        dump.split(";\n")
            .map(|chunk| {
                chunk
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("--"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|statement| !statement.trim().is_empty())
            .collect()
    }

    #[tokio::test]
    async fn dumps_structure_and_data_in_dependency_order() {
        {
            let dir = tempfile::tempdir().unwrap();
            let connection = sqlite(&dir, "source.db").await;
            for statement in [
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT)",
                "CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER REFERENCES customers(id), total REAL, doubled REAL GENERATED ALWAYS AS (total * 2) VIRTUAL)",
                "CREATE TABLE audit_log (id INTEGER PRIMARY KEY, message TEXT)",
                "INSERT INTO customers VALUES (1, 'Ada'), (2, 'O''Brien'), (3, NULL)",
                "INSERT INTO orders (id, customer_id, total) VALUES (10, 1, 9.5), (11, 2, 20)",
                "INSERT INTO audit_log VALUES (1, 'noise')",
            ] {
                connection
                    .execute_query(statement, None, None)
                    .await
                    .unwrap();
            }
            let tables = [
                DumpTable {
                    schema: None,
                    name: "orders".into(),
                    structure: true,
                    data: true,
                },
                DumpTable {
                    schema: None,
                    name: "audit_log".into(),
                    structure: true,
                    data: false,
                },
                DumpTable {
                    schema: None,
                    name: "customers".into(),
                    structure: true,
                    data: true,
                },
            ];
            let options = DumpOptions {
                drop_existing: true,
                disable_foreign_keys: false,
                rows_per_insert: 2,
            };
            let mut out = Vec::new();
            let progress = DumpProgress::default();
            let summary = write_dump(
                connection.as_ref(),
                blanco_core::DatabaseType::SQLite.dialect(),
                "main",
                &tables,
                &options,
                &mut out,
                &progress,
            )
            .await
            .expect("dump");
            let dump = String::from_utf8(out).unwrap();
            assert_eq!(
                summary,
                DumpSummary {
                    tables: 3,
                    rows: 5,
                    cyclic: false
                }
            );

            // Drops in reverse dependency order, before any CREATE.
            let drop_orders = dump.find("DROP TABLE IF EXISTS \"orders\";").unwrap();
            let drop_customers = dump.find("DROP TABLE IF EXISTS \"customers\";").unwrap();
            assert!(drop_orders < drop_customers);
            // Customers are created and filled before the orders that refer to them.
            assert!(
                dump.find("CREATE TABLE customers").unwrap()
                    < dump.find("CREATE TABLE orders").unwrap()
            );
            assert!(dump.contains(
                "INSERT INTO \"customers\" (\"id\", \"name\") VALUES\n  (1, 'Ada'),\n  (2, 'O''Brien');\n\
                 INSERT INTO \"customers\" (\"id\", \"name\") VALUES\n  (3, NULL);"
            ), "{dump}");
            // The generated column is left out of the INSERT.
            assert!(dump.contains(
                "INSERT INTO \"orders\" (\"id\", \"customer_id\", \"total\") VALUES\n  (10, 1, 9.5),\n  (11, 2, 20);"
            ), "{dump}");
            // Structure only for the log table.
            assert!(dump.contains("CREATE TABLE audit_log"));
            assert!(!dump.contains("noise"));

            // The dump loads into an empty database.
            let target = sqlite(&dir, "target.db").await;
            for statement in statements(&dump) {
                target
                    .execute_query(&statement, None, None)
                    .await
                    .unwrap_or_else(|error| panic!("{statement}: {error}"));
            }
            let result = target
                .execute_query("SELECT doubled FROM orders ORDER BY id", None, None)
                .await
                .unwrap();
            assert_eq!(result.rows[1][0].as_deref(), Some("40"));
        }
    }

    #[tokio::test]
    async fn reference_cycles_turn_foreign_key_checks_off() {
        {
            let dir = tempfile::tempdir().unwrap();
            let connection = sqlite(&dir, "cycle.db").await;
            for statement in [
                "CREATE TABLE a (id INTEGER PRIMARY KEY, b_id INTEGER REFERENCES b(id))",
                "CREATE TABLE b (id INTEGER PRIMARY KEY, a_id INTEGER REFERENCES a(id))",
            ] {
                connection
                    .execute_query(statement, None, None)
                    .await
                    .unwrap();
            }
            let tables: Vec<DumpTable> = ["a", "b"]
                .into_iter()
                .map(|name| DumpTable {
                    schema: None,
                    name: name.into(),
                    structure: true,
                    data: true,
                })
                .collect();
            let mut out = Vec::new();
            let summary = write_dump(
                connection.as_ref(),
                blanco_core::DatabaseType::SQLite.dialect(),
                "main",
                &tables,
                &DumpOptions::default(),
                &mut out,
                &DumpProgress::default(),
            )
            .await
            .unwrap();
            let dump = String::from_utf8(out).unwrap();
            assert!(summary.cyclic);
            assert!(dump.contains("PRAGMA foreign_keys = OFF;"));
            assert!(dump.trim_end().ends_with("PRAGMA foreign_keys = ON;"));
        }
    }
}
