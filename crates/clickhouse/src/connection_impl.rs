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

    async fn execute_script(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<Vec<QueryResult>> {
        // The HTTP interface runs a single query per request and cannot report
        // statement boundaries, so split the script ourselves and run each
        // statement in turn, preserving one `QueryResult` per statement.
        let statements = split_sql_statements(query);
        if statements.len() <= 1 {
            return Ok(vec![self.execute_query(query, database_name, None).await?]);
        }
        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            results.push(self.execute_query(&statement, database_name, None).await?);
        }
        Ok(results)
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

    async fn table_ddl(&self, schema: Option<&str>, table_name: &str) -> Result<String> {
        let db = schema.unwrap_or(&self.database);
        // SHOW CREATE TABLE returns a single `statement` column holding the full DDL.
        let sql = format!(
            "SHOW CREATE TABLE `{}`.`{}`",
            db.replace('`', "``"),
            table_name.replace('`', "``")
        );
        let tsv = self.raw_query_tsv(&sql, None).await?;
        tsv.rows
            .into_iter()
            .next()
            .and_then(|mut row| row.pop().flatten())
            .ok_or_else(|| anyhow::anyhow!("Table '{}.{}' not found", db, table_name))
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

/// Split a SQL script into its individual statements on top-level semicolons,
/// skipping semicolons inside string literals (`'...'`), quoted identifiers
/// (`"..."`, `` `...` ``), line comments (`-- ...`), and block comments
/// (`/* ... */`).
///
/// ClickHouse's HTTP interface (and its native protocol) runs a single
/// statement per request and rejects multi-statement scripts, so `execute_script`
/// splits here and issues one request per statement. The sqlx-based backends
/// don't need this: their drivers report statement boundaries directly via
/// `fetch_many`.
///
/// Trailing/blank fragments are dropped, so a script ending in `;` does not
/// yield an empty final statement. The returned statements are trimmed and
/// exclude the separating semicolon.
fn split_sql_statements(sql: &str) -> Vec<String> {
    #[derive(PartialEq)]
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        Backtick,
        LineComment,
        BlockComment,
    }

    let mut statements = Vec::new();
    let mut current = String::new();
    let mut state = State::Normal;
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        match state {
            State::Normal => match ch {
                '\'' => {
                    state = State::SingleQuote;
                    current.push(ch);
                }
                '"' => {
                    state = State::DoubleQuote;
                    current.push(ch);
                }
                '`' => {
                    state = State::Backtick;
                    current.push(ch);
                }
                '-' if chars.peek() == Some(&'-') => {
                    state = State::LineComment;
                    current.push(ch);
                    current.push(chars.next().unwrap_or('-'));
                }
                '/' if chars.peek() == Some(&'*') => {
                    state = State::BlockComment;
                    current.push(ch);
                    current.push(chars.next().unwrap_or('*'));
                }
                ';' => {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        statements.push(trimmed.to_string());
                    }
                    current.clear();
                }
                _ => current.push(ch),
            },
            State::SingleQuote => {
                current.push(ch);
                // Doubled quote (`''`) is an escaped quote, staying in-string.
                if ch == '\'' {
                    if chars.peek() == Some(&'\'') {
                        current.push(chars.next().unwrap_or('\''));
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::DoubleQuote => {
                current.push(ch);
                if ch == '"' {
                    if chars.peek() == Some(&'"') {
                        current.push(chars.next().unwrap_or('"'));
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::Backtick => {
                current.push(ch);
                if ch == '`' {
                    state = State::Normal;
                }
            }
            State::LineComment => {
                current.push(ch);
                if ch == '\n' {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                current.push(ch);
                if ch == '*' && chars.peek() == Some(&'/') {
                    current.push(chars.next().unwrap_or('/'));
                    state = State::Normal;
                }
            }
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        statements.push(trimmed.to_string());
    }

    statements
}

#[cfg(test)]
mod split_tests {
    use super::split_sql_statements;

    #[test]
    fn splits_simple_statements() {
        assert_eq!(
            split_sql_statements("SELECT 1; SELECT 2"),
            vec!["SELECT 1".to_string(), "SELECT 2".to_string()]
        );
    }

    #[test]
    fn trailing_semicolon_yields_no_empty_statement() {
        assert_eq!(
            split_sql_statements("SELECT 1;"),
            vec!["SELECT 1".to_string()]
        );
        assert_eq!(split_sql_statements("   ;  ; "), Vec::<String>::new());
    }

    #[test]
    fn ignores_semicolons_in_strings_and_comments() {
        assert_eq!(
            split_sql_statements("SELECT ';'; SELECT 2"),
            vec!["SELECT ';'".to_string(), "SELECT 2".to_string()]
        );
        assert_eq!(
            split_sql_statements("SELECT '' '' ;SELECT 2"),
            vec!["SELECT '' ''".to_string(), "SELECT 2".to_string()]
        );
        assert_eq!(
            split_sql_statements("SELECT 1 -- a; b\n; SELECT 2"),
            vec!["SELECT 1 -- a; b".to_string(), "SELECT 2".to_string()]
        );
        assert_eq!(
            split_sql_statements("SELECT 1 /* a; b */; SELECT 2"),
            vec!["SELECT 1 /* a; b */".to_string(), "SELECT 2".to_string()]
        );
        assert_eq!(
            split_sql_statements("SELECT `a;b`, \"c;d\" FROM t"),
            vec!["SELECT `a;b`, \"c;d\" FROM t".to_string()]
        );
    }

    #[test]
    fn escaped_quotes_stay_in_string() {
        assert_eq!(
            split_sql_statements("SELECT 'it''s; fine'; SELECT 2"),
            vec!["SELECT 'it''s; fine'".to_string(), "SELECT 2".to_string()]
        );
    }
}
