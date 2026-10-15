//! The server's sessions and the statements that cancel or end them.

use std::time::Duration;

use super::Dialect;
use crate::DatabaseType;

/// One session on the server as the process list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSession {
    /// The id the cancel and kill statements take: a backend pid or
    /// connection id, or for ClickHouse the running query's id.
    pub id: String,
    pub user: Option<String>,
    pub database: Option<String>,
    pub state: Option<String>,
    /// How long the current query has run, or the session has been in its
    /// state when it is idle.
    pub duration: Option<Duration>,
    pub query: Option<String>,
}

impl ServerSession {
    /// A row of [`Dialect::process_list_query`]: id, user, database, state,
    /// duration in seconds and query text, in that order. `None` for a row
    /// without an id.
    pub fn from_row(row: &[Option<String>]) -> Option<Self> {
        let cell = |ix: usize| {
            row.get(ix)
                .cloned()
                .flatten()
                .filter(|value| !value.trim().is_empty())
        };
        Some(Self {
            id: cell(0)?,
            user: cell(1),
            database: cell(2),
            state: cell(3),
            duration: cell(4)
                .and_then(|seconds| seconds.trim().parse::<f64>().ok())
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.)
                .map(Duration::from_secs_f64),
            query: cell(5),
        })
    }
}

impl Dialect {
    /// The query listing the server's client sessions as id, user, database,
    /// state, duration in seconds and query text, longest running first. The
    /// session running it is left out. `None` where Blanco has no process
    /// list.
    pub fn process_list_query(&self) -> Option<&'static str> {
        Some(match self.0 {
            DatabaseType::PostgreSQL => {
                "SELECT pid::text, usename, datname, state, \
                 EXTRACT(EPOCH FROM now() - COALESCE(CASE WHEN state = 'active' THEN query_start END, state_change, backend_start))::float8, \
                 query \
                 FROM pg_stat_activity \
                 WHERE backend_type = 'client backend' AND pid <> pg_backend_pid() \
                 ORDER BY 5 DESC NULLS LAST"
            }
            // TIME rather than MariaDB's TIME_MS, which MySQL lacks.
            DatabaseType::MySQL => {
                "SELECT CAST(ID AS CHAR), USER, DB, \
                 CASE WHEN STATE IS NULL OR STATE = '' THEN COMMAND ELSE CONCAT(COMMAND, ': ', STATE) END, \
                 TIME, INFO \
                 FROM information_schema.PROCESSLIST \
                 WHERE ID <> CONNECTION_ID() \
                 ORDER BY TIME DESC"
            }
            DatabaseType::MsSql => {
                "SELECT CAST(s.session_id AS varchar(10)), s.login_name, \
                 DB_NAME(COALESCE(r.database_id, s.database_id)), COALESCE(r.status, s.status), \
                 DATEDIFF(millisecond, COALESCE(r.start_time, s.last_request_start_time), SYSDATETIME()) / 1000.0, \
                 t.text \
                 FROM sys.dm_exec_sessions s \
                 LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id \
                 OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t \
                 WHERE s.is_user_process = 1 AND s.session_id <> @@SPID \
                 ORDER BY 5 DESC"
            }
            // ClickHouse has no sessions to list, only running queries.
            DatabaseType::ClickHouse => {
                "SELECT query_id, user, current_database, 'running', elapsed, query \
                 FROM system.processes \
                 WHERE query_id != queryID() \
                 ORDER BY elapsed DESC"
            }
            DatabaseType::SQLite | DatabaseType::DuckDb | DatabaseType::Redis => return None,
        })
    }

    pub fn supports_process_list(&self) -> bool {
        self.process_list_query().is_some()
    }

    /// The statement that cancels the query session `id` is running and
    /// leaves the session open. `None` where the backend cannot, or `id` is
    /// not a session id of this backend. SQL Server has no way to cancel
    /// another session's query short of ending it.
    pub fn cancel_query_statement(&self, id: &str) -> Option<String> {
        match self.0 {
            DatabaseType::PostgreSQL => Some(format!("SELECT pg_cancel_backend({})", numeric(id)?)),
            DatabaseType::MySQL => Some(format!("KILL QUERY {}", numeric(id)?)),
            DatabaseType::ClickHouse => Some(format!(
                "KILL QUERY WHERE query_id = {}",
                self.string_literal(id)
            )),
            DatabaseType::MsSql
            | DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::Redis => None,
        }
    }

    /// The statement that ends session `id`, rolling back its open
    /// transaction. `None` where the backend cannot, or `id` is not a
    /// session id of this backend. ClickHouse queries belong to no session
    /// that could be ended.
    pub fn kill_session_statement(&self, id: &str) -> Option<String> {
        match self.0 {
            DatabaseType::PostgreSQL => {
                Some(format!("SELECT pg_terminate_backend({})", numeric(id)?))
            }
            DatabaseType::MySQL | DatabaseType::MsSql => Some(format!("KILL {}", numeric(id)?)),
            DatabaseType::ClickHouse
            | DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::Redis => None,
        }
    }

    pub fn can_cancel_queries(&self) -> bool {
        self.cancel_query_statement("1").is_some()
    }

    pub fn can_kill_sessions(&self) -> bool {
        self.kill_session_statement("1").is_some()
    }
}

/// `id` when it is a plain number, as process ids are; anything else would
/// be pasted into a statement.
fn numeric(id: &str) -> Option<&str> {
    let id = id.trim();
    (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::ServerSession;
    use crate::DatabaseType;
    use std::time::Duration;

    #[test]
    fn statements_take_only_session_ids() {
        let postgres = DatabaseType::PostgreSQL.dialect();
        assert_eq!(
            postgres.cancel_query_statement("123").as_deref(),
            Some("SELECT pg_cancel_backend(123)")
        );
        assert_eq!(
            postgres.kill_session_statement(" 123 ").as_deref(),
            Some("SELECT pg_terminate_backend(123)")
        );
        assert_eq!(postgres.kill_session_statement("1; DROP TABLE x"), None);

        let mysql = DatabaseType::MySQL.dialect();
        assert_eq!(
            mysql.cancel_query_statement("7").as_deref(),
            Some("KILL QUERY 7")
        );
        assert_eq!(mysql.kill_session_statement("7").as_deref(), Some("KILL 7"));

        let mssql = DatabaseType::MsSql.dialect();
        assert_eq!(mssql.cancel_query_statement("53"), None);
        assert_eq!(
            mssql.kill_session_statement("53").as_deref(),
            Some("KILL 53")
        );

        let clickhouse = DatabaseType::ClickHouse.dialect();
        assert_eq!(
            clickhouse.cancel_query_statement("a'b").as_deref(),
            Some("KILL QUERY WHERE query_id = 'a''b'")
        );
        assert_eq!(clickhouse.kill_session_statement("abc"), None);

        for db_type in [
            DatabaseType::SQLite,
            DatabaseType::DuckDb,
            DatabaseType::Redis,
        ] {
            let dialect = db_type.dialect();
            assert!(!dialect.supports_process_list());
            assert!(!dialect.can_cancel_queries());
            assert!(!dialect.can_kill_sessions());
        }
    }

    #[test]
    fn sessions_parse_from_listing_rows() {
        let row = |cells: &[Option<&str>]| {
            cells
                .iter()
                .map(|cell| cell.map(str::to_string))
                .collect::<Vec<_>>()
        };
        let session = ServerSession::from_row(&row(&[
            Some("580"),
            Some("blanco"),
            Some("blanco"),
            Some("active"),
            Some("3.25"),
            Some("SELECT pg_sleep(20)"),
        ]))
        .expect("session");
        assert_eq!(session.id, "580");
        assert_eq!(session.duration, Some(Duration::from_millis(3250)));
        assert_eq!(session.query.as_deref(), Some("SELECT pg_sleep(20)"));

        let idle =
            ServerSession::from_row(&row(&[Some("9"), None, Some(""), None, Some("-1"), None]))
                .expect("session");
        assert_eq!(idle.database, None);
        assert_eq!(idle.duration, None);
        assert_eq!(ServerSession::from_row(&row(&[None, Some("x")])), None);
    }
}
