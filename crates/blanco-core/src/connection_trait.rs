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

/// Database driver types supported by the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriverType {
    SQLite,
    PostgreSQL,
    MySQL,
    ClickHouse,
}

impl DriverType {
    /// Convert from string representation to DriverType
    pub fn from_string(s: &str) -> Option<Self> {
        match s {
            "SQLite" => Some(Self::SQLite),
            "PostgreSQL" => Some(Self::PostgreSQL),
            "MySQL" => Some(Self::MySQL),
            "ClickHouse" => Some(Self::ClickHouse),
            _ => None,
        }
    }

    /// Convert to string representation
    pub fn to_string(&self) -> &'static str {
        match self {
            Self::SQLite => "SQLite",
            Self::PostgreSQL => "PostgreSQL",
            Self::MySQL => "MySQL",
            Self::ClickHouse => "ClickHouse",
        }
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

/// Information about a foreign key relationship
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ForeignKeyInfo {
    pub foreign_table_name: String,
    pub foreign_column_name: String,
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
    /// Get the connection type identifier (e.g., "SQLite", "PostgreSQL")
    fn get_connection_type(&self) -> &'static str;

    /// Get a human-readable display name for this connection
    fn get_display_name(&self) -> String;

    // === Connection Lifecycle ===

    /// Connect to the database using the provided connection string
    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error>;

    // === Query Execution ===

    /// Execute a SQL query and return the results
    /// If parameters is provided, the query will be executed as a prepared statement
    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult, anyhow::Error>;

    /// Execute a write statement (INSERT/UPDATE/DELETE/DDL) with optional nullable parameters.
    /// Unlike `execute_query`, each parameter may be `None` to bind SQL NULL.
    /// Returns the number of rows affected. Backends that do not override this fall back
    /// to `execute_query` after coercing `None` values to empty strings, which is unsafe
    /// for NULL handling; implementations should override.
    async fn execute_write(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64, anyhow::Error> {
        let coerced: Vec<String> = parameters
            .iter()
            .map(|v| v.clone().unwrap_or_default())
            .collect();
        let params = if coerced.is_empty() {
            None
        } else {
            Some(coerced.as_slice())
        };
        let result = self.execute_query(query, database_name, params).await?;
        Ok(result.rows_affected)
    }

    /// Execute a query and return a stream of rows for large datasets
    /// Returns a tuple of (columns, column_types, row_stream)
    async fn execute_query_stream(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<
        (
            Vec<String>,
            Vec<ColumnType>,
            Box<dyn std::marker::Send + std::marker::Sync>,
        ),
        anyhow::Error,
    > {
        // Default implementation uses regular query
        let result = self.execute_query(query, database_name, None).await?;
        Ok((result.columns, result.column_types, Box::new(result.rows)))
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
    #[allow(dead_code)]
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
    /// Returns detailed column metadata including names, types, and constraints
    async fn get_columns_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, anyhow::Error>;

    /// Get index information for a specific table
    /// Returns detailed index metadata including names, columns, and constraints
    async fn get_indexes_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>, anyhow::Error>;

    /// Extract the primary table name from a SQL query
    /// Returns None if no table can be extracted (e.g., for complex queries or parsing errors)
    fn extract_table_name_from_query(
        &self,
        query: &str,
        alias: bool,
    ) -> Result<Option<String>, anyhow::Error> {
        let driver =
            DriverType::from_string(self.get_connection_type()).unwrap_or(DriverType::PostgreSQL);
        let extractor = crate::TableExtractor::for_driver(driver);
        match extractor.extract_table(query, alias) {
            Ok(table_name) => Ok(Some(table_name)),
            Err(_) => Ok(None),
        }
    }

    /// Get database schema with pagination support
    /// Returns structured schema information including tables and columns
    async fn get_database_schema_paginated(
        &self,
        database_name: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<DatabaseSchemaResult, anyhow::Error>;

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
    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult, anyhow::Error>;
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
