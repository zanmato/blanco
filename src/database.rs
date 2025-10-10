use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::{Column, ConnectOptions, Row};
use std::str::FromStr;

pub struct DatabaseManager {
    pool: Option<SqlitePool>,
}

impl DatabaseManager {
    pub fn new() -> Self {
        Self { pool: None }
    }

    pub async fn connect(&mut self, database_path: &str) -> Result<(), sqlx::Error> {
        let options = SqliteConnectOptions::from_str(database_path)?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;
        self.pool = Some(pool);
        Ok(())
    }

    pub async fn execute_query(&self, query: &str) -> Result<QueryResult, Box<dyn std::error::Error>> {
        let pool = self.pool.as_ref().ok_or("Not connected to database")?;

        // Try to execute as a query that returns rows
        match sqlx::query(query).fetch_all(pool).await {
            Ok(rows) => {
                if rows.is_empty() {
                    return Ok(QueryResult {
                        columns: vec![],
                        rows: vec![],
                        rows_affected: 0,
                    });
                }

                // Extract column names from the first row
                let first_row = &rows[0];
                let columns: Vec<String> = first_row
                    .columns()
                    .iter()
                    .map(|col| col.name().to_string())
                    .collect();

                // Extract row data
                let data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        columns
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                // Try to get value as different types
                                if let Ok(val) = row.try_get::<String, _>(i) {
                                    val
                                } else if let Ok(val) = row.try_get::<i64, _>(i) {
                                    val.to_string()
                                } else if let Ok(val) = row.try_get::<f64, _>(i) {
                                    val.to_string()
                                } else if let Ok(val) = row.try_get::<bool, _>(i) {
                                    val.to_string()
                                } else {
                                    "NULL".to_string()
                                }
                            })
                            .collect()
                    })
                    .collect();

                Ok(QueryResult {
                    columns,
                    rows: data_rows,
                    rows_affected: 0,
                })
            }
            Err(e) => {
                // If it's not a SELECT query, try executing it as a statement
                let result = sqlx::query(query).execute(pool).await?;
                Ok(QueryResult {
                    columns: vec![],
                    rows: vec![],
                    rows_affected: result.rows_affected(),
                })
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.pool.is_some()
    }

    pub async fn disconnect(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub rows_affected: u64,
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
