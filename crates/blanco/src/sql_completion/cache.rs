use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Cache entry with timestamp
#[derive(Debug, Clone)]
pub struct CacheEntry<T> {
    pub data: T,
    pub timestamp: u64,
}

impl<T> CacheEntry<T> {
    pub fn new(data: T) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self { data, timestamp }
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
    pub tables: Option<CacheEntry<Vec<String>>>,
    pub columns: HashMap<String, CacheEntry<Vec<String>>>,
    pub table_info: HashMap<String, CacheEntry<String>>,
    pub column_info: HashMap<String, CacheEntry<String>>,
}

impl MetadataCache {
    pub fn new() -> Self {
        Self {
            tables: None,
            columns: HashMap::new(),
            table_info: HashMap::new(),
            column_info: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.tables = None;
        self.columns.clear();
        self.table_info.clear();
        self.column_info.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_none() && self.columns.is_empty()
    }
}
