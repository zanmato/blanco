use crate::icon::IconName;
use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt;

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
    async fn execute_query(&self, query: &str) -> Result<QueryResult, anyhow::Error>;

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
    async fn get_primary_key_for_table(&self, table_name: &str) -> Result<Option<String>, anyhow::Error>;

    /// Execute table change operations in a database-agnostic way
    /// Takes a list of change operations and executes them with proper SQL generation
    async fn execute_table_changes(&self, changes: &[crate::table_operations::TableChangeOperation]) -> Result<QueryResult, anyhow::Error>;

    // === Connection Management ===

    /// Test if the connection is alive with a simple ping query
    async fn test_connection(&self) -> Result<bool, anyhow::Error> {
        if !self.is_connected() {
            return Ok(false);
        }

        // Try a simple query that should work on most databases
        match self.execute_query("SELECT 1").await {
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
        match self.execute_query("SELECT CURRENT_DATABASE() as db_name").await {
            Ok(result) if !result.rows.is_empty() => Ok(Some(result.rows[0][0].clone())),
            _ => Ok(None),
        }
    }

    // === UI Integration Methods ===

    /// Check if this connection type supports Language Server Protocol
    #[allow(dead_code)]
    fn supports_lsp(&self) -> bool;

    /// Get LSP configuration for this connection (if supported)
    #[allow(dead_code)]
    fn get_lsp_config(&self) -> Option<LspConfig>;

    /// Get a consistent file-safe name for this connection
    #[allow(dead_code)]
    fn get_file_safe_name(&self) -> String;

    /// Get connection metadata optimized for UI display
    #[allow(dead_code)]
    fn get_ui_metadata(&self) -> ConnectionUIMetadata;
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

/// LSP configuration for connections that support Language Server Protocol
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LspConfig {
    pub server_name: String,
    pub connection_args: Vec<String>,
    pub workspace_path: Option<String>,
}

/// Connection metadata optimized for UI display
#[derive(Clone)]
#[allow(dead_code)]
pub struct ConnectionUIMetadata {
    pub display_name: String,
    pub file_safe_name: String,
    pub supports_schemas: bool,
    pub supports_lsp: bool,
    pub icon_name: crate::icon::IconName,
}

impl std::fmt::Debug for ConnectionUIMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionUIMetadata")
            .field("display_name", &self.display_name)
            .field("file_safe_name", &self.file_safe_name)
            .field("supports_schemas", &self.supports_schemas)
            .field("supports_lsp", &self.supports_lsp)
            .field("icon_name", &"<icon>")
            .finish()
    }
}

/// Factory trait for creating connections of different types
#[async_trait]
pub trait ConnectionFactory: Send + Sync {
    /// Create a new connection instance
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>, anyhow::Error>;

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

    pub fn register_factory(&mut self, connection_type: String, factory: Box<dyn ConnectionFactory>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon::IconName;

    // Mock connection for testing
    #[derive(Debug)]
    struct MockConnection {
        connected: bool,
        display_name: String,
    }

    #[async_trait]
    impl Connection for MockConnection {
        fn get_connection_key_str(&self) -> String {
            "mock_key".to_string()
        }

        fn get_connection_type(&self) -> &'static str {
            "Mock"
        }

        fn get_icon_name(&self) -> IconName {
            IconName::Database
        }

        fn get_display_name(&self) -> String {
            self.display_name.clone()
        }

        fn get_manager_any(&self) -> &dyn std::any::Any {
            &()
        }

        async fn connect(&mut self, _connection_string: &str) -> Result<(), anyhow::Error> {
            self.connected = true;
            Ok(())
        }

        async fn disconnect(&mut self) {
            self.connected = false;
        }

        fn is_connected(&self) -> bool {
            self.connected
        }

        async fn ensure_connected(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
            if !self.is_connected() {
                self.connect(connection_string).await?;
            }
            Ok(())
        }

        async fn execute_query(&self, _query: &str) -> Result<QueryResult, anyhow::Error> {
            Ok(QueryResult::default())
        }

        async fn execute_prepared_query(
            &self,
            _sql_template: &str,
            _parameters: &[String],
        ) -> Result<QueryResult, anyhow::Error> {
            Ok(QueryResult::default())
        }

        async fn get_databases(&self) -> Result<Vec<String>, anyhow::Error> {
            Ok(vec!["test_db".to_string()])
        }

        async fn get_schemas(&self) -> Result<Vec<String>, anyhow::Error> {
            Ok(vec!["public".to_string()])
        }

        async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
            Ok(vec!["test_table".to_string()])
        }
    }

    #[tokio::test]
    async fn test_connection_lifecycle() {
        let mut conn = MockConnection {
            connected: false,
            display_name: "Test Connection".to_string(),
        };

        assert!(!conn.is_connected());

        conn.connect("mock://connection").await.unwrap();
        assert!(conn.is_connected());

        conn.disconnect().await;
        assert!(!conn.is_connected());
    }

    #[tokio::test]
    async fn test_connection_info() {
        let conn = MockConnection {
            connected: true,
            display_name: "Test Connection".to_string(),
        };

        let info = conn.get_connection_info().await.unwrap();
        assert_eq!(info.connection_type, "Mock");
        assert_eq!(info.display_name, "Test Connection");
        assert!(info.is_connected);
    }
}