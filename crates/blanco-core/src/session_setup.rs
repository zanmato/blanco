//! Startup SQL: statements a connection runs on every new session before it
//! is used, for example to set the time zone or the search path. Drivers run
//! them on each physical connection they open, pooled ones and reconnects
//! included, and a session whose setup fails is never handed out.

use crate::write_guard::split_statements;

/// The statements of a connection's startup SQL, trimmed, without empty ones.
pub fn session_setup_statements(sql: &str) -> Vec<String> {
    split_statements(sql)
        .into_iter()
        .map(|statement| statement.trim().to_string())
        .filter(|statement| !statement.is_empty())
        .collect()
}

/// The error for a startup statement that failed, naming the statement.
pub fn session_setup_failure(statement: &str, error: impl std::fmt::Display) -> String {
    format!("Startup SQL failed at `{}`: {error}", statement.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_setup_statements() {
        assert_eq!(
            session_setup_statements(
                "SET TIME ZONE 'UTC';\n\n  SET search_path TO app, public ;\n;  "
            ),
            ["SET TIME ZONE 'UTC'", "SET search_path TO app, public"]
        );
        assert!(session_setup_statements("  \n").is_empty());
    }
}
