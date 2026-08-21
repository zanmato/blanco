//! The single enumeration of supported backends. Drivers report it from
//! `Connection::database_type()`, the service keys its factory registry on it,
//! and the UI derives dialect facts (placeholder styles, editor language,
//! formatter dialect) from it, so there is exactly one place to extend when a
//! backend is added.

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
        match self {
            DatabaseType::SQLite => false,
            DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql
            | DatabaseType::Redis => true,
        }
    }

    /// Whether this backend speaks SQL. When false, the editor disables
    /// SQL-only machinery (tree-sitter parsing, EXPLAIN, format/lint, bind
    /// parameter detection, DDL cache invalidation) and treats the editor
    /// buffer as line-oriented commands instead.
    pub fn supports_sql(&self) -> bool {
        match self {
            DatabaseType::SQLite
            | DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql => true,
            DatabaseType::Redis => false,
        }
    }

    /// Whether the connections sidebar offers relational table operations for
    /// this backend's objects: Export Data, Import Data, and Open Structure.
    /// These all assume a tabular, column-oriented object, so key/value stores
    /// (Redis) opt out and their keys are inspected instead. Kept separate from
    /// [`Self::supports_sql`] so a future driver can mix capabilities.
    pub fn supports_table_operations(&self) -> bool {
        match self {
            DatabaseType::SQLite
            | DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql => true,
            DatabaseType::Redis => false,
        }
    }

    /// Whether a leaf object is inspected directly rather than queried. Key/
    /// value stores (Redis) open a key inspector, so their leaf context-menu
    /// action reads "Inspect Key"; relational tables are a starting point for a
    /// query, so theirs reads "New Query". Drives the leaf menu label/behavior.
    pub fn inspects_objects(&self) -> bool {
        !self.supports_sql()
    }

    /// Display label for the sidebar folder that groups a connection's primary
    /// objects. SQL backends list "Tables"; key/value stores list "Keys". Only
    /// the visible label changes; the folder's internal slug stays `tables` so
    /// expansion bookkeeping is unaffected.
    pub fn primary_object_category_label(&self) -> &'static str {
        match self {
            DatabaseType::SQLite
            | DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql => "Tables",
            DatabaseType::Redis => "Keys",
        }
    }

    /// Separator used to fold flat key names into a nested folder tree in the
    /// connections sidebar. Key/value stores namespace keys by convention
    /// (Redis `user:1:name`), so returning `Some(sep)` tells the tree builder
    /// to group on it. Relational backends have a flat table list and return
    /// `None`. Keyed on the type so new drivers opt in without sprinkling
    /// driver checks through the UI.
    pub fn key_namespace_separator(&self) -> Option<char> {
        match self {
            DatabaseType::SQLite
            | DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql => None,
            DatabaseType::Redis => Some(':'),
        }
    }

    /// Name of the registered editor language used for syntax highlighting.
    /// Resolved against gpui-component's `LanguageRegistry` (see
    /// `blanco::sql::register_languages` / `blanco::redis_syntax::register_language`).
    /// Exhaustive on purpose: a new driver must pick its highlighting language.
    pub fn editor_language(&self) -> &'static str {
        match self {
            DatabaseType::SQLite
            | DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql => "sql",
            DatabaseType::Redis => "redis",
        }
    }

    /// Get the Sqruff dialect string for this database type
    pub fn to_sqruff_dialect(&self) -> &'static str {
        match self {
            DatabaseType::SQLite => "sqlite",
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::MySQL => "mysql",
            DatabaseType::ClickHouse => "ansi",
            DatabaseType::MsSql => "tsql",
            // Redis is not SQL; sqruff is never invoked for it (gated by
            // supports_sql), but a dialect string is still required here.
            DatabaseType::Redis => "ansi",
        }
    }

    /// The placeholder syntaxes this dialect recognizes as bind parameters.
    ///
    /// Note: `@name` in T-SQL is also used for local variable declarations
    /// (`DECLARE @x INT`), which share the parameter syntax. MySQL `@name` is a
    /// session variable, not a bind placeholder, so it is intentionally excluded.
    pub fn parameter_styles(&self) -> ParamStyles {
        match self {
            DatabaseType::SQLite => ParamStyles {
                question_mark: true,
                dollar_number: true,
                colon_named: true,
                at_named: true,
            },
            DatabaseType::PostgreSQL => ParamStyles {
                question_mark: false,
                dollar_number: true,
                colon_named: false,
                at_named: false,
            },
            DatabaseType::MySQL => ParamStyles {
                question_mark: true,
                dollar_number: false,
                colon_named: false,
                at_named: false,
            },
            DatabaseType::MsSql => ParamStyles {
                question_mark: false,
                dollar_number: false,
                colon_named: false,
                at_named: true,
            },
            DatabaseType::ClickHouse => ParamStyles::none(),
            DatabaseType::Redis => ParamStyles::none(),
        }
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
