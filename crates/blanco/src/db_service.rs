use crate::app_database::AppDatabase;
use crate::database::DatabaseManager;
use crate::postgres::PostgresManager;
use gpui::{App, Global};
use sqlx::postgres::PgConnectOptions;
use std::collections::HashMap;
use std::sync::Arc;
use std::str::FromStr;
use tokio::sync::RwLock;
use url;

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
        log::info!("🔍 PgConnectionKey parsing connection string using SQLX: {}", conn_str);

        let options = PgConnectOptions::from_str(conn_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse PostgreSQL connection string: {}", e))?;

        // Extract connection details from SQLX options
        let host = options.get_host().to_string();
        let port = options.get_port();
        let database = options.get_database().map(|db| db.to_string()).unwrap_or_default();
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
            if password.as_ref().map_or(false, |p| !p.is_empty()) { "<present>" } else { "<none>" });

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
    pub async fn get_or_create_pg_connection(&self, connection_string: &str) -> Result<Arc<PostgresManager>, anyhow::Error> {
        log::info!("🔍 DbService.get_or_create_pg_connection called with: {}", connection_string);
        let key = PgConnectionKey::from_connection_string(connection_string)?;
        
        let mut connections = self.pg_connections.write().await;
        
        // Check if connection already exists
        if let Some(manager) = connections.get(&key) {
            return Ok(Arc::clone(manager));
        }
        
        // Create new connection
        let mut manager = PostgresManager::new();
        manager.connect_async(connection_string).await?;
        let arc_manager = Arc::new(manager);
        
        // Store the connection
        connections.insert(key, Arc::clone(&arc_manager));
        
        Ok(arc_manager)
    }

    /// Get all PostgreSQL connections
    pub async fn get_all_pg_connections(&self) -> HashMap<PgConnectionKey, Arc<PostgresManager>> {
        self.pg_connections.read().await.clone()
    }

    /// Remove a PostgreSQL connection
    pub async fn remove_pg_connection(&self, key: &PgConnectionKey) -> Option<Arc<PostgresManager>> {
        let mut connections = self.pg_connections.write().await;
        connections.remove(key)
    }
}
