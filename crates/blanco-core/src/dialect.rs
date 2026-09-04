//! Every per-backend SQL fact in one place: quoting, literal escaping,
//! placeholders, row limiting, EXPLAIN, upsert spelling, capabilities and the
//! odd defaults (schema, port). The UI, transformers and import code ask a
//! [`Dialect`] instead of matching on [`DatabaseType`] themselves, so adding a
//! backend means filling in one arm per method here rather than hunting down
//! two dozen scattered matches.

use crate::{DatabaseType, ParamStyles, QueryResult, explain_plan};
use sqlparser::dialect::{GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect};

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

impl DatabaseType {
    /// Every supported backend, in the order the connection dialog lists
    /// them. Index into it with [`DatabaseType::index`].
    pub const ALL: [DatabaseType; 6] = [
        DatabaseType::SQLite,
        DatabaseType::PostgreSQL,
        DatabaseType::MySQL,
        DatabaseType::ClickHouse,
        DatabaseType::MsSql,
        DatabaseType::Redis,
    ];

    /// Position in [`DatabaseType::ALL`].
    pub fn index(&self) -> usize {
        match self {
            DatabaseType::SQLite => 0,
            DatabaseType::PostgreSQL => 1,
            DatabaseType::MySQL => 2,
            DatabaseType::ClickHouse => 3,
            DatabaseType::MsSql => 4,
            DatabaseType::Redis => 5,
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
        self.supports_sql()
    }

    /// Whether switching databases through the connection string is
    /// meaningful. A SQLite connection is one file.
    pub fn supports_database_switching(&self) -> bool {
        !matches!(self.0, DatabaseType::SQLite)
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

    /// Whether column and index DDL (ALTER TABLE) is offered from the structure
    /// tab.
    pub fn supports_column_ddl(&self) -> bool {
        self.supports_sql()
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

    /// Name of the registered editor language used for syntax highlighting.
    pub fn editor_language(&self) -> &'static str {
        match self.0 {
            DatabaseType::Redis => "redis",
            _ => "sql",
        }
    }

    /// The sqruff dialect used for formatting and linting.
    pub fn sqruff_dialect(&self) -> &'static str {
        match self.0 {
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

    /// The sqlparser-rs dialect used for table extraction.
    pub fn sqlparser_dialect(&self) -> Box<dyn sqlparser::dialect::Dialect> {
        match self.0 {
            DatabaseType::PostgreSQL => Box::new(PostgreSqlDialect {}),
            DatabaseType::MySQL => Box::new(MySqlDialect {}),
            DatabaseType::MsSql => Box::new(MsSqlDialect {}),
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
            DatabaseType::PostgreSQL => ParamStyles {
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
            DatabaseType::PostgreSQL => format!("${}", index + 1),
            _ => "?".to_string(),
        }
    }

    /// Schema that unqualified names resolve to when a tab has none:
    /// PostgreSQL `public`, SQL Server `dbo`. Backends whose "schema" is the
    /// database return `None` and callers fall back to the database name.
    pub fn default_schema(&self) -> Option<&'static str> {
        match self.0 {
            DatabaseType::PostgreSQL => Some("public"),
            DatabaseType::MsSql => Some("dbo"),
            DatabaseType::MySQL
            | DatabaseType::SQLite
            | DatabaseType::ClickHouse
            | DatabaseType::Redis => None,
        }
    }

    /// Conventional server port, 0 for file-based backends.
    pub fn default_port(&self) -> u16 {
        match self.0 {
            DatabaseType::PostgreSQL => 5432,
            DatabaseType::MySQL => 3306,
            DatabaseType::ClickHouse => 8123,
            DatabaseType::MsSql => 1433,
            DatabaseType::Redis => 6379,
            DatabaseType::SQLite => 0,
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
            DatabaseType::PostgreSQL | DatabaseType::SQLite | DatabaseType::Redis => {
                format!("\"{}\"", identifier.replace('"', "\"\""))
            }
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
        let (open, close) = self.identifier_quotes();
        path.split('.')
            .map(|part| {
                let part = part.trim();
                let bare = part
                    .strip_prefix(open)
                    .and_then(|value| value.strip_suffix(close))
                    .unwrap_or(part);
                self.quote_identifier(bare)
            })
            .collect::<Vec<_>>()
            .join(".")
    }

    fn identifier_quotes(&self) -> (char, char) {
        match self.0 {
            DatabaseType::MySQL | DatabaseType::ClickHouse => ('`', '`'),
            DatabaseType::MsSql => ('[', ']'),
            DatabaseType::PostgreSQL | DatabaseType::SQLite | DatabaseType::Redis => ('"', '"'),
        }
    }

    /// Escape `value` for use inside a single-quoted string literal (without
    /// the surrounding quotes). MySQL treats backslashes as escapes unless
    /// `NO_BACKSLASH_ESCAPES` is set, so they are doubled there as well.
    pub fn escape_string_literal(&self, value: &str) -> String {
        match self.0 {
            DatabaseType::MySQL => value.replace('\\', "\\\\").replace('\'', "''"),
            _ => value.replace('\'', "''"),
        }
    }

    /// `'value'` with [`Self::escape_string_literal`] applied.
    pub fn string_literal(&self, value: &str) -> String {
        format!("'{}'", self.escape_string_literal(value))
    }

    // ----- statement shapes -----

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

    /// Wrap `sql` in this backend's EXPLAIN. `EXPLAIN ANALYZE` is only used
    /// for plain selects on Postgres because `ANALYZE` executes the statement
    /// and would mutate data otherwise.
    pub fn explain(&self, sql: &str) -> String {
        let trimmed = sql.trim().trim_end_matches(';');
        let is_select = trimmed
            .split_whitespace()
            .next()
            .map(|word| word.eq_ignore_ascii_case("SELECT") || word.eq_ignore_ascii_case("WITH"))
            .unwrap_or(false);
        match self.0 {
            DatabaseType::PostgreSQL if is_select => {
                format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {trimmed}")
            }
            DatabaseType::PostgreSQL => format!("EXPLAIN (FORMAT JSON) {trimmed}"),
            DatabaseType::MySQL => format!("EXPLAIN FORMAT=TREE {trimmed}"),
            DatabaseType::MsSql => {
                format!("SET SHOWPLAN_ALL ON; {trimmed}; SET SHOWPLAN_ALL OFF")
            }
            DatabaseType::SQLite => format!("EXPLAIN QUERY PLAN {trimmed}"),
            DatabaseType::ClickHouse => format!("EXPLAIN PLAN {trimmed}"),
            // Redis has no EXPLAIN; the action is gated off by supports_sql.
            DatabaseType::Redis => trimmed.to_string(),
        }
    }

    /// Parse the result of [`Self::explain`] into a plan tree, or `None` when
    /// the backend's output is not structured (or did not parse).
    pub fn parse_explain_plan(&self, result: &QueryResult) -> Option<explain_plan::PlanTree> {
        match self.0 {
            DatabaseType::PostgreSQL => explain_plan::parse_postgres(result),
            DatabaseType::SQLite => explain_plan::parse_sqlite(result),
            DatabaseType::MySQL => explain_plan::parse_mysql_tree(result),
            DatabaseType::ClickHouse | DatabaseType::MsSql | DatabaseType::Redis => None,
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
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).explain("SELECT 1;"),
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 1"
        );
        assert_eq!(
            dialect(DatabaseType::PostgreSQL).explain("DELETE FROM t"),
            "EXPLAIN (FORMAT JSON) DELETE FROM t"
        );
        assert_eq!(
            dialect(DatabaseType::MySQL).explain("SELECT 1"),
            "EXPLAIN FORMAT=TREE SELECT 1"
        );
        assert_eq!(
            dialect(DatabaseType::MsSql).explain("SELECT 1"),
            "SET SHOWPLAN_ALL ON; SELECT 1; SET SHOWPLAN_ALL OFF"
        );
        assert_eq!(
            dialect(DatabaseType::SQLite).explain("SELECT 1"),
            "EXPLAIN QUERY PLAN SELECT 1"
        );
        assert_eq!(
            dialect(DatabaseType::ClickHouse).explain("SELECT 1"),
            "EXPLAIN PLAN SELECT 1"
        );
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
