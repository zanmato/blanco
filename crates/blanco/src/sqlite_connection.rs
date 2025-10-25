use crate::connection_trait::{Connection, ConnectionFactory, QueryResult};
use crate::icon::IconName;
use async_trait::async_trait;
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
    pub fn from_connection_string(connection_string: &str) -> Result<Self, anyhow::Error> {
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

        // Expand ~ to home directory if present
        let expanded_path = if let Some(home) = dirs::home_dir() {
            if path.starts_with('~') {
                path.replacen('~', &home.to_string_lossy(), 1)
            } else {
                path.to_string()
            }
        } else {
            path.to_string()
        };

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
    pub fn new(database_path: String) -> Result<Self, anyhow::Error> {
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

    // Helper methods that were previously provided by DatabaseManager

    /// Helper method to connect asynchronously
    async fn connect_async(&mut self, database_path: &str) -> Result<(), sqlx::Error> {
        let options = SqliteConnectOptions::from_str(database_path)?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;
        self.pool = Some(pool);
        Ok(())
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
    async fn execute_query_async(&self, query: &str) -> Result<QueryResult, Box<dyn std::error::Error>> {
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
    pub fn get_database_size(&self) -> Result<u64, anyhow::Error> {
        let metadata = std::fs::metadata(&self.database_path)?;
        Ok(metadata.len())
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
        IconName::Sqlite
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    fn get_manager_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
        log::info!("Connecting to SQLite database: {}", self.get_sanitize_path(connection_string));

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
        let options = SqliteConnectOptions::from_str(&self.database_path)?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;
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

    async fn ensure_connected(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
        if !self.is_connected() || !self.is_connection_healthy().await {
            log::info!("Reconnecting to SQLite database");
            self.connect(connection_string).await?;
        }
        Ok(())
    }

    async fn execute_query(&self, query: &str) -> Result<QueryResult, anyhow::Error> {
        log::debug!("Executing SQLite query: {}", query);

        let result = self.execute_query_async(query).await
            .map_err(|e| anyhow::anyhow!("SQLite query execution failed: {}", e))?;
        log::debug!("Query executed successfully, {} rows returned", result.row_count());

        Ok(result)
    }

    async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult, anyhow::Error> {
        log::debug!("Executing prepared SQLite query with {} parameters", parameters.len());

        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

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
                })
            }
        }
    }

    async fn get_databases(&self) -> Result<Vec<String>, anyhow::Error> {
        // SQLite has a single database, so we return the current database name
        let db_name = std::path::Path::new(&self.database_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();

        Ok(vec![db_name])
    }

    async fn get_schemas(&self) -> Result<Vec<String>, anyhow::Error> {
        // SQLite has a main schema by default, plus any attached databases
        let mut schemas = vec!["main".to_string()];

        // Try to get attached databases
        match self.execute_query("PRAGMA database_list").await {
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

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        let schema_filter = schema.unwrap_or("main");
        let query = format!(
            "SELECT name FROM {}.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            schema_filter
        );

        let result = self.execute_query(&query).await?;
        let tables: Vec<String> = result.rows.into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();

        log::debug!("Found {} tables in schema '{}'", tables.len(), schema_filter);
        Ok(tables)
    }

    fn supports_schemas(&self) -> bool {
        false // SQLite doesn't support schemas in the traditional sense
    }

    async fn get_primary_key_for_table(&self, table_name: &str) -> Result<Option<String>, anyhow::Error> {
        // Query SQLite's table_info to get primary key information
        let query = format!("PRAGMA table_info({})", table_name);

        match self.execute_query(&query).await {
            Ok(result) => {
                // Find the column with pk > 0 (primary key)
                for row in &result.rows {
                    if row.len() >= 6 {
                        let column_name = &row[1];
                        let pk_info = &row[5]; // The 6th column (index 5) indicates primary key

                        // SQLite uses 1 for primary key, 0 for non-primary key
                        if pk_info == "1" {
                            log::debug!("Found primary key '{}' for table '{}'", column_name, table_name);
                            return Ok(Some(column_name.clone()));
                        }
                    }
                }
                log::debug!("No primary key found for table '{}'", table_name);
                Ok(None)
            }
            Err(e) => {
                log::error!("Failed to query primary key for table '{}': {}", table_name, e);
                Err(e)
            }
        }
    }

    async fn execute_table_changes(&self, changes: &[crate::table_operations::TableChangeOperation]) -> Result<QueryResult, anyhow::Error> {
        use crate::table_operations::OperationType;

        let mut total_affected: u64 = 0;
        let mut all_results = Vec::new();

        for change in changes {
            let sql = match &change.operation_type {
                OperationType::Update => {
                    if let Some((pk_column, pk_value)) = self.extract_pk_info(&change.row_identifier) {
                        if let Some(column_change) = change.changes.first() {
                            let column_name = &column_change.column_name;
                            let new_value = &column_change.new_value;

                            let (quoted_value, _param_value) = self.quote_value(new_value);
                            let (quoted_pk, _) = self.quote_value(&Some(pk_value.clone()));

                            format!(
                                "UPDATE {} SET {} = {} WHERE {} = {}",
                                change.table_name, column_name, quoted_value, pk_column, quoted_pk
                            )
                        } else {
                            return Err(anyhow::anyhow!("Update operation requires at least one column change"));
                        }
                    } else {
                        return Err(anyhow::anyhow!("Update operation requires primary key"));
                    }
                }
                OperationType::Insert => {
                    let column_names: Vec<String> = change.changes.iter().map(|c| c.column_name.clone()).collect();
                    let values: Vec<(String, String)> = change.changes.iter()
                        .map(|c| self.quote_value(&c.new_value))
                        .collect();

                    if column_names.is_empty() {
                        return Err(anyhow::anyhow!("Insert operation requires at least one column"));
                    }

                    let columns_str = column_names.join(", ");
                    let values_str: String = values.iter().map(|(quoted, _)| quoted.as_str()).collect::<Vec<&str>>().join(", ");

                    format!(
                        "INSERT INTO {} ({}) VALUES ({})",
                        change.table_name, columns_str, values_str
                    )
                }
                OperationType::Delete => {
                    if let Some((pk_column, pk_value)) = self.extract_pk_info(&change.row_identifier) {
                        let (quoted_pk, _) = self.quote_value(&Some(pk_value));
                        format!(
                            "DELETE FROM {} WHERE {} = {}",
                            change.table_name, pk_column, quoted_pk
                        )
                    } else {
                        return Err(anyhow::anyhow!("Delete operation requires primary key"));
                    }
                }
            };

            log::debug!("Executing SQL: {}", sql);

            match self.execute_query(&sql).await {
                Ok(result) => {
                    let affected = result.row_count();
                    total_affected += affected as u64;
                    log::debug!("SQL execution affected {} rows", affected);
                    all_results.push(result);
                }
                Err(e) => {
                    log::error!("Failed to execute table change SQL: {}", e);
                    return Err(e);
                }
            }
        }

        // Create a combined result
        if all_results.len() == 1 {
            Ok(all_results.into_iter().next().unwrap())
        } else {
            // Create a result summarizing all operations
            Ok(QueryResult {
                columns: vec!["affected_rows".to_string()],
                column_types: vec!["INTEGER".to_string()],
                rows: vec![vec![total_affected.to_string()]],
                rows_affected: total_affected,
                query_text: Some("Batch table operations".to_string()),
                execution_time_ms: None,
                is_error: false,
            })
        }
    }

    async fn get_database_name(&self) -> Result<Option<String>, anyhow::Error> {
        // For SQLite, get the database file name without extension
        let path = std::path::Path::new(&self.database_path);
        let name = path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        Ok(name)
    }

    // === UI Integration Methods ===

    fn supports_lsp(&self) -> bool {
        false // SQLite doesn't support LSP
    }

    fn get_lsp_config(&self) -> Option<crate::connection_trait::LspConfig> {
        None // SQLite doesn't support LSP
    }

    fn get_file_safe_name(&self) -> String {
        // Create a file-safe name from SQLite database path
        let path = std::path::Path::new(&self.database_path);
        let name = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("sqlite_db");

        // Make it file-safe
        name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
    }

    fn get_ui_metadata(&self) -> crate::connection_trait::ConnectionUIMetadata {
        crate::connection_trait::ConnectionUIMetadata {
            display_name: self.get_display_name(),
            file_safe_name: self.get_file_safe_name(),
            supports_schemas: self.supports_schemas(),
            supports_lsp: self.supports_lsp(),
            icon_name: self.get_icon_name(),
        }
    }

  }

impl SqliteConnection {
    /// Sanitize path for logging (remove sensitive parts if any)
    fn get_sanitize_path(&self, path: &str) -> String {
        // For SQLite, paths are generally not sensitive, but we can still clean them up
        path.replace("\\", "/") // Normalize path separators
    }

    // === Helper Methods for Table Operations ===

    /// Extract primary key information from RowIdentifier
    fn extract_pk_info(&self, row_identifier: &crate::table_operations::RowIdentifier) -> Option<(String, String)> {
        use crate::table_operations::RowIdentifier;
        match row_identifier {
            RowIdentifier::PrimaryKey { column, value } => Some((column.clone(), value.clone())),
            RowIdentifier::RowIndex(_) => None,
        }
    }

    /// Quote a value for SQL and return both quoted and unquoted versions
    fn quote_value(&self, value: &Option<String>) -> (String, String) {
        match value {
            Some(v) => {
                if v.is_empty() {
                    ("NULL".to_string(), "NULL".to_string())
                } else {
                    let clean_value = v.trim_matches('\'');
                    let quoted = format!("'{}'", clean_value.replace("'", "''"));
                    (quoted, clean_value.to_string())
                }
            }
            None => ("NULL".to_string(), "NULL".to_string()),
        }
    }
}

/// Factory for creating SQLite connections
pub struct SqliteConnectionFactory;

#[async_trait]
impl ConnectionFactory for SqliteConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>, anyhow::Error> {
        let mut conn = SqliteConnection::new(connection_string.to_string())?;
        conn.connect(connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String, anyhow::Error> {
        let key = SqliteConnectionKey::from_connection_string(connection_string)?;
        Ok(key.to_connection_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "SQLite"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::fs::File;

    #[test]
    fn test_sqlite_connection_key_from_path() {
        let test_path = "/tmp/test.db";
        let key = SqliteConnectionKey::from_connection_string(test_path).unwrap();
        assert_eq!(key.database_path, test_path);
    }

    #[test]
    fn test_sqlite_connection_key_from_sqlite_url() {
        let test_path = "/tmp/test.db";
        let url = format!("sqlite://{}", test_path);
        let key = SqliteConnectionKey::from_connection_string(&url).unwrap();
        assert_eq!(key.database_path, test_path);
    }

    #[test]
    fn test_sqlite_connection_key_from_sqlite_prefix() {
        let test_path = "/tmp/test.db";
        let url = format!("sqlite:{}", test_path);
        let key = SqliteConnectionKey::from_connection_string(&url).unwrap();
        assert_eq!(key.database_path, test_path);
    }

    #[test]
    fn test_generate_display_name() {
        assert_eq!(
            SqliteConnection::generate_display_name("/path/to/mydb.sqlite"),
            "SQLite - mydb"
        );
        assert_eq!(
            SqliteConnection::generate_display_name("/path/to/mydb.db"),
            "SQLite - mydb"
        );
        assert_eq!(
            SqliteConnection::generate_display_name("/path/to/mydb"),
            "SQLite - mydb"
        );
    }

    #[tokio::test]
    async fn test_sqlite_connection_lifecycle() {
        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        let mut conn = SqliteConnection::new(connection_string.clone()).unwrap();

        // Initially not connected
        assert!(!conn.is_connected());

        // Connect
        conn.connect(&connection_string).await.unwrap();
        assert!(conn.is_connected());

        // Test a simple query
        let result = conn.execute_query("SELECT 1 as test_column").await.unwrap();
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], "1");

        // Disconnect
        conn.disconnect().await;
        assert!(!conn.is_connected());
    }

    #[tokio::test]
    async fn test_sqlite_schema_operations() {
        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        let mut conn = SqliteConnection::new(connection_string.clone()).unwrap();
        conn.connect(&connection_string).await.unwrap();

        // Create a test table
        conn.execute_query("CREATE TABLE test_table (id INTEGER, name TEXT)").await.unwrap();

        // Test get_tables
        let tables = conn.get_tables(None).await.unwrap();
        assert!(tables.contains(&"test_table".to_string()));

        // Test get_schemas
        let schemas = conn.get_schemas().await.unwrap();
        assert!(schemas.contains(&"main".to_string()));

        // Test get_databases
        let databases = conn.get_databases().await.unwrap();
        assert!(!databases.is_empty());
    }

    #[tokio::test]
    async fn test_sqlite_prepared_query() {
        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        let mut conn = SqliteConnection::new(connection_string.clone()).unwrap();
        conn.connect(&connection_string).await.unwrap();

        // Create a test table
        conn.execute_query("CREATE TABLE test_table (id INTEGER, name TEXT)").await.unwrap();

        // Insert data using prepared query
        let sql_template = "INSERT INTO test_table (id, name) VALUES (?, ?)";
        let parameters = vec!["1".to_string(), "test_name".to_string()];

        let result = conn.execute_prepared_query(sql_template, &parameters).await.unwrap();
        assert_eq!(result.rows_affected, 1);

        // Query the data back
        let select_result = conn.execute_query("SELECT id, name FROM test_table").await.unwrap();
        assert_eq!(select_result.rows.len(), 1);
        assert_eq!(select_result.rows[0][0], "1");
        assert_eq!(select_result.rows[0][1], "test_name");
    }

    #[tokio::test]
    async fn test_sqlite_connection_factory() {
        // Create a temporary database file
        let temp_file = NamedTempFile::new().unwrap();
        let db_path = temp_file.path().to_string_lossy().to_string();
        let connection_string = format!("sqlite://{}", db_path);

        let factory = SqliteConnectionFactory;

        // Test parsing
        let key = factory.parse_connection_string(&connection_string).unwrap();
        assert_eq!(key.database_path, db_path);

        // Test creation
        let conn = factory.create_connection(&connection_string).await.unwrap();
        assert!(conn.is_connected());
        assert_eq!(conn.get_display_name(), "SQLite - tmp");
    }
}