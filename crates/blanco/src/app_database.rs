mod connections;
mod query_history;
mod query_tabs;
mod schema;
mod settings;
mod snippets;
mod types;

pub use schema::{app_db_path, init_schema};
pub use types::{ConnectionData, EnvironmentType, QueryHistoryData, QueryTabData, SnippetData};

use gpui::{App, Global};
use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use std::future::Future;
use std::str::FromStr;
use tokio::runtime::Handle;

/// Application database for persisting query tabs, history, and connections.
///
/// sqlx is built with the `runtime-tokio` feature, so all of its work must be
/// driven on the shared tokio runtime, GPUI's smol executor would otherwise
/// panic with "this functionality requires a Tokio context". `run` shuttles
/// each call onto the runtime via `Handle::spawn` so that callers from
/// `cx.spawn` / `cx.background_spawn` (smol) keep working unchanged.
#[derive(Clone)]
pub struct AppDatabase {
    pool: SqlitePool,
    runtime: Handle,
}

impl Global for AppDatabase {}

impl AppDatabase {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub(crate) fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    pub(crate) async fn run<F, T>(&self, fut: F) -> Result<T, sqlx::Error>
    where
        F: Future<Output = Result<T, sqlx::Error>> + Send + 'static,
        T: Send + 'static,
    {
        match self.runtime.spawn(fut).await {
            Ok(result) => result,
            Err(e) => Err(sqlx::Error::Configuration(
                format!("tokio task join failed: {e}").into(),
            )),
        }
    }
}

impl AppDatabase {
    pub async fn new(runtime: Handle) -> Result<Self, sqlx::Error> {
        let db_path = app_db_path();

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = runtime
            .spawn(async move { SqlitePool::connect_with(options).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;
        let pool_for_init = pool.clone();
        runtime
            .spawn(async move { init_schema(&pool_for_init).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;

        Ok(Self { pool, runtime })
    }

    #[cfg(test)]
    pub async fn new_in_memory(runtime: Handle) -> Result<Self, sqlx::Error> {
        let options =
            SqliteConnectOptions::from_str("sqlite::memory:")?.disable_statement_logging();

        let pool = runtime
            .spawn(async move { SqlitePool::connect_with(options).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;
        let pool_for_init = pool.clone();
        runtime
            .spawn(async move { init_schema(&pool_for_init).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;

        Ok(Self { pool, runtime })
    }
}
