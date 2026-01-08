//! Internal SSH tunnel management
//! This module provides SSH tunneling functionality that is internal to the database crate

use anyhow::Result;
use async_trait::async_trait;
use russh::client as russh_client;
use russh::client::{Config as SshConfig, Handle as SshHandle};
use russh::keys::load_secret_key;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use tokio::io::copy_bidirectional;
use tokio::net::TcpListener;

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

/// SSH tunnel information
#[derive(Debug)]
pub struct TunnelInfo {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub created_at: Instant,
}

impl Clone for TunnelInfo {
    fn clone(&self) -> Self {
        Self {
            local_port: self.local_port,
            remote_host: self.remote_host.clone(),
            remote_port: self.remote_port,
            created_at: self.created_at,
        }
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
struct SshClientHandler;

#[async_trait]
impl russh_client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TODO: verify the server key against a known hosts file
        tracing::debug!("Accepting SSH server key");
        Ok(true)
    }
}

/// Connection identifier for tracking active connections
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectionId(pub String);

/// Real SSH tunnel implementation using russh
pub struct SshTunnel {
    session: Option<SshHandle<SshClientHandler>>,
    config: SshTunnelConfig,
    status: Arc<StdMutex<TunnelStatus>>,
    is_running: Arc<StdMutex<bool>>,
    active_connections: Arc<StdMutex<std::collections::HashSet<ConnectionId>>>,
    runtime_handle: tokio::runtime::Handle,
    tunnel_task: Option<tokio::task::JoinHandle<()>>,
}

impl SshTunnel {
    pub async fn create(
        config: SshTunnelConfig,
        runtime_handle: tokio::runtime::Handle,
    ) -> Result<Self> {
        tracing::info!(
            "Creating SSH tunnel to {}:{} -> {}:{}",
            config.ssh_host,
            config.ssh_port,
            config.remote_host,
            config.remote_port
        );

        let tunnel = Self {
            session: None,
            config,
            status: Arc::new(StdMutex::new(TunnelStatus::Disconnected)),
            is_running: Arc::new(StdMutex::new(false)),
            active_connections: Arc::new(StdMutex::new(std::collections::HashSet::new())),
            runtime_handle,
            tunnel_task: None,
        };

        Ok(tunnel)
    }

    /// Get tunnel information
    pub fn get_info(&self) -> TunnelInfo {
        TunnelInfo {
            local_port: self.config.local_port,
            remote_host: self.config.remote_host.clone(),
            remote_port: self.config.remote_port,
            created_at: Instant::now(),
        }
    }

    /// Set the tunnel task to keep it alive
    pub fn set_tunnel_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.tunnel_task = Some(task);
    }

    /// Check if tunnel is healthy (non-async version)
    pub fn is_healthy_sync(&self) -> bool {
        let status = self.status.lock().unwrap().clone();
        matches!(status, TunnelStatus::Connected)
    }

    /// Check if tunnel is healthy
    pub async fn is_healthy(&self) -> bool {
        self.is_healthy_sync()
    }

    /// Establish the SSH connection and set up TCP forwarding
    pub async fn connect(&mut self) -> Result<()> {
        tracing::info!(
            "Connecting SSH tunnel to {}@{}:{}",
            self.config.ssh_user,
            self.config.ssh_host,
            self.config.ssh_port
        );

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
            let key = load_secret_key(key_path, self.config.ssh_private_key_password.as_deref())
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

        // Set up TCP forwarding and store the task
        let task = self.setup_tcp_forwarding().await?;
        self.tunnel_task = Some(task);

        tracing::info!("SSH tunnel established successfully");
        Ok(())
    }

    /// Set up TCP forwarding to handle actual tunneling
    async fn setup_tcp_forwarding(&mut self) -> Result<tokio::task::JoinHandle<()>> {
        let remote_host = self.config.remote_host.clone();
        let remote_port = self.config.remote_port;
        let local_port = self.config.local_port;
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

        tracing::info!(
            "SSH tunnel listening on 127.0.0.1:{} (remote port: {})",
            local_port,
            remote_port
        );

        // Wrap the session in Arc<TokioMutex<>> so it can be shared between connection tasks
        let session = Arc::new(tokio::sync::Mutex::new(self.session.take().unwrap()));
        let active_connections = Arc::clone(&self.active_connections);

        tracing::debug!("Using tokio runtime handle for SSH tunnel spawn");
        let runtime_handle = self.runtime_handle.clone();
        let runtime_handle_inner = runtime_handle.clone();
        let task = runtime_handle.spawn(async move {
            // Handle incoming connections
            while *is_running.lock().unwrap() {
                match listener.accept().await {
                    Ok((mut local_socket, _)) => {
                        let remote_host = remote_host.clone();
                        let remote_port = remote_port;
                        let local_port = local_port;
                        let session = Arc::clone(&session);
                        let active_connections = Arc::clone(&active_connections);
                        let runtime_handle = runtime_handle_inner.clone();

                        // Generate a unique connection ID
                        let connection_id = ConnectionId(format!(
                            "{}:{}->{}:{}-{}",
                            "127.0.0.1",
                            local_port,
                            remote_host,
                            remote_port,
                            uuid::Uuid::new_v4()
                        ));

                        // Add connection to tracking
                        {
                            let mut connections = active_connections.lock().unwrap();
                            connections.insert(connection_id.clone());
                        }

                        // Handle connection in background task
                        runtime_handle.spawn(async move {
                            tracing::debug!("Processing connection {}", connection_id.0);

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
                                            tracing::debug!("Connection {} completed. {} bytes to local, {} bytes to remote",
                                                connection_id.0, bytes_to_local, bytes_to_remote);
                                        }
                                        Err(e) => {
                                            tracing::error!("Error copying data for connection {}: {}", connection_id.0, e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to open SSH forwarding channel for connection {}: {}", connection_id.0, e);
                                }
                            }

                            // Remove connection from tracking
                            {
                                let mut connections = active_connections.lock().unwrap();
                                connections.remove(&connection_id);
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("Failed to accept SSH tunnel connection: {}", e);
                    }
                }
            }

            tracing::info!("SSH tunnel listener stopped");
        });

        Ok(task)
    }

    /// Disconnect the tunnel
    pub async fn disconnect(&mut self) -> Result<()> {
        tracing::info!("Disconnecting SSH tunnel");
        *self.is_running.lock().unwrap() = false;
        *self.status.lock().unwrap() = TunnelStatus::Disconnected;
        self.session = None;
        // Abort the task to stop the TCP forwarding
        if let Some(task) = self.tunnel_task.take() {
            task.abort();
        }
        Ok(())
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        tracing::debug!("Dropping SSH tunnel");
        *self.is_running.lock().unwrap() = false;
        // Abort the task to stop the TCP forwarding
        if let Some(task) = self.tunnel_task.take() {
            task.abort();
        }
    }
}
