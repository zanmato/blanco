//! Dialect-specific EXPLAIN wrappers.
//!
//! Used by the right-click "Explain query" / "Ask AI to optimize" actions.
//! Each backend has a slightly different EXPLAIN syntax and quirk set: we
//! deliberately avoid `EXPLAIN ANALYZE` for non-SELECT statements on Postgres
//! since `ANALYZE` actually executes the statement and would mutate data.

use blanco_core::DatabaseType;

/// Wrap `sql` in a dialect-appropriate EXPLAIN statement. The returned string
/// can be sent through the normal `execute_script` path.
pub fn wrap_explain(db_type: DatabaseType, sql: &str) -> String {
    db_type.dialect().explain(sql)
}
