use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, QueryResult,
    connection_trait::{
        ColumnType, DatabaseSchemaResult, EntityType, ForeignKeyInfo, InboundForeignKey, IndexInfo,
        PaginationInfo, QueryableEntity, RoutineKind, TableSchemaInfo,
    },
};
use futures::Stream;
use std::collections::HashMap;
use std::sync::Arc;
use tiberius::{AuthMethod, Client, Config as TiberiusConfig, Query, Row};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::schema;

/// A live tiberius client over a tokio TCP stream (tiberius speaks futures-io,
/// so the stream is adapted with tokio-util's compat shim).
type MssqlClient = Client<Compat<TcpStream>>;

/// Cap on SQL Server connection redirects (`Error::Routing`, used by named
/// instances / Azure gateways) before giving up.
const MAX_ROUTING_REDIRECTS: usize = 2;

/// Whether a tiberius error means the connection is no longer usable and the
/// cached client should be dropped. `Server` errors are SQL-level (bad query,
/// permission, ...) and leave the connection healthy; everything else
/// (I/O, protocol, TLS) indicates a dead transport.
fn is_connection_lost(error: &tiberius::error::Error) -> bool {
    !matches!(error, tiberius::error::Error::Server(_))
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MssqlServerKey {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub trust_cert: bool,
    pub encrypt: bool,
}

pub struct MssqlConnection {
    /// One lazily-established client per database. The outer mutex guards the
    /// map; each inner mutex serializes access to a single client (a tiberius
    /// client cannot run concurrent queries) and lets a broken client be
    /// transparently replaced on the next use.
    clients: Arc<Mutex<HashMap<String, Arc<Mutex<Option<MssqlClient>>>>>>,
    server_key: MssqlServerKey,
    password: String,
    display_name: String,
    initial_database: Option<String>,
}

impl MssqlConnection {
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let parsed = url::Url::parse(connection_string)
            .map_err(|e| anyhow::anyhow!("invalid MSSQL connection URL: {e}"))?;

        let scheme = parsed.scheme();
        if scheme != "mssql" && scheme != "sqlserver" {
            return Err(anyhow::anyhow!(
                "MSSQL connection URL must use mssql:// or sqlserver://, got '{scheme}'"
            ));
        }

        let host = parsed.host_str().unwrap_or("localhost").to_string();
        let port = parsed.port().unwrap_or(1433);
        let username = parsed.username().to_string();
        let password = parsed.password().unwrap_or("").to_string();

        if username.is_empty() {
            return Err(anyhow::anyhow!(
                "username is required in MSSQL connection string"
            ));
        }

        let database = parsed.path().trim_start_matches('/').to_string();
        let initial_database = if database.is_empty() {
            None
        } else {
            Some(database)
        };

        let trust_cert = parsed.query_pairs().any(|(k, v)| {
            (k == "trust_cert" || k == "trustServerCertificate") && (v == "true" || v == "yes")
        });
        let encrypt = parsed
            .query_pairs()
            .find(|(k, _)| k == "encrypt")
            .map(|(_, v)| v == "true" || v == "yes" || v == "mandatory")
            .unwrap_or(false);

        let display_name = format!("{username}@{host}:{port}");

        Ok(Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            server_key: MssqlServerKey {
                host,
                port,
                username,
                trust_cert,
                encrypt,
            },
            password,
            display_name,
            initial_database,
        })
    }

    fn build_tiberius_config(&self, database: Option<&str>) -> TiberiusConfig {
        let mut config = TiberiusConfig::new();
        config.host(&self.server_key.host);
        config.port(self.server_key.port);
        config.authentication(AuthMethod::sql_server(
            &self.server_key.username,
            &self.password,
        ));
        if self.server_key.trust_cert {
            config.trust_cert();
        }
        config.encryption(if self.server_key.encrypt {
            tiberius::EncryptionLevel::Required
        } else {
            tiberius::EncryptionLevel::Off
        });

        if let Some(db) = database.or(self.initial_database.as_deref()) {
            config.database(db);
        }

        config
    }

    fn resolve_database(&self, database: Option<&str>) -> String {
        database
            .or(self.initial_database.as_deref())
            .unwrap_or("master")
            .to_string()
    }

    /// Open a fresh tiberius connection for `database`, following SQL Server
    /// connection redirects (named instances / Azure) up to a small bound.
    async fn establish(&self, database: Option<&str>) -> Result<MssqlClient> {
        let mut redirect: Option<(String, u16)> = None;
        for _ in 0..=MAX_ROUTING_REDIRECTS {
            let mut config = self.build_tiberius_config(database);
            if let Some((host, port)) = &redirect {
                config.host(host);
                config.port(*port);
            }
            let addr = config.get_addr();

            let tcp = TcpStream::connect(&addr)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to connect to SQL Server at {addr}: {e}"))?;
            tcp.set_nodelay(true).ok();

            match Client::connect(config, tcp.compat_write()).await {
                Ok(client) => return Ok(client),
                Err(tiberius::error::Error::Routing { host, port }) => {
                    redirect = Some((host, port));
                }
                Err(e) => return Err(anyhow::anyhow!("Failed to connect to SQL Server: {e}")),
            }
        }
        Err(anyhow::anyhow!(
            "SQL Server kept redirecting after {MAX_ROUTING_REDIRECTS} attempts"
        ))
    }

    /// Get (creating if needed) the per-database client slot. The returned slot
    /// may hold `None` until first use; callers lock it and establish on demand.
    async fn client_slot(&self, database: Option<&str>) -> Arc<Mutex<Option<MssqlClient>>> {
        let db = self.resolve_database(database);
        let mut clients = self.clients.lock().await;
        clients
            .entry(db)
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    }

    fn column_value_to_string(
        row: &Row,
        col_index: usize,
        col_type: &ColumnType,
    ) -> Option<String> {
        // Always use `try_get`, never `get`: tiberius' `get` *panics* on a
        // Rust/SQL type mismatch, and SQL Server's integer/temporal types map to
        // several distinct Rust types (INT->i32, BIGINT->i64, DATE->NaiveDate,
        // ...). We therefore probe the plausible representations in turn.
        let try_str = |index: usize| -> Option<String> {
            row.try_get::<&str, _>(index)
                .ok()
                .flatten()
                .map(String::from)
        };

        match col_type {
            ColumnType::Boolean => row
                .try_get::<bool, _>(col_index)
                .ok()
                .flatten()
                .map(|v| v.to_string()),
            ColumnType::Integer | ColumnType::UnsignedInteger => row
                .try_get::<i32, _>(col_index)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .or_else(|| {
                    row.try_get::<i64, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                })
                .or_else(|| {
                    row.try_get::<i16, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                })
                .or_else(|| {
                    row.try_get::<u8, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                }),
            ColumnType::Numeric => row
                .try_get::<rust_decimal::Decimal, _>(col_index)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .or_else(|| {
                    row.try_get::<f64, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                })
                .or_else(|| {
                    row.try_get::<f32, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                }),
            ColumnType::Uuid => row
                .try_get::<uuid::Uuid, _>(col_index)
                .ok()
                .flatten()
                .map(|v| v.to_string()),
            ColumnType::DateTime => row
                .try_get::<chrono::NaiveDateTime, _>(col_index)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .or_else(|| {
                    row.try_get::<chrono::NaiveDate, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                })
                .or_else(|| {
                    row.try_get::<chrono::NaiveTime, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                })
                .or_else(|| {
                    row.try_get::<chrono::DateTime<chrono::FixedOffset>, _>(col_index)
                        .ok()
                        .flatten()
                        .map(|v| v.to_string())
                }),
            ColumnType::Binary => row
                .try_get::<&[u8], _>(col_index)
                .ok()
                .flatten()
                .map(hex::encode),
            _ => try_str(col_index),
        }
    }

    pub fn map_mssql_type(type_name: &str) -> ColumnType {
        let lower = type_name.to_lowercase();
        let bare = lower.split('(').next().unwrap_or(&lower).trim();

        match bare {
            "bit" => ColumnType::Boolean,
            "tinyint" | "smallint" | "int" | "bigint" => ColumnType::Integer,
            "real" | "float" | "decimal" | "numeric" | "money" | "smallmoney" => {
                ColumnType::Numeric
            }
            "date" | "time" | "datetime" | "datetime2" | "smalldatetime" | "datetimeoffset" => {
                ColumnType::DateTime
            }
            "uniqueidentifier" => ColumnType::Uuid,
            "varbinary" | "binary" | "image" => ColumnType::Binary,
            "text" | "ntext" | "varchar" | "nvarchar" | "char" | "nchar" | "xml" => {
                ColumnType::Text
            }
            "json" => ColumnType::Json,
            _ => ColumnType::Unknown,
        }
    }

    fn map_tiberius_column_type(ct: tiberius::ColumnType) -> ColumnType {
        use tiberius::ColumnType as TC;
        match ct {
            TC::Bit | TC::Bitn => ColumnType::Boolean,
            TC::Int1 | TC::Int2 | TC::Int4 | TC::Int8 | TC::Intn => ColumnType::Integer,
            TC::Float4 | TC::Float8 | TC::Floatn | TC::Decimaln | TC::Numericn => {
                ColumnType::Numeric
            }
            TC::Money | TC::Money4 => ColumnType::Numeric,
            TC::Datetime4
            | TC::Datetime
            | TC::Datetimen
            | TC::Datetime2
            | TC::DatetimeOffsetn
            | TC::Daten
            | TC::Timen => ColumnType::DateTime,
            TC::Guid => ColumnType::Uuid,
            TC::BigVarChar
            | TC::BigChar
            | TC::NVarchar
            | TC::NChar
            | TC::Text
            | TC::NText
            | TC::Xml => ColumnType::Text,
            TC::BigVarBin | TC::BigBinary | TC::Image => ColumnType::Binary,
            TC::Udt | TC::SSVariant | TC::Null => ColumnType::Unknown,
        }
    }
}

impl Clone for MssqlConnection {
    fn clone(&self) -> Self {
        Self {
            clients: Arc::clone(&self.clients),
            server_key: self.server_key.clone(),
            password: self.password.clone(),
            display_name: self.display_name.clone(),
            initial_database: self.initial_database.clone(),
        }
    }
}

impl MssqlConnection {
    async fn fetch_inbound_foreign_keys(
        &self,
        database_name: Option<&str>,
        tables: &[TableSchemaInfo],
    ) -> Result<HashMap<(String, String), Vec<InboundForeignKey>>> {
        if tables.is_empty() {
            return Ok(HashMap::new());
        }

        let pairs: Vec<String> = tables
            .iter()
            .map(|t| {
                format!(
                    "('{}', '{}')",
                    t.schema.replace('\'', "''"),
                    t.name.replace('\'', "''"),
                )
            })
            .collect();
        let pair_list = pairs.join(",");

        let sql = format!(
            r#"
            WITH targets(schema_name, table_name) AS (
                SELECT v.schema_name, v.table_name FROM (VALUES {pair_list}) AS v(schema_name, table_name)
            )
            SELECT
                ts.name           AS target_schema,
                tt.name           AS target_table,
                tc.name           AS target_column,
                ss.name + '.' + st.name AS from_table,
                sc.name           AS from_column,
                fk.name           AS constraint_name
            FROM sys.foreign_keys fk
            JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
            JOIN sys.tables st  ON st.object_id = fkc.parent_object_id
            JOIN sys.schemas ss ON ss.schema_id = st.schema_id
            JOIN sys.columns sc ON sc.object_id = fkc.parent_object_id AND sc.column_id = fkc.parent_column_id
            JOIN sys.tables tt  ON tt.object_id = fkc.referenced_object_id
            JOIN sys.schemas ts ON ts.schema_id = tt.schema_id
            JOIN sys.columns tc ON tc.object_id = fkc.referenced_object_id AND tc.column_id = fkc.referenced_column_id
            JOIN targets t ON t.schema_name = ts.name AND t.table_name = tt.name
            ORDER BY ts.name, tt.name, ss.name, st.name, fkc.constraint_column_id
            "#
        );

        let result = self.execute_query(&sql, database_name, None).await?;
        let mut map: HashMap<(String, String), Vec<InboundForeignKey>> = HashMap::new();
        for row in result.rows {
            if row.len() >= 6 {
                let target_schema = row[0].clone().unwrap_or_default();
                let target_table = row[1].clone().unwrap_or_default();
                let target_column = row[2].clone().unwrap_or_default();
                let from_table = row[3].clone().unwrap_or_default();
                let from_column = row[4].clone().unwrap_or_default();
                let constraint_name = row[5].clone().filter(|s| !s.is_empty());
                map.entry((target_schema, target_table))
                    .or_default()
                    .push(InboundForeignKey {
                        from_table,
                        from_column,
                        to_column: target_column,
                        constraint_name,
                    });
            }
        }
        Ok(map)
    }
}

#[async_trait]
impl Connection for MssqlConnection {
    fn get_connection_type(&self) -> &'static str {
        "SQL Server"
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    fn supports_schemas(&self) -> bool {
        true
    }

    async fn connect(&mut self, _connection_string: &str) -> Result<()> {
        let mut client = self
            .establish(None)
            .await
            .map_err(|e| anyhow::anyhow!("MSSQL connection test failed: {e}"))?;
        client
            .simple_query("SELECT 1")
            .await
            .map_err(|e| anyhow::anyhow!("MSSQL connection test failed: {e}"))?;
        tracing::info!(
            "Connected to SQL Server at {}:{}",
            self.server_key.host,
            self.server_key.port
        );
        Ok(())
    }

    async fn ping(&self) -> Result<()> {
        // A throwaway connection surfaces the real login error immediately and
        // bounds the attempt with the configured connect timeout.
        let timeout = blanco_core::connect_timeout();

        let mut client = tokio::time::timeout(timeout, self.establish(None))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "SQL Server connection timed out after {}s",
                    timeout.as_secs()
                )
            })??;

        client
            .simple_query("SELECT 1")
            .await
            .map_err(|e| anyhow::anyhow!("SQL Server connection check failed: {e}"))?;

        Ok(())
    }

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        _parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let start = std::time::Instant::now();

        let slot = self.client_slot(database_name).await;
        let mut guard = slot.lock().await;
        if guard.is_none() {
            *guard = Some(self.establish(database_name).await?);
        }
        let client = guard.as_mut().expect("client established above");

        // `simple_query` + `into_results` both borrow the client; run them in one
        // block so only the owned result sets escape. On a transport error drop
        // the cached client so the next call reconnects.
        let result_sets = match async {
            let stream = client.simple_query(query).await?;
            stream.into_results().await
        }
        .await
        {
            Ok(result_sets) => result_sets,
            Err(error) => {
                if is_connection_lost(&error) {
                    *guard = None;
                }
                return Err(anyhow::anyhow!("MSSQL query error: {error}"));
            }
        };
        drop(guard);

        let mut columns: Vec<String> = Vec::new();
        let mut column_types: Vec<ColumnType> = Vec::new();
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();

        for result_set in &result_sets {
            if let Some(first_row) = result_set.first()
                && columns.is_empty()
            {
                for col in first_row.columns() {
                    columns.push(col.name().to_string());
                    column_types.push(Self::map_tiberius_column_type(col.column_type()));
                }
            }

            for row in result_set {
                let values: Vec<Option<String>> = column_types
                    .iter()
                    .enumerate()
                    .map(|(i, ct)| Self::column_value_to_string(row, i, ct))
                    .collect();
                rows.push(values);
            }
        }

        let rows_affected = rows.len() as u64;
        let query_text = query.to_string();

        Ok(QueryResult {
            columns,
            column_types,
            rows,
            rows_affected,
            query_text: Some(query_text),
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
        let slot = self.client_slot(database_name).await;
        let mut guard = slot.lock().await;
        if guard.is_none() {
            *guard = Some(self.establish(database_name).await?);
        }
        let client = guard.as_mut().expect("client established above");

        let result_sets = match async {
            let stream = client.simple_query(query).await?;
            stream.into_results().await
        }
        .await
        {
            Ok(result_sets) => result_sets,
            Err(error) => {
                if is_connection_lost(&error) {
                    *guard = None;
                }
                return Err(anyhow::anyhow!("MSSQL query error: {error}"));
            }
        };
        drop(guard);

        let mut out: Vec<QueryResult> = Vec::with_capacity(result_sets.len());
        for result_set in result_sets {
            let mut columns: Vec<String> = Vec::new();
            let mut column_types: Vec<ColumnType> = Vec::new();
            if let Some(first_row) = result_set.first() {
                for col in first_row.columns() {
                    columns.push(col.name().to_string());
                    column_types.push(Self::map_tiberius_column_type(col.column_type()));
                }
            }
            let mut rows: Vec<Vec<Option<String>>> = Vec::with_capacity(result_set.len());
            for row in &result_set {
                let values: Vec<Option<String>> = column_types
                    .iter()
                    .enumerate()
                    .map(|(i, ct)| Self::column_value_to_string(row, i, ct))
                    .collect();
                rows.push(values);
            }
            let rows_affected = rows.len() as u64;
            out.push(QueryResult {
                columns,
                column_types,
                rows,
                rows_affected,
                query_text: None,
                execution_time_ms: None,
                is_error: false,
                table_name: None,
                connection_id: None,
                table_columns: None,
            });
        }
        Ok(out)
    }

    async fn execute_write(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64> {
        let slot = self.client_slot(database_name).await;
        let mut guard = slot.lock().await;
        if guard.is_none() {
            *guard = Some(self.establish(database_name).await?);
        }
        let client = guard.as_mut().expect("client established above");

        let mut q = Query::new(query);
        for param in parameters {
            match param {
                Some(val) => q.bind(val.as_str()),
                None => q.bind(Option::<&str>::None),
            }
        }

        match q.execute(client).await {
            Ok(result) => Ok(result.total()),
            Err(error) => {
                if is_connection_lost(&error) {
                    *guard = None;
                }
                Err(anyhow::anyhow!("MSSQL write error: {error}"))
            }
        }
    }

    async fn execute_operations_transactional(
        &self,
        operations: &[String],
        database_name: Option<&str>,
    ) -> Result<blanco_core::BatchOutcome, blanco_core::BatchFailure> {
        use blanco_core::{BatchFailure, BatchOutcome};

        let slot = self.client_slot(database_name).await;
        let mut guard = slot.lock().await;
        if guard.is_none() {
            match self.establish(database_name).await {
                Ok(client) => *guard = Some(client),
                Err(error) => return Err(BatchFailure::atomic(error)),
            }
        }
        let client = guard.as_mut().expect("client established above");

        // Use `Query::execute` (which returns an owned `ExecuteResult`) for the
        // control statements too, rather than `simple_query` whose borrowed
        // stream would pin the `client` borrow across the whole block and clash
        // with the `*guard = None` reconnect handling.
        if let Err(error) = Query::new("BEGIN TRANSACTION").execute(client).await {
            let lost = is_connection_lost(&error);
            let failure = BatchFailure::atomic(anyhow::anyhow!("MSSQL begin error: {error}"));
            if lost {
                *guard = None;
            }
            return Err(failure);
        }

        let client = guard.as_mut().expect("client established above");
        let mut outcome = BatchOutcome::default();
        let mut failed: Option<(anyhow::Error, bool)> = None;
        for operation in operations {
            let query = Query::new(operation.as_str());
            match query.execute(client).await {
                Ok(result) => {
                    outcome.rows_affected += result.total();
                    outcome.operations_executed += 1;
                }
                Err(error) => {
                    let lost = is_connection_lost(&error);
                    failed = Some((anyhow::anyhow!("MSSQL operation error: {error}"), lost));
                    break;
                }
            }
        }

        if let Some((error, lost)) = failed {
            // Roll the whole batch back so nothing is applied. Skip it when the
            // connection is gone (the rollback would fail too); dropping the
            // client discards the aborted transaction.
            if !lost
                && let Err(rollback_error) =
                    Query::new("ROLLBACK TRANSACTION").execute(client).await
            {
                tracing::warn!("MSSQL rollback failed: {rollback_error}");
            }
            if lost {
                *guard = None;
            }
            return Err(BatchFailure::atomic(error));
        }

        let client = guard.as_mut().expect("client established above");
        if let Err(error) = Query::new("COMMIT TRANSACTION").execute(client).await {
            let lost = is_connection_lost(&error);
            let failure = BatchFailure::atomic(anyhow::anyhow!("MSSQL commit error: {error}"));
            if lost {
                *guard = None;
            }
            return Err(failure);
        }

        Ok(outcome)
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
        let result = self.execute_query(query, database_name, None).await?;
        let stream = futures::stream::iter(result.rows.into_iter().map(Ok));
        Ok((result.columns, result.column_types, Box::new(stream)))
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        let result = self
            .execute_query(
                "SELECT name FROM sys.databases WHERE database_id > 4 ORDER BY name",
                self.initial_database.as_deref(),
                None,
            )
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        let result = self
            .execute_query(
                "SELECT name FROM sys.schemas WHERE principal_id <> 16384 AND name NOT IN ('sys','INFORMATION_SCHEMA','guest') ORDER BY name",
                self.initial_database.as_deref(),
                None,
            )
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let sql = match schema {
            Some(s) => format!(
                "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE = 'BASE TABLE' AND TABLE_SCHEMA = '{}' ORDER BY TABLE_NAME",
                s.replace('\'', "''")
            ),
            None => String::from(
                "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME",
            ),
        };
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let sql = match schema {
            Some(s) => format!(
                "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA = '{}' ORDER BY TABLE_NAME",
                s.replace('\'', "''")
            ),
            None => {
                String::from("SELECT TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS ORDER BY TABLE_NAME")
            }
        };
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|mut row| row.pop().flatten())
            .collect())
    }

    async fn get_materialized_views(&self, _schema: Option<&str>) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn get_queryable_entities(&self, schema: Option<&str>) -> Result<Vec<QueryableEntity>> {
        let mut entities = Vec::new();
        for table in self.get_tables(schema).await? {
            entities.push(QueryableEntity {
                name: table,
                entity_type: EntityType::Table,
            });
        }
        for view in self.get_views(schema).await? {
            entities.push(QueryableEntity {
                name: view,
                entity_type: EntityType::View,
            });
        }
        entities.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entities)
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        let sql = schema::columns_for_table_sql(table_name, schema);
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| {
                let get = |i: usize| -> Option<String> { row.get(i).cloned().flatten() };
                let foreign_key = ForeignKeyInfo::from_parts(get(6), get(7), get(8));
                ColumnInfo {
                    name: get(0).unwrap_or_default(),
                    data_type: get(1).unwrap_or_default(),
                    is_nullable: get(2).map(|v| v == "YES").unwrap_or(true),
                    is_primary_key: get(3).map(|v| v == "1").unwrap_or(false),
                    default_value: get(4).filter(|v| !v.is_empty()),
                    character_maximum_length: get(5).and_then(|v| v.parse().ok()),
                    foreign_key,
                }
            })
            .collect())
    }

    async fn get_indexes_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>> {
        let sql = schema::indexes_for_table_sql(table_name, schema);
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| {
                let get = |i: usize| -> Option<String> { row.get(i).cloned().flatten() };
                let name = get(0).unwrap_or_default();
                let algorithm = get(1).unwrap_or_else(|| "BTREE".into());
                let is_unique = get(2).map(|v| v == "1").unwrap_or(false);
                let column_names_str = get(3).unwrap_or_default();
                IndexInfo {
                    name,
                    algorithm,
                    is_unique,
                    column_names: if column_names_str.is_empty() {
                        Vec::new()
                    } else {
                        column_names_str.split(',').map(String::from).collect()
                    },
                    condition: None,
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
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);

        let sql = schema::database_schema_paginated_sql(table_names, limit, offset);
        let result = self.execute_query(&sql, database_name, None).await?;

        let has_more = result.rows.len() > limit as usize;
        let table_rows: Vec<_> = if has_more {
            result.rows.into_iter().take(limit as usize).collect()
        } else {
            result.rows
        };

        let mut tables = Vec::new();
        for row in table_rows {
            let get = |i: usize| row.get(i).cloned().flatten();
            let name = get(0).unwrap_or_default();
            let schema_name = get(1).unwrap_or_else(|| "dbo".into());
            let object_type = get(2).unwrap_or_else(|| "TABLE".into());

            let columns = self
                .get_columns_for_table(&name, Some(&schema_name))
                .await?;
            let column_count = columns.len();
            tables.push(TableSchemaInfo {
                name,
                schema: schema_name,
                object_type,
                columns,
                column_count,
                referenced_by: Vec::new(),
            });
        }

        if !tables.is_empty() {
            let inbound = self
                .fetch_inbound_foreign_keys(database_name, &tables)
                .await?;
            for table in &mut tables {
                if let Some(list) = inbound.get(&(table.schema.clone(), table.name.clone())) {
                    table.referenced_by = list.clone();
                }
            }
        }

        Ok(DatabaseSchemaResult {
            connection_type: "SQL Server".to_string(),
            display_name: self.display_name.clone(),
            tables,
            pagination: PaginationInfo {
                limit: Some(limit),
                offset: Some(offset),
                has_more,
            },
        })
    }

    async fn list_procedures(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let schema_filter = schema.unwrap_or("dbo").replace('\'', "''");
        let sql = format!(
            "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES \
             WHERE ROUTINE_TYPE = 'PROCEDURE' AND ROUTINE_SCHEMA = '{schema_filter}' \
             ORDER BY ROUTINE_NAME"
        );
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect())
    }

    async fn list_functions(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let schema_filter = schema.unwrap_or("dbo").replace('\'', "''");
        let sql = format!(
            "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES \
             WHERE ROUTINE_TYPE = 'FUNCTION' AND ROUTINE_SCHEMA = '{schema_filter}' \
             ORDER BY ROUTINE_NAME"
        );
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect())
    }

    async fn list_triggers(&self, schema: Option<&str>) -> Result<Vec<String>> {
        // DML triggers belong to a parent table/view; resolve the parent's
        // schema. `is_ms_shipped` filters out system triggers.
        let schema_filter = schema.unwrap_or("dbo").replace('\'', "''");
        let sql = format!(
            "SELECT tr.name \
             FROM sys.triggers tr \
             JOIN sys.objects o ON o.object_id = tr.parent_id \
             JOIN sys.schemas s ON s.schema_id = o.schema_id \
             WHERE s.name = '{schema_filter}' AND tr.is_ms_shipped = 0 \
             ORDER BY tr.name"
        );
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .collect())
    }

    async fn object_ddl(
        &self,
        kind: RoutineKind,
        schema: Option<&str>,
        name: &str,
    ) -> Result<String> {
        // `OBJECT_DEFINITION` returns the original CREATE text for procedures,
        // functions and triggers alike, so one query covers every kind.
        let schema_name = schema.unwrap_or("dbo");
        let qualified = format!(
            "[{}].[{}]",
            schema_name.replace(']', "]]"),
            name.replace(']', "]]")
        );
        let sql = format!(
            "SELECT OBJECT_DEFINITION(OBJECT_ID('{}'))",
            qualified.replace('\'', "''")
        );
        let result = self
            .execute_query(&sql, self.initial_database.as_deref(), None)
            .await?;
        result
            .rows
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next().flatten())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{} '{}.{}' not found",
                    kind.display_name(),
                    schema_name,
                    name
                )
            })
    }

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult> {
        let quote_identifier = |identifier: &str| {
            identifier
                .split('.')
                .map(|part| {
                    let part = part.trim();
                    let unquoted = part
                        .strip_prefix('[')
                        .and_then(|value| value.strip_suffix(']'))
                        .unwrap_or(part);
                    format!("[{}]", unquoted.replace(']', "]]"))
                })
                .collect::<Vec<_>>()
                .join(".")
        };
        let sql = format!(
            "SELECT TOP 100 * FROM {} WHERE {} = '{}'",
            quote_identifier(table_name),
            quote_identifier(column_name),
            reference_value.replace('\'', "''")
        );
        self.execute_query(&sql, None, None).await
    }
}
