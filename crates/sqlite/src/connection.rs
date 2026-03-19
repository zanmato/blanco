use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, QueryResult, connection_trait::ColumnType,
    connection_trait::ForeignKeyInfo, connection_trait::IndexInfo,
};
use futures::{Stream, StreamExt};
use hex;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::{Column, ConnectOptions, Row, TypeInfo, ValueRef};
use std::collections::HashMap;
use std::str::FromStr;

/// SQLite connection implementation of the Connection trait
/// This uses SQLX directly to provide a unified interface
pub struct SqliteConnection {
    pool: Option<SqlitePool>,
    connection_key: SqliteConnectionKey,
    display_name: String,
    database_path: String,
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

        // Tilde expansion is not supported; paths must be absolute or relative to cwd
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
    /// Map SQLite type name to ColumnType enum
    fn map_sqlite_type(type_name: &str) -> ColumnType {
        match type_name.to_lowercase().as_str() {
            "integer" | "int" => ColumnType::Integer,
            "real" | "float" | "double" | "numeric" | "decimal" => ColumnType::Numeric,
            "text" | "varchar" => ColumnType::Text,
            "boolean" => ColumnType::Boolean,
            "date" | "datetime" => ColumnType::DateTime,
            "blob" => ColumnType::Binary,
            _ => ColumnType::Unknown,
        }
    }

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

    /// Helper method to connect asynchronously
    async fn connect_async(&mut self, database_path: &str) -> Result<sqlx::SqlitePool> {
        let options = SqliteConnectOptions::from_str(database_path)?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;
        Ok(pool)
    }

    /// Execute a query asynchronously using SQLX directly
    async fn execute_query_async(
        &self,
        query: &str,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        // Build query: use raw_sql for no parameters, otherwise bind parameters
        let sql_query = match parameters {
            Some(params) if !params.is_empty() => {
                let mut q = sqlx::query(query);
                for param in params {
                    q = q.bind(param);
                }
                #[allow(deprecated)]
                q.fetch_many(pool)
            }
            _ => sqlx::raw_sql(query).fetch_many(pool),
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
                                let raw_type = col.type_info().name().to_string();
                                (Self::map_sqlite_type(&raw_type), raw_type)
                            })
                            .unzip();
                        column_types = types;
                        raw_column_types = raw_types;
                    }

                    // Convert row to strings immediately to avoid memory doubling
                    let row_data: Vec<Option<String>> = columns
                        .iter()
                        .enumerate()
                        .map(|(i, _)| {
                            convert_sqlite_row_value_to_string(
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

        Ok(QueryResult {
            columns,
            column_types,
            rows,
            rows_affected,
            query_text: None,
            execution_time_ms: None,
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        })
    }

    /// Generate a human-readable display name for the connection
    fn generate_display_name(database_path: &str) -> String {
        let path = std::path::Path::new(database_path);

        // Try to get just the filename
        if let Some(file_name) = path.file_name()
            && let Some(name_str) = file_name.to_str()
        {
            // Remove the .sqlite, .db, or .db3 extension if present
            let name = if let Some(dot_pos) = name_str.rfind('.') {
                &name_str[..dot_pos]
            } else {
                name_str
            };

            return format!("SQLite - {}", name);
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
    fn get_connection_type(&self) -> &'static str {
        "SQLite"
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        tracing::info!(
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

        tracing::info!("Successfully connected to SQLite database");
        Ok(())
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
        parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        // SQLite only has one database, so we ignore the database parameter and execute normally
        self.execute_query_async(query, parameters).await
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        // SQLite has a main schema by default, plus any attached databases
        let mut schemas = vec!["main".to_string()];

        // Try to get attached databases
        match self.execute_query("PRAGMA database_list", None, None).await {
            Ok(result) => {
                for row in &result.rows {
                    if let Some(Some(schema_name)) = row.get(1)
                        && schema_name != "main"
                        && !schemas.contains(schema_name)
                    {
                        schemas.push(schema_name.clone());
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Could not get database list: {}", e);
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

        let result = self.execute_query(&query, None, None).await?;
        let tables: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect();

        tracing::debug!(
            "Found {} tables in schema '{}'",
            tables.len(),
            schema_filter
        );
        Ok(tables)
    }

    async fn get_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let schema_filter = schema.unwrap_or("main");
        let query = format!(
            "SELECT name FROM {}.sqlite_master WHERE type='view' ORDER BY name",
            schema_filter
        );

        let result = self.execute_query(&query, None, None).await?;
        let views: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect();

        Ok(views)
    }

    async fn get_queryable_entities(
        &self,
        schema: Option<&str>,
    ) -> Result<Vec<blanco_core::connection_trait::QueryableEntity>, anyhow::Error> {
        let schema_filter = schema.unwrap_or("main");
        let query = format!(
            "SELECT name, \
                CASE WHEN type = 'view' THEN 'VIEW' ELSE 'TABLE' END as entity_type \
             FROM {}.sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
            schema_filter
        );

        let result = self.execute_query(&query, None, None).await?;

        use blanco_core::connection_trait::{EntityType, QueryableEntity};
        let entities: Vec<QueryableEntity> = result
            .rows
            .into_iter()
            .filter_map(|row| {
                if row.len() >= 2 {
                    let name = row[0].clone()?;
                    let entity_type_str = row[1].as_deref().unwrap_or("");
                    let entity_type = match entity_type_str {
                        "VIEW" => EntityType::View,
                        _ => EntityType::Table,
                    };
                    Some(QueryableEntity { name, entity_type })
                } else {
                    None
                }
            })
            .collect();

        Ok(entities)
    }

    fn supports_schemas(&self) -> bool {
        false // SQLite doesn't support schemas in the traditional sense
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        tracing::debug!("Getting columns for SQLite table '{}'", table_name);

        // Fetch column info with PK data
        let query = format!("PRAGMA table_info({})", table_name);
        let result = self.execute_query(&query, None, None).await?;

        // Fetch foreign key info in parallel
        let fk_query = format!("PRAGMA foreign_key_list({})", table_name);
        let fk_result = self.execute_query(&fk_query, None, None).await?;

        // Build FK lookup map
        let mut foreign_keys: HashMap<String, ForeignKeyInfo> = HashMap::new();
        for row in fk_result.rows {
            if row.len() >= 5
                && let (Some(Some(from_column)), Some(Some(to_table)), Some(Some(to_column))) =
                    (row.get(3), row.get(2), row.get(4))
            {
                foreign_keys.insert(
                    from_column.clone(),
                    ForeignKeyInfo {
                        foreign_table_name: to_table.clone(),
                        foreign_column_name: to_column.clone(),
                        constraint_name: None,
                    },
                );
            }
        }

        let mut columns = Vec::new();
        for row in result.rows {
            if row.len() >= 6 {
                // PRAGMA table_info returns: cid, name, type, notnull, dflt_value, pk
                let column_name = row[1].clone().unwrap_or_default();
                let data_type = row[2].clone().unwrap_or_default();
                let not_null = row[3].as_deref().unwrap_or(""); // 1 for NOT NULL, 0 for nullable
                let default_value = row[4].clone(); // Default value or None
                let is_primary_key = row[5].as_deref().unwrap_or(""); // 1 for PK, 0 for not PK

                let column_info = ColumnInfo {
                    name: column_name.clone(),
                    data_type,
                    is_nullable: not_null != "1", // Reverse logic: notnull=1 means NOT NULL
                    is_primary_key: is_primary_key == "1",
                    default_value: default_value.filter(|v| !v.is_empty()),
                    character_maximum_length: None, // SQLite doesn't provide this info in PRAGMA
                    foreign_key: foreign_keys.get(&column_name).cloned(),
                };
                columns.push(column_info);
            }
        }

        tracing::debug!("Found {} columns for table '{}'", columns.len(), table_name);
        Ok(columns)
    }

    async fn execute_query_stream_rows(
        &self,
        query: &str,
        _database_name: Option<&str>, // SQLite doesn't support multiple databases
    ) -> Result<
        (
            Vec<String>,
            Vec<ColumnType>,
            Box<dyn Stream<Item = Result<Vec<Option<String>>, anyhow::Error>> + Send + Unpin>,
        ),
        anyhow::Error,
    > {
        tracing::debug!("Executing SQLite streaming query: {}", query);

        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SQLite connection not established"))?;

        // Use sqlx::query().fetch() for true streaming
        let rows_stream = sqlx::query(query).fetch(pool);

        // We need to collect all rows first to get column information since sqlx streams
        // don't allow us to peek at the first row without consuming it
        let mut rows = Vec::new();
        let mut stream = rows_stream;
        let mut columns = Vec::new();
        let mut column_types = Vec::new();
        let mut raw_column_types = Vec::new();

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

                let (types, raw_types): (Vec<ColumnType>, Vec<String>) = row
                    .columns()
                    .iter()
                    .map(|col| {
                        let raw_type = col.type_info().name().to_string();
                        (Self::map_sqlite_type(&raw_type), raw_type)
                    })
                    .unzip();
                column_types = types;
                raw_column_types = raw_types;

                let row_data: Vec<Option<String>> = (0..columns.len())
                    .map(|i| {
                        convert_sqlite_row_value_to_string(
                            &row,
                            i,
                            &column_types,
                            &raw_column_types,
                        )
                    })
                    .collect();

                first_row_data = Some(row_data.clone());
                rows.push(row_data);
            } else {
                // Process subsequent rows
                let row_data: Vec<Option<String>> = (0..columns.len())
                    .map(|i| {
                        convert_sqlite_row_value_to_string(
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

        if rows.is_empty() {
            return Ok((vec![], vec![], Box::new(futures::stream::empty())));
        }

        // Create stream from collected rows
        let all_rows_stream = futures::stream::iter(rows.into_iter().map(Ok));

        Ok((columns, column_types, Box::new(all_rows_stream)))
    }

    async fn get_database_schema_paginated(
        &self,
        _database_name: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<blanco_core::connection_trait::DatabaseSchemaResult> {
        let limit = limit.unwrap_or(20).min(100); // Default 20, max 100
        let offset = offset.unwrap_or(0);

        let tables = self
            .get_schema_paginated(table_names, limit, offset)
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

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult, anyhow::Error> {
        // SQLite doesn't provide table statistics, so always fetch only the referenced row
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
        tracing::debug!("Getting indexes for SQLite table '{}'", table_name);

        // Get list of indexes for the table
        // PRAGMA index_list returns: seq, name, unique, origin, partial
        let list_query = format!("PRAGMA index_list({})", table_name);
        let list_result = self.execute_query(&list_query, None, None).await?;

        let mut indexes = Vec::new();
        for row in list_result.rows {
            if row.len() >= 3 {
                let index_name = row[1].clone().unwrap_or_default();
                let is_unique = row[2].as_deref() == Some("1");

                // Get columns for this index
                // PRAGMA index_info returns: seqno, cid, name
                let info_query = format!("PRAGMA index_info({})", index_name);
                let info_result = self.execute_query(&info_query, None, None).await?;

                let column_names: Vec<String> = info_result
                    .rows
                    .iter()
                    .filter_map(|r| r.get(2).and_then(|v| v.clone()))
                    .collect();

                // SQLite uses B-tree for all indexes
                let index_info = IndexInfo {
                    name: index_name,
                    algorithm: "btree".to_string(),
                    is_unique,
                    column_names,
                    condition: None, // SQLite partial index WHERE clause not easily accessible
                    comment: None,
                };
                indexes.push(index_info);
            }
        }

        tracing::debug!("Found {} indexes for table '{}'", indexes.len(), table_name);
        Ok(indexes)
    }
}

// Helper functions for SQLite type conversion

/// Check if a value is NULL without attempting type conversion
fn is_null_value(row: &sqlx::sqlite::SqliteRow, column_index: usize) -> bool {
    if let Ok(raw_value) = row.try_get_raw(column_index) {
        raw_value.is_null()
    } else {
        false
    }
}

/// Handle SQLite integer types using the raw type name (already lowercased)
fn handle_integer_type(
    row: &sqlx::sqlite::SqliteRow,
    column_index: usize,
    raw_type: &str,
) -> Option<String> {
    // BIGINT, INT8
    if (raw_type.contains("bigint") || raw_type == "int8")
        && let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index)
    {
        return Some(v.to_string());
    }

    // SMALLINT, INT2
    if (raw_type.contains("smallint") || raw_type == "int2")
        && let Ok(Some(v)) = row.try_get::<Option<i16>, _>(column_index)
    {
        return Some(v.to_string());
    }

    // TINYINT
    if raw_type.contains("tinyint")
        && let Ok(Some(v)) = row.try_get::<Option<i8>, _>(column_index)
    {
        return Some(v.to_string());
    }

    // INTEGER, INT, INT4, MEDIUMINT
    if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(column_index) {
        return Some(v.to_string());
    }
    // Fallback to i64
    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
        return Some(v.to_string());
    }
    None
}

/// Handle SQLite numeric types using the raw type name (already lowercased)
fn handle_numeric_type(
    row: &sqlx::sqlite::SqliteRow,
    column_index: usize,
    raw_type: &str,
) -> Option<String> {
    // DECIMAL, NUMERIC
    if raw_type.contains("decimal") || raw_type.contains("numeric") {
        if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
            return Some(v.to_string());
        }
        if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index) {
            return Some(v.to_string());
        }
    }

    // REAL, DOUBLE, FLOAT
    if (raw_type.contains("real") || raw_type.contains("double") || raw_type.contains("float"))
        && let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index)
    {
        return Some(v.to_string());
    }

    // Fallback for NUMERIC affinity: try f64 then i64
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index) {
        return Some(v.to_string());
    }
    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
        return Some(v.to_string());
    }
    None
}

/// Handle SQLite datetime types using the raw type name (already lowercased)
fn handle_datetime_type(
    row: &sqlx::sqlite::SqliteRow,
    column_index: usize,
    raw_type: &str,
) -> Option<String> {
    // DATE
    if raw_type == "date"
        && let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDate>, _>(column_index)
    {
        return Some(v.format("%Y-%m-%d").to_string());
    }

    // TIME
    if raw_type == "time"
        && let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveTime>, _>(column_index)
    {
        return Some(v.format("%H:%M:%S").to_string());
    }

    // DATETIME, TIMESTAMP
    if (raw_type.contains("datetime") || raw_type.contains("timestamp"))
        && let Ok(Some(v)) = row.try_get::<Option<chrono::NaiveDateTime>, _>(column_index)
    {
        return Some(v.format("%Y-%m-%d %H:%M:%S").to_string());
    }

    // Fallback: try string conversion (SQLite often stores dates as strings)
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(column_index) {
        return Some(v);
    }
    None
}

/// Helper function for SQLite type conversion using column-type-first approach
fn convert_sqlite_row_value_to_string(
    row: &sqlx::sqlite::SqliteRow,
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
        .map(|s| s.to_lowercase())
        .unwrap_or_default();

    // Handle NULL values immediately
    if is_null_value(row, column_index) {
        return None;
    }

    // Route based on SQLite type affinity
    match column_type {
        ColumnType::Integer => handle_integer_type(row, column_index, &raw_type),
        ColumnType::Numeric => handle_numeric_type(row, column_index, &raw_type),
        ColumnType::Text => {
            if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
                return val;
            }
            None
        }
        ColumnType::Boolean => {
            if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(column_index) {
                return Some(if v { "true" } else { "false" }.to_string());
            }
            None
        }
        ColumnType::DateTime => handle_datetime_type(row, column_index, &raw_type),
        ColumnType::Json => {
            if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
                return val;
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
            // SQLite's dynamic typing: try common types in order
            if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(column_index) {
                return Some(v.to_string());
            }
            if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(column_index) {
                return Some(v.to_string());
            }
            if let Ok(val) = row.try_get::<Option<String>, _>(column_index) {
                return val;
            }
            None
        }
        _ => None,
    }
}
