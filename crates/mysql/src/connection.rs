use anyhow::Result;
use async_std::sync::RwLock;
use async_trait::async_trait;
use blanco_core::{ColumnInfo, Connection, ConnectionUIMetadata, QueryResult, TableMetadata};
use futures::StreamExt;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::{Column, Row};
use std::collections::HashMap;
use std::sync::Arc;

/// MySQL connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface with database-specific connection pools
pub struct MysqlConnection {
    pools: Arc<RwLock<HashMap<String, sqlx::MySqlPool>>>, // database_name -> connection pool
    server_key: MysqlServerKey, // Server-level connection key (no database)
    display_name: String,
    server_connection_string: String, // Connection string without database
    initial_database: Option<String>, // Original database from connection string
    ssh_config: Option<MysqlSshConfig>, // SSH tunnel configuration
    local_tunnel_port: Option<u16>,   // Local port for SSH tunnel (if configured)
}

/// SSH configuration for MySQL connections
#[derive(Debug, Clone)]
pub struct MysqlSshConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
}

impl std::fmt::Debug for MysqlConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Cannot use async in Debug trait, so show simplified info
        let pooled_databases = vec!["[async_debug]".to_string()];

        f.debug_struct("MysqlConnection")
            .field("server_key", &self.server_key)
            .field("display_name", &self.display_name)
            .field("pooled_databases", &pooled_databases)
            .field("server_connection_string", &"[REDACTED]")
            .finish()
    }
}

/// Server-level connection key for MySQL connections (no database)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MysqlServerKey {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
}

impl MysqlServerKey {
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

    /// Generate a server-level connection string with SSH tunnel support
    pub fn to_server_connection_string_with_tunnel(&self, local_tunnel_port: u16) -> String {
        if let Some(password) = &self.password {
            format!(
                "mysql://{}:{}@localhost:{}",
                self.username, password, local_tunnel_port
            )
        } else {
            format!("mysql://{}@localhost:{}", self.username, local_tunnel_port)
        }
    }

    /// Generate connection string for a specific database using SSH tunnel
    pub fn to_database_connection_string_with_tunnel(
        &self,
        database: &str,
        local_tunnel_port: u16,
    ) -> String {
        if let Some(password) = &self.password {
            format!(
                "mysql://{}:{}@localhost:{}/{}",
                self.username, password, local_tunnel_port, database
            )
        } else {
            format!(
                "mysql://{}@localhost:{}/{}",
                self.username, local_tunnel_port, database
            )
        }
    }
}

/// Connection key for MySQL connections (legacy - kept for compatibility)
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

    /// Convert to server key
    pub fn to_server_key(&self) -> MysqlServerKey {
        MysqlServerKey::new(
            self.host.clone(),
            self.port,
            self.username.clone(),
            self.password.clone(),
        )
    }
}

impl MysqlConnection {
    /// Create a new MySQL connection from a connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        tracing::info!("🔗 Creating MySQL connection from: {}", connection_string);

        let connection_key = MysqlConnectionKey::from_connection_string(connection_string)?;
        tracing::info!("📋 Parsed connection key:");
        tracing::info!("   - host: {}", connection_key.host);
        tracing::info!("   - port: {}", connection_key.port);
        tracing::info!("   - database: {}", connection_key.database);
        tracing::info!("   - username: {}", connection_key.username);
        tracing::info!(
            "   - password: [{}]",
            if connection_key.password.is_some() {
                "REDACTED"
            } else {
                "NONE"
            }
        );

        let server_key = connection_key.to_server_key();
        let display_name = Self::generate_server_display_name(&server_key);
        let server_connection_string = server_key.to_server_connection_string();

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

    /// Create a new MySQL connection from a MysqlConnectionKey
    pub fn from_key(connection_key: MysqlConnectionKey) -> Self {
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

    /// Create a new MySQL connection with SSH tunnel support
    pub fn from_key_with_ssh(
        connection_key: MysqlConnectionKey,
        ssh_config: MysqlSshConfig,
    ) -> Self {
        let server_key = connection_key.to_server_key();
        let display_name = Self::generate_server_display_name_with_ssh(&server_key, &ssh_config);

        Self {
            pools: Arc::new(RwLock::new(HashMap::new())),
            server_key,
            display_name,
            server_connection_string: connection_key.to_connection_string(),
            initial_database: Some(connection_key.database.clone()),
            ssh_config: Some(ssh_config),
            local_tunnel_port: Some(13306), // Default port for MySQL, will be auto-assigned
        }
    }

    fn generate_server_display_name(server_key: &MysqlServerKey) -> String {
        format!(
            "MySQL: {}@{}:{}",
            server_key.username, server_key.host, server_key.port
        )
    }

    fn generate_server_display_name_with_ssh(
        server_key: &MysqlServerKey,
        ssh_config: &MysqlSshConfig,
    ) -> String {
        format!(
            "MySQL via SSH: {}@{}:{} (via {}@{}:{})",
            server_key.username,
            server_key.host,
            server_key.port,
            ssh_config.ssh_user,
            ssh_config.ssh_host,
            ssh_config.ssh_port
        )
    }

    /// Get or create a connection pool for the specified database
    pub async fn get_or_create_pool(&self, database_name: &str) -> Result<sqlx::MySqlPool> {
        let pools = self.pools.read().await;

        if let Some(pool) = pools.get(database_name) {
            tracing::debug!("🔄 Using existing pool for database: {}", database_name);
            return Ok(pool.clone());
        }

        // Release the read lock before acquiring write lock
        drop(pools);

        tracing::info!("🚀 Creating new pool for database: {}", database_name);
        let mut pools = self.pools.write().await;

        // Check again in case another thread created it while we were waiting
        if let Some(pool) = pools.get(database_name) {
            return Ok(pool.clone());
        }

        let connection_string = if let Some(local_port) = self.local_tunnel_port {
            self.server_key
                .to_database_connection_string_with_tunnel(database_name, local_port)
        } else {
            self.server_key.to_database_connection_string(database_name)
        };
        tracing::debug!(
            "📡 Connection string for {}: {}",
            database_name,
            connection_string
        );

        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await?;

        pools.insert(database_name.to_string(), pool.clone());

        tracing::info!(
            "✅ Successfully created pool for database: {}",
            database_name
        );
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

    /// Convert MySQL row value to string
    pub fn convert_row_value_to_string(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column_index: usize,
        column_type: &str,
    ) -> String {
        // Use column type to determine the best conversion approach
        match column_type.to_uppercase().as_str() {
            // Integer types
            "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT"
            | "TINYINT SIGNED" | "SMALLINT SIGNED" | "MEDIUMINT SIGNED" | "INT SIGNED"
            | "BIGINT SIGNED" => {
                if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
                    // Special handling for TINYINT(1) which is often used for booleans
                    if column_type.to_uppercase().contains("TINYINT") && (v == 0 || v == 1) {
                        return if v == 1 { "true" } else { "false" }.to_string();
                    }
                    return v.to_string();
                }
            }

            "INT UNSIGNED" => {
                if let Ok(Some(v)) = row.try_get::<Option<u64>, _>(column_index) {
                    return v.to_string();
                }
            }

            // Floating point types
            "FLOAT" | "DOUBLE" | "REAL" => {
                if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index) {
                    return v.to_string();
                }
            }

            // Decimal types - try rust_decimal conversion, will fall back to string
            "DECIMAL" | "NUMERIC" => {
                if let Ok(Some(v)) = row.try_get::<Option<rust_decimal::Decimal>, _>(column_index) {
                    return v.to_string();
                }
            }

            // Boolean type
            "BOOLEAN" | "BOOL" | "TINYINT(1)" => {
                if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(column_index) {
                    return if v { "true" } else { "false" }.to_string();
                }
            }

            // Date and time types
            "DATE" => {
                if let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index) {
                    return v.to_string();
                }
            }

            "DATETIME" | "TIMESTAMP" => {
                // Try string conversion first for better compatibility
                if let Ok(Some(v)) = row.try_get::<Option<String>, _>(column_index) {
                    return v;
                }
                // Try NaiveDateTime
                if let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index) {
                    return v.to_string();
                }
                // Try DateTime<Utc> for TIMESTAMP
                if let Ok(Some(v)) =
                    row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(column_index)
                {
                    return v.to_string();
                }
            }

            "TIME" => {
                if let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index) {
                    return v.to_string();
                }
            }

            // String and binary types
            "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM"
            | "SET" | "JSON" => {
                if let Ok(v) = row.try_get::<Option<String>, _>(column_index) {
                    return v.unwrap_or_else(|| "NULL".to_string());
                }
            }

            // Binary types (these might be represented as strings in hex format)
            "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => {
                if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(column_index) {
                    return format!("0x{}", hex::encode(v));
                }
            }

            // Unknown type - try common numeric types first
            unknown => {
                tracing::debug!("unknown type: {}", unknown);

                // Try boolean first (for SELECT TRUE/FALSE literals)
                if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(column_index) {
                    return if v { "true" } else { "false" }.to_string();
                }
                // Try integer next
                if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
                    // Check if this could be a boolean (0/1)
                    if v == 0 || v == 1 {
                        return if v == 1 { "true" } else { "false" }.to_string();
                    }
                    return v.to_string();
                }
                // Try float
                if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index) {
                    return v.to_string();
                }
            }
        }

        // Universal fallback: try string conversion
        if let Ok(v) = row.try_get::<Option<String>, _>(column_index) {
            return v.unwrap_or_else(|| "NULL".to_string());
        }

        // Ultimate fallback if nothing works
        "NULL".to_string()
    }
}

#[async_trait]
impl Connection for MysqlConnection {
    fn get_connection_key_str(&self) -> String {
        self.server_key.to_server_connection_string()
    }

    fn get_connection_type(&self) -> &'static str {
        "MySQL"
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
        tracing::info!("🔌 Connecting to MySQL server: {}", connection_string);

        // Parse and validate the connection string to extract server details
        let key = MysqlConnectionKey::from_connection_string(connection_string)?;
        self.server_key = key.to_server_key();

        // SSH tunnel setup is now handled by DbService
        // The connection string received here already includes the tunnel port if SSH is used
        self.server_connection_string = connection_string.to_string();

        // Update display name
        self.display_name = Self::generate_server_display_name(&self.server_key);

        // Test connection by creating a pool for the initial database
        if !key.database.is_empty() {
            self.get_or_create_pool(&key.database).await?;
        }

        tracing::info!("✅ MySQL connection established successfully");
        Ok(())
    }

    async fn disconnect(&mut self) {
        tracing::info!("🔌 Disconnecting from MySQL: {}", self.display_name);

        let pools = self.pools.read().await;
        for (database, pool) in pools.iter() {
            tracing::debug!("🔄 Closing pool for database: {}", database);
            pool.close().await;
        }
    }

    fn is_connected(&self) -> bool {
        // In a real implementation, we'd check the connection status
        // For now, return true if we have pools configured
        true // Simplified for now
    }

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult, anyhow::Error> {
        let database = database_name
            .or(self.initial_database.as_ref().map(|s| s.as_str()))
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let start_time = std::time::Instant::now();

        tracing::debug!(
            "🎯 Executing MySQL query on database '{}': {} (parameters: {})",
            database,
            query,
            parameters.map(|p| p.len()).unwrap_or(0)
        );

        // Build query with parameters if provided
        let sql_query = if let Some(params) = parameters {
            let mut q = sqlx::query(query);
            for param in params {
                q = q.bind(param);
            }
            q
        } else {
            sqlx::query(query)
        };

        // Use fetch_many to handle both row-returning and row-affecting queries
        use sqlx::Either;
        let mut results = sql_query.fetch_many(&pool);

        let mut columns: Vec<String> = Vec::new();
        let mut column_types: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut rows_affected: u64 = 0;
        let mut collected_rows: Vec<sqlx::mysql::MySqlRow> = Vec::new();

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
                            .map(|col| col.type_info().to_string())
                            .collect();

                        tracing::info!("columns {:?}, {:?}", columns, column_types);
                    }

                    // Collect rows for processing later
                    collected_rows.push(row);
                }
            }
        }

        // Process collected rows into string format
        if !collected_rows.is_empty() {
            rows = collected_rows
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .enumerate()
                        .map(|(i, _)| self.convert_row_value_to_string(row, i, &column_types[i]))
                        .collect()
                })
                .collect();
        }

        let execution_time = start_time.elapsed().as_millis() as i64;

        tracing::debug!(
            "✅ Query executed successfully: {} rows returned, {} rows affected",
            rows.len(),
            rows_affected
        );

        Ok(QueryResult {
            columns,
            column_types,
            rows,
            rows_affected,
            query_text: Some(query.to_string()),
            execution_time_ms: Some(execution_time),
            is_error: false,
            table_name: None,
            primary_key_column: None,
            connection_id: None,
        })
    }

    async fn get_databases(&self) -> Result<Vec<String>, anyhow::Error> {
        tracing::debug!("🗄️ Getting MySQL databases");

        let pool = if let Some(database) = &self.initial_database {
            self.get_or_create_pool(database).await?
        } else {
            return Err(anyhow::anyhow!("No initial database available"));
        };

        let rows = sqlx::query("SHOW DATABASES").fetch_all(&pool).await?;

        let databases: Vec<String> = rows
            .iter()
            .map(|row| row.try_get::<String, _>(0).unwrap_or_default())
            .filter(|db| {
                !db.is_empty()
                    && db != "information_schema"
                    && db != "mysql"
                    && db != "performance_schema"
            })
            .collect();

        tracing::debug!("✅ Found {} databases", databases.len());
        Ok(databases)
    }

    async fn get_schemas(&self) -> Result<Vec<String>, anyhow::Error> {
        // MySQL doesn't have schemas in the same way as PostgreSQL
        // We return the current database as the only "schema"
        if let Some(database) = &self.initial_database {
            Ok(vec![database.clone()])
        } else {
            Ok(vec!["".to_string()])
        }
    }

    async fn get_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        tracing::debug!("📋 Getting MySQL tables");

        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let rows = sqlx::query("SHOW TABLES").fetch_all(&pool).await?;

        let tables: Vec<String> = rows
            .iter()
            .map(|row| row.try_get::<String, _>(0).unwrap_or_default())
            .filter(|table| !table.is_empty())
            .collect();

        tracing::debug!("✅ Found {} tables", tables.len());
        Ok(tables)
    }

    fn supports_schemas(&self) -> bool {
        false // MySQL doesn't support schemas in the PostgreSQL sense
    }

    async fn get_primary_key_for_table(
        &self,
        table_name: &str,
    ) -> Result<Option<String>, anyhow::Error> {
        tracing::debug!("🔑 Getting primary key for table: {}", table_name);

        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let query = format!(
            "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY'"
        );

        let rows = sqlx::query(&query)
            .bind(database)
            .bind(table_name)
            .fetch_all(&pool)
            .await?;

        if let Some(row) = rows.first() {
            let pk_column: String = row.try_get(0)?;
            tracing::debug!("✅ Found primary key: {}", pk_column);
            Ok(Some(pk_column))
        } else {
            tracing::debug!("ℹ️ No primary key found for table: {}", table_name);
            Ok(None)
        }
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, anyhow::Error> {
        tracing::debug!("📋 Getting columns for table: {}", table_name);

        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let query = format!(
            "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, CHARACTER_MAXIMUM_LENGTH, COLUMN_KEY
             FROM INFORMATION_SCHEMA.COLUMNS
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
             ORDER BY ORDINAL_POSITION"
        );

        let rows = sqlx::query(&query)
            .bind(database)
            .bind(table_name)
            .fetch_all(&pool)
            .await?;

        let mut columns = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let data_type: String = row.try_get(1)?;
            let is_nullable_str: String = row.try_get(2)?;
            let default_value: Option<String> = row.try_get(3).ok();
            let max_length: Option<i32> = row.try_get(4).ok();
            let column_key: String = row.try_get(5)?;

            let is_nullable = is_nullable_str == "YES";
            let is_primary_key = column_key == "PRI";

            columns.push(ColumnInfo {
                name,
                data_type,
                is_nullable,
                is_primary_key,
                default_value,
                character_maximum_length: max_length,
            });
        }

        tracing::debug!(
            "✅ Found {} columns for table: {}",
            columns.len(),
            table_name
        );
        Ok(columns)
    }

    async fn get_table_metadata(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<TableMetadata, anyhow::Error> {
        let columns = self.get_columns_for_table(table_name, None).await?;
        let primary_key = self.get_primary_key_for_table(table_name).await?;

        let mut metadata = TableMetadata::new(table_name.to_string(), None);
        metadata.columns = columns;
        metadata.primary_keys = primary_key.into_iter().collect();

        Ok(metadata)
    }

    fn extract_table_name_from_query(
        &self,
        query: &str,
        _alias: bool,
    ) -> Result<Option<String>, anyhow::Error> {
        // Simple regex-based extraction for common table patterns
        let query_lower = query.to_lowercase();

        // Look for FROM table_name patterns
        if let Some(from_pos) = query_lower.find(" from ") {
            let after_from = &query_lower[from_pos + 6..];
            let words: Vec<&str> = after_from.split_whitespace().collect();
            if let Some(first_word) = words.first() {
                return Ok(Some(first_word.to_string()));
            }
        }

        // Look for INTO table_name patterns
        if let Some(into_pos) = query_lower.find(" into ") {
            let after_into = &query_lower[into_pos + 6..];
            let words: Vec<&str> = after_into.split_whitespace().collect();
            if let Some(first_word) = words.first() {
                return Ok(Some(first_word.to_string()));
            }
        }

        Ok(None)
    }

    fn resolve_table_alias(
        &self,
        query: &str,
        alias: &str,
    ) -> Result<Option<String>, anyhow::Error> {
        // Simple regex-based alias resolution
        let query_lower = query.to_lowercase();

        // Look for alias patterns like "table_name AS alias" or "table_name alias"
        let patterns = vec![
            format!(" {} as ", alias),
            format!(" {} ", alias),
            format!("({} as ", alias),
            format!("({} ", alias),
        ];

        for pattern in patterns {
            if let Some(pos) = query_lower.find(&pattern) {
                // Find the word before the alias
                let before_alias = &query_lower[..pos];
                let words: Vec<&str> = before_alias.split_whitespace().collect();
                if let Some(last_word) = words.last() {
                    return Ok(Some(last_word.to_string()));
                }
            }
        }

        Ok(None)
    }

    fn get_file_safe_name(&self) -> String {
        format!("mysql_{}_{}", self.server_key.host, self.server_key.port)
            .replace(':', "_")
            .replace('.', "_")
    }

    fn get_ui_metadata(&self) -> ConnectionUIMetadata {
        ConnectionUIMetadata {
            display_name: self.display_name.clone(),
            file_safe_name: self.get_file_safe_name(),
            supports_schemas: false,
        }
    }

    async fn get_database_schema_paginated(
        &self,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<blanco_core::connection_trait::DatabaseSchemaResult> {
        let limit = limit.unwrap_or(20).min(100) as i32; // Default 20, max 100
        let offset = offset.unwrap_or(0) as i32;

        let tables = self
            .get_schema_paginated(table_names, limit, offset)
            .await?;
        let table_count = tables.len();

        Ok(blanco_core::connection_trait::DatabaseSchemaResult {
            connection_type: self.get_connection_type().to_string(),
            display_name: self.get_display_name(),
            tables,
            pagination: blanco_core::connection_trait::PaginationInfo {
                limit: Some(limit as i64),
                offset: Some(offset as i64),
                has_more: table_count == limit as usize,
            },
        })
    }
}
