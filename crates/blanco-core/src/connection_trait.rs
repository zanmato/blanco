use crate::DatabaseType;
use async_trait::async_trait;
use futures::Stream;

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

/// Routine and trigger objects in a schema. Used by `list_procedures`,
/// `list_functions`, and `list_triggers` on the `Connection` trait so the
/// sidebar can render them and `object_ddl` can fetch their source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutineKind {
    Procedure,
    Function,
    Trigger,
}

impl RoutineKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            RoutineKind::Procedure => "Procedure",
            RoutineKind::Function => "Function",
            RoutineKind::Trigger => "Trigger",
        }
    }
}

/// A queryable database entity with its type
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueryableEntity {
    pub name: String,
    pub entity_type: EntityType,
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
    /// Unknown/fallback type
    Unknown,
}

impl ColumnType {
    /// Returns true if this column type is numeric (Integer, UnsignedInteger, or Numeric)
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Integer | Self::UnsignedInteger | Self::Numeric)
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
    pub execution_time_ms: Option<i64>,
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

/// Information about a database index
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub algorithm: String,
    pub is_unique: bool,
    pub column_names: Vec<String>,
    pub condition: Option<String>,
    pub comment: Option<String>,
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

    /// List triggers in a schema. Defaults to empty.
    async fn list_triggers(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
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

    /// Return the SQL DDL source for a routine/trigger object. Used by the
    /// "Show DDL" tab. Drivers without source-stored objects return an error.
    async fn object_ddl(
        &self,
        _kind: RoutineKind,
        _schema: Option<&str>,
        _name: &str,
    ) -> Result<String, anyhow::Error> {
        Err(anyhow::anyhow!(
            "object_ddl is not supported by this connection type"
        ))
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
    use super::ForeignKeyInfo;

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
}
