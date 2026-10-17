mod connection_impl;
mod decode;
mod execute;
mod types;

pub use types::{PgConnectionKey, PgServerKey, PostgresSshConfig, QueryParam};

use anyhow::Result;
use blanco_core::{Connection, DatabaseType};
use smol::lock::RwLock;
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::sync::Arc;

/// PostgreSQL connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface with database-specific connection pools
pub struct PostgresConnection {
    /// PostgreSQL, or a server speaking its protocol (CockroachDB), whose
    /// catalog differs in places.
    database_type: DatabaseType,
    pools: Arc<RwLock<HashMap<String, sqlx::PgPool>>>, // database_name -> connection pool
    server_key: PgServerKey,                           // Server-level connection key (no database)
    display_name: String,
    server_connection_string: String, // Connection string without database
    initial_database: Option<String>, // Original database from connection string
    ssh_config: Option<PostgresSshConfig>, // SSH tunnel configuration
    local_tunnel_port: Option<u16>,   // Local port for SSH tunnel (if configured)
}

impl PostgresConnection {
    /// Create a new PostgreSQL connection from a connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let connection_key = PgConnectionKey::from_connection_string(connection_string)?;
        let server_key = connection_key.to_server_key();
        let display_name =
            Self::generate_server_display_name(DatabaseType::PostgreSQL, &server_key);

        Ok(Self {
            database_type: DatabaseType::PostgreSQL,
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string: connection_string.to_string(),
            initial_database: Some(connection_key.database),
            ssh_config: None,
            local_tunnel_port: None,
        })
    }

    /// Create a new server-level PostgreSQL connection (preferred method for multi-database support)
    pub fn from_server_key(server_key: PgServerKey) -> Self {
        let display_name =
            Self::generate_server_display_name(DatabaseType::PostgreSQL, &server_key);

        Self {
            database_type: DatabaseType::PostgreSQL,
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string: String::new(), // Will be set during connect
            initial_database: None,
            ssh_config: None,
            local_tunnel_port: None,
        }
    }

    /// Create a new PostgreSQL connection with SSH tunnel support
    pub fn from_key_with_ssh(
        connection_key: PgConnectionKey,
        ssh_config: PostgresSshConfig,
    ) -> Self {
        let server_key = connection_key.to_server_key();
        let display_name = Self::generate_server_display_name_with_ssh(
            DatabaseType::PostgreSQL,
            &server_key,
            &ssh_config,
        );

        Self {
            database_type: DatabaseType::PostgreSQL,
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string: String::new(), // Will be set during connect
            initial_database: Some(connection_key.database),
            ssh_config: Some(ssh_config),
            local_tunnel_port: None,
        }
    }

    /// Create a new server-level PostgreSQL connection with SSH tunnel support
    pub fn from_server_key_with_ssh(
        server_key: PgServerKey,
        ssh_config: PostgresSshConfig,
    ) -> Self {
        let display_name = Self::generate_server_display_name_with_ssh(
            DatabaseType::PostgreSQL,
            &server_key,
            &ssh_config,
        );

        Self {
            database_type: DatabaseType::PostgreSQL,
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string: String::new(), // Will be set during connect
            initial_database: None,
            ssh_config: Some(ssh_config),
            local_tunnel_port: None,
        }
    }

    /// Talk to `database_type`, a server speaking PostgreSQL's protocol, in
    /// place of PostgreSQL itself.
    pub fn with_database_type(mut self, database_type: DatabaseType) -> Self {
        self.database_type = database_type;
        self.display_name = match &self.ssh_config {
            Some(ssh_config) => Self::generate_server_display_name_with_ssh(
                database_type,
                &self.server_key,
                ssh_config,
            ),
            None => Self::generate_server_display_name(database_type, &self.server_key),
        };
        self
    }

    /// The backend's name for messages.
    pub(crate) fn backend_name(&self) -> &'static str {
        self.database_type.as_str()
    }

    /// Generate a human-readable display name for server-level connections
    fn generate_server_display_name(
        database_type: DatabaseType,
        server_key: &PgServerKey,
    ) -> String {
        format!(
            "{} - {}@{}:{}",
            database_type.as_str(),
            server_key.username,
            server_key.host,
            server_key.port
        )
    }

    /// Generate a human-readable display name for SSH connections
    fn generate_server_display_name_with_ssh(
        database_type: DatabaseType,
        server_key: &PgServerKey,
        ssh_config: &PostgresSshConfig,
    ) -> String {
        format!(
            "{} (via SSH) - {}@{}:{} → {}@{}:{}",
            database_type.as_str(),
            ssh_config.ssh_user,
            ssh_config.ssh_host,
            ssh_config.ssh_port,
            server_key.username,
            server_key.host,
            server_key.port
        )
    }

    /// Build a connection string for a specific database by replacing the database
    /// path component in the stored server connection string.
    fn connection_string_for_database(&self, database: &str) -> Result<String> {
        let mut parsed = url::Url::parse(&self.server_connection_string)
            .map_err(|e| anyhow::anyhow!("Failed to parse connection string: {}", e))?;
        parsed.set_path(&format!("/{}", database));
        Ok(parsed.to_string())
    }

    /// The driver options for `connection_string`, with the driver's own
    /// password file lookup switched off. `DatabaseService` has already done
    /// that lookup, against the configured host rather than an SSH tunnel's
    /// local end, and the driver's would read `~/.pgpass` whatever
    /// `PGPASSFILE` says. The driver sends an absent password as an empty
    /// one, so setting it empty changes nothing but that lookup.
    pub(crate) fn connect_options(
        connection_string: &str,
    ) -> Result<sqlx::postgres::PgConnectOptions> {
        use std::str::FromStr as _;

        let options = sqlx::postgres::PgConnectOptions::from_str(connection_string)
            .map_err(|e| anyhow::anyhow!("Failed to parse connection string: {}", e))?;
        let has_password = url::Url::parse(connection_string)
            .is_ok_and(|url| url.password().is_some_and(|password| !password.is_empty()));
        Ok(if has_password {
            options
        } else {
            options.password("")
        })
    }

    /// Get or create a connection pool for a specific database
    pub(crate) async fn get_or_create_pool(&self, database: &str) -> Result<sqlx::PgPool> {
        let mut pools = self.pools.write().await;

        if let Some(pool) = pools.get(database) {
            return Ok(pool.clone());
        }

        let database_connection_string = self.connection_string_for_database(database)?;
        let pool = PgPoolOptions::new()
            .max_connections(1)
            // sqlx pings a pooled connection before handing it out, which
            // costs a full round trip on every query to a remote server. A
            // connection the server dropped fails its query instead, and
            // `note_possible_disconnect` evicts it so the next run reconnects.
            .test_before_acquire(false)
            // Bound how long acquiring (and therefore establishing) a connection
            // may take so an unreachable host fails fast instead of hanging.
            .acquire_timeout(blanco_core::connect_timeout())
            .connect_with(Self::connect_options(&database_connection_string)?)
            .await
            .map_err(|e| {
                blanco_core::tag_sqlx(e)
                    .context(format!("Failed to connect to database '{database}'"))
            })?;

        pools.insert(database.to_string(), pool.clone());

        Ok(pool)
    }

    /// Get connection details (server-level)
    pub fn get_connection_details(&self) -> (&str, u16, &str) {
        (
            &self.server_key.host,
            self.server_key.port,
            &self.server_key.username,
        )
    }

    /// Get the server key
    pub fn get_server_key(&self) -> &PgServerKey {
        &self.server_key
    }

    /// Fetch PostgreSQL tables using async background task
    async fn fetch_postgres_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let schema_filter = schema.unwrap_or("public");
        let query = "
            SELECT table_name
            FROM information_schema.tables
            WHERE table_schema = $1
            AND table_type = 'BASE TABLE'
            ORDER BY table_name
        ";

        let result = self
            .execute_query(
                query,
                self.initial_database.as_deref(),
                Some(&[schema_filter.to_string()]),
            )
            .await?;
        let tables: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect();

        Ok(tables)
    }
}
