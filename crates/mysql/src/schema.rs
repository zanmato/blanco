//! MySQL schema query utilities

use crate::connection::MysqlConnection;
use anyhow::Result;
use blanco_core::connection_trait::{ColumnInfo, TableSchemaInfo};
use blanco_core::Connection;

impl MysqlConnection {
    /// Get MySQL database schema with pagination
    pub async fn get_schema_paginated(
        &self,
        table_names: Option<&str>,
        limit: i32,
        offset: i32,
    ) -> Result<Vec<TableSchemaInfo>> {
        let mut tables_array = Vec::new();

        // Get the list of tables using MySQL INFORMATION_SCHEMA
        let table_query = if let Some(table_names_str) = table_names {
            if table_names_str.trim().is_empty() {
                // No specific tables requested, return empty array
                return Ok(tables_array);
            }

            // Parse comma-separated table names
            let table_names: Vec<&str> =
                table_names_str.split(',').map(|name| name.trim()).collect();
            let placeholders = table_names
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "SELECT table_name FROM information_schema.tables
                 WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'
                 AND table_name IN ({})
                 ORDER BY table_name
                 LIMIT {} OFFSET {}",
                placeholders, limit, offset
            )
        } else {
            format!(
                "SELECT table_name FROM information_schema.tables
                 WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'
                 ORDER BY table_name
                 LIMIT {} OFFSET {}",
                limit, offset
            )
        };

        let table_result = self
            .execute_query(&table_query, None, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get MySQL tables: {}", e))?;

        for table_row in &table_result.rows {
            if table_row.is_empty() {
                continue;
            }

            let table_name = table_row[0].clone();

            // Get column information for this table
            let column_query = "SELECT
                    column_name,
                    data_type,
                    is_nullable,
                    column_default,
                    character_maximum_length,
                    column_key
                FROM information_schema.columns
                WHERE table_schema = DATABASE() AND table_name = ?
                ORDER BY ordinal_position";

            let column_result = self
                .execute_query(column_query, None, Some(std::slice::from_ref(&table_name)))
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to get columns for table {}: {}", table_name, e)
                })?;

            let mut columns_array = Vec::new();

            for col_row in column_result.rows.iter() {
                if col_row.len() < 6 {
                    continue;
                }

                let column_name = col_row[0].clone();
                let data_type = col_row[1].clone();
                let is_nullable_str = col_row[2].clone();
                let default_value = if col_row[3].is_empty() {
                    None
                } else {
                    Some(col_row[3].clone())
                };
                let max_length_str = col_row[4].clone();
                let column_key = col_row[5].clone();

                let max_length = if max_length_str.is_empty() {
                    None
                } else {
                    max_length_str.parse::<i32>().ok()
                };

                // Check if this column is a primary key (column_key = 'PRI')
                let is_primary_key = column_key == "PRI";

                let is_nullable = is_nullable_str == "YES";

                columns_array.push(ColumnInfo {
                    name: column_name,
                    data_type,
                    is_nullable,
                    is_primary_key,
                    default_value,
                    character_maximum_length: max_length,
                    foreign_key: None,
                });
            }

            tables_array.push(TableSchemaInfo {
                name: table_name,
                schema: self.get_display_name(),
                object_type: "TABLE".to_string(),
                columns: columns_array,
                column_count: column_result.rows.len(),
            });
        }

        Ok(tables_array)
    }
}
