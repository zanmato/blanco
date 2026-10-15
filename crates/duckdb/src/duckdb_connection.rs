//! DuckDB driver. DuckDB runs in process on a file (or in memory), through
//! duckdb-rs, which is synchronous, so every call runs on a blocking thread.
//!
//! One process may open a database file only once, so connections to the same
//! file share one DuckDB instance from [`OPEN_DATABASES`] and each takes its
//! own DuckDB connection from it. A [`DuckdbConnection`] keeps one session for
//! the statements the user runs, so `SET`, `USE`, temporary tables and open
//! transactions carry over between runs, and takes a fresh connection for each
//! catalog lookup so the sidebar does not wait behind a long query.

mod connection_impl;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use anyhow::{Context as _, Result};
use blanco_core::{ColumnType, QueryResult, StatementTimer};
use duckdb::arrow::array::{Array, AsArray as _};
use duckdb::arrow::datatypes::{DataType, Schema};
use duckdb::arrow::util::display::{ArrayFormatter, FormatOptions};
use duckdb::{AccessMode, Config, InterruptHandle};

/// Open DuckDB instances by [`Location::cache_key`]. Weak, so an instance
/// closes, and releases its file lock, once its last connection is gone.
static OPEN_DATABASES: LazyLock<Mutex<HashMap<String, Weak<SharedDatabase>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// What a connection string names: a file or a private in-memory database,
/// how to open it and which of its attached databases to work in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Location {
    /// The file path, or `:memory:`.
    path: String,
    read_only: bool,
    /// Tells the in-memory databases of different connections apart.
    memory_id: Option<String>,
    /// The attached database statements run in, `None` for the default one.
    database: Option<String>,
}

pub const MEMORY_PATH: &str = ":memory:";

impl Location {
    /// Reads `duckdb:<path>`, optionally followed by `?` and `&`-separated
    /// `read_only=1`, `memory_id=<id>` and `database=<name>` options.
    fn parse(connection_string: &str) -> Result<Self> {
        let rest = connection_string
            .strip_prefix("duckdb:")
            .unwrap_or(connection_string);
        let (path, options) = match rest.rsplit_once('?') {
            Some((path, options)) if options.split('&').all(is_option) => (path, options),
            _ => (rest, ""),
        };
        if path.is_empty() {
            anyhow::bail!("The DuckDB connection has no file path");
        }
        let mut location = Location {
            path: path.to_string(),
            read_only: false,
            memory_id: None,
            database: None,
        };
        for option in options.split('&').filter(|option| !option.is_empty()) {
            let (key, value) = option.split_once('=').unwrap_or((option, ""));
            match key {
                "read_only" => location.read_only = value == "1",
                "memory_id" => location.memory_id = Some(value.to_string()),
                "database" => {
                    location.database = Some(value.to_string()).filter(|name| !name.is_empty())
                }
                _ => {}
            }
        }
        Ok(location)
    }

    fn is_memory(&self) -> bool {
        self.path == MEMORY_PATH
    }

    fn cache_key(&self) -> String {
        if self.is_memory() {
            format!("{MEMORY_PATH}{}", self.memory_id.as_deref().unwrap_or(""))
        } else {
            std::fs::canonicalize(&self.path)
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| self.path.clone())
        }
    }
}

fn is_option(option: &str) -> bool {
    let key = option.split_once('=').map_or(option, |(key, _)| key);
    matches!(key, "read_only" | "memory_id" | "database")
}

/// The connection string for `path`, as [`Location::parse`] reads it.
pub fn connection_string(
    path: &str,
    read_only: bool,
    memory_id: Option<&str>,
    database: Option<&str>,
) -> String {
    let mut options = Vec::new();
    if read_only {
        options.push("read_only=1".to_string());
    }
    if let Some(memory_id) = memory_id {
        options.push(format!("memory_id={memory_id}"));
    }
    if let Some(database) = database.filter(|database| !database.is_empty()) {
        options.push(format!("database={database}"));
    }
    if options.is_empty() {
        format!("duckdb:{path}")
    } else {
        format!("duckdb:{path}?{}", options.join("&"))
    }
}

/// One open DuckDB instance. Its connection is only used to make more.
struct SharedDatabase {
    root: Mutex<duckdb::Connection>,
}

impl SharedDatabase {
    fn open(location: &Location) -> Result<Arc<Self>> {
        let key = location.cache_key();
        let mut open = OPEN_DATABASES
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(shared) = open.get(&key).and_then(Weak::upgrade) {
            return Ok(shared);
        }
        let connection = if location.is_memory() {
            duckdb::Connection::open_in_memory()?
        } else {
            let mode = if location.read_only {
                AccessMode::ReadOnly
            } else {
                AccessMode::ReadWrite
            };
            duckdb::Connection::open_with_flags(
                &location.path,
                Config::default().access_mode(mode)?,
            )
            .map_err(|error| open_error(&location.path, error))?
        };
        let shared = Arc::new(Self {
            root: Mutex::new(connection),
        });
        open.retain(|_, database| database.strong_count() > 0);
        open.insert(key, Arc::downgrade(&shared));
        Ok(shared)
    }

    fn connect(&self) -> Result<duckdb::Connection> {
        Ok(self
            .root
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_clone()?)
    }
}

/// DuckDB lets one process write a file at a time, so a file open in the
/// DuckDB CLI or a notebook refuses Blanco. Say what to do about it.
fn open_error(path: &str, error: duckdb::Error) -> anyhow::Error {
    let message = error.to_string();
    if message.contains("lock") {
        anyhow::anyhow!(
            "{path} is open for writing in another program. Close it there, or connect read only. ({message})"
        )
    } else {
        anyhow::Error::new(error).context(format!("Failed to open {path}"))
    }
}

/// The session the user's statements run in, and the handle that interrupts
/// whatever it is running.
struct Session {
    connection: Arc<Mutex<duckdb::Connection>>,
    interrupt: Arc<InterruptHandle>,
}

pub struct DuckdbConnection {
    location: Location,
    display_name: String,
    shared: Option<Arc<SharedDatabase>>,
    session: Option<Session>,
}

/// Interrupts the session's statement when dropped before [`Self::disarm`],
/// which happens when the editor cancels a run by dropping its future.
struct InterruptOnDrop {
    interrupt: Option<Arc<InterruptHandle>>,
}

impl InterruptOnDrop {
    fn disarm(mut self) {
        self.interrupt = None;
    }
}

impl Drop for InterruptOnDrop {
    fn drop(&mut self) {
        if let Some(interrupt) = self.interrupt.take() {
            interrupt.interrupt();
        }
    }
}

impl DuckdbConnection {
    pub fn new(connection_string: &str) -> Result<Self> {
        let location = Location::parse(connection_string)?;
        Ok(Self {
            display_name: display_name(&location.path),
            location,
            shared: None,
            session: None,
        })
    }

    fn shared(&self) -> Result<&Arc<SharedDatabase>> {
        self.shared
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to DuckDB"))
    }

    /// A new DuckDB connection working in this connection's database.
    fn fresh_connection(&self) -> Result<duckdb::Connection> {
        let connection = self.shared()?.connect()?;
        use_database(&connection, self.location.database.as_deref())?;
        Ok(connection)
    }

    /// Run `work` on a blocking thread with the user's session, interrupting
    /// it if the caller stops waiting.
    async fn with_session<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut duckdb::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to DuckDB"))?;
        let connection = Arc::clone(&session.connection);
        let guard = InterruptOnDrop {
            interrupt: Some(Arc::clone(&session.interrupt)),
        };
        let result = tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
            work(&mut connection)
        })
        .await
        .context("The DuckDB worker stopped")?;
        guard.disarm();
        result
    }

    /// Run `work` on a blocking thread with a fresh connection, for catalog
    /// lookups that should not queue behind the user's statements.
    async fn with_fresh_connection<T: Send + 'static>(
        &self,
        work: impl FnOnce(&duckdb::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let connection = self.fresh_connection()?;
        tokio::task::spawn_blocking(move || work(&connection))
            .await
            .context("The DuckDB worker stopped")?
    }

    /// The rows of a catalog query, with text parameters.
    async fn catalog_rows(
        &self,
        sql: &'static str,
        parameters: Vec<String>,
    ) -> Result<Vec<Vec<Option<String>>>> {
        self.with_fresh_connection(move |connection| {
            Ok(run_statement(connection, sql, &text_parameters(&parameters))?.rows)
        })
        .await
    }
}

fn use_database(connection: &duckdb::Connection, database: Option<&str>) -> Result<()> {
    if let Some(database) = database {
        let quoted = blanco_core::DatabaseType::DuckDb
            .dialect()
            .quote_identifier(database);
        connection
            .execute_batch(&format!("USE {quoted}"))
            .with_context(|| format!("Failed to switch to database {database}"))?;
    }
    Ok(())
}

fn display_name(path: &str) -> String {
    if path == MEMORY_PATH {
        return "DuckDB - in memory".to_string();
    }
    let name = std::path::Path::new(path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    format!("DuckDB - {name}")
}

fn text_parameters(parameters: &[String]) -> Vec<Option<&str>> {
    parameters
        .iter()
        .map(|parameter| Some(parameter.as_str()))
        .collect()
}

/// Every result-returning statement is converted through Arrow, whose
/// formatter knows every DuckDB type including nested ones. These formats
/// match how DuckDB prints dates and times itself.
const FORMAT_OPTIONS: FormatOptions<'static> = FormatOptions::new()
    .with_null("NULL")
    .with_date_format(Some("%Y-%m-%d"))
    .with_timestamp_format(Some("%Y-%m-%d %H:%M:%S%.f"))
    .with_timestamp_tz_format(Some("%Y-%m-%d %H:%M:%S%.f%:z"))
    .with_time_format(Some("%H:%M:%S%.f"));

/// Run one statement and read its whole result. A statement that only
/// changes rows reports them in `rows_affected` rather than as DuckDB's
/// one-column `Count` result.
fn run_statement(
    connection: &duckdb::Connection,
    sql: &str,
    parameters: &[Option<&str>],
) -> Result<QueryResult> {
    let mut statement = connection.prepare(sql)?;
    let mut timer = StatementTimer::start();
    let rows_changed = statement.execute(duckdb::params_from_iter(parameters.iter()))?;
    let schema = statement.schema();

    if changes_rows_only(sql, &schema) {
        return Ok(QueryResult {
            rows_affected: rows_changed as u64,
            timing: Some(timer.finish()),
            ..QueryResult::default()
        });
    }

    let columns: Vec<String> = schema
        .fields()
        .iter()
        .map(|field| field.name().to_string())
        .collect();
    let column_types = schema
        .fields()
        .iter()
        .map(|field| column_type(field.data_type()))
        .collect();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    while let Some(batch) = statement.step()? {
        if batch.is_empty() {
            continue;
        }
        timer.row();
        let formatters = batch
            .columns()
            .iter()
            .map(|array| ArrayFormatter::try_new(array.as_ref(), &FORMAT_OPTIONS))
            .collect::<Result<Vec<_>, _>>()?;
        for row in 0..batch.len() {
            rows.push(
                batch
                    .columns()
                    .iter()
                    .zip(&formatters)
                    .map(|(array, formatter)| cell_text(array.as_ref(), formatter, row))
                    .collect(),
            );
        }
    }

    Ok(QueryResult {
        columns,
        column_types,
        rows,
        rows_affected: rows_changed as u64,
        timing: Some(timer.finish()),
        ..QueryResult::default()
    })
}

/// Whether `sql` is an `INSERT`, `UPDATE` or `DELETE` without `RETURNING`,
/// which DuckDB answers with a single `Count` column.
fn changes_rows_only(sql: &str, schema: &Schema) -> bool {
    let only_count = matches!(schema.fields().as_ref(), [field] if field.name() == "Count");
    only_count
        && blanco_core::write_guard::write_kinds(blanco_core::DatabaseType::DuckDb, sql)
            .iter()
            .any(|kind| matches!(kind.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE"))
}

/// A cell as text: bytes in Blanco's `0x` hex notation, everything else as
/// the Arrow formatter writes it.
fn cell_text(array: &dyn Array, formatter: &ArrayFormatter, row: usize) -> Option<String> {
    if array.is_null(row) {
        return None;
    }
    let bytes = match array.data_type() {
        DataType::Binary => Some(array.as_binary::<i32>().value(row)),
        DataType::LargeBinary => Some(array.as_binary::<i64>().value(row)),
        DataType::FixedSizeBinary(_) => Some(array.as_fixed_size_binary().value(row)),
        DataType::BinaryView => Some(array.as_binary_view().value(row)),
        _ => None,
    };
    Some(match bytes {
        Some(bytes) => blanco_core::binary::encode(bytes),
        None => formatter.value(row).to_string(),
    })
}

fn column_type(data_type: &DataType) -> ColumnType {
    match data_type {
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 => ColumnType::Integer,
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => {
            ColumnType::UnsignedInteger
        }
        DataType::Float16
        | DataType::Float32
        | DataType::Float64
        | DataType::Decimal32(_, _)
        | DataType::Decimal64(_, _)
        | DataType::Decimal128(_, _)
        | DataType::Decimal256(_, _) => ColumnType::Numeric,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => ColumnType::Text,
        DataType::Boolean => ColumnType::Boolean,
        DataType::Date32
        | DataType::Date64
        | DataType::Timestamp(_, _)
        | DataType::Time32(_)
        | DataType::Time64(_) => ColumnType::DateTime,
        DataType::Binary
        | DataType::LargeBinary
        | DataType::FixedSizeBinary(_)
        | DataType::BinaryView => ColumnType::Binary,
        DataType::List(_)
        | DataType::LargeList(_)
        | DataType::ListView(_)
        | DataType::LargeListView(_)
        | DataType::FixedSizeList(_, _) => ColumnType::Array,
        DataType::Struct(_) | DataType::Map(_, _) | DataType::Union(_, _) => ColumnType::Json,
        _ => ColumnType::Unknown,
    }
}

/// Run a script one statement at a time, one result per statement. DuckDB's
/// own multi-statement prepare runs all but the last statement silently, so
/// the script is split first.
fn run_script(connection: &duckdb::Connection, script: &str) -> Result<Vec<QueryResult>> {
    let statements = blanco_core::statement_stream::split_script(
        blanco_core::DatabaseType::DuckDb.dialect(),
        script,
    );
    if statements.is_empty() {
        return Ok(vec![QueryResult {
            timing: Some(StatementTimer::start().finish()),
            ..QueryResult::default()
        }]);
    }
    statements
        .iter()
        .map(|statement| run_statement(connection, statement, &[]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_strings_round_trip() {
        let location = Location::parse(&connection_string(
            "/data/sales.duckdb",
            true,
            None,
            Some("other"),
        ))
        .expect("parse");
        assert_eq!(
            location,
            Location {
                path: "/data/sales.duckdb".to_string(),
                read_only: true,
                memory_id: None,
                database: Some("other".to_string()),
            }
        );
        let location = Location::parse("duckdb:/tmp/what?.duckdb").expect("parse");
        assert_eq!(location.path, "/tmp/what?.duckdb");
        let location = Location::parse(&connection_string(MEMORY_PATH, false, Some("7"), None))
            .expect("parse");
        assert!(location.is_memory());
        assert_eq!(location.cache_key(), ":memory:7");
        assert!(Location::parse("duckdb:").is_err());
    }
}
