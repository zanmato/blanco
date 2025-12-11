use crate::sql_parser::PostgresTableExtractor;
use anyhow::Result;
use async_std::sync::RwLock;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, ConnectionUIMetadata, IconName, QueryResult, TableChangeOperation,
    TableMetadata,
};
use futures::{Stream, StreamExt};
use sqlx::postgres::types::PgMoney;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Column, Row, TypeInfo, ValueRef};
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

/// SSH configuration for PostgreSQL connections
#[derive(Debug, Clone)]
pub struct PostgresSshConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
}

impl std::fmt::Debug for PostgresConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Cannot use async in Debug trait, so show simplified info
        let pooled_databases = vec!["[async_debug]".to_string()];

        f.debug_struct("PostgresConnection")
            .field("server_key", &self.server_key)
            .field("display_name", &self.display_name)
            .field("pooled_databases", &pooled_databases)
            .field("server_connection_string", &"[REDACTED]")
            .finish()
    }
}

/// Server-level connection key for PostgreSQL connections (no database)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PgServerKey {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
}

/// Connection key for PostgreSQL connections (legacy - kept for compatibility)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PgConnectionKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
}

impl PgServerKey {
    pub fn new(host: String, port: u16, username: String, password: Option<String>) -> Self {
        Self {
            host,
            port,
            username,
            password,
        }
    }

    /// Generate a server-level connection string (without database)
    pub fn to_server_connection_string(&self) -> String {
        let password_str = self.password.as_deref().unwrap_or("");
        let mut url = format!("postgresql://{}:{}", self.username, password_str);

        if !self.host.is_empty() && self.host != "localhost" {
            url = format!("{}@{}:{}", url, self.host, self.port);
        } else if self.host == "localhost" {
            url = format!("{}@localhost:{}", url, self.port);
        } else {
            // No host specified - this is an error case
            tracing::error!("No host specified in PostgreSQL connection string");
            return format!("postgresql://{}@localhost:{}", self.username, self.port);
        }

        // Add application_name parameter
        url = format!("{}?application_name=Blanco", url);

        tracing::debug!("Generated server connection string: {}", url);
        url
    }

    /// Generate connection string for a specific database
    pub fn to_database_connection_string(&self, database: &str) -> String {
        let password_str = self.password.as_deref().unwrap_or("");
        let mut url = format!("postgresql://{}:{}", self.username, password_str);

        if !self.host.is_empty() && self.host != "localhost" {
            url = format!("{}@{}:{}", url, self.host, self.port);
        } else if self.host == "localhost" {
            url = format!("{}@localhost:{}", url, self.port);
        } else {
            // No host specified - this is an error case
            tracing::error!("No host specified in PostgreSQL connection string");
            return format!("postgresql://{}@localhost:{}", self.username, self.port);
        }

        // Add database and application_name parameter
        url = format!("{}/{}?application_name=Blanco", url, database);

        tracing::debug!("Generated database connection string: {}", url);
        url
    }

    /// Generate a server-level connection string with SSH tunnel support
    pub fn to_server_connection_string_with_tunnel(&self, local_tunnel_port: u16) -> String {
        let password_str = self.password.as_deref().unwrap_or("");
        let mut url = format!("postgresql://{}:{}", self.username, password_str);

        // Always use localhost and the tunnel port when SSH tunneling
        url = format!("{}@localhost:{}", url, local_tunnel_port);

        // Add application_name parameter
        url = format!("{}?application_name=Blanco", url);

        tracing::debug!("Generated SSH tunnel server connection string: {}", url);
        url
    }

    /// Generate connection string for a specific database using SSH tunnel
    pub fn to_database_connection_string_with_tunnel(
        &self,
        database: &str,
        local_tunnel_port: u16,
    ) -> String {
        let password_str = self.password.as_deref().unwrap_or("");
        let mut url = format!("postgresql://{}:{}", self.username, password_str);

        // Always use localhost and the tunnel port when SSH tunneling
        url = format!("{}@localhost:{}", url, local_tunnel_port);

        // Add database and application_name parameter
        url = format!("{}/{}?application_name=Blanco", url, database);

        tracing::debug!("Generated SSH tunnel database connection string: {}", url);
        url
    }
}

impl PgConnectionKey {
    pub fn new(
        host: String,
        port: u16,
        database: String,
        username: String,
        password: Option<String>,
    ) -> Self {
        Self {
            host,
            port,
            database,
            username,
            password,
        }
    }

    /// Convert to server key (removing database)
    pub fn to_server_key(&self) -> PgServerKey {
        PgServerKey::new(
            self.host.clone(),
            self.port,
            self.username.clone(),
            self.password.clone(),
        )
    }

    /// Extract connection key from a PostgreSQL connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        // Handle different PostgreSQL connection string formats:
        // - postgresql://user:password@host:port/database
        // - postgres://user:password@host:port/database

        let url = if connection_string.starts_with("postgresql://")
            || connection_string.starts_with("postgres://")
        {
            connection_string
        } else {
            return Err(anyhow::anyhow!(
                "Invalid PostgreSQL connection string format"
            ));
        };

        // Parse the URL
        let parsed = url::Url::parse(url)?;

        let host = parsed.host_str().unwrap_or("localhost").to_string();
        let port = parsed.port().unwrap_or(5432);
        let database = parsed.path().trim_start_matches('/').to_string();
        let username = parsed.username().to_string();
        let password = parsed.password().map(|p| p.to_string());

        if database.is_empty() {
            return Err(anyhow::anyhow!(
                "Database name is required in connection string"
            ));
        }

        if username.is_empty() {
            return Err(anyhow::anyhow!("Username is required in connection string"));
        }

        Ok(PgConnectionKey {
            host,
            port,
            database,
            username,
            password,
        })
    }

    /// Generate a connection string from this key
    pub fn to_connection_string(&self) -> String {
        if let Some(ref password) = self.password {
            format!(
                "postgresql://{}:{}@{}:{}/{}?application_name=Blanco",
                self.username, password, self.host, self.port, self.database
            )
        } else {
            format!(
                "postgresql://{}@{}:{}/{}?application_name=Blanco",
                self.username, self.host, self.port, self.database
            )
        }
    }
}

impl PostgresConnection {
    /// Create a new PostgreSQL connection for a specific database (legacy constructor for compatibility)
    pub fn new(
        host: String,
        port: u16,
        _database: String, // Database name not used in server-level architecture
        username: String,
        password: Option<String>,
    ) -> Self {
        let server_key = PgServerKey::new(host, port, username, password);
        let display_name = Self::generate_server_display_name(&server_key);
        let server_connection_string = server_key.to_server_connection_string();

        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string,
            initial_database: None,
            ssh_config: None,
            local_tunnel_port: None,
        }
    }

    /// Create a new PostgreSQL connection from a connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        tracing::info!(
            "🔗 Creating PostgreSQL connection from: {}",
            connection_string
        );

        let connection_key = PgConnectionKey::from_connection_string(connection_string)?;
        tracing::info!("📋 Parsed connection key:");
        tracing::info!("   - host: {}", connection_key.host);
        tracing::info!("   - port: {}", connection_key.port);
        tracing::info!("   - database: {}", connection_key.database);
        tracing::info!("   - username: {}", connection_key.username);
        tracing::info!(
            "   - password: [{}]",
            if connection_key.password.is_some() {
                "present"
            } else {
                "none"
            }
        );

        let server_key = connection_key.to_server_key();
        let display_name = Self::generate_server_display_name(&server_key);
        let server_connection_string = server_key.to_server_connection_string();

        tracing::info!("🏢 Server key created:");
        tracing::info!("   - host: {}", server_key.host);
        tracing::info!("   - port: {}", server_key.port);
        tracing::info!("   - username: {}", server_key.username);
        tracing::info!("   - initial_database: {}", connection_key.database);

        Ok(Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string,
            initial_database: Some(connection_key.database.clone()),
            ssh_config: None,
            local_tunnel_port: None,
        })
    }

    /// Create a new PostgreSQL connection from a PgConnectionKey
    pub fn from_key(connection_key: PgConnectionKey) -> Self {
        let server_key = connection_key.to_server_key();
        let display_name = Self::generate_server_display_name(&server_key);
        let server_connection_string = server_key.to_server_connection_string();

        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string,
            initial_database: Some(connection_key.database.clone()),
            ssh_config: None,
            local_tunnel_port: None,
        }
    }

    /// Create a new server-level PostgreSQL connection (preferred method for multi-database support)
    pub fn from_server_key(server_key: PgServerKey) -> Self {
        let display_name = Self::generate_server_display_name(&server_key);
        let server_connection_string = server_key.to_server_connection_string();

        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string,
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
            server_connection_string: String::new(), // Will be set up during connect
            initial_database: Some(connection_key.database.clone()),
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
            server_connection_string: String::new(), // Will be set up during connect
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

    /// Find an available database to connect to when no specific database is specified
    async fn get_available_database(&self) -> Result<String> {
        // Try the initial database from the connection string first
        if let Some(ref initial_db) = self.initial_database {
            tracing::debug!(
                "Trying initial database '{}' from connection string",
                initial_db
            );
            if self.try_connect_to_database(initial_db).await {
                tracing::info!(
                    "Using initial database '{}' for metadata queries",
                    initial_db
                );
                return Ok(initial_db.clone());
            } else {
                tracing::warn!(
                    "Initial database '{}' is not accessible, trying alternatives",
                    initial_db
                );
            }
        }

        // Common PostgreSQL databases that are likely to exist and be accessible
        let common_databases = ["postgres", "template1", "template0"];
        tracing::debug!("Trying common databases: {}", common_databases.join(", "));

        for &db_name in &common_databases {
            if self.try_connect_to_database(db_name).await {
                tracing::info!(
                    "Successfully connected to common database '{}' for metadata queries",
                    db_name
                );
                return Ok(db_name.to_string());
            }
        }

        let mut tried_databases = Vec::new();
        if let Some(ref initial_db) = self.initial_database {
            tried_databases.push(initial_db.clone());
        }
        tried_databases.extend(common_databases.iter().map(|s| s.to_string()));

        Err(anyhow::anyhow!(
            "Could not connect to any database (tried: {})",
            tried_databases.join(", ")
        ))
    }

    /// Try to connect to a specific database and return true if successful
    async fn try_connect_to_database(&self, database: &str) -> bool {
        let database_connection_string = if let Some(local_port) = self.local_tunnel_port {
            self.server_key
                .to_database_connection_string_with_tunnel(database, local_port)
        } else {
            self.server_key.to_database_connection_string(database)
        };
        tracing::debug!(
            "Trying to connect to database '{}' for metadata queries",
            database
        );
        tracing::debug!("Connection string: {}", database_connection_string);

        tracing::debug!("Attempting connection with 5-second timeout...");
        match PgPoolOptions::new()
            .max_connections(1) // Just for testing connectivity
            .connect(&database_connection_string)
            .await
        {
            Ok(pool) => {
                tracing::debug!(
                    "Connected to database '{}', testing query execution",
                    database
                );
                // Test if we can actually execute queries
                match sqlx::query("SELECT 1").fetch_one(&pool).await {
                    Ok(_) => {
                        tracing::info!(
                            "✅ Successfully connected to database '{}' for metadata queries",
                            database
                        );
                        // Pre-cache this pool for future use
                        let mut pools = self.pools.write().await;
                        if !pools.contains_key(database) {
                            pools.insert(database.to_string(), pool);
                        }
                        true
                    }
                    Err(e) => {
                        tracing::warn!(
                            "⚠️ Connected to database '{}' but query test failed: {}",
                            database,
                            e
                        );
                        false
                    }
                }
            }
            Err(e) => {
                tracing::warn!("❌ Failed to connect to database '{}': {}", database, e);
                // Provide additional diagnostic info for common connection issues
                let error_str = e.to_string().to_lowercase();
                if error_str.contains("timeout") {
                    tracing::warn!(
                        "   → Connection timeout - check network connectivity and firewall"
                    );
                } else if error_str.contains("authentication") || error_str.contains("password") {
                    tracing::warn!("   → Authentication failed - check username/password");
                } else if error_str.contains("database") && error_str.contains("not exist") {
                    tracing::warn!("   → Database does not exist");
                } else if error_str.contains("connection") && error_str.contains("refused") {
                    tracing::warn!("   → Connection refused - check if PostgreSQL is running and accepting connections");
                }
                false
            }
        }
    }

    /// Get or create a connection pool for a specific database
    async fn get_or_create_pool(&self, database: &str) -> Result<sqlx::PgPool> {
        let mut pools = self.pools.write().await;

        if let Some(pool) = pools.get(database) {
            return Ok(pool.clone());
        }

        let database_connection_string = if let Some(local_port) = self.local_tunnel_port {
            self.server_key
                .to_database_connection_string_with_tunnel(database, local_port)
        } else {
            self.server_key.to_database_connection_string(database)
        };
        tracing::info!("Creating new connection pool for database: {}", database);

        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_connection_string)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to database '{}': {}", database, e))?;

        pools.insert(database.to_string(), pool.clone());
        tracing::info!(
            "Successfully created connection pool for database: {}",
            database
        );

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

    /// Check if the database connection is healthy with a ping query
    async fn is_connection_healthy(&self) -> bool {
        let pools = self.pools.read().await;

        // Check if we have any active pools
        if pools.is_empty() {
            return false;
        }

        // Check health of all pools - if any are healthy, connection is considered healthy
        for (_, pool) in pools.iter() {
            if (sqlx::query("SELECT 1").fetch_one(pool).await).is_ok() {
                return true;
            }
        }

        false
    }

    /// Check if a column type is an array type
    fn is_array_type(column_type: &str) -> bool {
        column_type == "ARRAY" || column_type.ends_with("[]")
    }

    /// Extract the base type from an array type (e.g., "TEXT[]" -> Some("TEXT"))
    fn extract_base_array_type(array_type: &str) -> Option<&str> {
        if array_type.ends_with("[]") {
            Some(&array_type[..array_type.len() - 2])
        } else {
            None
        }
    }

    /// Extract precision information from numeric types (e.g., "numeric(10,2)" -> Some("numeric"))
    fn get_numeric_precision_type(column_type: &str) -> Option<&str> {
        if column_type.starts_with("numeric(") || column_type.starts_with("decimal(") {
            if let Some(paren_pos) = column_type.find('(') {
                return Some(&column_type[..paren_pos]);
            }
        }
        None
    }

    /// Check if a value is NULL without attempting type conversion
    fn is_null_value(row: &sqlx::postgres::PgRow, column_index: usize) -> bool {
        if let Ok(raw_value) = row.try_get_raw(column_index) {
            raw_value.is_null()
        } else {
            false // If we can't even get raw value, assume it's not NULL
        }
    }

    /// Handle array types with proper base type detection
    fn handle_array_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> String {
        let base_type = Self::extract_base_array_type(column_type);

        match base_type {
            Some("text") | Some("varchar") | Some("char") | Some("BPCHAR") | Some("TEXT")
            | Some("VARCHAR") | Some("CHAR") | Some("NAME") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<String>>, _>(column_index) {
                    return array_val
                        .map(|v| {
                            let result = format!(
                                "{{{}}}",
                                v.iter()
                                    .map(|x| format!("\"{}\"", x))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                            result
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            Some("int4") | Some("integer") | Some("int") | Some("INT4") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<i32>>, _>(column_index) {
                    return array_val
                        .map(|v| {
                            let result = format!(
                                "{{{}}}",
                                v.iter()
                                    .map(|x| x.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                            tracing::debug!(
                                "Successfully converted array to Vec<i32> for column type '{}': {}",
                                column_type,
                                result
                            );
                            result
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            Some("int8") | Some("bigint") | Some("INT8") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<i64>>, _>(column_index) {
                    return array_val
                        .map(|v| {
                            let result = format!(
                                "{{{}}}",
                                v.iter()
                                    .map(|x| x.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                            tracing::debug!(
                                "Successfully converted array to Vec<i64> for column type '{}': {}",
                                column_type,
                                result
                            );
                            result
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            Some("int2") | Some("smallint") | Some("INT2") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<i16>>, _>(column_index) {
                    return array_val
                        .map(|v| {
                            let result = format!(
                                "{{{}}}",
                                v.iter()
                                    .map(|x| x.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                            tracing::debug!(
                                "Successfully converted array to Vec<i16> for column type '{}': {}",
                                column_type,
                                result
                            );
                            result
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            Some("float4") | Some("real") | Some("FLOAT4") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<f32>>, _>(column_index) {
                    return array_val
                        .map(|v| {
                            let result = format!(
                                "{{{}}}",
                                v.iter()
                                    .map(|x| x.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                            tracing::debug!(
                                "Successfully converted array to Vec<f32> for column type '{}': {}",
                                column_type,
                                result
                            );
                            result
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            Some("float8") | Some("double precision") | Some("FLOAT8") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<f64>>, _>(column_index) {
                    return array_val
                        .map(|v| {
                            let result = format!(
                                "{{{}}}",
                                v.iter()
                                    .map(|x| x.to_string())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                            tracing::debug!(
                                "Successfully converted array to Vec<f64> for column type '{}': {}",
                                column_type,
                                result
                            );
                            result
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            Some("bool") | Some("boolean") | Some("BOOL") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<bool>>, _>(column_index) {
                    return array_val.map(|v| {
                        let result = format!("{{{}}}", v.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join(","));
                        tracing::debug!("Successfully converted array to Vec<bool> for column type '{}': {}", column_type, result);
                        result
                    }).unwrap_or_else(|| "NULL".to_string());
                }
            }
            Some("uuid") | Some("UUID") => {
                if let Ok(array_val) = row.try_get::<Option<Vec<uuid::Uuid>>, _>(column_index) {
                    return array_val.map(|v| {
                        let result = format!("{{{}}}", v.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join(","));
                        tracing::debug!("Successfully converted array to Vec<uuid::Uuid> for column type '{}': {}", column_type, result);
                        result
                    }).unwrap_or_else(|| "NULL".to_string());
                }
            }
            _ => {
                // Fallback: try to get as string and parse as PostgreSQL array format
                if let Ok(array_val) = row.try_get::<Option<String>, _>(column_index) {
                    return array_val
                        .map(|v| {
                            tracing::debug!(
                                "Successfully converted array '{}' to String for column type '{}'",
                                v,
                                column_type
                            );
                            v
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
        }

        tracing::warn!(
            "Array column type '{}' couldn't be converted to any supported array type",
            column_type
        );
        "NULL".to_string()
    }

    /// Handle system catalog types like regclass, oid, etc.
    fn handle_system_catalog_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> String {
        match column_type {
            "regclass" => {
                // regclass will be resolved to actual table names via OID extraction
                // Return placeholder that will be replaced with resolved table name
                "RESOLVING_REGCLASS".to_string()
            }
            "oid" | "xid" | "cid" => {
                // Handle OID types directly as integers
                if let Ok(val) = row.try_get::<Option<i32>, _>(column_index) {
                    return val
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string());
                }
                self.try_string_conversion(row, column_index, column_type)
            }
            _ => {
                // For other system catalog types, try string conversion first
                self.try_string_conversion(row, column_index, column_type)
            }
        }
    }

    /// Handle UUID types
    fn handle_uuid_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<uuid::Uuid>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    /// Handle money and numeric types
    fn handle_money_numeric_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> String {
        match column_type {
            "money" => {
                if let Ok(val) = row.try_get::<Option<PgMoney>, _>(column_index) {
                    return val
                        .map(|v| {
                            let decimal_val = v.to_decimal(2); // Use 2 decimal places for currency
                            format!("${}", decimal_val)
                        })
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            "numeric" | "decimal" => {
                if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
                    return val
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string());
                }
            }
            _ => {}
        }
        "NULL".to_string()
    }

    /// Handle string types (text, varchar, char)
    fn handle_string_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
            val.unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    /// Handle integer types (smallint, integer, bigint)
    fn handle_i16_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<i16>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    fn handle_i32_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<i32>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    fn handle_i64_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<i64>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    /// Handle float types (real, double precision)
    fn handle_f32_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<f32>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    fn handle_f64_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<f64>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    /// Handle boolean types
    fn handle_bool_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<bool>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    /// Handle timestamp types
    fn handle_timestamp_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> String {
        // Handle TIMESTAMPTZ (PostgreSQL internal name for timestamp with time zone)
        if column_type == "TIMESTAMPTZ" || column_type.contains("with time zone") {
            // Try DateTime<Utc> for TIMESTAMPTZ
            if let Ok(val) = row.try_get::<Option<chrono::DateTime<chrono::Local>>, _>(column_index)
            {
                return val
                    .map(|v| v.to_rfc3339())
                    .unwrap_or_else(|| "NULL".to_string());
            }
            // Try DateTime<FixedOffset> as fallback
            if let Ok(val) =
                row.try_get::<Option<chrono::DateTime<chrono::FixedOffset>>, _>(column_index)
            {
                return val
                    .map(|v| v.format("%Y-%m-%d %H:%M:%S %:z").to_string())
                    .unwrap_or_else(|| "NULL".to_string());
            }
        }

        // Try timestamp with UTC for regular timestamp types
        if let Ok(val) = row.try_get::<Option<chrono::DateTime<chrono::Local>>, _>(column_index) {
            return val
                .map(|v| v.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }
        // Try timestamp without timezone
        if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index) {
            return val
                .map(|v| v.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }
        "NULL".to_string()
    }

    /// Handle date types
    fn handle_date_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    /// Handle time types
    fn handle_time_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index) {
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else {
            "NULL".to_string()
        }
    }

    /// Handle JSON types
    fn handle_json_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(column_index) {
            return val
                .map(|v| {
                    if v.is_string() {
                        v.as_str().unwrap_or("").to_string()
                    } else {
                        v.to_string()
                    }
                })
                .unwrap_or_else(|| "NULL".to_string());
        }
        // Fallback to string representation
        self.try_string_conversion(row, column_index, "json")
    }

    /// Generic string conversion attempt
    fn try_string_conversion(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> String {
        // First try optional string
        if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
            return val.unwrap_or_else(|| "NULL".to_string());
        }

        // For system catalog types like regclass, try specific conversions
        match column_type {
            "regclass" => {
                // Try i32 conversion for regclass (OID)
                if let Ok(oid_val) = row.try_get::<Option<i32>, _>(column_index) {
                    return oid_val
                        .map(|oid| format!("OID:{}", oid))
                        .unwrap_or_else(|| "NULL".to_string());
                }
                // Try non-optional i32
                if let Ok(oid_val) = row.try_get::<i32, _>(column_index) {
                    return format!("OID:{}", oid_val);
                }
            }
            _ => {
                // For other types, try various string conversion approaches
                // Try direct string conversion
                if let Ok(val) = row.try_get::<String, _>(column_index) {
                    return val;
                }
            }
        }

        // If all specific attempts fail, log and return NULL
        tracing::warn!("Failed to convert column type '{}' to string", column_type);
        "NULL".to_string()
    }

    /// Handle unknown types with improved raw value access
    fn handle_unknown_type(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_type: &str,
    ) -> String {
        // Try raw value access for unknown types (custom enums, domains, etc.)
        if let Ok(raw_value) = row.try_get_raw(column_index) {
            if raw_value.is_null() {
                return "NULL".to_string();
            }

            // Try to extract as text using raw value
            match raw_value.as_str() {
                Ok(text_val) => {
                    tracing::debug!(
                        "Successfully converted unknown type '{}' to string via raw access: {}",
                        column_type,
                        text_val
                    );
                    text_val.to_string()
                }
                Err(_) => {
                    // If raw access fails, try to get bytes and convert to UTF-8
                    match raw_value.as_bytes() {
                        Ok(bytes) => {
                            // Try UTF-8 conversion first
                            match String::from_utf8(bytes.to_vec()) {
                                Ok(string_val) => {
                                    tracing::debug!("Successfully converted unknown type '{}' to string via bytes: {}", column_type, string_val);
                                    string_val
                                }
                                Err(_) => {
                                    // If UTF-8 fails, provide hex representation for binary data
                                    let hex_repr = bytes
                                        .iter()
                                        .map(|b| format!("{:02x}", b))
                                        .collect::<String>();
                                    tracing::warn!("Unable to convert column type '{}' to valid UTF-8, showing hex: {}...", column_type, &hex_repr[..hex_repr.len().min(40)]);
                                    format!(
                                        "[binary data: {} bytes, starts with: {}]",
                                        bytes.len(),
                                        &hex_repr[..hex_repr.len().min(20)]
                                    )
                                }
                            }
                        }
                        Err(_) => {
                            tracing::warn!("Unable to access raw bytes for column type '{}' at index {}, falling back to NULL", column_type, column_index);
                            "NULL".to_string()
                        }
                    }
                }
            }
        } else {
            tracing::warn!(
                "Unmatched PostgreSQL column type '{}' at index {}, falling back to NULL",
                column_type,
                column_index
            );
            "NULL".to_string()
        }
    }

    /// Convert a PostgreSQL row value to string representation
    /// This method handles all PostgreSQL data types using column-type-first approach
    fn convert_row_value_to_string(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_types: &[String],
    ) -> String {
        // Get column type first for type-based routing
        let column_type = column_types
            .get(column_index)
            .map(|s| s.as_str())
            .unwrap_or("unknown");

        // 1. Handle NULL values immediately
        if Self::is_null_value(row, column_index) {
            return "NULL".to_string();
        }

        // 2. Route based on PostgreSQL type (column-type-first approach)
        match column_type {
            // Array types - priority handling
            ct if Self::is_array_type(ct) => self.handle_array_type(row, column_index, ct),

            // System catalog types
            "regclass" | "regproc" | "regtype" | "regnamespace" | "regrole" | "reglanguage"
            | "regconfig" | "regdictionary" | "oid" | "xid" | "cid" | "tid" => {
                self.handle_system_catalog_type(row, column_index, column_type)
            }

            // Numeric types with precision
            ct if Self::get_numeric_precision_type(ct).is_some() => {
                let base_type = Self::get_numeric_precision_type(ct).unwrap();
                self.handle_money_numeric_type(row, column_index, base_type)
            }

            // Standard PostgreSQL types - include both SQL names and internal type names
            "uuid" | "UUID" => self.handle_uuid_type(row, column_index, column_type),
            "money" => self.handle_money_numeric_type(row, column_index, column_type),
            "numeric" | "decimal" | "NUMERIC" => {
                if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
                    val.map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string())
                } else {
                    "NULL".to_string()
                }
            }
            "text" | "varchar" | "char" | "BPCHAR" | "CHAR" | "TEXT" | "NAME" => {
                self.handle_string_type(row, column_index, column_type)
            }
            "smallint" | "int2" | "INT2" => self.handle_i16_type(row, column_index, column_type),
            "integer" | "int" | "int4" | "INT4" => {
                self.handle_i32_type(row, column_index, column_type)
            }
            "bigint" | "int8" | "INT8" => self.handle_i64_type(row, column_index, column_type),
            "real" | "float4" | "FLOAT4" => self.handle_f32_type(row, column_index, column_type),
            "double precision" | "float8" | "FLOAT8" => {
                self.handle_f64_type(row, column_index, column_type)
            }
            "boolean" | "bool" | "BOOL" => self.handle_bool_type(row, column_index, column_type),

            // Timestamp types - include both SQL names and internal type names
            ct if ct.starts_with("timestamp") || ct == "TIMESTAMPTZ" || ct == "TIMESTAMP" => {
                self.handle_timestamp_type(row, column_index, ct)
            }
            ct if ct.starts_with("date") || ct == "DATE" => {
                self.handle_date_type(row, column_index, ct)
            }
            ct if ct.starts_with("time") || ct == "TIME" => {
                self.handle_time_type(row, column_index, ct)
            }

            // JSON types
            "json" | "jsonb" | "JSON" | "JSONB" => {
                self.handle_json_type(row, column_index, column_type)
            }

            // Unknown/custom types - use optimized raw value access
            _ => {
                tracing::warn!(
                    "Unknown column type '{}' falling back to raw value access",
                    column_type
                );
                self.handle_unknown_type(row, column_index, column_type)
            }
        }
    }
    /// Execute a query using a specific connection pool
    async fn execute_query_with_pool(
        &self,
        pool: &sqlx::PgPool,
        query: &str,
    ) -> Result<QueryResult> {
        // Try to execute as a query that returns rows
        match sqlx::query(query).fetch_all(pool).await {
            Ok(rows) => {
                if rows.is_empty() {
                    return Ok(QueryResult {
                        columns: vec![],
                        column_types: vec![],
                        rows: vec![],
                        rows_affected: 0,
                        query_text: None,
                        execution_time_ms: None,
                        is_error: false,
                        table_name: None,
                        primary_key_column: None,
                        connection_id: None,
                    });
                }

                // Extract column names and types from the first row
                let first_row = &rows[0];
                let columns: Vec<String> = first_row
                    .columns()
                    .iter()
                    .map(|col| col.name().to_string())
                    .collect();

                let column_types: Vec<String> = first_row
                    .columns()
                    .iter()
                    .map(|col| col.type_info().name().to_string())
                    .collect();

                // First pass: Process rows and collect OIDs from regclass columns
                let mut data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        (0..columns.len())
                            .map(|i| self.convert_row_value_to_string(row, i, &column_types))
                            .collect()
                    })
                    .collect();

                // Check if we have any regclass columns that need OID resolution
                let regclass_columns: Vec<usize> = column_types
                    .iter()
                    .enumerate()
                    .filter_map(|(i, col_type)| {
                        if col_type == "regclass" {
                            Some(i)
                        } else {
                            None
                        }
                    })
                    .collect();

                if !regclass_columns.is_empty() {
                    // Collect OIDs from regclass columns using raw bytes
                    let mut oids_to_resolve = Vec::new();
                    for (row_idx, row) in rows.iter().enumerate() {
                        for &col_idx in &regclass_columns {
                            if let Ok(raw_value) = row.try_get_raw(col_idx) {
                                if !raw_value.is_null() {
                                    // Use the same approach as handle_unknown_type with as_bytes()
                                    match raw_value.as_bytes() {
                                        Ok(bytes) => {
                                            // PostgreSQL OIDs are 4-byte integers in network byte order (big-endian)
                                            if bytes.len() >= 4 {
                                                let oid = i32::from_be_bytes([
                                                    bytes[0], bytes[1], bytes[2], bytes[3],
                                                ]);
                                                oids_to_resolve.push((row_idx, col_idx, oid));
                                            }
                                        }
                                        Err(_) => {
                                            // If we can't get bytes, we can't resolve this OID
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Resolve OIDs to table names in batch
                    if !oids_to_resolve.is_empty() {
                        let unique_oids: Vec<i32> = oids_to_resolve
                            .iter()
                            .map(|(_, _, oid)| *oid)
                            .collect::<std::collections::HashSet<_>>()
                            .into_iter()
                            .collect();

                        let oid_to_name = self.resolve_oids_to_names(pool, &unique_oids).await?;

                        // Update data rows with resolved names
                        for (row_idx, col_idx, oid) in oids_to_resolve {
                            if let Some(name) = oid_to_name.get(&oid) {
                                data_rows[row_idx][col_idx] = name.clone();
                            }
                        }
                    }
                }

                Ok(QueryResult {
                    columns,
                    column_types,
                    rows: data_rows,
                    rows_affected: rows.len().try_into().unwrap_or(0),
                    query_text: None,
                    execution_time_ms: None,
                    is_error: false,
                    table_name: None,
                    primary_key_column: None,
                    connection_id: None,
                })
            }
            Err(_e) => {
                // If it's not a SELECT query, try executing it as a statement
                let result = sqlx::query(query).execute(pool).await?;
                Ok(QueryResult {
                    columns: vec![],
                    column_types: vec![],
                    rows: vec![],
                    rows_affected: result.rows_affected(),
                    query_text: None,
                    execution_time_ms: None,
                    is_error: false,
                    table_name: None,
                    primary_key_column: None,
                    connection_id: None,
                })
            }
        }
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
            .execute_prepared_query(query, &[schema_filter.to_string()])
            .await?;
        let tables: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
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
            if let Ok(Some(oid_text)) = row.try_get::<Option<String>, _>(0) {
                if let Ok(oid_val) = oid_text.parse::<i32>() {
                    if let Ok(Some(name)) = row.try_get::<Option<String>, _>(1) {
                        oid_to_name.insert(oid_val, name);
                    }
                }
            }
        }

        Ok(oid_to_name)
    }
}

#[async_trait]
impl Connection for PostgresConnection {
    fn get_connection_key_str(&self) -> String {
        // Server-level connection key (no database)
        format!(
            "postgres:{}@{}:{}",
            self.server_key.username, self.server_key.host, self.server_key.port
        )
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }

    fn get_icon_name(&self) -> IconName {
        // Return DatabaseConnected when we have any active pools, otherwise Database
        // Since we can't do async in a sync trait method, use a simple heuristic
        IconName::DatabaseConnected
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    fn get_manager_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        tracing::info!(
            "Connecting to PostgreSQL server: {}@{}:{}",
            self.server_key.username,
            self.server_key.host,
            self.server_key.port
        );

        // Parse and validate the connection string to extract server details
        let key = PgConnectionKey::from_connection_string(connection_string)?;
        self.server_key = key.to_server_key();

        // SSH tunnel setup is now handled by DbService
        // The connection string received here already includes the tunnel port if SSH is used
        self.server_connection_string = connection_string.to_string();

        // Update display name
        self.display_name = Self::generate_server_display_name(&self.server_key);

        // Clear any existing pools (they will be recreated on demand)
        let mut pools = self.pools.write().await;
        pools.clear();
        drop(pools);

        tracing::info!("PostgreSQL server connection configured (pools will be created on demand)");
        tracing::info!("Connection string: {}", self.server_connection_string);
        Ok(())
    }

    async fn disconnect(&mut self) {
        tracing::info!(
            "Disconnecting from PostgreSQL server: {}",
            self.display_name
        );

        // Close all database connection pools
        let mut pools = self.pools.write().await;
        for (_, pool) in pools.drain() {
            pool.close().await;
        }
    }

    fn is_connected(&self) -> bool {
        // Since we can't use async in sync methods, use a simple heuristic
        // Assume connected if we have a display name
        !self.display_name.is_empty()
    }

    async fn ensure_connected(&mut self, connection_string: &str) -> Result<()> {
        if !self.is_connected() || !self.is_connection_healthy().await {
            tracing::info!("Reconnecting to PostgreSQL database");
            self.connect(connection_string).await?;
        }
        Ok(())
    }

    async fn execute_query(&self, query: &str, database_name: Option<&str>) -> Result<QueryResult> {
        tracing::debug!(
            "Executing PostgreSQL query: {} (database: {:?})",
            query,
            database_name
        );

        // If no database specified, we need to connect to a default database first
        // to get the list of available databases. We'll try common default databases.
        let target_database = if let Some(db) = database_name {
            db.to_string()
        } else {
            // Try to find an available database by testing common defaults
            self.get_available_database().await?
        };

        // Get or create connection pool for the specific database
        let pool = self
            .get_or_create_pool(&target_database)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to get connection pool for database '{}': {}",
                    target_database,
                    e
                )
            })?;

        // Execute the query using the database-specific pool
        let result = self
            .execute_query_with_pool(&pool, query)
            .await
            .map_err(|e| anyhow::anyhow!("PostgreSQL query execution failed: {}", e))?;

        tracing::debug!(
            "Query executed successfully on database '{}', {} rows returned",
            target_database,
            result.row_count()
        );

        Ok(result)
    }

    async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult> {
        tracing::debug!(
            "Executing prepared PostgreSQL query with {} parameters",
            parameters.len()
        );

        // For prepared queries, use the initial database or 'postgres' as fallback
        let database_name = self.initial_database.as_deref().unwrap_or("postgres");
        let pool = self.get_or_create_pool(database_name).await.map_err(|e| {
            anyhow::anyhow!("Failed to get connection pool for prepared query: {}", e)
        })?;

        // Build the query with parameter placeholders
        let mut query = sqlx::query(sql_template);

        // Add parameters to the query
        for param in parameters {
            query = query.bind(param);
        }

        // Try to execute as a query that returns rows
        match query.fetch_all(&pool).await {
            Ok(rows) => {
                if rows.is_empty() {
                    return Ok(QueryResult {
                        columns: vec![],
                        column_types: vec![],
                        rows: vec![],
                        rows_affected: 0,
                        query_text: Some(sql_template.to_string()),
                        execution_time_ms: None,
                        is_error: false,
                        table_name: None,
                        primary_key_column: None,
                        connection_id: None,
                    });
                }

                // Extract column names and types from the first row
                let first_row: &sqlx::postgres::PgRow = &rows[0];
                let columns: Vec<String> = first_row
                    .columns()
                    .iter()
                    .map(|col| col.name().to_string())
                    .collect();

                let column_types: Vec<String> = first_row
                    .columns()
                    .iter()
                    .map(|col| col.type_info().name().to_string())
                    .collect();

                // Extract row data using the reusable type conversion method
                let data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        (0..columns.len())
                            .map(|i| self.convert_row_value_to_string(row, i, &column_types))
                            .collect()
                    })
                    .collect();

                Ok(QueryResult {
                    columns,
                    column_types,
                    rows: data_rows,
                    rows_affected: 0,
                    query_text: Some(sql_template.to_string()),
                    execution_time_ms: None,
                    is_error: false,
                    table_name: None,
                    primary_key_column: None,
                    connection_id: None,
                })
            }
            Err(_e) => {
                // If it's not a SELECT query, try executing it as a statement
                // Need to recreate the query since it was consumed by fetch_all
                let mut statement_query = sqlx::query(sql_template);
                for param in parameters {
                    statement_query = statement_query.bind(param);
                }
                let result = statement_query.execute(&pool).await?;
                Ok(QueryResult {
                    columns: vec![],
                    column_types: vec![],
                    rows: vec![],
                    rows_affected: result.rows_affected(),
                    query_text: Some(sql_template.to_string()),
                    execution_time_ms: None,
                    is_error: false,
                    table_name: None,
                    primary_key_column: None,
                    connection_id: None,
                })
            }
        }
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        let result = self
            .execute_query(
                "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname",
                None,
            )
            .await?;
        let databases: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();
        Ok(databases)
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        let result = self
            .execute_query(
                "SELECT schema_name FROM information_schema.schemata WHERE schema_owner = 'pg_database_owner' ORDER BY schema_name",
                None,
            )
            .await?;
        let schemas: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        self.fetch_postgres_tables(schema).await
    }

    fn supports_schemas(&self) -> bool {
        true // PostgreSQL fully supports schemas
    }

    async fn get_primary_key_for_table(&self, table_name: &str) -> Result<Option<String>> {
        let query = "
            SELECT column_name
            FROM information_schema.table_constraints tc
            JOIN information_schema.key_column_usage kcu
                ON tc.constraint_name = kcu.constraint_name
                AND tc.table_schema = kcu.table_schema
            WHERE tc.constraint_type = 'PRIMARY KEY'
                AND tc.table_name = $1
                AND tc.table_schema = 'public'
        ";

        let result = self
            .execute_prepared_query(query, &[table_name.to_string()])
            .await?;

        if !result.rows.is_empty() {
            Ok(result.rows[0].first().cloned())
        } else {
            Ok(None)
        }
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        let schema_name = schema.unwrap_or("public");
        tracing::debug!(
            "Getting columns for PostgreSQL table '{}.{}",
            schema_name,
            table_name
        );

        let query = "
            SELECT
                column_name,
                data_type,
                is_nullable,
                column_default,
                character_maximum_length,
                numeric_precision,
                numeric_scale
            FROM information_schema.columns
            WHERE table_name = $1 AND table_schema = $2
            ORDER BY ordinal_position
        ";

        let result = self
            .execute_prepared_query(query, &[table_name.to_string(), schema_name.to_string()])
            .await?;

        let mut columns = Vec::new();
        for row in result.rows {
            if row.len() >= 7 {
                let column_name = &row[0];
                let data_type = &row[1];
                let is_nullable = &row[2]; // YES/NO
                let default_value = &row[3]; // Default value or NULL
                let max_length = &row[4]; // character_maximum_length
                                          // row[5] = numeric_precision, row[6] = numeric_scale (not used for now)

                // Check if it's a primary key
                let is_primary_key =
                    if let Ok(Some(pk)) = self.get_primary_key_for_table(table_name).await {
                        pk == *column_name
                    } else {
                        false
                    };

                let column_info = ColumnInfo {
                    name: column_name.clone(),
                    data_type: data_type.clone(),
                    is_nullable: is_nullable == "YES",
                    is_primary_key,
                    default_value: if default_value.is_empty() {
                        None
                    } else {
                        Some(default_value.clone())
                    },
                    character_maximum_length: max_length.parse().ok(),
                };
                columns.push(column_info);
            }
        }

        tracing::debug!(
            "Found {} columns for table '{}.{}",
            columns.len(),
            schema_name,
            table_name
        );
        Ok(columns)
    }

    async fn get_table_metadata(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<TableMetadata> {
        let schema_name = schema.unwrap_or("public");
        tracing::debug!(
            "Getting metadata for PostgreSQL table '{}.{}",
            schema_name,
            table_name
        );

        // Get basic table information
        let columns = self
            .get_columns_for_table(table_name, Some(schema_name))
            .await?;

        // Get row count
        let row_count = match self
            .execute_prepared_query(
                &format!(
                    "SELECT COUNT(*) FROM \"{}\".\"{}\"",
                    schema_name, table_name
                ),
                &[],
            )
            .await
        {
            Ok(count_result) if !count_result.rows.is_empty() => {
                count_result.rows[0][0].parse().ok()
            }
            _ => None,
        };

        // Extract primary key information
        let primary_keys: Vec<String> = columns
            .iter()
            .filter(|col| col.is_primary_key)
            .map(|col| col.name.clone())
            .collect();

        let mut metadata =
            TableMetadata::new(table_name.to_string(), Some(schema_name.to_string()));
        metadata.columns = columns;
        metadata.row_count = row_count;
        metadata.primary_keys = primary_keys;

        tracing::debug!(
            "Retrieved metadata for table '{}.{}': {} columns, {} PKs",
            schema_name,
            table_name,
            metadata.columns.len(),
            metadata.primary_keys.len()
        );

        Ok(metadata)
    }

    async fn execute_table_changes(
        &self,
        _changes: &[TableChangeOperation],
    ) -> Result<QueryResult> {
        // This is a placeholder - would need implementation of table operations for PostgreSQL
        Err(anyhow::anyhow!(
            "Table changes not yet implemented for PostgreSQL"
        ))
    }

    async fn get_database_name(&self) -> Result<Option<String>> {
        // For server-level connections, we don't have a specific database
        // Return None to indicate server-level connection
        Ok(None)
    }

    fn get_file_safe_name(&self) -> String {
        // Create a file-safe name from PostgreSQL server details
        let name = format!(
            "pg_{}_{}_{}",
            self.server_key.username, self.server_key.host, self.server_key.port
        );

        // Make it file-safe
        name.chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect()
    }

    async fn execute_query_stream_rows(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<
        (
            Vec<String>,
            Vec<String>,
            Box<dyn Stream<Item = Result<Vec<String>, anyhow::Error>> + Send + Unpin>,
        ),
        anyhow::Error,
    > {
        tracing::debug!(
            "Executing PostgreSQL streaming query: {} (database: {:?})",
            query,
            database_name
        );

        // Get the target database
        let target_database = if let Some(db) = database_name {
            db.to_string()
        } else {
            self.get_available_database().await?
        };

        // Get connection pool
        let pool = self
            .get_or_create_pool(&target_database)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to get connection pool for database '{}': {}",
                    target_database,
                    e
                )
            })?;

        // Use sqlx::query().fetch() for true streaming
        let rows_stream = sqlx::query(query).fetch(&pool);

        // We need to collect all rows first to get column information since sqlx streams
        // don't allow us to peek at the first row without consuming it
        let mut rows = Vec::new();
        let mut stream = rows_stream;
        let mut columns = Vec::new();
        let mut column_types = Vec::new();

        // Process the first row to get column information
        let mut first_row_data = None;
        while let Some(row_result) = stream.next().await {
            let row = row_result.map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?;

            if first_row_data.is_none() {
                // Extract column names and types from the first row
                columns = row
                    .columns()
                    .iter()
                    .map(|col| col.name().to_string())
                    .collect();
                column_types = row
                    .columns()
                    .iter()
                    .map(|col| col.type_info().name().to_string())
                    .collect();

                let row_data: Vec<String> = (0..columns.len())
                    .map(|i| self.convert_row_value_to_string(&row, i, &column_types))
                    .collect();

                first_row_data = Some(row_data.clone());
                rows.push(row_data);
            } else {
                // Process subsequent rows
                let row_data: Vec<String> = (0..columns.len())
                    .map(|i| self.convert_row_value_to_string(&row, i, &column_types))
                    .collect();
                rows.push(row_data);
            }
        }

        if rows.is_empty() {
            return Ok((vec![], vec![], Box::new(futures::stream::empty())));
        }

        // Create stream from collected rows
        let all_rows_stream = futures::stream::iter(rows.into_iter().map(Ok));

        Ok((columns, column_types, Box::new(all_rows_stream)))
    }

    fn get_ui_metadata(&self) -> ConnectionUIMetadata {
        ConnectionUIMetadata {
            display_name: self.display_name.clone(),
            file_safe_name: self.get_file_safe_name(),
            supports_schemas: self.supports_schemas(),
            icon_name: self.get_icon_name(),
        }
    }

    fn extract_table_name_from_query(&self, query: &str, alias: bool) -> Result<Option<String>> {
        tracing::debug!("Extracting table name from PostgreSQL query: {}", query);

        let extractor = PostgresTableExtractor::new();
        match extractor.extract_table(query, alias) {
            Ok(table_name) => {
                tracing::debug!("Successfully extracted table name: {}", table_name);
                Ok(Some(table_name))
            }
            Err(e) => {
                tracing::debug!("Could not extract table name from query: {}", e);
                Ok(None)
            }
        }
    }

    fn resolve_table_alias(&self, query: &str, alias: &str) -> Result<Option<String>> {
        tracing::debug!(
            "Resolving table alias '{}' from PostgreSQL query: {}",
            alias,
            query
        );

        let extractor = PostgresTableExtractor::new();
        match extractor.extract_table_aliases_with_names(query) {
            Ok(aliases) => {
                for (table_name, alias_name) in aliases {
                    if alias_name == alias {
                        tracing::debug!(
                            "Successfully resolved alias '{}' to table '{}'",
                            alias,
                            table_name
                        );
                        return Ok(Some(table_name));
                    }
                }
                tracing::debug!("Alias '{}' not found in query", alias);
                Ok(None)
            }
            Err(e) => {
                tracing::debug!("Could not resolve table alias from query: {}", e);
                Ok(None)
            }
        }
    }
}

// Implement Clone for PostgresConnection for use in async tasks
impl Clone for PostgresConnection {
    fn clone(&self) -> Self {
        Self {
            pools: Arc::clone(&self.pools),
            server_key: self.server_key.clone(),
            display_name: self.display_name.clone(),
            server_connection_string: self.server_connection_string.clone(),
            initial_database: self.initial_database.clone(),
            ssh_config: self.ssh_config.clone(),
            local_tunnel_port: self.local_tunnel_port,
        }
    }
}
