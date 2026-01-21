use async_trait::async_trait;
use futures::Stream;
use std::collections::HashMap;

/// Database driver types supported by the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriverType {
    SQLite,
    PostgreSQL,
    MySQL,
}

impl DriverType {
    /// Convert from string representation to DriverType
    pub fn from_string(s: &str) -> Option<Self> {
        match s {
            "SQLite" => Some(Self::SQLite),
            "PostgreSQL" => Some(Self::PostgreSQL),
            "MySQL" => Some(Self::MySQL),
            _ => None,
        }
    }

    /// Convert to string representation
    pub fn to_string(&self) -> &'static str {
        match self {
            Self::SQLite => "SQLite",
            Self::PostgreSQL => "PostgreSQL",
            Self::MySQL => "MySQL",
        }
    }
}
/// Result of a database query
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<String>>,
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
    #[allow(dead_code)]
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
    /// Get the unique connection key for this connection
    fn get_connection_key_str(&self) -> String;

    /// Get the connection type identifier (e.g., "SQLite", "PostgreSQL")
    fn get_connection_type(&self) -> &'static str;

    /// Get a human-readable display name for this connection
    fn get_display_name(&self) -> String;

    // === Connection Lifecycle ===

    /// Connect to the database using the provided connection string
    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error>;

    /// Disconnect from the database and clean up resources
    #[allow(dead_code)]
    async fn disconnect(&mut self);

    /// Check if the connection is currently active and healthy
    fn is_connected(&self) -> bool;

    // === Query Execution ===

    /// Execute a SQL query and return the results
    /// If parameters is provided, the query will be executed as a prepared statement
    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult, anyhow::Error>;

    /// Execute a query and return a stream of rows for large datasets
    /// Returns a tuple of (columns, column_types, row_stream)
    /// Default implementation uses regular query - should be overridden for large datasets
    async fn execute_query_stream(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<
        (
            Vec<String>,
            Vec<String>,
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
            Vec<String>,
            Box<dyn Stream<Item = Result<Vec<String>, anyhow::Error>> + Send + Unpin>,
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

    /// Check if this connection type supports schemas (like PostgreSQL) or uses flat table structure (like SQLite)
    fn supports_schemas(&self) -> bool;

    /// Get the primary key column for a specific table
    /// Returns the column name if a primary key exists, None if no primary key
    #[allow(dead_code)]
    async fn get_primary_key_for_table(
        &self,
        table_name: &str,
    ) -> Result<Option<String>, anyhow::Error>;

    /// Get column information for a specific table
    /// Returns detailed column metadata including names, types, and constraints
    async fn get_columns_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, anyhow::Error>;

    /// Extract the primary table name from a SQL query
    /// Returns None if no table can be extracted (e.g., for complex queries or parsing errors)
    fn extract_table_name_from_query(
        &self,
        query: &str,
        alias: bool,
    ) -> Result<Option<String>, anyhow::Error>;

    /// Resolve a table alias to its actual table name using the original SQL query
    /// Returns the actual table name if the alias is found, None otherwise
    fn resolve_table_alias(
        &self,
        query: &str,
        alias: &str,
    ) -> Result<Option<String>, anyhow::Error>;

    /// Get connection statistics and metadata
    async fn get_connection_info(&self) -> Result<ConnectionInfo, anyhow::Error> {
        Ok(ConnectionInfo {
            connection_type: self.get_connection_type().to_string(),
            display_name: self.get_display_name(),
            is_connected: self.is_connected(),
            schema_count: self.get_schemas().await.map(|s| s.len()).unwrap_or(0),
        })
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

    // === UI Integration Methods ===

    /// Get a consistent file-safe name for this connection
    #[allow(dead_code)]
    fn get_file_safe_name(&self) -> String;

    /// Get connection metadata optimized for UI display
    #[allow(dead_code)]
    fn get_ui_metadata(&self) -> ConnectionUIMetadata;
}

/// Table change operations for schema modifications
#[derive(Debug, Clone)]
pub enum TableChangeOperation {
    AddColumn {
        name: String,
        data_type: String,
        nullable: bool,
    },
    DropColumn {
        name: String,
    },
    RenameColumn {
        old_name: String,
        new_name: String,
    },
    ModifyColumn {
        name: String,
        new_type: String,
    },
}

/// Information about a database connection
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub connection_type: String,
    #[allow(dead_code)]
    pub display_name: String,
    pub is_connected: bool,
    pub schema_count: usize,
}

/// Connection metadata optimized for UI display
#[derive(Clone)]
#[allow(dead_code)]
pub struct ConnectionUIMetadata {
    pub display_name: String,
    pub file_safe_name: String,
    pub supports_schemas: bool,
}

impl std::fmt::Debug for ConnectionUIMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionUIMetadata")
            .field("display_name", &self.display_name)
            .field("file_safe_name", &self.file_safe_name)
            .field("supports_schemas", &self.supports_schemas)
            .field("icon_name", &"<icon>")
            .finish()
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

    /// Parse a connection string and return the connection key
    #[allow(dead_code)]
    fn parse_connection_string(&self, connection_string: &str) -> Result<String, anyhow::Error>;

    /// Get the connection type this factory creates
    #[allow(dead_code)]
    fn get_connection_type(&self) -> &'static str;
}

/// Registry for managing different connection types
pub struct ConnectionRegistry {
    factories: HashMap<String, Box<dyn ConnectionFactory>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register_factory(
        &mut self,
        connection_type: String,
        factory: Box<dyn ConnectionFactory>,
    ) {
        self.factories.insert(connection_type, factory);
    }

    #[allow(dead_code)]
    pub fn get_factory(&self, connection_type: &str) -> Option<&dyn ConnectionFactory> {
        self.factories.get(connection_type).map(|f| f.as_ref())
    }

    #[allow(dead_code)]
    pub fn get_supported_types(&self) -> Vec<&str> {
        self.factories.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
