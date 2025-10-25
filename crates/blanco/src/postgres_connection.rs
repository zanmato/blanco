use crate::connection_trait::{Connection, ConnectionFactory, QueryResult};
use crate::db_service::PgConnectionKey;
use crate::icon::IconName;
use crate::postgres::PostgresManager;
use async_trait::async_trait;
use std::sync::Arc;

/// PostgreSQL connection implementation of the Connection trait
/// This wraps the existing PostgresManager to provide a unified interface
pub struct PostgresConnection {
    manager: Arc<PostgresManager>,
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

impl PostgresConnection {
    /// Create a new PostgreSQL connection from connection details
    pub fn new(host: String, port: u16, database: String, username: String, password: Option<String>) -> Self {
        let connection_key = PgConnectionKey {
            host: host.clone(),
            port,
            database: database.clone(),
            username: username.clone(),
            password: password.clone(),
        };

        let display_name = Self::generate_display_name(&connection_key);
        let connection_string = Self::build_connection_string(&connection_key);

        Self {
            manager: Arc::new(PostgresManager::new()),
            connection_key,
            display_name,
            connection_string,
        }
    }

    /// Create a new PostgreSQL connection from a connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self, anyhow::Error> {
        let connection_key = PgConnectionKey::from_connection_string(connection_string)?;
        let display_name = Self::generate_display_name(&connection_key);

        Ok(Self {
            manager: Arc::new(PostgresManager::new()),
            connection_key,
            display_name,
            connection_string: connection_string.to_string(),
        })
    }

    /// Create a new PostgreSQL connection from a PgConnectionKey
    pub fn from_key(connection_key: PgConnectionKey) -> Self {
        let display_name = Self::generate_display_name(&connection_key);
        let connection_string = Self::build_connection_string(&connection_key);

        Self {
            manager: Arc::new(PostgresManager::new()),
            connection_key,
            display_name,
            connection_string,
        }
    }

    /// Generate a human-readable display name for the connection
    fn generate_display_name(key: &PgConnectionKey) -> String {
        format!("PostgreSQL - {}@{}:{}/{}", key.username, key.host, key.port, key.database)
    }

    /// Build a connection string from a PgConnectionKey
    fn build_connection_string(key: &PgConnectionKey) -> String {
        if let Some(ref password) = key.password {
            format!("postgresql://{}:{}@{}:{}/{}", key.username, password, key.host, key.port, key.database)
        } else {
            format!("postgresql://{}@{}:{}/{}", key.username, key.host, key.port, key.database)
        }
    }

    /// Get connection details
    pub fn get_connection_details(&self) -> (&str, u16, &str, &str) {
        (&self.connection_key.host, self.connection_key.port, &self.connection_key.database, &self.connection_key.username)
    }

    /// Test if we can connect to the PostgreSQL server
    pub async fn test_connectivity(&self) -> Result<bool, anyhow::Error> {
        // Create a temporary manager to test connectivity
        let mut test_manager = PostgresManager::new();
        match test_manager.connect_async(&self.connection_string).await {
            Ok(_) => {
                test_manager.disconnect().await;
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Get PostgreSQL server version
    pub async fn get_server_version(&self) -> Result<String, anyhow::Error> {
        let result = self.execute_query("SELECT version()").await?;
        if !result.rows.is_empty() && !result.rows[0].is_empty() {
            Ok(result.rows[0][0].clone())
        } else {
            Ok("Unknown".to_string())
        }
    }

    /// Get PostgreSQL server settings
    pub async fn get_server_settings(&self) -> Result<Vec<(String, String)>, anyhow::Error> {
        let result = self.execute_query("SHOW ALL").await?;
        let mut settings = Vec::new();

        for row in &result.rows {
            if row.len() >= 2 {
                settings.push((row[0].clone(), row[1].clone()));
            }
        }

        Ok(settings)
    }
}

#[async_trait]
impl Connection for PostgresConnection {
    fn get_connection_key_str(&self) -> String {
        format!("postgres:{}", self.connection_string)
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }

    fn get_icon_name(&self) -> IconName {
        IconName::Postgresql
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    fn get_manager_any(&self) -> &dyn std::any::Any {
        self.manager.as_ref()
    }

    async fn connect(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
        log::info!("Connecting to PostgreSQL: {}", Self::sanitize_connection_string(connection_string));

        // Parse the connection string to update our internal state
        let new_key = PgConnectionKey::from_connection_string(connection_string)?;
        self.connection_key = new_key.clone();
        self.display_name = Self::generate_display_name(&new_key);
        self.connection_string = connection_string.to_string();

        // Create a new manager and connect
        let mut manager = PostgresManager::new();
        manager.connect_async(connection_string).await?;

        // Store the connected manager
        self.manager = Arc::new(manager);

        log::info!("Successfully connected to PostgreSQL database");
        Ok(())
    }

    async fn disconnect(&mut self) {
        log::info!("Disconnecting from PostgreSQL database: {}", self.display_name);

        // We need to get a mutable reference to disconnect, but Arc doesn't allow that
        // For now, we'll drop the connection by creating a new empty manager
        // In a future iteration, we might want to use Arc<Mutex<>> or change the design
        if let Ok(manager) = Arc::try_unwrap(self.manager.clone()) {
            // This only works if we're the only owner, otherwise we create a new manager
            let mut mutable_manager = manager;
            mutable_manager.disconnect().await;
        } else {
            // Fallback: create a new empty manager
            self.manager = Arc::new(PostgresManager::new());
        }
    }

    fn is_connected(&self) -> bool {
        self.manager.is_connected()
    }

    async fn ensure_connected(&mut self, connection_string: &str) -> Result<(), anyhow::Error> {
        if !self.is_connected() {
            log::info!("Reconnecting to PostgreSQL database");
            self.connect(connection_string).await?;
        }
        Ok(())
    }

    async fn execute_query(&self, query: &str) -> Result<QueryResult, anyhow::Error> {
        log::debug!("Executing PostgreSQL query: {}", query);

        let result = self.manager.execute_query_async(query).await?;
        log::debug!("Query executed successfully, {} rows returned", result.row_count());

        Ok(result)
    }

    async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult, anyhow::Error> {
        log::debug!("Executing prepared PostgreSQL query with {} parameters", parameters.len());

        let result = self.manager.execute_prepared_query(sql_template, parameters).await?;
        log::debug!("Prepared query executed successfully, {} rows returned", result.row_count());

        Ok(result)
    }

    async fn get_databases(&self) -> Result<Vec<String>, anyhow::Error> {
        log::debug!("Getting PostgreSQL databases");
        let databases = self.manager.get_databases().await?;
        log::debug!("Found {} databases", databases.len());
        Ok(databases)
    }

    async fn get_schemas(&self) -> Result<Vec<String>, anyhow::Error> {
        log::debug!("Getting PostgreSQL schemas");
        let schemas = self.manager.get_schemas().await?;
        log::debug!("Found {} schemas", schemas.len());
        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>, anyhow::Error> {
        let schema_name = schema.unwrap_or("public");
        log::debug!("Getting PostgreSQL tables for schema: {}", schema_name);

        let tables = self.manager.get_tables(schema_name).await?;
        log::debug!("Found {} tables in schema '{}'", tables.len(), schema_name);

        Ok(tables)
    }

    fn supports_schemas(&self) -> bool {
        true // PostgreSQL supports schemas
    }

    async fn get_primary_key_for_table(&self, table_name: &str) -> Result<Option<String>, anyhow::Error> {
        // Query PostgreSQL's information_schema to get primary key information
        let query = format!(
            r#"
            SELECT a.attname
            FROM pg_attribute a
            JOIN pg_index i ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
            JOIN pg_class c ON c.oid = a.attrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE c.relname = $1
            AND n.nspname = 'public'
            AND i.indisprimary
            AND a.attnum > 0
            ORDER BY a.attnum
            LIMIT 1
            "#
        );

        match self.execute_query(&query).await {
            Ok(result) => {
                if !result.rows.is_empty() && !result.rows[0].is_empty() {
                    let pk_column = result.rows[0][0].clone();
                    log::debug!("Found primary key '{}' for table '{}'", pk_column, table_name);
                    Ok(Some(pk_column))
                } else {
                    log::debug!("No primary key found for table '{}'", table_name);
                    Ok(None)
                }
            }
            Err(e) => {
                log::error!("Failed to query primary key for table '{}': {}", table_name, e);
                Err(e)
            }
        }
    }

    async fn execute_table_changes(&self, changes: &[crate::table_operations::TableChangeOperation]) -> Result<crate::connection_trait::QueryResult, anyhow::Error> {
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

                    // For INSERT operations, we might want to return the inserted ID
                    // This could be enhanced with RETURNING clause for PostgreSQL
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
            Ok(crate::connection_trait::QueryResult {
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
        Ok(Some(self.connection_key.database.clone()))
    }

    async fn test_connection(&self) -> Result<bool, anyhow::Error> {
        if !self.is_connected() {
            return Ok(false);
        }

        // Try a simple PostgreSQL-specific query
        match self.execute_query("SELECT 1").await {
            Ok(_) => Ok(true),
            Err(e) => {
                log::debug!("PostgreSQL connection test failed: {}", e);
                Ok(false)
            }
        }
    }

    // === UI Integration Methods ===

    fn supports_lsp(&self) -> bool {
        true // PostgreSQL supports pg_query LSP
    }

    fn get_lsp_config(&self) -> Option<crate::connection_trait::LspConfig> {
        Some(crate::connection_trait::LspConfig {
            server_name: "pg_query".to_string(),
            connection_args: vec![
                format!("--host={}", self.connection_key.host),
                format!("--port={}", self.connection_key.port),
                format!("--dbname={}", self.connection_key.database),
                format!("--user={}", self.connection_key.username),
            ],
            workspace_path: None,
        })
    }

    fn get_file_safe_name(&self) -> String {
        // Create a file-safe name from PostgreSQL connection details
        format!("postgres_{}_{}_{}",
            self.connection_key.username,
            self.connection_key.host,
            self.connection_key.database
        ).chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect()
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

impl PostgresConnection {
    /// Sanitize connection string for logging (hide password)
    fn sanitize_connection_string(conn_str: &str) -> String {
        if let Some(at_pos) = conn_str.find('@') {
            if let Some(proto_end) = conn_str.find("://") {
                let proto = &conn_str[..proto_end + 3];
                let user_part = &conn_str[proto_end + 3..at_pos];
                let rest = &conn_str[at_pos..];

                if let Some(colon_pos) = user_part.find(':') {
                    let user = &user_part[..colon_pos];
                    return format!("{}{}:***{}", proto, user, rest);
                }
            }
        }
        conn_str.to_string()
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

/// Factory for creating PostgreSQL connections
pub struct PostgresConnectionFactory;

#[async_trait]
impl ConnectionFactory for PostgresConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>, anyhow::Error> {
        let mut conn = PostgresConnection::from_connection_string(connection_string)?;
        conn.connect(connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String, anyhow::Error> {
        PgConnectionKey::from_connection_string(connection_string)?;
        Ok(connection_string.to_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn get_test_connection_string() -> String {
        env::var("POSTGRES_CONNECTION_STRING")
            .unwrap_or_else(|_| "postgres://postgres:Bongotrumma24!@localhost:5432/bylyngamanager?sslmode=disable&timezone=Europe/Stockholm".to_string())
    }

    #[test]
    fn test_postgres_connection_from_connection_string() {
        let conn_string = "postgres://user:pass@localhost:5432/testdb";
        let conn = PostgresConnection::from_connection_string(conn_string).unwrap();

        assert_eq!(conn.connection_key.host, "localhost");
        assert_eq!(conn.connection_key.port, 5432);
        assert_eq!(conn.connection_key.database, "testdb");
        assert_eq!(conn.connection_key.username, "user");
        assert_eq!(conn.connection_key.password, Some("pass".to_string()));
    }

    #[test]
    fn test_postgres_connection_from_details() {
        let conn = PostgresConnection::new(
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "user".to_string(),
            Some("pass".to_string()),
        );

        assert_eq!(conn.connection_key.host, "localhost");
        assert_eq!(conn.connection_key.port, 5432);
        assert_eq!(conn.connection_key.database, "testdb");
        assert_eq!(conn.connection_key.username, "user");
        assert_eq!(conn.connection_key.password, Some("pass".to_string()));
    }

    #[test]
    fn test_generate_display_name() {
        let conn = PostgresConnection::new(
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "user".to_string(),
            Some("pass".to_string()),
        );

        assert_eq!(conn.get_display_name(), "PostgreSQL - user@localhost:5432/testdb");
    }

    #[test]
    fn test_build_connection_string() {
        let key = PgConnectionKey {
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            username: "user".to_string(),
            password: Some("pass".to_string()),
        };

        let conn_string = PostgresConnection::build_connection_string(&key);
        assert_eq!(conn_string, "postgresql://user:pass@localhost:5432/testdb");
    }

    #[test]
    fn test_build_connection_string_no_password() {
        let key = PgConnectionKey {
            host: "localhost".to_string(),
            port: 5432,
            database: "testdb".to_string(),
            username: "user".to_string(),
            password: None,
        };

        let conn_string = PostgresConnection::build_connection_string(&key);
        assert_eq!(conn_string, "postgresql://user@localhost:5432/testdb");
    }

    #[tokio::test]
    #[ignore] // Ignored by default since it requires a running PostgreSQL server
    async fn test_postgres_connection_lifecycle() {
        let connection_string = get_test_connection_string();

        let mut conn = match PostgresConnection::from_connection_string(&connection_string) {
            Ok(conn) => conn,
            Err(e) => {
                println!("Skipping PostgreSQL test - invalid connection string: {}", e);
                return;
            }
        };

        // Initially not connected
        assert!(!conn.is_connected());

        // Connect
        match conn.connect(&connection_string).await {
            Ok(_) => {
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
            Err(e) => {
                println!("Skipping PostgreSQL test - connection failed: {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore] // Ignored by default since it requires a running PostgreSQL server
    async fn test_postgres_schema_operations() {
        let connection_string = get_test_connection_string();

        let mut conn = match PostgresConnection::from_connection_string(&connection_string) {
            Ok(conn) => conn,
            Err(e) => {
                println!("Skipping PostgreSQL test - invalid connection string: {}", e);
                return;
            }
        };

        match conn.connect(&connection_string).await {
            Ok(_) => {
                // Test get_schemas
                let schemas = conn.get_schemas().await.unwrap();
                assert!(!schemas.is_empty());
                assert!(schemas.contains(&"public".to_string()));

                // Test get_tables
                let tables = conn.get_tables(Some("public")).await.unwrap();
                // Tables could be empty, but the query should succeed
                println!("Found {} tables in public schema", tables.len());

                // Test get_databases
                let databases = conn.get_databases().await.unwrap();
                assert!(!databases.is_empty());
            }
            Err(e) => {
                println!("Skipping PostgreSQL test - connection failed: {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore] // Ignored by default since it requires a running PostgreSQL server
    async fn test_postgres_prepared_query() {
        let connection_string = get_test_connection_string();

        let mut conn = match PostgresConnection::from_connection_string(&connection_string) {
            Ok(conn) => conn,
            Err(e) => {
                println!("Skipping PostgreSQL test - invalid connection string: {}", e);
                return;
            }
        };

        match conn.connect(&connection_string).await {
            Ok(_) => {
                // Create a temporary table for testing
                conn.execute_query("CREATE TEMP TABLE test_table (id INTEGER, name TEXT)").await.unwrap();

                // Insert data using prepared query
                let sql_template = "INSERT INTO test_table (id, name) VALUES ($1, $2)";
                let parameters = vec!["1".to_string(), "test_name".to_string()];

                let result = conn.execute_prepared_query(sql_template, &parameters).await.unwrap();
                assert_eq!(result.rows_affected, 1);

                // Query the data back
                let select_result = conn.execute_query("SELECT id, name FROM test_table").await.unwrap();
                assert_eq!(select_result.rows.len(), 1);
                assert_eq!(select_result.rows[0][0], "1");
                assert_eq!(select_result.rows[0][1], "test_name");
            }
            Err(e) => {
                println!("Skipping PostgreSQL test - connection failed: {}", e);
            }
        }
    }

    #[tokio::test]
    #[ignore] // Ignored by default since it requires a running PostgreSQL server
    async fn test_postgres_connection_factory() {
        let connection_string = get_test_connection_string();
        let factory = PostgresConnectionFactory;

        // Test parsing
        match factory.parse_connection_string(&connection_string) {
            Ok(key) => {
                assert_eq!(key.host, "localhost");
                assert_eq!(key.port, 5432);
                assert_eq!(key.database, "bylyngamanager");
                assert_eq!(key.username, "postgres");

                // Test creation
                match factory.create_connection(&connection_string).await {
                    Ok(conn) => {
                        assert!(conn.is_connected());
                        assert!(conn.get_display_name().contains("PostgreSQL"));
                    }
                    Err(e) => {
                        println!("Skipping PostgreSQL factory test - connection failed: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("Skipping PostgreSQL factory test - parsing failed: {}", e);
            }
        }
    }
}