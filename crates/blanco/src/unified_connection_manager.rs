use blanco_core::{Connection, ConnectionFactory, ConnectionRegistry};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use async_std::sync::RwLock;
use postgres::{PostgresConnection, PgConnectionKey};
use sqlite::{SqliteConnection, SqliteConnectionKey};




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