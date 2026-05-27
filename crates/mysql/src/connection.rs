use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, QueryResult, connection_trait::ColumnType,
    connection_trait::ForeignKeyInfo, connection_trait::IndexInfo, connection_trait::RoutineKind,
};
use futures::StreamExt;
use smol::lock::RwLock;
use sqlx::mysql::MySqlPoolOptions;
use sqlx::{Column, Row};
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
            .connect(&connection_string)
            .await?;

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
            && let Ok(Some(v)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(column_index)
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

#[async_trait]
impl Connection for MysqlConnection {
    fn get_connection_type(&self) -> &'static str {
        "MySQL"
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
        // Parse and validate the connection string to extract server details
        let key = MysqlConnectionKey::from_connection_string(connection_string)?;
        self.host = key.host;
        self.port = key.port;
        self.username = key.username.clone();
        self.password = key.password.clone();
        self.server_connection_string = connection_string.to_string();

        // Update display name
        self.display_name =
            Self::generate_server_display_name(&self.username, &self.host, self.port);

        // Test connection by creating a pool for the initial database
        if !key.database.is_empty() {
            self.get_or_create_pool(&key.database).await?;
        }

        Ok(())
    }

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult, anyhow::Error> {
        let database = database_name
            .or(self.initial_database.as_deref())
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let start_time = std::time::Instant::now();

        // Build query: use raw_sql for no parameters, otherwise bind parameters
        let sql_query = match parameters {
            Some(params) if !params.is_empty() => {
                let mut q = sqlx::query(query);
                for param in params {
                    q = q.bind(param);
                }
                #[allow(deprecated)]
                q.fetch_many(&pool)
            }
            _ => sqlx::raw_sql(query).fetch_many(&pool),
        };

        // Use fetch_many to handle both row-returning and row-affecting queries
        use sqlx::Either;
        let mut results = sql_query;

        let mut columns: Vec<String> = Vec::new();
        let mut column_types: Vec<ColumnType> = Vec::new();
        let mut raw_column_types: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        let mut rows_affected: u64 = 0;

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

                        let (types, raw_types): (Vec<ColumnType>, Vec<String>) = row
                            .columns()
                            .iter()
                            .map(|col| {
                                let raw_type = col.type_info().to_string();
                                (Self::map_mysql_type(&raw_type), raw_type)
                            })
                            .unzip();
                        column_types = types;
                        raw_column_types = raw_types;
                    }

                    // Convert row to strings immediately instead of collecting raw rows
                    // This prevents memory doubling by not holding both raw and converted data
                    let row_data: Vec<Option<String>> = columns
                        .iter()
                        .enumerate()
                        .map(|(i, _)| {
                            self.convert_row_value_to_string(
                                &row,
                                i,
                                &column_types,
                                &raw_column_types,
                            )
                        })
                        .collect();
                    rows.push(row_data);
                }
            }
        }

        let execution_time = start_time.elapsed().as_millis() as i64;

        Ok(QueryResult {
            columns,
            column_types,
            rows,
            rows_affected,
            query_text: Some(query.to_string()),
            execution_time_ms: Some(execution_time),
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        })
    }

    async fn execute_script(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<Vec<QueryResult>, anyhow::Error> {
        use sqlx::Either;

        let database = database_name
            .or(self.initial_database.as_deref())
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;
        let pool = self.get_or_create_pool(database).await?;
        let mut results = sqlx::raw_sql(query).fetch_many(&pool);

        let mut out: Vec<QueryResult> = Vec::new();
        let mut current: Option<MysqlStmtAcc> = None;

        while let Some(result) = results.next().await {
            match result? {
                Either::Left(exec) => {
                    let mut acc = current.take().unwrap_or_default();
                    acc.rows_affected += exec.rows_affected();
                    out.push(acc.finalize());
                }
                Either::Right(row) => {
                    let acc = current.get_or_insert_with(MysqlStmtAcc::default);
                    acc.absorb_row(&row, self);
                }
            }
        }

        if let Some(acc) = current.take() {
            out.push(acc.finalize());
        }

        Ok(out)
    }

    async fn execute_write(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64, anyhow::Error> {
        let database = database_name
            .or(self.initial_database.as_deref())
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;
        let pool = self.get_or_create_pool(database).await?;

        let mut q = sqlx::query(query);
        for param in parameters {
            q = match param {
                Some(value) => q.bind(value.clone()),
                None => q.bind(Option::<String>::None),
            };
        }
        let result = q.execute(&pool).await?;
        Ok(result.rows_affected())
    }

    async fn get_databases(&self) -> Result<Vec<String>, anyhow::Error> {
        tracing::debug!("Getting MySQL databases");

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

        tracing::debug!("Found {} databases", databases.len());
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
        tracing::debug!("Getting MySQL tables");

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

        tracing::debug!("Found {} tables", tables.len());
        Ok(tables)
    }

    async fn get_views(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let rows = sqlx::query("SHOW FULL TABLES WHERE TABLE_TYPE LIKE 'VIEW'")
            .fetch_all(&pool)
            .await?;

        let views: Vec<String> = rows
            .iter()
            .map(|row| row.try_get::<String, _>(0).unwrap_or_default())
            .filter(|view| !view.is_empty())
            .collect();

        Ok(views)
    }

    async fn list_procedures(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;
        let pool = self.get_or_create_pool(database).await?;
        let rows = sqlx::query(
            "SELECT ROUTINE_NAME FROM information_schema.routines
             WHERE ROUTINE_SCHEMA = ? AND ROUTINE_TYPE = 'PROCEDURE'
             ORDER BY ROUTINE_NAME",
        )
        .bind(database)
        .fetch_all(&pool)
        .await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>(0).ok())
            .collect())
    }

    async fn list_functions(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;
        let pool = self.get_or_create_pool(database).await?;
        let rows = sqlx::query(
            "SELECT ROUTINE_NAME FROM information_schema.routines
             WHERE ROUTINE_SCHEMA = ? AND ROUTINE_TYPE = 'FUNCTION'
             ORDER BY ROUTINE_NAME",
        )
        .bind(database)
        .fetch_all(&pool)
        .await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>(0).ok())
            .collect())
    }

    async fn list_triggers(&self, _schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;
        let pool = self.get_or_create_pool(database).await?;
        let rows = sqlx::query(
            "SELECT TRIGGER_NAME FROM information_schema.triggers
             WHERE TRIGGER_SCHEMA = ?
             ORDER BY TRIGGER_NAME",
        )
        .bind(database)
        .fetch_all(&pool)
        .await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>(0).ok())
            .collect())
    }

    async fn get_object_sizes(
        &self,
        _schema: Option<&str>,
    ) -> Result<std::collections::HashMap<String, u64>, anyhow::Error> {
        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;
        let pool = self.get_or_create_pool(database).await?;
        // DATA_LENGTH/INDEX_LENGTH are BIGINT UNSIGNED in information_schema.
        // Their sum returns DECIMAL on older MySQL/MariaDB versions, which
        // sqlx-mysql can't decode into i64/u64 directly. Casting to SIGNED
        // gives a plain BIGINT we can pull as i64.
        let rows = sqlx::query(
            "SELECT TABLE_NAME,
                    CAST(COALESCE(DATA_LENGTH, 0) + COALESCE(INDEX_LENGTH, 0) AS SIGNED)
                        AS size_bytes
             FROM information_schema.TABLES
             WHERE TABLE_SCHEMA = ?
               AND TABLE_TYPE = 'BASE TABLE'",
        )
        .bind(database)
        .fetch_all(&pool)
        .await?;
        let mut sizes = std::collections::HashMap::new();
        for row in rows {
            let name: String = match row.try_get(0) {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("MySQL get_object_sizes: failed to decode name: {}", e);
                    continue;
                }
            };
            match row.try_get::<i64, _>(1) {
                Ok(size) if size > 0 => {
                    sizes.insert(name, size as u64);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        "MySQL get_object_sizes: failed to decode size for {}: {}",
                        name,
                        e
                    );
                }
            }
        }
        tracing::debug!("MySQL sizes for {}: {} entries", database, sizes.len());
        Ok(sizes)
    }

    async fn object_ddl(
        &self,
        kind: RoutineKind,
        _schema: Option<&str>,
        name: &str,
    ) -> Result<String, anyhow::Error> {
        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;
        let pool = self.get_or_create_pool(database).await?;
        // SHOW CREATE PROCEDURE/FUNCTION/TRIGGER returns a row whose second
        // column ('Create Procedure'/'Create Function'/'SQL Original Statement')
        // contains the DDL. The exact column name varies, so we pick by index.
        let (stmt, ddl_col) = match kind {
            RoutineKind::Procedure => (
                format!("SHOW CREATE PROCEDURE `{}`.`{}`", database, name),
                2usize,
            ),
            RoutineKind::Function => (
                format!("SHOW CREATE FUNCTION `{}`.`{}`", database, name),
                2usize,
            ),
            RoutineKind::Trigger => (format!("SHOW CREATE TRIGGER `{}`.`{}`", database, name), 2),
        };
        let row = sqlx::query(&stmt)
            .fetch_one(&pool)
            .await
            .map_err(|e| anyhow::anyhow!("SHOW CREATE failed: {}", e))?;
        let ddl: String = row
            .try_get::<String, _>(ddl_col)
            .map_err(|e| anyhow::anyhow!("DDL column not found: {}", e))?;
        Ok(ddl)
    }

    async fn get_queryable_entities(
        &self,
        _schema: Option<&str>,
    ) -> Result<Vec<blanco_core::QueryableEntity>, anyhow::Error> {
        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let query = "
            SELECT TABLE_NAME as name,
                   CASE WHEN TABLE_TYPE = 'VIEW' THEN 'VIEW' ELSE 'TABLE' END as entity_type
            FROM information_schema.tables
            WHERE TABLE_SCHEMA = ?
            ORDER BY TABLE_NAME
        ";

        let rows = sqlx::query(query).bind(database).fetch_all(&pool).await?;

        use blanco_core::{EntityType, QueryableEntity};
        let entities: Vec<QueryableEntity> = rows
            .iter()
            .filter_map(|row| {
                let name: String = row.try_get(0).ok()?;
                let entity_type_str: String = row.try_get(1).ok()?;
                let entity_type = match entity_type_str.as_str() {
                    "VIEW" => EntityType::View,
                    _ => EntityType::Table,
                };
                Some(QueryableEntity { name, entity_type })
            })
            .collect();

        Ok(entities)
    }

    fn supports_schemas(&self) -> bool {
        false // MySQL doesn't support schemas in the PostgreSQL sense
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, anyhow::Error> {
        tracing::debug!("Getting columns for table: {}", table_name);

        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        // Single query to get columns with PK and FK information
        let query = "
            SELECT
                c.COLUMN_NAME,
                c.DATA_TYPE,
                c.IS_NULLABLE,
                c.COLUMN_DEFAULT,
                c.CHARACTER_MAXIMUM_LENGTH,
                c.COLUMN_KEY = 'PRI' AS is_primary_key,
                fk.REFERENCED_TABLE_NAME AS fk_table,
                fk.REFERENCED_COLUMN_NAME AS fk_column,
                fk.CONSTRAINT_NAME AS fk_constraint
            FROM INFORMATION_SCHEMA.COLUMNS c
            LEFT JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE fk
                ON fk.TABLE_SCHEMA = c.TABLE_SCHEMA
                AND fk.TABLE_NAME = c.TABLE_NAME
                AND fk.COLUMN_NAME = c.COLUMN_NAME
                AND fk.REFERENCED_TABLE_NAME IS NOT NULL
            WHERE c.TABLE_SCHEMA = ?
                AND c.TABLE_NAME = ?
            ORDER BY c.ORDINAL_POSITION
        ";

        let rows = sqlx::query(query)
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
            let is_primary_key: bool = row.try_get(5).unwrap_or(false);
            let fk_table: Option<String> = row.try_get(6).ok();
            let fk_column: Option<String> = row.try_get(7).ok();
            let fk_constraint: Option<String> = row.try_get(8).ok();

            let foreign_key = match (fk_table, fk_column) {
                (Some(table), Some(column)) => Some(ForeignKeyInfo {
                    foreign_table_name: table,
                    foreign_column_name: column,
                    constraint_name: fk_constraint,
                }),
                _ => None,
            };

            let is_nullable = is_nullable_str == "YES";

            columns.push(ColumnInfo {
                name,
                data_type,
                is_nullable,
                is_primary_key,
                default_value,
                character_maximum_length: max_length,
                foreign_key,
            });
        }

        tracing::debug!("Found {} columns for table: {}", columns.len(), table_name);
        Ok(columns)
    }

    async fn get_database_schema_paginated(
        &self,
        _database_name: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<blanco_core::DatabaseSchemaResult> {
        let limit = limit.unwrap_or(20).min(100) as i32; // Default 20, max 100
        let offset = offset.unwrap_or(0) as i32;

        let tables = self
            .get_schema_paginated(table_names, limit, offset)
            .await?;
        let table_count = tables.len();

        Ok(blanco_core::DatabaseSchemaResult {
            connection_type: self.get_connection_type().to_string(),
            display_name: self.get_display_name(),
            tables,
            pagination: blanco_core::PaginationInfo {
                limit: Some(limit as i64),
                offset: Some(offset as i64),
                has_more: table_count == limit as usize,
            },
        })
    }

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult, anyhow::Error> {
        const ROW_ESTIMATE_THRESHOLD: i64 = 20;
        const LIMIT_THRESHOLD: i64 = 100;

        // Get row estimate from INFORMATION_SCHEMA.TABLES
        let estimate_query = "SELECT TABLE_ROWS FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = ? AND TABLE_SCHEMA = DATABASE()";
        let estimate_result = self
            .execute_query(estimate_query, None, Some(&[table_name.to_string()]))
            .await?;

        let row_count = estimate_result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|val| val.as_ref())
            .and_then(|val| val.parse::<i64>().ok())
            .filter(|n| *n >= 0);

        if let Some(count) = row_count
            && count <= ROW_ESTIMATE_THRESHOLD
        {
            // Small table: fetch all rows with referenced row first
            let query = format!(
                "SELECT * FROM {} ORDER BY {} = '{}' DESC LIMIT {}",
                table_name, column_name, reference_value, LIMIT_THRESHOLD
            );
            return self.execute_query(&query, None, None).await;
        }

        // Large table or estimate unavailable: fetch only referenced row
        let query = format!(
            "SELECT * FROM {} WHERE {} = '{}'",
            table_name, column_name, reference_value
        );
        self.execute_query(&query, None, None).await
    }

    async fn get_indexes_for_table(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>, anyhow::Error> {
        tracing::debug!("Getting indexes for table: {}", table_name);

        let database = self
            .initial_database
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No database specified"))?;

        let pool = self.get_or_create_pool(database).await?;

        let query = "
            SELECT
                s.INDEX_NAME,
                s.NON_UNIQUE,
                s.INDEX_TYPE,
                GROUP_CONCAT(s.COLUMN_NAME ORDER BY s.SEQ_IN_INDEX SEPARATOR ',') AS columns,
                NULL AS idx_condition,
                NULL AS idx_comment
            FROM INFORMATION_SCHEMA.STATISTICS s
            WHERE s.TABLE_SCHEMA = ?
                AND s.TABLE_NAME = ?
            GROUP BY s.INDEX_NAME, s.NON_UNIQUE, s.INDEX_TYPE
            ORDER BY s.INDEX_NAME
        ";

        let rows = sqlx::query(query)
            .bind(database)
            .bind(table_name)
            .fetch_all(&pool)
            .await?;

        let mut indexes = Vec::new();
        for row in rows {
            let name: String = row.try_get(0)?;
            let non_unique: i32 = row.try_get(1).unwrap_or(1);
            let index_type: String = row.try_get(2).unwrap_or_else(|_| "BTREE".to_string());
            let columns_str: String = row.try_get(3).unwrap_or_default();
            let condition: Option<String> = row.try_get(4).ok();
            let comment: Option<String> = row.try_get(5).ok();

            let column_names: Vec<String> = columns_str.split(',').map(|s| s.to_string()).collect();

            indexes.push(IndexInfo {
                name,
                algorithm: index_type,
                is_unique: non_unique == 0,
                column_names,
                condition,
                comment,
            });
        }

        tracing::debug!("Found {} indexes for table: {}", indexes.len(), table_name);
        Ok(indexes)
    }
}

/// Per-statement accumulator used by `execute_script` to preserve result-set
/// boundaries when running multi-statement SQL.
#[derive(Default)]
struct MysqlStmtAcc {
    columns: Vec<String>,
    column_types: Vec<ColumnType>,
    raw_column_types: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    rows_affected: u64,
}

impl MysqlStmtAcc {
    fn absorb_row(&mut self, row: &sqlx::mysql::MySqlRow, conn: &MysqlConnection) {
        if self.columns.is_empty() {
            self.columns = row
                .columns()
                .iter()
                .map(|col| col.name().to_string())
                .collect();
            let (types, raw_types): (Vec<ColumnType>, Vec<String>) = row
                .columns()
                .iter()
                .map(|col| {
                    let raw_type = col.type_info().to_string();
                    (MysqlConnection::map_mysql_type(&raw_type), raw_type)
                })
                .unzip();
            self.column_types = types;
            self.raw_column_types = raw_types;
        }
        let row_data: Vec<Option<String>> = (0..self.columns.len())
            .map(|i| {
                conn.convert_row_value_to_string(row, i, &self.column_types, &self.raw_column_types)
            })
            .collect();
        self.rows.push(row_data);
    }

    fn finalize(self) -> QueryResult {
        QueryResult {
            columns: self.columns,
            column_types: self.column_types,
            rows: self.rows,
            rows_affected: self.rows_affected,
            query_text: None,
            execution_time_ms: None,
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        }
    }
}
