//! Conservative classification of statements into reads and writes, used to
//! enforce read-only connections and to confirm writes on production
//! connections. The classifier errs on the side of "write": anything it does
//! not positively recognise as a read is treated as a write, so an unknown
//! statement is blocked on a read-only connection rather than let through.

use crate::DatabaseType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementAccess {
    Read,
    Write,
}

/// Leading keywords that never modify data. `WITH` is handled separately
/// because a CTE can feed an INSERT/UPDATE/DELETE.
const SQL_READ_KEYWORDS: &[&str] = &[
    "SELECT",
    "SHOW",
    "EXPLAIN",
    "DESCRIBE",
    "DESC",
    "VALUES",
    "TABLE",
    "USE",
    "BEGIN",
    "START",
    "COMMIT",
    "ROLLBACK",
    "END",
    "SET",
    "RESET",
    "DISCARD",
    "CHECKPOINT",
    "ANALYZE",
    "EXISTS",
    // DuckDB's reads: a query may start with its FROM clause, and SUMMARIZE,
    // PIVOT and UNPIVOT are queries.
    "FROM",
    "SUMMARIZE",
    "PIVOT",
    "UNPIVOT",
];

/// Keywords that mark a data-modifying CTE body.
const SQL_WRITE_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT", "CREATE", "ALTER", "DROP",
    "TRUNCATE", "COPY", "IMPORT",
];

/// Redis commands that only read. Everything else (SET, DEL, HSET, FLUSHALL,
/// CONFIG SET, EVAL, ...) counts as a write.
const REDIS_READ_COMMANDS: &[&str] = &[
    "GET",
    "MGET",
    "GETRANGE",
    "STRLEN",
    "GETBIT",
    "BITCOUNT",
    "BITPOS",
    "EXISTS",
    "TYPE",
    "TTL",
    "PTTL",
    "EXPIRETIME",
    "PEXPIRETIME",
    "KEYS",
    "SCAN",
    "RANDOMKEY",
    "DBSIZE",
    "INFO",
    "PING",
    "ECHO",
    "TIME",
    "SELECT",
    "DUMP",
    "OBJECT",
    "MEMORY",
    "LASTSAVE",
    "ROLE",
    "LOLWUT",
    "HELLO",
    "AUTH",
    "HGET",
    "HMGET",
    "HGETALL",
    "HKEYS",
    "HVALS",
    "HLEN",
    "HEXISTS",
    "HSCAN",
    "HSTRLEN",
    "HRANDFIELD",
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LPOS",
    "SMEMBERS",
    "SISMEMBER",
    "SMISMEMBER",
    "SCARD",
    "SSCAN",
    "SRANDMEMBER",
    "SINTER",
    "SINTERCARD",
    "SUNION",
    "SDIFF",
    "ZRANGE",
    "ZRANGEBYSCORE",
    "ZREVRANGE",
    "ZREVRANGEBYSCORE",
    "ZRANGEBYLEX",
    "ZREVRANGEBYLEX",
    "ZCARD",
    "ZCOUNT",
    "ZLEXCOUNT",
    "ZSCORE",
    "ZMSCORE",
    "ZRANK",
    "ZREVRANK",
    "ZSCAN",
    "ZRANDMEMBER",
    "ZINTER",
    "ZINTERCARD",
    "ZUNION",
    "ZDIFF",
    "XRANGE",
    "XREVRANGE",
    "XLEN",
    "XREAD",
    "XINFO",
    "XPENDING",
    "PFCOUNT",
    "GEOPOS",
    "GEODIST",
    "GEOHASH",
    "GEOSEARCH",
    "GEORADIUS_RO",
    "GEORADIUSBYMEMBER_RO",
    "COMMAND",
    "CLIENT",
    "CONFIG",
    "SLOWLOG",
    "LATENCY",
    "MONITOR",
    "WAIT",
    "TOUCH",
    "SORT_RO",
    "BITFIELD_RO",
    "EVAL_RO",
    "EVALSHA_RO",
    "FCALL_RO",
    "SUBSCRIBE",
    "PSUBSCRIBE",
    "SSUBSCRIBE",
    "PUBSUB",
];

/// Sub-commands of otherwise read-only container commands that do write.
const REDIS_WRITE_SUBCOMMANDS: &[(&str, &str)] = &[
    ("CONFIG", "SET"),
    ("CONFIG", "REWRITE"),
    ("CONFIG", "RESETSTAT"),
    ("CLIENT", "KILL"),
    ("CLIENT", "PAUSE"),
    ("CLIENT", "SETNAME"),
    ("CLIENT", "UNBLOCK"),
    ("MEMORY", "PURGE"),
    ("OBJECT", "FREQ"),
    ("SLOWLOG", "RESET"),
    ("LATENCY", "RESET"),
    ("XINFO", "CONSUMERS"),
];

pub fn classify(driver: DatabaseType, text: &str) -> StatementAccess {
    if driver.dialect().supports_sql() {
        classify_sql(text)
    } else {
        classify_redis(text)
    }
}

/// Classify a SQL script: the result is `Write` if any statement in it writes.
pub fn classify_sql(sql: &str) -> StatementAccess {
    if executed_statements(sql)
        .iter()
        .any(|statement| sql_write_kind(statement).is_some())
    {
        StatementAccess::Write
    } else {
        StatementAccess::Read
    }
}

/// Classify a Redis command line (one command per line).
pub fn classify_redis(text: &str) -> StatementAccess {
    if text.lines().any(|line| redis_write_kind(line).is_some()) {
        StatementAccess::Write
    } else {
        StatementAccess::Read
    }
}

/// The distinct kinds of write in `text`, in order of appearance: the leading
/// keyword of each writing SQL statement (`INSERT`, `CREATE`, `DO`, ...), the
/// write keyword inside a data-modifying CTE, or the Redis command (`SET`,
/// `CONFIG SET`). Empty when `text` only reads.
pub fn write_kinds(driver: DatabaseType, text: &str) -> Vec<String> {
    let kinds: Vec<String> = if driver.dialect().supports_sql() {
        executed_statements(text)
            .iter()
            .filter_map(|statement| sql_write_kind(statement))
            .collect()
    } else {
        text.lines().filter_map(redis_write_kind).collect()
    };
    let mut distinct = Vec::with_capacity(kinds.len());
    for kind in kinds {
        if !distinct.contains(&kind) {
            distinct.push(kind);
        }
    }
    distinct
}

/// The statements of `sql` that the server executes. SQL Server only
/// compiles the statements between `SET SHOWPLAN_ALL ON` (or `_TEXT`, `_XML`)
/// and the matching `OFF`, to show their estimated plan.
fn executed_statements(sql: &str) -> Vec<String> {
    let mut showplan = false;
    split_statements(sql)
        .into_iter()
        .filter(|statement| {
            let words = keywords(statement);
            let switches_showplan = words.len() == 3
                && words[0] == "SET"
                && matches!(
                    words[1].as_str(),
                    "SHOWPLAN_ALL" | "SHOWPLAN_TEXT" | "SHOWPLAN_XML"
                );
            if switches_showplan {
                showplan = words[2] == "ON";
                return true;
            }
            !showplan
        })
        .collect()
}

fn sql_write_kind(statement: &str) -> Option<String> {
    let words = keywords(statement);
    match words.first()?.as_str() {
        // `PRAGMA name` and `PRAGMA name(arg)` (table_info, index_list)
        // read, `PRAGMA name = value` writes. SQLite also accepts the
        // parenthesised form for setting, but it is rare in practice.
        "PRAGMA" => statement.contains('=').then(|| "PRAGMA".to_string()),
        _ => words_write_kind(&words),
    }
}

fn words_write_kind(words: &[String]) -> Option<String> {
    let first = words.first()?;
    match first.as_str() {
        "WITH" => words
            .iter()
            .find(|word| SQL_WRITE_KEYWORDS.contains(&word.as_str()))
            .cloned(),
        // `EXPLAIN ANALYZE`, `EXPLAIN (ANALYZE, ...)` and MariaDB's
        // `ANALYZE <statement>` execute the statement they explain, so they
        // write when it does. `ANALYZE table` only gathers statistics.
        "EXPLAIN" | "ANALYZE" => {
            let verb = words
                .iter()
                .skip(1)
                .position(|word| is_statement_verb(word))?
                + 1;
            let executes =
                first == "ANALYZE" || words[1..verb].iter().any(|word| word == "ANALYZE");
            if executes {
                words_write_kind(&words[verb..])
            } else {
                None
            }
        }
        keyword => (!SQL_READ_KEYWORDS.contains(&keyword)).then(|| first.clone()),
    }
}

/// A keyword that starts the statement an `EXPLAIN` or `ANALYZE` wraps.
fn is_statement_verb(word: &str) -> bool {
    SQL_WRITE_KEYWORDS.contains(&word)
        || matches!(
            word,
            "SELECT" | "WITH" | "VALUES" | "TABLE" | "FROM" | "EXECUTE"
        )
}

fn redis_write_kind(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    let command = parts.next()?.to_ascii_uppercase();
    if !REDIS_READ_COMMANDS.contains(&command.as_str()) {
        return Some(command);
    }
    let subcommand = parts.next()?.to_ascii_uppercase();
    REDIS_WRITE_SUBCOMMANDS
        .iter()
        .any(|(c, s)| *c == command && *s == subcommand)
        .then(|| format!("{command} {subcommand}"))
}

/// Uppercase bare words of a statement with strings, quoted identifiers and
/// comments removed, so keywords inside literals do not count.
fn keywords(statement: &str) -> Vec<String> {
    let stripped = strip_literals_and_comments(statement);
    stripped
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_uppercase())
        .collect()
}

/// Walk `sql` reporting each character together with whether it sits inside
/// a string literal, quoted identifier, comment or dollar quoted body. Quoted
/// regions are reported with `inside == true` (delimiters included); comments
/// are replaced by a single space.
fn scan(sql: &str, mut visit: impl FnMut(char, bool)) {
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' | '`' | '[' => {
                let closer = if c == '[' { ']' } else { c };
                visit(c, true);
                for inner in chars.by_ref() {
                    visit(inner, true);
                    if inner == closer {
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                // The line break stays, since a SQL Server `GO` on the next
                // line separates batches.
                let ended_line = chars.by_ref().any(|inner| inner == '\n');
                visit(if ended_line { '\n' } else { ' ' }, false);
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for inner in chars.by_ref() {
                    if previous == '*' && inner == '/' {
                        break;
                    }
                    previous = inner;
                }
                visit(' ', false);
            }
            '$' => {
                // Dollar quoting: `$tag$ ... $tag$`. Anything else starting
                // with `$` (a positional parameter) passes through.
                let mut tag = String::from("$");
                let mut is_dollar_quote = false;
                while let Some(&next) = chars.peek() {
                    if next == '$' {
                        chars.next();
                        tag.push('$');
                        is_dollar_quote = true;
                        break;
                    } else if next.is_ascii_alphanumeric() || next == '_' {
                        tag.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                for tag_char in tag.chars() {
                    visit(tag_char, is_dollar_quote);
                }
                if is_dollar_quote {
                    let mut body = String::new();
                    for inner in chars.by_ref() {
                        visit(inner, true);
                        body.push(inner);
                        if body.ends_with(&tag) {
                            break;
                        }
                    }
                }
            }
            _ => visit(c, false),
        }
    }
}

/// Split on `;` outside strings, quoted identifiers, comments and dollar
/// quoted bodies.
fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut line_start = 0;
    let mut line_is_code = true;
    scan(sql, |c, inside| {
        if c == ';' && !inside {
            statements.push(std::mem::take(&mut current));
            line_start = 0;
        } else if c == '\n' && !inside && line_is_code && is_go_line(&current[line_start..]) {
            // SQL Server's batch separator. Elsewhere it is a syntax error,
            // so splitting on it cannot hide a write.
            current.truncate(line_start);
            statements.push(std::mem::take(&mut current));
            line_start = 0;
        } else {
            current.push(c);
            if c == '\n' && !inside {
                line_start = current.len();
                line_is_code = true;
            }
        }
        if inside {
            line_is_code = false;
        }
    });
    if line_is_code && is_go_line(&current[line_start..]) {
        current.truncate(line_start);
    }
    statements.push(current);
    statements
        .into_iter()
        .filter(|statement| !statement.trim().is_empty())
        .collect()
}

/// A line holding only `GO` or `GO <count>`.
fn is_go_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("GO"))
        && words
            .next()
            .is_none_or(|count| count.parse::<u32>().is_ok())
        && words.next().is_none()
}

/// Remove string literals, quoted identifiers, comments and dollar quoted
/// bodies, leaving the structural keywords.
fn strip_literals_and_comments(statement: &str) -> String {
    let mut output = String::with_capacity(statement.len());
    scan(statement, |c, inside| {
        output.push(if inside { ' ' } else { c });
    });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_reads_are_reads() {
        for sql in [
            "SELECT * FROM users",
            "  select 1;",
            "EXPLAIN SELECT 1",
            "SHOW TABLES",
            "WITH recent AS (SELECT * FROM orders) SELECT * FROM recent",
            "PRAGMA table_info(users)",
            "BEGIN; SELECT 1; COMMIT;",
            "SELECT 'DROP TABLE users' AS note -- DELETE nothing",
            "SELECT /* UPDATE */ 1",
            "SET search_path TO app",
            "",
        ] {
            assert_eq!(classify_sql(sql), StatementAccess::Read, "{sql}");
        }
    }

    #[test]
    fn writes_are_writes() {
        for sql in [
            "INSERT INTO users VALUES (1)",
            "update users set name = 'x'",
            "SELECT 1; DELETE FROM users",
            "WITH gone AS (DELETE FROM users RETURNING *) SELECT * FROM gone",
            "CREATE TABLE t (id INT)",
            "DROP TABLE users",
            "TRUNCATE users",
            "CALL cleanup()",
            "PRAGMA journal_mode = WAL",
            "COPY users FROM '/tmp/u.csv'",
            "DO $$ BEGIN DELETE FROM users; END $$",
        ] {
            assert_eq!(classify_sql(sql), StatementAccess::Write, "{sql}");
        }
    }

    #[test]
    fn write_kinds_name_each_distinct_write() {
        assert_eq!(
            write_kinds(DatabaseType::PostgreSQL, "SELECT 1"),
            Vec::<String>::new()
        );
        assert_eq!(
            write_kinds(
                DatabaseType::PostgreSQL,
                "insert into a values (1); SELECT 1; INSERT INTO b VALUES (2); update a set x = 1"
            ),
            ["INSERT", "UPDATE"]
        );
        assert_eq!(
            write_kinds(
                DatabaseType::PostgreSQL,
                "WITH gone AS (DELETE FROM users RETURNING *) SELECT * FROM gone"
            ),
            ["DELETE"]
        );
        assert_eq!(
            write_kinds(DatabaseType::SQLite, "PRAGMA journal_mode = WAL"),
            ["PRAGMA"]
        );
        assert_eq!(
            write_kinds(
                DatabaseType::Redis,
                "GET a\nset a 1\nCONFIG SET maxmemory 1mb"
            ),
            ["SET", "CONFIG SET"]
        );
    }

    #[test]
    fn explain_is_a_write_only_when_it_runs_a_write() {
        for sql in [
            "EXPLAIN DELETE FROM users",
            "EXPLAIN (FORMAT JSON) UPDATE users SET name = 'x'",
            "EXPLAIN ANALYZE SELECT * FROM users",
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 1",
            "ANALYZE SELECT * FROM users",
            "ANALYZE users",
            "ANALYZE VERBOSE update_log",
        ] {
            assert_eq!(classify_sql(sql), StatementAccess::Read, "{sql}");
        }
        for sql in [
            "EXPLAIN ANALYZE DELETE FROM users",
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) UPDATE users SET name = 'x'",
            "EXPLAIN ANALYZE WITH gone AS (DELETE FROM users RETURNING *) SELECT * FROM gone",
            "ANALYZE INSERT INTO users VALUES (1)",
            "ANALYZE FORMAT=JSON DELETE FROM users",
            "SET STATISTICS PROFILE ON; DELETE FROM users; SET STATISTICS PROFILE OFF",
        ] {
            assert_eq!(classify_sql(sql), StatementAccess::Write, "{sql}");
        }
        assert_eq!(
            write_kinds(
                DatabaseType::PostgreSQL,
                "EXPLAIN ANALYZE DELETE FROM users"
            ),
            ["DELETE"]
        );
    }

    #[test]
    fn go_separates_sql_server_batches() {
        assert_eq!(
            classify_sql("SET SHOWPLAN_ALL ON\nGO\nDELETE FROM users\nGO\nSET SHOWPLAN_ALL OFF"),
            StatementAccess::Read,
            "statements under SHOWPLAN are only compiled"
        );
        for sql in [
            "SELECT 1\nGO\nDELETE FROM users",
            "SELECT 1 -- note\ngo 2\nDELETE FROM users",
            "SELECT 1\n  GO  \nINSERT INTO t VALUES (1)\nGO",
        ] {
            assert_eq!(classify_sql(sql), StatementAccess::Write, "{sql}");
        }
        assert_eq!(
            write_kinds(DatabaseType::MsSql, "SELECT 1\nGO\nDELETE FROM users"),
            ["DELETE"]
        );
        for sql in ["SELECT 'a\nGO\nDELETE'", "SELECT 1 GO"] {
            assert_eq!(classify_sql(sql), StatementAccess::Read, "{sql}");
        }
        assert_eq!(
            classify_sql(
                "SET SHOWPLAN_XML ON\nGO\nSELECT 1\nGO\nSET SHOWPLAN_XML OFF\nGO\nDELETE FROM users"
            ),
            StatementAccess::Write,
            "statements after SHOWPLAN is switched off run"
        );
    }

    #[test]
    fn pragma_assignment_is_a_write() {
        assert_eq!(classify_sql("PRAGMA foreign_keys"), StatementAccess::Read);
        assert_eq!(
            classify_sql("PRAGMA foreign_keys = ON"),
            StatementAccess::Write
        );
    }

    #[test]
    fn redis_commands() {
        assert_eq!(classify_redis("GET foo"), StatementAccess::Read);
        assert_eq!(classify_redis("hgetall user:1"), StatementAccess::Read);
        assert_eq!(
            classify_redis("CONFIG GET maxmemory"),
            StatementAccess::Read
        );
        assert_eq!(classify_redis("SET foo bar"), StatementAccess::Write);
        assert_eq!(
            classify_redis("CONFIG SET maxmemory 1"),
            StatementAccess::Write
        );
        assert_eq!(classify_redis("GET a\nDEL a"), StatementAccess::Write);
        assert_eq!(classify_redis("FLUSHALL"), StatementAccess::Write);
        assert_eq!(classify_redis("SOMETHINGNEW x"), StatementAccess::Write);
    }

    #[test]
    fn driver_dispatch() {
        assert_eq!(
            classify(DatabaseType::Redis, "SET a b"),
            StatementAccess::Write
        );
        assert_eq!(
            classify(DatabaseType::PostgreSQL, "SET search_path TO app"),
            StatementAccess::Read
        );
    }
}
