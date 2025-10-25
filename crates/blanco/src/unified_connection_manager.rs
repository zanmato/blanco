use crate::connection_trait::{Connection, ConnectionFactory, ConnectionRegistry, ConnectionInfo};
use crate::postgres_connection::{PostgresConnectionFactory};
use crate::sqlite_connection::{SqliteConnectionFactory};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A unified connection manager that can handle multiple database types
/// through a common interface while maintaining type-specific functionality
#[derive(Clone)]
pub struct UnifiedConnectionManager {
    registry: Arc<ConnectionRegistry>,
    connections: Arc<RwLock<HashMap<String, Arc<dyn Connection>>>>,
    connection_factories: Arc<HashMap<String, Arc<dyn ConnectionFactory>>>,
}

impl UnifiedConnectionManager {
    /// Create a new unified connection manager with default connection factories
    pub fn new() -> Self {
        let mut registry = ConnectionRegistry::new();
        registry.register_factory("SQLite".to_string(), Box::new(SqliteConnectionFactory));
        registry.register_factory("PostgreSQL".to_string(), Box::new(PostgresConnectionFactory));

        let mut factories: HashMap<String, Arc<dyn ConnectionFactory>> = HashMap::new();
        factories.insert("SQLite".to_string(), Arc::new(SqliteConnectionFactory));
        factories.insert("PostgreSQL".to_string(), Arc::new(PostgresConnectionFactory));

        Self {
            registry: Arc::new(registry),
            connections: Arc::new(RwLock::new(HashMap::new())),
            connection_factories: Arc::new(factories),
        }
    }

    /// Get or create a connection for the given connection string
    /// This method will parse the connection string, determine the database type,
    /// and either return an existing connection or create a new one
    pub async fn get_or_create_connection(&self, connection_string: &str) -> Result<Arc<dyn Connection>, anyhow::Error> {
        let connection_key = self.generate_connection_key(connection_string)?;

        {
            let connections = self.connections.read().await;
            if let Some(existing_conn) = connections.get(&connection_key) {
                // Check if the connection is still healthy
                if existing_conn.test_connection().await.unwrap_or(false) {
                    log::debug!("Using existing healthy connection: {}", connection_key);
                    return Ok(Arc::clone(existing_conn));
                } else {
                    log::info!("Existing connection is unhealthy, will recreate: {}", connection_key);
                    // Drop the read lock before we try to get a write lock
                    drop(connections);
                }
            }
        }

        // Connection doesn't exist or is unhealthy, create a new one
        let mut connections = self.connections.write().await;

        // Double-check in case another thread created the connection while we waited for the write lock
        if let Some(existing_conn) = connections.get(&connection_key) {
            if existing_conn.test_connection().await.unwrap_or(false) {
                log::debug!("Using existing healthy connection (double-check): {}", connection_key);
                return Ok(Arc::clone(existing_conn));
            }
        }

        // Create new connection
        let connection_type = self.detect_connection_type(connection_string)?;
        let factory = self.connection_factories.get(&connection_type)
            .ok_or_else(|| anyhow::anyhow!("No factory found for connection type: {}", connection_type))?;

        log::info!("Creating new {} connection: {}", connection_type, connection_key);
        let new_connection = factory.create_connection(connection_string).await?;
        let arc_connection: Arc<dyn Connection> = Arc::from(new_connection);

        // Store the connection
        connections.insert(connection_key.clone(), arc_connection.clone());

        log::info!("Successfully created and stored connection: {}", connection_key);
        Ok(arc_connection)
    }

    /// Get a connection by its key
    pub async fn get_connection_by_key(&self, connection_key: &str) -> Option<Arc<dyn Connection>> {
        let connections = self.connections.read().await;
        connections.get(connection_key).cloned()
    }

    /// Get a connection by its connection string
    pub async fn get_connection_by_string(&self, connection_string: &str) -> Option<Arc<dyn Connection>> {
        let connections = self.connections.read().await;

        // Find the connection that matches the given connection string
        for (key, connection) in connections.iter() {
            if key == connection_string {
                return Some(connection.clone());
            }
        }

        None
    }

    /// Get a specific connection by connection string
    pub async fn get_connection(&self, connection_string: &str) -> Option<Arc<dyn Connection>> {
        let connections = self.connections.read().await;
        connections.get(connection_string).cloned()
    }

    /// Get all active connections
    pub async fn get_all_connections(&self) -> Vec<Arc<dyn Connection>> {
        let connections = self.connections.read().await;
        connections.values().cloned().collect()
    }

    /// Get connection information for all active connections
    pub async fn get_all_connection_info(&self) -> Result<Vec<ConnectionInfo>, anyhow::Error> {
        let connections = self.connections.read().await;
        let mut infos = Vec::new();

        for connection in connections.values() {
            match connection.get_connection_info().await {
                Ok(info) => infos.push(info),
                Err(e) => {
                    log::warn!("Failed to get connection info for {}: {}", connection.get_display_name(), e);
                }
            }
        }

        Ok(infos)
    }

    /// Remove a connection by its key
    pub async fn remove_connection(&self, connection_key: &str) -> Result<Option<Arc<dyn Connection>>, anyhow::Error> {
        let mut connections = self.connections.write().await;
        if let Some(connection) = connections.remove(connection_key) {
            log::info!("Removed connection: {}", connection_key);
            Ok(Some(connection))
        } else {
            Ok(None)
        }
    }

    /// Validate and clean up unhealthy connections
    pub async fn validate_and_cleanup_connections(&self) -> Result<usize, anyhow::Error> {
        let mut connections = self.connections.write().await;
        let _initial_count = connections.len();
        let mut removed_count = 0;

        // Collect unhealthy keys first to avoid borrowing issues
        let mut unhealthy_keys = Vec::new();
        for (key, connection) in connections.iter() {
            match connection.test_connection().await {
                Ok(is_healthy) => {
                    if !is_healthy {
                        log::warn!("Found unhealthy connection: {}", key);
                        unhealthy_keys.push(key.clone());
                    }
                }
                Err(e) => {
                    log::warn!("Failed to test connection {}: {}", key, e);
                    unhealthy_keys.push(key.clone());
                }
            }
        }

        // Remove unhealthy connections
        for key in unhealthy_keys {
            connections.remove(&key);
            removed_count += 1;
        }

        if removed_count > 0 {
            log::info!("Cleaned up {} unhealthy connections", removed_count);
        }

        Ok(removed_count)
    }

    /// Get connection statistics
    pub async fn get_connection_stats(&self) -> ConnectionStats {
        let connections = self.connections.read().await;
        let total = connections.len();
        let mut healthy = 0;
        let mut type_counts: HashMap<String, usize> = HashMap::new();

        for connection in connections.values() {
            // Check health
            match connection.test_connection().await {
                Ok(is_healthy) => {
                    if is_healthy {
                        healthy += 1;
                    }
                }
                Err(_) => {}
            }

            // Count by type
            let conn_type = connection.get_connection_type();
            *type_counts.entry(conn_type.to_string()).or_insert(0) += 1;
        }

        ConnectionStats {
            total,
            healthy,
            unhealthy: total - healthy,
            type_counts,
        }
    }

    /// Detect connection type from connection string
    fn detect_connection_type(&self, connection_string: &str) -> Result<String, anyhow::Error> {
        let connection_string_lower = connection_string.to_lowercase();

        if connection_string_lower.starts_with("sqlite://")
            || connection_string_lower.starts_with("sqlite:")
            || connection_string_lower.ends_with(".sqlite")
            || connection_string_lower.ends_with(".db")
            || connection_string_lower.ends_with(".db3") {
            Ok("SQLite".to_string())
        } else if connection_string_lower.starts_with("postgres://")
            || connection_string_lower.starts_with("postgresql://") {
            Ok("PostgreSQL".to_string())
        } else {
            Err(anyhow::anyhow!("Unable to detect connection type from connection string: {}", connection_string))
        }
    }

    /// Generate a unique key for a connection string
    pub fn generate_connection_key(&self, connection_string: &str) -> Result<String, anyhow::Error> {
        // Normalize the connection string to generate a consistent key
        let normalized = connection_string.trim().to_lowercase();

        // For different connection types, we might need different key generation strategies
        if normalized.starts_with("sqlite://") || normalized.starts_with("sqlite:") {
            // For SQLite, the file path is the key
            Ok(format!("sqlite:{}", connection_string))
        } else if normalized.starts_with("postgres://") || normalized.starts_with("postgresql://") {
            // For PostgreSQL, the full connection string is the key
            Ok(format!("postgres:{}", connection_string))
        } else {
            // For other types, use the full string as the key
            Ok(connection_string.to_string())
        }
    }

    /// Get supported connection types
    pub fn get_supported_types(&self) -> Vec<&str> {
        self.registry.get_supported_types()
    }

    /// Create a connection of a specific type
    pub async fn create_connection_of_type(
        &self,
        connection_type: &str,
        connection_string: &str
    ) -> Result<Arc<dyn Connection>, anyhow::Error> {
        let factory = self.connection_factories.get(connection_type)
            .ok_or_else(|| anyhow::anyhow!("No factory found for connection type: {}", connection_type))?;

        let connection = factory.create_connection(connection_string).await?;
        Ok(Arc::from(connection))
    }
}

impl Default for UnifiedConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about connections in the manager
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub total: usize,
    pub healthy: usize,
    pub unhealthy: usize,
    pub type_counts: HashMap<String, usize>,
}

/// A trait for objects that need to use connections from the unified manager
#[async_trait]
pub trait ConnectionUser {
    /// Use a connection from the unified manager
    async fn use_connection<F, R>(&self, connection_string: &str, operation: F) -> Result<R, anyhow::Error>
    where
        F: FnOnce(&dyn Connection) -> Result<R, anyhow::Error> + Send + 'static;

    /// Get a reference to the unified connection manager
    fn get_connection_manager(&self) -> &UnifiedConnectionManager;
}

/// Extension trait to make working with unified connections easier
pub trait ConnectionExt {
    /// Execute a query with automatic connection management
    async fn execute_query_with_auto_connect(
        &self,
        connection_string: &str,
        query: &str,
    ) -> Result<crate::connection_trait::QueryResult, anyhow::Error>;

    /// Execute a prepared query with automatic connection management
    async fn execute_prepared_query_with_auto_connect(
        &self,
        connection_string: &str,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<crate::connection_trait::QueryResult, anyhow::Error>;

    /// Get schemas for a connection with automatic connection management
    async fn get_schemas_with_auto_connect(
        &self,
        connection_string: &str,
    ) -> Result<Vec<String>, anyhow::Error>;

    /// Get tables for a connection with automatic connection management
    async fn get_tables_with_auto_connect(
        &self,
        connection_string: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, anyhow::Error>;
}

impl ConnectionExt for UnifiedConnectionManager {
    async fn execute_query_with_auto_connect(
        &self,
        connection_string: &str,
        query: &str,
    ) -> Result<crate::connection_trait::QueryResult, anyhow::Error> {
        let connection = self.get_or_create_connection(connection_string).await?;
        connection.execute_query(query).await
    }

    async fn execute_prepared_query_with_auto_connect(
        &self,
        connection_string: &str,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<crate::connection_trait::QueryResult, anyhow::Error> {
        let connection = self.get_or_create_connection(connection_string).await?;
        connection.execute_prepared_query(sql_template, parameters).await
    }

    async fn get_schemas_with_auto_connect(
        &self,
        connection_string: &str,
    ) -> Result<Vec<String>, anyhow::Error> {
        let connection = self.get_or_create_connection(connection_string).await?;
        connection.get_schemas().await
    }

    async fn get_tables_with_auto_connect(
        &self,
        connection_string: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, anyhow::Error> {
        let connection = self.get_or_create_connection(connection_string).await?;
        connection.get_tables(schema).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_unified_connection_manager_creation() {
        let manager = UnifiedConnectionManager::new();
        let supported_types = manager.get_supported_types();

        assert!(supported_types.contains(&"SQLite"));
        assert!(supported_types.contains(&"PostgreSQL"));
    }

    #[tokio::test]
    async fn test_connection_type_detection() {
        let manager = UnifiedConnectionManager::new();

        // SQLite detection
        assert_eq!(manager.detect_connection_type("sqlite:///tmp/test.db").unwrap(), "SQLite");
        assert_eq!(manager.detect_connection_type("sqlite:/tmp/test.db").unwrap(), "SQLite");
        assert_eq!(manager.detect_connection_type("/tmp/test.sqlite").unwrap(), "SQLite");
        assert_eq!(manager.detect_connection_type("/tmp/test.db").unwrap(), "SQLite");

        // PostgreSQL detection
        assert_eq!(manager.detect_connection_type("postgres://user:pass@localhost:5432/db").unwrap(), "PostgreSQL");
        assert_eq!(manager.detect_connection_type("postgresql://user@localhost:5432/db").unwrap(), "PostgreSQL");
    }

    #[tokio::test]
    async fn test_connection_key_generation() {
        let manager = UnifiedConnectionManager::new();

        // SQLite keys
        assert_eq!(
            manager.generate_connection_key("sqlite:///tmp/test.db").unwrap(),
            "sqlite:sqlite:///tmp/test.db"
        );
        assert_eq!(
            manager.generate_connection_key("sqlite:/tmp/test.db").unwrap(),
            "sqlite:sqlite:/tmp/test.db"
        );

        // PostgreSQL keys
        assert_eq!(
            manager.generate_connection_key("postgres://user@localhost:5432/db").unwrap(),
            "postgres:postgres://user@localhost:5432/db"
        );
    }

    #[tokio::test]
    async fn test_sqlite_connection_creation() {
        let manager = UnifiedConnectionManager::new();

        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        // Create connection
        let connection = manager.get_or_create_connection(&connection_string).await.unwrap();

        assert!(connection.is_connected());
        assert_eq!(connection.get_connection_type(), "SQLite");
        assert!(connection.get_display_name().contains("SQLite"));

        // Test query execution
        let result = connection.execute_query("SELECT 1 as test").await.unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], "1");

        // Test that the same connection is reused
        let connection2 = manager.get_or_create_connection(&connection_string).await.unwrap();
        assert!(Arc::ptr_eq(&connection, &connection2));
    }

    #[tokio::test]
    async fn test_connection_stats() {
        let manager = UnifiedConnectionManager::new();

        // Initially no connections
        let stats = manager.get_connection_stats().await;
        assert_eq!(stats.total, 0);
        assert_eq!(stats.healthy, 0);
        assert_eq!(stats.unhealthy, 0);

        // Create a SQLite connection
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        let _connection = manager.get_or_create_connection(&connection_string).await.unwrap();

        // Should have one connection now
        let stats = manager.get_connection_stats().await;
        assert_eq!(stats.total, 1);
        assert_eq!(stats.healthy, 1);
        assert_eq!(stats.unhealthy, 0);
        assert_eq!(stats.type_counts.get("SQLite"), Some(&1));
    }

    #[tokio::test]
    async fn test_connection_cleanup() {
        let manager = UnifiedConnectionManager::new();

        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        // Create connection
        let connection = manager.get_or_create_connection(&connection_string).await.unwrap();
        assert!(connection.is_connected());

        // Manually disconnect to simulate unhealthy connection
        // Note: This is a bit hacky since we can't easily make a connection unhealthy
        // In a real scenario, connections might become unhealthy due to network issues, etc.

        // Test cleanup (should remove 0 connections since they're still healthy)
        let removed_count = manager.validate_and_cleanup_connections().await.unwrap();
        assert_eq!(removed_count, 0);
    }

    #[tokio::test]
    async fn test_connection_extension_trait() {
        let manager = UnifiedConnectionManager::new();

        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        // Test auto-connect query execution
        let result = manager.execute_query_with_auto_connect(&connection_string, "SELECT 1").await.unwrap();
        assert_eq!(result.rows.len(), 1);

        // Test auto-connect prepared query
        let prepared_result = manager.execute_prepared_query_with_auto_connect(
            &connection_string,
            "SELECT ? as value",
            &vec!["test".to_string()]
        ).await.unwrap();
        assert_eq!(prepared_result.rows.len(), 1);
        assert_eq!(prepared_result.rows[0][0], "test");

        // Test auto-connect schema operations
        let schemas = manager.get_schemas_with_auto_connect(&connection_string).await.unwrap();
        assert!(!schemas.is_empty());

        let tables = manager.get_tables_with_auto_connect(&connection_string, None).await.unwrap();
        // Should be empty initially, but the operation should succeed
    }
}