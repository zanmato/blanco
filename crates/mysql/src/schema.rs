//! MySQL schema query utilities

use crate::connection::MysqlConnection;
use anyhow::Result;
use blanco_core::Connection;
use blanco_core::connection_trait::{ColumnInfo, TableSchemaInfo};

impl MysqlConnection {
    /// Get MySQL database schema with pagination using a single JSON aggregation query
    pub async fn get_schema_paginated(
        &self,
        table_names: Option<&str>,
        limit: i32,
        offset: i32,
    ) -> Result<Vec<TableSchemaInfo>> {
        let (query, params) = self.build_schema_query_and_params(limit, offset, table_names)?;

        let query_result = self
            .execute_query(&query, None, Some(&params))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get MySQL schema: {}", e))?;

        let mut tables = Vec::new();
        for row in &query_result.rows {
            if !row.is_empty()
                && let Some(row_value) = row[0].as_deref()
                && let Ok(table_info_json) = serde_json::from_str::<serde_json::Value>(row_value)
            {
                // Parse the JSON into our structured types
                if let Some(table_name) = table_info_json.get("name").and_then(|v| v.as_str())
                    && let Some(columns_array) =
                        table_info_json.get("columns").and_then(|v| v.as_array())
                {
                    let column_count = table_info_json
                        .get("column_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;

                    let mut columns = Vec::new();
                    for col_json in columns_array {
                        if let Some(name) = col_json.get("name").and_then(|v| v.as_str()) {
                            let data_type = col_json
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let nullable = col_json
                                .get("nullable")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            let primary_key = col_json
                                .get("primary_key")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let default_value = col_json
                                .get("default_value")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            let character_maximum_length = col_json
                                .get("character_maximum_length")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as i32);

                            columns.push(ColumnInfo {
                                name: name.to_string(),
                                data_type,
                                is_nullable: nullable,
                                is_primary_key: primary_key,
                                default_value,
                                character_maximum_length,
                                foreign_key: None,
                            });
                        }
                    }

                    tables.push(TableSchemaInfo {
                        name: table_name.to_string(),
                        schema: self.get_display_name(),
                        object_type: "TABLE".to_string(),
                        columns,
                        column_count,
                    });
                }
            }
        }

        tracing::info!(
            "MySQL schema query completed: {} tables found",
            tables.len()
        );
        Ok(tables)
    }

    /// Build the schema query string and parameters for MySQL
    fn build_schema_query_and_params(
        &self,
        limit: i32,
        offset: i32,
        table_names: Option<&str>,
    ) -> Result<(String, Vec<String>)> {
        let mut params = Vec::new();
        let mut where_conditions = vec!["t.table_type = 'BASE TABLE'".to_string()];

        // Add table name filter with LIKE wildcard support if specified
        if let Some(names_str) = table_names
            && !names_str.trim().is_empty()
        {
            let patterns: Vec<String> = names_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if !patterns.is_empty() {
                let like_conditions = patterns
                    .iter()
                    .map(|_| "t.table_name LIKE ?")
                    .collect::<Vec<_>>()
                    .join(" OR ");
                where_conditions.push(format!("({})", like_conditions));
                for pattern in &patterns {
                    params.push(pattern.clone());
                }
            }
        }

        let where_clause = where_conditions.join(" AND ");

        let query = format!(
            r#"
            SELECT
                JSON_OBJECT(
                    'name', t.table_name,
                    'columns', JSON_ARRAYAGG(
                        JSON_OBJECT(
                            'name', c.column_name,
                            'type', c.data_type,
                            'nullable', c.is_nullable = 'YES',
                            'primary_key', c.column_key = 'PRI',
                            'default_value', c.column_default,
                            'character_maximum_length', c.character_maximum_length
                        )
                    ),
                    'column_count', COUNT(c.column_name)
                ) as table_info
            FROM information_schema.tables t
            LEFT JOIN information_schema.columns c ON t.table_name = c.table_name
                AND t.table_schema = c.table_schema
            WHERE {}
            GROUP BY t.table_name
            ORDER BY t.table_name
            LIMIT ? OFFSET ?
            "#,
            where_clause
        );

        params.push(limit.to_string());
        params.push(offset.to_string());

        Ok((query, params))
    }
}
