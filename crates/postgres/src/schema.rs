//! PostgreSQL schema query utilities

use crate::connection::{PostgresConnection, QueryParam};
use anyhow::Result;
use blanco_core::{ColumnInfo, ForeignKeyInfo, InboundForeignKey, TableSchemaInfo};

fn parse_referenced_by(table_info: &serde_json::Value) -> Vec<InboundForeignKey> {
    table_info
        .get("referenced_by")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let from_table = item.get("from_table").and_then(|v| v.as_str())?;
                    let from_column = item.get("from_column").and_then(|v| v.as_str())?;
                    let to_column = item.get("to_column").and_then(|v| v.as_str())?;
                    let constraint_name = item
                        .get("constraint_name")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    Some(InboundForeignKey {
                        from_table: from_table.to_string(),
                        from_column: from_column.to_string(),
                        to_column: to_column.to_string(),
                        constraint_name,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl PostgresConnection {
    /// Get PostgreSQL schema using optimized JSON aggregation queries with pagination
    pub async fn get_schema_paginated(
        &self,
        database_name: Option<&str>,
        table_names: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TableSchemaInfo>> {
        let (query, params) = if let Some(names) = table_names {
            let table_names_vec: Vec<String> = names
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if table_names_vec.is_empty() {
                self.build_schema_query_and_params(limit, offset, None)
            } else {
                self.build_schema_query_and_params(limit, offset, Some(table_names_vec))
            }
        } else {
            self.build_schema_query_and_params(limit, offset, None)
        };

        let query_result = self
            .execute_query_with_params_typed(&query, database_name, &params)
            .await?;

        let mut tables = Vec::new();
        for row in &query_result.rows {
            if !row.is_empty()
                && let Some(row_value) = row[0].as_deref()
                && let Ok(table_info_json) = serde_json::from_str::<serde_json::Value>(row_value)
            {
                // Parse the JSON into our structured types
                if let Some(table_name) = table_info_json.get("name").and_then(|v| v.as_str())
                    && let Some(schema_name) =
                        table_info_json.get("schema").and_then(|v| v.as_str())
                    && let Some(object_type) =
                        table_info_json.get("object_type").and_then(|v| v.as_str())
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
                            let fk_table = col_json
                                .get("fk_table")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty());
                            let fk_column = col_json
                                .get("fk_column")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty());
                            let fk_constraint = col_json
                                .get("fk_constraint")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string());
                            let foreign_key =
                                ForeignKeyInfo::from_parts(fk_table, fk_column, fk_constraint);

                            columns.push(ColumnInfo {
                                name: name.to_string(),
                                data_type,
                                is_nullable: nullable,
                                is_primary_key: primary_key,
                                default_value,
                                character_maximum_length,
                                foreign_key,
                            });
                        }
                    }

                    let referenced_by = parse_referenced_by(&table_info_json);
                    tables.push(TableSchemaInfo {
                        name: table_name.to_string(),
                        schema: schema_name.to_string(),
                        object_type: object_type.to_string(),
                        columns,
                        column_count,
                        referenced_by,
                    });
                }
            }
        }

        Ok(tables)
    }

    /// Build the schema query and parameters with optional table name filtering
    fn build_schema_query_and_params(
        &self,
        limit: i64,
        offset: i64,
        table_names: Option<Vec<String>>,
    ) -> (String, Vec<QueryParam>) {
        let (where_clause, mut params) = if let Some(names) = table_names {
            (
                " AND t.table_name LIKE ANY($1)".to_string(),
                vec![QueryParam::StringArray(names)],
            )
        } else {
            (String::new(), vec![])
        };

        params.push(QueryParam::I64(limit));
        params.push(QueryParam::I64(offset));

        let param_start = if params.len() == 3 { 2 } else { 1 };
        let limit_param = param_start;
        let offset_param = param_start + 1;

        let query = format!(
            r#"
            WITH fk_columns AS (
                SELECT
                    n.nspname        AS table_schema,
                    cls.relname      AS table_name,
                    a.attname        AS column_name,
                    fn.nspname || '.' || fcls.relname AS foreign_table_name,
                    af.attname       AS foreign_column_name,
                    con.conname      AS constraint_name
                FROM pg_constraint con
                JOIN pg_class cls       ON cls.oid = con.conrelid
                JOIN pg_namespace n     ON n.oid = cls.relnamespace
                JOIN pg_class fcls      ON fcls.oid = con.confrelid
                JOIN pg_namespace fn    ON fn.oid = fcls.relnamespace
                JOIN LATERAL unnest(con.conkey, con.confkey)
                    WITH ORDINALITY AS k(conkey, confkey, ord) ON TRUE
                JOIN pg_attribute a     ON a.attrelid = con.conrelid AND a.attnum = k.conkey
                JOIN pg_attribute af    ON af.attrelid = con.confrelid AND af.attnum = k.confkey
                WHERE con.contype = 'f'
            )
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
                                'character_maximum_length', c.character_maximum_length,
                                'fk_table', fk.foreign_table_name,
                                'fk_column', fk.foreign_column_name,
                                'fk_constraint', fk.constraint_name
                            ) ORDER BY c.ordinal_position
                        ) FILTER (WHERE c.column_name IS NOT NULL),
                        '[]'::json
                    ),
                    'column_count', COUNT(c.column_name),
                    'referenced_by', COALESCE((
                        SELECT json_agg(json_build_object(
                            'from_table', src_n.nspname || '.' || src.relname,
                            'from_column', a.attname,
                            'to_column', af.attname,
                            'constraint_name', con.conname
                        ) ORDER BY src_n.nspname, src.relname, k.ord)
                        FROM pg_constraint con
                        JOIN pg_class src         ON src.oid = con.conrelid
                        JOIN pg_namespace src_n   ON src_n.oid = src.relnamespace
                        JOIN pg_class target      ON target.oid = con.confrelid
                        JOIN pg_namespace target_n ON target_n.oid = target.relnamespace
                        JOIN LATERAL unnest(con.conkey, con.confkey)
                            WITH ORDINALITY AS k(conkey, confkey, ord) ON TRUE
                        JOIN pg_attribute a       ON a.attrelid = con.conrelid AND a.attnum = k.conkey
                        JOIN pg_attribute af      ON af.attrelid = con.confrelid AND af.attnum = k.confkey
                        WHERE con.contype = 'f'
                          AND target_n.nspname = t.table_schema
                          AND target.relname = t.table_name
                    ), '[]'::json)
                ) as table_info
            FROM information_schema.tables t
            LEFT JOIN information_schema.columns c ON t.table_name = c.table_name AND t.table_schema = c.table_schema
            LEFT JOIN fk_columns fk
                ON fk.table_schema = c.table_schema
                AND fk.table_name = c.table_name
                AND fk.column_name = c.column_name
            WHERE t.table_type = 'BASE TABLE'
                AND t.table_schema NOT IN ('information_schema', 'pg_catalog', 'pg_toast')
                AND t.table_schema NOT LIKE 'pg_%'
                {}
            GROUP BY t.table_name, t.table_schema
            ORDER BY t.table_name
            LIMIT ${} OFFSET ${}
        "#,
            where_clause, limit_param, offset_param
        );

        (query, params)
    }

    /// Execute a query with typed parameters (helper for schema queries)
    async fn execute_query_with_params_typed(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: &[QueryParam],
    ) -> Result<blanco_core::QueryResult> {
        let database_name = database_name.ok_or(anyhow::anyhow!("missing database"))?;

        let pool = self.get_or_create_pool(database_name).await.map_err(|e| {
            anyhow::anyhow!(
                "Failed to get connection pool for database '{}': {}",
                database_name,
                e
            )
        })?;

        self.execute_query_with_params(&pool, query, parameters)
            .await
            .map_err(|e| anyhow::anyhow!("PostgreSQL query execution failed: {}", e))
    }
}
