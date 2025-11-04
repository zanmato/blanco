use crate::app_database::AppDatabase;
use async_std::sync::RwLock;
use gpui::{App, Global};
use std::sync::Arc;
use std::collections::HashMap;
use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory, ConnectionRegistry};
use postgres::{PostgresConnection, PgConnectionKey};
use sqlite::{SqliteConnection, SqliteConnectionKey};
use sqlx;

/// SQLite connection factory using the sqlite crate implementation
pub struct SqliteConnectionFactory;

#[async_trait]
impl ConnectionFactory for SqliteConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let key = SqliteConnectionKey::from_connection_string(connection_string)?;
        let mut conn = SqliteConnection::from_key(key);
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String> {
        Ok(connection_string.to_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "SQLite"
    }
}

/// PostgreSQL connection factory using the postgres crate implementation
pub struct PostgresConnectionFactory;

#[async_trait]
impl ConnectionFactory for PostgresConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let key = PgConnectionKey::from_connection_string(connection_string)?;
        let mut conn = PostgresConnection::from_key(key);
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String> {
        Ok(connection_string.to_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }
}

/// A unified connection manager that can handle multiple database types
/// through a common interface while maintaining type-specific functionality
#[derive(Clone)]
pub struct UnifiedConnectionManager {
    #[allow(dead_code)]
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

    /// Get or create a connection based on connection string
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

        // Double-check in case another thread created it while we were waiting for the write lock
        if let Some(existing_conn) = connections.get(&connection_key) {
            log::debug!("Found connection created by another thread: {}", connection_key);
            return Ok(Arc::clone(existing_conn));
        }

        // Determine connection type from connection string
        let connection_type = self.detect_connection_type(connection_string)?;

        // Get the appropriate factory
        let factory = self.connection_factories
            .get(&connection_type)
            .ok_or_else(|| anyhow::anyhow!("No factory found for connection type: {}", connection_type))?;

        log::info!("Creating new connection: {} (type: {})", connection_key, connection_type);

        // Create new connection
        let conn = factory.create_connection(connection_string).await?;
        let conn_arc: Arc<dyn Connection> = Arc::from(conn);

        // Store the connection
        connections.insert(connection_key.clone(), conn_arc.clone());

        log::info!("Successfully created and stored connection: {}", connection_key);
        Ok(conn_arc)
    }

    /// Get an existing connection by connection string (returns None if not found)
    pub async fn get_connection(&self, connection_string: &str) -> Option<Arc<dyn Connection>> {
        let connection_key = match self.generate_connection_key(connection_string) {
            Ok(key) => key,
            Err(_) => return None,
        };

        let connections = self.connections.read().await;
        connections.get(&connection_key).cloned()
    }

    /// Get all active connections
    pub async fn get_all_connections(&self) -> Vec<Arc<dyn Connection>> {
        let connections = self.connections.read().await;
        connections.values().cloned().collect()
    }

    /// Close and remove a connection
    #[allow(dead_code)]
    pub async fn close_connection(&self, connection_string: &str) -> Result<()> {
        let connection_key = self.generate_connection_key(connection_string)?;
        let mut connections = self.connections.write().await;

        if let Some(_conn) = connections.remove(&connection_key) {
            log::info!("Closing connection: {}", connection_key);
            // Note: We can't disconnect here as we need a mutable reference
            // The connection will be dropped when Arc goes out of scope
        }

        Ok(())
    }

    /// Close all connections
    #[allow(dead_code)]
    pub async fn close_all_connections(&self) -> Result<()> {
        let mut connections = self.connections.write().await;
        let count = connections.len();
        connections.clear();
        log::info!("Closed {} connections", count);
        Ok(())
    }

    /// Generate a unique connection key from a connection string
    pub fn generate_connection_key(&self, connection_string: &str) -> Result<String> {
        // Simple implementation - just use the connection string as the key
        // In a real implementation, you might normalize it (e.g., remove password)
        Ok(connection_string.to_string())
    }

    /// Detect connection type from connection string
    fn detect_connection_type(&self, connection_string: &str) -> Result<String> {
        let connection_lower = connection_string.to_lowercase();

        // Log the connection string and detected type for debugging
        log::debug!("Detecting connection type for: '{}'", connection_string);

        if connection_lower.starts_with("postgres://") ||
           connection_lower.starts_with("postgresql://") {
            log::debug!("Detected PostgreSQL connection type");
            Ok("PostgreSQL".to_string())
        } else if connection_lower.starts_with("sqlite:") ||
                  connection_lower.contains(".db") ||
                  connection_lower == ":memory:" ||
                  connection_lower == "sqlite::memory:" {
            log::debug!("Detected SQLite connection type");
            Ok("SQLite".to_string())
        } else {
            // Default to SQLite for unknown types
            log::warn!("Unknown connection type for '{}', defaulting to SQLite", connection_string);
            Ok("SQLite".to_string())
        }
    }

    /// Test a connection string without storing the connection
    #[allow(dead_code)]
    pub async fn test_connection(&self, connection_string: &str) -> Result<bool> {
        let connection_type = self.detect_connection_type(connection_string)?;

        let factory = self.connection_factories
            .get(&connection_type)
            .ok_or_else(|| anyhow::anyhow!("No factory found for connection type: {}", connection_type))?;

        let conn = factory.create_connection(connection_string).await?;
        conn.test_connection().await
    }
}

/// Global database service that holds app database and unified connection manager
#[derive(Clone)]
pub struct DbService {
    pub app_db: Arc<RwLock<Option<AppDatabase>>>,
    pub unified_manager: Arc<RwLock<UnifiedConnectionManager>>,
}

impl Global for DbService {}

impl DbService {
    pub fn new() -> Self {
        Self {
            app_db: Arc::new(RwLock::new(None)),
            unified_manager: Arc::new(RwLock::new(UnifiedConnectionManager::new())),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Get a clone of the app database lock
    pub fn app_db_handle(&self) -> Arc<RwLock<Option<AppDatabase>>> {
        self.app_db.clone()
    }

    /// Get a clone of the unified connection manager
    #[allow(dead_code)]
    pub fn unified_manager_handle(&self) -> Arc<RwLock<UnifiedConnectionManager>> {
        self.unified_manager.clone()
    }

    /// Get access to the unified connection manager
    pub async fn unified_manager(&self) -> std::sync::Arc<RwLock<UnifiedConnectionManager>> {
        self.unified_manager.clone()
    }

    /// Convenience method to get or create a connection
    pub async fn get_or_create_unified_connection(
        &self,
        connection_string: &str,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        let unified_manager = self.unified_manager().await;
        let result = unified_manager
            .read()
            .await
            .get_or_create_connection(connection_string)
            .await;
        result
    }

    /// Convenience method to execute a query
    pub async fn execute_query_unified(
        &self,
        connection_string: &str,
        sql: &str,
    ) -> Result<blanco_core::QueryResult, anyhow::Error> {
        let unified_manager = self.unified_manager().await;
        let manager_read = unified_manager.read().await;

        // Try to get the connection
        match manager_read.get_connection(connection_string).await {
            Some(connection) => connection.execute_query(sql).await,
            None => {
                // Log detailed debug information about available connections
                log::error!("=== CONNECTION NOT FOUND DEBUG INFO ===");
                log::error!("Looking for connection: {}", connection_string);

                // Try to generate the key to see what format is expected
                if let Ok(expected_key) = manager_read.generate_connection_key(connection_string) {
                    log::error!("Expected connection key format: {}", expected_key);
                }

                // Log all available connections in the manager
                let connections = manager_read.get_all_connections().await;
                log::error!("Available connections in unified manager:");
                if connections.is_empty() {
                    log::error!("  No connections available");
                } else {
                    for (i, conn) in connections.iter().enumerate() {
                        let conn_key = conn.get_connection_key_str();
                        log::error!(
                            "  {}. Key: '{}', Type: {}",
                            i + 1,
                            conn_key,
                            conn.get_connection_type()
                        );
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!(
                    "Connection not found: {}",
                    connection_string
                ))
            }
        }
    }

    /// Convenience method to execute a prepared query
    #[allow(dead_code)]
    pub async fn execute_prepared_query_unified(
        &self,
        connection_string: &str,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<blanco_core::QueryResult, anyhow::Error> {
        let unified_manager = self.unified_manager().await;
        let manager_read = unified_manager.read().await;

        // Try to get the connection
        match manager_read.get_connection(connection_string).await {
            Some(connection) => {
                connection
                    .execute_prepared_query(sql_template, parameters)
                    .await
            }
            None => {
                // Log detailed debug information about available connections
                log::error!("=== CONNECTION NOT FOUND DEBUG INFO (Prepared Query) ===");
                log::error!("Looking for connection: {}", connection_string);

                // Try to generate the key to see what format is expected
                if let Ok(expected_key) = manager_read.generate_connection_key(connection_string) {
                    log::error!("Expected connection key format: {}", expected_key);
                }

                // Log all available connections in the manager
                let connections = manager_read.get_all_connections().await;
                log::error!("Available connections in unified manager:");
                if connections.is_empty() {
                    log::error!("  No connections available");
                } else {
                    for (i, conn) in connections.iter().enumerate() {
                        let conn_key = conn.get_connection_key_str();
                        log::error!(
                            "  {}. Key: '{}', Type: {}",
                            i + 1,
                            conn_key,
                            conn.get_connection_type()
                        );
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!(
                    "Connection not found: {}",
                    connection_string
                ))
            }
        }
    }

    /// Convenience method to get schemas
    #[allow(dead_code)]
    pub async fn get_schemas_unified(
        &self,
        connection_string: &str,
    ) -> Result<Vec<String>, anyhow::Error> {
        let unified_manager = self.unified_manager().await;
        let manager_read = unified_manager.read().await;

        // Try to get the connection
        match manager_read.get_connection(connection_string).await {
            Some(connection) => connection.get_schemas().await,
            None => {
                // Log detailed debug information about available connections
                log::error!("=== CONNECTION NOT FOUND DEBUG INFO (Get Schemas) ===");
                log::error!("Looking for connection: {}", connection_string);

                // Try to generate the key to see what format is expected
                if let Ok(expected_key) = manager_read.generate_connection_key(connection_string) {
                    log::error!("Expected connection key format: {}", expected_key);
                }

                // Log all available connections in the manager
                let connections = manager_read.get_all_connections().await;
                log::error!("Available connections in unified manager:");
                if connections.is_empty() {
                    log::error!("  No connections available");
                } else {
                    for (i, conn) in connections.iter().enumerate() {
                        let conn_key = conn.get_connection_key_str();
                        log::error!(
                            "  {}. Key: '{}', Type: {}",
                            i + 1,
                            conn_key,
                            conn.get_connection_type()
                        );
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!(
                    "Connection not found: {}",
                    connection_string
                ))
            }
        }
    }

    /// Convenience method to get tables
    pub async fn get_tables_unified(
        &self,
        connection_string: &str,
        schema: Option<&str>,
    ) -> Result<Vec<String>, anyhow::Error> {
        let unified_manager = self.unified_manager().await;
        let manager_read = unified_manager.read().await;

        // Try to get the connection
        match manager_read.get_connection(connection_string).await {
            Some(connection) => connection.get_tables(schema).await,
            None => {
                // Log detailed debug information about available connections
                log::error!("=== CONNECTION NOT FOUND DEBUG INFO (Get Tables) ===");
                log::error!("Looking for connection: {}", connection_string);

                // Try to generate the key to see what format is expected
                if let Ok(expected_key) = manager_read.generate_connection_key(connection_string) {
                    log::error!("Expected connection key format: {}", expected_key);
                }

                // Log all available connections in the manager
                let connections = manager_read.get_all_connections().await;
                log::error!("Available connections in unified manager:");
                if connections.is_empty() {
                    log::error!("  No connections available");
                } else {
                    for (i, conn) in connections.iter().enumerate() {
                        let conn_key = conn.get_connection_key_str();
                        log::error!(
                            "  {}. Key: '{}', Type: {}",
                            i + 1,
                            conn_key,
                            conn.get_connection_type()
                        );
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!(
                    "Connection not found: {}",
                    connection_string
                ))
            }
        }
    }

    /// Convenience method to get tables for a specific schema
    #[allow(dead_code)]
    pub async fn get_schema_tables_unified(
        &self,
        connection_string: &str,
        schema_name: &str,
    ) -> Result<Vec<String>, anyhow::Error> {
        self.get_tables_unified(connection_string, Some(schema_name))
            .await
    }

    /// Get or create a connection by connection_id from the app database
    pub async fn get_or_create_connection_by_id(
        &self,
        connection_id: i64,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        // Get the app database
        let app_db_lock = self.app_db.read().await;
        let app_db = app_db_lock.as_ref()
            .ok_or_else(|| anyhow::anyhow!("App database not initialized"))?;

        // Query the connections table to get the connection string
        let connection_string: Option<String> = sqlx::query_scalar(
            "SELECT connection_string FROM connections WHERE id = ? AND is_active = 1"
        )
        .bind(connection_id)
        .fetch_one(app_db.pool())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to query connection {}: {}", connection_id, e))?;

        let connection_string = connection_string
            .ok_or_else(|| anyhow::anyhow!("Connection with id {} not found or inactive", connection_id))?;

        log::debug!("Found connection string for id {}: {}", connection_id, connection_string);

        // Use the existing get_or_create_unified_connection method
        self.get_or_create_unified_connection(&connection_string).await
    }
}

// Implement the DatabaseService trait for DbService
#[async_trait::async_trait]
impl blanco_core::DatabaseService for DbService {
    async fn get_or_create_connection(
        &self,
        connection_string: &str,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        self.get_or_create_unified_connection(connection_string)
            .await
    }
}
