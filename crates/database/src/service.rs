//! Database service implementation with connection and SSH tunnel management

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    Connection, ConnectionFactory, DatabaseService as DatabaseServiceTrait, DriverType,
};
use gpui::Global;
use smol::channel;
use smol::lock::RwLock;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use crate::connection_config::{ConnectionConfig, DatabaseType};
use crate::factories::{
    MysqlConnectionFactory, PostgresConnectionFactory, SqliteConnectionFactory,
};
use crate::ssh_tunnel::{SshTunnel, SshTunnelConfig, TunnelInfo};

// Message types for channel-based action dispatch
#[derive(Clone, Debug)]
pub struct DatabaseConnectedMessage {
    pub connection_id: i64,
    pub database_name: String,
}

#[derive(Clone, Debug)]
pub struct DatabaseDisconnectedMessage {
    pub connection_id: i64,
    pub database_name: String,
}

/// Enum for all messages that can be sent through the database service channel
#[derive(Clone, Debug)]
pub enum DatabaseServiceMessage {
    Connected(DatabaseConnectedMessage),
    Disconnected(DatabaseDisconnectedMessage),
}

type SharedActionSender = Arc<StdMutex<Option<channel::Sender<DatabaseServiceMessage>>>>;

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

    // Channel sender for dispatching actions from any context (shared via Arc)
    action_sender: SharedActionSender,

    // Dependencies
    runtime_handle: tokio::runtime::Handle,
}

impl DatabaseService {
    pub fn new(runtime_handle: tokio::runtime::Handle) -> Self {
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
            action_sender: Arc::new(StdMutex::new(None)),
            runtime_handle,
        }
    }

    /// Set the action sender (called via cx.update_global from app initialization)
    pub fn set_action_sender(&mut self, sender: channel::Sender<DatabaseServiceMessage>) {
        tracing::info!(
            "Setting action_sender on DatabaseService, Arc address: {:p}",
            self.action_sender
        );
        *self.action_sender.lock().unwrap() = Some(sender);
        tracing::info!("Action sender set successfully");
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

    /// Get or create a connection with automatic SSH handling
    pub async fn get_or_create_connection(
        &self,
        config_id: DatabaseConfigId,
        database: Option<&str>,
    ) -> Result<Arc<dyn Connection>> {
        let connection_id = (config_id, database.unwrap_or("default").to_string());

        // Get connection config to check if SSH is required
        let configs = self.connection_configs.read().await;
        let config = configs
            .get(&config_id)
            .ok_or_else(|| {
                anyhow::anyhow!("Connection configuration not found for ID: {}", config_id)
            })?
            .clone();
        drop(configs);

        // For SSH connections, check tunnel health before returning cached connection
        if config.requires_ssh_tunnel() {
            // Check tunnel health
            let tunnel_healthy = {
                let tunnels = self.ssh_tunnels.read().await;
                if let Some(tunnel_mutex) = tunnels.get(&config_id) {
                    let tunnel = tunnel_mutex.lock().unwrap();
                    let healthy = tunnel.is_healthy_sync();
                    drop(tunnel);
                    healthy
                } else {
                    false
                }
                // tunnels guard dropped here
            };

            if !tunnel_healthy {
                tracing::info!(
                    "SSH tunnel for connection {} is unhealthy, will recreate connection",
                    config_id
                );
                // Remove the unhealthy cached connection
                let mut connections = self.active_connections.write().await;
                connections.remove(&connection_id);
                drop(connections);
            } else {
                // Tunnel is healthy, check if we have a cached connection
                let connections = self.active_connections.read().await;
                if let Some(existing_conn) = connections.get(&connection_id) {
                    return Ok(Arc::clone(existing_conn));
                }
            }
        } else {
            // Non-SSH connection, use simple caching
            let connections = self.active_connections.read().await;
            if let Some(existing_conn) = connections.get(&connection_id) {
                return Ok(Arc::clone(existing_conn));
            }
        }

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
            config.connection_string(database, connection_host, connection_port);
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

        // Send action message for newly created connections
        tracing::info!(
            "About to send, action_sender Arc address: {:p}",
            self.action_sender
        );
        let sender_opt = self.action_sender.lock().unwrap().clone();
        // Lock is dropped here

        if let Some(sender) = sender_opt {
            tracing::info!(
                "Dispatching DatabaseServiceMessage::Connected for connection ID: {}",
                config_id
            );
            sender
                .send(DatabaseServiceMessage::Connected(
                    DatabaseConnectedMessage {
                        connection_id: config_id,
                        database_name: database.unwrap_or("default").to_string(),
                    },
                ))
                .await?;
        } else {
            tracing::warn!("action_sender is None, cannot dispatch DatabaseServiceMessage");
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
        let disconnected_connections: Vec<(DatabaseConfigId, String)>;

        // Remove from active_connections
        {
            let mut connections = self.active_connections.write().await;

            if let Some(database_name) = database {
                // Disconnect specific database connection
                let connection_id = (config_id, database_name.to_string());
                connections.remove(&connection_id);
                disconnected_connections = vec![connection_id];
            } else {
                // Disconnect all connections for this config_id
                disconnected_connections = connections
                    .keys()
                    .filter(|(id, _)| *id == config_id)
                    .cloned()
                    .collect();

                for connection_id in &disconnected_connections {
                    connections.remove(connection_id);
                }
            }
        }

        // First, remove from tunnel_connections tracking to get accurate tunnel usage
        let tunnels_to_check: std::collections::HashSet<DatabaseConfigId> = {
            let tunnel_connections = self.tunnel_connections.read().await;

            disconnected_connections
                .iter()
                .filter_map(|connection_id| tunnel_connections.get(connection_id))
                .copied()
                .collect()
        };

        // Remove the disconnected connections from tunnel_connections tracking
        {
            let mut tunnel_connections = self.tunnel_connections.write().await;
            for connection_id in &disconnected_connections {
                tunnel_connections.remove(connection_id);
            }
        }

        // Now check if we need to clean up SSH tunnels (after removal)
        for tunnel_config_id in tunnels_to_check {
            let should_remove_tunnel;

            {
                let tunnel_connections = self.tunnel_connections.read().await;

                // Check if any other connections are using this tunnel
                let tunnels_in_use = tunnel_connections
                    .values()
                    .any(|&id| id == tunnel_config_id);

                should_remove_tunnel = !tunnels_in_use;
            }

            if should_remove_tunnel {
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

        tracing::info!(
            "Disconnected {} connections for config {:?}",
            disconnected_connections.len(),
            config_id
        );
        Ok(())
    }

    /// Get active connections reference
    pub async fn get_active_connections(
        &self,
    ) -> std::collections::HashMap<ConnectionId, Arc<dyn Connection>> {
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
        // Check if tunnel already exists and is healthy
        {
            let tunnels = self.ssh_tunnels.read().await;
            if let Some(tunnel_mutex) = tunnels.get(&config.id) {
                let tunnel = tunnel_mutex.lock().unwrap();
                if tunnel.is_healthy_sync() {
                    return Ok(tunnel.get_info());
                }
                tracing::info!(
                    "SSH tunnel for connection {} is unhealthy, recreating",
                    config.id
                );
            }
        }

        // Remove unhealthy tunnel if it exists
        {
            let mut tunnels = self.ssh_tunnels.write().await;
            tunnels.remove(&config.id);
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

        // Create tunnel using the tokio runtime handle
        let runtime_handle = self.runtime_handle.clone();
        let runtime_handle_inner = runtime_handle.clone();
        let tunnel = runtime_handle
            .spawn(async move {
                let mut tunnel =
                    SshTunnel::create(ssh_config, runtime_handle_inner.clone()).await?;
                tunnel.connect().await?;
                Result::<SshTunnel, anyhow::Error>::Ok(tunnel)
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to spawn SSH tunnel creation: {}", e))??;

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

    /// Test a connection without caching it. Creates a temporary connection
    /// (and SSH tunnel if needed), verifies it works, then drops everything.
    pub async fn test_connection(&self, config: &ConnectionConfig) -> Result<()> {
        let mut connection_host = None;
        let mut connection_port = None;
        let mut temp_tunnel: Option<SshTunnel> = None;

        if config.requires_ssh_tunnel() {
            let remote_host = &config.host;
            let remote_port = config.port;
            let local_port = self.assign_local_port();

            let ssh_config = SshTunnelConfig {
                ssh_host: config
                    .ssh_host
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("SSH host is required"))?
                    .clone(),
                ssh_port: config.ssh_port(),
                ssh_user: config
                    .ssh_user
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("SSH user is required"))?
                    .clone(),
                ssh_password: config.ssh_password.clone(),
                ssh_private_key_path: config.ssh_private_key_path.clone(),
                ssh_private_key_password: config.ssh_private_key_password.clone(),
                remote_host: remote_host.to_string(),
                remote_port,
                local_port,
            };

            let runtime_handle = self.runtime_handle.clone();
            let runtime_handle_inner = runtime_handle.clone();
            let tunnel = runtime_handle
                .spawn(async move {
                    let mut tunnel =
                        SshTunnel::create(ssh_config, runtime_handle_inner.clone()).await?;
                    tunnel.connect().await?;
                    Result::<SshTunnel, anyhow::Error>::Ok(tunnel)
                })
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create SSH tunnel: {}", e))??;

            connection_host = Some("localhost");
            connection_port = Some(local_port);
            temp_tunnel = Some(tunnel);
        }

        let connection_string = config.connection_string(None, connection_host, connection_port);

        let factory = self
            .connection_factories
            .get(&config.db_type.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", config.db_type)
            })?;

        let result = factory.create_connection(&connection_string).await;

        // Clean up temporary tunnel
        drop(temp_tunnel);

        result.map(|_| ())
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

    async fn get_connection_status(
        &self,
        connection_id: i64,
        database_name: Option<&str>,
    ) -> Result<blanco_core::database_service::ConnectionStatus> {
        let db_name = database_name.unwrap_or("default").to_string();
        let connection_id_key = (connection_id, db_name.clone());

        let is_connected = {
            let connections = self.active_connections.read().await;
            connections.contains_key(&connection_id_key)
        };

        let connection_type = if is_connected {
            let connections = self.active_connections.read().await;
            connections
                .get(&connection_id_key)
                .map(|conn| conn.get_connection_type().to_owned())
                .unwrap_or_else(|| "Unknown".to_string())
        } else {
            // Get connection type from config if not connected
            let configs = self.connection_configs.read().await;
            configs
                .get(&connection_id)
                .map(|config| DriverType::from(config.db_type).to_string().to_owned())
                .unwrap_or_else(|| "Unknown".to_string())
        };

        Ok(blanco_core::database_service::ConnectionStatus {
            connection_id,
            database_name: db_name,
            is_connected,
            connection_type,
        })
    }

    async fn get_active_connection_statuses(
        &self,
    ) -> Result<
        std::collections::HashMap<(i64, String), blanco_core::database_service::ConnectionStatus>,
    > {
        let connections = self.active_connections.read().await;
        let mut statuses = std::collections::HashMap::new();

        for ((config_id, db_name), connection) in connections.iter() {
            statuses.insert(
                (*config_id, db_name.clone()),
                blanco_core::database_service::ConnectionStatus {
                    connection_id: *config_id,
                    database_name: db_name.clone(),
                    is_connected: true,
                    connection_type: connection.get_connection_type().to_owned(),
                },
            );
        }

        Ok(statuses)
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
            action_sender: Arc::clone(&self.action_sender),
            runtime_handle: self.runtime_handle.clone(),
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
