//! Internal SSH tunnel management
//! This module provides SSH tunneling functionality that is internal to the database crate

use anyhow::Result;
use async_trait::async_trait;
use russh::client as russh_client;
use russh::client::{Config as SshConfig, Handle as SshHandle};
use russh::keys::{check_known_hosts_path, load_secret_key};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use crate::ssh_config::{parse_jump_hosts, SshConfig as SshConfigFile};
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
    /// Jump hosts in `ProxyJump` form (`user@bastion:22, other`), `none` for
    /// a direct connection, or `None` to take `ProxyJump` from
    /// `~/.ssh/config`.
    pub jump_hosts: Option<String>,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
}

/// One SSH server on the way to the database host: the jump hosts in order,
/// then the SSH host itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshHop {
    /// The address connected to, after `HostName` from the ssh config.
    pub host: String,
    pub port: u16,
    pub user: String,
    /// `IdentityFile`s from the ssh config for this host.
    pub identity_files: Vec<PathBuf>,
}

impl std::fmt::Display for SshHop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}:{}", self.user, self.host, self.port)
    }
}

/// The hops a tunnel goes through, resolving aliases, users, ports and
/// identity files with `ssh_config`. A port of 22 counts as not set, so a
/// `Port` from the config wins over the dialog's default.
pub fn plan_hops(
    config: &SshTunnelConfig,
    ssh_config: &SshConfigFile,
    local_user: &str,
) -> Result<Vec<SshHop>> {
    let target = ssh_config.resolve(&config.ssh_host);
    let jumps = match config
        .jump_hosts
        .as_deref()
        .filter(|jumps| !jumps.trim().is_empty())
        .or(target.proxy_jump.as_deref())
    {
        Some(jumps) => parse_jump_hosts(jumps).map_err(anyhow::Error::msg)?,
        None => Vec::new(),
    };
    let mut hops: Vec<SshHop> = jumps
        .into_iter()
        .map(|jump| {
            let resolved = ssh_config.resolve(&jump.host);
            SshHop {
                host: resolved.host_name.unwrap_or(jump.host),
                port: jump.port.or(resolved.port).unwrap_or(22),
                user: jump
                    .user
                    .or(resolved.user)
                    .unwrap_or_else(|| local_user.to_string()),
                identity_files: resolved.identity_files,
            }
        })
        .collect();
    let user = Some(config.ssh_user.trim())
        .filter(|user| !user.is_empty())
        .map(str::to_string)
        .or(target.user)
        .unwrap_or_else(|| local_user.to_string());
    hops.push(SshHop {
        host: target.host_name.unwrap_or_else(|| config.ssh_host.clone()),
        port: match config.ssh_port {
            22 => target.port.unwrap_or(22),
            port => port,
        },
        user,
        identity_files: target.identity_files,
    });
    Ok(hops)
}

/// The keys OpenSSH tries when nothing names one.
fn default_identity_files() -> Vec<PathBuf> {
    let Some(ssh_dir) = dirs::home_dir().map(|home| home.join(".ssh")) else {
        return Vec::new();
    };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| ssh_dir.join(name))
        .collect()
}

fn local_user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_string())
}

/// Authenticate `hop`, trying in turn what the connection itself names (the
/// key chosen in the dialog, then the password, which is only offered to the
/// SSH host itself), then the hop's `IdentityFile`s (or the default keys),
/// and last the SSH agent's keys.
///
/// The order matters because a server allows only a few attempts. An agent
/// can hold more keys than that, so everything named for this host goes
/// first, and among the agent's keys the ones that belong to an identity
/// file lead, as in OpenSSH.
async fn authenticate(
    session: &mut SshHandle<SshClientHandler>,
    hop: &SshHop,
    config: &SshTunnelConfig,
    is_target: bool,
) -> Result<()> {
    let mut tried: Vec<String> = Vec::new();
    let user = hop.user.as_str();
    // The server hangs up once its attempts are used up, which surfaces as
    // a send error on the next one.
    let closed = |tried: &[String], error: russh::Error| {
        anyhow::anyhow!(
            "SSH authentication failed for {hop}: the host closed the connection ({error}), \
             most likely after too many attempts. Choose a private key for the connection or \
             name one with IdentityFile in ~/.ssh/config; tried {}",
            tried.join(", ")
        )
    };

    if let Some(path) = &config.ssh_private_key_path {
        match load_secret_key(path, config.ssh_private_key_password.as_deref()) {
            Ok(key) => {
                tried.push(path.clone());
                match session.authenticate_publickey(user, Arc::new(key)).await {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(error) => return Err(closed(&tried, error)),
                }
            }
            Err(error) if is_target => {
                anyhow::bail!("Failed to load private key {path}: {error}");
            }
            // The key may be meant for the SSH host alone, so a jump host
            // carries on without it, but says so if nothing else works.
            Err(error) => tried.push(format!("{path} (not loaded: {error})")),
        }
    }

    if let Some(password) = config.ssh_password.as_ref().filter(|_| is_target) {
        tried.push("password".to_string());
        match session.authenticate_password(user, password).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => return Err(closed(&tried, error)),
        }
    }

    let explicit = config.ssh_private_key_path.as_deref().map(PathBuf::from);
    let identity_files: Vec<PathBuf> = if hop.identity_files.is_empty() {
        default_identity_files()
    } else {
        hop.identity_files.clone()
    }
    .into_iter()
    .filter(|file| file.exists() && Some(file) != explicit.as_ref())
    .collect();
    // A key the agent holds is usually encrypted on disk, so it is left to
    // the agent below; its public half says which agent key that is.
    let mut agent_first = Vec::new();
    for file in &identity_files {
        match load_secret_key(file, config.ssh_private_key_password.as_deref()) {
            Ok(key) => {
                tried.push(file.display().to_string());
                match session.authenticate_publickey(user, Arc::new(key)).await {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(error) => return Err(closed(&tried, error)),
                }
            }
            Err(error) => {
                let mut public_file = file.clone().into_os_string();
                public_file.push(".pub");
                match russh::keys::load_public_key(&public_file) {
                    Ok(public_key) => agent_first.push(public_key),
                    Err(_) => tried.push(format!("{} (not loaded: {error})", file.display())),
                }
            }
        }
    }

    let agent = russh::keys::agent::client::AgentClient::connect_env().await;
    if let Ok(mut agent) = agent {
        let mut identities = agent.request_identities().await.unwrap_or_default();
        identities.sort_by_key(|key| !agent_first.contains(key));
        let count = identities.len();
        if count > 0 {
            tried.push(format!("{count} key(s) from the SSH agent"));
        }
        for key in identities {
            let (returned, result) = session.authenticate_future(user, key, agent).await;
            agent = returned;
            match result {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                // The agent could not sign with this key; the next may work.
                Err(russh::AgentAuthError::Key(_)) => {}
                Err(russh::AgentAuthError::Send(_)) => {
                    return Err(closed(&tried, russh::Error::SendError));
                }
            }
        }
    }

    if tried.is_empty() {
        anyhow::bail!(
            "No way to authenticate {hop}: no key file, SSH agent, default key or password"
        );
    }
    anyhow::bail!(
        "SSH authentication failed for {hop}; tried {}",
        tried.join(", ")
    )
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

/// SSH client handler with access to tunnel state
struct SshClientHandler {
    status: Arc<StdMutex<TunnelStatus>>,
    is_running: Arc<StdMutex<bool>>,
    host: String,
    port: u16,
}

#[async_trait]
impl russh_client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // The file OpenSSH keeps on every platform. russh's own default is
        // `~/ssh/known_hosts` on Windows, which nothing writes to.
        let trusted = match dirs::home_dir() {
            Some(home) => check_known_hosts_path(
                &self.host,
                self.port,
                server_public_key,
                home.join(".ssh").join("known_hosts"),
            )?,
            None => false,
        };
        if !trusted {
            tracing::error!(
                host = %self.host,
                port = self.port,
                fingerprint = %server_public_key.fingerprint(),
                "SSH server key is not present in known_hosts"
            );
        }
        Ok(trusted)
    }
}

impl Drop for SshClientHandler {
    fn drop(&mut self) {
        tracing::info!("SSH session ended, marking tunnel as disconnected");
        if let Ok(mut running) = self.is_running.lock() {
            *running = false;
        }
        if let Ok(mut status) = self.status.lock() {
            *status = TunnelStatus::Disconnected;
        }
    }
}

/// Connection identifier for tracking active connections
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectionId(pub String);

/// Real SSH tunnel implementation using russh
pub struct SshTunnel {
    session: Option<SshHandle<SshClientHandler>>,
    /// Sessions to the jump hosts, kept open while the tunnel runs through
    /// them.
    jump_sessions: Vec<SshHandle<SshClientHandler>>,
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
            jump_sessions: Vec::new(),
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
        self.status
            .lock()
            .ok()
            .map(|status| matches!(*status, TunnelStatus::Connected))
            .unwrap_or(false)
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

        // Clean up any existing tunnel before reconnecting
        if let Ok(mut running) = self.is_running.lock() {
            *running = false;
        }
        if let Some(task) = self.tunnel_task.take() {
            task.abort();
        }
        // Give a moment for the disconnect to complete
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        if let Ok(mut status) = self.status.lock() {
            *status = TunnelStatus::Connecting;
        }

        let hops = plan_hops(
            &self.config,
            &SshConfigFile::load_user(),
            &local_user_name(),
        )?;
        let ssh_config = Arc::new(SshConfig::default());
        let mut sessions: Vec<SshHandle<SshClientHandler>> = Vec::new();
        let last = hops.len() - 1;
        for (ix, hop) in hops.iter().enumerate() {
            // Known hosts are checked for every hop, against its real name.
            let handler = SshClientHandler {
                status: Arc::clone(&self.status),
                is_running: Arc::clone(&self.is_running),
                host: hop.host.clone(),
                port: hop.port,
            };
            let connected = match sessions.last() {
                None => {
                    russh_client::connect(
                        Arc::clone(&ssh_config),
                        (hop.host.as_str(), hop.port),
                        handler,
                    )
                    .await
                }
                Some(previous) => {
                    let channel = previous
                        .channel_open_direct_tcpip(
                            hop.host.clone(),
                            u32::from(hop.port),
                            "127.0.0.1",
                            0,
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!("Jump host could not reach {hop}: {e}"))?;
                    russh_client::connect_stream(
                        Arc::clone(&ssh_config),
                        channel.into_stream(),
                        handler,
                    )
                    .await
                }
            };
            let mut session = connected.map_err(|e| match e {
                russh::Error::UnknownKey => anyhow::anyhow!(
                    "The host key of {}:{} is not in ~/.ssh/known_hosts; connect once with ssh to add it",
                    hop.host,
                    hop.port
                ),
                e => anyhow::anyhow!("Failed to connect to SSH server {hop}: {e}"),
            })?;
            authenticate(&mut session, hop, &self.config, ix == last).await?;
            sessions.push(session);
        }
        let session = sessions
            .pop()
            .ok_or_else(|| anyhow::anyhow!("No SSH host to connect to"))?;
        self.jump_sessions = sessions;

        self.session = Some(session);
        if let Ok(mut status) = self.status.lock() {
            *status = TunnelStatus::Connected;
        }

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

        if let Ok(mut running) = is_running.lock() {
            *running = true;
        }

        // Create local listener
        let listener = TcpListener::bind(format!("127.0.0.1:{}", local_port))
            .await
            .map_err(|e| {
                if let Ok(mut status) = status.lock() {
                    *status = TunnelStatus::Failed(format!("Failed to bind local port: {}", e));
                }
                anyhow::anyhow!("Failed to bind local port: {}", e)
            })?;

        tracing::info!(
            "SSH tunnel listening on 127.0.0.1:{} (remote port: {})",
            local_port,
            remote_port
        );

        // Wrap the session in Arc<TokioMutex<>> so it can be shared between connection tasks
        let session = self
            .session
            .take()
            .ok_or_else(|| anyhow::anyhow!("SSH session must be established before forwarding"))?;
        let session = Arc::new(tokio::sync::Mutex::new(session));
        let active_connections = Arc::clone(&self.active_connections);

        tracing::debug!("Using tokio runtime handle for SSH tunnel spawn");
        let runtime_handle = self.runtime_handle.clone();
        let runtime_handle_inner = runtime_handle.clone();
        let task = runtime_handle.spawn(async move {
            // Handle incoming connections
            loop {
                let accept_result = listener.accept().await;

                // Check if tunnel is still running after accept
                if !is_running.lock().ok().map(|r| *r).unwrap_or(false) {
                    break;
                }

                match accept_result {
                    Ok((mut local_socket, _)) => {
                        let remote_host = remote_host.clone();
                        let remote_port = remote_port;
                        let local_port = local_port;
                        let session = Arc::clone(&session);
                        let active_connections = Arc::clone(&active_connections);
                        let is_running_check = Arc::clone(&is_running);
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
                        if let Ok(mut connections) = active_connections.lock() {
                            connections.insert(connection_id.clone());
                        }

                        // Handle connection in background task
                        runtime_handle.spawn(async move {
                            tracing::debug!("Processing connection {}", connection_id.0);

                            // Check if tunnel is still running
                            if !is_running_check.lock().ok().map(|r| *r).unwrap_or(false) {
                                tracing::debug!("Tunnel no longer running, rejecting connection {}", connection_id.0);
                                return;
                            }

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
                            if let Ok(mut connections) = active_connections.lock() {
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
        if let Ok(mut running) = self.is_running.lock() {
            *running = false;
        }
        if let Ok(mut status) = self.status.lock() {
            *status = TunnelStatus::Disconnected;
        }
        self.session = None;
        self.jump_sessions.clear();
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
        if let Ok(mut running) = self.is_running.lock() {
            *running = false;
        }
        // Abort the task to stop the TCP forwarding
        if let Some(task) = self.tunnel_task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tunnel(host: &str, port: u16, user: &str, jumps: Option<&str>) -> SshTunnelConfig {
        SshTunnelConfig {
            ssh_host: host.to_string(),
            ssh_port: port,
            ssh_user: user.to_string(),
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            jump_hosts: jumps.map(str::to_string),
            remote_host: "db".to_string(),
            remote_port: 5432,
            local_port: 0,
        }
    }

    fn hop(user: &str, host: &str, port: u16) -> String {
        format!("{user}@{host}:{port}")
    }

    #[test]
    fn hops_come_from_the_ssh_config() {
        let config = SshConfigFile::parse(
            "Host prod\n  HostName prod.internal\n  User deploy\n  Port 2222\n  ProxyJump gw1,admin@gw2:2200\n\
             Host gw1\n  HostName gw1.example.com\n  User jumper\n  IdentityFile /keys/gw\n",
            Path::new("/nowhere"),
        );
        let hops = plan_hops(&tunnel("prod", 22, "", None), &config, "me").unwrap();
        assert_eq!(
            hops.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                hop("jumper", "gw1.example.com", 22),
                hop("admin", "gw2", 2200),
                hop("deploy", "prod.internal", 2222),
            ]
        );
        assert_eq!(hops[0].identity_files, [PathBuf::from("/keys/gw")]);
    }

    #[test]
    fn the_dialog_wins_over_the_config() {
        let config = SshConfigFile::parse(
            "Host prod\n  User deploy\n  Port 2222\n  ProxyJump gw1\n",
            Path::new("/nowhere"),
        );
        // A user, a port other than 22 and jump hosts typed in the dialog
        // take precedence; `none` turns the config's ProxyJump off.
        let hops = plan_hops(
            &tunnel("prod", 2022, "alice", Some("bob@bastion")),
            &config,
            "me",
        )
        .unwrap();
        assert_eq!(
            hops.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [hop("bob", "bastion", 22), hop("alice", "prod", 2022)]
        );
        let hops = plan_hops(&tunnel("prod", 22, "", Some("none")), &config, "me").unwrap();
        assert_eq!(
            hops.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [hop("deploy", "prod", 2222)]
        );
        // Without any config the local user and port 22 are used.
        let hops = plan_hops(
            &tunnel("plain", 22, "", None),
            &SshConfigFile::default(),
            "me",
        )
        .unwrap();
        assert_eq!(hops[0].to_string(), hop("me", "plain", 22));
        assert!(plan_hops(&tunnel("x", 22, "", Some("a@:1")), &config, "me").is_err());
    }
}
