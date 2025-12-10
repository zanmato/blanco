use anyhow::Result;
use async_compat;
use russh::client as russh_client;
use russh::client::{Config as SshConfig, Handle as SshHandle};
use russh::keys::load_secret_key;
use std::sync::{Arc, Mutex};
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;

use gpui::BackgroundExecutor;

/// SSH tunnel configuration
#[derive(Debug, Clone)]
pub struct SshTunnelConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
}

/// Connection identifier for tracking active connections
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionId(pub String);

impl SshTunnelConfig {
    pub fn from_connection_data(
        conn: &crate::app_database::ConnectionData,
        assigned_local_port: u16,
    ) -> Result<Self> {
        if conn.ssh_host.is_none() || conn.ssh_user.is_none() {
            anyhow::bail!("SSH configuration is incomplete");
        }

        Ok(Self {
            ssh_host: conn.ssh_host.as_ref().unwrap().clone(),
            ssh_port: conn.ssh_port.unwrap_or(22) as u16,
            ssh_user: conn.ssh_user.as_ref().unwrap().clone(),
            ssh_password: conn.ssh_password.clone(),
            ssh_private_key_path: conn.ssh_private_key_path.clone(),
            ssh_private_key_password: conn.ssh_private_key_password.clone(),
            remote_host: conn.host.as_ref().unwrap().clone(),
            remote_port: conn.port.unwrap_or(5432) as u16,
            local_port: assigned_local_port,
        })
    }
}

/// SSH tunnel status
#[derive(Debug, Clone, PartialEq)]
pub enum TunnelStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// SSH client handler
struct SshClientHandler {
    // Add any state needed for the SSH client handler
}

#[async_trait::async_trait]
impl russh_client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // In production, you should verify the server key against a known hosts file
        Ok(true)
    }
}

/// Real SSH tunnel implementation using russh
pub struct SshTunnel {
    session: Option<SshHandle<SshClientHandler>>,
    local_port: u16,
    config: SshTunnelConfig,
    status: Arc<Mutex<TunnelStatus>>,
    is_running: Arc<Mutex<bool>>,
    active_connections: Arc<Mutex<std::collections::HashSet<ConnectionId>>>,
    background_executor: BackgroundExecutor,
}

impl SshTunnel {
    pub async fn create(
        config: SshTunnelConfig,
        background_executor: BackgroundExecutor,
    ) -> Result<Self> {
        let mut tunnel = Self {
            session: None,
            local_port: config.local_port,
            config,
            status: Arc::new(Mutex::new(TunnelStatus::Disconnected)),
            is_running: Arc::new(Mutex::new(false)),
            active_connections: Arc::new(Mutex::new(std::collections::HashSet::new())),
            background_executor,
        };

        tunnel.connect().await?;
        Ok(tunnel)
    }

    async fn connect(&mut self) -> Result<()> {
        *self.status.lock().unwrap() = TunnelStatus::Connecting;

        // Create SSH config
        let ssh_config = Arc::new(SshConfig::default());
        let handler = SshClientHandler {};

        // Connect to SSH server
        let mut session = russh_client::connect(
            ssh_config,
            (self.config.ssh_host.as_str(), self.config.ssh_port),
            handler,
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to SSH server: {}", e))?;

        // Handle authentication
        if let Some(key_path) = &self.config.ssh_private_key_path {
            let key = load_secret_key(&key_path, self.config.ssh_private_key_password.as_deref())
                .map_err(|e| anyhow::anyhow!("Failed to load private key: {}", e))?;

            session
                .authenticate_publickey(&self.config.ssh_user, Arc::new(key))
                .await
                .map_err(|e| anyhow::anyhow!("SSH key authentication failed: {}", e))?;
        } else if let Some(password) = &self.config.ssh_password {
            session
                .authenticate_password(&self.config.ssh_user, password)
                .await
                .map_err(|e| anyhow::anyhow!("SSH password authentication failed: {}", e))?;
        } else {
            anyhow::bail!("No authentication method provided");
        }

        self.session = Some(session);
        *self.status.lock().unwrap() = TunnelStatus::Connected;
        Ok(())
    }

    pub async fn setup_tcp_forwarding(&mut self) -> Result<()> {
        let remote_host = self.config.remote_host.clone();
        let remote_port = self.config.remote_port;
        let local_port = self.local_port;
        let is_running = Arc::clone(&self.is_running);
        let status = Arc::clone(&self.status);

        *is_running.lock().unwrap() = true;

        // Create local listener
        let listener = TcpListener::bind(format!("127.0.0.1:{}", local_port))
            .await
            .map_err(|e| {
                *status.lock().unwrap() =
                    TunnelStatus::Failed(format!("Failed to bind local port: {}", e));
                anyhow::anyhow!("Failed to bind local port: {}", e)
            })?;

        log::info!(
            "SSH tunnel listening on 127.0.0.1:{} (remote port: {})",
            local_port,
            remote_port
        );

        // Wrap the session in Arc<TokioMutex<>> so it can be shared between connection tasks
        let session = Arc::new(tokio::sync::Mutex::new(self.session.take().unwrap()));
        let active_connections = Arc::clone(&self.active_connections);

        log::debug!("Using background executor for SSH tunnel spawn");
        let background_executor = self.background_executor.clone();
        let bg_executor_for_connections = self.background_executor.clone();
        background_executor.spawn(async move {
            async_compat::Compat::new(async move {
                // Handle incoming connections
                while *is_running.lock().unwrap() {
                    match listener.accept().await {
                        Ok((mut local_socket, _)) => {
                            let remote_host = remote_host.clone();
                            let remote_port = remote_port;
                            let local_port = local_port;
                            let session = Arc::clone(&session);
                            let active_connections = Arc::clone(&active_connections);

                            // Generate a unique connection ID
                            let connection_id = ConnectionId(format!("{}:{}->{}:{}-{}",
                                "127.0.0.1", local_port, remote_host, remote_port,
                                uuid::Uuid::new_v4().to_string()));

                            // Add connection to tracking
                            {
                                let mut connections = active_connections.lock().unwrap();
                                connections.insert(connection_id.clone());
                                log::debug!("Added connection {} to tunnel. Active connections: {}",
                                    connection_id.0, connections.len());
                            }

                            // Spawn a separate task for each connection
                            let bg_executor_inner = bg_executor_for_connections.clone();
                            bg_executor_inner.spawn(async move {
                                async_compat::Compat::new(async move {
                                    log::debug!("Processing connection {}", connection_id.0);

                                    // Open SSH channel to remote host:port
                                    let ssh_channel = {
                                        let session = session.lock().await;
                                        session.channel_open_direct_tcpip(
                                            &remote_host,
                                            remote_port as u32,
                                            "127.0.0.1",
                                            local_port as u32
                                        ).await
                                    };

                                    match ssh_channel {
                                        Ok(ssh_channel) => {
                                            let mut ssh_stream = ssh_channel.into_stream();

                                            // Copy data bidirectionally between local socket and SSH stream
                                            match copy_bidirectional(&mut local_socket, &mut ssh_stream).await {
                                                Ok((bytes_to_local, bytes_to_remote)) => {
                                                    log::debug!("Connection {} completed. {} bytes to local, {} bytes to remote",
                                                        connection_id.0, bytes_to_local, bytes_to_remote);
                                                }
                                                Err(e) => {
                                                    log::error!("Error copying data for connection {}: {}", connection_id.0, e);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to open SSH forwarding channel for connection {}: {}", connection_id.0, e);
                                        }
                                    }

                                    // Remove connection from tracking
                                    {
                                        let mut connections = active_connections.lock().unwrap();
                                        connections.remove(&connection_id);
                                        let connection_count = connections.len();
                                        log::debug!("Removed connection {} from tunnel. Active connections: {}",
                                            connection_id.0, connection_count);

                                        // If no more active connections, consider closing the tunnel
                                        if connection_count == 0 {
                                            log::info!("No more active connections for tunnel on port {}", local_port);
                                            // Note: We don't close the tunnel automatically here as the DbService
                                            // should manage the tunnel lifecycle based on its own logic
                                        }
                                    }
                            }).await
                        }).detach();
                        }
                        Err(e) => {
                            log::error!("Failed to accept local connection: {}", e);
                            break;
                        }
                    }
                }
                log::warn!("Tunnel is not running anymore");
            }).await
        }).detach();

        Ok(())
    }

    pub fn status(&self) -> TunnelStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub async fn close(&mut self) -> Result<()> {
        *self.status.lock().unwrap() = TunnelStatus::Disconnected;
        *self.is_running.lock().unwrap() = false;
        if let Some(session) = self.session.take() {
            // The session will be closed when dropped
            drop(session);
        }
        Ok(())
    }

    /// Get the number of active connections for this tunnel
    pub fn active_connection_count(&self) -> usize {
        self.active_connections.lock().unwrap().len()
    }

    /// Check if there are any active connections
    pub fn has_active_connections(&self) -> bool {
        !self.active_connections.lock().unwrap().is_empty()
    }

    /// Get a copy of the active connection IDs
    pub fn get_active_connections(&self) -> std::collections::HashSet<ConnectionId> {
        self.active_connections.lock().unwrap().clone()
    }
}
