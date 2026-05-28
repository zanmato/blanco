mod connection_impl;

use anyhow::Result;
use blanco_core::{QueryResult, connection_trait::ColumnType};
use futures::StreamExt;
use hex;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::{Column, ConnectOptions, Row, TypeInfo, ValueRef};
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
