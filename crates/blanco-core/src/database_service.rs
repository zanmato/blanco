//! Database Service Trait
//!
//! This module defines a trait for database services that can be used by tools
//! and other components that need database access without depending on specific
//! implementations.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::Connection;

/// A trait that provides database connection management for tools and providers
#[async_trait]
pub trait DatabaseService: Send + Sync {
    /// Get or create a database connection using the provided connection string
    async fn get_or_create_connection(
        &self,
        connection_string: &str,
    ) -> Result<Arc<dyn Connection>>;

    /// Get or create a database connection using the provided connection ID
    async fn get_or_create_connection_by_id(
        &self,
        _connection_id: i64,
    ) -> Result<Arc<dyn Connection>> {
        // Default implementation - resolve connection_id to connection_string
        // This should be overridden by implementations that have access to the app database
        Err(anyhow::anyhow!(
            "get_or_create_connection_by_id not implemented - trait default only"
        ))
    }

    /// Execute a query using the provided connection string
    async fn execute_query(
        &self,
        connection_string: &str,
        sql: &str,
    ) -> Result<crate::QueryResult> {
        let connection = self.get_or_create_connection(connection_string).await?;
        connection.execute_query(sql, None).await
    }

    /// Execute a query using the provided connection ID
    async fn execute_query_by_id(
        &self,
        connection_id: i64,
        sql: &str,
    ) -> Result<crate::QueryResult> {
        let connection = self.get_or_create_connection_by_id(connection_id).await?;
        connection.execute_query(sql, None).await
    }

    /// Get database schema information as JSON with pagination support
    async fn get_database_schema_paginated(
        &self,
        connection_id: i64,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Value> {
        let connection = self.get_or_create_connection_by_id(connection_id).await?;

        // Get basic connection info
        let connection_type = connection.get_connection_type();
        let display_name = connection.get_display_name();

        log::info!(
            "Getting database schema for {} ({}) with limit={:?}, offset={:?}",
            display_name,
            connection_type,
            limit,
            offset
        );

        let limit = limit.unwrap_or(20).min(100); // Default 20, max 100
        let offset = offset.unwrap_or(0);

        let tables = match connection_type {
            "PostgreSQL" => {
                self.get_postgresql_schema_paginated(&connection, table_names, limit, offset)
                    .await?
            }
            "SQLite" => {
                self.get_sqlite_schema_paginated(&connection, table_names, limit, offset)
                    .await?
            }
            _ => {
                // Fallback to the original method for unknown database types
                log::warn!(
                    "Using fallback method for unknown database type: {}",
                    connection_type
                );
                self.get_schema_fallback_paginated(&connection, table_names, limit, offset)
                    .await?
            }
        };

        Ok(serde_json::json!({
            "connection_type": connection_type,
            "database_name": display_name,
            "tables": tables,
            "pagination": {
                "limit": limit,
                "offset": offset,
                "has_more": tables.len() == limit as usize
            }
        }))
    }

    /// Get PostgreSQL schema using optimized JSON aggregation queries
    async fn get_postgresql_schema(&self, connection: &Arc<dyn Connection>) -> Result<Vec<Value>> {
        self.get_postgresql_schema_paginated(connection, None, 20, 0)
            .await
    }

    /// Get PostgreSQL schema using optimized JSON aggregation queries with pagination
    async fn get_postgresql_schema_paginated(
        &self,
        connection: &Arc<dyn Connection>,
        table_names: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Value>> {
        let where_clause = if let Some(names) = table_names {
            format!(
                " AND t.table_name = ANY(ARRAY['{}'])",
                names.replace(',', "','")
            )
        } else {
            String::new()
        };

        let query = format!(
            r#"
            SELECT
                json_build_object(
                    'name', t.table_name,
                    'schema', t.table_schema,
                    'object_type', 'TABLE',
                    'columns', COALESCE(
                        json_agg(
                            json_build_object(
                                'name', c.column_name,
                                'type', c.data_type,
                                'nullable', c.is_nullable = 'YES',
                                'primary_key', c.column_default LIKE '%nextval%' OR
                                              EXISTS (
                                                  SELECT 1 FROM information_schema.table_constraints tc
                                                  JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name
                                                  WHERE tc.constraint_type = 'PRIMARY KEY'
                                                    AND tc.table_name = t.table_name
                                                    AND kcu.column_name = c.column_name
                                              ),
                                'default_value', c.column_default,
                                'character_maximum_length', c.character_maximum_length
                            ) ORDER BY c.ordinal_position
                        ) FILTER (WHERE c.column_name IS NOT NULL),
                        '[]'::json
                    ),
                    'column_count', COUNT(c.column_name)
                ) as table_info
            FROM information_schema.tables t
            LEFT JOIN information_schema.columns c ON t.table_name = c.table_name AND t.table_schema = c.table_schema
            WHERE t.table_type = 'BASE TABLE'
                AND t.table_schema NOT IN ('information_schema', 'pg_catalog', 'pg_toast')
                AND t.table_schema NOT LIKE 'pg_%'
                {}
            GROUP BY t.table_name, t.table_schema
            ORDER BY t.table_name
            LIMIT {} OFFSET {}
        "#,
            where_clause, limit, offset
        );

        let query_result = connection.execute_query(&query, None).await?;

        let mut tables = Vec::new();
        for row in query_result.rows {
            if !row.is_empty() {
                if let Ok(table_info) = serde_json::from_str::<Value>(&row[0]) {
                    tables.push(table_info);
                }
            }
        }

        log::info!(
            "PostgreSQL schema query completed: {} tables found",
            tables.len()
        );
        Ok(tables)
    }

    /// Get SQLite schema using optimized JSON aggregation queries
    async fn get_sqlite_schema(&self, connection: &Arc<dyn Connection>) -> Result<Vec<Value>> {
        self.get_sqlite_schema_paginated(connection, None, 20, 0)
            .await
    }

    /// Get SQLite schema using optimized JSON aggregation queries with pagination
    async fn get_sqlite_schema_paginated(
        &self,
        connection: &Arc<dyn Connection>,
        table_names: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Value>> {
        // Build WHERE clause for table name filtering if provided
        let where_clause = if let Some(names) = table_names {
            let name_list: Vec<&str> = names.split(',').map(|s| s.trim()).collect();
            format!(
                " AND name IN ({})",
                name_list
                    .iter()
                    .map(|s| format!("'{}'", s))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            String::new()
        };

        // Get paginated tables first
        let tables_query = format!(
            r#"
            SELECT name, 'main' as schema, 'TABLE' as object_type
            FROM sqlite_master
            WHERE type = 'table'
                AND name NOT LIKE 'sqlite_%'
                AND name NOT LIKE 'pg_%'
                {}
            ORDER BY name
            LIMIT {} OFFSET {}
        "#,
            where_clause, limit, offset
        );

        let tables_result = connection.execute_query(&tables_query, None).await?;
        let mut tables = Vec::new();

        for table_row in tables_result.rows {
            if !table_row.is_empty() {
                let table_name = &table_row[0];

                // Get column information for this table using JSON aggregation
                let columns_query = format!(
                    r#"
                    SELECT json_group_array(
                        json_object(
                            'name', name,
                            'type', type,
                            'nullable', NOT "notnull",
                            'primary_key', pk > 0,
                            'default_value', dflt_value
                        )
                    ) as columns,
                    COUNT(*) as column_count
                    FROM pragma_table_info('{}')
                "#,
                    table_name
                );

                let columns_result = connection.execute_query(&columns_query, None).await?;

                let columns_json = columns_result
                    .rows
                    .first()
                    .and_then(|row| row.first())
                    .map_or("[]".to_string(), |s| s.clone());

                let column_count = columns_result
                    .rows
                    .first()
                    .and_then(|row| row.get(1))
                    .and_then(|count| count.parse::<i64>().ok())
                    .unwrap_or(0);

                let table_info = serde_json::json!({
                    "name": table_name,
                    "schema": "main",
                    "object_type": "TABLE",
                    "columns": serde_json::from_str::<Value>(&columns_json).unwrap_or(Value::Array(vec![])),
                    "column_count": column_count
                });
                tables.push(table_info);
            }
        }

        log::info!(
            "SQLite schema query completed: {} tables found",
            tables.len()
        );
        Ok(tables)
    }

    /// Fallback method for unknown database types
    async fn get_schema_fallback(&self, connection: &Arc<dyn Connection>) -> Result<Vec<Value>> {
        self.get_schema_fallback_paginated(connection, None, 20, 0)
            .await
    }

    /// Fallback method for unknown database types with pagination
    async fn get_schema_fallback_paginated(
        &self,
        connection: &Arc<dyn Connection>,
        table_names: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Value>> {
        log::warn!("Using fallback schema method - making multiple queries");

        let mut tables = connection.get_tables(table_names).await?;

        // Apply manual sorting and pagination
        tables.sort();

        let start_idx = offset as usize;
        let end_idx = (start_idx + limit as usize).min(tables.len());

        if start_idx >= tables.len() {
            return Ok(Vec::new());
        }

        let paginated_tables = &tables[start_idx..end_idx];
        let mut tables_array = Vec::new();

        for table_name in paginated_tables {
            let columns = connection.get_columns_for_table(table_name, None).await?;

            let mut columns_array = Vec::new();
            for column in columns {
                columns_array.push(serde_json::json!({
                    "name": column.name,
                    "type": column.data_type,
                    "nullable": column.is_nullable,
                    "primary_key": column.is_primary_key,
                    "default_value": column.default_value,
                    "character_maximum_length": column.character_maximum_length
                }));
            }

            tables_array.push(serde_json::json!({
                "name": table_name,
                "schema": "public",
                "object_type": "TABLE",
                "columns": columns_array,
                "column_count": columns_array.len()
            }));
        }

        Ok(tables_array)
    }
}
