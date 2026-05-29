use blanco_core::QueryableEntity;
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

/// Key used for caching columns: schema-qualified table name.
pub fn columns_key(schema: &str, table: &str) -> String {
    format!("{schema}.{table}")
}

/// Metadata cache for schemas, tables, and columns.
///
/// Tables are keyed by schema name and columns by `schema.table` so that the
/// same connection can serve completions for any schema without collisions
/// (e.g. `public.customers` vs `sales.customers`).
#[derive(Debug, Clone)]
pub struct MetadataCache {
    pub schemas: Option<CacheEntry<Vec<String>>>,
    /// Whether the backend supports schemas (false for MySQL/SQLite). Cached
    /// alongside `schemas` since they are fetched together.
    pub supports_schemas: Option<bool>,
    pub tables_by_schema: HashMap<String, CacheEntry<Vec<QueryableEntity>>>,
    pub columns: HashMap<String, CacheEntry<Vec<String>>>,
}

impl MetadataCache {
    pub fn new() -> Self {
        Self {
            schemas: None,
            supports_schemas: None,
            tables_by_schema: HashMap::new(),
            columns: HashMap::new(),
        }
    }

    pub fn invalidate_all(&mut self) {
        self.schemas = None;
        self.supports_schemas = None;
        self.tables_by_schema.clear();
        self.columns.clear();
    }
}
