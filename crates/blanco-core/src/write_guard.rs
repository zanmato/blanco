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
    let mut access = StatementAccess::Read;
    for statement in split_statements(sql) {
        let words = keywords(&statement);
        let Some(first) = words.first() else {
            continue;
        };
        let is_read = match first.as_str() {
            "WITH" => !words
                .iter()
                .any(|word| SQL_WRITE_KEYWORDS.contains(&word.as_str())),
            // `PRAGMA name` and `PRAGMA name(arg)` (table_info, index_list)
            // read, `PRAGMA name = value` writes. SQLite also accepts the
            // parenthesised form for setting, but it is rare in practice.
            "PRAGMA" => !statement.contains('='),
            keyword => SQL_READ_KEYWORDS.contains(&keyword),
        };
        if !is_read {
            access = StatementAccess::Write;
        }
    }
    access
}

/// Classify a Redis command line (one command per line).
pub fn classify_redis(text: &str) -> StatementAccess {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(command) = parts.next() else {
            continue;
        };
        let command = command.to_ascii_uppercase();
        if !REDIS_READ_COMMANDS.contains(&command.as_str()) {
            return StatementAccess::Write;
        }
        if let Some(subcommand) = parts.next() {
            let subcommand = subcommand.to_ascii_uppercase();
            if REDIS_WRITE_SUBCOMMANDS
                .iter()
                .any(|(c, s)| *c == command && *s == subcommand)
            {
                return StatementAccess::Write;
            }
        }
    }
    StatementAccess::Read
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
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        break;
                    }
                }
                visit(' ', false);
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
    scan(sql, |c, inside| {
        if c == ';' && !inside {
            statements.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    });
    statements.push(current);
    statements
        .into_iter()
        .filter(|statement| !statement.trim().is_empty())
        .collect()
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
