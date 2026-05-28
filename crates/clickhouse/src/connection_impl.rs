use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, QueryResult,
    connection_trait::{
        ColumnType, DatabaseSchemaResult, EntityType, IndexInfo, PaginationInfo, QueryableEntity,
        TableSchemaInfo,
    },
};
use futures::Stream;
use std::time::Instant;

use super::ClickhouseConnection;

#[async_trait]
impl Connection for ClickhouseConnection {
    fn get_connection_type(&self) -> &'static str {
        "ClickHouse"
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    async fn connect(&mut self, _connection_string: &str) -> Result<()> {
        let start = Instant::now();
        let result = self.raw_query("SELECT 1", None).await;
        match result {
            Ok(_) => {
                self.connected = true;
                tracing::info!(
                    "Connected to ClickHouse at {} in {}ms",
                    self.base_url,
                    start.elapsed().as_millis()
                );
                Ok(())
            }
            Err(e) => {
                self.connected = false;
                Err(e)
            }
        }
    }

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        _parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let start = Instant::now();
        let db = database_name.or(Some(self.database.as_str()));

        let trimmed = query.trim().to_uppercase();
        let is_write = trimmed.starts_with("INSERT")
            || trimmed.starts_with("UPDATE")
            || trimmed.starts_with("DELETE")
            || trimmed.starts_with("ALTER")
            || trimmed.starts_with("CREATE")
            || trimmed.starts_with("DROP")
            || trimmed.starts_with("TRUNCATE")
            || trimmed.starts_with("RENAME")
            || trimmed.starts_with("ATTACH")
            || trimmed.starts_with("DETACH")
            || trimmed.starts_with("OPTIMIZE")
            || trimmed.starts_with("KILL");

        if is_write {
            let body = self.raw_query(query, db).await?;
            let rows_affected = parse_rows_affected(&body);
            return Ok(QueryResult {
                columns: vec![],
                column_types: vec![],
                rows: vec![],
                rows_affected,
                query_text: Some(query.to_string()),
                execution_time_ms: Some(start.elapsed().as_millis() as i64),
                is_error: false,
                table_name: None,
                connection_id: None,
                table_columns: None,
            });
        }

        let tsv = self.raw_query_tsv(query, db).await?;
        let column_types: Vec<ColumnType> = tsv
            .column_types
            .iter()
            .map(|t| Self::map_clickhouse_type(t))
            .collect();

        Ok(QueryResult {
            columns: tsv.columns,
            column_types,
            rows: tsv.rows,
            rows_affected: 0,
            query_text: Some(query.to_string()),
            execution_time_ms: Some(start.elapsed().as_millis() as i64),
            is_error: false,
            table_name: None,
            connection_id: None,
            table_columns: None,
        })
    }

    async fn execute_write(
        &self,
        query: &str,
        database_name: Option<&str>,
        _parameters: &[Option<String>],
    ) -> Result<u64> {
        let db = database_name.or(Some(self.database.as_str()));
        let body = self.raw_query(query, db).await?;
        Ok(parse_rows_affected(&body))
    }

    async fn execute_query_stream_rows(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<(
        Vec<String>,
        Vec<ColumnType>,
        Box<dyn Stream<Item = Result<Vec<Option<String>>>> + Send + Unpin>,
    )> {
        let tsv = self.raw_query_tsv(query, database_name).await?;
        let column_types: Vec<ColumnType> = tsv
            .column_types
            .iter()
            .map(|t| Self::map_clickhouse_type(t))
            .collect();
        let stream = futures::stream::iter(tsv.rows.into_iter().map(Ok));
        Ok((tsv.columns, column_types, Box::new(stream)))
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        let tsv = self
            .raw_query_tsv("SELECT name FROM system.databases ORDER BY name", None)
            .await?;
        Ok(tsv
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        // ClickHouse has no Postgres-style schemas; the top-level grouping
        // is the database. The connections panel treats `supports_schemas`
        // as false and renders `connection → schema → tables`, so we expose
        // each database as a "schema" entry (matching MySQL's approach).
        self.get_databases().await
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let db = schema.unwrap_or(&self.database);
        let sql = format!(
            "SELECT name FROM system.tables WHERE database = '{}' AND engine NOT IN ('View','MaterializedView') ORDER BY name",
            db.replace('\'', "''")
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        Ok(tsv
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let db = schema.unwrap_or(&self.database);
        let sql = format!(
            "SELECT name FROM system.tables WHERE database = '{}' AND engine = 'View' ORDER BY name",
            db.replace('\'', "''")
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        Ok(tsv
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_materialized_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let db = schema.unwrap_or(&self.database);
        let sql = format!(
            "SELECT name FROM system.tables WHERE database = '{}' AND engine = 'MaterializedView' ORDER BY name",
            db.replace('\'', "''")
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        Ok(tsv
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_queryable_entities(&self, schema: Option<&str>) -> Result<Vec<QueryableEntity>> {
        let db = schema.unwrap_or(&self.database);
        let sql = format!(
            "SELECT name, engine FROM system.tables WHERE database = '{}' ORDER BY name",
            db.replace('\'', "''")
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        Ok(tsv
            .rows
            .into_iter()
            .filter_map(|mut row| {
                let engine = row.pop().flatten()?;
                let name = row.pop().flatten()?;
                let entity_type = match engine.as_str() {
                    "View" => EntityType::View,
                    "MaterializedView" => EntityType::MaterializedView,
                    _ => EntityType::Table,
                };
                Some(QueryableEntity { name, entity_type })
            })
            .collect())
    }

    fn supports_schemas(&self) -> bool {
        false
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        let db = schema.unwrap_or(&self.database);
        let sql = format!(
            "SELECT name, type, default_expression, is_in_primary_key, is_in_partition_key FROM system.columns WHERE database = '{}' AND table = '{}' ORDER BY position",
            db.replace('\'', "''"),
            table_name.replace('\'', "''")
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        Ok(tsv
            .rows
            .into_iter()
            .map(|mut row| {
                let is_in_pk = row.pop().flatten().map(|v| v == "1").unwrap_or(false);
                let default_expr = row.pop().flatten().filter(|v| !v.is_empty());
                let data_type = row.pop().flatten().unwrap_or_default();
                let name = row.pop().flatten().unwrap_or_default();
                let is_nullable = data_type.starts_with("Nullable(");
                ColumnInfo {
                    name,
                    data_type,
                    is_nullable,
                    is_primary_key: is_in_pk,
                    default_value: default_expr,
                    character_maximum_length: None,
                    foreign_key: None,
                }
            })
            .collect())
    }

    async fn get_indexes_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>> {
        let db = schema.unwrap_or(&self.database);
        let sql = format!(
            "SELECT name, type, expr FROM system.data_skipping_indices WHERE database = '{}' AND table = '{}'",
            db.replace('\'', "''"),
            table_name.replace('\'', "''")
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        Ok(tsv
            .rows
            .into_iter()
            .map(|mut row| {
                let expr = row.pop().flatten().unwrap_or_default();
                let algorithm = row.pop().flatten().unwrap_or_else(|| "skip".to_string());
                let name = row.pop().flatten().unwrap_or_default();
                IndexInfo {
                    name,
                    algorithm,
                    is_unique: false,
                    column_names: vec![],
                    condition: if expr.is_empty() { None } else { Some(expr) },
                    comment: None,
                }
            })
            .collect())
    }

    async fn get_database_schema_paginated(
        &self,
        database_name: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<DatabaseSchemaResult> {
        let db = database_name.unwrap_or(&self.database);
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);

        let mut where_clauses = vec![format!("database = '{}'", db.replace('\'', "''"))];
        if let Some(names) = table_names {
            let quoted: Vec<String> = names
                .split(',')
                .map(|n| format!("'{}'", n.trim().replace('\'', "''")))
                .collect();
            where_clauses.push(format!("name IN ({})", quoted.join(",")));
        }

        let sql = format!(
            "SELECT name, engine FROM system.tables WHERE {} ORDER BY name LIMIT {} OFFSET {}",
            where_clauses.join(" AND "),
            limit + 1,
            offset
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        let has_more = tsv.rows.len() > limit as usize;
        let table_rows: Vec<_> = if has_more {
            tsv.rows.into_iter().take(limit as usize).collect()
        } else {
            tsv.rows
        };

        let mut tables = Vec::new();
        for mut row in table_rows {
            let engine = row.pop().flatten().unwrap_or_default();
            let name = row.pop().flatten().unwrap_or_default();
            let object_type = match engine.as_str() {
                "View" => "VIEW".to_string(),
                "MaterializedView" => "MATERIALIZED VIEW".to_string(),
                _ => "TABLE".to_string(),
            };
            let columns = self.get_columns_for_table(&name, Some(db)).await?;
            let column_count = columns.len();
            tables.push(TableSchemaInfo {
                name,
                schema: db.to_string(),
                object_type,
                columns,
                column_count,
                referenced_by: Vec::new(),
            });
        }

        Ok(DatabaseSchemaResult {
            connection_type: "ClickHouse".to_string(),
            display_name: self.display_name.clone(),
            tables,
            pagination: PaginationInfo {
                limit: Some(limit),
                offset: Some(offset),
                has_more,
            },
        })
    }

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult> {
        let sql = format!(
            "SELECT * FROM \"{}\" WHERE \"{}\" = '{}' LIMIT 100",
            table_name.replace('"', "\"\""),
            column_name.replace('"', "\"\""),
            reference_value.replace('\'', "''")
        );
        self.execute_query(&sql, None, None).await
    }
}

fn parse_rows_affected(body: &str) -> u64 {
    body.trim()
        .lines()
        .next()
        .and_then(|line| line.trim().parse::<u64>().ok())
        .unwrap_or(0)
}
