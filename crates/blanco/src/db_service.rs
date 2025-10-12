use crate::app_database::AppDatabase;
use crate::database::DatabaseManager;
use crate::postgres::PostgresManager;
use gpui::{App, Global};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Connection key for PostgreSQL connections
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PgConnectionKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
}

impl PgConnectionKey {
    pub fn from_connection_string(conn_str: &str) -> Result<Self, anyhow::Error> {
        // Parse connection string to extract connection details
        // Expected format: postgresql://username[:password]@host:port/database
        let url = url::Url::parse(conn_str)
            .map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;
        
        let host = url.host_str().unwrap_or("localhost").to_string();
        let port = url.port().unwrap_or(5432);
        let database = url.path().trim_start_matches('/').to_string();
        let username = url.username().to_string();
        
        Ok(PgConnectionKey {
            host,
            port,
            database,
            username,
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
