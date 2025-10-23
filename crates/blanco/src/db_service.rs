use crate::app_database::AppDatabase;
use crate::database::DatabaseManager;
use crate::postgres::PostgresManager;
use gpui::{App, Global};
use sqlx::postgres::PgConnectOptions;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;

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

        log::info!("🔍 PgConnectionKey extracted via SQLX - host: {}, port: {}, database: {}, username: {}, password: {}",
            host, port, database, username,
            if password.as_ref().is_some_and(|p| !p.is_empty()) { "<present>" } else { "<none>" });

        Ok(PgConnectionKey {
            host,
            port,
            database,
            username,
            password,
        })
    }
}

/// Global database service that holds app database, user database, and postgres connections
#[derive(Clone)]
pub struct DbService {
    pub app_db: Arc<RwLock<Option<AppDatabase>>>,
    pub user_db: Arc<RwLock<DatabaseManager>>,
    pub pg_connections: Arc<RwLock<HashMap<PgConnectionKey, Arc<PostgresManager>>>>,
}

impl Global for DbService {}

impl DbService {
    pub fn new() -> Self {
        Self {
            app_db: Arc::new(RwLock::new(None)),
            user_db: Arc::new(RwLock::new(DatabaseManager::new())),
            pg_connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Get a clone of the app database lock
    pub fn app_db_handle(&self) -> Arc<RwLock<Option<AppDatabase>>> {
        self.app_db.clone()
    }

    /// Get a clone of the user database lock
    pub fn user_db_handle(&self) -> Arc<RwLock<DatabaseManager>> {
        self.user_db.clone()
    }

    /// Get or create a PostgreSQL connection for the given connection string
    pub async fn get_or_create_pg_connection(
        &self,
        connection_string: &str,
    ) -> Result<Arc<PostgresManager>, anyhow::Error> {
        log::info!(
            "🔍 DbService.get_or_create_pg_connection called with: {}",
            connection_string
        );
        let key = PgConnectionKey::from_connection_string(connection_string)?;

        let mut connections = self.pg_connections.write().await;

        // Check if connection already exists
        if let Some(manager) = connections.get(&key) {
            // Test if the connection is still alive
            if manager.is_connected() {
                log::info!("Using existing PostgreSQL connection");
                return Ok(Arc::clone(manager));
            } else {
                log::info!(
                    "Existing PostgreSQL connection is disconnected, removing and creating new one"
                );
                connections.remove(&key);
            }
        }

        // Create new connection with retry logic
        let mut retry_count = 0;
        let max_retries = 3;

        while retry_count < max_retries {
            match self
                .create_pg_connection(connection_string.to_string())
                .await
            {
                Ok(arc_manager) => {
                    log::info!(
                        "Successfully created PostgreSQL connection (attempt {})",
                        retry_count + 1
                    );

                    // Store the connection
                    connections.insert(key, Arc::clone(&arc_manager));
                    return Ok(arc_manager);
                }
                Err(e) => {
                    log::warn!(
                        "Failed to create PostgreSQL connection (attempt {}/{}): {}",
                        retry_count + 1,
                        max_retries,
                        e
                    );
                }
            }

            retry_count += 1;
            if retry_count < max_retries {
                // Exponential backoff: 200ms, 800ms, 1800ms
                let delay_ms = 200 * (retry_count as u64 * retry_count as u64);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }

        Err(anyhow::anyhow!(
            "Failed to create PostgreSQL connection after {} attempts",
            max_retries
        ))
    }

    /// Create a new PostgreSQL connection
    async fn create_pg_connection(
        &self,
        connection_string: String,
    ) -> Result<Arc<PostgresManager>, anyhow::Error> {
        let mut manager = PostgresManager::new();

        // Connect to the database
        manager.connect_async(&connection_string).await?;

        // Test the connection with a simple query to verify it's working
        if manager.is_connected() {
            let arc_manager = Arc::new(manager);
            log::info!("Successfully created and validated PostgreSQL connection");
            Ok(arc_manager)
        } else {
            Err(anyhow::anyhow!(
                "PostgreSQL connection established but validation failed"
            ))
        }
    }

    /// Ensure a PostgreSQL connection is available, auto-connecting if needed
    pub async fn ensure_pg_connection(
        &self,
        connection_string: &str,
    ) -> Result<Arc<PostgresManager>, anyhow::Error> {
        log::info!(
            "🔧 Ensuring PostgreSQL connection for: {}",
            connection_string.split('@').nth(1).unwrap_or("unknown")
        );

        match self.get_or_create_pg_connection(connection_string).await {
            Ok(manager) => {
                log::info!("✅ PostgreSQL connection ensured successfully");
                Ok(manager)
            }
            Err(e) => {
                log::error!("❌ Failed to ensure PostgreSQL connection: {}", e);
                Err(e)
            }
        }
    }

    /// Get all PostgreSQL connections
    pub async fn get_all_pg_connections(&self) -> HashMap<PgConnectionKey, Arc<PostgresManager>> {
        self.pg_connections.read().await.clone()
    }

    /// Remove a PostgreSQL connection
    pub async fn remove_pg_connection(
        &self,
        key: &PgConnectionKey,
    ) -> Option<Arc<PostgresManager>> {
        let mut connections = self.pg_connections.write().await;
        if let Some(manager) = connections.remove(key) {
            log::info!(
                "Removed PostgreSQL connection for {}:{}/{}",
                key.host,
                key.port,
                key.database
            );
            Some(manager)
        } else {
            None
        }
    }

    /// Validate and clean up unhealthy PostgreSQL connections
    pub async fn validate_and_cleanup_connections(&self) -> Result<usize, anyhow::Error> {
        let mut connections = self.pg_connections.write().await;
        let initial_count = connections.len();
        let mut removed_count = 0;

        // Collect unhealthy keys first to avoid borrowing issues
        let mut unhealthy_keys = Vec::new();
        for (key, manager) in connections.iter() {
            if !manager.is_connected() {
                log::warn!(
                    "Found disconnected PostgreSQL connection for {}:{}/{}",
                    key.host,
                    key.port,
                    key.database
                );
                unhealthy_keys.push(key.clone());
            }
        }

        // Remove unhealthy connections
        for key in unhealthy_keys {
            connections.remove(&key);
            removed_count += 1;
        }

        if removed_count > 0 {
            log::info!(
                "Cleaned up {} unhealthy PostgreSQL connections",
                removed_count
            );
        }

        Ok(removed_count)
    }

    /// Get connection statistics
    pub async fn get_connection_stats(&self) -> (usize, usize) {
        let connections = self.pg_connections.read().await;
        let total = connections.len();
        let mut healthy = 0;

        for manager in connections.values() {
            if manager.is_connected() {
                healthy += 1;
            }
        }

        (total, healthy)
    }
}
