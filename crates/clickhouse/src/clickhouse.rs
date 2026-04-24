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
use reqwest::Client as HttpClient;
use std::time::Instant;

pub struct ClickhouseConnection {
    http: HttpClient,
    base_url: String,
    username: String,
    password: Option<String>,
    database: String,
    display_name: String,
    connected: bool,
}

impl ClickhouseConnection {
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let parsed = url::Url::parse(connection_string)
            .map_err(|e| anyhow::anyhow!("invalid ClickHouse connection URL: {}", e))?;

        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(anyhow::anyhow!(
                "ClickHouse connection URL must use http or https, got '{}'",
                scheme
            ));
        }

        let host = parsed.host_str().unwrap_or("localhost");
        let port = parsed
            .port()
            .unwrap_or(if scheme == "https" { 8443 } else { 8123 });
        let base_url = format!("{}://{}:{}", scheme, host, port);

        let username = parsed.username().to_string();
        let password = parsed.password().map(|p| p.to_string());

        let mut database = "default".to_string();
        let mut ssl_mode: Option<String> = None;
        let mut ssl_root_cert: Option<String> = None;
        let mut ssl_client_cert: Option<String> = None;
        let mut ssl_client_key: Option<String> = None;
        for (k, v) in parsed.query_pairs() {
            match k.as_ref() {
                "database" => database = v.to_string(),
                "ssl_mode" => ssl_mode = Some(v.to_string()),
                "sslrootcert" => ssl_root_cert = Some(v.to_string()),
                "sslcert" => ssl_client_cert = Some(v.to_string()),
                "sslkey" => ssl_client_key = Some(v.to_string()),
                _ => {}
            }
        }

        let display_name = format!(
            "{}@{}:{}/{}",
            if username.is_empty() {
                "default"
            } else {
                &username
            },
            host,
            port,
            database
        );

        let mut builder = HttpClient::builder().pool_max_idle_per_host(2);
        if scheme == "https" {
            match ssl_mode.as_deref() {
                Some("disabled") => {
                    builder = builder
                        .danger_accept_invalid_certs(true)
                        .danger_accept_invalid_hostnames(true);
                }
                Some("verify-ca") => {
                    builder = builder.danger_accept_invalid_hostnames(true);
                }
                _ => {}
            }
            if let Some(ca_path) = &ssl_root_cert {
                let pem = std::fs::read(ca_path).map_err(|e| {
                    anyhow::anyhow!("failed to read CA certificate {}: {}", ca_path, e)
                })?;
                let cert = reqwest::Certificate::from_pem(&pem)
                    .map_err(|e| anyhow::anyhow!("invalid CA certificate: {}", e))?;
                builder = builder.add_root_certificate(cert);
            }
            if let (Some(cert_path), Some(key_path)) = (&ssl_client_cert, &ssl_client_key) {
                let cert_pem = std::fs::read(cert_path).map_err(|e| {
                    anyhow::anyhow!("failed to read client certificate {}: {}", cert_path, e)
                })?;
                let key_pem = std::fs::read(key_path).map_err(|e| {
                    anyhow::anyhow!("failed to read client key {}: {}", key_path, e)
                })?;
                let identity = reqwest::Identity::from_pkcs8_pem(&cert_pem, &key_pem)
                    .map_err(|e| anyhow::anyhow!("invalid client identity: {}", e))?;
                builder = builder.identity(identity);
            }
        }
        let http = builder
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {}", e))?;

        Ok(Self {
            http,
            base_url,
            username,
            password,
            database,
            display_name,
            connected: false,
        })
    }

    fn build_query_url(&self, database: Option<&str>, extra_params: &[(&str, &str)]) -> String {
        let db = database.unwrap_or(&self.database);
        let mut url = format!("{}?database={}", self.base_url, db);
        if !self.username.is_empty() {
            url.push_str(&format!("&user={}", self.username));
        }
        if let Some(ref pw) = self.password {
            url.push_str(&format!("&password={}", pw));
        }
        for (k, v) in extra_params {
            url.push_str(&format!("&{}={}", k, v));
        }
        url
    }

    async fn raw_query(&self, sql: &str, database: Option<&str>) -> Result<String> {
        // reqwest depends on a tokio reactor being available on the current
        // thread. Blanco's database service runs on smol, so wrap the HTTP
        // call in `async_compat::Compat` which starts and enters a shared
        // tokio runtime when the future is polled.
        let url = self.build_query_url(database, &[]);
        let http = self.http.clone();
        let body_bytes = sql.to_string();
        async_compat::Compat::new(async move {
            let response = http.post(&url).body(body_bytes).send().await?;
            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                return Err(anyhow::anyhow!(
                    "ClickHouse query failed ({}): {}",
                    status,
                    body.trim()
                ));
            }
            Ok(body)
        })
        .await
    }

    async fn raw_query_tsv(&self, sql: &str, database: Option<&str>) -> Result<TsvResult> {
        let sql_with_format = format!("{} FORMAT TSVWithNamesAndTypes", sql.trim_end_matches(';'));
        let body = self.raw_query(&sql_with_format, database).await?;
        parse_tsv(&body)
    }

    fn map_clickhouse_type(type_name: &str) -> ColumnType {
        let inner = Self::unwrap_type_wrappers(type_name);
        let lower = inner.to_lowercase();

        if lower.starts_with("uint") {
            ColumnType::UnsignedInteger
        } else if lower.starts_with("int")
            || lower == "bigint"
            || lower == "smallint"
            || lower == "tinyint"
            || lower == "mediumint"
            || lower == "serial"
            || lower == "bigserial"
        {
            ColumnType::Integer
        } else if lower.starts_with("float")
            || lower.starts_with("double")
            || lower == "real"
            || lower.starts_with("decimal")
            || lower.starts_with("numeric")
            || lower == "money"
        {
            ColumnType::Numeric
        } else if lower == "bool" || lower == "boolean" {
            ColumnType::Boolean
        } else if lower.starts_with("date")
            || lower.starts_with("datetime")
            || lower == "time"
            || lower == "timestamp"
        {
            ColumnType::DateTime
        } else if lower == "uuid" {
            ColumnType::Uuid
        } else if lower == "json" || lower.starts_with("object") {
            ColumnType::Json
        } else if lower.starts_with("array") {
            ColumnType::Array
        } else if lower.starts_with("string")
            || lower.starts_with("fixedstring")
            || lower.starts_with("enum")
            || lower.starts_with("lowcardinality")
            || lower == "name"
            || lower == "nullable"
        {
            ColumnType::Text
        } else {
            ColumnType::Unknown
        }
    }

    fn unwrap_type_wrappers(type_name: &str) -> &str {
        let s = type_name.trim();
        if let Some(inner) = s.strip_prefix("Nullable(")
            && let Some(inner) = inner.strip_suffix(')')
        {
            return Self::unwrap_type_wrappers(inner);
        }
        if let Some(inner) = s.strip_prefix("LowCardinality(")
            && let Some(inner) = inner.strip_suffix(')')
        {
            return Self::unwrap_type_wrappers(inner);
        }
        s
    }
}

struct TsvResult {
    columns: Vec<String>,
    column_types: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

fn parse_tsv(body: &str) -> Result<TsvResult> {
    let mut lines = body.lines();

    let header_line = match lines.next() {
        Some(l) => l,
        None => {
            return Ok(TsvResult {
                columns: vec![],
                column_types: vec![],
                rows: vec![],
            });
        }
    };
    let type_line = match lines.next() {
        Some(l) => l,
        None => {
            return Ok(TsvResult {
                columns: split_tsv_row(header_line),
                column_types: vec![],
                rows: vec![],
            });
        }
    };

    let columns = split_tsv_row(header_line);
    let column_types = split_tsv_row(type_line);

    let mut rows = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        rows.push(split_tsv_row_with_null(line));
    }

    Ok(TsvResult {
        columns,
        column_types,
        rows,
    })
}

fn split_tsv_row(line: &str) -> Vec<String> {
    line.split('\t').map(|s| s.to_string()).collect()
}

fn split_tsv_row_with_null(line: &str) -> Vec<Option<String>> {
    line.split('\t')
        .map(|cell| {
            if cell == "\\N" {
                None
            } else {
                Some(unescape_tsv_cell(cell))
            }
        })
        .collect()
}

fn unescape_tsv_cell(cell: &str) -> String {
    let mut result = String::with_capacity(cell.len());
    let chars: Vec<char> = cell.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                'n' => {
                    result.push('\n');
                    i += 2;
                    continue;
                }
                't' => {
                    result.push('\t');
                    i += 2;
                    continue;
                }
                'r' => {
                    result.push('\r');
                    i += 2;
                    continue;
                }
                '0' => {
                    result.push('\0');
                    i += 2;
                    continue;
                }
                '\\' => {
                    result.push('\\');
                    i += 2;
                    continue;
                }
                '\'' => {
                    result.push('\'');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_map_clickhouse_types() {
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("UInt64"),
            ColumnType::UnsignedInteger
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("UInt8"),
            ColumnType::UnsignedInteger
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Int32"),
            ColumnType::Integer
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Int64"),
            ColumnType::Integer
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Float64"),
            ColumnType::Numeric
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Decimal(18,3)"),
            ColumnType::Numeric
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Bool"),
            ColumnType::Boolean
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("String"),
            ColumnType::Text
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("FixedString(32)"),
            ColumnType::Text
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("DateTime"),
            ColumnType::DateTime
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("DateTime64(3)"),
            ColumnType::DateTime
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Date"),
            ColumnType::DateTime
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Date32"),
            ColumnType::DateTime
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("UUID"),
            ColumnType::Uuid
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("JSON"),
            ColumnType::Json
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Array(Int32)"),
            ColumnType::Array
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Nullable(String)"),
            ColumnType::Text
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("LowCardinality(String)"),
            ColumnType::Text
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Nullable(Int64)"),
            ColumnType::Integer
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Map(String, Int64)"),
            ColumnType::Unknown
        );
        assert_eq!(
            ClickhouseConnection::map_clickhouse_type("Tuple(Int32, String)"),
            ColumnType::Unknown
        );
    }

    #[test]
    fn test_parse_tsv_basic() {
        let body = "name\tage\tcity\nString\tUInt32\tString\nAlice\t30\tNYC\nBob\t\\N\tLA\n";
        let result = parse_tsv(body).unwrap();
        assert_eq!(result.columns, vec!["name", "age", "city"]);
        assert_eq!(result.column_types, vec!["String", "UInt32", "String"]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(
            result.rows[0],
            vec![Some("Alice".into()), Some("30".into()), Some("NYC".into())]
        );
        assert_eq!(
            result.rows[1],
            vec![Some("Bob".into()), None, Some("LA".into())]
        );
    }

    #[test]
    fn test_parse_tsv_empty() {
        let result = parse_tsv("").unwrap();
        assert!(result.columns.is_empty());
        assert!(result.rows.is_empty());
    }

    #[test]
    fn test_parse_tsv_header_only() {
        let result = parse_tsv("name\tage\nString\tUInt32\n").unwrap();
        assert_eq!(result.columns, vec!["name", "age"]);
        assert!(result.rows.is_empty());
    }

    #[test]
    fn test_unescape_tsv() {
        assert_eq!(unescape_tsv_cell("hello\\nworld"), "hello\nworld");
        assert_eq!(unescape_tsv_cell("tab\\there"), "tab\there");
        assert_eq!(unescape_tsv_cell("back\\\\slash"), "back\\slash");
    }

    #[test]
    fn test_connection_string_parsing() {
        let conn = ClickhouseConnection::from_connection_string(
            "http://admin:secret@ch.example.com:8123/?database=analytics",
        )
        .unwrap();
        assert_eq!(conn.base_url, "http://ch.example.com:8123");
        assert_eq!(conn.username, "admin");
        assert_eq!(conn.password, Some("secret".to_string()));
        assert_eq!(conn.database, "analytics");
    }

    #[test]
    fn test_connection_string_default_port() {
        let conn =
            ClickhouseConnection::from_connection_string("http://localhost/?database=default")
                .unwrap();
        assert_eq!(conn.base_url, "http://localhost:8123");
        assert_eq!(conn.database, "default");
    }

    #[test]
    fn test_clickhouse_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            let connection_string = env::var("CLICKHOUSE_CONNECTION_STRING").unwrap_or_else(|_| {
                "http://blanco:blanco@localhost:8124/?database=blanco".to_string()
            });

            let mut conn = ClickhouseConnection::from_connection_string(&connection_string)?;

            if conn.connect(&connection_string).await.is_err() {
                println!(
                    "Skipping ClickHouse serialization test - server not available at {}",
                    connection_string
                );
                println!(
                    "To run this test, start the container with: docker-compose up -d clickhousetestdb"
                );
                return Ok(());
            }

            // Drop table if exists
            let _ = conn
                .execute_query("DROP TABLE IF EXISTS comprehensive_test", None, None)
                .await;

            // Create table with a broad set of ClickHouse data types
            conn.execute_query(
                r#"
                CREATE TABLE comprehensive_test (
                    id UInt64,
                    uint8_col UInt8,
                    uint32_col UInt32,
                    int8_col Int8,
                    int32_col Int32,
                    int64_col Int64,
                    float32_col Float32,
                    float64_col Float64,
                    decimal_col Decimal(10, 2),
                    string_col String,
                    fixed_string_col FixedString(16),
                    bool_col Bool,
                    date_col Date,
                    date32_col Date32,
                    datetime_col DateTime,
                    uuid_col UUID,
                    nullable_int_col Nullable(Int64),
                    nullable_string_col Nullable(String),
                    array_int_col Array(Int32),
                    low_card_col LowCardinality(String)
                )
                ENGINE = MergeTree()
                ORDER BY id
                "#,
                None,
                None,
            )
            .await?;

            // Insert a row
            conn.execute_query(
                r#"
                INSERT INTO comprehensive_test VALUES (
                    1,
                    255,
                    4000000000,
                    -128,
                    2147483647,
                    9223372036854775807,
                    123.456,
                    987654321.123456789,
                    12345.67,
                    'hello world with unicode: ñiño 你好 🚀',
                    'fixed_16_bytes_',
                    true,
                    '2025-01-15',
                    '2025-01-15',
                    '2025-06-15 12:30:45',
                    '550e8400-e29b-41d4-a716-446655440000',
                    NULL,
                    'not null string',
                    [1, 2, 3, 4, 5],
                    'repeated_value'
                )
                "#,
                None,
                None,
            )
            .await?;

            // Query it back through Blanco's connection
            let result = conn
                .execute_query("SELECT * FROM comprehensive_test ORDER BY id", None, None)
                .await?;

            assert!(
                !result.rows.is_empty(),
                "Query should return at least one row"
            );
            let first_row = result.rows.first().unwrap();

            let value_map: std::collections::HashMap<String, Option<String>> = result
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| (col.clone(), first_row[i].clone()))
                .collect();

            let get_val =
                |key: &str| -> &str { value_map.get(key).and_then(|v| v.as_deref()).unwrap_or("") };

            // Unsigned integers
            assert_eq!(get_val("id"), "1");
            assert_eq!(get_val("uint8_col"), "255");
            assert_eq!(get_val("uint32_col"), "4000000000");

            // Signed integers
            assert_eq!(get_val("int8_col"), "-128");
            assert_eq!(get_val("int32_col"), "2147483647");
            assert_eq!(get_val("int64_col"), "9223372036854775807");

            // Floats
            assert!(get_val("float32_col").starts_with("123.456"));
            assert!(get_val("float64_col").starts_with("987654321.123456"));

            // Decimal
            assert_eq!(get_val("decimal_col"), "12345.67");

            // Strings
            assert_eq!(
                get_val("string_col"),
                "hello world with unicode: ñiño 你好 🚀"
            );
            assert!(get_val("fixed_string_col").starts_with("fixed_16_bytes_"));

            // Boolean
            assert_eq!(get_val("bool_col"), "true");

            // Dates (verify they contain expected patterns)
            let date_val = get_val("date_col");
            assert!(
                date_val.contains("2025-01-15"),
                "Date should contain 2025-01-15, got: {}",
                date_val
            );

            // DateTime
            let dt_val = get_val("datetime_col");
            assert!(
                dt_val.contains("2025-06-15"),
                "DateTime should contain 2025-06-15, got: {}",
                dt_val
            );
            assert!(
                dt_val.contains("12:30:45"),
                "DateTime should contain 12:30:45, got: {}",
                dt_val
            );

            // UUID
            assert_eq!(get_val("uuid_col"), "550e8400-e29b-41d4-a716-446655440000");

            // Nullable columns
            assert!(
                value_map.get("nullable_int_col").unwrap().is_none(),
                "nullable_int_col should be NULL"
            );
            assert_eq!(get_val("nullable_string_col"), "not null string");

            // Array
            assert_eq!(get_val("array_int_col"), "[1,2,3,4,5]");

            // LowCardinality
            assert_eq!(get_val("low_card_col"), "repeated_value");

            // Verify column types were detected correctly
            let type_map: std::collections::HashMap<String, ColumnType> = result
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| (col.clone(), result.column_types[i]))
                .collect();

            assert_eq!(type_map["id"], ColumnType::UnsignedInteger);
            assert_eq!(type_map["int64_col"], ColumnType::Integer);
            assert_eq!(type_map["float64_col"], ColumnType::Numeric);
            assert_eq!(type_map["string_col"], ColumnType::Text);
            assert_eq!(type_map["bool_col"], ColumnType::Boolean);
            assert_eq!(type_map["date_col"], ColumnType::DateTime);
            assert_eq!(type_map["datetime_col"], ColumnType::DateTime);
            assert_eq!(type_map["uuid_col"], ColumnType::Uuid);
            assert_eq!(type_map["array_int_col"], ColumnType::Array);

            // Clean up
            let _ = conn
                .execute_query("DROP TABLE comprehensive_test", None, None)
                .await;

            Ok(())
        })
    }
}
