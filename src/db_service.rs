use crate::app_database::AppDatabase;
use crate::database::DatabaseManager;
use gpui::{App, Global};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Global database service that holds both the app database and user database manager
#[derive(Clone)]
pub struct DbService {
    pub app_db: Arc<RwLock<Option<AppDatabase>>>,
    pub user_db: Arc<RwLock<DatabaseManager>>,
}

impl Global for DbService {}

impl DbService {
    pub fn new() -> Self {
        Self {
            app_db: Arc::new(RwLock::new(None)),
            user_db: Arc::new(RwLock::new(DatabaseManager::new())),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Get a clone of the app database lock
    pub fn app_db_handle(&self) -> Arc<RwLock<Option<AppDatabase>>> {
        self.app_db.clone()
    }

    /// Get a clone of the user database lock
    pub fn user_db_handle(&self) -> Arc<RwLock<DatabaseManager>> {
        self.user_db.clone()
    }
}
