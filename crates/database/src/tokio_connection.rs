//! Wrapper around `Arc<dyn Connection>` that drives every backend call on the
//! shared tokio runtime. GPUI runs on smol; database drivers (sqlx with
//! `runtime-tokio`, reqwest, tiberius, ...) require a tokio reactor on the
//! polling thread, so we hop onto the tokio runtime via `Handle::spawn` and
//! await the resulting `JoinHandle` from smol.
//!
//! All cached connections handed out by `DatabaseService` are wrapped in this
//! type, so UI code can keep calling `connection.execute_query(...)` from a
//! `cx.background_spawn` task without panicking.
//!
//! `connect` is intentionally not implemented: connections are connected by
//! their factory before being wrapped, and the wrapper is then handed out as
//! immutable shared state.
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use blanco_core::{
    connection_trait::{ColumnType, DatabaseSchemaResult, IndexInfo, QueryableEntity, RoutineKind},
    ColumnInfo, Connection, QueryResult,
};
use futures::Stream;
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::Handle;

pub struct TokioConnection {
    inner: Arc<dyn Connection>,
    runtime: Handle,
}

impl TokioConnection {
    pub fn new(inner: Arc<dyn Connection>, runtime: Handle) -> Self {
        Self { inner, runtime }
    }

    async fn run<F, T>(&self, fut: F) -> Result<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        self.runtime
            .spawn(fut)
            .await
            .map_err(|e| anyhow!("tokio task join failed: {e}"))?
    }
}

#[async_trait]
impl Connection for TokioConnection {
    fn get_connection_type(&self) -> &'static str {
        self.inner.get_connection_type()
    }

    fn get_display_name(&self) -> String {
        self.inner.get_display_name()
    }

    fn supports_schemas(&self) -> bool {
        self.inner.supports_schemas()
    }

    fn extract_table_name_from_query(&self, query: &str, alias: bool) -> Result<Option<String>> {
        self.inner.extract_table_name_from_query(query, alias)
    }

    async fn connect(&mut self, _connection_string: &str) -> Result<()> {
        Err(anyhow!(
            "TokioConnection::connect must not be called; connections are connected by their factory before being wrapped"
        ))
    }

    async fn execute_query(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: Option<&[String]>,
    ) -> Result<QueryResult> {
        let inner = Arc::clone(&self.inner);
        let query = query.to_string();
        let database_name = database_name.map(str::to_string);
        let parameters = parameters.map(|p| p.to_vec());
        self.run(async move {
            inner
                .execute_query(&query, database_name.as_deref(), parameters.as_deref())
                .await
        })
        .await
    }

    async fn execute_script(
        &self,
        query: &str,
        database_name: Option<&str>,
    ) -> Result<Vec<QueryResult>> {
        let inner = Arc::clone(&self.inner);
        let query = query.to_string();
        let database_name = database_name.map(str::to_string);
        self.run(async move { inner.execute_script(&query, database_name.as_deref()).await })
            .await
    }

    async fn execute_write(
        &self,
        query: &str,
        database_name: Option<&str>,
        parameters: &[Option<String>],
    ) -> Result<u64> {
        let inner = Arc::clone(&self.inner);
        let query = query.to_string();
        let database_name = database_name.map(str::to_string);
        let parameters = parameters.to_vec();
        self.run(async move {
            inner
                .execute_write(&query, database_name.as_deref(), &parameters)
                .await
        })
        .await
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
        // Existing backends (sqlite, postgres, clickhouse) eagerly collect
        // rows inside `execute_query_stream_rows` and return a
        // `futures::stream::iter` whose items do no further driver IO, so
        // returning the stream as-is from the spawned tokio task is safe.
        // If a future backend ever returns a stream that polls live IO, that
        // backend should buffer rows into a runtime-agnostic channel here.
        let inner = Arc::clone(&self.inner);
        let query = query.to_string();
        let database_name = database_name.map(str::to_string);
        self.runtime
            .spawn(async move {
                inner
                    .execute_query_stream_rows(&query, database_name.as_deref())
                    .await
            })
            .await
            .map_err(|e| anyhow!("tokio task join failed: {e}"))?
    }

    async fn get_databases(&self) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        self.run(async move { inner.get_databases().await }).await
    }

    async fn get_schemas(&self) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        self.run(async move { inner.get_schemas().await }).await
    }

    async fn get_tables(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.get_tables(schema.as_deref()).await })
            .await
    }

    async fn get_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.get_views(schema.as_deref()).await })
            .await
    }

    async fn get_materialized_views(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.get_materialized_views(schema.as_deref()).await })
            .await
    }

    async fn list_procedures(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.list_procedures(schema.as_deref()).await })
            .await
    }

    async fn list_functions(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.list_functions(schema.as_deref()).await })
            .await
    }

    async fn list_triggers(&self, schema: Option<&str>) -> Result<Vec<String>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.list_triggers(schema.as_deref()).await })
            .await
    }

    async fn get_object_sizes(
        &self,
        schema: Option<&str>,
    ) -> Result<std::collections::HashMap<String, u64>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.get_object_sizes(schema.as_deref()).await })
            .await
    }

    async fn object_ddl(
        &self,
        kind: RoutineKind,
        schema: Option<&str>,
        name: &str,
    ) -> Result<String> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        let name = name.to_string();
        self.run(async move { inner.object_ddl(kind, schema.as_deref(), &name).await })
            .await
    }

    async fn get_queryable_entities(&self, schema: Option<&str>) -> Result<Vec<QueryableEntity>> {
        let inner = Arc::clone(&self.inner);
        let schema = schema.map(str::to_string);
        self.run(async move { inner.get_queryable_entities(schema.as_deref()).await })
            .await
    }

    async fn get_columns_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>> {
        let inner = Arc::clone(&self.inner);
        let table_name = table_name.to_string();
        let schema = schema.map(str::to_string);
        self.run(async move {
            inner
                .get_columns_for_table(&table_name, schema.as_deref())
                .await
        })
        .await
    }

    async fn get_indexes_for_table(
        &self,
        table_name: &str,
        schema: Option<&str>,
    ) -> Result<Vec<IndexInfo>> {
        let inner = Arc::clone(&self.inner);
        let table_name = table_name.to_string();
        let schema = schema.map(str::to_string);
        self.run(async move {
            inner
                .get_indexes_for_table(&table_name, schema.as_deref())
                .await
        })
        .await
    }

    async fn get_database_schema_paginated(
        &self,
        database_name: Option<&str>,
        table_names: Option<&str>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<DatabaseSchemaResult> {
        let inner = Arc::clone(&self.inner);
        let database_name = database_name.map(str::to_string);
        let table_names = table_names.map(str::to_string);
        self.run(async move {
            inner
                .get_database_schema_paginated(
                    database_name.as_deref(),
                    table_names.as_deref(),
                    limit,
                    offset,
                )
                .await
        })
        .await
    }

    async fn foreign_key_lookup(
        &self,
        table_name: &str,
        column_name: &str,
        reference_value: &str,
    ) -> Result<QueryResult> {
        let inner = Arc::clone(&self.inner);
        let table_name = table_name.to_string();
        let column_name = column_name.to_string();
        let reference_value = reference_value.to_string();
        self.run(async move {
            inner
                .foreign_key_lookup(&table_name, &column_name, &reference_value)
                .await
        })
        .await
    }
}
