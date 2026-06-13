mod connection_impl;

use anyhow::Result;
use blanco_core::connection_trait::ColumnType;
use smol::lock::RwLock;
use sqlx::Row;
use sqlx::mysql::MySqlPoolOptions;
use std::collections::HashMap;
use std::sync::Arc;

/// MySQL connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface with database-specific connection pools
pub struct MysqlConnection {
    pools: Arc<RwLock<HashMap<String, sqlx::MySqlPool>>>, // database_name -> connection pool
    host: String,
    port: u16,
    username: String,
    password: Option<String>,
    display_name: String,
    server_connection_string: String, // Connection string without database
    initial_database: Option<String>, // Original database from connection string
}

/// Connection key for MySQL connections
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MysqlConnectionKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
}

impl MysqlConnectionKey {
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

    /// Parse connection string from URL format
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let url = url::Url::parse(connection_string)?;

        if url.scheme() != "mysql" {
            return Err(anyhow::anyhow!(
                "Invalid MySQL connection string. Expected mysql:// scheme"
            ));
        }

        let host = url.host_str().unwrap_or("localhost").to_string();
        let port = url.port().unwrap_or(3306);
        let username = url.username().to_string();
        let password = url.password().map(|p| p.to_string());

        // Get database from path (remove leading /)
        let database = if !url.path().is_empty() {
            url.path().trim_start_matches('/').to_string()
        } else {
            return Err(anyhow::anyhow!(
                "Database name is required in connection string"
            ));
        };

        if username.is_empty() {
            return Err(anyhow::anyhow!("Username is required in connection string"));
        }

        if database.is_empty() {
            return Err(anyhow::anyhow!(
                "Database name is required in connection string"
            ));
        }

        Ok(MysqlConnectionKey {
            host,
            port,
            database,
            username,
            password,
        })
    }

    pub fn to_connection_string(&self) -> String {
        if let Some(password) = &self.password {
            format!(
                "mysql://{}:{}@{}:{}/{}",
                self.username, password, self.host, self.port, self.database
            )
        } else {
            format!(
                "mysql://{}@{}:{}/{}",
                self.username, self.host, self.port, self.database
            )
        }
    }

    /// Generate server connection string (without database)
    pub fn to_server_connection_string(&self) -> String {
        if let Some(password) = &self.password {
            format!(
                "mysql://{}:{}@{}:{}",
                self.username, password, self.host, self.port
            )
        } else {
            format!("mysql://{}@{}:{}", self.username, self.host, self.port)
        }
    }

    /// Generate connection string for a specific database
    pub fn to_database_connection_string(&self, database: &str) -> String {
        if let Some(password) = &self.password {
            format!(
                "mysql://{}:{}@{}:{}/{}",
                self.username, password, self.host, self.port, database
            )
        } else {
            format!(
                "mysql://{}@{}:{}/{}",
                self.username, self.host, self.port, database
            )
        }
    }
}

impl MysqlConnection {
    /// Map MySQL type name to ColumnType enum
    fn map_mysql_type(type_name: &str) -> ColumnType {
        let type_lower = type_name.to_lowercase();

        // Check for unsigned integer types first
        if type_lower.ends_with("unsigned") {
            let base_type = type_lower.trim_end_matches("unsigned").trim();
            return match base_type {
                "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => {
                    ColumnType::UnsignedInteger
                }
                _ => ColumnType::Unknown,
            };
        }

        match type_lower.as_str() {
            "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => ColumnType::Integer,
            "float" | "double" | "decimal" | "numeric" => ColumnType::Numeric,
            "char" | "varchar" | "text" => ColumnType::Text,
            "date" | "datetime" | "timestamp" | "time" | "year" => ColumnType::DateTime,
            "json" => ColumnType::Json,
            "binary" | "varbinary" | "blob" => ColumnType::Binary,
            "boolean" => ColumnType::Boolean,
            _ => ColumnType::Unknown,
        }
    }

    /// Create a new MySQL connection from a connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let connection_key = MysqlConnectionKey::from_connection_string(connection_string)?;
        let display_name = Self::generate_server_display_name(
            &connection_key.username,
            &connection_key.host,
            connection_key.port,
        );
        let server_connection_string = connection_key.to_server_connection_string();

        Ok(Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            host: connection_key.host,
            port: connection_key.port,
            username: connection_key.username.clone(),
            password: connection_key.password.clone(),
            display_name,
            server_connection_string,
            initial_database: Some(connection_key.database),
        })
    }

    fn generate_server_display_name(username: &str, host: &str, port: u16) -> String {
        format!("MySQL - {}@{}:{}", username, host, port)
    }

    /// Generate connection string for a specific database
    fn generate_database_connection_string(&self, database: &str) -> String {
        if let Some(password) = &self.password {
            format!(
                "mysql://{}:{}@{}:{}/{}",
                self.username, password, self.host, self.port, database
            )
        } else {
            format!(
                "mysql://{}@{}:{}/{}",
                self.username, self.host, self.port, database
            )
        }
    }

    /// Get or create a connection pool for the specified database
    pub async fn get_or_create_pool(&self, database_name: &str) -> Result<sqlx::MySqlPool> {
        let pools = self.pools.read().await;

        if let Some(pool) = pools.get(database_name) {
            return Ok(pool.clone());
        }

        // Release the read lock before acquiring write lock
        drop(pools);

        let mut pools = self.pools.write().await;

        // Check again in case another thread created it while we were waiting
        if let Some(pool) = pools.get(database_name) {
            return Ok(pool.clone());
        }

        let connection_string = self.generate_database_connection_string(database_name);
        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            // Bound how long acquiring (and therefore establishing) a connection
            // may take so an unreachable host fails fast instead of hanging.
            .acquire_timeout(blanco_core::connect_timeout())
            .connect(&connection_string)
            .await
            .map_err(blanco_core::tag_sqlx)?;

        pools.insert(database_name.to_string(), pool.clone());

        Ok(pool)
    }

    /// Get MySQL database name from connection string
    pub fn get_database_name_from_connection_string(connection_string: &str) -> Result<String> {
        let url = url::Url::parse(connection_string)?;

        // Get database from path (remove leading /)
        let database = if !url.path().is_empty() {
            url.path().trim_start_matches('/').to_string()
        } else {
            return Err(anyhow::anyhow!(
                "No database name found in connection string"
            ));
        };

        if database.is_empty() {
            return Err(anyhow::anyhow!(
                "Database name is required in connection string"
            ));
        }

        Ok(database)
    }

    /// Handle MySQL integer types using the raw type name (already uppercased)
    fn handle_integer_type(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column_index: usize,
        raw_type: &str,
    ) -> Option<String> {
        // Check for TINYINT(1) which is often used for booleans
        if raw_type.contains("TINYINT") {
            // Check if it's TINYINT(1) - boolean representation
            if (raw_type.contains("TINYINT(1)") || raw_type == "TINYINT")
                && let Ok(Some(v)) = row.try_get::<Option<i8>, _>(column_index)
            {
                if v == 0 || v == 1 {
                    return Some(if v == 1 { "true" } else { "false" }.to_string());
                }
                return Some(v.to_string());
            }
            if let Ok(Some(v)) = row.try_get::<Option<i8>, _>(column_index) {
                return Some(v.to_string());
            }
        }

        // SMALLINT
        if raw_type.contains("SMALLINT")
            && let Ok(Some(v)) = row.try_get::<Option<i16>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // MEDIUMINT
        if raw_type.contains("MEDIUMINT")
            && let Ok(Some(v)) = row.try_get::<Option<i32>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // INT, INTEGER
        if raw_type.contains("INT")
            && !raw_type.contains("TINYINT")
            && !raw_type.contains("SMALLINT")
            && !raw_type.contains("MEDIUMINT")
            && !raw_type.contains("BIGINT")
            && let Ok(Some(v)) = row.try_get::<Option<i32>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // BIGINT
        if raw_type.contains("BIGINT")
            && let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // Fallback: try i64
        if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
            return Some(v.to_string());
        }
        None
    }

    /// Handle MySQL unsigned integer types (raw_type already uppercased)
    fn handle_unsigned_integer_type(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column_index: usize,
        raw_type: &str,
    ) -> Option<String> {
        if raw_type.contains("TINYINT")
            && let Ok(Some(v)) = row.try_get::<Option<u8>, _>(column_index)
        {
            return Some(v.to_string());
        }
        if raw_type.contains("SMALLINT")
            && let Ok(Some(v)) = row.try_get::<Option<u16>, _>(column_index)
        {
            return Some(v.to_string());
        }
        if raw_type.contains("MEDIUMINT")
            && let Ok(Some(v)) = row.try_get::<Option<u32>, _>(column_index)
        {
            return Some(v.to_string());
        }
        if raw_type.contains("BIGINT")
            && let Ok(Some(v)) = row.try_get::<Option<u64>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // Fallback
        if let Ok(Some(v)) = row.try_get::<Option<u64>, _>(column_index) {
            return Some(v.to_string());
        }
        None
    }

    /// Handle MySQL numeric types using the raw type name (already uppercased)
    fn handle_numeric_type(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column_index: usize,
        raw_type: &str,
    ) -> Option<String> {
        // DECIMAL, NUMERIC
        if (raw_type.contains("DECIMAL") || raw_type.contains("NUMERIC"))
            && let Ok(Some(v)) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // FLOAT
        if raw_type.contains("FLOAT")
            && let Ok(Some(v)) = row.try_get::<Option<f32>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // DOUBLE, REAL
        if (raw_type.contains("DOUBLE") || raw_type.contains("REAL"))
            && let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index)
        {
            return Some(v.to_string());
        }

        // Fallback: try decimal then f64
        if let Ok(Some(v)) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
            return Some(v.to_string());
        }
        if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index) {
            return Some(v.to_string());
        }
        None
    }

    /// Handle MySQL datetime types using the raw type name (already uppercased)
    fn handle_datetime_type(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column_index: usize,
        raw_type: &str,
    ) -> Option<String> {
        // DATE
        if raw_type.contains("DATE")
            && !raw_type.contains("DATETIME")
            && let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index)
        {
            return Some(v.format("%Y-%m-%d").to_string());
        }

        // TIME
        if raw_type.contains("TIME")
            && !raw_type.contains("DATETIME")
            && !raw_type.contains("TIMESTAMP")
            && let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index)
        {
            return Some(v.format("%H:%M:%S").to_string());
        }

        // DATETIME, TIMESTAMP — sqlx decodes TIMESTAMP as DateTime<Utc>
        // (since TIMESTAMP is stored as UTC), and DATETIME as NaiveDateTime.
        if raw_type.contains("TIMESTAMP")
            && let Ok(Some(v)) =
                row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(column_index)
        {
            return Some(v.format("%Y-%m-%d %H:%M:%S").to_string());
        }
        if raw_type.contains("DATETIME")
            && let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index)
        {
            return Some(v.format("%Y-%m-%d %H:%M:%S").to_string());
        }

        // YEAR — sqlx may decode as u16, i16, i32, or u32 depending on driver version.
        if raw_type.contains("YEAR") {
            if let Ok(Some(v)) = row.try_get::<Option<u16>, _>(column_index) {
                return Some(v.to_string());
            }
            if let Ok(Some(v)) = row.try_get::<Option<i16>, _>(column_index) {
                return Some(v.to_string());
            }
            if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(column_index) {
                return Some(v.to_string());
            }
        }

        // Fallback: try string conversion
        if let Ok(Some(v)) = row.try_get::<Option<String>, _>(column_index) {
            return Some(v);
        }
        None
    }

    /// Convert MySQL row value to Option<String> (None = SQL NULL)
    pub fn convert_row_value_to_string(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column_index: usize,
        column_types: &[ColumnType],
        raw_column_types: &[String],
    ) -> Option<String> {
        let column_type = column_types
            .get(column_index)
            .copied()
            .unwrap_or(ColumnType::Unknown);

        let raw_type = raw_column_types
            .get(column_index)
            .map(|s| s.to_uppercase())
            .unwrap_or_default();

        match column_type {
            ColumnType::Integer => self.handle_integer_type(row, column_index, &raw_type),
            ColumnType::UnsignedInteger => {
                self.handle_unsigned_integer_type(row, column_index, &raw_type)
            }
            ColumnType::Numeric => self.handle_numeric_type(row, column_index, &raw_type),
            ColumnType::Boolean => {
                if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(column_index) {
                    return Some(if v { "true" } else { "false" }.to_string());
                }
                None
            }
            ColumnType::DateTime => self.handle_datetime_type(row, column_index, &raw_type),
            ColumnType::Text => {
                if let Ok(v) = row.try_get::<Option<String>, _>(column_index) {
                    return v;
                }
                None
            }
            ColumnType::Json => {
                if let Ok(v) = row.try_get::<Option<String>, _>(column_index) {
                    return v;
                }
                None
            }
            ColumnType::Binary => {
                if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(column_index) {
                    return Some(format!("0x{}", hex::encode(v)));
                }
                None
            }
            ColumnType::Unknown => {
                // Fallback for unknown types
                if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(column_index) {
                    return Some(if v { "true" } else { "false" }.to_string());
                }
                if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
                    return Some(v.to_string());
                }
                if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index) {
                    return Some(v.to_string());
                }
                if let Ok(v) = row.try_get::<Option<String>, _>(column_index) {
                    return v;
                }
                None
            }
            _ => None,
        }
    }
}
