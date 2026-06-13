mod connection_impl;
mod decode;
mod execute;
mod types;

pub use types::{PgConnectionKey, PgServerKey, PostgresSshConfig, QueryParam};

use anyhow::Result;
use blanco_core::Connection;
use smol::lock::RwLock;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::sync::Arc;

/// PostgreSQL connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface with database-specific connection pools
pub struct PostgresConnection {
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
        let display_name = Self::generate_server_display_name(&server_key);

        Ok(Self {
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
        let display_name = Self::generate_server_display_name(&server_key);

        Self {
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
        let display_name = Self::generate_server_display_name_with_ssh(&server_key, &ssh_config);

        Self {
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
        let display_name = Self::generate_server_display_name_with_ssh(&server_key, &ssh_config);

        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string: String::new(), // Will be set during connect
            initial_database: None,
            ssh_config: Some(ssh_config),
            local_tunnel_port: None,
        }
    }

    /// Generate a human-readable display name for server-level connections
    fn generate_server_display_name(server_key: &PgServerKey) -> String {
        format!(
            "PostgreSQL - {}@{}:{}",
            server_key.username, server_key.host, server_key.port
        )
    }

    /// Generate a human-readable display name for SSH connections
    fn generate_server_display_name_with_ssh(
        server_key: &PgServerKey,
        ssh_config: &PostgresSshConfig,
    ) -> String {
        format!(
            "PostgreSQL (via SSH) - {}@{}:{} → {}@{}:{}",
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

    /// Get or create a connection pool for a specific database
    pub(crate) async fn get_or_create_pool(&self, database: &str) -> Result<sqlx::PgPool> {
        let mut pools = self.pools.write().await;

        if let Some(pool) = pools.get(database) {
            return Ok(pool.clone());
        }

        let database_connection_string = self.connection_string_for_database(database)?;
        let pool = PgPoolOptions::new()
            .max_connections(1)
            // Bound how long acquiring (and therefore establishing) a connection
            // may take so an unreachable host fails fast instead of hanging.
            .acquire_timeout(blanco_core::connect_timeout())
            .connect(&database_connection_string)
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

    /// Resolve OIDs to names using batch query for performance
    async fn resolve_oids_to_names(
        &self,
        pool: &sqlx::PgPool,
        oids: &[i32],
    ) -> Result<std::collections::HashMap<i32, String>> {
        if oids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        // Create a comma-separated list of OIDs for the IN clause
        let oid_list: String = oids
            .iter()
            .map(|oid| oid.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let query = format!(
            "SELECT oid::text, relname FROM pg_class WHERE oid IN ({})",
            oid_list
        );

        let rows = sqlx::query(&query).fetch_all(pool).await?;

        let mut oid_to_name = std::collections::HashMap::new();
        for row in rows {
            if let Ok(Some(oid_text)) = row.try_get::<Option<String>, _>(0)
                && let Ok(oid_val) = oid_text.parse::<i32>()
                && let Ok(Some(name)) = row.try_get::<Option<String>, _>(1)
            {
                oid_to_name.insert(oid_val, name);
            }
        }

        Ok(oid_to_name)
    }
}
