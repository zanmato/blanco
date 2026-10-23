//! Every per-backend SQL fact in one place: quoting, literal escaping,
//! placeholders, row limiting, EXPLAIN, upsert spelling, capabilities and the
//! odd defaults (schema, port). The UI, transformers and import code ask a
//! [`Dialect`] instead of matching on [`DatabaseType`] themselves, so adding a
//! backend means filling in one arm per method here rather than hunting down
//! two dozen scattered matches.

use crate::{DatabaseType, ParamStyles, QueryResult, explain_plan};
use sqlparser::dialect::{
    DuckDbDialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect,
};

mod column_types;
mod data_types;
mod process_list;
pub use data_types::DataTypeName;
pub use process_list::ServerSession;

/// Dialect facts for one [`DatabaseType`]. Obtained through
/// [`DatabaseType::dialect`]; cheap to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dialect(DatabaseType);

/// How a dialect spells "insert, but tolerate duplicates".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertStyle {
    /// `INSERT ... ON CONFLICT (keys) DO NOTHING | DO UPDATE SET c = EXCLUDED.c`
    OnConflict,
    /// `INSERT IGNORE ...` / `INSERT ... ON DUPLICATE KEY UPDATE c = VALUES(c)`
    OnDuplicateKey,
}

/// Which plan an explain asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplainMode {
    /// The planner's estimate. The statement is not executed.
    Estimated,
    /// The statement is executed and the plan carries actual rows and
    /// timings.
    Analyze,
}

/// The server behind a [`DatabaseType`] where servers of one type spell
/// things differently. MySQL connections may talk to MariaDB, which has no
/// `EXPLAIN FORMAT=TREE` and analyzes with `ANALYZE <statement>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerVariant {
    #[default]
    Standard,
    MariaDb,
}

impl ServerVariant {
    /// The variant a MySQL server's `SELECT VERSION()` string names.
    pub fn from_mysql_version(version: &str) -> Self {
        if version.to_ascii_lowercase().contains("mariadb") {
            ServerVariant::MariaDb
        } else {
            ServerVariant::Standard
        }
    }
}

/// How a `VALUES` list is turned into a selectable relation when exporting
/// rows as a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValuesQueryStyle {
    /// Wrap the rows in `WITH t(cols) AS (VALUES ...) SELECT * FROM t` instead
    /// of `SELECT * FROM (VALUES ...) AS t(cols)`.
    pub as_cte: bool,
    /// Each row needs the `ROW(...)` keyword.
    pub row_keyword: bool,
}

/// The tree-sitter grammar that parses and highlights a backend's SQL. Each
/// is one dialect of tree-sitter-sql-polyglot; `Ansi` is its strict base,
/// for SQL that belongs to no backend, such as snippets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqlGrammar {
    Ansi,
    PostgreSql,
    MariaDb,
    Sqlite,
    TSql,
    ClickHouse,
    DuckDb,
    CockroachDb,
}

impl SqlGrammar {
    pub const ALL: [SqlGrammar; 8] = [
        SqlGrammar::Ansi,
        SqlGrammar::PostgreSql,
        SqlGrammar::MariaDb,
        SqlGrammar::Sqlite,
        SqlGrammar::TSql,
        SqlGrammar::ClickHouse,
        SqlGrammar::DuckDb,
        SqlGrammar::CockroachDb,
    ];

    /// The name the grammar is registered under for editor highlighting.
    pub fn language_name(&self) -> &'static str {
        match self {
            SqlGrammar::Ansi => "sql",
            SqlGrammar::PostgreSql => "sql_postgres",
            SqlGrammar::MariaDb => "sql_mariadb",
            SqlGrammar::Sqlite => "sql_sqlite",
            SqlGrammar::TSql => "sql_tsql",
            SqlGrammar::ClickHouse => "sql_clickhouse",
            SqlGrammar::DuckDb => "sql_duckdb",
            SqlGrammar::CockroachDb => "sql_cockroachdb",
        }
    }

    /// The grammar registered under `name`, if any.
    pub fn from_language_name(name: &str) -> Option<SqlGrammar> {
        Self::ALL
            .into_iter()
            .find(|grammar| grammar.language_name() == name)
    }
}

/// The family of native command line tools (dump, restore, shell) that can
/// back up and restore a backend's databases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientTools {
    /// `pg_dump`, `pg_restore` and `psql`.
    PostgreSql,
    /// `mysqldump` and `mysql`, or their `mariadb-dump` and `mariadb` names.
    MySql,
}

impl ClientTools {
    /// A query whose single value is the server's version string, in the
    /// form the tools print theirs (`16.4 (Debian ...)`, `10.11.6-MariaDB`).
    pub fn server_version_query(&self) -> &'static str {
        match self {
            ClientTools::PostgreSql => "SHOW server_version",
            ClientTools::MySql => "SELECT VERSION()",
        }
    }
}

impl DatabaseType {
    /// Every supported backend, in the order the connection dialog lists
    /// them. Index into it with [`DatabaseType::index`].
    pub const ALL: [DatabaseType; 8] = [
        DatabaseType::SQLite,
        DatabaseType::DuckDb,
        DatabaseType::PostgreSQL,
        DatabaseType::CockroachDb,
        DatabaseType::MySQL,
        DatabaseType::ClickHouse,
        DatabaseType::MsSql,
        DatabaseType::Redis,
    ];

    /// Position in [`DatabaseType::ALL`].
    pub fn index(&self) -> usize {
        match self {
            DatabaseType::SQLite => 0,
            DatabaseType::DuckDb => 1,
            DatabaseType::PostgreSQL => 2,
            DatabaseType::CockroachDb => 3,
            DatabaseType::MySQL => 4,
            DatabaseType::ClickHouse => 5,
            DatabaseType::MsSql => 6,
            DatabaseType::Redis => 7,
        }
    }

    pub const fn dialect(&self) -> Dialect {
        Dialect(*self)
    }
}

impl Dialect {
    pub const fn database_type(&self) -> DatabaseType {
        self.0
    }

    // ----- capabilities -----

    /// Whether this backend speaks SQL. When false, the editor disables
    /// SQL-only machinery (tree-sitter parsing, EXPLAIN, format/lint, bind
    /// parameter detection, DDL cache invalidation) and treats the editor
    /// buffer as line-oriented commands instead.
    pub fn supports_sql(&self) -> bool {
        match self.0 {
            DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::PostgreSQL
            | DatabaseType::CockroachDb
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql => true,
            DatabaseType::Redis => false,
        }
    }

    /// Whether the database is a local file opened by path rather than a
    /// server reached by host and port, and so has no password.
    pub fn is_file_based(&self) -> bool {
        matches!(self.0, DatabaseType::SQLite | DatabaseType::DuckDb)
    }

    /// The database name a query tab on a database file is opened with.
    /// The sidebar addresses a SQLite file by its `main` schema; an empty
    /// name keeps a DuckDB tab in the file's own default database.
    pub fn file_database_name(&self) -> &'static str {
        match self.0 {
            DatabaseType::SQLite => "main",
            _ => "",
        }
    }

    /// Statements that make a newly created database file write its header,
    /// so the file is recognised as a database when opened again. SQLite
    /// leaves a new file empty until its first write, and in WAL mode that
    /// write waits in the log until a checkpoint. DuckDB writes the header
    /// when the file is created.
    pub fn new_file_statement(&self) -> Option<&'static str> {
        match self.0 {
            DatabaseType::SQLite => {
                Some("PRAGMA user_version = 0; PRAGMA wal_checkpoint(TRUNCATE)")
            }
            _ => None,
        }
    }

    /// Whether the connections sidebar offers relational table operations for
    /// this backend's objects: Export Data, Import Data, and Open Structure.
    /// These all assume a tabular, column-oriented object, so key/value stores
    /// (Redis) opt out and their keys are inspected instead. Kept separate from
    /// [`Self::supports_sql`] so a future driver can mix capabilities.
    pub fn supports_table_operations(&self) -> bool {
        self.supports_sql()
    }

    /// Whether switching databases through the connection string is
    /// meaningful. A SQLite connection is one file.
    pub fn supports_database_switching(&self) -> bool {
        !matches!(self.0, DatabaseType::SQLite)
    }

    /// Whether the sidebar lists every database on the server above the
    /// schema level, which is what "only show the configured database" trims.
    /// The other backends show a single level that the driver already scopes.
    pub fn lists_server_databases(&self) -> bool {
        matches!(
            self.0,
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb | DatabaseType::MsSql
        )
    }

    /// A query answering one row with the database and the schema a new
    /// session lands in, which the sidebar opens on connect. PostgreSQL's
    /// `current_schema()` is the first schema on the search path that exists,
    /// or NULL when none does. `None` for backends whose tree has no database
    /// and schema levels to open.
    pub fn current_namespace_query(&self) -> Option<&'static str> {
        match self.0 {
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb | DatabaseType::DuckDb => {
                Some("SELECT current_database()::text, current_schema()::text")
            }
            DatabaseType::MsSql => Some("SELECT DB_NAME(), SCHEMA_NAME()"),
            DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::ClickHouse
            | DatabaseType::Redis => None,
        }
    }

    /// Whether a leaf object is inspected directly rather than queried. Key/
    /// value stores (Redis) open a key inspector, so their leaf context-menu
    /// action reads "Inspect Key"; relational tables are a starting point for a
    /// query, so theirs reads "New Query".
    pub fn inspects_objects(&self) -> bool {
        !self.supports_sql()
    }

    /// Whether the driver can produce a `CREATE TABLE` statement for the
    /// structure tab. SQL Server has no implementation, so the button is
    /// hidden rather than surfacing an error.
    pub fn supports_create_table_ddl(&self) -> bool {
        self.supports_sql() && !matches!(self.0, DatabaseType::MsSql)
    }

    /// Whether the results grid may edit rows in place. ClickHouse mutations
    /// are asynchronous and its tables rarely have a usable primary key.
    pub fn rows_editable(&self) -> bool {
        self.supports_sql() && !matches!(self.0, DatabaseType::ClickHouse)
    }

    /// The primary key a table declared without one gets behind the scenes,
    /// which `SELECT *` leaves out: CockroachDB's `rowid`. A read for the
    /// grid names it, so the rows of such a table can be edited.
    pub fn hidden_row_key(&self) -> Option<&'static str> {
        match self.0 {
            DatabaseType::CockroachDb => Some("rowid"),
            _ => None,
        }
    }

    /// Whether column and index DDL (ALTER TABLE) is offered from the structure
    /// tab.
    pub fn supports_column_ddl(&self) -> bool {
        self.supports_sql()
    }

    /// The native client tools that back up and restore this backend, `None`
    /// where Blanco does not drive them. CockroachDB speaks PostgreSQL's
    /// protocol, but `pg_dump` fails on its catalog.
    pub fn client_tools(&self) -> Option<ClientTools> {
        match self.0 {
            DatabaseType::PostgreSQL => Some(ClientTools::PostgreSql),
            DatabaseType::MySQL => Some(ClientTools::MySql),
            DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::CockroachDb
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql
            | DatabaseType::Redis => None,
        }
    }

    /// Whether the backend has foreign keys to list in the structure tab.
    pub fn supports_foreign_keys(&self) -> bool {
        self.supports_sql() && !matches!(self.0, DatabaseType::ClickHouse)
    }

    /// Whether foreign keys can be added and dropped in place. SQLite and
    /// DuckDB only declare them in CREATE TABLE.
    pub fn supports_foreign_key_ddl(&self) -> bool {
        self.supports_foreign_keys()
            && !matches!(self.0, DatabaseType::SQLite | DatabaseType::DuckDb)
    }

    /// Whether a failed batch of DDL rolls back as a whole. MySQL and MariaDB
    /// commit each DDL statement implicitly, CockroachDB commits the open
    /// transaction before each one (`autocommit_before_ddl`), and ClickHouse
    /// has no transactions, so there a failure leaves the statements before
    /// it applied.
    pub fn rolls_back_ddl(&self) -> bool {
        self.supports_sql()
            && !matches!(
                self.0,
                DatabaseType::MySQL | DatabaseType::CockroachDb | DatabaseType::ClickHouse
            )
    }

    /// Whether tables and columns carry comments the structure tab can set.
    /// SQLite stores none.
    pub fn supports_comments(&self) -> bool {
        self.supports_sql() && !matches!(self.0, DatabaseType::SQLite)
    }

    /// Whether some release of the backend has unique indexes that count
    /// NULLs as equal. See [`Self::supports_nulls_not_distinct`] for the
    /// server at hand.
    pub fn has_nulls_not_distinct(&self) -> bool {
        matches!(self.0, DatabaseType::PostgreSQL)
    }

    /// Whether a server reporting `server_version_num` (PostgreSQL's numeric
    /// version, `150004` for 15.4) takes `NULLS NOT DISTINCT`, added in 15.
    pub fn supports_nulls_not_distinct(&self, server_version_num: Option<u32>) -> bool {
        self.has_nulls_not_distinct()
            && server_version_num.is_some_and(|version| version >= 150_000)
    }

    /// Whether an index may cover only the rows matching a `WHERE` condition.
    pub fn supports_partial_indexes(&self) -> bool {
        matches!(
            self.0,
            DatabaseType::PostgreSQL
                | DatabaseType::CockroachDb
                | DatabaseType::SQLite
                | DatabaseType::MsSql
        )
    }

    /// The index access methods a new index may name, the backend's default
    /// first. ClickHouse lists its data skipping index types. Empty where
    /// the method cannot be chosen.
    pub fn index_methods(&self) -> &'static [&'static str] {
        match self.0 {
            DatabaseType::PostgreSQL => &["btree", "hash", "gist", "spgist", "gin", "brin"],
            DatabaseType::CockroachDb => &["btree", "gin", "gist"],
            DatabaseType::ClickHouse => &[
                "minmax",
                "set(100)",
                "bloom_filter",
                "tokenbf_v1(256, 2, 0)",
                "ngrambf_v1(3, 256, 2, 0)",
            ],
            DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::MsSql
            | DatabaseType::Redis => &[],
        }
    }

    /// The type and MySQL attributes of the `id` column a new table starts
    /// with: a key the database numbers itself where it can.
    pub fn new_table_key(&self) -> (&'static str, Option<&'static str>) {
        match self.0 {
            DatabaseType::PostgreSQL => ("bigint GENERATED BY DEFAULT AS IDENTITY", None),
            DatabaseType::CockroachDb => ("int8 DEFAULT unique_rowid()", None),
            DatabaseType::MySQL => ("bigint unsigned", Some("AUTO_INCREMENT")),
            DatabaseType::SQLite => ("INTEGER", None),
            DatabaseType::MsSql => ("bigint IDENTITY(1,1)", None),
            DatabaseType::ClickHouse => ("UInt64", None),
            DatabaseType::DuckDb => ("BIGINT", None),
            DatabaseType::Redis => ("", None),
        }
    }

    // ----- sidebar / editor facts -----

    /// Display label for the sidebar folder that groups a connection's primary
    /// objects. SQL backends list "Tables"; key/value stores list "Keys".
    pub fn primary_object_category_label(&self) -> &'static str {
        if self.supports_sql() {
            "Tables"
        } else {
            "Keys"
        }
    }

    /// Separator used to fold flat key names into a nested folder tree in the
    /// connections sidebar (Redis `user:1:name`). Relational backends have a
    /// flat table list and return `None`.
    pub fn key_namespace_separator(&self) -> Option<char> {
        match self.0 {
            DatabaseType::Redis => Some(':'),
            _ => None,
        }
    }

    /// Keywords completion offers only on the backends that understand them.
    /// Every keyword not listed here is offered everywhere.
    pub const BACKEND_KEYWORDS: &'static [&'static str] = &[
        "LIMIT",
        "RETURNING",
        "TOP",
        "OUTPUT",
        "FINAL",
        "SETTINGS",
        "ILIKE",
        "UPSERT",
    ];

    /// The entries of [`Self::BACKEND_KEYWORDS`] this backend understands.
    pub fn backend_keywords(&self) -> &'static [&'static str] {
        match self.0 {
            DatabaseType::PostgreSQL | DatabaseType::DuckDb => &["LIMIT", "RETURNING", "ILIKE"],
            DatabaseType::CockroachDb => &["LIMIT", "RETURNING", "ILIKE", "UPSERT"],
            DatabaseType::SQLite => &["LIMIT", "RETURNING"],
            DatabaseType::MySQL => &["LIMIT"],
            DatabaseType::ClickHouse => &["LIMIT", "FINAL", "SETTINGS", "ILIKE"],
            DatabaseType::MsSql => &["TOP", "OUTPUT"],
            DatabaseType::Redis => &[],
        }
    }

    /// Whether completion should offer `keyword` on this backend.
    pub fn offers_keyword(&self, keyword: &str) -> bool {
        !Self::BACKEND_KEYWORDS.contains(&keyword) || self.backend_keywords().contains(&keyword)
    }

    /// The SQL grammar for this backend. MySQL connections use MariaDB's,
    /// which reads MySQL's syntax plus MariaDB's additions. Redis speaks no
    /// SQL and gets the ANSI base for anything SQL-shaped around it.
    pub fn sql_grammar(&self) -> SqlGrammar {
        match self.0 {
            DatabaseType::PostgreSQL => SqlGrammar::PostgreSql,
            DatabaseType::CockroachDb => SqlGrammar::CockroachDb,
            DatabaseType::MySQL => SqlGrammar::MariaDb,
            DatabaseType::SQLite => SqlGrammar::Sqlite,
            DatabaseType::MsSql => SqlGrammar::TSql,
            DatabaseType::ClickHouse => SqlGrammar::ClickHouse,
            DatabaseType::DuckDb => SqlGrammar::DuckDb,
            DatabaseType::Redis => SqlGrammar::Ansi,
        }
    }

    /// Name of the registered editor language used for syntax highlighting.
    pub fn editor_language(&self) -> &'static str {
        match self.0 {
            DatabaseType::Redis => "redis",
            _ => self.sql_grammar().language_name(),
        }
    }

    /// The sqruff dialect used for formatting and linting.
    pub fn sqruff_dialect(&self) -> &'static str {
        match self.0 {
            DatabaseType::SQLite => "sqlite",
            DatabaseType::DuckDb => "duckdb",
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => "postgres",
            DatabaseType::MySQL => "mysql",
            DatabaseType::ClickHouse => "ansi",
            DatabaseType::MsSql => "tsql",
            // Redis is not SQL; sqruff is never invoked for it (gated by
            // supports_sql), but a dialect string is still required here.
            DatabaseType::Redis => "ansi",
        }
    }

    /// The sqlparser-rs dialect used for table extraction.
    pub fn sqlparser_dialect(&self) -> Box<dyn sqlparser::dialect::Dialect> {
        match self.0 {
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => Box::new(PostgreSqlDialect {}),
            DatabaseType::MySQL => Box::new(MySqlDialect {}),
            DatabaseType::MsSql => Box::new(MsSqlDialect {}),
            DatabaseType::DuckDb => Box::new(DuckDbDialect {}),
            DatabaseType::SQLite | DatabaseType::ClickHouse | DatabaseType::Redis => {
                Box::new(GenericDialect {})
            }
        }
    }

    /// The placeholder syntaxes this dialect recognizes as bind parameters.
    ///
    /// Note: `@name` in T-SQL is also used for local variable declarations
    /// (`DECLARE @x INT`), which share the parameter syntax. MySQL `@name` is a
    /// session variable, not a bind placeholder, so it is intentionally excluded.
    pub fn parameter_styles(&self) -> ParamStyles {
        match self.0 {
            DatabaseType::SQLite => ParamStyles::all(),
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => ParamStyles {
                dollar_number: true,
                ..ParamStyles::none()
            },
            DatabaseType::DuckDb => ParamStyles {
                question_mark: true,
                dollar_number: true,
                ..ParamStyles::none()
            },
            DatabaseType::MySQL => ParamStyles {
                question_mark: true,
                ..ParamStyles::none()
            },
            DatabaseType::MsSql => ParamStyles {
                at_named: true,
                ..ParamStyles::none()
            },
            DatabaseType::ClickHouse | DatabaseType::Redis => ParamStyles::none(),
        }
    }

    /// The placeholder to emit for the `index`th (0-based) bind parameter of a
    /// generated statement.
    pub fn placeholder(&self, index: usize) -> String {
        match self.0 {
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => format!("${}", index + 1),
            _ => "?".to_string(),
        }
    }

    /// Whether a bind parameter must be cast to its column's type
    /// (`$1::integer`), because the driver sends every value as text and
    /// the server will not compare text with a number.
    pub fn casts_bind_parameters(&self) -> bool {
        matches!(self.0, DatabaseType::PostgreSQL | DatabaseType::CockroachDb)
    }

    /// Whether unquoted identifiers fold to lower case, so a name with a
    /// capital letter is only found quoted.
    pub fn folds_unquoted_identifiers(&self) -> bool {
        matches!(self.0, DatabaseType::PostgreSQL | DatabaseType::CockroachDb)
    }

    /// Whether strings may be dollar quoted (`$$...$$`, `$tag$...$tag$`).
    pub fn supports_dollar_quotes(&self) -> bool {
        matches!(
            self.0,
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb | DatabaseType::DuckDb
        )
    }

    /// Schema that unqualified names resolve to when a tab has none:
    /// PostgreSQL `public`, SQL Server `dbo`. Backends whose "schema" is the
    /// database return `None` and callers fall back to the database name.
    pub fn default_schema(&self) -> Option<&'static str> {
        match self.0 {
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => Some("public"),
            DatabaseType::MsSql => Some("dbo"),
            DatabaseType::DuckDb => Some("main"),
            DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::ClickHouse
            | DatabaseType::Redis => None,
        }
    }

    /// Whether `schema` holds the server's own catalog rather than user
    /// objects, such as PostgreSQL's `pg_catalog`.
    pub fn is_system_schema(&self, schema: &str) -> bool {
        match self.0 {
            DatabaseType::PostgreSQL => {
                schema == "information_schema"
                    || schema == "pg_catalog"
                    || schema.starts_with("pg_toast")
                    || schema.starts_with("pg_temp")
            }
            DatabaseType::CockroachDb => matches!(
                schema,
                "information_schema" | "pg_catalog" | "pg_extension" | "crdb_internal"
            ),
            DatabaseType::MsSql => schema == "sys" || schema == "INFORMATION_SCHEMA",
            DatabaseType::DuckDb => schema == "information_schema" || schema == "pg_catalog",
            DatabaseType::MySQL => matches!(
                schema,
                "information_schema" | "mysql" | "performance_schema" | "sys"
            ),
            DatabaseType::ClickHouse => matches!(
                schema,
                "system" | "information_schema" | "INFORMATION_SCHEMA"
            ),
            DatabaseType::SQLite | DatabaseType::Redis => false,
        }
    }

    /// Conventional server port, 0 for file-based backends.
    pub fn default_port(&self) -> u16 {
        match self.0 {
            DatabaseType::PostgreSQL => 5432,
            DatabaseType::CockroachDb => 26257,
            DatabaseType::MySQL => 3306,
            DatabaseType::ClickHouse => 8123,
            DatabaseType::MsSql => 1433,
            DatabaseType::Redis => 6379,
            DatabaseType::SQLite | DatabaseType::DuckDb => 0,
        }
    }

    /// One sentence describing what statement text is for this backend, for
    /// the assistant's system prompt.
    pub fn command_syntax_description(&self) -> String {
        match self.0 {
            DatabaseType::Redis => format!(
                "The connection is {}, which does not speak SQL: the strings you pass to `db.query`, \
                 `db.execute` and `db.transaction` are command text in `redis-cli` syntax (e.g. \
                 `GET user:1`), one command per call, and bind parameters are unavailable.",
                self.0.as_str()
            ),
            _ => format!(
                "The connection is {}, so every statement string you pass to `db.query`, \
                 `db.execute` and `db.transaction` must be {} SQL.",
                self.0.as_str(),
                self.0.as_str()
            ),
        }
    }

    // ----- identifiers and literals -----

    /// Quote one identifier. Input is taken as a bare name; any quote
    /// characters in it are escaped, not interpreted.
    pub fn quote_identifier(&self, identifier: &str) -> String {
        match self.0 {
            DatabaseType::MySQL | DatabaseType::ClickHouse => {
                format!("`{}`", identifier.replace('`', "``"))
            }
            DatabaseType::MsSql => format!("[{}]", identifier.replace(']', "]]")),
            DatabaseType::PostgreSQL
            | DatabaseType::CockroachDb
            | DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::Redis => format!("\"{}\"", identifier.replace('"', "\"\"")),
        }
    }

    /// `schema.name` with both parts quoted, or just `name` when `schema` is
    /// absent or empty.
    pub fn quote_qualified(&self, schema: Option<&str>, name: &str) -> String {
        match schema.filter(|schema| !schema.is_empty()) {
            Some(schema) => format!(
                "{}.{}",
                self.quote_identifier(schema),
                self.quote_identifier(name)
            ),
            None => self.quote_identifier(name),
        }
    }

    /// Quote a possibly dotted, possibly already quoted path such as
    /// `public.users` or `` `db`.`t` ``: each segment is unquoted in this
    /// dialect's own style and requoted, so user-typed table names round trip.
    pub fn quote_path(&self, path: &str) -> String {
        self.path_segments(path)
            .iter()
            .map(|segment| self.quote_identifier(segment))
            .collect::<Vec<_>>()
            .join(".")
    }

    /// The identifiers of a dotted path such as `public.users` or
    /// `"Sales Data"."Orders"`, unquoted. Only dots outside quotes separate
    /// segments, and a doubled closing quote inside quotes is one character.
    /// Unquoted segments are taken as written.
    pub fn path_segments(&self, path: &str) -> Vec<String> {
        let (open, close) = self.identifier_quotes();
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut quoted = false;
        let mut chars = path.chars().peekable();
        while let Some(c) = chars.next() {
            if quoted {
                if c != close {
                    current.push(c);
                } else if chars.peek() == Some(&close) {
                    current.push(close);
                    chars.next();
                } else {
                    quoted = false;
                }
            } else if c == open {
                quoted = true;
            } else if c == '.' {
                segments.push(std::mem::take(&mut current));
            } else if !c.is_whitespace() {
                current.push(c);
            }
        }
        segments.push(current);
        segments
    }

    /// The schema and name of a table path: its last segment is the name,
    /// and the one before it, when there is one, the schema.
    pub fn split_table_path(&self, path: &str) -> (Option<String>, String) {
        let mut segments = self.path_segments(path);
        let name = segments.pop().unwrap_or_default();
        (segments.pop(), name)
    }

    /// The path segment for the identifier `name`: bare when the backend
    /// reads it back unquoted as the same identifier, quoted otherwise. A
    /// path built from these reads well in the UI and quotes back exactly.
    pub fn path_segment(&self, name: &str) -> String {
        let plain = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && self.unquoted_identifier(name) == name;
        if plain {
            name.to_string()
        } else {
            self.quote_identifier(name)
        }
    }

    /// The identifier an unquoted name stands for: PostgreSQL and
    /// CockroachDB fold it to lower case, the others keep it as written
    /// (MySQL, SQLite, SQL Server and DuckDB compare names without regard to
    /// case either way).
    pub fn unquoted_identifier(&self, name: &str) -> String {
        match self.0 {
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => name.to_lowercase(),
            DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql
            | DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::Redis => name.to_string(),
        }
    }

    fn identifier_quotes(&self) -> (char, char) {
        match self.0 {
            DatabaseType::MySQL | DatabaseType::ClickHouse => ('`', '`'),
            DatabaseType::MsSql => ('[', ']'),
            DatabaseType::PostgreSQL
            | DatabaseType::CockroachDb
            | DatabaseType::SQLite
            | DatabaseType::DuckDb
            | DatabaseType::Redis => ('"', '"'),
        }
    }

    /// Escape `value` for use inside a single-quoted string literal (without
    /// the surrounding quotes). MySQL treats backslashes as escapes unless
    /// `NO_BACKSLASH_ESCAPES` is set, and ClickHouse always does, so they are
    /// doubled there as well.
    pub fn escape_string_literal(&self, value: &str) -> String {
        match self.0 {
            DatabaseType::MySQL | DatabaseType::ClickHouse => {
                value.replace('\\', "\\\\").replace('\'', "''")
            }
            _ => value.replace('\'', "''"),
        }
    }

    /// `'value'` with [`Self::escape_string_literal`] applied.
    pub fn string_literal(&self, value: &str) -> String {
        format!("'{}'", self.escape_string_literal(value))
    }

    /// A literal for the bytes written as `hex` digits (no prefix), in the
    /// backend's own spelling.
    pub fn binary_literal(&self, hex: &str) -> String {
        match self.0 {
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => format!("'\\x{hex}'::bytea"),
            DatabaseType::MySQL | DatabaseType::SQLite => format!("X'{hex}'"),
            DatabaseType::MsSql => format!("0x{hex}"),
            DatabaseType::DuckDb => format!("from_hex('{hex}')"),
            DatabaseType::ClickHouse | DatabaseType::Redis => format!("unhex('{hex}')"),
        }
    }

    /// The literal for a cell's text in a column of `column_type`: the bytes
    /// of a binary column written as hex, a quoted string for everything
    /// else. Binary text that is not hex notation is sent as the string it is.
    pub fn value_literal(&self, value: &str, column_type: crate::ColumnType) -> String {
        if column_type == crate::ColumnType::Binary
            && let Some(hex) = crate::binary::hex_digits(value)
        {
            return self.binary_literal(&hex);
        }
        self.string_literal(value)
    }

    // ----- statement shapes -----

    /// The rows of `table` (a possibly schema-qualified path) whose `column`
    /// equals `value`, as for following a foreign key to the row it
    /// references. A numeric value is written bare when `numeric`, since
    /// ClickHouse does not compare a number column with a string.
    /// The hex notation of a `binary` column is compared as bytes.
    pub fn referenced_row_query(
        &self,
        table: &str,
        column: &str,
        value: &str,
        numeric: bool,
        binary: bool,
    ) -> String {
        let number = value.trim();
        let is_number = number
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
            && number.parse::<f64>().is_ok();
        let literal = if numeric && is_number {
            number.to_string()
        } else if binary {
            self.value_literal(value, crate::ColumnType::Binary)
        } else {
            self.string_literal(value)
        };
        format!(
            "SELECT * FROM {} WHERE {} = {}",
            self.quote_path(table),
            self.quote_identifier(column),
            literal
        )
    }

    /// Row limiting for a `SELECT`: SQL Server has no `LIMIT`, so the limit
    /// becomes `TOP n` right after `SELECT` (or `SELECT DISTINCT`); every other
    /// backend gets a trailing `LIMIT n`. `sql` is returned unchanged when it
    /// already carries a limit or is not a plain select.
    pub fn apply_row_limit(&self, sql: &str, limit: usize) -> String {
        if self.has_row_limit(sql) {
            return sql.to_string();
        }
        let trimmed = sql.trim_end().trim_end_matches(';').trim_end();
        match self.0 {
            DatabaseType::MsSql => {
                let mut words = trimmed.split_whitespace();
                let Some(select) = words
                    .next()
                    .filter(|word| word.eq_ignore_ascii_case("SELECT"))
                else {
                    return sql.to_string();
                };
                let rest = trimmed[select.len()..].trim_start();
                match rest.split_whitespace().next() {
                    Some(distinct) if distinct.eq_ignore_ascii_case("DISTINCT") => format!(
                        "{select} {distinct} TOP {limit} {}",
                        rest[distinct.len()..].trim_start()
                    ),
                    _ => format!("{select} TOP {limit} {rest}"),
                }
            }
            _ => format!("{trimmed}\nLIMIT {limit}"),
        }
    }

    /// Whether `sql` already bounds its result (a `LIMIT`, or `TOP` on SQL
    /// Server) or is not a `SELECT` at all.
    pub fn has_row_limit(&self, sql: &str) -> bool {
        let mut words = sql
            .split_whitespace()
            .map(|word| word.trim_end_matches(';').to_ascii_uppercase());
        let Some(first) = words.next() else {
            return true;
        };
        if first != "SELECT" {
            return true;
        }
        match self.0 {
            DatabaseType::MsSql => words.any(|word| word == "TOP" || word == "FETCH"),
            _ => words.any(|word| word == "LIMIT"),
        }
    }

    /// The `SELECT *` scaffold opened for a table from the sidebar.
    pub fn select_scaffold(&self, schema: Option<&str>, table: &str, limit: usize) -> String {
        let target = match schema.filter(|schema| !schema.is_empty()) {
            Some(schema) => format!("{schema}.{table}"),
            None => table.to_string(),
        };
        format!(
            "{};",
            self.apply_row_limit(&format!("SELECT * FROM {target}"), limit)
        )
    }

    /// Whether servers of this type differ in syntax, so the connection's
    /// [`ServerVariant`] must be asked before building their SQL.
    pub fn has_server_variants(&self) -> bool {
        matches!(self.0, DatabaseType::MySQL)
    }

    /// Whether the backend can execute a statement and report its actual
    /// plan. SQLite and ClickHouse only estimate.
    pub fn supports_explain_analyze(&self) -> bool {
        matches!(
            self.0,
            DatabaseType::PostgreSQL
                | DatabaseType::CockroachDb
                | DatabaseType::MySQL
                | DatabaseType::MsSql
                | DatabaseType::DuckDb
        )
    }

    /// Wrap `sql` in this backend's EXPLAIN, or `None` when the backend has
    /// no such plan (Redis, or [`ExplainMode::Analyze`] where
    /// [`Self::supports_explain_analyze`] is false). In
    /// [`ExplainMode::Analyze`] the statement runs, writes included.
    pub fn explain(&self, sql: &str, mode: ExplainMode, variant: ServerVariant) -> Option<String> {
        let trimmed = sql.trim().trim_end_matches(';');
        let analyze = mode == ExplainMode::Analyze;
        if analyze && !self.supports_explain_analyze() {
            return None;
        }
        let wrapped = match self.0 {
            DatabaseType::PostgreSQL if analyze => {
                format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {trimmed}")
            }
            DatabaseType::PostgreSQL => format!("EXPLAIN (FORMAT JSON) {trimmed}"),
            // CockroachDB has no JSON plan, only its indented text tree.
            DatabaseType::CockroachDb if analyze => format!("EXPLAIN ANALYZE {trimmed}"),
            DatabaseType::CockroachDb => format!("EXPLAIN {trimmed}"),
            DatabaseType::MySQL => match (variant, analyze) {
                (ServerVariant::MariaDb, true) => format!("ANALYZE {trimmed}"),
                (ServerVariant::MariaDb, false) => format!("EXPLAIN {trimmed}"),
                // EXPLAIN ANALYZE prints the tree format with actual values.
                (ServerVariant::Standard, true) => format!("EXPLAIN ANALYZE {trimmed}"),
                (ServerVariant::Standard, false) => format!("EXPLAIN FORMAT=TREE {trimmed}"),
            },
            DatabaseType::MsSql if analyze => {
                format!("SET STATISTICS PROFILE ON; {trimmed}; SET STATISTICS PROFILE OFF")
            }
            // SHOWPLAN must be alone in its batch, and the driver splits
            // batches on `GO` lines.
            DatabaseType::MsSql => {
                format!("SET SHOWPLAN_ALL ON\nGO\n{trimmed}\nGO\nSET SHOWPLAN_ALL OFF")
            }
            DatabaseType::SQLite => format!("EXPLAIN QUERY PLAN {trimmed}"),
            DatabaseType::DuckDb if analyze => format!("EXPLAIN ANALYZE {trimmed}"),
            DatabaseType::DuckDb => format!("EXPLAIN {trimmed}"),
            DatabaseType::ClickHouse => format!("EXPLAIN PLAN {trimmed}"),
            DatabaseType::Redis => return None,
        };
        Some(wrapped)
    }

    /// Parse the result of [`Self::explain`] into a plan tree, or `None` when
    /// the backend's output is not structured (or did not parse).
    pub fn parse_explain_plan(&self, result: &QueryResult) -> Option<explain_plan::PlanTree> {
        match self.0 {
            DatabaseType::PostgreSQL => explain_plan::parse_postgres(result),
            DatabaseType::CockroachDb => explain_plan::parse_cockroachdb(result),
            DatabaseType::SQLite => explain_plan::parse_sqlite(result),
            DatabaseType::MySQL => explain_plan::parse_mysql_tree(result),
            DatabaseType::DuckDb
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql
            | DatabaseType::Redis => None,
        }
    }

    pub fn upsert_style(&self) -> UpsertStyle {
        match self.0 {
            DatabaseType::MySQL => UpsertStyle::OnDuplicateKey,
            _ => UpsertStyle::OnConflict,
        }
    }

    pub fn values_query_style(&self) -> ValuesQueryStyle {
        ValuesQueryStyle {
            as_cte: matches!(self.0, DatabaseType::SQLite),
            row_keyword: matches!(self.0, DatabaseType::MySQL),
        }
    }

    /// Whether a new row's untouched primary key is inserted as an explicit
    /// NULL instead of being omitted. SQLite has no column defaults for
    /// generated keys: `INTEGER PRIMARY KEY` assigns a rowid when NULL is
    /// inserted, which is also how other SQLite editors spell it.
    pub fn generated_key_is_null(&self) -> bool {
        matches!(self.0, DatabaseType::SQLite)
    }

    /// An insert that takes every column's default. MySQL has no
    /// `DEFAULT VALUES` form, everything else has no `()` form.
    pub fn insert_defaults_sql(&self, quoted_table: &str) -> String {
        match self.0 {
            DatabaseType::MySQL => format!("INSERT INTO {quoted_table} () VALUES ()"),
            _ => format!("INSERT INTO {quoted_table} DEFAULT VALUES"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialect(db_type: DatabaseType) -> Dialect {
        db_type.dialect()
    }

    #[test]
    fn client_tools_and_system_schemas() {
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).client_tools(),
            Some(ClientTools::PostgreSql)
        );
        assert_eq!(
            dialect(DatabaseType::MySQL).client_tools(),
            Some(ClientTools::MySql)
        );
        assert_eq!(dialect(DatabaseType::SQLite).client_tools(), None);
        assert_eq!(dialect(DatabaseType::Redis).client_tools(), None);
        let postgres = dialect(DatabaseType::PostgreSQL);
        assert!(postgres.is_system_schema("pg_catalog"));
        assert!(postgres.is_system_schema("pg_toast_temp_1"));
        assert!(!postgres.is_system_schema("public"));
    }

    #[test]
    fn referenced_row_query_quotes_for_the_dialect() {
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).referenced_row_query(
                "public.users",
                "id",
                "42",
                true,
                false
            ),
            r#"SELECT * FROM "public"."users" WHERE "id" = 42"#
        );
        assert_eq!(
            dialect(DatabaseType::MySQL)
                .referenced_row_query("users", "code", "O'Brien", false, false),
            "SELECT * FROM `users` WHERE `code` = 'O''Brien'"
        );
        // Not a number after all: quoted rather than spliced in.
        assert_eq!(
            dialect(DatabaseType::MsSql)
                .referenced_row_query("users", "id", "1; DROP", true, false),
            "SELECT * FROM [users] WHERE [id] = '1; DROP'"
        );
        assert_eq!(
            dialect(DatabaseType::SQLite).referenced_row_query("t", "x", "NaN", true, false),
            r#"SELECT * FROM "t" WHERE "x" = 'NaN'"#
        );
        assert_eq!(
            dialect(DatabaseType::MySQL).referenced_row_query("t", "id", "0xCAFE", false, true),
            "SELECT * FROM `t` WHERE `id` = X'cafe'"
        );
    }

    #[test]
    fn all_is_indexed_in_order() {
        for (index, db_type) in DatabaseType::ALL.iter().enumerate() {
            assert_eq!(db_type.index(), index);
        }
    }

    #[test]
    fn quoting_per_backend() {
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).quote_identifier("we\"ird"),
            "\"we\"\"ird\""
        );
        assert_eq!(
            dialect(DatabaseType::MySQL).quote_identifier("a`b"),
            "`a``b`"
        );
        assert_eq!(
            dialect(DatabaseType::ClickHouse).quote_identifier("t"),
            "`t`"
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).quote_identifier("a]b"),
            "[a]]b]"
        );
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).quote_qualified(Some("public"), "users"),
            "\"public\".\"users\""
        );
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).quote_qualified(Some(""), "users"),
            "\"users\""
        );
    }

    #[test]
    fn quote_path_round_trips_prequoted_segments() {
        assert_eq!(
            dialect(DatabaseType::MySQL).quote_path("`db`.users"),
            "`db`.`users`"
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).quote_path("[dbo].[t]"),
            "[dbo].[t]"
        );
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).quote_path(" public . \"Users\" "),
            "\"public\".\"Users\""
        );
    }

    #[test]
    fn paths_split_only_outside_quotes() {
        let postgres = dialect(DatabaseType::PostgreSQL);
        assert_eq!(
            postgres.path_segments(r#""Sales.Data"."Or""ders""#),
            ["Sales.Data", "Or\"ders"]
        );
        assert_eq!(
            postgres.quote_path(r#""Sales.Data"."Or""ders""#),
            r#""Sales.Data"."Or""ders""#
        );
        assert_eq!(
            postgres.split_table_path("public.users"),
            (Some("public".to_string()), "users".to_string())
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).split_table_path("[db].[dbo].[t]"),
            (Some("dbo".to_string()), "t".to_string())
        );
        assert_eq!(
            postgres.split_table_path("users"),
            (None, "users".to_string())
        );
    }

    #[test]
    fn path_segments_quote_what_would_not_read_back_bare() {
        let postgres = dialect(DatabaseType::PostgreSQL);
        assert_eq!(postgres.path_segment("orders"), "orders");
        assert_eq!(postgres.path_segment("Orders"), "\"Orders\"");
        assert_eq!(postgres.path_segment("sales data"), "\"sales data\"");
        assert_eq!(
            dialect(DatabaseType::MySQL).path_segment("Orders"),
            "Orders"
        );
        assert_eq!(dialect(DatabaseType::MsSql).path_segment("1st"), "[1st]");
    }

    #[test]
    fn string_literals_escape_per_backend() {
        assert_eq!(
            dialect(DatabaseType::MySQL).string_literal("a\\b'c"),
            "'a\\\\b''c'"
        );
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).string_literal("a\\b'c"),
            "'a\\b''c'"
        );
    }

    #[test]
    fn placeholders() {
        assert_eq!(dialect(DatabaseType::PostgreSQL).placeholder(0), "$1");
        assert_eq!(dialect(DatabaseType::MySQL).placeholder(3), "?");
    }

    #[test]
    fn row_limits() {
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).apply_row_limit("select * from t;", 100),
            "select * from t\nLIMIT 100"
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).apply_row_limit("SELECT * FROM t", 100),
            "SELECT TOP 100 * FROM t"
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).apply_row_limit("select distinct a from t", 5),
            "select distinct TOP 5 a from t"
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).apply_row_limit("SELECT TOP 3 * FROM t", 100),
            "SELECT TOP 3 * FROM t"
        );
        assert_eq!(
            dialect(DatabaseType::MySQL).apply_row_limit("SELECT * FROM t LIMIT 5", 100),
            "SELECT * FROM t LIMIT 5"
        );
        assert_eq!(
            dialect(DatabaseType::MySQL).apply_row_limit("UPDATE t SET a = 1", 100),
            "UPDATE t SET a = 1"
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).select_scaffold(Some("dbo"), "t", 100),
            "SELECT TOP 100 * FROM dbo.t;"
        );
        assert_eq!(
            dialect(DatabaseType::SQLite).select_scaffold(None, "t", 100),
            "SELECT * FROM t\nLIMIT 100;"
        );
    }

    #[test]
    fn explain_per_backend() {
        use ExplainMode::{Analyze, Estimated};
        use ServerVariant::{MariaDb, Standard};
        let explain = |db_type: DatabaseType, sql: &str, mode, variant| {
            dialect(db_type).explain(sql, mode, variant)
        };
        let cases = [
            (
                DatabaseType::PostgreSQL,
                Estimated,
                Standard,
                Some("EXPLAIN (FORMAT JSON) SELECT 1"),
            ),
            (
                DatabaseType::PostgreSQL,
                Analyze,
                Standard,
                Some("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 1"),
            ),
            (
                DatabaseType::MySQL,
                Estimated,
                Standard,
                Some("EXPLAIN FORMAT=TREE SELECT 1"),
            ),
            (
                DatabaseType::MySQL,
                Analyze,
                Standard,
                Some("EXPLAIN ANALYZE SELECT 1"),
            ),
            (
                DatabaseType::MySQL,
                Estimated,
                MariaDb,
                Some("EXPLAIN SELECT 1"),
            ),
            (
                DatabaseType::MySQL,
                Analyze,
                MariaDb,
                Some("ANALYZE SELECT 1"),
            ),
            (
                DatabaseType::MsSql,
                Estimated,
                Standard,
                Some("SET SHOWPLAN_ALL ON\nGO\nSELECT 1\nGO\nSET SHOWPLAN_ALL OFF"),
            ),
            (
                DatabaseType::MsSql,
                Analyze,
                Standard,
                Some("SET STATISTICS PROFILE ON; SELECT 1; SET STATISTICS PROFILE OFF"),
            ),
            (
                DatabaseType::SQLite,
                Estimated,
                Standard,
                Some("EXPLAIN QUERY PLAN SELECT 1"),
            ),
            (DatabaseType::SQLite, Analyze, Standard, None),
            (
                DatabaseType::DuckDb,
                Estimated,
                Standard,
                Some("EXPLAIN SELECT 1"),
            ),
            (
                DatabaseType::DuckDb,
                Analyze,
                Standard,
                Some("EXPLAIN ANALYZE SELECT 1"),
            ),
            (
                DatabaseType::ClickHouse,
                Estimated,
                Standard,
                Some("EXPLAIN PLAN SELECT 1"),
            ),
            (DatabaseType::ClickHouse, Analyze, Standard, None),
            (DatabaseType::Redis, Estimated, Standard, None),
        ];
        for (db_type, mode, variant, expected) in cases {
            assert_eq!(
                explain(db_type, "SELECT 1;", mode, variant).as_deref(),
                expected,
                "{db_type:?} {mode:?} {variant:?}"
            );
        }
        assert_eq!(
            explain(
                DatabaseType::PostgreSQL,
                "DELETE FROM t",
                Estimated,
                Standard
            )
            .as_deref(),
            Some("EXPLAIN (FORMAT JSON) DELETE FROM t"),
            "an estimate never runs the statement"
        );
        assert_eq!(
            ServerVariant::from_mysql_version("11.4.2-MariaDB-ubu2404"),
            MariaDb
        );
        assert_eq!(ServerVariant::from_mysql_version("8.4.0"), Standard);
    }

    #[test]
    fn capabilities_and_defaults() {
        assert!(!dialect(DatabaseType::Redis).supports_sql());
        assert!(!dialect(DatabaseType::MsSql).supports_create_table_ddl());
        assert!(!dialect(DatabaseType::ClickHouse).rows_editable());
        assert!(dialect(DatabaseType::PostgreSQL).rows_editable());
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).default_schema(),
            Some("public")
        );
        assert_eq!(dialect(DatabaseType::MsSql).default_schema(), Some("dbo"));
        assert_eq!(dialect(DatabaseType::MySQL).default_schema(), None);
        assert_eq!(dialect(DatabaseType::ClickHouse).default_port(), 8123);
        assert_eq!(
            dialect(DatabaseType::MySQL).upsert_style(),
            UpsertStyle::OnDuplicateKey
        );
        assert_eq!(
            dialect(DatabaseType::MySQL).insert_defaults_sql("`t`"),
            "INSERT INTO `t` () VALUES ()"
        );
        assert_eq!(
            dialect(DatabaseType::SQLite).insert_defaults_sql("\"t\""),
            "INSERT INTO \"t\" DEFAULT VALUES"
        );
    }
}
