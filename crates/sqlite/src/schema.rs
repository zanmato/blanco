//! SQLite schema query utilities

use crate::connection::SqliteConnection;
use anyhow::Result;
use blanco_core::Connection;
use blanco_core::{ColumnInfo, ForeignKeyInfo, InboundForeignKey, TableSchemaInfo};
use std::collections::{HashMap, HashSet};

impl SqliteConnection {
    /// Get SQLite schema using optimized JSON aggregation queries with pagination
    pub async fn get_schema_paginated(
        &self,
        table_names: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TableSchemaInfo>> {
        let (tables_query, params) = self.build_tables_query_and_params(table_names, limit, offset);

        let tables_result = self
            .execute_query(&tables_query, None, Some(&params))
            .await?;
        let mut tables = Vec::new();

        for table_row in &tables_result.rows {
            if !table_row.is_empty() {
                let table_name = match &table_row[0] {
                    Some(name) => name,
                    None => continue,
                };

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

                let columns_result = self.execute_query(&columns_query, None, None).await?;

                let fk_query = format!("PRAGMA foreign_key_list({})", table_name);
                let fk_result = self.execute_query(&fk_query, None, None).await?;
                let mut foreign_keys: HashMap<String, ForeignKeyInfo> = HashMap::new();
                for row in fk_result.rows {
                    if row.len() >= 5
                        && let (
                            Some(Some(from_column)),
                            Some(Some(to_table)),
                            Some(Some(to_column)),
                        ) = (row.get(3), row.get(2), row.get(4))
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

                let columns_json = columns_result
                    .rows
                    .first()
                    .and_then(|row| row.first())
                    .and_then(|s| s.as_ref())
                    .cloned()
                    .unwrap_or_else(|| "[]".to_string());

                let column_count = columns_result
                    .rows
                    .first()
                    .and_then(|row| row.get(1))
                    .and_then(|count| count.as_ref())
                    .and_then(|count| count.parse::<i64>().ok())
                    .unwrap_or(0);

                // Parse the columns JSON into ColumnInfo structs
                let columns: Vec<ColumnInfo> = if let Ok(json_value) =
                    serde_json::from_str::<serde_json::Value>(&columns_json)
                {
                    if let Some(array) = json_value.as_array() {
                        array
                            .iter()
                            .filter_map(|col| {
                                if let Some(name) = col.get("name").and_then(|v| v.as_str()) {
                                    let data_type = col
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown")
                                        .to_string();
                                    let nullable = col
                                        .get("nullable")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(true);
                                    let primary_key = col
                                        .get("primary_key")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    let default_value = col
                                        .get("default_value")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| {
                                            if s == "NULL" || s.is_empty() {
                                                None
                                            } else {
                                                Some(s.to_string())
                                            }
                                        });

                                    let foreign_key = foreign_keys.get(name).cloned();
                                    Some(ColumnInfo {
                                        name: name.to_string(),
                                        data_type,
                                        is_nullable: nullable,
                                        is_primary_key: primary_key,
                                        default_value,
                                        character_maximum_length: None, // SQLite doesn't specify this in pragma_table_info
                                        foreign_key,
                                    })
                                } else {
                                    None
                                }
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };

                tables.push(TableSchemaInfo {
                    name: table_name.clone(),
                    schema: "main".to_string(),
                    object_type: "TABLE".to_string(),
                    columns,
                    column_count: column_count as usize,
                    referenced_by: Vec::new(),
                });
            }
        }

        if !tables.is_empty() {
            let target_names: HashSet<String> = tables.iter().map(|t| t.name.clone()).collect();
            let inbound = self.fetch_inbound_foreign_keys(&target_names).await?;
            for table in &mut tables {
                if let Some(list) = inbound.get(&table.name) {
                    table.referenced_by = list.clone();
                }
            }
        }

        Ok(tables)
    }

    async fn fetch_inbound_foreign_keys(
        &self,
        targets: &HashSet<String>,
    ) -> Result<HashMap<String, Vec<InboundForeignKey>>> {
        let all_tables_query =
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'";
        let all_tables = self.execute_query(all_tables_query, None, None).await?;

        let mut map: HashMap<String, Vec<InboundForeignKey>> = HashMap::new();
        for row in all_tables.rows {
            let from_table = match row.first().and_then(|c| c.clone()) {
                Some(name) => name,
                None => continue,
            };
            let fk_query = format!("PRAGMA foreign_key_list({})", from_table);
            let fk_result = self.execute_query(&fk_query, None, None).await?;
            for fk_row in fk_result.rows {
                if fk_row.len() >= 5
                    && let (Some(Some(to_table)), Some(Some(from_column)), Some(Some(to_column))) =
                        (fk_row.get(2), fk_row.get(3), fk_row.get(4))
                    && targets.contains(to_table)
                {
                    map.entry(to_table.clone())
                        .or_default()
                        .push(InboundForeignKey {
                            from_table: from_table.clone(),
                            from_column: from_column.clone(),
                            to_column: to_column.clone(),
                            constraint_name: None,
                        });
                }
            }
        }
        Ok(map)
    }

    /// Build the tables query string and parameters for SQLite
    fn build_tables_query_and_params(
        &self,
        table_names: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> (String, Vec<String>) {
        let mut params = Vec::new();
        let mut where_conditions = vec![
            "type = 'table'".to_string(),
            "name NOT LIKE 'sqlite_%'".to_string(),
            "name NOT LIKE 'pg_%'".to_string(),
        ];

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
                    .map(|_| "name LIKE ?")
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
            SELECT name, 'main' as schema, 'TABLE' as object_type
            FROM sqlite_master
            WHERE {}
            ORDER BY name
            LIMIT ? OFFSET ?
        "#,
            where_clause
        );

        params.push(limit.to_string());
        params.push(offset.to_string());

        (query, params)
    }
}
