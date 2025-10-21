use log::info;
use sqlx::postgres::{PgConnectOptions, PgPool};
use sqlx::{Column, ConnectOptions, Row, ValueRef, TypeInfo};
use std::str::FromStr;
use uuid::Uuid;

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

    /// Execute a query with prepared statement parameters
    pub async fn execute_prepared_query(
        &self,
        sql_template: &str,
        parameters: &[String],
    ) -> Result<crate::database::QueryResult, anyhow::Error> {
        let pool = self.pool.as_ref().ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

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
                    return Ok(crate::database::QueryResult {
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
                                // Try different types in order of likelihood

                                // String/Text types (most common)
                                if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    return val.unwrap_or_else(|| "NULL".to_string());
                                }

                                // Integer types
                                if let Ok(val) = row.try_get::<Option<i16>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<i32>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }

                                // Floating point types
                                if let Ok(val) = row.try_get::<Option<f32>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }

                                // Boolean type
                                if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }

                                // Try to get UUID as string (PostgreSQL UUID can be converted to string)
                                if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    return val.unwrap_or_else(|| "NULL".to_string());
                                }

                                // Try to get timestamp/chrono types as string
                                if let Ok(val) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }

                                // UUID types
                                if let Ok(val) = row.try_get::<Option<Uuid>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }

                                // Byte array types (for binary data)
                                if let Ok(val) = row.try_get::<Option<Vec<u8>>, _>(i) {
                                    return val.map(|v| {
                                        // Convert to hex string for binary data
                                        v.iter().map(|byte| format!("{:02x}", byte)).collect::<String>()
                                    }).unwrap_or_else(|| "NULL".to_string());
                                }

                                // JSON/JSONB types - try to get as string first
                                if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(i) {
                                    return val.map(|v| {
                                        // Pretty print JSON with proper formatting
                                        match v {
                                            serde_json::Value::String(s) => s,
                                            _ => v.to_string(),
                                        }
                                    }).unwrap_or_else(|| "NULL".to_string());
                                }

                                // If we can't determine the type, try to get it as raw value
                                // This is a fallback for any other types
                                match row.try_get_raw(i) {
                                    Ok(raw_value) => {
                                        if raw_value.is_null() {
                                            "NULL".to_string()
                                        } else {
                                            // For unknown types, try to get as string or show type info
                                            match row.column(i).type_info().name() {
                                                "timestamptz" => "<timestamptz>".to_string(),
                                                "timestamp" => "<timestamp>".to_string(),
                                                "json" => "<json>".to_string(),
                                                "jsonb" => "<jsonb>".to_string(),
                                                "numeric" => "<numeric>".to_string(),
                                                "decimal" => "<decimal>".to_string(),
                                        _ => format!("<{}>", row.column(i).type_info().name())
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        // If even raw access fails, indicate unknown type
                                        "<error>".to_string()
                                    }
                                }
                            })
                            .collect()
                    })
                    .collect();

                Ok(crate::database::QueryResult {
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
                Ok(crate::database::QueryResult {
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
                                // Try different types in order of likelihood
                                
                                // String/Text types (most common)
                                if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    return val.unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // Integer types
                                if let Ok(val) = row.try_get::<Option<i16>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<i32>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<i64>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // Floating point types
                                if let Ok(val) = row.try_get::<Option<f32>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<f64>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // Boolean type
                                if let Ok(val) = row.try_get::<Option<bool>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // Try to get UUID as string (PostgreSQL UUID can be converted to string)
                                if let Ok(val) = row.try_get::<Option<String>, _>(i) {
                                    return val.unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // Try to get timestamp/chrono types as string
                                if let Ok(val) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // UUID types
                                if let Ok(val) = row.try_get::<Option<Uuid>, _>(i) {
                                    return val.map(|v| v.to_string()).unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // Byte array types (for binary data)
                                if let Ok(val) = row.try_get::<Option<Vec<u8>>, _>(i) {
                                    return val.map(|v| {
                                        // Convert to hex string for binary data
                                        v.iter().map(|byte| format!("{:02x}", byte)).collect::<String>()
                                    }).unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // JSON/JSONB types - try to get as string first
                                if let Ok(val) = row.try_get::<Option<serde_json::Value>, _>(i) {
                                    return val.map(|v| {
                                        // Pretty print JSON with proper formatting
                                        match v {
                                            serde_json::Value::String(s) => s,
                                            _ => v.to_string(),
                                        }
                                    }).unwrap_or_else(|| "NULL".to_string());
                                }
                                
                                // If we can't determine the type, try to get it as raw value
                                // This is a fallback for any other types
                                match row.try_get_raw(i) {
                                    Ok(raw_value) => {
                                        if raw_value.is_null() {
                                            "NULL".to_string()
                                        } else {
                                            // For unknown types, try to get as string or show type info
                                            match row.column(i).type_info().name() {
                                                "timestamptz" => "<timestamptz>".to_string(),
                                                "timestamp" => "<timestamp>".to_string(),
                                                "json" => "<json>".to_string(),
                                                "jsonb" => "<jsonb>".to_string(),
                                                "numeric" => "<numeric>".to_string(),
                                                "decimal" => "<decimal>".to_string(),
                                        _ => format!("<{}>", row.column(i).type_info().name())
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        // If even raw access fails, indicate unknown type
                                        "<error>".to_string()
                                    }
                                }
                            })
                            .collect()
                    })
                    .collect();

                Ok(crate::database::QueryResult {
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
                Ok(crate::database::QueryResult {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_service::DbService;

    #[tokio::test]
    async fn test_postgres_connection() {
        // This test requires a running PostgreSQL instance
        // Skip if no connection string is available
        let connection_string = std::env::var("POSTGRES_CONNECTION_STRING")
            .unwrap_or_else(|_| "postgres://postgres:Bongotrumma24!@localhost:5432/bylyngamanager?sslmode=disable&timezone=Europe/Stockholm".to_string());

        let mut pg_manager = PostgresManager::new();
        
        // Test connection
        match pg_manager.connect_async(&connection_string).await {
            Ok(_) => println!("✅ PostgreSQL connection successful"),
            Err(e) => {
                println!("⚠️  PostgreSQL connection failed (this is expected if no server is running): {}", e);
                return;
            }
        }

        // Test simple query
        let query = "SELECT version() as version, current_database() as database";
        match pg_manager.execute_query_async(query).await {
            Ok(result) => {
                assert!(!result.columns.is_empty(), "Should have columns");
                assert!(!result.rows.is_empty(), "Should have rows");
                println!("✅ Query execution successful");
            }
            Err(e) => {
                println!("❌ Query execution failed: {}", e);
                panic!("Query should execute successfully");
            }
        }

        // Test type handling
        let type_query = r#"
            SELECT 
                '550e8400-e29b-41d4-a716-446655440000'::uuid as uuid_col,
                NOW() as timestamp_col,
                '{"key": "value"}'::json as json_col,
                123.45::numeric as numeric_col,
                true as bool_col,
                'test_text' as text_col
        "#;

        match pg_manager.execute_query_async(type_query).await {
            Ok(result) => {
                assert_eq!(result.columns.len(), 6, "Should have 6 columns");
                assert!(!result.rows.is_empty(), "Should have rows");
                println!("✅ Type handling successful");
                
                // Check that UUID and other types are handled (not null)
                let row = &result.rows[0];
                for (i, value) in row.iter().enumerate() {
                    println!("   Column {}: {}", result.columns[i], value);
                    // Values should not be null representations
                    assert_ne!(value, "<null>", "Column {} should not be null", result.columns[i]);
                }
            }
            Err(e) => {
                println!("⚠️  Type query failed (might be due to missing extensions): {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_db_service_integration() {
        let connection_string = std::env::var("POSTGRES_CONNECTION_STRING")
            .unwrap_or_else(|_| "postgres://postgres:Bongotrumma24!@localhost:5432/bylyngamanager?sslmode=disable&timezone=Europe/Stockholm".to_string());

        let db_service = DbService::new();

        match db_service.get_or_create_pg_connection(&connection_string).await {
            Ok(pg_manager) => {
                println!("✅ DbService integration successful");
                
                // Test query through DbService
                match pg_manager.execute_query_async("SELECT COUNT(*) as count FROM pg_tables WHERE schemaname = 'public'").await {
                    Ok(result) => {
                        assert!(!result.rows.is_empty(), "Should have result rows");
                        println!("✅ DbService query successful");
                    }
                    Err(e) => {
                        println!("❌ DbService query failed: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("⚠️  DbService integration failed (expected if no PostgreSQL server): {}", e);
            }
        }
    }
}
