use log::{debug, error, info};
use sqlx::postgres::{PgConnectOptions, PgPool};
use sqlx::{Column, ConnectOptions, Row};
use std::str::FromStr;

/// PostgreSQL database manager
pub struct PostgresManager {
    pub pool: Option<PgPool>,
    pub connection_string: Option<String>,
}

impl PostgresManager {
    pub fn new() -> Self {
        Self {
            pool: None,
            connection_string: None,
        }
    }

    /// Connect to PostgreSQL database
    pub async fn connect_async(&mut self, connection_string: &str) -> Result<(), sqlx::Error> {
        info!("Connecting to PostgreSQL: {}", Self::sanitize_connection_string(connection_string));

        let options = PgConnectOptions::from_str(connection_string)?
            .disable_statement_logging();

        let pool = PgPool::connect_with(options).await?;
        self.pool = Some(pool);
        self.connection_string = Some(connection_string.to_string());

        info!("Successfully connected to PostgreSQL");
        Ok(())
    }

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

    pub fn is_connected(&self) -> bool {
        self.pool.is_some()
    }

    pub async fn disconnect(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }
        self.connection_string = None;
    }

    /// Get list of databases
    pub async fn get_databases(&self) -> Result<Vec<String>, anyhow::Error> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        let rows = sqlx::query(
            "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname"
        )
        .fetch_all(pool)
        .await?;

        let databases: Vec<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("datname").ok())
            .collect();

        Ok(databases)
    }

    /// Get list of schemas for current database
    pub async fn get_schemas(&self) -> Result<Vec<String>, anyhow::Error> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        let rows = sqlx::query(
            "SELECT schema_name FROM information_schema.schemata
             WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
             ORDER BY schema_name"
        )
        .fetch_all(pool)
        .await?;

        let schemas: Vec<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("schema_name").ok())
            .collect();

        Ok(schemas)
    }

    /// Get list of tables for a specific schema
    pub async fn get_tables(&self, schema: &str) -> Result<Vec<String>, anyhow::Error> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        let query = format!(
            "SELECT tablename FROM pg_tables WHERE schemaname = '{}' ORDER BY tablename",
            schema
        );

        let rows = sqlx::query(&query)
            .fetch_all(pool)
            .await?;

        let tables: Vec<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("tablename").ok())
            .collect();

        Ok(tables)
    }

    /// Execute a query and return results
    pub async fn execute_query_async(
        &self,
        query: &str,
    ) -> Result<crate::database::QueryResult, anyhow::Error> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        // Try to execute as a query that returns rows
        match sqlx::query(query).fetch_all(pool).await {
            Ok(rows) => {
                if rows.is_empty() {
                    return Ok(crate::database::QueryResult {
                        columns: vec![],
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

                // Extract row data
                let data_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|row| {
                        columns
                            .iter()
                            .enumerate()
                            .map(|(i, _)| {
                                // Try different types
                                if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    val.unwrap_or_else(|| "NULL".to_string())
                                } else if let Ok(val) = row.try_get::<Option<i32>, _>(i) {
                                    val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string())
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

                Ok(crate::database::QueryResult {
                    columns,
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
                Ok(crate::database::QueryResult {
                    columns: vec![],
                    rows: vec![],
                    rows_affected: result.rows_affected(),
                    query_text: None,
                    execution_time_ms: None,
                    is_error: false,
                })
            }
        }
    }
}

/// Hierarchical structure for PostgreSQL database tree
#[derive(Debug, Clone)]
pub struct PostgresHierarchy {
    pub connection_name: String,
    pub database: String,
    pub schemas: Vec<SchemaNode>,
}

#[derive(Debug, Clone)]
pub struct SchemaNode {
    pub name: String,
    pub tables: Vec<String>,
    pub expanded: bool,
}

impl SchemaNode {
    pub fn new(name: String) -> Self {
        Self {
            name,
            tables: Vec::new(),
            expanded: false,
        }
    }
}
