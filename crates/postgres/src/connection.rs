use crate::sql_parser::PostgresTableExtractor;
use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    connection_trait::ColumnType, connection_trait::ForeignKeyInfo, ColumnInfo, Connection,
    QueryResult,
};
use futures::{Stream, StreamExt};
use smol::lock::RwLock;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Column, Row, TypeInfo, ValueRef};
use std::collections::HashMap;
use std::sync::Arc;

/// Typed parameter for PostgreSQL queries
#[derive(Debug, Clone)]
pub enum QueryParam {
    String(String),
    StringArray(Vec<String>),
    I64(i64),
    I32(i32),
    F64(f64),
    Bool(bool),
}

impl From<String> for QueryParam {
    fn from(s: String) -> Self {
        QueryParam::String(s)
    }
}

impl From<&str> for QueryParam {
    fn from(s: &str) -> Self {
        QueryParam::String(s.to_string())
    }
}

impl From<Vec<String>> for QueryParam {
    fn from(v: Vec<String>) -> Self {
        QueryParam::StringArray(v)
    }
}

impl From<i64> for QueryParam {
    fn from(n: i64) -> Self {
        QueryParam::I64(n)
    }
}

impl From<i32> for QueryParam {
    fn from(n: i32) -> Self {
        QueryParam::I32(n)
    }
}

impl From<f64> for QueryParam {
    fn from(n: f64) -> Self {
        QueryParam::F64(n)
    }
}

impl From<bool> for QueryParam {
    fn from(b: bool) -> Self {
        QueryParam::Bool(b)
    }
}

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

    /// Get or create a connection pool for a specific database
    pub(crate) async fn get_or_create_pool(&self, database: &str) -> Result<sqlx::PgPool> {
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

    /// Extract the base type from an array type (e.g., "TEXT[]" -> Some("TEXT"))
    fn extract_base_array_type(array_type: &str) -> Option<&str> {
        array_type.strip_suffix("[]")
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
        column_types: &[ColumnType],
    ) -> String {
        // Get column type first for type-based routing
        let column_type = column_types
            .get(column_index)
            .copied()
            .unwrap_or(ColumnType::Unknown);

        // 1. Handle NULL values immediately
        if Self::is_null_value(row, column_index) {
            return "NULL".to_string();
        }

        // 2. Route based on PostgreSQL type (column-type-first approach)
        match column_type {
            // Array types - priority handling
            ColumnType::Array => {
                // For arrays, we need the original type name from sqlx
                // Get the raw type info to determine the array element type
                let type_name = if let Ok(col) = row.try_get_raw(column_index) {
                    col.type_info().name().to_string()
                } else {
                    "array".to_string()
                };
                self.handle_array_type(row, column_index, &type_name)
            }

            // Integer types
            ColumnType::Integer | ColumnType::UnsignedInteger => {
                // Try different integer sizes
                if let Ok(val) = row.try_get::<Option<i16>, _>(column_index) {
                    val.map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string())
                } else if let Ok(val) = row.try_get::<Option<i32>, _>(column_index) {
                    val.map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string())
                } else if let Ok(val) = row.try_get::<Option<i64>, _>(column_index) {
                    val.map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string())
                } else {
                    "NULL".to_string()
                }
            }

            // Numeric types
            ColumnType::Numeric => {
                if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
                    val.map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string())
                } else if let Ok(val) = row.try_get::<Option<f32>, _>(column_index) {
                    val.map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string())
                } else if let Ok(val) = row.try_get::<Option<f64>, _>(column_index) {
                    val.map(|v| v.to_string())
                        .unwrap_or_else(|| "NULL".to_string())
                } else {
                    "NULL".to_string()
                }
            }

            // Boolean type
            ColumnType::Boolean => self.handle_bool_type(row, column_index, ""),

            // Text type
            ColumnType::Text => self.handle_string_type(row, column_index, ""),

            // DateTime types
            ColumnType::DateTime => self.handle_timestamp_type(row, column_index, ""),

            // UUID type
            ColumnType::Uuid => self.handle_uuid_type(row, column_index, ""),

            // JSON types
            ColumnType::Json => self.handle_json_type(row, column_index, ""),

            // Binary type
            ColumnType::Binary => self.handle_unknown_type(row, column_index, "bytea"),

            // Unknown/custom types - use optimized raw value access
            ColumnType::Unknown => {
                tracing::warn!("Unknown column type falling back to raw value access");
                self.handle_unknown_type(row, column_index, "unknown")
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
            .execute_query(
                query,
                self.initial_database.as_deref(),
                Some(&[schema_filter.to_string()]),
            )
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

impl PostgresConnection {
    /// Map PostgreSQL type name to ColumnType enum
    fn map_postgres_type(type_name: &str) -> ColumnType {
        match type_name.to_lowercase().as_str() {
            "smallint" | "int2" | "int" | "int4" | "integer" | "bigint" | "int8" | "serial"
            | "bigserial" => ColumnType::Integer,
            "real" | "float4" | "double precision" | "float8" | "numeric" | "decimal" | "money" => {
                ColumnType::Numeric
            }
            "boolean" | "bool" => ColumnType::Boolean,
            "text" | "varchar" | "character varying" | "char" | "bpchar" | "name" => {
                ColumnType::Text
            }
            "timestamp" | "timestamptz" | "date" | "time" | "timetz" | "interval" => {
                ColumnType::DateTime
            }
            "uuid" => ColumnType::Uuid,
            "json" | "jsonb" => ColumnType::Json,
            "array" | _ if type_name.to_lowercase().ends_with("[]") => ColumnType::Array,
            "bytea" => ColumnType::Binary,
            _ => ColumnType::Unknown,
        }
    }

    /// Helper method to execute a query with parameters
    pub(crate) async fn execute_query_with_params(
        &self,
        pool: &sqlx::PgPool,
        sql_template: &str,
        parameters: &[QueryParam],
    ) -> Result<QueryResult> {
        use sqlx::Either;

        tracing::debug!(
            "Executing PostgreSQL query with {} parameters",
            parameters.len()
        );

        // Use raw_sql for queries without parameters, otherwise use query with bindings
        let mut results = if parameters.is_empty() {
            sqlx::raw_sql(sql_template).fetch_many(pool)
        } else {
            let mut query = sqlx::query(sql_template);
            for param in parameters {
                query = match param {
                    QueryParam::String(s) => query.bind(s.as_str()),
                    QueryParam::StringArray(arr) => query.bind(arr.as_slice()),
                    QueryParam::I64(n) => query.bind(*n),
                    QueryParam::I32(n) => query.bind(*n),
                    QueryParam::F64(n) => query.bind(*n),
                    QueryParam::Bool(b) => query.bind(*b),
                };
            }
            #[allow(deprecated)]
            query.fetch_many(pool)
        };

        let mut columns: Vec<String> = Vec::new();
        let mut column_types: Vec<ColumnType> = Vec::new();
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut rows_affected: u64 = 0;

        // Track OIDs that need resolution: (row_idx, col_idx, oid_value)
        // Only store the minimal data needed instead of full raw rows
        let mut oids_to_resolve: Vec<(usize, usize, i32)> = Vec::new();

        while let Some(result) = results.next().await {
            match result? {
                Either::Left(execution_result) => {
                    rows_affected += execution_result.rows_affected();
                }
                Either::Right(row) => {
                    // Extract column info from the first row
                    if columns.is_empty() {
                        columns = row
                            .columns()
                            .iter()
                            .map(|col| col.name().to_string())
                            .collect();

                        column_types = row
                            .columns()
                            .iter()
                            .map(|col| Self::map_postgres_type(col.type_info().name()))
                            .collect();
                    }

                    // Convert row to strings immediately to avoid memory doubling
                    let row_idx = rows.len();
                    let row_data: Vec<String> = (0..columns.len())
                        .map(|i| self.convert_row_value_to_string(&row, i, &column_types))
                        .collect();

                    // Collect OIDs from Unknown (regclass) columns for later resolution
                    // We do this before pushing the row so we have the current row_idx
                    for (col_idx, col_type) in column_types.iter().enumerate() {
                        if *col_type == ColumnType::Unknown {
                            if let Ok(raw_value) = row.try_get_raw(col_idx) {
                                if !raw_value.is_null() {
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

                    rows.push(row_data);
                }
            }
        }

        // Resolve OIDs to table names if any were collected
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
                    rows[row_idx][col_idx] = name.clone();
                }
            }
        }

        Ok(QueryResult {
            columns,
            column_types,
            rows,
            rows_affected,
            query_text: Some(sql_template.to_string()),
            execution_time_ms: None,
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        })
    }
}

#[async_trait]
impl Connection for PostgresConnection {
    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
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

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let database_name = database_name.ok_or(anyhow::anyhow!("missing database"))?;

        // Get or create connection pool for the specific database
        let pool = self.get_or_create_pool(database_name).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to get connection pool for database '{}': {}",
                database_name,
                e
            )
        })?;

        // Convert String parameters to QueryParam
        let params: Vec<QueryParam> = parameters
            .unwrap_or(&[])
            .iter()
            .map(|s| QueryParam::String(s.clone()))
            .collect();

        let result = self
            .execute_query_with_params(&pool, query, &params)
            .await
            .map_err(|e| anyhow::anyhow!("PostgreSQL query execution failed: {}", e))?;

        Ok(result)
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        let result = self
            .execute_query(
                "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname",
                self.initial_database.as_deref(),
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
                self.initial_database.as_deref(),
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

        // Single query to get columns with PK and FK information
        let query = "
            WITH foreign_keys AS (
                SELECT
                    conname,
                    conrelid,
                    confrelid,
                    unnest(conkey)  AS conkey,
                    unnest(confkey) AS confkey
                FROM pg_constraint
                WHERE contype = 'f' AND conrelid::regclass = $1::regclass
            )
            SELECT
                c.column_name,
                c.data_type,
                c.is_nullable,
                c.column_default,
                c.character_maximum_length,
                COALESCE(pk.column_name, '') AS is_primary_key,
                COALESCE(fk.foreign_table_name::text, '') AS fk_table,
                COALESCE(fk.foreign_column_name::text, '') AS fk_column,
                fk.constraint_name
            FROM information_schema.columns c
            LEFT JOIN (
                SELECT a.attname AS column_name
                FROM   pg_index i 
                JOIN   pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
                WHERE  i.indrelid = $1::regclass
                AND    i.indisprimary
            ) pk ON pk.column_name = c.column_name
            LEFT JOIN (
                SELECT
                    a.attname AS column_name,
                    fk.confrelid::regclass  AS foreign_table_name,
                    af.attname AS foreign_column_name,
                    fk.conname AS constraint_name
                FROM foreign_keys fk
                JOIN pg_attribute af ON af.attnum = fk.confkey AND af.attrelid = fk.confrelid
                JOIN pg_attribute a ON a.attnum = conkey AND a.attrelid = fk.conrelid
            ) fk ON fk.column_name = c.column_name
            WHERE c.table_name = $1 AND c.table_schema = $2
            ORDER BY c.ordinal_position
        ";

        let result = self
            .execute_query(
                query,
                self.initial_database.as_deref(),
                Some(&[table_name.to_string(), schema_name.to_string()]),
            )
            .await?;

        let mut columns = Vec::new();
        for row in result.rows {
            if row.len() >= 9 {
                let column_name = &row[0];
                let data_type = &row[1];
                let is_nullable = &row[2];
                let default_value = &row[3];
                let max_length = &row[4];
                let is_primary_key = &row[5]; // PK column name or empty
                let fk_table = &row[6];
                let fk_column = &row[7];
                let fk_constraint = &row[8];

                // Only create ForeignKeyInfo if we have actual FK values
                let foreign_key = match (fk_table.as_str(), fk_column.as_str()) {
                    ("", "") | (_, "") => None,
                    (table, column) => Some(ForeignKeyInfo {
                        foreign_table_name: table.to_string(),
                        foreign_column_name: column.to_string(),
                        constraint_name: if fk_constraint.is_empty() {
                            None
                        } else {
                            Some(fk_constraint.clone())
                        },
                    }),
                };

                columns.push(ColumnInfo {
                    name: column_name.clone(),
                    data_type: data_type.clone(),
                    is_nullable: is_nullable == "YES",
                    is_primary_key: !is_primary_key.is_empty(),
                    default_value: if default_value.is_empty() {
                        None
                    } else {
                        Some(default_value.clone())
                    },
                    character_maximum_length: max_length.parse().ok(),
                    foreign_key,
                });
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

    async fn execute_query_stream_rows(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<
        (
            Vec<String>,
            Vec<ColumnType>,
            Box<dyn Stream<Item = Result<Vec<String>, anyhow::Error>> + Send + Unpin>,
        ),
        anyhow::Error,
    > {
        let database_name = database_name.ok_or(anyhow::anyhow!("missing database"))?;

        // Get connection pool
        let pool = self.get_or_create_pool(database_name).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to get connection pool for database '{}': {}",
                database_name,
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
                    .map(|col| Self::map_postgres_type(col.type_info().name()))
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

    async fn get_database_schema_paginated(
        &self,
        database_name: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<blanco_core::connection_trait::DatabaseSchemaResult> {
        let limit = limit.unwrap_or(20).min(100); // Default 20, max 100
        let offset = offset.unwrap_or(0);

        let tables = self
            .get_schema_paginated(database_name, table_names, limit, offset)
            .await?;
        let table_count = tables.len();

        Ok(blanco_core::connection_trait::DatabaseSchemaResult {
            connection_type: self.get_connection_type().to_string(),
            display_name: self.get_display_name(),
            tables,
            pagination: blanco_core::connection_trait::PaginationInfo {
                limit: Some(limit),
                offset: Some(offset),
                has_more: table_count == limit as usize,
            },
        })
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
