use blanco_core::connection_trait::QueryableEntity;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Cache entry with timestamp
#[derive(Debug, Clone)]
pub struct CacheEntry<T> {
    pub data: Arc<T>,
    pub timestamp: u64,
}

impl<T> CacheEntry<T> {
    pub fn new(data: T) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            data: Arc::new(data),
            timestamp,
        }
    }

    pub fn is_expired(&self, ttl_seconds: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.timestamp) > ttl_seconds
    }
}

/// Metadata cache for tables and columns
#[derive(Debug, Clone)]
pub struct MetadataCache {
    pub tables: Option<CacheEntry<Vec<QueryableEntity>>>,
    pub columns: HashMap<String, CacheEntry<Vec<String>>>,
}

impl MetadataCache {
    pub fn new() -> Self {
        Self {
            tables: None,
            columns: HashMap::new(),
        }
    }

    pub fn invalidate_all(&mut self) {
        self.tables = None;
        self.columns.clear();
    }
}
