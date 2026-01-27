mod connections;
mod schema;
mod settings;
mod snippets;
mod types;
mod query_tabs;

pub use schema::{app_db_path, init_schema};
pub use types::{ConnectionData, EnvironmentType, QueryTabData, SnippetData};

use gpui::{App, Global};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::ConnectOptions;
use std::str::FromStr;

/// Application database for persisting query tabs, history, and connections
#[derive(Clone)]
pub struct AppDatabase {
    pool: SqlitePool,
}

impl Global for AppDatabase {}

impl AppDatabase {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Get access to the underlying database pool
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

impl AppDatabase {
    pub async fn new() -> Result<Self, sqlx::Error> {
        let db_path = crate::app_database::app_db_path();

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;

        let db = Self { pool };
        crate::app_database::init_schema(&db.pool).await?;

        Ok(db)
    }
}
