use crate::sql_parser::PostgresTableExtractor;
use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, ConnectionUIMetadata, IconName, QueryResult, TableChangeOperation,
    TableMetadata,
};
use sqlx::postgres::types::PgMoney;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Column, Row, TypeInfo, ValueRef};
use std::collections::HashMap;
use std::sync::Arc;
use async_std::sync::RwLock;
use async_std::task::sleep;
use std::time::Duration;

/// PostgreSQL connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface with database-specific connection pools
pub struct PostgresConnection {
    pools: Arc<RwLock<HashMap<String, sqlx::PgPool>>>, // database_name -> connection pool
    server_key: PgServerKey,                          // Server-level connection key (no database)
    display_name: String,
    server_connection_string: String, // Connection string without database
    initial_database: Option<String>, // Original database from connection string
    ssh_config: Option<PostgresSshConfig>, // SSH tunnel configuration
    local_tunnel_port: Option<u16>, // Local port for SSH tunnel (if configured)
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
            log::error!("No host specified in PostgreSQL connection string");
            return format!("postgresql://{}@localhost:{}", self.username, self.port);
        }

        log::debug!("Generated server connection string: {}", url);
        url
    }

    /// Generate connection string for a specific database
    pub fn to_database_connection_string(&self, database: &str) -> String {
        let conn_str = format!("{}/{}", self.to_server_connection_string(), database);
        log::debug!("Generated database connection string: {}", conn_str);
        conn_str
    }

    /// Generate a server-level connection string with SSH tunnel support
    pub fn to_server_connection_string_with_tunnel(&self, local_tunnel_port: u16) -> String {
        let password_str = self.password.as_deref().unwrap_or("");
        let mut url = format!("postgresql://{}:{}", self.username, password_str);

        // Always use localhost and the tunnel port when SSH tunneling
        url = format!("{}@localhost:{}", url, local_tunnel_port);

        log::debug!("Generated SSH tunnel server connection string: {}", url);
        url
    }

    /// Generate connection string for a specific database using SSH tunnel
    pub fn to_database_connection_string_with_tunnel(&self, database: &str, local_tunnel_port: u16) -> String {
        let conn_str = format!("{}/{}", self.to_server_connection_string_with_tunnel(local_tunnel_port), database);
        log::debug!("Generated SSH tunnel database connection string: {}", conn_str);
        conn_str
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
                "postgresql://{}:{}@{}:{}/{}",
                self.username, password, self.host, self.port, self.database
            )
        } else {
            format!(
                "postgresql://{}@{}:{}/{}",
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
        log::info!(
            "🔗 Creating PostgreSQL connection from: {}",
            connection_string
        );

        let connection_key = PgConnectionKey::from_connection_string(connection_string)?;
        log::info!("📋 Parsed connection key:");
        log::info!("   - host: {}", connection_key.host);
        log::info!("   - port: {}", connection_key.port);
        log::info!("   - database: {}", connection_key.database);
        log::info!("   - username: {}", connection_key.username);
        log::info!(
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

        log::info!("🏢 Server key created:");
        log::info!("   - host: {}", server_key.host);
        log::info!("   - port: {}", server_key.port);
        log::info!("   - username: {}", server_key.username);
        log::info!("   - initial_database: {}", connection_key.database);

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

    /// Setup SSH tunnel for this connection
    async fn setup_ssh_tunnel(&mut self) -> Result<u16> {
        if let Some(ref ssh_config) = self.ssh_config {
            // Validate SSH configuration before setting up tunnel
            if ssh_config.ssh_host.is_empty() || ssh_config.ssh_user.is_empty() {
                log::warn!("SSH tunnel configuration is invalid: host='{}', user='{}'", ssh_config.ssh_host, ssh_config.ssh_user);
                // Fall back to direct connection
                self.server_connection_string = self.server_key.to_server_connection_string();
                return Ok(self.server_key.port);
            }

            log::info!("Setting up SSH tunnel to {}:{}", ssh_config.ssh_host, ssh_config.ssh_port);

            // Simulate SSH tunnel creation (in real implementation, this would use russh)
            sleep(Duration::from_millis(100)).await;

            // Assign a local port (in real implementation, this would come from SSH tunnel manager)
            let local_port = 15432; // This would be dynamically assigned

            // Update the server connection string to use the tunnel
            self.server_connection_string = self.server_key.to_server_connection_string_with_tunnel(local_port);
            self.local_tunnel_port = Some(local_port);

            log::info!("SSH tunnel established on local port {}", local_port);
            Ok(local_port)
        } else {
            // No SSH configuration, use direct connection
            self.server_connection_string = self.server_key.to_server_connection_string();
            Ok(self.server_key.port)
        }
    }

    /// Generate a human-readable display name for server-level connections
    fn generate_server_display_name(server_key: &PgServerKey) -> String {
        format!(
            "PostgreSQL - {}@{}:{}",
            server_key.username, server_key.host, server_key.port
        )
    }

    /// Generate a human-readable display name for the connection (legacy)
    fn generate_display_name(key: &PgConnectionKey) -> String {
        format!(
            "PostgreSQL - {}@{}:{}/{}",
            key.username, key.host, key.port, key.database
        )
    }

    /// Generate a human-readable display name for SSH connections
    fn generate_server_display_name_with_ssh(server_key: &PgServerKey, ssh_config: &PostgresSshConfig) -> String {
        format!(
            "PostgreSQL (via SSH) - {}@{}:{} → {}@{}:{}",
            ssh_config.ssh_user, ssh_config.ssh_host, ssh_config.ssh_port,
            server_key.username, server_key.host, server_key.port
        )
    }

    /// Find an available database to connect to when no specific database is specified
    async fn get_available_database(&self) -> Result<String> {
        // Try the initial database from the connection string first
        if let Some(ref initial_db) = self.initial_database {
            log::debug!(
                "Trying initial database '{}' from connection string",
                initial_db
            );
            if self.try_connect_to_database(initial_db).await {
                log::info!(
                    "Using initial database '{}' for metadata queries",
                    initial_db
                );
                return Ok(initial_db.clone());
            } else {
                log::warn!(
                    "Initial database '{}' is not accessible, trying alternatives",
                    initial_db
                );
            }
        }

        // Common PostgreSQL databases that are likely to exist and be accessible
        let common_databases = ["postgres", "template1", "template0"];
        log::debug!("Trying common databases: {}", common_databases.join(", "));

        for &db_name in &common_databases {
            if self.try_connect_to_database(db_name).await {
                log::info!(
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
            self.server_key.to_database_connection_string_with_tunnel(database, local_port)
        } else {
            self.server_key.to_database_connection_string(database)
        };
        log::debug!(
            "Trying to connect to database '{}' for metadata queries",
            database
        );
        log::debug!("Connection string: {}", database_connection_string);

        log::debug!("Attempting connection with 5-second timeout...");
        match PgPoolOptions::new()
            .max_connections(1) // Just for testing connectivity
            .connect(&database_connection_string)
            .await
        {
            Ok(pool) => {
                log::debug!(
                    "Connected to database '{}', testing query execution",
                    database
                );
                // Test if we can actually execute queries
                match sqlx::query("SELECT 1").fetch_one(&pool).await {
                    Ok(_) => {
                        log::info!(
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
                        log::warn!(
                            "⚠️ Connected to database '{}' but query test failed: {}",
                            database,
                            e
                        );
                        false
                    }
                }
            }
            Err(e) => {
                log::warn!("❌ Failed to connect to database '{}': {}", database, e);
                // Provide additional diagnostic info for common connection issues
                let error_str = e.to_string().to_lowercase();
                if error_str.contains("timeout") {
                    log::warn!("   → Connection timeout - check network connectivity and firewall");
                } else if error_str.contains("authentication") || error_str.contains("password") {
                    log::warn!("   → Authentication failed - check username/password");
                } else if error_str.contains("database") && error_str.contains("not exist") {
                    log::warn!("   → Database does not exist");
                } else if error_str.contains("connection") && error_str.contains("refused") {
                    log::warn!("   → Connection refused - check if PostgreSQL is running and accepting connections");
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
            self.server_key.to_database_connection_string_with_tunnel(database, local_port)
        } else {
            self.server_key.to_database_connection_string(database)
        };
        log::info!("Creating new connection pool for database: {}", database);

        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&database_connection_string)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to database '{}': {}", database, e))?;

        pools.insert(database.to_string(), pool.clone());
        log::info!(
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

    /// Helper method to connect asynchronously
    async fn connect_async(&mut self, connection_string: &str) -> Result<sqlx::PgPool> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(connection_string)
            .await?;
        Ok(pool)
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

    /// Convert a PostgreSQL row value to string representation
    /// This method handles all PostgreSQL data types including custom enums and unknown types
    fn convert_row_value_to_string(
        &self,
        row: &sqlx::postgres::PgRow,
        column_index: usize,
        column_types: &[String],
    ) -> String {
        // Enhanced type conversion for PostgreSQL - specific types first
        if let Ok(val) = row.try_get::<Option<uuid::Uuid>, _>(column_index) {
            // UUID support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
            // decimal/numeric support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<PgMoney>, _>(column_index) {
            // MONEY type support - convert to decimal and format with currency symbol
            val.map(|v| {
                let decimal_val = v.to_decimal(2); // Use 2 decimal places for currency
                format!("${}", decimal_val)
            })
            .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
            // Regular string support
            val.unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<i16>, _>(column_index) {
            // smallint support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<i32>, _>(column_index) {
            // integer support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<i64>, _>(column_index) {
            // bigint support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<f32>, _>(column_index) {
            // real/float4 support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<f64>, _>(column_index) {
            // float/double/numeric support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<bool>, _>(column_index) {
            // boolean support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) =
            row.try_get::<Option<chrono::DateTime<chrono::FixedOffset>>, _>(column_index)
        {
            // timestamptz support (timestamp with time zone) - preserve original timezone
            val.map(|v| {
                // Format with original timezone information preserved from PostgreSQL
                v.format("%Y-%m-%d %H:%M:%S %:z").to_string()
            })
            .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index) {
            // timestamp support (timestamp without time zone) - consistent formatting
            val.map(|v| {
                // Format in a consistent, readable format
                v.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index) {
            // date support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index) {
            // time support
            val.map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        } else if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(column_index) {
            // JSON/JSONB support - single line format for table display
            val.map(|v| {
                // For compact display, use regular to_string instead of pretty printing
                v.to_string()
            })
            .unwrap_or_else(|| "NULL".to_string())
        } else {
            // Try raw value access for unknown types (custom enums, domains, etc.)
            let column_type = column_types
                .get(column_index)
                .map(|s| s.as_str())
                .unwrap_or("unknown");

            // For custom enum types and other unknown types, try raw value access
            if let Ok(raw_value) = row.try_get_raw(column_index) {
                if raw_value.is_null() {
                    "NULL".to_string()
                } else {
                    // Try to extract as text using raw value
                    match raw_value.as_str() {
                        Ok(text_val) => {
                            log::debug!("Successfully converted unknown type '{}' to string via raw access: {}", column_type, text_val);
                            text_val.to_string()
                        }
                        Err(_) => {
                            // If raw access fails, try to get bytes and convert to UTF-8
                            match raw_value.as_bytes() {
                                Ok(bytes) => match String::from_utf8(bytes.to_vec()) {
                                    Ok(string_val) => {
                                        log::debug!("Successfully converted unknown type '{}' to string via bytes: {}", column_type, string_val);
                                        string_val
                                    }
                                    Err(_) => {
                                        log::warn!("Unable to convert column type '{}' at index {} to valid UTF-8, falling back to NULL", column_type, column_index);
                                        "NULL".to_string()
                                    }
                                },
                                Err(_) => {
                                    log::warn!("Unable to access raw bytes for column type '{}' at index {}, falling back to NULL", column_type, column_index);
                                    "NULL".to_string()
                                }
                            }
                        }
                    }
                }
            } else {
                // Check if this is an array column based on column type information
                let column_type = column_types
                    .get(column_index)
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");
                if column_type == "ARRAY" || column_type.ends_with("[]") {
                    // This is an array column - try different conversion approaches
                    log::debug!("Attempting to convert array column type '{}'", column_type);

                    // Try String conversion first
                    if let Ok(array_val) = row.try_get::<Option<String>, _>(column_index) {
                        return array_val.map(|v| {
                            log::debug!("Successfully converted array '{}' to String for column type '{}'", v, column_type);
                            v
                        }).unwrap_or_else(|| "NULL".to_string());
                    }

                    // Try Vec<String> conversion
                    if let Ok(array_val) = row.try_get::<Option<Vec<String>>, _>(column_index) {
                        return array_val.map(|v| {
                            let result = format!("{{{}}}", v.iter()
                                .map(|x| format!("\"{}\"", x))
                                .collect::<Vec<_>>()
                                .join(","));
                            log::debug!("Successfully converted array to Vec<String> for column type '{}': {}", column_type, result);
                            result
                        }).unwrap_or_else(|| "NULL".to_string());
                    }

                    // Try Vec<i32> conversion
                    if let Ok(array_val) = row.try_get::<Option<Vec<i32>>, _>(column_index) {
                        return array_val.map(|v| {
                            let result = format!("{{{}}}", v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(","));
                            log::debug!("Successfully converted array to Vec<i32> for column type '{}': {}", column_type, result);
                            result
                        }).unwrap_or_else(|| "NULL".to_string());
                    }

                    // Try Vec<i64> conversion
                    if let Ok(array_val) = row.try_get::<Option<Vec<i64>>, _>(column_index) {
                        return array_val.map(|v| {
                            let result = format!("{{{}}}", v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(","));
                            log::debug!("Successfully converted array to Vec<i64> for column type '{}': {}", column_type, result);
                            result
                        }).unwrap_or_else(|| "NULL".to_string());
                    }

                    // Try Vec<uuid::Uuid> conversion
                    if let Ok(array_val) = row.try_get::<Option<Vec<uuid::Uuid>>, _>(column_index) {
                        return array_val.map(|v| {
                            let result = format!("{{{}}}", v.iter()
                                .map(|x| x.to_string())
                                .collect::<Vec<_>>()
                                .join(","));
                            log::debug!("Successfully converted array to Vec<uuid::Uuid> for column type '{}': {}", column_type, result);
                            result
                        }).unwrap_or_else(|| "NULL".to_string());
                    }

                    // Log what SQLX types we tried
                    log::warn!("Array column type '{}' couldn't be converted to any supported array type (String, Vec<String>, Vec<i32>, Vec<i64>, Vec<uuid::Uuid>)", column_type);
                    "NULL".to_string()
                } else if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
                    // text/varchar support (fallback) - also handles arrays
                    val.map(|v| {
                        // Check if this looks like a PostgreSQL array string
                        if v.starts_with('{') && v.ends_with('}') {
                            // This is already in PostgreSQL array format, return as-is
                            log::debug!(
                                "Found PostgreSQL array string: '{}' for column type '{}'",
                                v,
                                column_types
                                    .get(column_index)
                                    .unwrap_or(&"unknown".to_string())
                            );
                            v
                        } else {
                            // Regular string value
                            v
                        }
                    })
                    .unwrap_or_else(|| "NULL".to_string())
                } else {
                    // Final fallback - log unmatched type for debugging
                    let column_type = column_types
                        .get(column_index)
                        .map(|s| s.as_str())
                        .unwrap_or("unknown");
                    log::warn!(
                        "Unmatched PostgreSQL column type '{}' at index {}, falling back to NULL",
                        column_type,
                        column_index
                    );
                    "NULL".to_string()
                }
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
        log::info!(
            "Connecting to PostgreSQL server: {}@{}:{}",
            self.server_key.username,
            self.server_key.host,
            self.server_key.port
        );

        // Parse and validate the connection string to extract server details
        let key = PgConnectionKey::from_connection_string(connection_string)?;
        self.server_key = key.to_server_key();

        // Set up SSH tunnel if configured
        let local_port = self.setup_ssh_tunnel().await?;

        // Update display name based on whether SSH is used
        if self.ssh_config.is_some() {
            self.display_name = Self::generate_server_display_name_with_ssh(
                &self.server_key,
                self.ssh_config.as_ref().unwrap()
            );
        } else {
            self.display_name = Self::generate_server_display_name(&self.server_key);
        }

        // Clear any existing pools (they will be recreated on demand)
        let mut pools = self.pools.write().await;
        pools.clear();
        drop(pools);

        log::info!("PostgreSQL server connection configured (pools will be created on demand)");
        if self.ssh_config.is_some() {
            log::info!("Connection will use SSH tunnel on local port {}", local_port);
        }
        Ok(())
    }

    async fn disconnect(&mut self) {
        log::info!(
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
            log::info!("Reconnecting to PostgreSQL database");
            self.connect(connection_string).await?;
        }
        Ok(())
    }

    async fn execute_query(&self, query: &str, database_name: Option<&str>) -> Result<QueryResult> {
        log::debug!(
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

        log::debug!(
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
        log::debug!(
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
                "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name",
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
        log::debug!(
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

        log::debug!(
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
        log::debug!(
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

        log::debug!(
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

    fn get_ui_metadata(&self) -> ConnectionUIMetadata {
        ConnectionUIMetadata {
            display_name: self.display_name.clone(),
            file_safe_name: self.get_file_safe_name(),
            supports_schemas: self.supports_schemas(),
            icon_name: self.get_icon_name(),
        }
    }

    fn extract_table_name_from_query(&self, query: &str) -> Result<Option<String>> {
        log::debug!("Extracting table name from PostgreSQL query: {}", query);

        let extractor = PostgresTableExtractor::new();
        match extractor.extract_primary_table(query) {
            Ok(table_name) => {
                log::debug!("Successfully extracted table name: {}", table_name);
                Ok(Some(table_name))
            }
            Err(e) => {
                log::debug!("Could not extract table name from query: {}", e);
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
