use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{
    ColumnInfo, Connection, QueryResult, connection_trait::ColumnType,
    connection_trait::ForeignKeyInfo, connection_trait::IndexInfo,
};
use futures::{Stream, StreamExt};
use sqlx::{Column, Row, TypeInfo};
use std::collections::HashMap;

use super::{SqliteConnection, SqliteConnectionKey, convert_sqlite_row_value_to_string};

#[async_trait]
impl Connection for SqliteConnection {
    fn get_connection_type(&self) -> &'static str {
        "SQLite"
    }

    fn get_display_name(&self) -> String {
        self.display_name.clone()
    }

    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        // Parse and validate the connection string
        let key = SqliteConnectionKey::from_connection_string(connection_string)?;
        self.connection_key = key.clone();
        self.database_path = key.database_path.clone();
        self.display_name = Self::generate_display_name(&key.database_path);

        // Create the database directory if it doesn't exist
        if let Some(parent) = std::path::Path::new(&self.database_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Connect using SQLX directly
        let database_path = self.database_path.clone();
        let pool = self.connect_async(&database_path).await?;
        self.pool = Some(pool);

        Ok(())
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        // SQLite has a single database, so we return the current database name
        let db_name = std::path::Path::new(&self.database_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();

        Ok(vec![db_name])
    }

    async fn execute_query(
        &self,
        query: &str,
        _database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        // SQLite only has one database, so we ignore the database parameter and execute normally
        self.execute_query_async(query, parameters).await
    }

    async fn execute_script(
        &self,
        query: &str,
        _database_name: Option<&str>,
    ) -> Result<Vec<QueryResult>> {
        self.execute_script_async(query).await
    }

    async fn execute_write(
        &self,
        query: &str,
        _database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Not connected to database"))?;

        let mut q = sqlx::query(query);
        for param in parameters {
            q = match param {
                Some(value) => q.bind(value),
                None => q.bind(Option::<String>::None),
            };
        }
        let result = q.execute(pool).await?;
        Ok(result.rows_affected())
    }

    async fn execute_operations_transactional(
        &self,
        operations: &[String],
        _database_name: Option<&str>,
    ) -> Result<blanco_core::BatchOutcome, blanco_core::BatchFailure> {
        let pool = self.pool.as_ref().ok_or_else(|| {
            blanco_core::BatchFailure::atomic(anyhow::anyhow!("Not connected to database"))
        })?;
        blanco_core::run_sqlx_transaction!(pool, operations)
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        // SQLite has a main schema by default, plus any attached databases
        let mut schemas = vec!["main".to_string()];

        // Try to get attached databases
        match self.execute_query("PRAGMA database_list", None, None).await {
            Ok(result) => {
                for row in &result.rows {
                    if let Some(Some(schema_name)) = row.get(1)
                        && schema_name != "main"
                        && !schemas.contains(schema_name)
                    {
                        schemas.push(schema_name.clone());
                    }
                }
            }
            Err(e) => {
                tracing::debug!("Could not get database list: {}", e);
            }
        }

        Ok(schemas)
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let schema_filter = schema.unwrap_or("main");
        let query = format!(
            "SELECT name FROM {}.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            schema_filter
        );

        let result = self.execute_query(&query, None, None).await?;
        let tables: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect();

        tracing::debug!(
            "Found {} tables in schema '{}'",
            tables.len(),
            schema_filter
        );
        Ok(tables)
    }

    async fn get_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let schema_filter = schema.unwrap_or("main");
        let query = format!(
            "SELECT name FROM {}.sqlite_master WHERE type='view' ORDER BY name",
            schema_filter
        );

        let result = self.execute_query(&query, None, None).await?;
        let views: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect();

        Ok(views)
    }

    async fn get_object_sizes(
        &self,
        schema: Option<&str>,
    ) -> Result<std::collections::HashMap<String, u64>> {
        let schema_filter = schema.unwrap_or("main");
        // dbstat is a virtual table built into the default SQLite amalgamation;
        // it exposes per-page byte usage for each table/index. If it's not
        // available (older builds, or when the dbstat extension wasn't
        // compiled in), fall through silently to an empty result.
        let query = format!(
            "SELECT name, SUM(pgsize) AS size_bytes
             FROM {}.dbstat
             GROUP BY name",
            schema_filter
        );
        let result = match self.execute_query(&query, None, None).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("dbstat unavailable, skipping table sizes: {}", e);
                return Ok(std::collections::HashMap::new());
            }
        };
        let mut sizes = std::collections::HashMap::new();
        for row in result.rows {
            let mut cells = row.into_iter();
            let Some(name) = cells.next().flatten() else {
                continue;
            };
            if let Some(size_str) = cells.next().flatten()
                && let Ok(size) = size_str.parse::<i64>()
                && size > 0
            {
                sizes.insert(name, size as u64);
            }
        }
        Ok(sizes)
    }

    async fn get_queryable_entities(
        &self,
        schema: Option<&str>,
    ) -> Result<Vec<blanco_core::QueryableEntity>, anyhow::Error> {
        let schema_filter = schema.unwrap_or("main");
        let query = format!(
            "SELECT name, \
                CASE WHEN type = 'view' THEN 'VIEW' ELSE 'TABLE' END as entity_type \
             FROM {}.sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
            schema_filter
        );

        let result = self.execute_query(&query, None, None).await?;

        use blanco_core::{EntityType, QueryableEntity};
        let entities: Vec<QueryableEntity> = result
            .rows
            .into_iter()
            .filter_map(|row| {
                if row.len() >= 2 {
                    let name = row[0].clone()?;
                    let entity_type_str = row[1].as_deref().unwrap_or("");
                    let entity_type = match entity_type_str {
                        "VIEW" => EntityType::View,
                        _ => EntityType::Table,
                    };
                    Some(QueryableEntity { name, entity_type })
                } else {
                    None
                }
            })
            .collect();

        Ok(entities)
    }

    fn supports_schemas(&self) -> bool {
        false // SQLite doesn't support schemas in the traditional sense
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        tracing::debug!("Getting columns for SQLite table '{}'", table_name);

        // Fetch column info with PK data
        let query = format!("PRAGMA table_info({})", table_name);
        let result = self.execute_query(&query, None, None).await?;

        // Fetch foreign key info in parallel
        let fk_query = format!("PRAGMA foreign_key_list({})", table_name);
        let fk_result = self.execute_query(&fk_query, None, None).await?;

        // Build FK lookup map
        let mut foreign_keys: HashMap<String, ForeignKeyInfo> = HashMap::new();
        for row in fk_result.rows {
            if row.len() >= 5
                && let (Some(Some(from_column)), Some(Some(to_table)), Some(Some(to_column))) =
                    (row.get(3), row.get(2), row.get(4))
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

        let mut columns = Vec::new();
        for row in result.rows {
            if row.len() >= 6 {
                // PRAGMA table_info returns: cid, name, type, notnull, dflt_value, pk
                let column_name = row[1].clone().unwrap_or_default();
                let data_type = row[2].clone().unwrap_or_default();
                let not_null = row[3].as_deref().unwrap_or(""); // 1 for NOT NULL, 0 for nullable
                let default_value = row[4].clone(); // Default value or None
                let is_primary_key = row[5].as_deref().unwrap_or(""); // 1 for PK, 0 for not PK

                let column_info = ColumnInfo {
                    name: column_name.clone(),
                    data_type,
                    is_nullable: not_null != "1", // Reverse logic: notnull=1 means NOT NULL
                    is_primary_key: is_primary_key == "1",
                    default_value: default_value.filter(|v| !v.is_empty()),
                    character_maximum_length: None, // SQLite doesn't provide this info in PRAGMA
                    foreign_key: foreign_keys.get(&column_name).cloned(),
                };
                columns.push(column_info);
            }
        }

        tracing::debug!("Found {} columns for table '{}'", columns.len(), table_name);
        Ok(columns)
    }

    async fn table_ddl(&self, _schema: Option<&str>, table_name: &str) -> Result<String> {
        // The table's own CREATE statement and its index CREATE statements all
        // live in sqlite_master. Ordering by `type = 'index'` keeps the table
        // definition first, followed by its indexes.
        let query = "SELECT sql FROM sqlite_master \
             WHERE tbl_name = ?1 AND sql IS NOT NULL \
             ORDER BY type = 'index'";
        let result = self
            .execute_query(query, None, Some(&[table_name.to_string()]))
            .await?;

        let statements: Vec<String> = result
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect();

        if statements.is_empty() {
            return Err(anyhow::anyhow!("Table '{}' not found", table_name));
        }

        Ok(statements.join(";\n"))
    }

    async fn execute_query_stream_rows(
        &self,
        query: &str,
        _database_name: Option<&str>, // SQLite doesn't support multiple databases
    ) -> Result<
        (
            Vec<String>,
            Vec<ColumnType>,
            Box<dyn Stream<Item = Result<Vec<Option<String>>, anyhow::Error>> + Send + Unpin>,
        ),
        anyhow::Error,
    > {
        tracing::debug!("Executing SQLite streaming query: {}", query);

        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("SQLite connection not established"))?;

        // Use sqlx::query().fetch() for true streaming
        let rows_stream = sqlx::query(query).fetch(pool);

        // We need to collect all rows first to get column information since sqlx streams
        // don't allow us to peek at the first row without consuming it
        let mut rows = Vec::new();
        let mut stream = rows_stream;
        let mut columns = Vec::new();
        let mut column_types = Vec::new();
        let mut raw_column_types = Vec::new();

        // Process the first row to get column information
        let mut first_row_data = None;
        while let Some(row_result) = stream.next().await {
            let row = row_result.map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?;

            if first_row_data.is_none() {
                // Extract column names and types from the first row
                columns = row
                    .columns()
                    .iter()
                    .map(|col| col.name().to_string())
                    .collect();

                let (types, raw_types): (Vec<ColumnType>, Vec<String>) = row
                    .columns()
                    .iter()
                    .map(|col| {
                        let raw_type = col.type_info().name().to_string();
                        (Self::map_sqlite_type(&raw_type), raw_type)
                    })
                    .unzip();
                column_types = types;
                raw_column_types = raw_types;

                let row_data: Vec<Option<String>> = (0..columns.len())
                    .map(|i| {
                        convert_sqlite_row_value_to_string(
                            &row,
                            i,
                            &column_types,
                            &raw_column_types,
                        )
                    })
                    .collect();

                first_row_data = Some(row_data.clone());
                rows.push(row_data);
            } else {
                // Process subsequent rows
                let row_data: Vec<Option<String>> = (0..columns.len())
                    .map(|i| {
                        convert_sqlite_row_value_to_string(
                            &row,
                            i,
                            &column_types,
                            &raw_column_types,
                        )
                    })
                    .collect();
                rows.push(row_data);
            }
        }

        if rows.is_empty() {
            return Ok((vec![], vec![], Box::new(futures::stream::empty())));
        }

        // Create stream from collected rows
        let all_rows_stream = futures::stream::iter(rows.into_iter().map(Ok));

        Ok((columns, column_types, Box::new(all_rows_stream)))
    }

    async fn get_database_schema_paginated(
        &self,
        _database_name: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<blanco_core::DatabaseSchemaResult> {
        let limit = limit.unwrap_or(20).min(100); // Default 20, max 100
        let offset = offset.unwrap_or(0);

        let tables = self
            .get_schema_paginated(table_names, limit, offset)
            .await?;
        let table_count = tables.len();

        Ok(blanco_core::DatabaseSchemaResult {
            connection_type: self.get_connection_type().to_string(),
            display_name: self.get_display_name(),
            tables,
            pagination: blanco_core::PaginationInfo {
                limit: Some(limit),
                offset: Some(offset),
                has_more: table_count == limit as usize,
            },
        })
    }

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult, anyhow::Error> {
        // SQLite doesn't provide table statistics, so always fetch only the referenced row
        let query = format!(
            "SELECT * FROM {} WHERE {} = '{}'",
            table_name, column_name, reference_value
        );
        self.execute_query(&query, None, None).await
    }

    async fn get_indexes_for_table(
        &self,
        table_name: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>, anyhow::Error> {
        tracing::debug!("Getting indexes for SQLite table '{}'", table_name);

        // Get list of indexes for the table
        // PRAGMA index_list returns: seq, name, unique, origin, partial
        let list_query = format!("PRAGMA index_list({})", table_name);
        let list_result = self.execute_query(&list_query, None, None).await?;

        let mut indexes = Vec::new();
        for row in list_result.rows {
            if row.len() >= 3 {
                let index_name = row[1].clone().unwrap_or_default();
                let is_unique = row[2].as_deref() == Some("1");

                // Get columns for this index
                // PRAGMA index_info returns: seqno, cid, name
                let info_query = format!("PRAGMA index_info({})", index_name);
                let info_result = self.execute_query(&info_query, None, None).await?;

                let column_names: Vec<String> = info_result
                    .rows
                    .iter()
                    .filter_map(|r| r.get(2).and_then(|v| v.clone()))
                    .collect();

                // SQLite uses B-tree for all indexes
                let index_info = IndexInfo {
                    name: index_name,
                    algorithm: "btree".to_string(),
                    is_unique,
                    column_names,
                    condition: None, // SQLite partial index WHERE clause not easily accessible
                    comment: None,
                };
                indexes.push(index_info);
            }
        }

        tracing::debug!("Found {} indexes for table '{}'", indexes.len(), table_name);
        Ok(indexes)
    }
}
