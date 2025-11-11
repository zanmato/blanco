use crate::app_database::AppDatabase;
use anyhow::Result;
use async_std::sync::RwLock;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory, ConnectionRegistry};
use gpui::{App, Global};
use postgres::{connection::PostgresSshConfig, PgConnectionKey, PostgresConnection};
use sqlite::{SqliteConnection, SqliteConnectionKey};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

/// Replace the database name in a PostgreSQL connection string
fn replace_database_in_postgres_connection_string(
    connection_string: &str,
    new_database: &str,
) -> String {
    if let Ok(mut url) = Url::parse(connection_string) {
        // Set the new database path
        url.set_path(&format!("/{}", new_database));
        url.to_string()
    } else {
        // If URL parsing fails, try a simple string replacement for postgresql:// URLs
        if connection_string.starts_with("postgresql://") {
            // Find the database part (last segment after the last /)
            if let Some(last_slash_pos) = connection_string.rfind('/') {
                let before_db = &connection_string[..last_slash_pos];
                format!("{}/{}", before_db, new_database)
            } else {
                // No slash found, append the database
                format!("{}/{}", connection_string, new_database)
            }
        } else {
            // Not a PostgreSQL URL, return as-is
            connection_string.to_string()
        }
    }
}

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
pub struct PostgresConnectionFactory {
    // Store SSH configuration for connections
    ssh_configs: Arc<async_std::sync::Mutex<std::collections::HashMap<i64, PostgresSshConfig>>>,
}

impl PostgresConnectionFactory {
    pub fn new() -> Self {
        Self {
            ssh_configs: Arc::new(async_std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Set SSH configuration for a connection ID
    pub async fn set_ssh_config(&self, connection_id: i64, ssh_config: PostgresSshConfig) {
        let mut configs = self.ssh_configs.lock().await;
        configs.insert(connection_id, ssh_config);
    }

    /// Get SSH configuration for a connection ID
    pub async fn get_ssh_config(&self, connection_id: i64) -> Option<PostgresSshConfig> {
        let configs = self.ssh_configs.lock().await;
        configs.get(&connection_id).cloned()
    }

    /// Create PostgreSQL connection with optional SSH configuration
    async fn create_postgres_connection(
        &self,
        connection_string: &str,
        ssh_config: Option<PostgresSshConfig>,
    ) -> Result<Box<dyn Connection>> {
        let key = PgConnectionKey::from_connection_string(connection_string)?;

        let mut conn = if let Some(ssh_cfg) = ssh_config {
            PostgresConnection::from_key_with_ssh(key, ssh_cfg)
        } else {
            PostgresConnection::from_key(key)
        };

        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}

impl Default for PostgresConnectionFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionFactory for PostgresConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        self.create_postgres_connection(connection_string, None)
            .await
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
    postgres_factory: Arc<PostgresConnectionFactory>,
}

impl UnifiedConnectionManager {
    /// Create a new unified connection manager with default connection factories
    pub fn new() -> Self {
        let postgres_factory = PostgresConnectionFactory::new();
        let postgres_factory_arc: Arc<PostgresConnectionFactory> = Arc::new(postgres_factory);

        let mut registry = ConnectionRegistry::new();
        registry.register_factory("SQLite".to_string(), Box::new(SqliteConnectionFactory));
        registry.register_factory(
            "PostgreSQL".to_string(),
            Box::new(SqliteConnectionFactory), // Use a simple factory for the registry
        );

        let mut factories: HashMap<String, Arc<dyn ConnectionFactory>> = HashMap::new();
        factories.insert("SQLite".to_string(), Arc::new(SqliteConnectionFactory));
        factories.insert("PostgreSQL".to_string(), postgres_factory_arc.clone());

        Self {
            registry: Arc::new(registry),
            connections: Arc::new(RwLock::new(HashMap::new())),
            connection_factories: Arc::new(factories),
            postgres_factory: postgres_factory_arc,
        }
    }

    /// Get or create a connection based on connection string
    pub async fn get_or_create_connection(
        &self,
        connection_string: &str,
    ) -> Result<Arc<dyn Connection>, anyhow::Error> {
        let connection_key = self.generate_connection_key(connection_string)?;

        {
            let connections = self.connections.read().await;
            if let Some(existing_conn) = connections.get(&connection_key) {
                // Check if the connection is still healthy
                if existing_conn.test_connection().await.unwrap_or(false) {
                    log::debug!("Using existing healthy connection: {}", connection_key);
                    return Ok(Arc::clone(existing_conn));
                } else {
                    log::info!(
                        "Existing connection is unhealthy, will recreate: {}",
                        connection_key
                    );
                    // Drop the read lock before we try to get a write lock
                    drop(connections);
                }
            }
        }

        // Connection doesn't exist or is unhealthy, create a new one
        let mut connections = self.connections.write().await;

        // Double-check in case another thread created it while we were waiting for the write lock
        if let Some(existing_conn) = connections.get(&connection_key) {
            log::debug!(
                "Found connection created by another thread: {}",
                connection_key
            );
            return Ok(Arc::clone(existing_conn));
        }

        // Determine connection type from connection string
        let connection_type = self.detect_connection_type(connection_string)?;

        // Get the appropriate factory
        let factory = self
            .connection_factories
            .get(&connection_type)
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", connection_type)
            })?;

        log::info!(
            "Creating new connection: {} (type: {})",
            connection_key,
            connection_type
        );

        // Create new connection
        let conn = factory.create_connection(connection_string).await?;
        let conn_arc: Arc<dyn Connection> = Arc::from(conn);

        // Store the connection
        connections.insert(connection_key.clone(), conn_arc.clone());

        log::info!(
            "Successfully created and stored connection: {}",
            connection_key
        );
        Ok(conn_arc)
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

        if connection_lower.starts_with("postgres://")
            || connection_lower.starts_with("postgresql://")
        {
            log::debug!("Detected PostgreSQL connection type");
            Ok("PostgreSQL".to_string())
        } else if connection_lower.starts_with("sqlite:")
            || connection_lower.contains(".db")
            || connection_lower == ":memory:"
            || connection_lower == "sqlite::memory:"
        {
            log::debug!("Detected SQLite connection type");
            Ok("SQLite".to_string())
        } else {
            // Default to SQLite for unknown types
            log::warn!(
                "Unknown connection type for '{}', defaulting to SQLite",
                connection_string
            );
            Ok("SQLite".to_string())
        }
    }

    /// Test a connection string without storing the connection
    #[allow(dead_code)]
    pub async fn test_connection(&self, connection_string: &str) -> Result<bool> {
        let connection_type = self.detect_connection_type(connection_string)?;

        let factory = self
            .connection_factories
            .get(&connection_type)
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", connection_type)
            })?;

        let conn = factory.create_connection(connection_string).await?;
        conn.test_connection().await
    }

    /// Get or create a connection with SSH support for a PostgreSQL connection
    pub async fn get_or_create_postgres_connection_with_ssh(
        &self,
        connection_string: &str,
        ssh_config: Option<PostgresSshConfig>,
    ) -> Result<Arc<dyn Connection>, anyhow::Error> {
        let connection_key = self.generate_connection_key(connection_string)?;

        // Check for existing connection
        {
            let connections = self.connections.read().await;
            if let Some(existing_conn) = connections.get(&connection_key) {
                if existing_conn.test_connection().await.unwrap_or(false) {
                    log::debug!("Using existing healthy SSH connection: {}", connection_key);
                    return Ok(Arc::clone(existing_conn));
                }
            }
        }

        // Create new connection with SSH support
        let mut connections = self.connections.write().await;
        let conn = self
            .postgres_factory
            .create_postgres_connection(connection_string, ssh_config)
            .await?;
        let conn_arc: Arc<dyn Connection> = Arc::from(conn);

        // Store the connection
        connections.insert(connection_key.clone(), conn_arc.clone());
        log::info!(
            "Created new PostgreSQL connection with SSH: {}",
            connection_key
        );

        Ok(conn_arc)
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

    /// Primary method to get or create a connection by ID
    pub async fn get_or_create_connection(
        &self,
        connection_id: i64,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        self.get_or_create_connection_with_database(connection_id, None)
            .await
    }

    /// Get or create a connection with optional database override
    pub async fn get_or_create_connection_with_database(
        &self,
        connection_id: i64,
        database_name: Option<&str>,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        // Get the app database
        let app_db_lock = self.app_db.read().await;
        let app_db = app_db_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("App database not initialized"))?;

        // Query the connections table to get connection data including SSH configuration
        let connection_row = sqlx::query(
            r#"
            SELECT
                CASE
                    WHEN db_type = 'SQLite' THEN CONCAT('sqlite:',database_path)
                    ELSE connection_string
                END as connection_string,
                db_type,
                ssh_host, ssh_port, ssh_user, ssh_password,
                ssh_private_key_path, ssh_private_key_password
            FROM connections
            WHERE id = ? AND is_active = 1
            "#,
        )
        .bind(connection_id)
        .fetch_one(app_db.pool())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to query connection {}: {}", connection_id, e))?;

        let mut connection_string: String = connection_row.try_get("connection_string")?;
        let db_type: String = connection_row.try_get("db_type")?;

        // Apply database override for PostgreSQL connections
        if db_type == "PostgreSQL" && database_name.is_some() {
            connection_string = replace_database_in_postgres_connection_string(
                &connection_string,
                database_name.unwrap(),
            );
            log::debug!(
                "Applied database override '{}' to connection string: {}",
                database_name.unwrap(),
                connection_string
            );
        }

        log::debug!(
            "Found connection data for id {}: type={}, string={}",
            connection_id,
            db_type,
            connection_string
        );

        // Check if this is a PostgreSQL connection with SSH configuration
        if db_type == "PostgreSQL" {
            let ssh_host: Option<String> = connection_row.try_get("ssh_host").ok();

            if let Some(host) = ssh_host {
                // SSH configuration exists
                let ssh_user: Option<String> = connection_row.try_get("ssh_user").ok();
                if let Some(user) = ssh_user {
                    // Validate that SSH host and user are not empty
                    if !host.trim().is_empty() && !user.trim().is_empty() {
                        let ssh_port: i32 = connection_row.try_get("ssh_port").unwrap_or(22);
                        let ssh_password: Option<String> =
                            connection_row.try_get("ssh_password").ok();
                        let ssh_private_key_path: Option<String> =
                            connection_row.try_get("ssh_private_key_path").ok();
                        let ssh_private_key_password: Option<String> =
                            connection_row.try_get("ssh_private_key_password").ok();

                        let ssh_config = PostgresSshConfig {
                            ssh_host: host.trim().to_string(),
                            ssh_port: ssh_port as u16,
                            ssh_user: user.trim().to_string(),
                            ssh_password,
                            ssh_private_key_path,
                            ssh_private_key_password,
                        };

                        log::info!("Creating PostgreSQL connection with SSH tunnel to {}:{} for connection ID {}", ssh_config.ssh_host, ssh_config.ssh_port, connection_id);
                        let unified_manager = self.unified_manager().await;
                        return unified_manager
                            .read()
                            .await
                            .get_or_create_postgres_connection_with_ssh(
                                &connection_string,
                                Some(ssh_config),
                            )
                            .await;
                    } else {
                        log::warn!("SSH configuration has empty host or user for connection ID {}, ignoring SSH tunnel", connection_id);
                    }
                }
            }
        }

        // No SSH configuration or not PostgreSQL, use standard connection
        self.get_or_create_unified_connection_internal(&connection_string)
            .await
    }

    /// Internal method to get or create a connection by connection string
    async fn get_or_create_unified_connection_internal(
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

    /// Execute a query by connection ID
    #[allow(dead_code)]
    pub async fn execute_query_by_id(
        &self,
        connection_id: i64,
        sql: &str,
    ) -> Result<blanco_core::QueryResult, anyhow::Error> {
        // Get the connection by ID
        let connection = self.get_or_create_connection(connection_id).await?;

        // Execute the query
        connection.execute_query(sql, None).await
    }
}

// Implement the DatabaseService trait for DbService
#[async_trait::async_trait]
impl blanco_core::DatabaseService for DbService {
    async fn get_or_create_connection(
        &self,
        connection_string: &str,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        // For the trait, keep the old interface but internally delegate to connection_id lookup
        self.get_or_create_unified_connection_internal(connection_string)
            .await
    }

    async fn get_or_create_connection_by_id(
        &self,
        connection_id: i64,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        // Use our primary method for connection_id lookup
        self.get_or_create_connection(connection_id).await
    }
}
