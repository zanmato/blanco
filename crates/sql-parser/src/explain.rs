//! Dialect-specific EXPLAIN wrappers.
//!
//! Used by the right-click "Explain query" / "Ask AI to optimize" actions.
//! Each backend has a slightly different EXPLAIN syntax and quirk set: we
//! deliberately avoid `EXPLAIN ANALYZE` for non-SELECT statements on Postgres
//! since `ANALYZE` actually executes the statement and would mutate data.

use blanco_core::DatabaseType;

/// Whether the given SQL string is a plain `SELECT` (eligible for ANALYZE on
/// Postgres) or some other statement (in which case we use a non-executing
/// EXPLAIN form).
fn is_select(sql: &str) -> bool {
    sql.split_whitespace()
        .next()
        .map(|tok| tok.eq_ignore_ascii_case("SELECT") || tok.eq_ignore_ascii_case("WITH"))
        .unwrap_or(false)
}

/// Wrap `sql` in a dialect-appropriate EXPLAIN statement. The returned string
/// can be sent through the normal `execute_script` path.
pub fn wrap_explain(db_type: DatabaseType, sql: &str) -> String {
    let trimmed = sql.trim().trim_end_matches(';');
    match db_type {
        DatabaseType::PostgreSQL => {
            if is_select(trimmed) {
                format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}", trimmed)
            } else {
                format!("EXPLAIN (FORMAT JSON) {}", trimmed)
            }
        }
        DatabaseType::MySQL => format!("EXPLAIN FORMAT=TREE {}", trimmed),
        DatabaseType::MsSql => format!("SET SHOWPLAN_ALL ON; {}; SET SHOWPLAN_ALL OFF", trimmed),
        DatabaseType::SQLite => format!("EXPLAIN QUERY PLAN {}", trimmed),
        DatabaseType::ClickHouse => format!("EXPLAIN PLAN {}", trimmed),
        // Redis has no EXPLAIN; the Explain action is gated off by
        // supports_sql() so this arm is never reached in practice.
        DatabaseType::Redis => trimmed.to_string(),
    }
}
