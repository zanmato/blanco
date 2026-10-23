use crate::DatabaseType;
use async_trait::async_trait;
use futures::Stream;
use std::time::{Duration, Instant};

/// Type of queryable database entity
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EntityType {
    Table,
    View,
    MaterializedView,
}

impl EntityType {
    pub fn display_name(&self) -> &'static str {
        match self {
            EntityType::Table => "Table",
            EntityType::View => "View",
            EntityType::MaterializedView => "Materialized View",
        }
    }
}

/// The schema objects the sidebar lists outside tables and views: routines
/// and triggers (`list_procedures`, `list_functions`, `list_triggers`), user
/// defined types (`list_types`) and sequences (`list_sequences`), whose DDL
/// `object_ddl` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Procedure,
    Function,
    Trigger,
    Enum,
    CompositeType,
    Domain,
    Sequence,
}

/// A user-defined function's signature as reported by the database catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignatureInfo {
    pub schema: Option<String>,
    pub name: String,
    /// One entry per parameter, as `name type` when the catalog names them and
    /// just the type otherwise.
    pub parameters: Vec<String>,
    pub return_type: Option<String>,
    pub comment: Option<String>,
}

impl ObjectKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            ObjectKind::Procedure => "Procedure",
            ObjectKind::Function => "Function",
            ObjectKind::Trigger => "Trigger",
            ObjectKind::Enum => "Enum",
            ObjectKind::CompositeType => "Composite type",
            ObjectKind::Domain => "Domain",
            ObjectKind::Sequence => "Sequence",
        }
    }
}

/// Comments stored on the objects of a schema, keyed by object name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectComments {
    /// Tables, views and materialized views.
    pub relations: std::collections::HashMap<String, String>,
    /// Functions and procedures.
    pub routines: std::collections::HashMap<String, String>,
}

impl ObjectComments {
    /// File a `(kind, name, comment)` row, `kind` being `r` for a relation
    /// and anything else for a routine. Empty comments are skipped.
    pub fn insert_row(&mut self, kind: &str, name: String, comment: String) {
        if comment.is_empty() {
            return;
        }
        let map = if kind == "r" {
            &mut self.relations
        } else {
            &mut self.routines
        };
        map.insert(name, comment);
    }
}

/// The table a table descends from: the partitioned table of a partition, or
/// the parent of a table created with `INHERITS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableParent {
    pub name: String,
    /// Whether the child is a partition of `name` rather than an inheritance
    /// child.
    pub is_partition: bool,
}

/// A queryable database entity with its type
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueryableEntity {
    pub name: String,
    pub entity_type: EntityType,
    /// The comment stored on the table or view, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Represents the semantic type of a database column
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ColumnType {
    /// Integer types: smallint, int, bigint, serial, etc.
    Integer,
    /// Unsigned integer types: INT UNSIGNED, BIGINT UNSIGNED, etc.
    UnsignedInteger,
    /// Numeric types: float, double, real, numeric, decimal, money
    Numeric,
    /// Text types: text, varchar, char, etc.
    Text,
    /// Boolean types: bool, boolean
    Boolean,
    /// DateTime types: timestamp, timestamptz, datetime, date
    DateTime,
    /// UUID type
    Uuid,
    /// JSON types: json, jsonb
    Json,
    /// Array types (PostgreSQL)
    Array,
    /// Binary types: bytea, blob, binary
    Binary,
    /// Key/value maps: PostgreSQL's hstore. Values are the server's text
    /// form, which `blanco_core::hstore` reads and writes.
    KeyValue,
    /// Unknown/fallback type
    Unknown,
}

impl ColumnType {
    /// Returns true if this column type is numeric (Integer, UnsignedInteger, or Numeric)
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Integer | Self::UnsignedInteger | Self::Numeric)
    }
}

/// How long one statement took, measured by the driver around its own row
/// loop, so connection setup and anything run before the statement is not
/// counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueryTiming {
    /// Until the first row arrived, or until the statement finished when it
    /// returned no rows.
    pub execution: Duration,
    /// From the first row to the last. `None` when no rows came back.
    pub fetch: Option<Duration>,
}

impl QueryTiming {
    pub fn total(&self) -> Duration {
        self.execution + self.fetch.unwrap_or_default()
    }

    /// The timings of several statements added up. A fetch time is kept when
    /// any of them returned rows.
    pub fn sum(timings: impl IntoIterator<Item = QueryTiming>) -> Option<QueryTiming> {
        timings.into_iter().reduce(|sum, timing| QueryTiming {
            execution: sum.execution + timing.execution,
            fetch: match (sum.fetch, timing.fetch) {
                (None, None) => None,
                (left, right) => Some(left.unwrap_or_default() + right.unwrap_or_default()),
            },
        })
    }
}

/// Measures [`QueryTiming`] inside a driver's row loop: start it right before
/// the statement is sent, call [`row`](Self::row) for every row and
/// [`finish`](Self::finish) when the statement is done.
#[derive(Debug, Clone, Copy)]
pub struct StatementTimer {
    started: Instant,
    first_row: Option<Instant>,
}

impl StatementTimer {
    pub fn start() -> Self {
        Self::started_at(Instant::now())
    }

    pub fn started_at(started: Instant) -> Self {
        Self {
            started,
            first_row: None,
        }
    }

    pub fn row(&mut self) {
        if self.first_row.is_none() {
            self.first_row = Some(Instant::now());
        }
    }

    pub fn finish(&self) -> QueryTiming {
        self.finish_at(Instant::now())
    }

    /// Finish at an earlier moment, for drivers that only learn a statement
    /// ended when the next one's first message arrives.
    pub fn finish_at(&self, finished: Instant) -> QueryTiming {
        match self.first_row {
            Some(first_row) => QueryTiming {
                execution: first_row - self.started,
                fetch: Some(finished - first_row),
            },
            None => QueryTiming {
                execution: finished - self.started,
                fetch: None,
            },
        }
    }

    /// Finish this statement and start timing the next one of a script, which
    /// the server begins as soon as this one is done.
    pub fn finish_and_restart(&mut self) -> QueryTiming {
        let timing = self.finish();
        *self = Self::start();
        timing
    }
}

/// Result of a database query
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub column_types: Vec<ColumnType>,
    pub rows: Vec<Vec<Option<String>>>,
    pub rows_affected: u64,
    pub query_text: Option<String>,
    pub timing: Option<QueryTiming>,
    pub is_error: bool,
    /// Table metadata extracted from the query (if applicable)
    pub table_name: Option<String>,
    /// Connection ID used for this query (for subsequent operations)
    pub connection_id: Option<i64>,
    /// Full column metadata for the table (including foreign keys)
    pub table_columns: Option<Vec<ColumnInfo>>,
}

/// The Redis type of a key, as reported by the `TYPE` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedisType {
    String,
    List,
    Set,
    Hash,
    ZSet,
    Stream,
    None,
}

impl RedisType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RedisType::String => "string",
            RedisType::List => "list",
            RedisType::Set => "set",
            RedisType::Hash => "hash",
            RedisType::ZSet => "zset",
            RedisType::Stream => "stream",
            RedisType::None => "none",
        }
    }

    pub fn from_type_reply(reply: &str) -> Self {
        match reply {
            "string" => RedisType::String,
            "list" => RedisType::List,
            "set" => RedisType::Set,
            "hash" => RedisType::Hash,
            "zset" => RedisType::ZSet,
            "stream" => RedisType::Stream,
            _ => RedisType::None,
        }
    }
}

/// The decoded value of a single Redis key, shaped per its type. Produced by the
/// key inspector (`Connection::inspect_key`) and rendered by a dedicated
/// key/value view rather than the tabular results table.
#[derive(Debug, Clone, PartialEq)]
pub enum RedisValue {
    /// A simple string value (GET).
    Str(String),
    /// An ordered list (LRANGE).
    List(Vec<String>),
    /// A hash of field/value pairs (HGETALL).
    Hash(Vec<(String, String)>),
    /// An unordered set of members (SMEMBERS).
    Set(Vec<String>),
    /// A sorted set of member/score pairs (ZRANGE WITHSCORES).
    ZSet(Vec<(String, f64)>),
    /// A stream of entries: (id, [field/value pairs]) (XRANGE).
    Stream(Vec<(String, Vec<(String, String)>)>),
    /// The key does not exist.
    None,
}

/// Inspection result for a single Redis key: its name, type, TTL, and decoded
/// value. The TTL is in seconds; `None` means the key has no expiry (-1) or does
/// not exist (-2).
#[derive(Debug, Clone, PartialEq)]
pub struct KeyValueResult {
    pub key: String,
    pub key_type: RedisType,
    pub ttl: Option<i64>,
    pub value: RedisValue,
    /// The compression a string value was stored with (`gzip`, `zstd`,
    /// `lz4`). The value shown is decompressed and cannot be edited.
    pub compression: Option<String>,
}

/// One page of keys from a key/value store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyListing {
    pub keys: Vec<String>,
    /// More keys match than `keys` holds.
    pub truncated: bool,
}

/// A result produced by a backend, either a relational/tabular query result or a
/// non-tabular key/value payload (Redis). The tabular variant is the common case
/// and reuses the full SQL results table; the key/value variant drives a
/// dedicated inspector view.
#[derive(Debug, Clone, PartialEq)]
pub enum ResultPayload {
    Tabular(QueryResult),
    KeyValue(KeyValueResult),
}

impl ResultPayload {
    /// Borrow the tabular result, if this payload is tabular.
    pub fn as_tabular(&self) -> Option<&QueryResult> {
        match self {
            ResultPayload::Tabular(result) => Some(result),
            ResultPayload::KeyValue(_) => None,
        }
    }
}

impl From<QueryResult> for ResultPayload {
    fn from(result: QueryResult) -> Self {
        ResultPayload::Tabular(result)
    }
}

/// Information about a foreign key relationship
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ForeignKeyInfo {
    pub foreign_table_name: String,
    pub foreign_column_name: String,
    pub constraint_name: Option<String>,
}

impl ForeignKeyInfo {
    /// Build a foreign key from a catalog row's referenced table/column.
    /// Returns `None` unless both are present, since a column without a complete
    /// reference is not a foreign key. Drivers share this so their
    /// introspection queries only differ in how they read the catalog columns.
    pub fn from_parts(
        foreign_table_name: Option<impl Into<String>>,
        foreign_column_name: Option<impl Into<String>>,
        constraint_name: Option<String>,
    ) -> Option<Self> {
        match (foreign_table_name, foreign_column_name) {
            (Some(table), Some(column)) => Some(Self {
                foreign_table_name: table.into(),
                foreign_column_name: column.into(),
                constraint_name,
            }),
            _ => None,
        }
    }
}

/// Information about an inbound foreign key (a row in another table that references this one)
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct InboundForeignKey {
    pub from_table: String,
    pub from_column: String,
    pub to_column: String,
    pub constraint_name: Option<String>,
}

/// One foreign key constraint declared on a table, as listed in the structure
/// tab. Unlike [`ForeignKeyInfo`], which describes a single column, this keeps
/// composite keys together and carries the referential actions.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ForeignKeyConstraint {
    pub name: String,
    /// Referencing columns on this table, in key order.
    pub columns: Vec<String>,
    /// Schema of the referenced table when the backend has schemas.
    pub referenced_schema: Option<String>,
    pub referenced_table: String,
    /// Referenced columns, paired by position with `columns`.
    pub referenced_columns: Vec<String>,
    /// Action as the catalog spells it (`CASCADE`, `NO ACTION`, ...).
    pub on_update: String,
    pub on_delete: String,
}

impl ForeignKeyConstraint {
    /// Fold catalog rows into constraints. Each row is one key column:
    /// `(name, referenced_schema, referenced_table, column, referenced_column,
    /// on_update, on_delete)`, ordered by constraint and then key position, so
    /// consecutive rows with the same name belong to one constraint. Drivers
    /// share this so their queries only differ in how they read the catalog.
    pub fn from_column_rows(rows: &[Vec<Option<String>>]) -> Vec<Self> {
        let mut constraints: Vec<Self> = Vec::new();
        for row in rows {
            let get = |i: usize| row.get(i).cloned().flatten();
            let (Some(name), Some(referenced_table), Some(column)) = (get(0), get(2), get(3))
            else {
                continue;
            };
            let referenced_column = get(4).unwrap_or_default();
            match constraints.last_mut() {
                Some(last) if last.name == name => {
                    last.columns.push(column);
                    last.referenced_columns.push(referenced_column);
                }
                _ => constraints.push(Self {
                    name,
                    columns: vec![column],
                    referenced_schema: get(1).filter(|schema| !schema.is_empty()),
                    referenced_table,
                    referenced_columns: vec![referenced_column],
                    on_update: get(5).unwrap_or_else(|| "NO ACTION".to_string()),
                    on_delete: get(6).unwrap_or_else(|| "NO ACTION".to_string()),
                }),
            }
        }
        constraints
    }
}

/// Information about a database index
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub algorithm: String,
    pub is_unique: bool,
    pub column_names: Vec<String>,
    pub condition: Option<String>,
    pub comment: Option<String>,
    /// A unique index that counts NULLs as equal (PostgreSQL 15 and later).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub nulls_not_distinct: bool,
}

/// What the structure tab shows about a table as a whole, as far as the
/// backend records it. Every field is optional because no backend keeps
/// them all.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct TableInfo {
    pub comment: Option<String>,
    /// The catalog's or planner's estimate, not a count.
    pub estimated_rows: Option<u64>,
    pub data_bytes: Option<u64>,
    pub index_bytes: Option<u64>,
    /// When the table was created, as the server prints the time.
    pub created: Option<String>,
    pub last_modified: Option<LastModified>,
    /// Backend specific settings shown as they are: engine, collation, the
    /// next auto increment value.
    pub options: Vec<(String, String)>,
    /// What a statement restating a column must repeat, where the backend
    /// changes a column only by restating it (MySQL's CHANGE COLUMN). Empty
    /// elsewhere.
    pub column_definitions: Vec<ColumnDefinition>,
}

/// A column's full type and the attributes beside it, as MySQL reports
/// them (`int(10) unsigned`, `auto_increment`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ColumnDefinition {
    pub name: String,
    pub column_type: String,
    /// Clauses after the default: `auto_increment`, `on update
    /// current_timestamp()`, `INVISIBLE`.
    pub attributes: Option<String>,
    /// A generated column, whose definition carries an expression.
    pub generated: bool,
}

/// When a table's data last changed, with what the time is worth: the
/// servers that keep it lose it on a restart.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LastModified {
    pub at: String,
    pub note: String,
}

/// Information about a database column
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub default_value: Option<String>,
    pub character_maximum_length: Option<i32>,
    pub foreign_key: Option<ForeignKeyInfo>,
    /// The comment stored on the column, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Precision and scale of exact numeric types (`numeric(10, 2)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric_precision: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric_scale: Option<i32>,
    /// The labels of the column's enum type, in their order. Empty for any
    /// other type.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enum_labels: Vec<String>,
}

impl ColumnInfo {
    /// The type with its length, or precision and scale, when the type name
    /// does not carry them already: `character varying(255)`,
    /// `numeric(10,2)`.
    pub fn display_type(&self) -> String {
        if self.data_type.contains('(') {
            return self.data_type.clone();
        }
        let exact_numeric = matches!(
            self.data_type.to_ascii_lowercase().as_str(),
            "numeric" | "decimal"
        );
        match (
            self.character_maximum_length,
            self.numeric_precision.filter(|_| exact_numeric),
            self.numeric_scale,
        ) {
            // MySQL reports 4294967295 for LONGTEXT and the like; a length
            // that large is not one anybody declared.
            (Some(length), _, _) if length > 0 && length < 1 << 30 => {
                format!("{}({length})", self.data_type)
            }
            (_, Some(precision), Some(scale)) if scale > 0 => {
                format!("{}({precision},{scale})", self.data_type)
            }
            (_, Some(precision), _) => format!("{}({precision})", self.data_type),
            _ => self.data_type.clone(),
        }
    }
}

/// Information about a table in the database schema
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TableSchemaInfo {
    pub name: String,
    pub schema: String,
    pub object_type: String,
    pub columns: Vec<ColumnInfo>,
    pub column_count: usize,
    #[serde(default)]
    pub referenced_by: Vec<InboundForeignKey>,
}

/// Result of paginated database schema query
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DatabaseSchemaResult {
    pub connection_type: String,
    pub display_name: String,
    pub tables: Vec<TableSchemaInfo>,
    pub pagination: PaginationInfo,
}

/// Pagination information for schema queries
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PaginationInfo {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub has_more: bool,
}

/// Metadata about a database table
#[derive(Debug, Clone)]
pub struct TableMetadata {
    pub name: String,
    pub schema: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub row_count: Option<i64>,
    pub primary_keys: Vec<String>,
}

impl TableMetadata {
    pub fn new(name: String, schema: Option<String>) -> Self {
        Self {
            name,
            schema,
            columns: Vec::new(),
            row_count: None,
            primary_keys: Vec::new(),
        }
    }

    pub fn get_column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns
            .iter()
            .find(|col| col.name.eq_ignore_ascii_case(name))
    }

    pub fn get_primary_key_columns(&self) -> Vec<&ColumnInfo> {
        self.columns
            .iter()
            .filter(|col| col.is_primary_key)
            .collect()
    }
}

impl QueryResult {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.rows_affected == 0
    }

    pub fn row_count(&self) -> usize {
        if !self.rows.is_empty() {
            self.rows.len()
        } else {
            self.rows_affected as usize
        }
    }
}

/// Reads rows of a name and a byte count, as size queries return them. A row
/// whose size is NULL or negative, such as a database the user may not read
/// the size of, is left out.
pub fn sizes_from_rows(rows: Vec<Vec<Option<String>>>) -> std::collections::HashMap<String, u64> {
    rows.into_iter()
        .filter_map(|row| {
            let mut cells = row.into_iter();
            let name = cells.next().flatten()?;
            let size = cells.next().flatten()?.parse::<u64>().ok()?;
            Some((name, size))
        })
        .collect()
}

/// Core trait that defines the interface for all database connections
/// This trait provides a unified interface for SQLite, PostgreSQL, and future database types
#[async_trait]
pub trait Connection: Send + Sync {
    /// The backend this connection talks to.
    fn database_type(&self) -> DatabaseType;

    /// Display name of the backend (e.g. "SQLite", "PostgreSQL").
    fn get_connection_type(&self) -> &'static str {
        self.database_type().as_str()
    }

    /// Get a human-readable display name for this connection
    fn get_display_name(&self) -> String;

    // === Connection Lifecycle ===

    /// Connect to the database using the provided connection string
    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error>;

    /// Validate that the connection is actually usable by performing a minimal
    /// round-trip to the server. Lazy backends (PostgreSQL/MySQL create their
    /// pool on first use, so `connect` alone never authenticates) surface bad
    /// credentials here. The default lists schemas, which every backend
    /// implements with a real query; drivers may override with a cheaper or
    /// faster-failing check.
    async fn ping(&self) -> Result<(), anyhow::Error> {
        self.get_schemas().await.map(|_| ())
    }

    /// The server behind this connection where servers of one database type
    /// differ in syntax (MariaDB behind a MySQL connection).
    async fn server_variant(&self) -> Result<crate::ServerVariant, anyhow::Error> {
        Ok(crate::ServerVariant::Standard)
    }

    // === Query Execution ===

    /// Execute a SQL query and return the results
    /// If parameters is provided, the query will be executed as a prepared statement
    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult, anyhow::Error>;

    /// Execute a SQL script that may contain multiple statements, returning one
    /// `QueryResult` per result-set produced by the database. Drivers that can
    /// observe statement boundaries (sqlx `fetch_many`, tiberius `into_results`)
    /// should override this to preserve them; the default falls back to
    /// `execute_query` which merges everything into a single result.
    async fn execute_script(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<Vec<QueryResult>, anyhow::Error> {
        let result = self.execute_query(query, database_name, None).await?;
        Ok(vec![result])
    }

    /// Execute a write statement (INSERT/UPDATE/DELETE/DDL) with optional nullable parameters.
    /// Unlike `execute_query`, each parameter may be `None` to bind SQL NULL.
    /// Returns the number of rows affected.
    async fn execute_write(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64, anyhow::Error>;

    /// Execute a batch of write statements as a single atomic unit.
    ///
    /// Used by the results-panel table editor to commit a set of
    /// INSERT/UPDATE/DELETE operations at once. The default implementation runs
    /// them sequentially and is **not** atomic: on a mid-batch failure the
    /// leading operations stay applied and the returned [`BatchFailure::applied`]
    /// reports how many. Backends with transactional DML (PostgreSQL, MySQL,
    /// SQLite, MSSQL) override this to wrap the batch in a real transaction so a
    /// failure rolls everything back and `applied` is `0`. Backends without
    /// transactions (Redis, ClickHouse) keep the sequential default, which
    /// honestly reports partial application rather than pretending atomicity.
    async fn execute_operations_transactional(
        &self,
        operations: &[WriteOperation],
        database_name: Option<&str>,
    ) -> Result<BatchOutcome, BatchFailure> {
        let mut outcome = BatchOutcome::default();
        for operation in operations {
            match self
                .execute_write(&operation.sql, database_name, &operation.parameters)
                .await
            {
                Ok(rows_affected) => {
                    outcome.rows_affected += rows_affected;
                    outcome.operations_executed += 1;
                }
                Err(error) => {
                    return Err(BatchFailure {
                        error,
                        applied: outcome.operations_executed,
                    });
                }
            }
        }
        Ok(outcome)
    }

    /// Column names and types a read query would produce, without executing
    /// it. Result-set drivers only learn the header from the first row, so a
    /// statement that matches nothing comes back with no columns; the editor
    /// uses this to rebuild the header so the grid can still add rows. The
    /// default returns nothing, which is right for backends whose results
    /// already carry the header regardless of row count (ClickHouse) or that
    /// have no tabular schema (Redis).
    async fn describe_query_columns(
        &self,
        _query: &str,
        _database_name: Option<&str>,
    ) -> Result<(Vec<String>, Vec<ColumnType>), anyhow::Error> {
        Ok((Vec::new(), Vec::new()))
    }

    /// Execute a query and return a true stream of rows for large datasets
    /// Returns a stream of row data (Vec<String>) that can be processed incrementally
    async fn execute_query_stream_rows(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<
        (
            Vec<String>,
            Vec<ColumnType>,
            Box<dyn Stream<Item = Result<Vec<Option<String>>, anyhow::Error>> + Send + Unpin>,
        ),
        anyhow::Error,
    > {
        // Default implementation converts regular query to stream
        let result = self.execute_query(query, database_name, None).await?;
        let rows = result.rows.into_iter().map(Ok);
        Ok((
            result.columns,
            result.column_types,
            Box::new(futures::stream::iter(rows)),
        ))
    }

    // === Schema Exploration ===

    /// Get list of databases available on this connection
    /// For SQLite, this may return just the current database name
    async fn get_databases(&self) -> Result<Vec<String>, anyhow::Error>;

    /// Get list of schemas available on this connection
    /// For SQLite, this may return a single schema like "main"
    async fn get_schemas(&self) -> Result<Vec<String>, anyhow::Error>;

    /// Get list of tables for a given schema (or all schemas if None)
    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, anyhow::Error>;

    /// Get list of views for a given schema (or all schemas if None)
    async fn get_views(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        // Default implementation returns empty (for databases that don't support views)
        Ok(Vec::new())
    }

    /// Get list of materialized views for a given schema (or all schemas if None)
    /// Materialized views are PostgreSQL-specific; other databases return empty
    async fn get_materialized_views(
        &self,
        _schema: Option<&str>,
    ) -> Result<Vec<String>, anyhow::Error> {
        // Default implementation returns empty (most databases don't support materialized views)
        Ok(Vec::new())
    }

    /// List stored procedures in a schema. Defaults to empty so drivers
    /// without stored procedures (SQLite, ClickHouse) need not override.
    async fn list_procedures(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// List user-defined functions in a schema. Defaults to empty.
    async fn list_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// List user-defined functions in a schema with their parameters, for
    /// completion and signature help. One entry per overload. Defaults to
    /// empty for drivers without a routine catalog.
    async fn list_function_signatures(
        &self,
        _schema: Option<&str>,
    ) -> Result<Vec<FunctionSignatureInfo>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// List triggers in a schema. Defaults to empty.
    async fn list_triggers(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// List the user defined types of a schema with their kind:
    /// [`ObjectKind::Enum`], [`ObjectKind::CompositeType`] or
    /// [`ObjectKind::Domain`]. Defaults to empty.
    async fn list_types(
        &self,
        _schema: Option<&str>,
    ) -> Result<Vec<(String, ObjectKind)>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// List the sequences of a schema. Defaults to empty.
    async fn list_sequences(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// Return physical sizes in bytes for tables and materialized views in a
    /// schema, keyed by object name. Names not present in the map are treated
    /// as having no reported size. Defaults to empty; drivers that can report
    /// storage size should override.
    async fn get_object_sizes(
        &self,
        _schema: Option<&str>,
    ) -> Result<std::collections::HashMap<String, u64>, anyhow::Error> {
        Ok(std::collections::HashMap::new())
    }

    /// Return the size in bytes of each database the sidebar lists at the top
    /// of a connection, keyed by name: the names `get_databases` returns when
    /// the backend `supports_schemas`, else the names `get_schemas` returns.
    /// A database whose size the user may not read is left out. Defaults to
    /// empty; drivers that can report storage size should override.
    async fn get_database_sizes(
        &self,
    ) -> Result<std::collections::HashMap<String, u64>, anyhow::Error> {
        Ok(std::collections::HashMap::new())
    }

    /// Return the comments on the tables, views and routines of a schema.
    /// Defaults to none; drivers whose database stores comments override.
    async fn get_object_comments(
        &self,
        _schema: Option<&str>,
    ) -> Result<ObjectComments, anyhow::Error> {
        Ok(ObjectComments::default())
    }

    /// Return the parent of every table in a schema that is a partition or an
    /// inheritance child of another table in the same schema, keyed by child
    /// name. Defaults to empty for backends without table inheritance.
    async fn get_table_parents(
        &self,
        _schema: Option<&str>,
    ) -> Result<std::collections::HashMap<String, TableParent>, anyhow::Error> {
        Ok(std::collections::HashMap::new())
    }

    /// Return the SQL DDL of a routine, trigger, type or sequence, for the
    /// read only DDL tab. Drivers without such objects return an error.
    async fn object_ddl(
        &self,
        _kind: ObjectKind,
        _schema: Option<&str>,
        _name: &str,
    ) -> Result<String, anyhow::Error> {
        Err(anyhow::anyhow!(
            "object_ddl is not supported by this connection type"
        ))
    }

    /// Return what the backend records about a table as a whole: comment,
    /// size, row estimate, times and storage options. Defaults to nothing
    /// known.
    async fn table_info(
        &self,
        _schema: Option<&str>,
        _table_name: &str,
    ) -> Result<TableInfo, anyhow::Error> {
        Ok(TableInfo::default())
    }

    /// The server's version as one number where features depend on it:
    /// PostgreSQL's `server_version_num` (`150004` for 15.4). `None` where
    /// nothing asks for it.
    async fn server_version_num(&self) -> Result<Option<u32>, anyhow::Error> {
        Ok(None)
    }

    /// Return the reconstructed `CREATE TABLE` statement (including indexes and
    /// constraints where available). Drivers that cannot produce it return an
    /// error; the UI hides the "Show Create Statement" button for those drivers
    /// rather than surfacing the error.
    async fn table_ddl(
        &self,
        _schema: Option<&str>,
        _table_name: &str,
    ) -> Result<String, anyhow::Error> {
        Err(anyhow::anyhow!(
            "table_ddl is not supported by this connection type"
        ))
    }

    /// Get all queryable entities (tables, views, materialized views) in a single query
    /// This is more efficient than calling get_tables, get_views, and get_materialized_views separately
    async fn get_queryable_entities(
        &self,
        _schema: Option<&str>,
    ) -> Result<Vec<QueryableEntity>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// Check if this connection type supports schemas (like PostgreSQL) or uses flat table structure (like SQLite)
    fn supports_schemas(&self) -> bool;

    /// Get column information for a specific table
    /// Returns detailed column metadata including names, types, and constraints.
    /// Defaults to empty so non-relational drivers (e.g. Redis) that have no
    /// column catalog need not implement it.
    async fn get_columns_for_table(
        &self,
        _table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// Get index information for a specific table
    /// Returns detailed index metadata including names, columns, and constraints.
    /// Defaults to empty so non-relational drivers need not implement it.
    async fn get_indexes_for_table(
        &self,
        _table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// Foreign key constraints declared on a table, one entry per constraint.
    /// Defaults to empty for backends without foreign keys.
    async fn get_foreign_keys_for_table(
        &self,
        _table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ForeignKeyConstraint>, anyhow::Error> {
        Ok(Vec::new())
    }

    /// Extract the primary table name from a SQL query
    /// Returns None if no table can be extracted (e.g., for complex queries or parsing errors)
    fn extract_table_name_from_query(
        &self,
        query: &str,
        alias: bool,
    ) -> Result<Option<String>, anyhow::Error> {
        let extractor = crate::TableExtractor::for_driver(self.database_type());
        match extractor.extract_table(query, alias) {
            Ok(table_name) => Ok(Some(table_name)),
            Err(_) => Ok(None),
        }
    }

    /// Get database schema with pagination support
    /// Returns structured schema information including tables and columns.
    /// Defaults to an empty schema so non-relational drivers need not implement
    /// it; relational drivers override with real introspection.
    async fn get_database_schema_paginated(
        &self,
        _database_name: Option<&str>,
        _table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<DatabaseSchemaResult, anyhow::Error> {
        Ok(DatabaseSchemaResult {
            connection_type: self.get_connection_type().to_string(),
            display_name: self.get_display_name(),
            tables: Vec::new(),
            pagination: PaginationInfo {
                limit,
                offset,
                has_more: false,
            },
        })
    }

    /// Perform a foreign key lookup that returns all rows for small tables
    /// or just the referenced row for large tables
    ///
    /// Parameters:
    /// - table_name: The foreign key table to query
    /// - column_name: The column containing the foreign key value
    /// - reference_value: The value to look up
    ///
    /// Returns rows from the table:
    /// - For small tables (≤20 rows): All rows, with referenced row first
    /// - For large tables: Only the referenced row
    /// - For SQLite (no estimate): Only the referenced row
    /// Defaults to an error so non-relational drivers (which have no foreign
    /// keys) need not implement it; the UI only invokes this for relational
    /// backends.
    async fn foreign_key_lookup(
        &self,
        _table_name: &str,
        _column_name: &str,
        _reference_value: &str,
    ) -> Result<QueryResult, anyhow::Error> {
        Err(anyhow::anyhow!(
            "foreign_key_lookup is not supported by this connection type"
        ))
    }

    /// List up to `limit` keys matching the glob `pattern`, filtered on the
    /// server. Defaults to `get_tables` for stores without a pattern match.
    async fn list_keys(&self, _pattern: &str, limit: usize) -> Result<KeyListing, anyhow::Error> {
        let mut keys = self.get_tables(None).await?;
        let truncated = keys.len() > limit;
        keys.truncate(limit);
        Ok(KeyListing { keys, truncated })
    }

    /// Give `key` a time to live in seconds, or with `None` remove it.
    async fn set_key_ttl(&self, _key: &str, _ttl: Option<i64>) -> Result<(), anyhow::Error> {
        Err(anyhow::anyhow!(
            "set_key_ttl is not supported by this connection type"
        ))
    }

    /// Inspect a single key and return its type, TTL and decoded value. This is
    /// the key/value counterpart to `execute_query` for key/value stores like
    /// Redis. Defaults to an error; relational drivers never implement it.
    async fn inspect_key(
        &self,
        _database_name: Option<&str>,
        _key: &str,
    ) -> Result<KeyValueResult, anyhow::Error> {
        Err(anyhow::anyhow!(
            "inspect_key is not supported by this connection type"
        ))
    }
}

/// One statement of a transactional batch, with optional bind parameters
/// (`None` binds SQL NULL, as in [`Connection::execute_write`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOperation {
    pub sql: String,
    pub parameters: Vec<Option<String>>,
}

impl WriteOperation {
    pub fn new(sql: impl Into<String>, parameters: Vec<Option<String>>) -> Self {
        Self {
            sql: sql.into(),
            parameters,
        }
    }
}

impl From<String> for WriteOperation {
    fn from(sql: String) -> Self {
        Self::new(sql, Vec::new())
    }
}

impl From<&str> for WriteOperation {
    fn from(sql: &str) -> Self {
        Self::new(sql, Vec::new())
    }
}

/// Outcome of a successful [`Connection::execute_operations_transactional`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    pub rows_affected: u64,
    pub operations_executed: usize,
}

/// Failure from [`Connection::execute_operations_transactional`].
///
/// `applied` is the number of leading operations that were actually persisted
/// before the failure: `0` for atomic backends that rolled the whole batch
/// back, and the count of committed operations for non-atomic backends (Redis,
/// ClickHouse) that cannot roll back. Callers use it to drop already-applied
/// edits from their pending-change state so a retry does not re-apply them.
#[derive(Debug)]
pub struct BatchFailure {
    pub error: anyhow::Error,
    pub applied: usize,
}

impl BatchFailure {
    /// Construct a failure where nothing was applied (the atomic/rolled-back
    /// case, and the case where the batch never started).
    pub fn atomic(error: anyhow::Error) -> Self {
        Self { error, applied: 0 }
    }
}

/// Factory trait for creating connections of different types
#[async_trait]
pub trait ConnectionFactory: Send + Sync {
    /// Create a new connection instance
    async fn create_connection(
        &self,
        connection_string: &str,
    ) -> Result<Box<dyn Connection>, anyhow::Error>;
}

#[cfg(test)]
mod tests {
    use super::{
        ColumnInfo, ForeignKeyConstraint, ForeignKeyInfo, QueryTiming, StatementTimer,
        sizes_from_rows,
    };

    #[test]
    fn sizes_from_rows_skips_rows_without_a_size() {
        let sizes = sizes_from_rows(vec![
            vec![Some("app".to_string()), Some("8192".to_string())],
            vec![Some("locked".to_string()), None],
            vec![Some("odd".to_string()), Some("-1".to_string())],
            vec![None, Some("10".to_string())],
        ]);
        assert_eq!(
            sizes,
            std::collections::HashMap::from([("app".to_string(), 8192)])
        );
    }
    use std::time::{Duration, Instant};

    #[test]
    fn statement_without_rows_reports_only_execution() {
        let started = Instant::now();
        let timer = StatementTimer::started_at(started);
        let timing = timer.finish_at(started + Duration::from_millis(5));
        assert_eq!(
            timing,
            QueryTiming {
                execution: Duration::from_millis(5),
                fetch: None,
            }
        );
    }

    #[test]
    fn first_row_splits_execution_from_fetch() {
        let started = Instant::now();
        let mut timer = StatementTimer::started_at(started);
        timer.row();
        let first_row = timer.first_row.expect("first row recorded");
        timer.row();
        assert_eq!(
            timer.first_row,
            Some(first_row),
            "later rows keep the first"
        );
        let timing = timer.finish_at(first_row + Duration::from_millis(20));
        assert_eq!(timing.execution, first_row - started);
        assert_eq!(timing.fetch, Some(Duration::from_millis(20)));
        assert_eq!(
            timing.total(),
            first_row - started + Duration::from_millis(20)
        );
    }

    #[test]
    fn sum_keeps_fetch_when_any_statement_returned_rows() {
        let write = QueryTiming {
            execution: Duration::from_millis(3),
            fetch: None,
        };
        let read = QueryTiming {
            execution: Duration::from_millis(4),
            fetch: Some(Duration::from_millis(10)),
        };
        assert_eq!(
            QueryTiming::sum([write, read]),
            Some(QueryTiming {
                execution: Duration::from_millis(7),
                fetch: Some(Duration::from_millis(10)),
            })
        );
        assert_eq!(
            QueryTiming::sum([write, write]).and_then(|sum| sum.fetch),
            None
        );
        assert_eq!(QueryTiming::sum([]), None);
    }

    #[test]
    fn foreign_key_from_parts_requires_table_and_column() {
        assert_eq!(
            ForeignKeyInfo::from_parts(Some("users"), Some("id"), Some("fk_orders".to_string())),
            Some(ForeignKeyInfo {
                foreign_table_name: "users".to_string(),
                foreign_column_name: "id".to_string(),
                constraint_name: Some("fk_orders".to_string()),
            })
        );

        assert_eq!(
            ForeignKeyInfo::from_parts(Some("users"), None::<&str>, None),
            None
        );
        assert_eq!(
            ForeignKeyInfo::from_parts(None::<&str>, Some("id"), None),
            None
        );
    }

    fn column(data_type: &str) -> ColumnInfo {
        ColumnInfo {
            name: "c".to_string(),
            data_type: data_type.to_string(),
            is_nullable: true,
            is_primary_key: false,
            default_value: None,
            character_maximum_length: None,
            foreign_key: None,
            comment: None,
            numeric_precision: None,
            numeric_scale: None,
            enum_labels: Vec::new(),
        }
    }

    #[test]
    fn display_type_adds_length_precision_and_scale() {
        let varchar = ColumnInfo {
            character_maximum_length: Some(255),
            ..column("character varying")
        };
        assert_eq!(varchar.display_type(), "character varying(255)");

        let money = ColumnInfo {
            numeric_precision: Some(10),
            numeric_scale: Some(2),
            ..column("numeric")
        };
        assert_eq!(money.display_type(), "numeric(10,2)");

        let whole = ColumnInfo {
            numeric_precision: Some(12),
            numeric_scale: Some(0),
            ..column("decimal")
        };
        assert_eq!(whole.display_type(), "decimal(12)");

        // Integers report a precision too, which nobody declared.
        let int = ColumnInfo {
            numeric_precision: Some(32),
            numeric_scale: Some(0),
            ..column("integer")
        };
        assert_eq!(int.display_type(), "integer");

        // Types that already carry their arguments are left alone, and
        // MySQL's LONGTEXT length is not shown.
        let declared = ColumnInfo {
            character_maximum_length: Some(11),
            ..column("int(11)")
        };
        assert_eq!(declared.display_type(), "int(11)");
        let longtext = ColumnInfo {
            character_maximum_length: Some(i32::MAX),
            ..column("longtext")
        };
        assert_eq!(longtext.display_type(), "longtext");
    }

    #[test]
    fn foreign_key_rows_fold_into_composite_constraints() {
        let row = |values: [Option<&str>; 7]| {
            values
                .iter()
                .map(|v| v.map(str::to_string))
                .collect::<Vec<_>>()
        };
        let rows = vec![
            row([
                Some("a_fk"),
                Some("app"),
                Some("r"),
                Some("x"),
                Some("id"),
                Some("CASCADE"),
                None,
            ]),
            row([
                Some("a_fk"),
                Some("app"),
                Some("r"),
                Some("y"),
                Some("region"),
                Some("CASCADE"),
                None,
            ]),
            row([
                Some("b_fk"),
                Some(""),
                Some("s"),
                Some("z"),
                Some("id"),
                None,
                Some("SET NULL"),
            ]),
            // Rows without a referenced table are not foreign keys.
            row([Some("c_fk"), None, None, Some("w"), None, None, None]),
        ];
        let constraints = ForeignKeyConstraint::from_column_rows(&rows);
        assert_eq!(constraints.len(), 2);
        assert_eq!(constraints[0].columns, vec!["x", "y"]);
        assert_eq!(constraints[0].referenced_columns, vec!["id", "region"]);
        assert_eq!(constraints[0].referenced_schema.as_deref(), Some("app"));
        assert_eq!(constraints[0].on_update, "CASCADE");
        assert_eq!(constraints[0].on_delete, "NO ACTION");
        assert_eq!(constraints[1].referenced_schema, None);
        assert_eq!(constraints[1].on_delete, "SET NULL");
    }
}
