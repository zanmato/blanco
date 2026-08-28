//! The single enumeration of supported backends. Drivers report it from
//! `Connection::database_type()`, the service keys its factory registry on it,
//! and every dialect fact (quoting, placeholders, editor language, formatter
//! dialect, ...) is answered by [`crate::Dialect`] via `db_type.dialect()`, so
//! there is exactly one place to extend when a backend is added.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Database type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatabaseType {
    SQLite,
    PostgreSQL,
    MySQL,
    ClickHouse,
    MsSql,
    Redis,
}

/// Which placeholder syntaxes a SQL dialect recognizes as bind parameters.
///
/// The SQL grammar happily parses tokens like `?` as bind parameters, but those
/// same tokens are operators in some dialects (e.g. Postgres jsonb key-exists
/// `?`, `?|`, `?&`). Filtering parameter detection by dialect avoids prompting
/// the user for a value where no placeholder actually exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamStyles {
    /// `?` positional placeholder (JDBC / MySQL / SQLite).
    pub question_mark: bool,
    /// `$1`, `$2` positional placeholder (Postgres / SQLite).
    pub dollar_number: bool,
    /// `:name` named placeholder.
    pub colon_named: bool,
    /// `@name` named placeholder (T-SQL).
    pub at_named: bool,
}

impl ParamStyles {
    /// Every style enabled. Used by callers that only need statement extraction
    /// and ignore the detected parameters.
    pub const fn all() -> Self {
        Self {
            question_mark: true,
            dollar_number: true,
            colon_named: true,
            at_named: true,
        }
    }

    /// No style enabled.
    pub const fn none() -> Self {
        Self {
            question_mark: false,
            dollar_number: false,
            colon_named: false,
            at_named: false,
        }
    }
}

impl DatabaseType {
    /// Get the string representation of the database type
    pub fn as_str(&self) -> &'static str {
        match self {
            DatabaseType::SQLite => "SQLite",
            DatabaseType::PostgreSQL => "PostgreSQL",
            DatabaseType::MySQL => "MySQL",
            DatabaseType::ClickHouse => "ClickHouse",
            DatabaseType::MsSql => "SQL Server",
            DatabaseType::Redis => "Redis",
        }
    }

    /// Parse a string into a DatabaseType
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "SQLite" => Some(DatabaseType::SQLite),
            "PostgreSQL" => Some(DatabaseType::PostgreSQL),
            "MySQL" => Some(DatabaseType::MySQL),
            "ClickHouse" => Some(DatabaseType::ClickHouse),
            "SQL Server" => Some(DatabaseType::MsSql),
            "Redis" => Some(DatabaseType::Redis),
            _ => None,
        }
    }

    /// Check if this database type supports switching databases via connection string
    pub fn supports_database_switching(&self) -> bool {
        self.dialect().supports_database_switching()
    }

    /// See [`crate::Dialect::supports_sql`].
    pub fn supports_sql(&self) -> bool {
        self.dialect().supports_sql()
    }

    /// See [`crate::Dialect::supports_table_operations`].
    pub fn supports_table_operations(&self) -> bool {
        self.dialect().supports_table_operations()
    }

    /// See [`crate::Dialect::inspects_objects`].
    pub fn inspects_objects(&self) -> bool {
        self.dialect().inspects_objects()
    }

    /// See [`crate::Dialect::primary_object_category_label`].
    pub fn primary_object_category_label(&self) -> &'static str {
        self.dialect().primary_object_category_label()
    }

    /// See [`crate::Dialect::key_namespace_separator`].
    pub fn key_namespace_separator(&self) -> Option<char> {
        self.dialect().key_namespace_separator()
    }

    /// See [`crate::Dialect::editor_language`].
    pub fn editor_language(&self) -> &'static str {
        self.dialect().editor_language()
    }

    /// See [`crate::Dialect::sqruff_dialect`].
    pub fn to_sqruff_dialect(&self) -> &'static str {
        self.dialect().sqruff_dialect()
    }

    /// See [`crate::Dialect::parameter_styles`].
    pub fn parameter_styles(&self) -> ParamStyles {
        self.dialect().parameter_styles()
    }

    /// Parse from the string representation stored in app_database. Same
    /// strings as [`Self::as_str`] / [`Self::from_name`].
    pub fn from_db_type_str(s: &str) -> Option<Self> {
        Self::from_name(s)
    }
}

impl fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
