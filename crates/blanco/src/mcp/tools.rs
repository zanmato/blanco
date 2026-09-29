//! Pure helpers shared by the MCP tools, the in-app agent tools and the editor:
//! the write-confirmation policy and the JSON shape a query result is reported
//! in. Nothing here touches GPUI or the network, so all of it is unit tested.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use blanco_core::{QueryResult, StatementAccess};

/// Rows a tool result carries before it is cut off. Agents iterate rather than
/// page, so a small cap keeps a stray `SELECT *` from flooding the context.
pub(crate) const DEFAULT_MAX_ROWS: usize = 100;

/// Upper bound a caller may raise `max_rows` to.
pub(crate) const MAX_ROWS_LIMIT: usize = 1000;

/// Whether a statement may run straight away or needs the user's click first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WritePolicy {
    Run,
    Confirm,
}

/// Reads always run. Writes against a PROD connection and destructive DDL
/// (DROP, TRUNCATE) always confirm, no matter the setting, because they are
/// the statements a wrong guess cannot undo. Every other write follows the
/// `allow_writes` setting. The classifier is conservative, so a statement it
/// does not recognise counts as a write and lands in the confirm path too.
pub(crate) fn write_policy(
    access: StatementAccess,
    destructive_ddl: bool,
    is_prod: bool,
    allow_writes: bool,
) -> WritePolicy {
    match access {
        StatementAccess::Read => WritePolicy::Run,
        StatementAccess::Write if is_prod || destructive_ddl => WritePolicy::Confirm,
        StatementAccess::Write if allow_writes => WritePolicy::Run,
        StatementAccess::Write => WritePolicy::Confirm,
    }
}

/// Write kinds (`INSERT`, `UPDATE`, ...) the user chose not to be asked about
/// again, per connection, for one MCP session.
#[derive(Debug, Default)]
pub(crate) struct SessionGrants(Mutex<HashSet<(i64, String)>>);

impl SessionGrants {
    /// Whether every kind in `kinds` is granted on the connection. A statement
    /// with no recognised kind is never covered.
    pub(crate) fn covers(&self, connection_id: i64, kinds: &[String]) -> bool {
        let grants = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        !kinds.is_empty()
            && kinds
                .iter()
                .all(|kind| grants.contains(&(connection_id, kind.clone())))
    }

    pub(crate) fn grant(&self, connection_id: i64, kinds: &[String]) {
        let mut grants = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        grants.extend(kinds.iter().map(|kind| (connection_id, kind.clone())));
    }
}

/// Uses the SQL grammar so a `DROP` inside a string literal or comment does not
/// trip the guard, while `DROP\tTABLE` / `TRUNCATE\nTABLE` still do. When the
/// text does not parse cleanly the tree cannot be trusted, so fall back to a
/// word-boundary scan and err on the side of confirming.
pub(crate) fn contains_destructive_ddl(sql: &str) -> bool {
    match sql_parser::statement_parser::contains_node_kind(
        sql,
        &["keyword_drop", "keyword_truncate"],
    ) {
        Some(found) => found,
        None => {
            let upper = sql.to_uppercase();
            upper
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|word| word == "DROP" || word == "TRUNCATE")
        }
    }
}

/// The JSON a tool hands back for one result set: columns, at most `max_rows`
/// rows (with `truncated` and a hint when cut), affected-row count and timing.
/// A driver-reported error (`is_error`) is folded into `error`/`error_message`
/// rather than raised, so the agent sees the database's own wording.
pub(crate) fn result_to_json(result: &QueryResult, max_rows: usize) -> serde_json::Value {
    let truncated = result.rows.len() > max_rows;
    let error_message = if result.is_error {
        result.rows.first().map(|first_row| {
            first_row
                .iter()
                .filter_map(|value| value.as_deref())
                .collect::<Vec<&str>>()
                .join(" ")
        })
    } else {
        None
    };
    let display_rows: Vec<_> = result.rows.iter().take(max_rows).collect();

    let mut json = serde_json::json!({
        "columns": result.columns,
        "rows": display_rows,
        "rows_affected": result.rows_affected,
        "execution_time_ms": result.execution_time_ms,
        "truncated": truncated,
    });

    if truncated {
        json["hint"] = serde_json::json!(format!(
            "Result truncated to {max_rows} rows. Add a LIMIT clause or WHERE filter to narrow results."
        ));
    }

    if result.is_error {
        json["error"] = serde_json::json!(true);
        if let Some(message) = error_message {
            json["error_message"] = serde_json::json!(message);
        }
    }

    json
}

/// Run one statement (or script) and report the final result set as JSON. This
/// is the shared execution path of the agent's `execute-sql` tool and the MCP
/// `run_sql` tool: the row limit is applied in the dialect's own spelling, the
/// script path is used so the last statement's result is returned, mirroring
/// the editor's Run button.
pub(crate) async fn run_sql_json(
    db_service: &dyn blanco_core::DatabaseService,
    connection_id: i64,
    database: Option<&str>,
    db_type: blanco_core::DatabaseType,
    sql: &str,
    auto_limit: Option<usize>,
    max_rows: usize,
) -> anyhow::Result<serde_json::Value> {
    let sql = match auto_limit {
        Some(limit) => db_type.dialect().apply_row_limit(sql, limit),
        None => sql.to_string(),
    };
    let results = db_service
        .execute_script(connection_id, database, &sql)
        .await?;
    let result = results.into_iter().last().unwrap_or_default();
    Ok(result_to_json(&result, max_rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_run_even_on_prod() {
        assert_eq!(
            write_policy(StatementAccess::Read, false, true, false),
            WritePolicy::Run
        );
    }

    #[test]
    fn writes_confirm_unless_allowed() {
        assert_eq!(
            write_policy(StatementAccess::Write, false, false, false),
            WritePolicy::Confirm
        );
        assert_eq!(
            write_policy(StatementAccess::Write, false, false, true),
            WritePolicy::Run
        );
    }

    #[test]
    fn prod_and_destructive_writes_always_confirm() {
        assert_eq!(
            write_policy(StatementAccess::Write, false, true, true),
            WritePolicy::Confirm
        );
        assert_eq!(
            write_policy(StatementAccess::Write, true, false, true),
            WritePolicy::Confirm
        );
    }

    #[test]
    fn session_grants_cover_only_granted_kinds_on_that_connection() {
        let grants = SessionGrants::default();
        let insert = vec!["INSERT".to_string()];
        let insert_and_update = vec!["INSERT".to_string(), "UPDATE".to_string()];
        assert!(!grants.covers(1, &insert));

        grants.grant(1, &insert);
        assert!(grants.covers(1, &insert));
        assert!(!grants.covers(1, &insert_and_update));
        assert!(!grants.covers(2, &insert));
        assert!(!grants.covers(1, &[]));

        grants.grant(1, &insert_and_update);
        assert!(grants.covers(1, &insert_and_update));
    }

    #[test]
    fn destructive_ddl_guard_matches_any_whitespace() {
        assert!(contains_destructive_ddl("DROP TABLE users"));
        assert!(contains_destructive_ddl("drop\ttable users"));
        assert!(contains_destructive_ddl("TRUNCATE\nTABLE users;"));
        assert!(contains_destructive_ddl("SELECT 1; DROP TABLE users"));
    }

    #[test]
    fn destructive_ddl_guard_ignores_literals_and_identifiers() {
        assert!(!contains_destructive_ddl(
            "SELECT 'please DROP this' AS note"
        ));
        assert!(!contains_destructive_ddl("SELECT drop_count FROM stats"));
        assert!(!contains_destructive_ddl("SELECT * FROM users"));
    }

    fn result_with_rows(row_count: usize) -> QueryResult {
        QueryResult {
            columns: vec!["id".to_string()],
            rows: (0..row_count)
                .map(|index| vec![Some(index.to_string())])
                .collect(),
            rows_affected: row_count as u64,
            execution_time_ms: Some(3),
            ..QueryResult::default()
        }
    }

    #[test]
    fn result_json_truncates_and_flags() {
        let json = result_to_json(&result_with_rows(5), 2);
        assert_eq!(json["truncated"], true);
        assert_eq!(json["rows"].as_array().map(Vec::len), Some(2));
        assert_eq!(json["rows_affected"], 5);
        assert!(json["hint"].as_str().is_some());
        assert!(json.get("error").is_none());
    }

    #[test]
    fn result_json_reports_driver_errors() {
        let mut result = result_with_rows(0);
        result.is_error = true;
        result.rows = vec![vec![Some("syntax error near FROM".to_string())]];
        let json = result_to_json(&result, 10);
        assert_eq!(json["error"], true);
        assert_eq!(json["error_message"], "syntax error near FROM");
        assert_eq!(json["truncated"], false);
    }
}
