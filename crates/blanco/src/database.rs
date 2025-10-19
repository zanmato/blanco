use crate::gpui_tokio::Tokio;
use gpui::AppContext;
use gpui::Task;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::{Column, ConnectOptions, Row, TypeInfo};
use std::str::FromStr;

pub struct DatabaseManager {
    pub pool: Option<SqlitePool>,
}

impl DatabaseManager {
    pub fn new() -> Self {
        Self { pool: None }
    }

    pub fn connect<C: AppContext>(
        &mut self,
        database_path: &str,
        cx: &C,
    ) -> C::Result<Task<anyhow::Result<()>>> {
        let database_path = database_path.to_string();
        Tokio::spawn_result(cx, async move {
            let options = SqliteConnectOptions::from_str(&database_path)?
                .create_if_missing(true)
                .disable_statement_logging();

            let _pool = SqlitePool::connect_with(options).await?;
            // Note: We can't modify self from here, this needs to be handled differently
            // For now, this is a limitation of the current design
            anyhow::bail!("Database connection needs to be handled at a higher level");
        })
    }

    pub async fn execute_query_async(
        &self,
        query: &str,
    ) -> Result<QueryResult, Box<dyn std::error::Error>> {
        let pool = self.pool.as_ref().ok_or("Not connected to database")?;

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
                                    val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
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

    pub fn execute_query<C: AppContext>(
        &self,
        query: String,
        cx: &C,
    ) -> C::Result<Task<anyhow::Result<QueryResult>>> {
        let query_clone = query.clone();
        let pool = self.pool.clone();

        Tokio::spawn_result(cx, async move {
            let pool = pool.ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

            // Try to execute as a query that returns rows
            match sqlx::query(&query_clone).fetch_all(&pool).await {
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

                    // Extract column names from the first row
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
                                        val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
                                    } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                        val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
                                    } else if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                        val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
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
                    let result = sqlx::query(&query_clone).execute(&pool).await?;
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
        })
    }

    pub fn is_connected(&self) -> bool {
        self.pool.is_some()
    }

    /// Check if the database connection is healthy with a ping query
    pub async fn is_connection_healthy(&self) -> bool {
        if let Some(pool) = &self.pool {
            // Execute a simple ping query to check connection health
            match sqlx::query("SELECT 1").fetch_one(pool).await {
                Ok(_) => true,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    pub async fn disconnect(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }
    }

    // Helper method to set the pool from outside the async context
    pub fn set_pool(&mut self, pool: SqlitePool) {
        self.pool = Some(pool);
    }

    // Helper method to connect asynchronously
    pub async fn connect_async(&mut self, database_path: &str) -> Result<(), sqlx::Error> {
        let options = SqliteConnectOptions::from_str(database_path)?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;
        self.pool = Some(pool);
        Ok(())
    }

    /// Execute a query with prepared statement parameters
    pub async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<QueryResult, Box<dyn std::error::Error>> {
        let pool = self.pool.as_ref().ok_or("Not connected to database")?;

        // Create the query with parameters
        let mut query = sqlx::query(sql_template);

        // Bind parameters in order
        for param in parameters {
            query = query.bind(param);
        }

        // Execute the query
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
                                    val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
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
                let mut query = sqlx::query(sql_template);
                for param in parameters {
                    query = query.bind(param);
                }

                let result = query.execute(pool).await?;
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

    /// Get list of table names from the database
    pub async fn get_tables(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let pool = self.pool.as_ref().ok_or("Not connected to database")?;

        let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .fetch_all(pool)
            .await?;

        let tables: Vec<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("name").ok())
            .collect();

        Ok(tables)
    }
}

#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub rows_affected: u64,
    pub query_text: Option<String>,
    pub execution_time_ms: Option<i64>,
    pub is_error: bool,
}

impl QueryResult {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.rows_affected == 0
    }

    pub fn row_count(&self) -> usize {
        if !self.rows.is_empty() {
            self.rows.len()
        } else {
            self.rows_affected as usize
        }
    }
}
