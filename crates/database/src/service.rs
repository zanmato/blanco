//! Database service implementation with connection and SSH tunnel management

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory, DatabaseService as DatabaseServiceTrait};
use gpui::Global;
use smol::channel;
use smol::lock::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use crate::connection_config::{
    ConnectionConfig, ConnectionSecretStore, DatabaseType, NoSecretStore, SecretKind,
};
use crate::factories::{
    ClickhouseConnectionFactory, MssqlConnectionFactory, MysqlConnectionFactory,
    PostgresConnectionFactory, RedisConnectionFactory, SqliteConnectionFactory,
};
use crate::secret_prompt::{
    rejected_secret, NoSecretPrompt, SecretPrompt, SecretPromptCancelled, SecretPromptRequest,
};
use crate::ssh_tunnel::{SshTunnel, SshTunnelConfig, TunnelInfo};
use crate::tokio_connection::TokioConnection;

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
    /// Closed on purpose after its idle timeout, rather than lost.
    pub idle: bool,
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

/// How an external client tool reaches one database. Holds the password, so
/// it is built right before the tool starts and dropped with it.
#[derive(Clone, Debug)]
pub struct ClientToolEndpoint {
    pub config: ConnectionConfig,
    pub database: String,
    /// Port of the SSH tunnel on `127.0.0.1`, `None` to connect to
    /// `config.host` and `config.port` directly.
    pub tunnel_port: Option<u16>,
}

/// Main database service that manages connections and SSH tunnels internally
pub struct DatabaseService {
    // Connection storage
    connection_configs: Arc<RwLock<HashMap<DatabaseConfigId, ConnectionConfig>>>,
    active_connections: Arc<RwLock<HashMap<ConnectionId, Arc<dyn Connection>>>>,
    // When each active connection was last handed out or seen busy
    last_used: Arc<StdMutex<HashMap<ConnectionId, Instant>>>,

    // Connection factories
    connection_factories: Arc<HashMap<DatabaseType, Arc<dyn ConnectionFactory>>>,

    // Internal SSH management (private)
    ssh_tunnels: Arc<RwLock<HashMap<DatabaseConfigId, Arc<StdMutex<SshTunnel>>>>>,
    tunnel_connections: Arc<RwLock<HashMap<ConnectionId, DatabaseConfigId>>>,

    // Channel sender for dispatching actions from any context (shared via Arc)
    action_sender: SharedActionSender,

    // Resolves a connection's secrets the moment it is opened
    secret_store: Arc<dyn ConnectionSecretStore>,

    // Asks the user for secrets that are not stored
    secret_prompt: Arc<dyn SecretPrompt>,

    // Secrets the user typed at a prompt, kept for reconnects until the
    // connection is closed or edited. The lock is held across a prompt so
    // connections opening together ask once and share the answer.
    session_secrets: Arc<smol::lock::Mutex<HashMap<(DatabaseConfigId, SecretKind), String>>>,

    // Dependencies
    runtime_handle: tokio::runtime::Handle,
}

impl DatabaseService {
    pub fn new(runtime_handle: tokio::runtime::Handle) -> Self {
        let factories: HashMap<DatabaseType, Arc<dyn ConnectionFactory>> = HashMap::from([
            (
                DatabaseType::SQLite,
                Arc::new(SqliteConnectionFactory) as Arc<dyn ConnectionFactory>,
            ),
            (
                DatabaseType::PostgreSQL,
                Arc::new(PostgresConnectionFactory::new()),
            ),
            (DatabaseType::MySQL, Arc::new(MysqlConnectionFactory::new())),
            (
                DatabaseType::ClickHouse,
                Arc::new(ClickhouseConnectionFactory::new()),
            ),
            (DatabaseType::MsSql, Arc::new(MssqlConnectionFactory::new())),
            (DatabaseType::Redis, Arc::new(RedisConnectionFactory::new())),
        ]);

        Self {
            connection_configs: Arc::new(RwLock::new(HashMap::new())),
            active_connections: Arc::new(RwLock::new(HashMap::new())),
            last_used: Arc::new(StdMutex::new(HashMap::new())),
            connection_factories: Arc::new(factories),
            ssh_tunnels: Arc::new(RwLock::new(HashMap::new())),
            tunnel_connections: Arc::new(RwLock::new(HashMap::new())),
            action_sender: Arc::new(StdMutex::new(None)),
            secret_store: Arc::new(NoSecretStore),
            secret_prompt: Arc::new(NoSecretPrompt),
            session_secrets: Arc::new(smol::lock::Mutex::new(HashMap::new())),
            runtime_handle,
        }
    }

    pub fn set_secret_store(&mut self, store: Arc<dyn ConnectionSecretStore>) {
        self.secret_store = store;
    }

    pub fn secret_store(&self) -> Arc<dyn ConnectionSecretStore> {
        Arc::clone(&self.secret_store)
    }

    pub fn set_secret_prompt(&mut self, prompt: Arc<dyn SecretPrompt>) {
        self.secret_prompt = prompt;
    }

    /// Drop the secrets typed for `config_id` this session, so the next
    /// connect asks again.
    pub async fn forget_session_secrets(&self, config_id: DatabaseConfigId) {
        self.session_secrets
            .lock()
            .await
            .retain(|(id, _), _| *id != config_id);
    }

    /// Fill in the secrets `config` is missing, one store read after another
    /// so a burst of connections never floods the backing keyring.
    #[cfg(test)]
    async fn resolve_secrets(&self, config: &mut ConnectionConfig) -> Result<()> {
        self.resolve_secrets_asking(config, None).await.map(|_| ())
    }

    /// Fill in the secrets `config` is missing. Secrets the connection asks
    /// for come from this session's answers or a prompt, the rest from the
    /// store. `rejected` names a secret the server just refused: it is asked
    /// for again unless another connection has remembered a newer answer
    /// meanwhile. Returns the secrets that came from the user.
    async fn resolve_secrets_asking(
        &self,
        config: &mut ConnectionConfig,
        rejected: Option<Rejected>,
    ) -> Result<Vec<SecretKind>> {
        let rejected_kind = rejected.as_ref().map(|rejected| rejected.kind);
        let asked: Vec<SecretKind> = SecretKind::ALL
            .into_iter()
            .filter(|kind| {
                config.uses_secret(*kind)
                    && (config.asks_for(*kind) || rejected_kind == Some(*kind))
            })
            .collect();
        let mut from_user = Vec::new();
        for kind in config.missing_secret_kinds() {
            if asked.contains(&kind) {
                continue;
            }
            // An answer typed earlier when the server asked for a secret the
            // connection lacks stands in for the store until forgotten.
            let remembered = self
                .session_secrets
                .lock()
                .await
                .get(&(config.id, kind))
                .cloned();
            if remembered.is_some() {
                config.set_secret(kind, remembered);
                from_user.push(kind);
                continue;
            }
            let value = self.secret_store.read(config.id, kind).await?;
            config.set_secret(kind, value);
        }

        for kind in asked {
            let mut answers = self.session_secrets.lock().await;
            let key = (config.id, kind);
            if let Some(rejected) = rejected.as_ref().filter(|rejected| rejected.kind == kind) {
                if answers.get(&key) == rejected.value.as_ref() {
                    answers.remove(&key);
                }
            }
            // A remembered answer is reused, including one another connection
            // stored while this one waited for the lock.
            if let Some(value) = answers.get(&key) {
                config.set_secret(kind, Some(value.clone()));
                from_user.push(kind);
                continue;
            }
            let retry_reason = rejected
                .as_ref()
                .filter(|rejected| rejected.kind == kind)
                .and_then(|rejected| rejected.reason.clone());
            let request = SecretPromptRequest::new(config, kind, retry_reason);
            match self.secret_prompt.ask(request).await? {
                Some(value) => {
                    answers.insert(key, value.clone());
                    config.set_secret(kind, Some(value));
                    from_user.push(kind);
                }
                None => return Err(SecretPromptCancelled { kind }.into()),
            }
        }
        Ok(from_user)
    }

    /// Set the action sender (called via cx.update_global from app initialization)
    pub fn set_action_sender(&mut self, sender: channel::Sender<DatabaseServiceMessage>) {
        if let Ok(mut sender_slot) = self.action_sender.lock() {
            *sender_slot = Some(sender);
        }
    }

    /// Add a connection configuration. Replacing a connection's config also forgets the secrets typed for it,
    /// since they may belong to the old user or host.
    pub async fn add_connection_config(&self, config: ConnectionConfig) {
        self.forget_session_secrets(config.id).await;
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
                    tunnel_mutex
                        .lock()
                        .ok()
                        .map(|tunnel| tunnel.is_healthy_sync())
                        .unwrap_or(false)
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
                    self.touch(&connection_id);
                    return Ok(Arc::clone(existing_conn));
                }
            }
        } else {
            // Non-SSH connection, use simple caching
            let connections = self.active_connections.read().await;
            if let Some(existing_conn) = connections.get(&connection_id) {
                self.touch(&connection_id);
                return Ok(Arc::clone(existing_conn));
            }
        }

        // Secrets are resolved only now, after the cache misses, and live in
        // a local copy of the config for the duration of the connect. A
        // refused secret that the user can supply is asked for again, a few
        // times at most.
        const ATTEMPTS: usize = 3;
        let mut rejected = None;
        for attempt in 1..=ATTEMPTS {
            let mut attempt_config = config.clone();
            let from_user = self
                .resolve_secrets_asking(&mut attempt_config, rejected.take())
                .await?;
            match self
                .open_connection(&attempt_config, database, &from_user)
                .await
            {
                Ok(connection) => return Ok(connection),
                Err(error) => {
                    let refused = self
                        .secret_prompt
                        .is_interactive()
                        .then(|| rejected_secret(&attempt_config, &from_user, &error))
                        .flatten();
                    let Some(kind) = refused else {
                        return Err(error);
                    };
                    let typed = from_user.contains(&kind);
                    if attempt == ATTEMPTS {
                        // Do not keep an answer the server refused for the
                        // next reconnect.
                        if typed {
                            let key = (config_id, kind);
                            let mut answers = self.session_secrets.lock().await;
                            if answers.get(&key).map(String::as_str) == attempt_config.secret(kind)
                            {
                                answers.remove(&key);
                            }
                        }
                        return Err(error);
                    }
                    tracing::info!(
                        "Connection {} refused its {}; asking again",
                        config_id,
                        kind.label()
                    );
                    // A missing secret the server wants is simply asked
                    // for; a typed one it refused says why.
                    rejected = Some(Rejected {
                        kind,
                        value: attempt_config.secret(kind).map(str::to_string),
                        reason: typed.then(|| format!("{error:#}")),
                    });
                }
            }
        }
        unreachable!("the last attempt returns")
    }

    /// Open a connection with secrets already resolved and cache it.
    /// `from_user` lists the secrets typed at a prompt; when the password is
    /// one of them, or there is none, the server is pinged so a refusal
    /// surfaces now, while it can still be asked for, rather than at the
    /// first query.
    async fn open_connection(
        &self,
        config: &ConnectionConfig,
        database: Option<&str>,
        from_user: &[SecretKind],
    ) -> Result<Arc<dyn Connection>> {
        let config_id = config.id;
        let connection_id = (config_id, database.unwrap_or("default").to_string());

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
            .get(&config.db_type)
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", config.db_type)
            })?
            .clone();

        // Drivers (sqlx with `runtime-tokio`, reqwest, ...) require a tokio
        // reactor on the polling thread, so we run the factory's `connect`
        // call on the shared tokio runtime and wrap the resulting connection
        // in `TokioConnection` so subsequent method calls also hop runtimes.
        let connection_string_owned = connection_string.clone();
        let runtime_handle = self.runtime_handle.clone();
        let connect_timeout = blanco_core::connect_timeout();
        let inner_conn = runtime_handle
            .clone()
            .spawn(async move {
                tokio::time::timeout(
                    connect_timeout,
                    factory.create_connection(&connection_string_owned),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Database connection timed out after {}s",
                        connect_timeout.as_secs()
                    )
                })?
            })
            .await
            .map_err(|e| anyhow::anyhow!("tokio task join failed: {}", e))??;
        let inner_arc: Arc<dyn Connection> = Arc::from(inner_conn);
        let conn_arc: Arc<dyn Connection> = Arc::new(TokioConnection::new(
            inner_arc,
            runtime_handle,
            config.read_only,
        ));

        let password_unverified = config.uses_secret(SecretKind::Password)
            && (from_user.contains(&SecretKind::Password)
                || config.password.as_deref().is_none_or(str::is_empty));
        if password_unverified && self.secret_prompt.is_interactive() {
            if let Err(error) = conn_arc.ping().await {
                // Only a refused password stops the connect here; anything
                // else surfaces at the first query as it did before.
                let message = format!("{error:#}").to_lowercase();
                if crate::secret_prompt::is_authentication_failure(&message) {
                    return Err(error);
                }
                tracing::debug!("Ping after connect failed: {error:#}");
            }
        }

        // Store in active_connections
        {
            let mut connections = self.active_connections.write().await;
            connections.insert(connection_id.clone(), conn_arc.clone());
        }
        self.touch(&connection_id);

        // Track which connections use SSH tunnels
        if config.requires_ssh_tunnel() {
            let mut tunnel_connections = self.tunnel_connections.write().await;
            tunnel_connections.insert(connection_id.clone(), config_id);
        }

        // Send action message for newly created connections
        let sender_opt = self.action_sender.lock().ok().and_then(|s| s.clone());

        if let Some(sender) = sender_opt {
            sender
                .send(DatabaseServiceMessage::Connected(
                    DatabaseConnectedMessage {
                        connection_id: config_id,
                        database_name: database.unwrap_or("default").to_string(),
                    },
                ))
                .await?;
        }

        tracing::info!(
            "Successfully created and stored connection: {:?}",
            connection_id
        );
        Ok(conn_arc)
    }

    /// Where an external client tool such as `pg_dump` reaches `database`:
    /// the connection's settings with its password, and the local port of its
    /// SSH tunnel when it has one. Opening the connection first starts the
    /// tunnel and keeps it registered, so it outlives the tool's run unless the
    /// user disconnects.
    pub async fn client_tool_endpoint(
        &self,
        config_id: DatabaseConfigId,
        database: &str,
    ) -> Result<ClientToolEndpoint> {
        self.get_or_create_connection(config_id, Some(database))
            .await?;
        let mut config = self.get_connection_config(config_id).await.ok_or_else(|| {
            anyhow::anyhow!("Connection configuration not found for ID: {}", config_id)
        })?;
        // A secret that is asked for was just answered for the connection
        // above and is remembered for the session, so this does not ask again.
        self.resolve_secrets_asking(&mut config, None).await?;
        let tunnel_port = if config.requires_ssh_tunnel() {
            let tunnels = self.ssh_tunnels.read().await;
            let tunnel = tunnels
                .get(&config_id)
                .ok_or_else(|| anyhow::anyhow!("The SSH tunnel for this connection is closed"))?
                .lock()
                .map_err(|error| anyhow::anyhow!("Failed to lock SSH tunnel mutex: {}", error))?;
            Some(tunnel.get_info().local_port)
        } else {
            None
        };
        Ok(ClientToolEndpoint {
            config,
            database: database.to_string(),
            tunnel_port,
        })
    }

    /// Disconnect a connection
    pub async fn disconnect(
        &self,
        config_id: DatabaseConfigId,
        database: Option<&str>,
    ) -> Result<()> {
        tracing::debug!(?config_id, ?database, "disconnecting connection");
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
        if let Ok(mut last_used) = self.last_used.lock() {
            for connection_id in &disconnected_connections {
                last_used.remove(connection_id);
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

    /// Record that `connection_id` is in use now.
    fn touch(&self, connection_id: &ConnectionId) {
        if let Ok(mut last_used) = self.last_used.lock() {
            last_used.insert(connection_id.clone(), Instant::now());
        }
    }

    /// Close every connection that has been idle for longer than its timeout,
    /// with its SSH tunnel, and report each as disconnected so the next action
    /// reconnects. `default_timeout` applies to connections without their own.
    ///
    /// A connection counts as busy while anything besides the cache holds it,
    /// which a running query or export does, and is never closed then.
    /// Connections listed in `keep` (for example ones with uncommitted grid
    /// edits) are left open too. Returns the closed connection ids.
    pub async fn disconnect_idle_connections(
        &self,
        default_timeout: Option<Duration>,
        keep: &HashSet<DatabaseConfigId>,
    ) -> Vec<DatabaseConfigId> {
        let now = Instant::now();
        let idle = {
            let connections = self.active_connections.read().await;
            let configs = self.connection_configs.read().await;
            let Ok(mut last_used) = self.last_used.lock() else {
                return Vec::new();
            };
            // The most recent use of any database opened through each
            // connection, or `None` once one of them is busy.
            let mut latest: HashMap<DatabaseConfigId, Option<Instant>> = HashMap::new();
            for (connection_id, connection) in connections.iter() {
                let busy = Arc::strong_count(connection) > 1;
                if busy {
                    last_used.insert(connection_id.clone(), now);
                }
                let used = last_used.get(connection_id).copied().unwrap_or(now);
                let entry = latest.entry(connection_id.0).or_insert(Some(used));
                *entry = match (*entry, busy) {
                    (_, true) | (None, _) => None,
                    (Some(previous), false) => Some(previous.max(used)),
                };
            }
            latest
                .into_iter()
                .filter(|(config_id, _)| !keep.contains(config_id))
                .filter_map(|(config_id, used)| {
                    let timeout = configs.get(&config_id)?.idle_timeout(default_timeout)?;
                    (now.duration_since(used?) >= timeout).then_some(config_id)
                })
                .collect::<Vec<_>>()
        };

        for &config_id in &idle {
            tracing::info!("Closing connection {config_id} after its idle timeout");
            if let Err(error) = self.disconnect(config_id, None).await {
                tracing::error!("Failed to close idle connection {config_id}: {error}");
                continue;
            }
            let sender = self.action_sender.lock().ok().and_then(|s| s.clone());
            if let Some(sender) = sender {
                let name = self
                    .get_connection_config(config_id)
                    .await
                    .map_or_else(|| config_id.to_string(), |config| config.name);
                let message = DatabaseServiceMessage::Disconnected(DatabaseDisconnectedMessage {
                    connection_id: config_id,
                    database_name: name,
                    idle: true,
                });
                if let Err(error) = sender.send(message).await {
                    tracing::error!("Failed to send disconnect notification: {error}");
                }
            }
        }
        idle
    }

    /// Get active connections reference
    pub async fn get_active_connections(
        &self,
    ) -> std::collections::HashMap<ConnectionId, Arc<dyn Connection>> {
        self.active_connections.read().await.clone()
    }

    /// Inspect an error returned by a query/script. If it looks like the
    /// underlying connection (or SSH tunnel) has dropped, evict the cached
    /// connection so the next call reconnects, and notify the UI so the sidebar
    /// can mark it disconnected instead of leaving a stale "connected" state.
    async fn note_possible_disconnect(
        &self,
        config_id: DatabaseConfigId,
        database: Option<&str>,
        error: &anyhow::Error,
    ) {
        if !blanco_core::is_connection_lost(error) {
            return;
        }

        tracing::info!(
            "Connection {} appears to have dropped ({}); evicting and marking disconnected",
            config_id,
            error
        );

        // Drop the whole connection: a broken socket or dead SSH tunnel affects
        // every database opened through it, not just the one that errored.
        if let Err(e) = self.disconnect(config_id, None).await {
            tracing::error!("Failed to evict dropped connection {}: {}", config_id, e);
        }

        let sender_opt = self.action_sender.lock().ok().and_then(|s| s.clone());
        if let Some(sender) = sender_opt {
            let message = DatabaseServiceMessage::Disconnected(DatabaseDisconnectedMessage {
                connection_id: config_id,
                database_name: database.unwrap_or("default").to_string(),
                idle: false,
            });
            if let Err(e) = sender.send(message).await {
                tracing::error!("Failed to send disconnect notification: {}", e);
            }
        }
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
                let tunnel = tunnel_mutex
                    .lock()
                    .map_err(|e| anyhow::anyhow!("Failed to lock SSH tunnel mutex: {}", e))?;
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
        let ssh_host = config
            .ssh_host
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SSH host is required to build an SSH tunnel"))?
            .clone();
        let ssh_user = config
            .ssh_user
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SSH user is required to build an SSH tunnel"))?
            .clone();
        let ssh_config = SshTunnelConfig {
            ssh_host,
            ssh_port: config.ssh_port(),
            ssh_user,
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
        let connect_timeout = blanco_core::connect_timeout();
        let tunnel = runtime_handle
            .spawn(async move {
                tokio::time::timeout(connect_timeout, async move {
                    let mut tunnel =
                        SshTunnel::create(ssh_config, runtime_handle_inner.clone()).await?;
                    tunnel.connect().await?;
                    Result::<SshTunnel, anyhow::Error>::Ok(tunnel)
                })
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "SSH tunnel connection timed out after {}s",
                        connect_timeout.as_secs()
                    )
                })?
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
        // A test asks for the secrets the connection asks for, every time; the
        // answers are not remembered, since the connection is not saved yet.
        let mut asked = config.clone();
        for kind in SecretKind::ALL {
            if asked.uses_secret(kind) && asked.asks_for(kind) && asked.secret(kind).is_none() {
                let request = SecretPromptRequest::new(&asked, kind, None);
                match self.secret_prompt.ask(request).await? {
                    Some(value) => asked.set_secret(kind, Some(value)),
                    None => return Err(SecretPromptCancelled { kind }.into()),
                }
            }
        }
        let config = &asked;
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
            let connect_timeout = blanco_core::connect_timeout();
            let tunnel = runtime_handle
                .spawn(async move {
                    tokio::time::timeout(connect_timeout, async move {
                        let mut tunnel =
                            SshTunnel::create(ssh_config, runtime_handle_inner.clone()).await?;
                        tunnel.connect().await?;
                        Result::<SshTunnel, anyhow::Error>::Ok(tunnel)
                    })
                    .await
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "SSH tunnel connection timed out after {}s",
                            connect_timeout.as_secs()
                        )
                    })?
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
            .get(&config.db_type)
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", config.db_type)
            })?
            .clone();

        // The factory's `connect` call needs a tokio reactor; run it on the
        // shared runtime. For lazy backends `create_connection` only parses the
        // connection string, so we follow it with `ping` (a real round-trip) to
        // actually validate credentials and reachability.
        let connect_timeout = blanco_core::connect_timeout();
        let result = self
            .runtime_handle
            .spawn(async move {
                let connection = tokio::time::timeout(
                    connect_timeout,
                    factory.create_connection(&connection_string),
                )
                .await
                .map_err(|_| {
                    anyhow::anyhow!(
                        "Database connection timed out after {}s",
                        connect_timeout.as_secs()
                    )
                })??;
                connection.ping().await
            })
            .await
            .map_err(|e| anyhow::anyhow!("tokio task join failed: {}", e))?;

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

    async fn execute_query(
        &self,
        connection_id: i64,
        database: Option<&str>,
        sql: &str,
    ) -> Result<blanco_core::QueryResult> {
        let connection = self
            .get_or_create_connection(connection_id, database)
            .await?;
        let result = connection.execute_query(sql, database, None).await;
        if let Err(error) = &result {
            self.note_possible_disconnect(connection_id, database, error)
                .await;
        }
        result
    }

    async fn execute_script(
        &self,
        connection_id: i64,
        database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<blanco_core::QueryResult>> {
        let connection = self
            .get_or_create_connection(connection_id, database)
            .await?;
        let result = connection.execute_script(sql, database).await;
        if let Err(error) = &result {
            self.note_possible_disconnect(connection_id, database, error)
                .await;
        }
        result
    }

    async fn execute_query_with_params(
        &self,
        connection_id: i64,
        database: Option<&str>,
        sql: &str,
        parameters: &[String],
    ) -> Result<blanco_core::QueryResult> {
        let connection = self
            .get_or_create_connection(connection_id, database)
            .await?;
        let result = connection
            .execute_query(sql, database, Some(parameters))
            .await;
        if let Err(error) = &result {
            self.note_possible_disconnect(connection_id, database, error)
                .await;
        }
        result
    }

    async fn execute_operations_transactional(
        &self,
        connection_id: i64,
        database: Option<&str>,
        operations: &[blanco_core::WriteOperation],
    ) -> Result<blanco_core::BatchOutcome, blanco_core::BatchFailure> {
        let connection = self
            .get_or_create_connection(connection_id, database)
            .await
            .map_err(blanco_core::BatchFailure::atomic)?;
        let result = connection
            .execute_operations_transactional(operations, database)
            .await;
        if let Err(failure) = &result {
            self.note_possible_disconnect(connection_id, database, &failure.error)
                .await;
        }
        result
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
                .map(|config| config.db_type.to_string())
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

/// A secret a connect attempt was refused for.
struct Rejected {
    kind: SecretKind,
    /// The value that was refused, so a newer remembered answer is not
    /// mistaken for it.
    value: Option<String>,
    /// What to tell the user, when the refused value was theirs.
    reason: Option<String>,
}

impl Clone for DatabaseService {
    fn clone(&self) -> Self {
        Self {
            connection_configs: Arc::clone(&self.connection_configs),
            active_connections: Arc::clone(&self.active_connections),
            last_used: Arc::clone(&self.last_used),
            connection_factories: Arc::clone(&self.connection_factories),
            ssh_tunnels: Arc::clone(&self.ssh_tunnels),
            tunnel_connections: Arc::clone(&self.tunnel_connections),
            action_sender: Arc::clone(&self.action_sender),
            secret_store: Arc::clone(&self.secret_store),
            secret_prompt: Arc::clone(&self.secret_prompt),
            session_secrets: Arc::clone(&self.session_secrets),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_config::{ConnectionConfig, SecretKind};

    struct MapSecretStore(HashMap<(i64, SecretKind), String>);

    #[async_trait]
    impl ConnectionSecretStore for MapSecretStore {
        async fn read(&self, connection_id: i64, kind: SecretKind) -> Result<Option<String>> {
            Ok(self.0.get(&(connection_id, kind)).cloned())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolve_secrets_fills_only_missing_kinds_from_the_store() {
        let mut service = DatabaseService::new(tokio::runtime::Handle::current());
        service.set_secret_store(Arc::new(MapSecretStore(HashMap::from([
            ((1, SecretKind::Password), "db-secret".to_string()),
            ((1, SecretKind::SshPassword), "ssh-secret".to_string()),
        ]))));

        let mut config = ConnectionConfig::new(
            1,
            "pg".into(),
            DatabaseType::PostgreSQL,
            "localhost".into(),
            5432,
            "app".into(),
            "user".into(),
            Some("inline".into()),
        )
        .without_secrets();
        assert_eq!(config.missing_secret_kinds(), vec![SecretKind::Password]);

        service.resolve_secrets(&mut config).await.expect("resolve");
        assert_eq!(config.password.as_deref(), Some("db-secret"));
        assert_eq!(config.ssh_password, None);

        let mut tunnelled = config.clone().with_ssh_config(
            "bastion".into(),
            "ssh-user".into(),
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            tunnelled.missing_secret_kinds(),
            vec![SecretKind::SshPassword, SecretKind::SshPrivateKeyPassword]
        );
        service
            .resolve_secrets(&mut tunnelled)
            .await
            .expect("resolve");
        assert_eq!(tunnelled.ssh_password.as_deref(), Some("ssh-secret"));
        assert_eq!(tunnelled.ssh_private_key_password, None);

        let dir = tempfile::tempdir().expect("temp dir");
        assert!(sqlite_config(2, &dir.path().join("x.db"))
            .missing_secret_kinds()
            .is_empty());
    }

    fn sqlite_config(id: i64, path: &std::path::Path) -> ConnectionConfig {
        ConnectionConfig::new_sqlite(
            id,
            format!("test-{id}"),
            path.to_string_lossy().into_owned(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_connection_config_roundtrip() {
        let service = DatabaseService::new(tokio::runtime::Handle::current());
        let dir = tempfile::tempdir().expect("temp dir");
        let config = sqlite_config(1, &dir.path().join("roundtrip.db"));

        assert!(service.get_connection_config(1).await.is_none());
        service.add_connection_config(config.clone()).await;

        let fetched = service
            .get_connection_config(1)
            .await
            .expect("config present");
        assert_eq!(fetched.id, config.id);
        assert_eq!(fetched.name, config.name);
        assert_eq!(fetched.path, config.path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_get_or_create_connection_caches_by_key() {
        let service = DatabaseService::new(tokio::runtime::Handle::current());
        let dir = tempfile::tempdir().expect("temp dir");
        service
            .add_connection_config(sqlite_config(1, &dir.path().join("cache.db")))
            .await;

        let first = service
            .get_or_create_connection(1, None)
            .await
            .expect("first connection");
        let second = service
            .get_or_create_connection(1, None)
            .await
            .expect("second connection");

        assert!(
            Arc::ptr_eq(&first, &second),
            "same (config_id, database) must return the cached connection"
        );
        assert_eq!(service.get_active_connections().await.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_get_or_create_connection_distinct_databases() {
        let service = DatabaseService::new(tokio::runtime::Handle::current());
        let dir = tempfile::tempdir().expect("temp dir");
        service
            .add_connection_config(sqlite_config(1, &dir.path().join("distinct.db")))
            .await;

        let default_db = service
            .get_or_create_connection(1, None)
            .await
            .expect("default connection");
        let named_db = service
            .get_or_create_connection(1, Some("other"))
            .await
            .expect("named connection");

        assert!(
            !Arc::ptr_eq(&default_db, &named_db),
            "different database names must key distinct cache entries"
        );
        assert_eq!(service.get_active_connections().await.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_disconnect_evicts_cached_connection() {
        let service = DatabaseService::new(tokio::runtime::Handle::current());
        let dir = tempfile::tempdir().expect("temp dir");
        service
            .add_connection_config(sqlite_config(1, &dir.path().join("evict.db")))
            .await;

        let first = service
            .get_or_create_connection(1, None)
            .await
            .expect("first connection");
        service.disconnect(1, None).await.expect("disconnect");
        assert!(service.get_active_connections().await.is_empty());

        let recreated = service
            .get_or_create_connection(1, None)
            .await
            .expect("recreated connection");
        assert!(
            !Arc::ptr_eq(&first, &recreated),
            "after disconnect a fresh connection must be created, not the evicted one"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_disconnect_idle_connections() {
        let service = DatabaseService::new(tokio::runtime::Handle::current());
        let dir = tempfile::tempdir().expect("temp dir");
        service
            .add_connection_config(sqlite_config(1, &dir.path().join("idle.db")))
            .await;
        service
            .add_connection_config(
                sqlite_config(2, &dir.path().join("never.db")).with_idle_timeout_minutes(Some(0)),
            )
            .await;
        let immediately = Some(Duration::ZERO);
        let nothing_kept = HashSet::new();

        // No default timeout and no override: nothing closes.
        let held = service.get_or_create_connection(1, None).await.unwrap();
        service.get_or_create_connection(2, None).await.unwrap();
        assert!(service
            .disconnect_idle_connections(None, &nothing_kept)
            .await
            .is_empty());

        // A connection in use, or one with pending edits, stays open.
        assert!(
            service
                .disconnect_idle_connections(immediately, &nothing_kept)
                .await
                .is_empty(),
            "a held connection is busy, and connection 2 opted out"
        );
        drop(held);
        assert!(service
            .disconnect_idle_connections(immediately, &HashSet::from([1]))
            .await
            .is_empty());

        assert_eq!(
            service
                .disconnect_idle_connections(immediately, &nothing_kept)
                .await,
            [1]
        );
        let remaining = service.get_active_connections().await;
        assert_eq!(remaining.keys().map(|(id, _)| *id).collect::<Vec<_>>(), [2]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_get_or_create_connection_unknown_config_errors() {
        let service = DatabaseService::new(tokio::runtime::Handle::current());
        let result = service.get_or_create_connection(999, None).await;
        assert!(result.is_err(), "unknown config id must error, not panic");
    }

    /// Regression test: every connection handed out by the service is wrapped
    /// in `TokioConnection`, which must forward
    /// `execute_operations_transactional` to the driver. When the wrapper
    /// fell back to the trait default, batches ran sequentially and a
    /// mid-batch failure left earlier writes applied.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_transactional_batch_rolls_back_through_service() {
        let service = DatabaseService::new(tokio::runtime::Handle::current());
        let dir = tempfile::tempdir().expect("temp dir");
        service
            .add_connection_config(sqlite_config(1, &dir.path().join("atomic.db")))
            .await;
        let connection = service
            .get_or_create_connection(1, None)
            .await
            .expect("connection");

        connection
            .execute_write(
                "CREATE TABLE account (id INTEGER PRIMARY KEY, balance INTEGER NOT NULL CHECK (balance >= 0))",
                None,
                &[],
            )
            .await
            .expect("create table");
        connection
            .execute_write(
                "INSERT INTO account (id, balance) VALUES (1, 100)",
                None,
                &[],
            )
            .await
            .expect("seed row");

        let operations: Vec<blanco_core::WriteOperation> = vec![
            "UPDATE account SET balance = 50 WHERE id = 1".into(),
            "UPDATE account SET balance = -10 WHERE id = 1".into(),
        ];
        let failure = service
            .execute_operations_transactional(1, None, &operations)
            .await
            .expect_err("second UPDATE violates the CHECK constraint");
        assert_eq!(
            failure.applied, 0,
            "batch must be atomic through the service"
        );

        let result = connection
            .execute_query("SELECT balance FROM account WHERE id = 1", None, None)
            .await
            .expect("select balance");
        let balance = result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|cell| cell.as_deref())
            .expect("balance cell");
        assert_eq!(balance, "100", "first UPDATE must have been rolled back");
    }

    #[test]
    fn test_connection_lost_marker_drives_eviction_decision() {
        // A driver-tagged error (typed `ConnectionLost` in its chain, regardless
        // of the descriptive message wrapped around it) is treated as a drop.
        let tagged = anyhow::anyhow!("PostgreSQL query execution failed")
            .context(blanco_core::ConnectionLost);
        assert!(blanco_core::is_connection_lost(&tagged));

        // An ordinary query error carries no marker and must not evict.
        let ordinary = anyhow::anyhow!("syntax error at or near \"SELET\"");
        assert!(!blanco_core::is_connection_lost(&ordinary));
    }

    /// Answers prompts from a script and records what it was asked.
    struct ScriptedPrompt {
        answers: StdMutex<Vec<Option<String>>>,
        asked: StdMutex<Vec<SecretPromptRequest>>,
    }

    impl ScriptedPrompt {
        fn new(answers: &[Option<&str>]) -> Arc<Self> {
            Arc::new(Self {
                answers: StdMutex::new(
                    answers
                        .iter()
                        .rev()
                        .map(|a| a.map(str::to_string))
                        .collect(),
                ),
                asked: StdMutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<SecretPromptRequest> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SecretPrompt for ScriptedPrompt {
        async fn ask(&self, request: SecretPromptRequest) -> Result<Option<String>> {
            self.asked.lock().unwrap().push(request);
            Ok(self.answers.lock().unwrap().pop().flatten())
        }
    }

    fn asking_config() -> ConnectionConfig {
        ConnectionConfig::new(
            7,
            "prod".into(),
            DatabaseType::PostgreSQL,
            "db.example.com".into(),
            5432,
            "app".into(),
            "alice".into(),
            None,
        )
        .with_ask_secrets(vec![SecretKind::Password])
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn asked_secrets_skip_the_store_and_are_remembered_for_the_session() {
        let mut service = DatabaseService::new(tokio::runtime::Handle::current());
        service.set_secret_store(Arc::new(MapSecretStore(HashMap::from([(
            (7, SecretKind::Password),
            "stale-stored".to_string(),
        )]))));
        let prompt = ScriptedPrompt::new(&[Some("typed")]);
        service.set_secret_prompt(prompt.clone());

        let mut config = asking_config();
        let from_user = service
            .resolve_secrets_asking(&mut config, None)
            .await
            .expect("resolve");
        assert_eq!(from_user, vec![SecretKind::Password]);
        assert_eq!(config.password.as_deref(), Some("typed"));
        let asked = prompt.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].connection_name, "prod");
        assert_eq!(asked[0].account, "alice@db.example.com");

        // A reconnect reuses the answer without asking.
        let mut again = asking_config();
        service
            .resolve_secrets_asking(&mut again, None)
            .await
            .expect("resolve");
        assert_eq!(again.password.as_deref(), Some("typed"));
        assert_eq!(prompt.asked().len(), 1);

        // Forgetting, as Disconnect does, asks again; cancelling fails the
        // connect with a typed error.
        service.forget_session_secrets(7).await;
        let error = service
            .resolve_secrets_asking(&mut asking_config(), None)
            .await
            .expect_err("cancelled");
        assert!(error.downcast_ref::<SecretPromptCancelled>().is_some());
        assert_eq!(prompt.asked().len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rejected_secret_is_asked_again_with_the_reason() {
        let mut service = DatabaseService::new(tokio::runtime::Handle::current());
        let prompt = ScriptedPrompt::new(&[Some("first"), Some("second")]);
        service.set_secret_prompt(prompt.clone());

        let mut config = asking_config();
        service
            .resolve_secrets_asking(&mut config, None)
            .await
            .expect("resolve");
        let mut retry = asking_config();
        service
            .resolve_secrets_asking(
                &mut retry,
                Some(Rejected {
                    kind: SecretKind::Password,
                    value: Some("first".into()),
                    reason: Some("password authentication failed".into()),
                }),
            )
            .await
            .expect("resolve");
        assert_eq!(retry.password.as_deref(), Some("second"));
        let asked = prompt.asked();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].retry_reason, None);
        assert_eq!(
            asked[1].retry_reason.as_deref(),
            Some("password authentication failed")
        );
    }
}
