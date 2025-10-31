use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    Connection, ConnectionFactory, IconName, QueryResult, ColumnInfo, TableMetadata,
    ConnectionUIMetadata, TableChangeOperation,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Column, Row, TypeInfo};
use sqlx::postgres::types::PgMoney;
use crate::sql_parser::PostgresTableExtractor;

/// PostgreSQL connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface
pub struct PostgresConnection {
    pool: Option<sqlx::PgPool>,
    connection_key: PgConnectionKey,
    display_name: String,
    connection_string: String,
}

impl std::fmt::Debug for PostgresConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresConnection")
            .field("connection_key", &self.connection_key)
            .field("display_name", &self.display_name)
            .field("connection_string", &"[REDACTED]")
            .finish()
    }
}

/// Connection key for PostgreSQL connections
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PgConnectionKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
}

impl PgConnectionKey {
    pub fn new(host: String, port: u16, database: String, username: String, password: Option<String>) -> Self {
        Self {
            host,
            port,
            database,
            username,
            password,
        }
    }

    /// Extract connection key from a PostgreSQL connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        // Handle different PostgreSQL connection string formats:
        // - postgresql://user:password@host:port/database
        // - postgres://user:password@host:port/database

        let url = if connection_string.starts_with("postgresql://") {
            connection_string
        } else if connection_string.starts_with("postgres://") {
            connection_string
        } else {
            return Err(anyhow::anyhow!("Invalid PostgreSQL connection string format"));
        };

        // Parse the URL
        let parsed = url::Url::parse(url)?;

        let host = parsed.host_str().unwrap_or("localhost").to_string();
        let port = parsed.port().unwrap_or(5432);
        let database = parsed.path().trim_start_matches('/').to_string();
        let username = parsed.username().to_string();
        let password = parsed.password().map(|p| p.to_string());

        if database.is_empty() {
            return Err(anyhow::anyhow!("Database name is required in connection string"));
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
            format!("postgresql://{}:{}@{}:{}/{}", self.username, password, self.host, self.port, self.database)
        } else {
            format!("postgresql://{}@{}:{}/{}", self.username, self.host, self.port, self.database)
        }
    }
}

impl PostgresConnection {
    /// Create a new PostgreSQL connection
    pub fn new(host: String, port: u16, database: String, username: String, password: Option<String>) -> Self {
        let connection_key = PgConnectionKey::new(host, port, database, username, password);
        let display_name = Self::generate_display_name(&connection_key);
        let connection_string = connection_key.to_connection_string();

        Self {
            pool: None,
            connection_key,
            display_name,
            connection_string,
        }
    }

    /// Create a new PostgreSQL connection from a connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let connection_key = PgConnectionKey::from_connection_string(connection_string)?;
        let display_name = Self::generate_display_name(&connection_key);

        Ok(Self {
            pool: None,
            connection_key,
            display_name,
            connection_string: connection_string.to_string(),
        })
    }

    /// Create a new PostgreSQL connection from a PgConnectionKey
    pub fn from_key(connection_key: PgConnectionKey) -> Self {
        let display_name = Self::generate_display_name(&connection_key);
        let connection_string = connection_key.to_connection_string();

        Self {
            pool: None,
            connection_key,
            display_name,
            connection_string,
        }
    }

    /// Generate a human-readable display name for the connection
    fn generate_display_name(key: &PgConnectionKey) -> String {
        format!("PostgreSQL - {}@{}:{}/{}", key.username, key.host, key.port, key.database)
    }

    /// Get connection details
    pub fn get_connection_details(&self) -> (&str, u16, &str, &str) {
        (&self.connection_key.host, self.connection_key.port, &self.connection_key.database, &self.connection_key.username)
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
        if let Some(pool) = &self.pool {
            // Execute a simple ping query to check connection health
            (sqlx::query("SELECT 1").fetch_one(pool).await).is_ok()
        } else {
            false
        }
    }

    /// Execute a query asynchronously using SQLX directly
    async fn execute_query_async(&self, query: &str) -> Result<QueryResult> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

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
                        connection_string: None,
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

                // Extract row data
                let data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        columns
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                // Enhanced type conversion for PostgreSQL - specific types first
                                if let Ok(val) = row.try_get::<Option<uuid::Uuid>, _>(i) {
                                    // UUID support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(i) {
                                    // decimal/numeric support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<PgMoney>, _>(i) {
                                    // MONEY type support - convert to decimal and format with currency symbol
                                    val.map(|v| {
                                        let decimal_val = v.to_decimal(2); // Use 2 decimal places for currency
                                        format!("${}", decimal_val)
                                    })
                                    .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    // Regular string support
                                    val.unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i16>, _>(i) {
                                    // smallint support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i32>, _>(i) {
                                    // integer support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
                                    // bigint support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f32>, _>(i) {
                                    // real/float4 support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    // float/double/numeric support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    // boolean support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::DateTime<chrono::FixedOffset>>, _>(i) {
                                    // timestamptz support (timestamp with time zone) - preserve original timezone
                                    val.map(|v| {
                                        // Format with original timezone information preserved from PostgreSQL
                                        v.format("%Y-%m-%d %H:%M:%S %:z").to_string()
                                    })
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(i) {
                                    // timestamp support (timestamp without time zone) - consistent formatting
                                    val.map(|v| {
                                        // Format in a consistent, readable format
                                        v.format("%Y-%m-%d %H:%M:%S").to_string()
                                    })
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                                    // date support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                                    // time support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(i) {
                                    // JSON/JSONB support with pretty printing
                                    val.map(|v| {
                                        serde_json::to_string_pretty(&v)
                                            .unwrap_or_else(|_| v.to_string())
                                    })
                                    .unwrap_or_else(|| "NULL".to_string())
                                } else {
                                    // Check if this is an array column based on column type information
                                    let column_type = column_types.get(i).map(|s| s.as_str()).unwrap_or("unknown");
                                    if column_type == "ARRAY" || column_type.ends_with("[]") {
                                        // This is an array column - try different conversion approaches
                                        log::debug!("Attempting to convert array column type '{}'", column_type);

                                        // Try String conversion first
                                        if let Ok(array_val) = row.try_get::<Option<String>, _>(i) {
                                            return array_val.map(|v| {
                                                log::debug!("Successfully converted array '{}' to String for column type '{}'", v, column_type);
                                                v
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<String> conversion
                                        if let Ok(array_val) = row.try_get::<Option<Vec<String>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| format!("\"{}\"", x))
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<String> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<i32> conversion for integer arrays
                                        if let Ok(array_val) = row.try_get::<Option<Vec<i32>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| x.to_string())
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<i32> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<i64> conversion for bigint arrays
                                        if let Ok(array_val) = row.try_get::<Option<Vec<i64>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| x.to_string())
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<i64> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<uuid::Uuid> conversion for UUID arrays
                                        if let Ok(array_val) = row.try_get::<Option<Vec<uuid::Uuid>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| x.to_string())
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<uuid::Uuid> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Log what SQLX types we tried
                                        log::warn!("Array column type '{}' couldn't be converted to any supported array type (String, Vec<String>, Vec<i32>, Vec<i64>, Vec<uuid::Uuid>)", column_type);
                                        "NULL".to_string()
                                    } else if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                        // text/varchar support (fallback) - also handles arrays
                                        val.map(|v| {
                                            // Check if this looks like a PostgreSQL array string
                                            if v.starts_with('{') && v.ends_with('}') {
                                                // This is already in PostgreSQL array format, return as-is
                                                log::debug!("Found PostgreSQL array string: '{}' for column type '{}'", v, column_types.get(i).unwrap_or(&"unknown".to_string()));
                                                v
                                            } else {
                                                // Regular string value
                                                v
                                            }
                                        })
                                        .unwrap_or_else(|| "NULL".to_string())
                                    } else {
                                        // Final fallback - log unmatched type for debugging
                                        let column_type = column_types.get(i).map(|s| s.as_str()).unwrap_or("unknown");
                                        log::warn!("Unmatched PostgreSQL column type '{}' at index {}, falling back to NULL", column_type, i);
                                        "NULL".to_string()
                                    }
                                }
                            })
                            .collect()
                    })
                    .collect();

                Ok(QueryResult {
                    columns,
                    column_types,
                    rows: data_rows,
                    rows_affected: 0,
                    query_text: None,
                    execution_time_ms: None,
                    is_error: false,
                    table_name: None,
                    primary_key_column: None,
                    connection_string: None,
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
                    connection_string: None,
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

        let result = self.execute_prepared_query(query, &[schema_filter.to_string()]).await?;
        let tables: Vec<String> = result.rows.into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();

        Ok(tables)
    }
}

#[async_trait]
impl Connection for PostgresConnection {
    fn get_connection_key_str(&self) -> String {
        format!("postgres:{}@{}:{}/{}",
            self.connection_key.username,
            self.connection_key.host,
            self.connection_key.port,
            self.connection_key.database
        )
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }

    fn get_icon_name(&self) -> IconName {
        IconName::Postgres
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    fn get_manager_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        log::info!("Connecting to PostgreSQL database: {}@{}:{}/{}",
            self.connection_key.username,
            self.connection_key.host,
            self.connection_key.port,
            self.connection_key.database
        );

        // Parse and validate the connection string
        let key = PgConnectionKey::from_connection_string(connection_string)?;
        self.connection_key = key.clone();
        self.display_name = Self::generate_display_name(&key);
        self.connection_string = key.to_connection_string();

        // Connect using SQLX directly
        let connection_string = self.connection_string.clone();
        let pool = self.connect_async(&connection_string).await?;
        self.pool = Some(pool);

        log::info!("Successfully connected to PostgreSQL database");
        Ok(())
    }

    async fn disconnect(&mut self) {
        log::info!("Disconnecting from PostgreSQL database: {}", self.display_name);
        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }
    }

    fn is_connected(&self) -> bool {
        self.pool.is_some()
    }

    async fn ensure_connected(&mut self, connection_string: &str) -> Result<()> {
        if !self.is_connected() || !self.is_connection_healthy().await {
            log::info!("Reconnecting to PostgreSQL database");
            self.connect(connection_string).await?;
        }
        Ok(())
    }

    async fn execute_query(&self, query: &str) -> Result<QueryResult> {
        log::debug!("Executing PostgreSQL query: {}", query);

        let result = self.execute_query_async(query).await
            .map_err(|e| anyhow::anyhow!("PostgreSQL query execution failed: {}", e))?;
        log::debug!("Query executed successfully, {} rows returned", result.row_count());

        Ok(result)
    }

    async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult> {
        log::debug!("Executing prepared PostgreSQL query with {} parameters", parameters.len());

        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        // Build the query with parameter placeholders
        let mut query = sqlx::query(sql_template);

        // Add parameters to the query
        for param in parameters {
            query = query.bind(param);
        }

        // Try to execute as a query that returns rows
        match query.fetch_all(pool).await {
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
                        connection_string: None,
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

                // Extract row data
                let data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        columns
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                // Enhanced type conversion for PostgreSQL - specific types first
                                if let Ok(val) = row.try_get::<Option<uuid::Uuid>, _>(i) {
                                    // UUID support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<rust_decimal::Decimal>, _>(i) {
                                    // decimal/numeric support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<PgMoney>, _>(i) {
                                    // MONEY type support - convert to decimal and format with currency symbol
                                    val.map(|v| {
                                        let decimal_val = v.to_decimal(2); // Use 2 decimal places for currency
                                        format!("${}", decimal_val)
                                    })
                                    .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    // Regular string support
                                    val.unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i16>, _>(i) {
                                    // smallint support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i32>, _>(i) {
                                    // integer support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
                                    // bigint support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f32>, _>(i) {
                                    // real/float4 support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    // float/double/numeric support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    // boolean support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::DateTime<chrono::FixedOffset>>, _>(i) {
                                    // timestamptz support (timestamp with time zone) - preserve original timezone
                                    val.map(|v| {
                                        // Format with original timezone information preserved from PostgreSQL
                                        v.format("%Y-%m-%d %H:%M:%S %:z").to_string()
                                    })
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(i) {
                                    // timestamp support (timestamp without time zone) - consistent formatting
                                    val.map(|v| {
                                        // Format in a consistent, readable format
                                        v.format("%Y-%m-%d %H:%M:%S").to_string()
                                    })
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                                    // date support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                                    // time support
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(i) {
                                    // JSON/JSONB support with pretty printing
                                    val.map(|v| {
                                        serde_json::to_string_pretty(&v)
                                            .unwrap_or_else(|_| v.to_string())
                                    })
                                    .unwrap_or_else(|| "NULL".to_string())
                                } else {
                                    // Check if this is an array column based on column type information
                                    let column_type = column_types.get(i).map(|s| s.as_str()).unwrap_or("unknown");
                                    if column_type == "ARRAY" || column_type.ends_with("[]") {
                                        // This is an array column - try different conversion approaches
                                        log::debug!("Attempting to convert array column type '{}'", column_type);

                                        // Try String conversion first
                                        if let Ok(array_val) = row.try_get::<Option<String>, _>(i) {
                                            return array_val.map(|v| {
                                                log::debug!("Successfully converted array '{}' to String for column type '{}'", v, column_type);
                                                v
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<String> conversion
                                        if let Ok(array_val) = row.try_get::<Option<Vec<String>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| format!("\"{}\"", x))
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<String> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<i32> conversion for integer arrays
                                        if let Ok(array_val) = row.try_get::<Option<Vec<i32>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| x.to_string())
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<i32> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<i64> conversion for bigint arrays
                                        if let Ok(array_val) = row.try_get::<Option<Vec<i64>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| x.to_string())
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<i64> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Try Vec<uuid::Uuid> conversion for UUID arrays
                                        if let Ok(array_val) = row.try_get::<Option<Vec<uuid::Uuid>>, _>(i) {
                                            return array_val.map(|v| {
                                                let result = format!("{{{}}}", v.iter()
                                                    .map(|x| x.to_string())
                                                    .collect::<Vec<_>>()
                                                    .join(","));
                                                log::debug!("Successfully converted array '{:?}' to Vec<uuid::Uuid> for column type '{}': {}", v, column_type, result);
                                                result
                                            }).unwrap_or_else(|| "NULL".to_string());
                                        }

                                        // Log what SQLX types we tried
                                        log::warn!("Array column type '{}' couldn't be converted to any supported array type (String, Vec<String>, Vec<i32>, Vec<i64>, Vec<uuid::Uuid>)", column_type);
                                        "NULL".to_string()
                                    } else if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                        // text/varchar support (fallback) - also handles arrays
                                        val.map(|v| {
                                            // Check if this looks like a PostgreSQL array string
                                            if v.starts_with('{') && v.ends_with('}') {
                                                // This is already in PostgreSQL array format, return as-is
                                                log::debug!("Found PostgreSQL array string: '{}' for column type '{}'", v, column_types.get(i).unwrap_or(&"unknown".to_string()));
                                                v
                                            } else {
                                                // Regular string value
                                                v
                                            }
                                        })
                                        .unwrap_or_else(|| "NULL".to_string())
                                    } else {
                                        // Final fallback - log unmatched type for debugging
                                        let column_type = column_types.get(i).map(|s| s.as_str()).unwrap_or("unknown");
                                        log::warn!("Unmatched PostgreSQL column type '{}' at index {}, falling back to NULL", column_type, i);
                                        "NULL".to_string()
                                    }
                                }
                            })
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
                    connection_string: None,
                })
            }
            Err(_e) => {
                // If it's not a SELECT query, try executing it as a statement
                // Need to recreate the query since it was consumed by fetch_all
                let mut statement_query = sqlx::query(sql_template);
                for param in parameters {
                    statement_query = statement_query.bind(param);
                }
                let result = statement_query.execute(pool).await?;
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
                    connection_string: None,
                })
            }
        }
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        let result = self.execute_query("SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname").await?;
        let databases: Vec<String> = result.rows.into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();
        Ok(databases)
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        let result = self.execute_query("SELECT schema_name FROM information_schema.schemata ORDER BY schema_name").await?;
        let schemas: Vec<String> = result.rows.into_iter()
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

        let result = self.execute_prepared_query(query, &[table_name.to_string()]).await?;

        if !result.rows.is_empty() {
            Ok(result.rows[0].get(0).cloned())
        } else {
            Ok(None)
        }
    }

    async fn get_columns_for_table(&self, table_name: &str, schema: Option<&str>) -> Result<Vec<ColumnInfo>> {
        let schema_name = schema.unwrap_or("public");
        log::debug!("Getting columns for PostgreSQL table '{}.{}", schema_name, table_name);

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

        let result = self.execute_prepared_query(query, &[table_name.to_string(), schema_name.to_string()]).await?;

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
                let is_primary_key = if let Ok(Some(pk)) = self.get_primary_key_for_table(table_name).await {
                    pk == *column_name
                } else {
                    false
                };

                let column_info = ColumnInfo {
                    name: column_name.clone(),
                    data_type: data_type.clone(),
                    is_nullable: is_nullable == "YES",
                    is_primary_key,
                    default_value: if default_value.is_empty() { None } else { Some(default_value.clone()) },
                    character_maximum_length: max_length.parse().ok(),
                };
                columns.push(column_info);
            }
        }

        log::debug!("Found {} columns for table '{}.{}", columns.len(), schema_name, table_name);
        Ok(columns)
    }

    async fn get_table_metadata(&self, table_name: &str, schema: Option<&str>) -> Result<TableMetadata> {
        let schema_name = schema.unwrap_or("public");
        log::debug!("Getting metadata for PostgreSQL table '{}.{}", schema_name, table_name);

        // Get basic table information
        let columns = self.get_columns_for_table(table_name, Some(schema_name)).await?;

        // Get row count
        let row_count = match self.execute_prepared_query(
            &format!("SELECT COUNT(*) FROM \"{}\".\"{}\"", schema_name, table_name),
            &[]
        ).await {
            Ok(count_result) if !count_result.rows.is_empty() => {
                count_result.rows[0][0].parse().ok()
            }
            _ => None
        };

        // Extract primary key information
        let primary_keys: Vec<String> = columns.iter()
            .filter(|col| col.is_primary_key)
            .map(|col| col.name.clone())
            .collect();

        let mut metadata = TableMetadata::new(table_name.to_string(), Some(schema_name.to_string()));
        metadata.columns = columns;
        metadata.row_count = row_count;
        metadata.primary_keys = primary_keys;

        log::debug!("Retrieved metadata for table '{}.{}': {} columns, {} PKs",
                   schema_name, table_name, metadata.columns.len(), metadata.primary_keys.len());

        Ok(metadata)
    }

    async fn execute_table_changes(&self, _changes: &[TableChangeOperation]) -> Result<QueryResult> {
        // This is a placeholder - would need implementation of table operations for PostgreSQL
        Err(anyhow::anyhow!("Table changes not yet implemented for PostgreSQL"))
    }

    async fn get_database_name(&self) -> Result<Option<String>> {
        Ok(Some(self.connection_key.database.clone()))
    }

    fn get_file_safe_name(&self) -> String {
        // Create a file-safe name from PostgreSQL connection details
        let name = format!("{}_{}_{}",
            self.connection_key.username,
            self.connection_key.host,
            self.connection_key.database
        );

        // Make it file-safe
        name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
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
            pool: self.pool.clone(),
            connection_key: self.connection_key.clone(),
            display_name: self.display_name.clone(),
            connection_string: self.connection_string.clone(),
        }
    }
}