use crate::sql_parser::SqliteTableExtractor;
use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, ConnectionUIMetadata, IconName, QueryResult, TableChangeOperation,
    TableMetadata,
};
use futures::{Stream, StreamExt};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::{Column, ConnectOptions, Row, TypeInfo};
use std::str::FromStr;

/// SQLite connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface
pub struct SqliteConnection {
    pool: Option<SqlitePool>,
    connection_key: SqliteConnectionKey,
    display_name: String,
    database_path: String,
}

impl std::fmt::Debug for SqliteConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteConnection")
            .field("connection_key", &self.connection_key)
            .field("display_name", &self.display_name)
            .field("database_path", &self.database_path)
            .finish()
    }
}

/// Connection key for SQLite connections
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SqliteConnectionKey {
    pub database_path: String,
}

impl SqliteConnectionKey {
    pub fn new(database_path: String) -> Self {
        Self { database_path }
    }

    /// Extract connection key from a SQLite connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        // Handle different SQLite connection string formats:
        // - sqlite://path/to/db.sqlite
        // - sqlite:path/to/db.sqlite
        // - path/to/db.sqlite (direct path)

        let path = if connection_string.starts_with("sqlite://") {
            connection_string.trim_start_matches("sqlite://")
        } else if connection_string.starts_with("sqlite:") {
            connection_string.trim_start_matches("sqlite:")
        } else {
            connection_string
        };

        // For now, don't expand ~ - just use the path as-is
        let expanded_path = path.to_string();

        // Convert to absolute path
        let absolute_path = if std::path::Path::new(&expanded_path).is_absolute() {
            expanded_path
        } else {
            match std::env::current_dir() {
                Ok(current_dir) => {
                    let path_buf = current_dir.join(&expanded_path);
                    path_buf.to_string_lossy().to_string()
                }
                Err(_) => expanded_path,
            }
        };

        Ok(SqliteConnectionKey {
            database_path: absolute_path,
        })
    }

    /// Generate a connection string from this key
    pub fn to_connection_string(&self) -> String {
        format!("sqlite://{}", self.database_path)
    }
}

impl SqliteConnection {
    /// Create a new SQLite connection
    pub fn new(database_path: String) -> Result<Self> {
        let connection_key = SqliteConnectionKey::from_connection_string(&database_path)?;
        let display_name = Self::generate_display_name(&connection_key.database_path);

        Ok(Self {
            pool: None,
            connection_key,
            display_name,
            database_path,
        })
    }

    /// Create a new SQLite connection from a connection key
    pub fn from_key(connection_key: SqliteConnectionKey) -> Self {
        let display_name = Self::generate_display_name(&connection_key.database_path);
        let database_path = connection_key.database_path.clone();

        Self {
            pool: None,
            connection_key,
            display_name,
            database_path,
        }
    }

    /// Helper method to connect asynchronously
    async fn connect_async(&mut self, database_path: &str) -> Result<sqlx::SqlitePool> {
        let options = SqliteConnectOptions::from_str(database_path)?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;
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
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

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

                // Extract row data
                let data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        columns
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                // Check if the value is NULL first
                                if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    val.unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<Vec<u8>>, _>(i) {
                                    // BLOB support - convert to hex string
                                    val.map(|bytes| {
                                        bytes
                                            .iter()
                                            .map(|b| format!("{:02x}", b))
                                            .collect::<String>()
                                    })
                                    .unwrap_or_else(|| "NULL".to_string())
                                } else {
                                    "NULL".to_string()
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

    /// Generate a human-readable display name for the connection
    fn generate_display_name(database_path: &str) -> String {
        let path = std::path::Path::new(database_path);

        // Try to get just the filename
        if let Some(file_name) = path.file_name() {
            if let Some(name_str) = file_name.to_str() {
                // Remove the .sqlite, .db, or .db3 extension if present
                let name = if let Some(dot_pos) = name_str.rfind('.') {
                    &name_str[..dot_pos]
                } else {
                    name_str
                };

                return format!("SQLite - {}", name);
            }
        }

        // Fallback to full path if we can't extract a nice name
        format!("SQLite - {}", database_path)
    }

    /// Get the database file path
    pub fn get_database_path(&self) -> &str {
        &self.database_path
    }

    /// Check if the database file exists
    pub fn database_file_exists(&self) -> bool {
        std::path::Path::new(&self.database_path).exists()
    }

    /// Get the size of the database file in bytes
    pub fn get_database_size(&self) -> Result<u64> {
        let metadata = std::fs::metadata(&self.database_path)?;
        Ok(metadata.len())
    }

    /// Sanitize path for logging (remove sensitive parts if any)
    fn get_sanitize_path(&self, path: &str) -> String {
        // For SQLite, paths are generally not sensitive, but we can still clean them up
        path.replace("\\", "/") // Normalize path separators
    }
}

#[async_trait]
impl Connection for SqliteConnection {
    fn get_connection_key_str(&self) -> String {
        format!("sqlite:{}", self.connection_key.database_path)
    }

    fn get_connection_type(&self) -> &'static str {
        "SQLite"
    }

    fn get_icon_name(&self) -> IconName {
        // Return DatabaseConnected when pool is Some (active connection), otherwise Database
        match self.pool {
            Some(_) => IconName::DatabaseConnected,
            None => IconName::Database,
        }
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    fn get_manager_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        log::info!(
            "Connecting to SQLite database: {}",
            self.get_sanitize_path(connection_string)
        );

        // Parse and validate the connection string
        let key = SqliteConnectionKey::from_connection_string(connection_string)?;
        self.connection_key = key.clone();
        self.database_path = key.database_path.clone();
        self.display_name = Self::generate_display_name(&key.database_path);

        // Create the database directory if it doesn't exist
        if let Some(parent) = std::path::Path::new(&self.database_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Connect using SQLX directly
        let database_path = self.database_path.clone();
        let pool = self.connect_async(&database_path).await?;
        self.pool = Some(pool);

        log::info!("Successfully connected to SQLite database");
        Ok(())
    }

    async fn disconnect(&mut self) {
        log::info!("Disconnecting from SQLite database: {}", self.display_name);
        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }
    }

    fn is_connected(&self) -> bool {
        self.pool.is_some()
    }

    async fn ensure_connected(&mut self, connection_string: &str) -> Result<()> {
        if !self.is_connected() || !self.is_connection_healthy().await {
            log::info!("Reconnecting to SQLite database");
            self.connect(connection_string).await?;
        }
        Ok(())
    }

    async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult> {
        log::debug!(
            "Executing prepared SQLite query with {} parameters",
            parameters.len()
        );

        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        // Build the query with parameter placeholders
        let mut query = sqlx::query(sql_template);

        // Add parameters to the query (SQLite uses 1-based indexing)
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

                // Extract row data
                let data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        columns
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                // Check if the value is NULL first
                                if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    val.unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    val.map(|v| v.to_string())
                                        .unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<Vec<u8>>, _>(i) {
                                    // BLOB support - convert to hex string
                                    val.map(|bytes| {
                                        bytes
                                            .iter()
                                            .map(|b| format!("{:02x}", b))
                                            .collect::<String>()
                                    })
                                    .unwrap_or_else(|| "NULL".to_string())
                                } else {
                                    "NULL".to_string()
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
                    connection_id: None,
                })
            }
        }
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        // SQLite has a single database, so we return the current database name
        let db_name = std::path::Path::new(&self.database_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();

        Ok(vec![db_name])
    }

    async fn execute_query(
        &self,
        query: &str,
        _database_name: Option<&str>,
    ) -> Result<QueryResult> {
        // SQLite only has one database, so we ignore the database parameter and execute normally
        self.execute_query_async(query).await
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        // SQLite has a main schema by default, plus any attached databases
        let mut schemas = vec!["main".to_string()];

        // Try to get attached databases
        match self.execute_query("PRAGMA database_list", None).await {
            Ok(result) => {
                for row in &result.rows {
                    if let Some(schema_name) = row.get(1) {
                        if schema_name != "main" && !schemas.contains(schema_name) {
                            schemas.push(schema_name.clone());
                        }
                    }
                }
            }
            Err(e) => {
                log::debug!("Could not get database list: {}", e);
            }
        }

        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let schema_filter = schema.unwrap_or("main");
        let query = format!(
            "SELECT name FROM {}.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            schema_filter
        );

        let result = self.execute_query(&query, None).await?;
        let tables: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();

        log::debug!(
            "Found {} tables in schema '{}'",
            tables.len(),
            schema_filter
        );
        Ok(tables)
    }

    fn supports_schemas(&self) -> bool {
        false // SQLite doesn't support schemas in the traditional sense
    }

    async fn get_primary_key_for_table(&self, table_name: &str) -> Result<Option<String>> {
        // Query SQLite's table_info to get primary key information
        let query = format!("PRAGMA table_info({})", table_name);

        match self.execute_query(&query, None).await {
            Ok(result) => {
                // Find the column with pk > 0 (primary key)
                for row in &result.rows {
                    if row.len() >= 6 {
                        let column_name = &row[1];
                        let pk_info = &row[5]; // The 6th column (index 5) indicates primary key

                        // SQLite uses 1 for primary key, 0 for non-primary key
                        if pk_info == "1" {
                            log::debug!(
                                "Found primary key '{}' for table '{}'",
                                column_name,
                                table_name
                            );
                            return Ok(Some(column_name.clone()));
                        }
                    }
                }
                log::debug!("No primary key found for table '{}'", table_name);
                Ok(None)
            }
            Err(e) => {
                log::error!(
                    "Failed to query primary key for table '{}': {}",
                    table_name,
                    e
                );
                Err(e)
            }
        }
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        log::debug!("Getting columns for SQLite table '{}'", table_name);

        // Use PRAGMA table_info to get column information
        let query = format!("PRAGMA table_info({})", table_name);

        let result = self.execute_query(&query, None).await?;

        let mut columns = Vec::new();
        for row in result.rows {
            if row.len() >= 6 {
                // PRAGMA table_info returns: cid, name, type, notnull, dflt_value, pk
                let column_name = &row[1];
                let data_type = &row[2];
                let not_null = &row[3]; // 1 for NOT NULL, 0 for nullable
                let default_value = &row[4]; // Default value or NULL
                let is_primary_key = &row[5]; // 1 for PK, 0 for not PK

                let column_info = ColumnInfo {
                    name: column_name.clone(),
                    data_type: data_type.clone(),
                    is_nullable: not_null != "1", // Reverse logic: notnull=1 means NOT NULL
                    is_primary_key: is_primary_key == "1",
                    default_value: if default_value.is_empty() {
                        None
                    } else {
                        Some(default_value.clone())
                    },
                    character_maximum_length: None, // SQLite doesn't provide this info in PRAGMA
                };
                columns.push(column_info);
            }
        }

        log::debug!("Found {} columns for table '{}'", columns.len(), table_name);
        Ok(columns)
    }

    async fn get_table_metadata(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<TableMetadata> {
        let schema_name = schema.unwrap_or("main");
        log::debug!("Getting metadata for SQLite table '{}'", table_name);

        // Get basic table information
        let columns = self
            .get_columns_for_table(table_name, Some(schema_name))
            .await?;

        // Get row count
        let row_count = match self
            .execute_prepared_query(&format!("SELECT COUNT(*) FROM \"{}\"", table_name), &[])
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

        let mut metadata = TableMetadata::new(table_name.to_string(), None); // SQLite doesn't use schemas
        metadata.columns = columns;
        metadata.row_count = row_count;
        metadata.primary_keys = primary_keys;

        log::debug!(
            "Retrieved metadata for table '{}': {} columns, {} PKs",
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
        // This is a placeholder - would need implementation of table operations
        Err(anyhow::anyhow!(
            "Table changes not yet implemented for SQLite"
        ))
    }

    async fn get_database_name(&self) -> Result<Option<String>> {
        // For SQLite, get the database file name without extension
        let path = std::path::Path::new(&self.database_path);
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        Ok(name)
    }

    fn get_file_safe_name(&self) -> String {
        // Create a file-safe name from SQLite database path
        let path = std::path::Path::new(&self.database_path);
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("sqlite_db");

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

    fn extract_table_name_from_query(&self, query: &str, alias: bool) -> Result<Option<String>> {
        log::debug!("Extracting table name from SQLite query: {}", query);

        let extractor = SqliteTableExtractor::new();
        match extractor.extract_table(query, alias) {
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

    fn resolve_table_alias(&self, query: &str, alias: &str) -> Result<Option<String>> {
        log::debug!(
            "Resolving table alias '{}' from SQLite query: {}",
            alias,
            query
        );

        let extractor = SqliteTableExtractor::new();
        match extractor.extract_table_aliases_with_names(query) {
            Ok(aliases) => {
                for (table_name, alias_name) in aliases {
                    if alias_name == alias {
                        log::debug!(
                            "Successfully resolved alias '{}' to table '{}'",
                            alias,
                            table_name
                        );
                        return Ok(Some(table_name));
                    }
                }
                log::debug!("Alias '{}' not found in query", alias);
                Ok(None)
            }
            Err(e) => {
                log::debug!("Could not resolve table alias from query: {}", e);
                Ok(None)
            }
        }
    }

    async fn execute_query_stream_rows(
        &self,
        query: &str,
        _database_name: Option<&str>, // SQLite doesn't support multiple databases
    ) -> Result<(Vec<String>, Vec<String>, Box<dyn Stream<Item = Result<Vec<String>, anyhow::Error>> + Send + Unpin>), anyhow::Error> {
        log::debug!("Executing SQLite streaming query: {}", query);

        let pool = self.pool.as_ref().ok_or_else(|| {
            anyhow::anyhow!("SQLite connection not established")
        })?;

        // Use sqlx::query().fetch() for true streaming
        let rows_stream = sqlx::query(query).fetch(pool);

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
                columns = row.columns()
                    .iter()
                    .map(|col| col.name().to_string())
                    .collect();
                column_types = row.columns()
                    .iter()
                    .map(|col| col.type_info().name().to_string())
                    .collect();

                let row_data: Vec<String> = (0..columns.len())
                    .map(|i| convert_sqlite_row_value_to_string(&row, i, &column_types))
                    .collect();

                first_row_data = Some(row_data.clone());
                rows.push(row_data);
            } else {
                // Process subsequent rows
                let row_data: Vec<String> = (0..columns.len())
                    .map(|i| convert_sqlite_row_value_to_string(&row, i, &column_types))
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
}

// Helper function for SQLite type conversion
fn convert_sqlite_row_value_to_string(
    row: &sqlx::sqlite::SqliteRow,
    column_index: usize,
    _column_types: &[String],
) -> String {
    // SQLite type conversion - simpler than PostgreSQL
    if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
        // String support
        val.unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<i32>, _>(column_index) {
        // INTEGER support
        val.map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<i64>, _>(column_index) {
        // BIGINT support
        val.map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<f64>, _>(column_index) {
        // FLOAT/REAL support
        val.map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<bool>, _>(column_index) {
        // BOOLEAN support (SQLite 3.23+)
        val.map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index) {
        // DATETIME support
        val.map(|v| {
            // Format in a consistent, readable format
            v.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index) {
        // DATE support
        val.map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index) {
        // TIME support
        val.map(|v| v.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    } else if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(column_index) {
        // JSON support
        val.map(|v| {
            // For compact display, use regular to_string instead of pretty printing
            v.to_string()
        })
        .unwrap_or_else(|| "NULL".to_string())
    } else {
        // For unknown types, try basic conversion
        match row.try_get::<Option<String>, _>(column_index) {
            Ok(val) => val.unwrap_or_else(|| "NULL".to_string()),
            Err(_) => {
                // Last resort: try to get as string directly
                format!("NULL")
            }
        }
    }
}
