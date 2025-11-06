use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt;

/// Icon types for database connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconName {
    Database,
    DatabaseConnected,
    Sqlite,
    Postgres,
    Table,
    Column,
    Key,
    Folder,
    FolderOpen,
    File,
    Settings,
    Refresh,
    Play,
    Stop,
    Plus,
    Minus,
    Search,
    Filter,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Check,
    X,
    Alert,
    Info,
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
    /// Primary key column detected for the table (if applicable)
    pub primary_key_column: Option<String>,
    /// Connection ID used for this query (for subsequent operations)
    pub connection_id: Option<i64>,
}

/// Information about a database column
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub default_value: Option<String>,
    pub character_maximum_length: Option<i32>,
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
pub trait Connection: Send + Sync + fmt::Debug {
    /// Get the unique connection key for this connection
    fn get_connection_key_str(&self) -> String;

    /// Get the connection type identifier (e.g., "SQLite", "PostgreSQL")
    fn get_connection_type(&self) -> &'static str;

    /// Get the icon to display for this connection type in the UI
    fn get_icon_name(&self) -> IconName;

    /// Get a human-readable display name for this connection
    fn get_display_name(&self) -> String;

    /// Get the underlying manager for advanced operations
    #[allow(dead_code)]
    fn get_manager_any(&self) -> &dyn std::any::Any;

    // === Connection Lifecycle ===

    /// Connect to the database using the provided connection string
    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error>;

    /// Disconnect from the database and clean up resources
    #[allow(dead_code)]
    async fn disconnect(&mut self);

    /// Check if the connection is currently active and healthy
    fn is_connected(&self) -> bool;

    /// Ensure the connection is established, reconnecting if necessary
    #[allow(dead_code)]
    async fn ensure_connected(&mut self, connection_string: &str) -> Result<(), anyhow::Error>;

    // === Query Execution ===

    /// Execute a SQL query and return the results
    /// If database_name is provided, the query will be executed in the context of that database
    async fn execute_query(&self, query: &str, database_name: Option<&str>) -> Result<QueryResult, anyhow::Error>;

    /// Execute a parameterized query with prepared statements
    async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult, anyhow::Error>;

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

    /// Get comprehensive table metadata including columns and statistics
    /// This method lazy-loads all necessary information for auto-completion and hover
    async fn get_table_metadata(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<TableMetadata, anyhow::Error>;

    /// Extract the primary table name from a SQL query
    /// Returns None if no table can be extracted (e.g., for complex queries or parsing errors)
    fn extract_table_name_from_query(&self, query: &str) -> Result<Option<String>, anyhow::Error>;

    /// Execute table change operations in a database-agnostic way
    /// Takes a list of change operations and executes them with proper SQL generation
    async fn execute_table_changes(
        &self,
        changes: &[TableChangeOperation],
    ) -> Result<QueryResult, anyhow::Error>;

    // === Connection Management ===

    /// Test if the connection is alive with a simple ping query
    async fn test_connection(&self) -> Result<bool, anyhow::Error> {
        if !self.is_connected() {
            return Ok(false);
        }

        // Try a simple query that should work on most databases
        match self.execute_query("SELECT 1", None).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Get connection statistics and metadata
    async fn get_connection_info(&self) -> Result<ConnectionInfo, anyhow::Error> {
        Ok(ConnectionInfo {
            connection_type: self.get_connection_type().to_string(),
            display_name: self.get_display_name(),
            is_connected: self.is_connected(),
            database_name: self.get_database_name().await?,
            schema_count: self.get_schemas().await.map(|s| s.len()).unwrap_or(0),
        })
    }

    /// Get the current database name
    async fn get_database_name(&self) -> Result<Option<String>, anyhow::Error> {
        // Default implementation - can be overridden by specific implementations
        match self
            .execute_query("SELECT CURRENT_DATABASE() as db_name", None)
            .await
        {
            Ok(result) if !result.rows.is_empty() => Ok(Some(result.rows[0][0].clone())),
            _ => Ok(None),
        }
    }

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
    #[allow(dead_code)]
    pub database_name: Option<String>,
    pub schema_count: usize,
}

/// Connection metadata optimized for UI display
#[derive(Clone)]
#[allow(dead_code)]
pub struct ConnectionUIMetadata {
    pub display_name: String,
    pub file_safe_name: String,
    pub supports_schemas: bool,
    pub icon_name: IconName,
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
