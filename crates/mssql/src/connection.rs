use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, QueryResult,
    connection_trait::{
        ColumnType, DatabaseSchemaResult, EntityType, ForeignKeyInfo, IndexInfo, PaginationInfo,
        QueryableEntity, TableSchemaInfo,
    },
};
use futures::Stream;
use std::collections::HashMap;
use std::sync::Arc;
use tiberius::{AuthMethod, Config as TiberiusConfig, Query, Row};
use tokio::sync::RwLock;

use crate::schema;

type MssqlPool = bb8::Pool<bb8_tiberius::ConnectionManager>;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MssqlServerKey {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub trust_cert: bool,
    pub encrypt: bool,
}

pub struct MssqlConnection {
    pools: Arc<RwLock<HashMap<String, MssqlPool>>>,
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
            pools: Arc::new(RwLock::new(HashMap::new())),
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

    async fn get_or_create_pool(&self, database: Option<&str>) -> Result<MssqlPool> {
        let db = self.resolve_database(database);

        {
            let pools = self.pools.read().await;
            if let Some(pool) = pools.get(&db) {
                return Ok(pool.clone());
            }
        }

        let mut pools = self.pools.write().await;
        if let Some(pool) = pools.get(&db) {
            return Ok(pool.clone());
        }

        let config = self.build_tiberius_config(Some(&db));
        let manager = bb8_tiberius::ConnectionManager::new(config);
        let pool = bb8::Pool::builder()
            .max_size(1)
            .build(manager)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create MSSQL pool for '{db}': {e}"))?;

        pools.insert(db, pool.clone());
        Ok(pool)
    }

    fn column_value_to_string(
        row: &Row,
        col_index: usize,
        col_type: &ColumnType,
    ) -> Option<String> {
        match col_type {
            ColumnType::Boolean => row.get::<bool, _>(col_index).map(|v| v.to_string()),
            ColumnType::Integer => row.get::<i64, _>(col_index).map(|v| v.to_string()),
            ColumnType::UnsignedInteger => row.get::<i64, _>(col_index).map(|v| v.to_string()),
            ColumnType::Numeric => {
                if let Some(val) = row.get::<rust_decimal::Decimal, _>(col_index) {
                    return Some(val.to_string());
                }
                row.get::<f64, _>(col_index).map(|v| v.to_string())
            }
            ColumnType::Uuid => row.get::<uuid::Uuid, _>(col_index).map(|v| v.to_string()),
            ColumnType::DateTime => {
                if let Some(val) = row.get::<chrono::NaiveDateTime, _>(col_index) {
                    return Some(val.to_string());
                }
                if let Some(val) = row.get::<chrono::NaiveDate, _>(col_index) {
                    return Some(val.to_string());
                }
                if let Some(val) = row.get::<chrono::NaiveTime, _>(col_index) {
                    return Some(val.to_string());
                }
                row.get::<chrono::DateTime<chrono::FixedOffset>, _>(col_index)
                    .map(|v| v.to_string())
            }
            ColumnType::Binary => row
                .try_get::<&[u8], _>(col_index)
                .ok()
                .flatten()
                .map(hex::encode),
            _ => row
                .try_get::<&str, usize>(col_index)
                .ok()
                .flatten()
                .map(|v| v.to_string()),
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
            pools: Arc::clone(&self.pools),
            server_key: self.server_key.clone(),
            password: self.password.clone(),
            display_name: self.display_name.clone(),
            initial_database: self.initial_database.clone(),
        }
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
        let pool = self.get_or_create_pool(None).await?;
        let mut client = pool
            .get()
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

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        _parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let start = std::time::Instant::now();

        let pool = self.get_or_create_pool(database_name).await?;
        let mut client = pool
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("MSSQL connection failed: {e}"))?;

        let stream = client
            .simple_query(query)
            .await
            .map_err(|e| anyhow::anyhow!("MSSQL query error: {e}"))?;
        let result_sets = stream
            .into_results()
            .await
            .map_err(|e| anyhow::anyhow!("MSSQL result fetch error: {e}"))?;

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

    async fn execute_write(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64> {
        let pool = self.get_or_create_pool(database_name).await?;
        let mut client = pool
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("MSSQL connection failed: {e}"))?;

        let mut q = Query::new(query);
        for param in parameters {
            match param {
                Some(val) => q.bind(val.as_str()),
                None => q.bind(Option::<&str>::None),
            }
        }

        let result = q
            .execute(&mut client)
            .await
            .map_err(|e| anyhow::anyhow!("MSSQL write error: {e}"))?;

        Ok(result.total() as u64)
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
        let result = self.execute_query(&sql, self.initial_database.as_deref(), None).await?;
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
        let result = self.execute_query(&sql, self.initial_database.as_deref(), None).await?;
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
        let result = self.execute_query(&sql, self.initial_database.as_deref(), None).await?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| {
                let get = |i: usize| -> Option<String> { row.get(i).cloned().flatten() };
                let fk_table = get(6);
                let fk_col = get(7);
                let fk_name = get(8);
                let foreign_key = fk_table.and_then(|table| {
                    fk_col.map(|col| ForeignKeyInfo {
                        foreign_table_name: table,
                        foreign_column_name: col,
                        constraint_name: fk_name,
                    })
                });
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
        let result = self.execute_query(&sql, self.initial_database.as_deref(), None).await?;
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
            });
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

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult> {
        let sql = format!(
            "SELECT TOP 100 * FROM [{}] WHERE [{}] = '{}'",
            table_name.replace(']', "]]"),
            column_name.replace(']', "]]"),
            reference_value.replace('\'', "''")
        );
        self.execute_query(&sql, None, None).await
    }
}
