use crate::app_database::AppDatabase;
use crate::unified_connection_manager::UnifiedConnectionManager;
use gpui::{App, Global};
use sqlx::postgres::PgConnectOptions;
use std::str::FromStr;
use std::sync::Arc;
use async_std::sync::RwLock;

/// Connection key for PostgreSQL connections
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PgConnectionKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
}

impl PgConnectionKey {
    pub fn from_connection_string(conn_str: &str) -> Result<Self, anyhow::Error> {
        // Parse connection string using SQLX's built-in DSN parser
        // This handles all PostgreSQL connection string formats including query parameters
        log::info!(
            "🔍 PgConnectionKey parsing connection string using SQLX: {}",
            conn_str
        );

        let options = PgConnectOptions::from_str(conn_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse PostgreSQL connection string: {}", e))?;

        // Extract connection details from SQLX options
        let host = options.get_host().to_string();
        let port = options.get_port();
        let database = options
            .get_database()
            .map(|db| db.to_string())
            .unwrap_or_default();
        let username = options.get_username().to_string();

        // SQLX doesn't expose password directly, so we need to parse it from the original string
        // We'll use URL parsing as a fallback just for the password
        let password = if let Ok(url) = url::Url::parse(conn_str) {
            url.password().map(|p| p.to_string())
        } else {
            None
        };

        Ok(PgConnectionKey {
            host,
            port,
            database,
            username,
            password,
        })
    }

    #[allow(dead_code)]
    pub fn to_connection_string(&self) -> String {
        let _url_str = format!(
            "postgresql://{}:{}@{}:{}/{}",
            self.username,
            self.password.as_deref().unwrap_or(""),
            self.host,
            self.port,
            self.database
        );

        // Add query parameters if needed
        _url_str
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
    pub async fn get_or_create_unified_connection(&self, connection_string: &str) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        let unified_manager = self.unified_manager().await;
        let result = unified_manager.read().await.get_or_create_connection(connection_string).await;
        result
    }

    /// Convenience method to execute a query
    pub async fn execute_query_unified(&self, connection_string: &str, sql: &str) -> Result<blanco_core::QueryResult, anyhow::Error> {
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
                        log::error!("  {}. Key: '{}', Type: {}", i + 1, conn_key, conn.get_connection_type());
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!("Connection not found: {}", connection_string))
            }
        }
    }

    /// Convenience method to execute a prepared query
    #[allow(dead_code)]
    pub async fn execute_prepared_query_unified(&self, connection_string: &str, sql_template: &str, parameters: &[String]) -> Result<blanco_core::QueryResult, anyhow::Error> {
        let unified_manager = self.unified_manager().await;
        let manager_read = unified_manager.read().await;

        // Try to get the connection
        match manager_read.get_connection(connection_string).await {
            Some(connection) => connection.execute_prepared_query(sql_template, parameters).await,
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
                        log::error!("  {}. Key: '{}', Type: {}", i + 1, conn_key, conn.get_connection_type());
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!("Connection not found: {}", connection_string))
            }
        }
    }

    /// Convenience method to get schemas
    #[allow(dead_code)]
    pub async fn get_schemas_unified(&self, connection_string: &str) -> Result<Vec<String>, anyhow::Error> {
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
                        log::error!("  {}. Key: '{}', Type: {}", i + 1, conn_key, conn.get_connection_type());
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!("Connection not found: {}", connection_string))
            }
        }
    }

    /// Convenience method to get tables
    pub async fn get_tables_unified(&self, connection_string: &str, schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
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
                        log::error!("  {}. Key: '{}', Type: {}", i + 1, conn_key, conn.get_connection_type());
                    }
                }

                log::error!("=== END DEBUG INFO ===");

                Err(anyhow::anyhow!("Connection not found: {}", connection_string))
            }
        }
    }

    /// Convenience method to get tables for a specific schema
    #[allow(dead_code)]
    pub async fn get_schema_tables_unified(&self, connection_string: &str, schema_name: &str) -> Result<Vec<String>, anyhow::Error> {
        self.get_tables_unified(connection_string, Some(schema_name)).await
    }
}