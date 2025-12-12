//! Database service implementation with connection and SSH tunnel management

use anyhow::Result;
use async_std::sync::RwLock;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory, DatabaseService as DatabaseServiceTrait};
use gpui::{BackgroundExecutor, Global};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use crate::connection_config::{ConnectionConfig, DatabaseType};
use crate::factories::{
    MysqlConnectionFactory, PostgresConnectionFactory, SqliteConnectionFactory,
};
use crate::ssh_tunnel::{SshTunnel, SshTunnelConfig, TunnelInfo};

// Type aliases for clarity
pub type DatabaseConfigId = i64; // Connection ID from app_database
pub type ConnectionId = (DatabaseConfigId, String); // (config_id, database_name)

/// Main database service that manages connections and SSH tunnels internally
pub struct DatabaseService {
    // Connection storage
    connection_configs: Arc<RwLock<HashMap<DatabaseConfigId, ConnectionConfig>>>,
    active_connections: Arc<RwLock<HashMap<ConnectionId, Arc<dyn Connection>>>>,

    // Connection factories
    connection_factories: Arc<HashMap<String, Arc<dyn ConnectionFactory>>>,

    // Internal SSH management (private)
    ssh_tunnels: Arc<RwLock<HashMap<DatabaseConfigId, Arc<StdMutex<SshTunnel>>>>>,
    tunnel_connections: Arc<RwLock<HashMap<ConnectionId, DatabaseConfigId>>>,

    // Dependencies
    background_executor: BackgroundExecutor,
}

impl DatabaseService {
    pub fn new(background_executor: BackgroundExecutor) -> Self {
        // Initialize connection factories
        let mut factories: HashMap<String, Arc<dyn ConnectionFactory>> = HashMap::new();
        factories.insert(
            DatabaseType::SQLite.to_string(),
            Arc::new(SqliteConnectionFactory),
        );
        factories.insert(
            DatabaseType::PostgreSQL.to_string(),
            Arc::new(PostgresConnectionFactory::new()),
        );
        factories.insert(
            DatabaseType::MySQL.to_string(),
            Arc::new(MysqlConnectionFactory::new()),
        );

        Self {
            connection_configs: Arc::new(RwLock::new(HashMap::new())),
            active_connections: Arc::new(RwLock::new(HashMap::new())),
            connection_factories: Arc::new(factories),
            ssh_tunnels: Arc::new(RwLock::new(HashMap::new())),
            tunnel_connections: Arc::new(RwLock::new(HashMap::new())),
            background_executor,
        }
    }

    /// Add a connection configuration
    pub async fn add_connection_config(&self, config: ConnectionConfig) {
        let mut configs = self.connection_configs.write().await;
        configs.insert(config.id, config.clone());
        tracing::info!(
            "Added connection configuration: {} ({})",
            config.name,
            config.db_type
        );
    }

    /// Get a connection configuration
    pub async fn get_connection_config(&self, id: DatabaseConfigId) -> Option<ConnectionConfig> {
        let configs = self.connection_configs.read().await;
        configs.get(&id).cloned()
    }

    /// Main API - get or create a connection with automatic SSH handling
    pub async fn get_or_create_connection(
        &self,
        config_id: DatabaseConfigId,
        database: Option<&str>,
    ) -> Result<Arc<dyn Connection>> {
        // Check if connection is in active_connections
        let connection_id = (config_id, database.unwrap_or("default").to_string());

        {
            let connections = self.active_connections.read().await;
            if let Some(existing_conn) = connections.get(&connection_id) {
                // Check if the connection is still healthy
                if existing_conn.test_connection().await.unwrap_or(false) {
                    tracing::debug!("Using existing healthy connection: {:?}", connection_id);
                    return Ok(Arc::clone(existing_conn));
                } else {
                    tracing::info!(
                        "Existing connection is unhealthy, will recreate: {:?}",
                        connection_id
                    );
                    drop(connections);
                }
            }
        }

        // Get connection configuration
        let configs = self.connection_configs.read().await;
        let config = configs
            .get(&config_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Connection configuration not found for ID: {}", config_id)
            })?
            .clone();
        drop(configs);

        // Check if this connection requires an SSH tunnel
        let mut connection_host = None;
        let mut connection_port = None;

        if config.requires_ssh_tunnel() {
            // Use the config's host and port directly for SSH tunnel
            let remote_host = &config.host;
            let remote_port = config.port;

            // Ensure SSH tunnel exists
            let tunnel_info = self
                .ensure_ssh_tunnel(&config, remote_host, remote_port)
                .await?;

            // Override host and port for connection string to use tunnel
            connection_host = Some("localhost");
            connection_port = Some(tunnel_info.local_port);

            tracing::info!(
                "Created SSH tunnel for {}@{}:{} -> localhost:{}",
                config.database,
                remote_host,
                remote_port,
                tunnel_info.local_port
            );
        }

        // Get connection string with optional database, host, and port overrides
        let connection_string =
            config.connection_string(database, connection_host.as_deref(), connection_port);
        if let Some(database_name) = database {
            tracing::debug!(
                "Applied database override '{}' to connection string: {}",
                database_name,
                connection_string
            );
        }

        // Create connection via factory
        let factory = self
            .connection_factories
            .get(&config.db_type.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", config.db_type)
            })?;

        let conn = factory.create_connection(&connection_string).await?;
        let conn_arc: Arc<dyn Connection> = Arc::from(conn);

        // Store in active_connections
        {
            let mut connections = self.active_connections.write().await;
            connections.insert(connection_id.clone(), conn_arc.clone());
        }

        // Track which connections use SSH tunnels
        if config.requires_ssh_tunnel() {
            let mut tunnel_connections = self.tunnel_connections.write().await;
            tunnel_connections.insert(connection_id.clone(), config_id);
        }

        tracing::info!(
            "Successfully created and stored connection: {:?}",
            connection_id
        );
        Ok(conn_arc)
    }

    /// Disconnect a connection
    pub async fn disconnect(
        &self,
        config_id: DatabaseConfigId,
        database: Option<&str>,
    ) -> Result<()> {
        let connection_id = (config_id, database.unwrap_or("default").to_string());

        // Remove from active_connections
        {
            let mut connections = self.active_connections.write().await;
            connections.remove(&connection_id);
        }

        // Check if we need to clean up SSH tunnel
        {
            let tunnel_connections = self.tunnel_connections.read().await;
            if let Some(tunnel_config_id) = tunnel_connections.get(&connection_id) {
                let tunnel_config_id = *tunnel_config_id;

                // Check if any other connections are using this tunnel
                let tunnels_in_use = tunnel_connections
                    .values()
                    .any(|&id| id == tunnel_config_id);

                if !tunnels_in_use {
                    // No other connections using this tunnel, remove it
                    let mut tunnels = self.ssh_tunnels.write().await;
                    if let Some(_tunnel_mutex) = tunnels.remove(&tunnel_config_id) {
                        // The tunnel will be disconnected when Arc is dropped
                        tracing::info!(
                            "Removed SSH tunnel for connection config {}",
                            tunnel_config_id
                        );
                    }
                }
            }
        }

        // Remove from tunnel_connections tracking
        {
            let mut tunnel_connections = self.tunnel_connections.write().await;
            tunnel_connections.remove(&connection_id);
        }

        tracing::info!("Disconnected connection: {:?}", connection_id);
        Ok(())
    }

    /// Get active connections reference
    pub async fn get_active_connections(&self) -> std::collections::HashMap<ConnectionId, Arc<dyn Connection>> {
        self.active_connections.read().await.clone()
    }

    // Private helper methods

    /// Ensure SSH tunnel exists, creating if necessary
    async fn ensure_ssh_tunnel(
        &self,
        config: &ConnectionConfig,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<TunnelInfo> {
        // Check if tunnel already exists
        {
            let tunnels = self.ssh_tunnels.read().await;
            if let Some(tunnel_mutex) = tunnels.get(&config.id) {
                // Check health using sync version
                let tunnel = tunnel_mutex.lock().unwrap();
                if tunnel.is_healthy_sync() {
                    return Ok(tunnel.get_info());
                }
            }
        }

        // Create new tunnel
        tracing::info!("Creating SSH tunnel for connection config {}", config.id);

        let local_port = self.assign_local_port();
        let ssh_config = SshTunnelConfig {
            ssh_host: config.ssh_host.as_ref().unwrap().clone(),
            ssh_port: config.ssh_port(),
            ssh_user: config.ssh_user.as_ref().unwrap().clone(),
            ssh_password: config.ssh_password.clone(),
            ssh_private_key_path: config.ssh_private_key_path.clone(),
            ssh_private_key_password: config.ssh_private_key_password.clone(),
            remote_host: remote_host.to_string(),
            remote_port,
            local_port,
        };

        // Create tunnel - wrap with async-compat to bridge tokio to async-std
        let mut tunnel = async_compat::Compat::new(async {
            SshTunnel::create(ssh_config, self.background_executor.clone()).await
        })
        .await?;

        // Connect before putting in mutex - wrap with async-compat to bridge tokio to async-std
        async_compat::Compat::new(async { tunnel.connect().await }).await?;

        // Get tunnel info
        let tunnel_info = tunnel.get_info();

        // Store in mutex after connection (the task is now stored inside the tunnel)
        let tunnel_mutex = Arc::new(StdMutex::new(tunnel));

        // Store tunnel
        {
            let mut tunnels = self.ssh_tunnels.write().await;
            tunnels.insert(config.id, tunnel_mutex);
        }

        Ok(tunnel_info)
    }

    /// Assign a local port for SSH tunnel
    fn assign_local_port(&self) -> u16 {
        // Simple port assignment starting from 15432
        static PORT_COUNTER: std::sync::atomic::AtomicU16 =
            std::sync::atomic::AtomicU16::new(15432);
        PORT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

// Implement the DatabaseService trait from blanco_core
#[async_trait]
impl DatabaseServiceTrait for DatabaseService {
    async fn get_or_create_connection_by_id(
        &self,
        connection_id: i64,
        database: Option<&str>,
    ) -> Result<Arc<dyn Connection>> {
        self.get_or_create_connection(connection_id, database).await
    }
}

impl Clone for DatabaseService {
    fn clone(&self) -> Self {
        Self {
            connection_configs: Arc::clone(&self.connection_configs),
            active_connections: Arc::clone(&self.active_connections),
            connection_factories: Arc::clone(&self.connection_factories),
            ssh_tunnels: Arc::clone(&self.ssh_tunnels),
            tunnel_connections: Arc::clone(&self.tunnel_connections),
            background_executor: self.background_executor.clone(),
        }
    }
}

impl Global for DatabaseService {}

impl DatabaseService {
    /// Get the global DatabaseService instance
    pub fn global(cx: &gpui::App) -> &Self {
        cx.global::<Self>()
    }
}
