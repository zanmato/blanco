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
    /// Get or create a database connection using the provided connection ID and optional database name
    /// The database parameter allows switching databases for connections that support it (e.g., PostgreSQL)
    async fn get_or_create_connection_by_id(
        &self,
        _connection_id: i64,
        _database: Option<&str>,
    ) -> Result<Arc<dyn Connection>> {
        // Default implementation - should be overridden
        Err(anyhow::anyhow!(
            "get_or_create_connection_by_id not implemented - trait default only"
        ))
    }

    /// Execute a query using the provided connection ID
    async fn execute_query_by_id(
        &self,
        connection_id: i64,
        sql: &str,
    ) -> Result<crate::QueryResult> {
        let connection = self
            .get_or_create_connection_by_id(connection_id, None)
            .await?;
        connection.execute_query(sql, None, None).await
    }

    /// Execute a query using the provided connection ID and optional database
    async fn execute_query_by_id_with_database(
        &self,
        connection_id: i64,
        database: Option<&str>,
        sql: &str,
    ) -> Result<crate::QueryResult> {
        let connection = self
            .get_or_create_connection_by_id(connection_id, database)
            .await?;
        connection.execute_query(sql, database, None).await
    }

    /// Execute a parameterized query using the provided connection ID
    async fn execute_query_by_id_with_params(
        &self,
        connection_id: i64,
        database: Option<&str>,
        sql: &str,
        parameters: &[String],
    ) -> Result<crate::QueryResult> {
        let connection = self
            .get_or_create_connection_by_id(connection_id, database)
            .await?;
        connection
            .execute_query(sql, database, Some(parameters))
            .await
    }

    /// Get database schema information as JSON with pagination support
    async fn get_database_schema_paginated(
        &self,
        connection_id: i64,
        database: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Value> {
        let connection = self
            .get_or_create_connection_by_id(connection_id, database)
            .await?;

        tracing::info!(
            "Getting database schema for {} ({}) with limit={:?}, offset={:?}",
            connection.get_display_name(),
            connection.get_connection_type(),
            limit,
            offset
        );

        // Use the Connection trait's unified method
        let schema_result = connection
            .get_database_schema_paginated(database, table_names, limit, offset)
            .await?;

        // Convert the structured result to JSON for compatibility with existing code
        let json_result = serde_json::json!({
            "connection_type": schema_result.connection_type,
            "database_name": schema_result.display_name,
            "tables": schema_result.tables,
            "pagination": {
                "limit": schema_result.pagination.limit,
                "offset": schema_result.pagination.offset,
                "has_more": schema_result.pagination.has_more
            }
        });

        Ok(json_result)
    }
}
