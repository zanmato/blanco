use crate::app_database::AppDatabase;
use crate::database::DatabaseManager;
use gpui::{App, Global};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::runtime::Runtime;

/// Global database service that holds both the app database and user database manager
#[derive(Clone)]
pub struct DbService {
    pub app_db: Arc<RwLock<Option<AppDatabase>>>,
    pub user_db: Arc<RwLock<DatabaseManager>>,
    pub runtime: Arc<Runtime>,
}

impl Global for DbService {}

impl DbService {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Runtime::new()
            .expect("Failed to create tokio runtime");

        Self {
            app_db: Arc::new(RwLock::new(None)),
            user_db: Arc::new(RwLock::new(DatabaseManager::new())),
            runtime: Arc::new(runtime),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Initialize the app database (runs in tokio context)
    pub fn init_app_db(&self) -> Result<(), Box<dyn std::error::Error>> {
        let app_db_lock = self.app_db.clone();
        self.runtime.block_on(async move {
            let app_db = AppDatabase::new().await?;
            *app_db_lock.write().await = Some(app_db);
            Ok(())
        })
    }

    /// Get a clone of the app database lock
    pub fn app_db_handle(&self) -> Arc<RwLock<Option<AppDatabase>>> {
        self.app_db.clone()
    }

    /// Get a clone of the user database lock
    pub fn user_db_handle(&self) -> Arc<RwLock<DatabaseManager>> {
        self.user_db.clone()
    }

    /// Get a handle to the tokio runtime for running database operations
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }
}
