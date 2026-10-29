mod connection_impl;

use anyhow::Result;
use blanco_core::connection_trait::ColumnType;
use blanco_core::{ConnectionLost, QueryTiming, StatementTimer};
use reqwest::Client as HttpClient;

/// Convert a reqwest error into an `anyhow::Error`, attaching the shared
/// [`ConnectionLost`] marker when it represents a dropped or unreachable server.
/// ClickHouse talks HTTP over reqwest rather than sqlx, so without this its
/// dropped connections would never be classified and the reconnect UX would
/// diverge from the sqlx-based backends.
fn tag_reqwest(error: reqwest::Error) -> anyhow::Error {
    if reqwest_connection_lost(&error) {
        anyhow::Error::new(error).context(ConnectionLost)
    } else {
        anyhow::Error::new(error)
    }
}

/// True when a reqwest error means the connection to the server failed: an
/// unreachable/refused host, a timeout, or a socket dropped mid-request
/// (broken pipe, reset, unexpected EOF) found by walking the source chain.
fn reqwest_connection_lost(error: &reqwest::Error) -> bool {
    if error.is_connect() || error.is_timeout() {
        return true;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::NotConnected
            )
        {
            return true;
        }
        source = current.source();
    }
    false
}

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
        let (body, _) = self.raw_query_timed(sql, database).await?;
        Ok(body)
    }

    /// Run a query and time it, counting the first body chunk as the first
    /// row: ClickHouse streams the response while the query runs.
    async fn raw_query_timed(
        &self,
        sql: &str,
        database: Option<&str>,
    ) -> Result<(String, StatementTimer)> {
        self.raw_query_timed_with_parameters(sql, database, &[])
            .await
    }

    /// Like `raw_query_timed`, binding `{name:Type}` placeholders to
    /// `parameters`, sent as `param_<name>` in the URL.
    async fn raw_query_timed_with_parameters(
        &self,
        sql: &str,
        database: Option<&str>,
        parameters: &[(String, String)],
    ) -> Result<(String, StatementTimer)> {
        let url = self.build_query_url(database, &[]);
        let parameters: Vec<(String, String)> = parameters
            .iter()
            .map(|(name, value)| (format!("param_{name}"), escape_parameter(value)))
            .collect();
        let mut timer = StatementTimer::start();
        let mut response = self
            .http
            .post(&url)
            .query(&parameters)
            .body(sql.to_string())
            .send()
            .await
            .map_err(tag_reqwest)?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(tag_reqwest)? {
            timer.row();
            bytes.extend_from_slice(&chunk);
        }
        let body = String::from_utf8_lossy(&bytes).into_owned();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "ClickHouse query failed ({}): {}",
                status,
                body.trim()
            ));
        }
        Ok((body, timer))
    }

    async fn raw_query_tsv(&self, sql: &str, database: Option<&str>) -> Result<TsvResult> {
        let (tsv, _) = self.raw_query_tsv_timed(sql, database).await?;
        Ok(tsv)
    }

    async fn raw_query_tsv_with_parameters(
        &self,
        sql: &str,
        parameters: &[(String, String)],
    ) -> Result<TsvResult> {
        let sql_with_format = format!("{} FORMAT TSVWithNamesAndTypes", sql.trim_end_matches(';'));
        let (body, _) = self
            .raw_query_timed_with_parameters(&sql_with_format, None, parameters)
            .await?;
        parse_tsv(&body)
    }

    async fn raw_query_tsv_timed(
        &self,
        sql: &str,
        database: Option<&str>,
    ) -> Result<(TsvResult, QueryTiming)> {
        let sql_with_format = format!("{} FORMAT TSVWithNamesAndTypes", sql.trim_end_matches(';'));
        let (body, timer) = self.raw_query_timed(&sql_with_format, database).await?;
        let tsv = parse_tsv(&body)?;
        let timing = timer.finish();
        // The header lines arrive as a chunk even when no row follows.
        let timing = if tsv.rows.is_empty() {
            QueryTiming {
                execution: timing.total(),
                fetch: None,
            }
        } else {
            timing
        };
        Ok((tsv, timing))
    }

    fn map_clickhouse_type(type_name: &str) -> ColumnType {
        blanco_core::DatabaseType::ClickHouse
            .dialect()
            .column_type(type_name)
    }
}

struct TsvResult {
    columns: Vec<String>,
    column_types: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

/// ClickHouse reads a query parameter's value in its escaped text format,
/// where a backslash starts an escape sequence.
fn escape_parameter(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
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

#[cfg(test)]
#[allow(clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use blanco_core::Connection;
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

    /// Decide whether to fail or skip a test when the ClickHouse server is
    /// not reachable. `BLANCO_RUN_DB_TESTS=1` makes missing servers a hard
    /// failure; otherwise prints a skip message and returns `Ok(())`.
    fn require_db_or_skip(
        test_name: &str,
        err: &dyn std::fmt::Display,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1") {
            Err(format!("{test_name}: ClickHouse unreachable: {err}").into())
        } else {
            eprintln!(
                "skip {test_name}: ClickHouse unreachable ({err}). Set BLANCO_RUN_DB_TESTS=1 to require."
            );
            Ok(())
        }
    }

    #[test]
    fn test_clickhouse_data_type_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            let connection_string = env::var("CLICKHOUSE_CONNECTION_STRING").unwrap_or_else(|_| {
                "http://blanco:blanco@localhost:8124/?database=blanco".to_string()
            });

            let mut conn = ClickhouseConnection::from_connection_string(&connection_string)?;

            if let Err(e) = conn.connect(&connection_string).await {
                return require_db_or_skip("test_clickhouse_data_type_serialization", &e);
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

    #[test]
    fn test_clickhouse_ping_validates_credentials() -> Result<(), Box<dyn std::error::Error>> {
        use std::time::{Duration, Instant};

        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            let test_name = "test_clickhouse_ping_validates_credentials";
            let connection_string = env::var("CLICKHOUSE_CONNECTION_STRING").unwrap_or_else(|_| {
                "http://blanco:blanco@localhost:8124/?database=blanco".to_string()
            });

            // Valid credentials should ping successfully (or skip if unreachable).
            let conn = ClickhouseConnection::from_connection_string(&connection_string)?;
            if let Err(e) = conn.ping().await {
                return require_db_or_skip(test_name, &e);
            }

            // A bad password must fail, and reasonably fast (an HTTP auth error
            // returns immediately).
            let bad = ClickhouseConnection::from_connection_string(
                "http://blanco:wrong-password@localhost:8124/?database=blanco",
            )?;
            let started = Instant::now();
            let result = bad.ping().await;
            let elapsed = started.elapsed();

            assert!(result.is_err(), "ping with a bad password must error");
            assert!(
                elapsed < Duration::from_secs(10),
                "ping should fail fast, took {elapsed:?}"
            );

            Ok(())
        })
    }

    #[test]
    fn test_clickhouse_schema_wide_reads_match_the_per_table_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            let test_name = "test_clickhouse_schema_wide_reads_match_the_per_table_reads";
            let connection_string = env::var("CLICKHOUSE_CONNECTION_STRING").unwrap_or_else(|_| {
                "http://blanco:blanco@localhost:8124/?database=blanco".to_string()
            });
            let mut conn = ClickhouseConnection::from_connection_string(&connection_string)?;
            if let Err(e) = conn.connect(&connection_string).await {
                return require_db_or_skip(test_name, &e);
            }

            // A database of its own, so tables other tests create meanwhile
            // do not change the set compared.
            let database = "blanco_schema_graph";
            let other_database = "blanco_schema_graph_other";
            conn.execute_script(
                &format!(
                    "DROP DATABASE IF EXISTS {database};
                     DROP DATABASE IF EXISTS {other_database};
                     CREATE DATABASE {database};
                     CREATE DATABASE {other_database};
                     CREATE TABLE {database}.orders (id UInt64, region String, total Decimal(10, 2), note Nullable(String) COMMENT 'Free text')
                         ENGINE = MergeTree ORDER BY (region, id) COMMENT 'Placed orders';
                     CREATE TABLE {other_database}.remote (id UInt64 COMMENT 'Remote key', label String)
                         ENGINE = Memory COMMENT 'Lives elsewhere';
                     CREATE TABLE {database}.`it's` (key String, value String) ENGINE = MergeTree ORDER BY key;
                     CREATE TABLE {database}.events (at DateTime) ENGINE = Memory;
                     CREATE VIEW {database}.big_orders AS SELECT * FROM {database}.orders WHERE total > 100;
                     CREATE MATERIALIZED VIEW {database}.order_totals ENGINE = MergeTree ORDER BY region
                         AS SELECT region, sum(total) AS total FROM {database}.orders GROUP BY region;"
                ),
                None,
            )
            .await?;

            let mut expected = Vec::new();
            let comments = conn.get_object_comments(Some(database)).await?.relations;
            for table in conn.get_tables(Some(database)).await? {
                let columns = conn.get_columns_for_table(&table, Some(database)).await?;
                expected.push(blanco_core::SchemaTableColumns {
                    schema: None,
                    comment: comments.get(&table).cloned(),
                    table,
                    columns: columns.into_iter().map(blanco_core::SchemaColumn::from).collect(),
                });
            }
            for (schema, table) in [("system", "one"), (other_database, "remote")] {
                let columns = conn.get_columns_for_table(table, Some(schema)).await?;
                let comments = conn.get_object_comments(Some(schema)).await?.relations;
                expected.push(blanco_core::SchemaTableColumns {
                    schema: Some(schema.to_string()),
                    table: table.to_string(),
                    comment: comments.get(table).cloned(),
                    columns: columns.into_iter().map(blanco_core::SchemaColumn::from).collect(),
                });
            }

            let referenced = [
                ("system".to_string(), "one".to_string()),
                ("system".to_string(), "missing\\table".to_string()),
                (database.to_string(), "missing".to_string()),
                (other_database.to_string(), "remote".to_string()),
            ];
            let result = conn.schema_columns(Some(database), &referenced).await;
            let keys = conn.schema_foreign_keys(Some(database)).await;
            conn.execute_script(
                &format!("DROP DATABASE {database}; DROP DATABASE {other_database};"),
                None,
            )
            .await?;

            let tables = result?;
            assert_eq!(tables, expected);
            // The materialized view's storage table (`.inner_id.<uuid>`) is
            // listed too, as `get_tables` lists it.
            assert_eq!(
                tables
                    .iter()
                    .map(|table| table.table.as_str())
                    .filter(|table| !table.starts_with(".inner"))
                    .collect::<Vec<_>>(),
                ["events", "it's", "orders", "one", "remote"]
            );
            let comments: Vec<_> = tables
                .iter()
                .filter(|table| table.table == "orders" || table.table == "remote")
                .map(|table| {
                    let column_comments: Vec<_> = table
                        .columns
                        .iter()
                        .map(|column| column.comment.as_deref())
                        .collect();
                    (table.comment.as_deref(), column_comments)
                })
                .collect();
            assert_eq!(
                comments,
                [
                    (Some("Placed orders"), vec![None, None, None, Some("Free text")]),
                    (Some("Lives elsewhere"), vec![Some("Remote key"), None]),
                ]
            );
            let orders_keys: Vec<bool> = tables
                .iter()
                .find(|table| table.table == "orders")
                .map(|table| table.columns.iter().map(|column| column.is_primary_key).collect())
                .unwrap_or_default();
            assert_eq!(orders_keys, [true, true, false, false]);
            assert!(keys?.is_empty());
            Ok(())
        })
    }
}
