//! Split a SQL script into statements as it streams in, so a file far larger
//! than the editor could hold runs one statement at a time.
//!
//! The splitter knows enough of each backend's lexing to find statement ends:
//! quoted strings and identifiers, line and block comments, PostgreSQL dollar
//! quotes, MySQL `DELIMITER` lines and SQL Server `GO` batch separators.

use crate::{DatabaseType, Dialect};

/// Bytes kept unscanned at the end of the buffer until more input arrives, so
/// a token split across two chunks (`--`, `*/`, `$tag$`, a delimiter) is never
/// read half.
const LOOKAHEAD: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Lex {
    Normal,
    Quoted(char),
    LineComment,
    BlockComment,
    DollarQuoted(String),
}

pub struct StatementSplitter {
    database_type: DatabaseType,
    buffer: String,
    /// Where the statement being collected starts in `buffer`.
    start: usize,
    /// How far `buffer` has been scanned.
    position: usize,
    /// Where the current line starts in `buffer`.
    line_start: usize,
    lex: Lex,
    /// The statement so far has something besides whitespace and comments.
    has_code: bool,
    delimiter: String,
}

impl StatementSplitter {
    pub fn new(dialect: Dialect) -> Self {
        Self {
            database_type: dialect.database_type(),
            buffer: String::new(),
            start: 0,
            position: 0,
            line_start: 0,
            lex: Lex::Normal,
            has_code: false,
            delimiter: ";".to_string(),
        }
    }

    /// Feed the next chunk of the script and take the statements it
    /// completes, without their terminators.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        let mut statements = Vec::new();
        self.scan(false, &mut statements);
        self.compact();
        statements
    }

    /// The script ended: take the statements left, including an
    /// unterminated last one.
    pub fn finish(mut self) -> Vec<String> {
        let mut statements = Vec::new();
        self.scan(true, &mut statements);
        // A final `GO` line has no newline to end it.
        self.end_line(self.buffer.len(), &mut statements);
        self.emit(self.buffer.len(), self.buffer.len(), &mut statements);
        statements
    }

    fn scan(&mut self, last: bool, statements: &mut Vec<String>) {
        let limit = if last {
            self.buffer.len()
        } else {
            let mut limit = self.buffer.len().saturating_sub(LOOKAHEAD);
            while !self.buffer.is_char_boundary(limit) {
                limit -= 1;
            }
            limit
        };
        while self.position < limit {
            let rest = &self.buffer[self.position..];
            let c = rest.chars().next().unwrap_or_default();
            let width = c.len_utf8();
            match &self.lex {
                Lex::Normal => {
                    if self.database_type == DatabaseType::MySQL
                        && self.buffer[self.line_start..self.position]
                            .trim()
                            .is_empty()
                        && rest
                            .get(..10)
                            .is_some_and(|word| word.eq_ignore_ascii_case("delimiter "))
                    {
                        // `DELIMITER ;;` changes the terminator, so the line
                        // has to be read whole before its `;` is.
                        let line = match rest.find('\n') {
                            Some(end) => &rest[..end],
                            None if last => rest,
                            None => break,
                        };
                        let line_len = line.len();
                        if let Some(delimiter) = line.split_whitespace().nth(1) {
                            let delimiter = delimiter.to_string();
                            self.emit(self.position, self.position + line_len, statements);
                            self.delimiter = delimiter;
                        }
                        self.position += line_len;
                        continue;
                    }
                    if rest.starts_with(self.delimiter.as_str()) {
                        let end = self.position;
                        self.position += self.delimiter.len();
                        self.emit(end, self.position, statements);
                        continue;
                    }
                    if c == '\n' {
                        self.end_line(self.position, statements);
                        self.position += 1;
                        self.line_start = self.position;
                        continue;
                    }
                    if rest.starts_with("--")
                        || (c == '#' && self.database_type == DatabaseType::MySQL)
                    {
                        self.lex = Lex::LineComment;
                    } else if rest.starts_with("/*") {
                        // MySQL runs `/*! ... */` as code.
                        if rest.starts_with("/*!") {
                            self.has_code = true;
                        }
                        self.lex = Lex::BlockComment;
                        self.position += 2;
                        continue;
                    } else if let Some(tag) = self.dollar_tag(rest) {
                        self.has_code = true;
                        self.position += tag.len();
                        self.lex = Lex::DollarQuoted(tag);
                        continue;
                    } else if let Some(close) = self.quote_close(c) {
                        self.has_code = true;
                        self.lex = Lex::Quoted(close);
                    } else if !c.is_whitespace() {
                        self.has_code = true;
                    }
                    self.position += width;
                }
                Lex::Quoted(close) => {
                    let close = *close;
                    if c == '\\' && self.backslash_escapes() && close != ']' {
                        self.position += 1;
                        if let Some(next) = self.buffer[self.position..].chars().next() {
                            self.position += next.len_utf8();
                        }
                        continue;
                    }
                    if c == close {
                        self.lex = Lex::Normal;
                    }
                    if c == '\n' {
                        self.line_start = self.position + 1;
                    }
                    self.position += width;
                }
                Lex::LineComment => {
                    if c == '\n' {
                        self.lex = Lex::Normal;
                        continue;
                    }
                    self.position += width;
                }
                Lex::BlockComment => {
                    if rest.starts_with("*/") {
                        self.lex = Lex::Normal;
                        self.position += 2;
                        continue;
                    }
                    if c == '\n' {
                        self.line_start = self.position + 1;
                    }
                    self.position += width;
                }
                Lex::DollarQuoted(tag) => {
                    if rest.starts_with(tag.as_str()) {
                        self.position += tag.len();
                        self.lex = Lex::Normal;
                        continue;
                    }
                    if c == '\n' {
                        self.line_start = self.position + 1;
                    }
                    self.position += width;
                }
            }
        }
    }

    /// A line ended at `end` in normal text: a `GO` line ends a SQL Server
    /// batch.
    fn end_line(&mut self, end: usize, statements: &mut Vec<String>) {
        if self.lex != Lex::Normal
            || self.line_start > end
            || self.database_type != DatabaseType::MsSql
        {
            return;
        }
        if self.buffer[self.line_start..end]
            .trim()
            .eq_ignore_ascii_case("go")
        {
            self.emit(self.line_start, end, statements);
        }
    }

    /// Take `buffer[start..end]` as a statement when it holds code, and start
    /// the next statement at `next`.
    fn emit(&mut self, end: usize, next: usize, statements: &mut Vec<String>) {
        if self.has_code {
            let statement = self.buffer[self.start..end].trim();
            if !statement.is_empty() {
                statements.push(statement.to_string());
            }
        }
        self.start = next;
        self.line_start = self.line_start.max(next);
        self.has_code = false;
    }

    /// Drop what has been emitted so the buffer holds one statement at most.
    fn compact(&mut self) {
        if self.start == 0 {
            return;
        }
        self.buffer.drain(..self.start);
        self.position -= self.start;
        self.line_start = self.line_start.saturating_sub(self.start);
        self.start = 0;
    }

    fn quote_close(&self, c: char) -> Option<char> {
        quote_close(self.database_type, c)
    }

    fn backslash_escapes(&self) -> bool {
        backslash_escapes(self.database_type)
    }

    fn dollar_tag(&self, rest: &str) -> Option<String> {
        dollar_tag(self.database_type, rest)
    }
}

/// The character that closes a quote opened by `c`.
fn quote_close(database_type: DatabaseType, c: char) -> Option<char> {
    match (c, database_type) {
        ('\'', _) | ('"', _) => Some(c),
        ('`', DatabaseType::MySQL | DatabaseType::ClickHouse | DatabaseType::SQLite) => Some('`'),
        ('[', DatabaseType::MsSql | DatabaseType::SQLite) => Some(']'),
        _ => None,
    }
}

fn backslash_escapes(database_type: DatabaseType) -> bool {
    matches!(
        database_type,
        DatabaseType::MySQL | DatabaseType::ClickHouse
    )
}

/// A dollar-quote opener (`$$` or `$tag$`) at the start of `rest`. `$1` is a
/// parameter, not a quote.
fn dollar_tag(database_type: DatabaseType, rest: &str) -> Option<String> {
    if !database_type.dialect().supports_dollar_quotes() || !rest.starts_with('$') {
        return None;
    }
    let body = &rest[1..];
    let end = body.find('$')?;
    let tag = &body[..end];
    let valid = tag.chars().enumerate().all(|(ix, c)| {
        c == '_'
            || if ix == 0 {
                c.is_alphabetic()
            } else {
                c.is_alphanumeric()
            }
    });
    valid.then(|| format!("${tag}$"))
}

/// What a stretch of SQL text is to the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlRegionKind {
    Code,
    /// A string literal, its quotes included.
    String,
    /// A comment, a quoted identifier or a dollar-quoted body.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlRegion {
    pub range: std::ops::Range<usize>,
    pub kind: SqlRegionKind,
}

/// `sql` cut into consecutive regions of code, string literals and the rest,
/// lexed as the backend lexes them. An unterminated quote or comment runs to
/// the end.
pub fn sql_regions(dialect: Dialect, sql: &str) -> Vec<SqlRegion> {
    let database_type = dialect.database_type();
    let mut regions: Vec<SqlRegion> = Vec::new();
    let mut push = |range: std::ops::Range<usize>, kind: SqlRegionKind| {
        if range.is_empty() {
            return;
        }
        match regions.last_mut() {
            Some(last) if last.kind == kind && kind == SqlRegionKind::Code => {
                last.range.end = range.end;
            }
            _ => regions.push(SqlRegion { range, kind }),
        }
    };
    let mut position = 0;
    while let Some(c) = sql[position..].chars().next() {
        let start = position;
        let rest = &sql[position..];
        let (end, kind) =
            if rest.starts_with("--") || (c == '#' && database_type == DatabaseType::MySQL) {
                let end = rest.find('\n').map_or(sql.len(), |ix| start + ix);
                (end, SqlRegionKind::Other)
            } else if let Some(body) = rest.strip_prefix("/*") {
                let end = body.find("*/").map_or(sql.len(), |ix| start + 2 + ix + 2);
                (end, SqlRegionKind::Other)
            } else if let Some(tag) = dollar_tag(database_type, rest) {
                let body = &rest[tag.len()..];
                let end = body
                    .find(tag.as_str())
                    .map_or(sql.len(), |ix| start + tag.len() + ix + tag.len());
                (end, SqlRegionKind::Other)
            } else if let Some(close) = quote_close(database_type, c) {
                let mut end = sql.len();
                let mut chars = rest.char_indices().skip(1);
                while let Some((ix, next)) = chars.next() {
                    if next == '\\' && backslash_escapes(database_type) && close != ']' {
                        chars.next();
                    } else if next == close {
                        end = start + ix + next.len_utf8();
                        break;
                    }
                }
                // MySQL reads a double-quoted text as a string unless ANSI_QUOTES
                // is set.
                let string = c == '\'' || (c == '"' && database_type == DatabaseType::MySQL);
                let kind = if string {
                    SqlRegionKind::String
                } else {
                    SqlRegionKind::Other
                };
                (end, kind)
            } else {
                (start + c.len_utf8(), SqlRegionKind::Code)
            };
        push(start..end, kind);
        position = end;
    }
    regions
}

/// Split a whole script at once.
pub fn split_script(dialect: Dialect, script: &str) -> Vec<String> {
    let mut splitter = StatementSplitter::new(dialect);
    let mut statements = splitter.push(script);
    statements.extend(splitter.finish());
    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions(db_type: DatabaseType, sql: &str) -> Vec<(&str, SqlRegionKind)> {
        sql_regions(db_type.dialect(), sql)
            .into_iter()
            .map(|region| (&sql[region.range], region.kind))
            .collect()
    }

    #[test]
    fn regions_tell_code_from_strings_and_the_rest() {
        use SqlRegionKind::{Code, Other, String};
        assert_eq!(
            regions(
                DatabaseType::PostgreSQL,
                "SELECT 'a''b', \"id\" -- c\nFROM $x$ body $x$ /* d */ WHERE $1"
            ),
            vec![
                ("SELECT ", Code),
                ("'a'", String),
                ("'b'", String),
                (", ", Code),
                ("\"id\"", Other),
                (" ", Code),
                ("-- c", Other),
                ("\nFROM ", Code),
                ("$x$ body $x$", Other),
                (" ", Code),
                ("/* d */", Other),
                (" WHERE $1", Code),
            ]
        );
        assert_eq!(
            regions(DatabaseType::MySQL, r#"SELECT "a\"b", 'é # x"#),
            vec![
                ("SELECT ", Code),
                (r#""a\"b""#, String),
                (", ", Code),
                ("'é # x", String),
            ],
            "backslash escapes, and an unterminated string runs to the end"
        );
    }

    fn split(db_type: DatabaseType, script: &str) -> Vec<String> {
        split_script(db_type.dialect(), script)
    }

    /// Feeding the script one byte at a time gives the same statements as
    /// feeding it whole.
    fn split_in_pieces(db_type: DatabaseType, script: &str, piece: usize) -> Vec<String> {
        let mut splitter = StatementSplitter::new(db_type.dialect());
        let mut statements = Vec::new();
        let mut rest = script;
        while !rest.is_empty() {
            let mut cut = piece.min(rest.len());
            while !rest.is_char_boundary(cut) {
                cut += 1;
            }
            statements.extend(splitter.push(&rest[..cut]));
            rest = &rest[cut..];
        }
        statements.extend(splitter.finish());
        statements
    }

    #[test]
    fn splits_on_semicolons_outside_strings_and_comments() {
        let script = "CREATE TABLE t (a text);\n\
            INSERT INTO t VALUES ('a;b'), ('it''s; fine');\n\
            -- a comment; with a semicolon\n\
            /* block; comment */ SELECT \"x;y\" FROM t;\n\
            SELECT 1";
        let expected = vec![
            "CREATE TABLE t (a text)".to_string(),
            "INSERT INTO t VALUES ('a;b'), ('it''s; fine')".to_string(),
            "-- a comment; with a semicolon\n/* block; comment */ SELECT \"x;y\" FROM t"
                .to_string(),
            "SELECT 1".to_string(),
        ];
        assert_eq!(split(DatabaseType::PostgreSQL, script), expected);
        for piece in [1, 2, 3, 7, 64, 100] {
            assert_eq!(
                split_in_pieces(DatabaseType::PostgreSQL, script, piece),
                expected,
                "piece {piece}"
            );
        }
    }

    #[test]
    fn skips_empty_and_comment_only_statements() {
        assert_eq!(
            split(
                DatabaseType::SQLite,
                ";; -- nothing\n; /* only */ ;\nSELECT 1;\n-- trailing"
            ),
            vec!["SELECT 1"]
        );
    }

    #[test]
    fn postgres_dollar_quotes() {
        let script = "CREATE FUNCTION f() RETURNS int AS $body$ BEGIN; RETURN 1; END $body$ LANGUAGE plpgsql;\nSELECT $$a;b$$, $1;";
        let expected = vec![
            "CREATE FUNCTION f() RETURNS int AS $body$ BEGIN; RETURN 1; END $body$ LANGUAGE plpgsql",
            "SELECT $$a;b$$, $1",
        ];
        assert_eq!(split(DatabaseType::PostgreSQL, script), expected);
        assert_eq!(
            split_in_pieces(DatabaseType::PostgreSQL, script, 1),
            expected
        );
        // Elsewhere `$` is not a quote.
        assert_eq!(split(DatabaseType::MySQL, "SELECT '$$'; SELECT 2").len(), 2);
    }

    #[test]
    fn mysql_backslashes_hash_comments_and_delimiter() {
        let script = "INSERT INTO t VALUES ('a\\';b');\n\
            # hash comment;\n\
            DELIMITER ;;\n\
            CREATE TRIGGER tr BEFORE INSERT ON t FOR EACH ROW BEGIN SET NEW.a = 'x'; END;;\n\
            DELIMITER ;\n\
            SELECT `a;b` FROM t;";
        let expected = vec![
            "INSERT INTO t VALUES ('a\\';b')",
            "CREATE TRIGGER tr BEFORE INSERT ON t FOR EACH ROW BEGIN SET NEW.a = 'x'; END",
            "SELECT `a;b` FROM t",
        ];
        assert_eq!(split(DatabaseType::MySQL, script), expected);
        assert_eq!(split_in_pieces(DatabaseType::MySQL, script, 1), expected);
        // Without backslash escapes the quote closes at the backslash.
        assert_eq!(
            split(DatabaseType::PostgreSQL, "SELECT 'a\\'; SELECT 2"),
            vec!["SELECT 'a\\'", "SELECT 2"]
        );
    }

    #[test]
    fn sql_server_go_batches_and_brackets() {
        let script =
            "CREATE TABLE [a;b] (x int)\nGO\nINSERT INTO [a;b] VALUES (1)\n  go  \nSELECT 1\nGO";
        let expected = vec![
            "CREATE TABLE [a;b] (x int)",
            "INSERT INTO [a;b] VALUES (1)",
            "SELECT 1",
        ];
        assert_eq!(split(DatabaseType::MsSql, script), expected);
        assert_eq!(split_in_pieces(DatabaseType::MsSql, script, 1), expected);
        // GO inside a statement or elsewhere is not a separator.
        assert_eq!(split(DatabaseType::PostgreSQL, "SELECT 1\nGO\n").len(), 1);
    }

    #[test]
    fn multibyte_text_across_chunks() {
        let script = "INSERT INTO t VALUES ('åäö; 日本');SELECT 'ö'";
        let expected = vec!["INSERT INTO t VALUES ('åäö; 日本')", "SELECT 'ö'"];
        assert_eq!(split_in_pieces(DatabaseType::SQLite, script, 1), expected);
    }

    #[test]
    fn memory_stays_bounded_while_streaming() {
        let mut splitter = StatementSplitter::new(DatabaseType::SQLite.dialect());
        let mut count = 0;
        for _ in 0..10_000 {
            count += splitter.push("INSERT INTO t VALUES (1);\n").len();
            assert!(splitter.buffer.len() < 200, "{}", splitter.buffer.len());
        }
        count += splitter.finish().len();
        assert_eq!(count, 10_000);
    }
}
