use crate::app_database::AppDatabase;
use anyhow::Result;
use async_std::sync::RwLock;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory, ConnectionRegistry};
use gpui::{App, Global};
use postgres::{PgConnectionKey, PostgresConnection, connection::PostgresSshConfig};
use sqlite::{SqliteConnection, SqliteConnectionKey};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

// Import SSH tunnel types to resolve the compilation issues
use crate::ssh_tunnel::{SshTunnel, SshTunnelConfig};

/// Replace the database name in a PostgreSQL connection string
fn replace_database_in_postgres_connection_string(
    connection_string: &str,
    new_database: &str,
) -> String {
    if let Ok(mut url) = Url::parse(connection_string) {
        // Set the new database path
        url.set_path(&format!("/{}", new_database));
        url.to_string()
    } else {
        // If URL parsing fails, try a simple string replacement for postgresql:// URLs
        if connection_string.starts_with("postgresql://") {
            // Find the database part (last segment after the last /)
            if let Some(last_slash_pos) = connection_string.rfind('/') {
                let before_db = &connection_string[..last_slash_pos];
                format!("{}/{}", before_db, new_database)
            } else {
                // No slash found, append the database
                format!("{}/{}", connection_string, new_database)
            }
        } else {
            // Not a PostgreSQL URL, return as-is
            connection_string.to_string()
        }
    }
}

/// SQLite connection factory using the sqlite crate implementation
pub struct SqliteConnectionFactory;

#[async_trait]
impl ConnectionFactory for SqliteConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let key = SqliteConnectionKey::from_connection_string(connection_string)?;
        let mut conn = SqliteConnection::from_key(key);
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String> {
        Ok(connection_string.to_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "SQLite"
    }
}

/// PostgreSQL connection factory using the postgres crate implementation
pub struct PostgresConnectionFactory {}

impl PostgresConnectionFactory {
    pub fn new() -> Self {
        Self {}
    }

    /// Create PostgreSQL connection with optional SSH configuration
    async fn create_postgres_connection(
        &self,
        connection_string: &str,
        ssh_config: Option<PostgresSshConfig>,
    ) -> Result<Box<dyn Connection>> {
        // If SSH configuration is provided, reject connection (tunnels must be established separately)
        if ssh_config.is_some() {
            return Err(anyhow::anyhow!(
                "SSH configuration provided but SSH tunnels must be established separately. \
                Call establish_ssh_tunnel_with_gpui_context() first, then use tunneled connection strings."
            ));
        }

        // No SSH configuration - use connection string as-is
        let final_connection_string = connection_string.to_string();

        let key = PgConnectionKey::from_connection_string(&final_connection_string)?;

        // Create regular PostgresConnection (no SSH config needed now)
        let mut conn = PostgresConnection::from_key(key);

        Connection::connect(&mut conn, &final_connection_string).await?;
        Ok(Box::new(conn))
    }
}

impl Default for PostgresConnectionFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionFactory for PostgresConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        self.create_postgres_connection(connection_string, None)
            .await
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String> {
        Ok(connection_string.to_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }
}

/// A unified connection manager that can handle multiple database types
/// through a common interface while maintaining type-specific functionality
#[derive(Clone)]
pub struct UnifiedConnectionManager {
    #[allow(dead_code)]
    registry: Arc<ConnectionRegistry>,
    connections: Arc<RwLock<HashMap<String, Arc<dyn Connection>>>>,
    connection_factories: Arc<HashMap<String, Arc<dyn ConnectionFactory>>>,
}

impl UnifiedConnectionManager {
    /// Create a new unified connection manager with default connection factories
    pub fn new() -> Self {
        let postgres_factory = PostgresConnectionFactory::new();
        let postgres_factory_arc: Arc<PostgresConnectionFactory> = Arc::new(postgres_factory);

        let mut registry = ConnectionRegistry::new();
        registry.register_factory("SQLite".to_string(), Box::new(SqliteConnectionFactory));
        registry.register_factory(
            "PostgreSQL".to_string(),
            Box::new(SqliteConnectionFactory), // Use a simple factory for the registry
        );

        let mut factories: HashMap<String, Arc<dyn ConnectionFactory>> = HashMap::new();
        factories.insert("SQLite".to_string(), Arc::new(SqliteConnectionFactory));
        factories.insert("PostgreSQL".to_string(), postgres_factory_arc.clone());

        Self {
            registry: Arc::new(registry),
            connections: Arc::new(RwLock::new(HashMap::new())),
            connection_factories: Arc::new(factories),
        }
    }

    /// Get or create a connection based on connection string
    pub async fn get_or_create_connection(
        &self,
        connection_string: &str,
    ) -> Result<Arc<dyn Connection>, anyhow::Error> {
        let connection_key = self.generate_connection_key(connection_string)?;

        {
            let connections = self.connections.read().await;
            if let Some(existing_conn) = connections.get(&connection_key) {
                // Check if the connection is still healthy
                if existing_conn.test_connection().await.unwrap_or(false) {
                    log::debug!("Using existing healthy connection: {}", connection_key);
                    return Ok(Arc::clone(existing_conn));
                } else {
                    log::info!(
                        "Existing connection is unhealthy, will recreate: {}",
                        connection_key
                    );
                    // Drop the read lock before we try to get a write lock
                    drop(connections);
                }
            }
        }

        // Connection doesn't exist or is unhealthy, create a new one
        let mut connections = self.connections.write().await;

        // Double-check in case another thread created it while we were waiting for the write lock
        if let Some(existing_conn) = connections.get(&connection_key) {
            log::debug!(
                "Found connection created by another thread: {}",
                connection_key
            );
            return Ok(Arc::clone(existing_conn));
        }

        // Determine connection type from connection string
        let connection_type = self.detect_connection_type(connection_string)?;

        // Get the appropriate factory
        let factory = self
            .connection_factories
            .get(&connection_type)
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", connection_type)
            })?;

        log::info!(
            "Creating new connection: {} (type: {})",
            connection_key,
            connection_type
        );

        // Create new connection
        let conn = factory.create_connection(connection_string).await?;
        let conn_arc: Arc<dyn Connection> = Arc::from(conn);

        // Store the connection
        connections.insert(connection_key.clone(), conn_arc.clone());

        log::info!(
            "Successfully created and stored connection: {}",
            connection_key
        );
        Ok(conn_arc)
    }

    /// Close and remove a connection
    #[allow(dead_code)]
    pub async fn close_connection(&self, connection_string: &str) -> Result<()> {
        let connection_key = self.generate_connection_key(connection_string)?;
        let mut connections = self.connections.write().await;

        if let Some(_conn) = connections.remove(&connection_key) {
            log::info!("Closing connection: {}", connection_key);
            // Note: We can't disconnect here as we need a mutable reference
            // The connection will be dropped when Arc goes out of scope
        }

        Ok(())
    }

    /// Close all connections
    #[allow(dead_code)]
    pub async fn close_all_connections(&self) -> Result<()> {
        let mut connections = self.connections.write().await;
        let count = connections.len();
        connections.clear();
        log::info!("Closed {} connections", count);
        Ok(())
    }

    /// Generate a unique connection key from a connection string
    pub fn generate_connection_key(&self, connection_string: &str) -> Result<String> {
        // Simple implementation - just use the connection string as the key
        // In a real implementation, you might normalize it (e.g., remove password)
        Ok(connection_string.to_string())
    }

    /// Detect connection type from connection string
    fn detect_connection_type(&self, connection_string: &str) -> Result<String> {
        let connection_lower = connection_string.to_lowercase();

        // Log the connection string and detected type for debugging
        log::debug!("Detecting connection type for: '{}'", connection_string);

        if connection_lower.starts_with("postgres://")
            || connection_lower.starts_with("postgresql://")
        {
            log::debug!("Detected PostgreSQL connection type");
            Ok("PostgreSQL".to_string())
        } else if connection_lower.starts_with("sqlite:")
            || connection_lower.contains(".db")
            || connection_lower == ":memory:"
            || connection_lower == "sqlite::memory:"
        {
            log::debug!("Detected SQLite connection type");
            Ok("SQLite".to_string())
        } else {
            // Default to SQLite for unknown types
            log::warn!(
                "Unknown connection type for '{}', defaulting to SQLite",
                connection_string
            );
            Ok("SQLite".to_string())
        }
    }

    /// Test a connection string without storing the connection
    #[allow(dead_code)]
    pub async fn test_connection(&self, connection_string: &str) -> Result<bool> {
        let connection_type = self.detect_connection_type(connection_string)?;

        let factory = self
            .connection_factories
            .get(&connection_type)
            .ok_or_else(|| {
                anyhow::anyhow!("No factory found for connection type: {}", connection_type)
            })?;

        let conn = factory.create_connection(connection_string).await?;
        conn.test_connection().await
    }
}

/// SSH tunnel connection info
#[derive(Debug, Clone)]
pub struct SshTunnelConnection {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub created_at: std::time::Instant,
}

/// Global database service that holds app database and unified connection manager
#[derive(Clone)]
pub struct DbService {
    pub app_db: Arc<RwLock<Option<AppDatabase>>>,
    pub unified_manager: Arc<RwLock<UnifiedConnectionManager>>,
    // SSH tunnel management
    ssh_tunnels: Arc<async_std::sync::Mutex<HashMap<String, SshTunnelConnection>>>,
    // GPUI tokio runtime handle for automatic SSH tunnel establishment
    runtime_handle: Option<tokio::runtime::Handle>,
}

impl Global for DbService {}

impl DbService {
    pub fn new(runtime_handle: Option<tokio::runtime::Handle>) -> Self {
        Self {
            app_db: Arc::new(RwLock::new(None)),
            unified_manager: Arc::new(RwLock::new(UnifiedConnectionManager::new())),
            ssh_tunnels: Arc::new(async_std::sync::Mutex::new(HashMap::new())),
            runtime_handle,
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

    /// Create SSH tunnel key for connection
    fn create_ssh_tunnel_key(
        &self,
        ssh_host: &str,
        ssh_port: u16,
        ssh_user: &str,
        remote_host: &str,
        remote_port: u16,
    ) -> String {
        format!(
            "{}@{}:{}->{}:{}",
            ssh_user, ssh_host, ssh_port, remote_host, remote_port
        )
    }

    /// Establish SSH tunnel automatically using stored runtime handle
    fn establish_ssh_tunnel_automatic(
        &self,
        ssh_host: String,
        ssh_port: u16,
        ssh_user: String,
        ssh_password: Option<String>,
        ssh_private_key_path: Option<String>,
        ssh_private_key_password: Option<String>,
        remote_host: String,
        remote_port: u16,
    ) -> Result<u16, anyhow::Error> {
        let Some(runtime_handle) = &self.runtime_handle else {
            return Err(anyhow::anyhow!(
                "No runtime handle available for automatic SSH tunnel establishment"
            ));
        };

        log::info!(
            "Automatically establishing SSH tunnel for {}@{}:{} -> {}:{}",
            ssh_user,
            ssh_host,
            ssh_port,
            remote_host,
            remote_port
        );

        let local_port = self.assign_local_port();

        // Create SSH tunnel configuration
        let tunnel_config = SshTunnelConfig {
            ssh_host: ssh_host.clone(),
            ssh_port,
            ssh_user: ssh_user.clone(),
            ssh_password,
            ssh_private_key_path,
            ssh_private_key_password,
            remote_host: remote_host.clone(),
            remote_port,
            local_port,
        };

        // Spawn SSH tunnel establishment on the stored runtime handle
        let ssh_tunnels = self.ssh_tunnels.clone();
        let tunnel_key =
            self.create_ssh_tunnel_key(&ssh_host, ssh_port, &ssh_user, &remote_host, remote_port);
        let local_port_for_async = local_port;
        let remote_host_clone = remote_host.clone();
        let remote_port_clone = remote_port;

        let _ = runtime_handle.spawn(async move {
            match SshTunnel::create(tunnel_config).await {
                Ok(mut tunnel) => {
                    log::info!("SSH tunnel established automatically: {}", tunnel_key);

                    // Store tunnel connection info
                    {
                        let mut tunnels = ssh_tunnels.lock().await;
                        tunnels.insert(
                            tunnel_key.clone(),
                            SshTunnelConnection {
                                local_port: local_port_for_async,
                                remote_host: remote_host_clone,
                                remote_port: remote_port_clone,
                                created_at: std::time::Instant::now(),
                            },
                        );
                    }

                    // Set up TCP forwarding - this keeps the tunnel alive
                    if let Err(e) = tunnel.setup_tcp_forwarding().await {
                        log::error!(
                            "Failed to setup TCP forwarding for auto-established tunnel {}: {}",
                            tunnel_key,
                            e
                        );

                        // Remove failed tunnel
                        let mut tunnels = ssh_tunnels.lock().await;
                        tunnels.remove(&tunnel_key);
                    }
                }
                Err(e) => {
                    log::error!(
                        "Failed to establish SSH tunnel automatically {}: {}",
                        tunnel_key,
                        e
                    );
                }
            }
        });

        Ok(local_port)
    }

    /// Get existing SSH tunnel connection info
    pub async fn get_ssh_tunnel(
        &self,
        ssh_host: &str,
        ssh_port: u16,
        ssh_user: &str,
        remote_host: &str,
        remote_port: u16,
    ) -> Option<SshTunnelConnection> {
        let tunnel_key =
            self.create_ssh_tunnel_key(ssh_host, ssh_port, ssh_user, remote_host, remote_port);
        let tunnels = self.ssh_tunnels.lock().await;
        tunnels.get(&tunnel_key).cloned()
    }

    /// Create a tunneled connection string using existing SSH tunnel
    fn create_tunneled_connection_string(
        &self,
        _original_connection_string: &str,
        local_port: u16,
        database: &str,
        username: &str,
        password: Option<&String>,
    ) -> Result<String> {
        let host = "localhost";

        let connection_string = if let Some(password) = password {
            if password.is_empty() {
                format!(
                    "postgresql://{}@{}:{}/{}",
                    username, host, local_port, database
                )
            } else {
                format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    username, password, host, local_port, database
                )
            }
        } else {
            format!(
                "postgresql://{}@{}:{}/{}",
                username, host, local_port, database
            )
        };

        log::debug!("Created tunneled connection string: {}", connection_string);
        Ok(connection_string)
    }

    /// Assign a local port for SSH tunnel
    fn assign_local_port(&self) -> u16 {
        // Simple port assignment starting from 15432
        static PORT_COUNTER: std::sync::atomic::AtomicU16 =
            std::sync::atomic::AtomicU16::new(15432);
        PORT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Primary method to get or create a connection by ID
    pub async fn get_or_create_connection(
        &self,
        connection_id: i64,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        self.get_or_create_connection_with_database(connection_id, None)
            .await
    }

    /// Get or create a connection with optional database override
    pub async fn get_or_create_connection_with_database(
        &self,
        connection_id: i64,
        database_name: Option<&str>,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        // Get the app database
        let app_db_lock = self.app_db.read().await;
        let app_db = app_db_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("App database not initialized"))?;

        // Query the connections table to get connection data including SSH configuration
        let connection_row = sqlx::query(
            r#"
            SELECT
                CASE
                    WHEN db_type = 'SQLite' THEN CONCAT('sqlite:',database_path)
                    ELSE connection_string
                END as connection_string,
                db_type,
                ssh_host, ssh_port, ssh_user, ssh_password,
                ssh_private_key_path, ssh_private_key_password
            FROM connections
            WHERE id = ? AND is_active = 1
            "#,
        )
        .bind(connection_id)
        .fetch_one(app_db.pool())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to query connection {}: {}", connection_id, e))?;

        let mut connection_string: String = connection_row.try_get("connection_string")?;
        let db_type: String = connection_row.try_get("db_type")?;

        // Apply database override for PostgreSQL connections
        if db_type == "PostgreSQL"
            && let Some(database_name) = database_name
        {
            connection_string =
                replace_database_in_postgres_connection_string(&connection_string, database_name);
            log::debug!(
                "Applied database override '{}' to connection string: {}",
                database_name,
                connection_string
            );
        }

        log::debug!(
            "Found connection data for id {}: type={}, string={}",
            connection_id,
            db_type,
            connection_string
        );

        // Check if this is a PostgreSQL connection with SSH configuration
        if db_type == "PostgreSQL" {
            let ssh_host: Option<String> = connection_row.try_get("ssh_host").ok();

            if let Some(host) = ssh_host {
                // SSH configuration exists
                let ssh_user: Option<String> = connection_row.try_get("ssh_user").ok();
                if let Some(user) = ssh_user {
                    // Validate that SSH host and user are not empty
                    if !host.trim().is_empty() && !user.trim().is_empty() {
                        let ssh_port: i32 = connection_row.try_get("ssh_port").unwrap_or(22);
                        let ssh_password: Option<String> = connection_row
                            .try_get("ssh_password")
                            .ok()
                            .filter(|s: &String| !s.is_empty());
                        let ssh_private_key_path: Option<String> = connection_row
                            .try_get("ssh_private_key_path")
                            .ok()
                            .filter(|s: &String| !s.is_empty());
                        let ssh_private_key_password: Option<String> = connection_row
                            .try_get("ssh_private_key_password")
                            .ok()
                            .filter(|s: &String| !s.is_empty());

                        let ssh_config = PostgresSshConfig {
                            ssh_host: host.trim().to_string(),
                            ssh_port: ssh_port as u16,
                            ssh_user: user.trim().to_string(),
                            ssh_password,
                            ssh_private_key_path,
                            ssh_private_key_password,
                        };

                        log::info!(
                            "Checking for existing SSH tunnel to {}:{} for connection ID {}",
                            ssh_config.ssh_host,
                            ssh_config.ssh_port,
                            connection_id
                        );

                        // Extract remote host and port from the original connection string
                        let key = PgConnectionKey::from_connection_string(&connection_string)?;
                        let remote_host = key.host;
                        let remote_port = key.port;

                        log::info!(
                            "Connection string {:?}, key {:?}",
                            connection_string,
                            remote_port,
                        );

                        // Check if SSH tunnel already exists
                        if let Some(tunnel_info) = self
                            .get_ssh_tunnel(
                                &ssh_config.ssh_host,
                                ssh_config.ssh_port,
                                &ssh_config.ssh_user,
                                &remote_host,
                                remote_port,
                            )
                            .await
                        {
                            log::info!(
                                "Using existing SSH tunnel {} -> localhost:{} for connection ID {}",
                                tunnel_info.remote_host,
                                tunnel_info.local_port,
                                connection_id
                            );

                            // Create connection string using existing tunnel
                            let tunneled_connection_string = self
                                .create_tunneled_connection_string(
                                    &connection_string,
                                    tunnel_info.local_port,
                                    &key.database,
                                    &key.username,
                                    key.password.as_ref(),
                                )?;

                            log::info!(
                                "Creating tunneled PostgreSQL connection to {}:{} via tunnel localhost:{} for connection ID {}",
                                remote_host,
                                remote_port,
                                tunnel_info.local_port,
                                connection_id
                            );

                            return self
                                .get_or_create_unified_connection_internal(
                                    &tunneled_connection_string,
                                )
                                .await;
                        } else {
                            log::info!(
                                "No existing SSH tunnel found for {}@{}:{} -> {}:{}, establishing automatically",
                                ssh_config.ssh_user,
                                ssh_config.ssh_host,
                                ssh_config.ssh_port,
                                remote_host,
                                remote_port
                            );

                            // Automatically establish SSH tunnel using stored runtime handle
                            log::info!("private key path {:?}", ssh_config.ssh_private_key_path);
                            let local_port = self.establish_ssh_tunnel_automatic(
                                ssh_config.ssh_host.clone(),
                                ssh_config.ssh_port,
                                ssh_config.ssh_user.clone(),
                                ssh_config.ssh_password.clone(),
                                ssh_config.ssh_private_key_path.clone(),
                                ssh_config.ssh_private_key_password.clone(),
                                remote_host.to_string(),
                                remote_port as u16,
                            )?;

                            log::info!(
                                "SSH tunnel automatically established on local port: {}",
                                local_port
                            );

                            // Create tunneled connection string using the newly established tunnel
                            let tunneled_connection_string = self
                                .create_tunneled_connection_string(
                                    &connection_string,
                                    local_port,
                                    &key.database,
                                    &key.username,
                                    key.password.as_ref(),
                                )?;

                            log::info!(
                                "Creating tunneled PostgreSQL connection using automatically established SSH tunnel"
                            );

                            return self
                                .get_or_create_unified_connection_internal(
                                    &tunneled_connection_string,
                                )
                                .await;
                        }
                    } else {
                        log::warn!(
                            "SSH configuration has empty host or user for connection ID {}, ignoring SSH tunnel",
                            connection_id
                        );
                    }
                }
            }
        }

        // No SSH configuration or not PostgreSQL, use standard connection
        self.get_or_create_unified_connection_internal(&connection_string)
            .await
    }

    /// Internal method to get or create a connection by connection string
    async fn get_or_create_unified_connection_internal(
        &self,
        connection_string: &str,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        let unified_manager = self.unified_manager().await;

        unified_manager
            .read()
            .await
            .get_or_create_connection(connection_string)
            .await
    }

    /// Execute a query by connection ID
    #[allow(dead_code)]
    pub async fn execute_query_by_id(
        &self,
        connection_id: i64,
        sql: &str,
    ) -> Result<blanco_core::QueryResult, anyhow::Error> {
        // Get the connection by ID
        let connection = self.get_or_create_connection(connection_id).await?;

        // Execute the query
        connection.execute_query(sql, None).await
    }
}

// Implement the DatabaseService trait for DbService
#[async_trait::async_trait]
impl blanco_core::DatabaseService for DbService {
    async fn get_or_create_connection(
        &self,
        connection_string: &str,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        // For the trait, keep the old interface but internally delegate to connection_id lookup
        self.get_or_create_unified_connection_internal(connection_string)
            .await
    }

    async fn get_or_create_connection_by_id(
        &self,
        connection_id: i64,
    ) -> Result<std::sync::Arc<dyn blanco_core::Connection>, anyhow::Error> {
        // Use our primary method for connection_id lookup
        self.get_or_create_connection(connection_id).await
    }
}
