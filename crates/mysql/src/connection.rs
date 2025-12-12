use anyhow::Result;
use async_std::sync::RwLock;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, ConnectionUIMetadata, IconName, QueryResult, TableChangeOperation,
    TableMetadata,
};
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
    fn convert_row_value_to_string(
        &self,
        row: &sqlx::mysql::MySqlRow,
        column_index: usize,
        _column_type: &str,
    ) -> String {
        // Try different types in order of likelihood
        if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
            return val.unwrap_or_else(|| "NULL".to_string());
        }

        if let Ok(val) = row.try_get::<Option<i64>, _>(column_index) {
            return val
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if let Ok(val) = row.try_get::<Option<f64>, _>(column_index) {
            return val
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if let Ok(val) = row.try_get::<Option<bool>, _>(column_index) {
            return val
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index) {
            return val
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index) {
            return val
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        // Fallback: try to get as raw string
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

    fn get_icon_name(&self) -> IconName {
        IconName::MySQL
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    fn get_manager_any(&self) -> &dyn std::any::Any {
        self
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

    async fn ensure_connected(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
        if !self.is_connected() {
            self.connect(connection_string).await?;
        }
        Ok(())
    }

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<QueryResult, anyhow::Error> {
        let database = database_name
            .or(self.initial_database.as_ref().map(|s| s.as_str()))
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let start_time = std::time::Instant::now();

        tracing::debug!(
            "🎯 Executing MySQL query on database '{}': {}",
            database,
            query
        );

        let rows = sqlx::query(query).fetch_all(&pool).await?;

        let execution_time = start_time.elapsed().as_millis() as i64;

        if rows.is_empty() {
            tracing::debug!("✅ Query returned no rows");
            return Ok(QueryResult {
                columns: vec![],
                column_types: vec![],
                rows: vec![],
                rows_affected: 0,
                query_text: Some(query.to_string()),
                execution_time_ms: Some(execution_time),
                is_error: false,
                table_name: None,
                primary_key_column: None,
                connection_id: None,
            });
        }

        // Extract column information from first row
        let first_row = &rows[0];
        let columns: Vec<String> = first_row
            .columns()
            .iter()
            .map(|col| col.name().to_string())
            .collect();

        let column_types: Vec<String> = first_row
            .columns()
            .iter()
            .map(|col| col.type_info().to_string())
            .collect();

        // Convert rows to string format
        let result_rows: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .enumerate()
                    .map(|(i, _)| self.convert_row_value_to_string(row, i, &column_types[i]))
                    .collect()
            })
            .collect();

        let rows_count = result_rows.len();
        tracing::debug!(
            "✅ Query executed successfully: {} rows returned",
            rows_count
        );

        Ok(QueryResult {
            columns,
            column_types,
            rows: result_rows,
            rows_affected: rows_count as u64,
            query_text: Some(query.to_string()),
            execution_time_ms: Some(execution_time),
            is_error: false,
            table_name: None,
            primary_key_column: None,
            connection_id: None,
        })
    }

    async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult, anyhow::Error> {
        tracing::debug!("🎯 Executing MySQL prepared query: {}", sql_template);
        tracing::debug!("📋 Parameters: {:?}", parameters);

        // For now, implement simple parameter substitution
        // In a production environment, you'd want to use actual prepared statements
        let mut query = sql_template.to_string();
        for param in parameters {
            query = query.replacen('?', &format!("'{}'", param), 1);
        }

        self.execute_query(&query, self.initial_database.as_deref())
            .await
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

    async fn execute_table_changes(
        &self,
        _changes: &[TableChangeOperation],
    ) -> Result<QueryResult, anyhow::Error> {
        // For now, return an error indicating this isn't implemented
        Err(anyhow::anyhow!(
            "Table changes are not yet implemented for MySQL"
        ))
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
            icon_name: IconName::MySQL,
        }
    }
}
