//! Which keywords fit where the cursor is in a statement.
//!
//! The tree-sitter tree is mostly ERROR nodes while a statement is being
//! typed, so the position is read from the tokens before the cursor instead:
//! the innermost parenthesized group the cursor is in, the last clause keyword
//! in it, and whether what follows that keyword is a finished item.

use blanco_core::Dialect;

/// Every keyword completion offers, in the order they are listed when the
/// position expects none of them in particular.
const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "UPSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "WITH",
    "CREATE",
    "ALTER",
    "DROP",
    "TABLE",
    "INDEX",
    "VIEW",
    "JOIN",
    "INNER JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "CROSS JOIN",
    "OUTER JOIN",
    "ON",
    "USING",
    "GROUP BY",
    "ORDER BY",
    "BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "TOP",
    "FINAL",
    "SETTINGS",
    "RETURNING",
    "OUTPUT",
    "AND",
    "OR",
    "NOT",
    "IN",
    "EXISTS",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "NULL",
    "NOT NULL",
    "TRUE",
    "FALSE",
    "ASC",
    "DESC",
    "DISTINCT",
    "UNION",
    "UNION ALL",
    "ALL",
    "AS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
];

/// Keywords a statement may hold any number of times, so having used one
/// does not rank it lower.
const REPEATABLE: &[&str] = &[
    "SELECT",
    "JOIN",
    "INNER JOIN",
    "LEFT JOIN",
    "RIGHT JOIN",
    "CROSS JOIN",
    "OUTER JOIN",
    "ON",
    "USING",
    "BY",
    "AND",
    "OR",
    "NOT",
    "IN",
    "EXISTS",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "NULL",
    "NOT NULL",
    "TRUE",
    "FALSE",
    "ASC",
    "DESC",
    "DISTINCT",
    "UNION",
    "UNION ALL",
    "ALL",
    "AS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
];

/// Keywords that end an expression, the way a name or a literal does.
const VALUE_WORDS: &[&str] = &["NULL", "TRUE", "FALSE", "END", "ASC", "DESC"];

/// The clauses of a `SELECT` that may follow the one being written, in the
/// order they are written.
const AFTER_FROM: &[&str] = &[
    "WHERE",
    "JOIN",
    "INNER JOIN",
    "LEFT JOIN",
    "GROUP BY",
    "ORDER BY",
    "LIMIT",
    "FINAL",
    "SETTINGS",
    "UNION",
    "AS",
];
const AFTER_WHERE: &[&str] = &["GROUP BY", "ORDER BY", "LIMIT", "SETTINGS", "UNION"];
const AFTER_GROUP_BY: &[&str] = &["HAVING", "ORDER BY", "LIMIT", "SETTINGS", "UNION"];
const AFTER_HAVING: &[&str] = &["ORDER BY", "LIMIT", "SETTINGS", "UNION"];
const AFTER_ON: &[&str] = &[
    "JOIN",
    "INNER JOIN",
    "LEFT JOIN",
    "WHERE",
    "GROUP BY",
    "ORDER BY",
    "LIMIT",
];
const COMPARISONS: &[&str] = &["IS", "IN", "LIKE", "ILIKE", "BETWEEN", "NOT"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A bare word, upper-cased. It may be a keyword or a name.
    Word(String),
    /// A quoted or qualified name.
    Name,
    /// A string, a number or a parameter.
    Literal,
    /// A closed parenthesized group.
    Group,
    Comma,
    Star,
    Operator,
}

impl Token {
    fn word(&self) -> Option<&str> {
        match self {
            Token::Word(word) => Some(word),
            _ => None,
        }
    }

    fn is(&self, keyword: &str) -> bool {
        self.word() == Some(keyword)
    }

    /// Whether the token ends an item, such as a column, a table or an
    /// expression, after which the next clause may start.
    fn ends_item(&self) -> bool {
        match self {
            Token::Word(word) => VALUE_WORDS.contains(&word.as_str()) || !is_keyword_word(word),
            Token::Name | Token::Literal | Token::Group | Token::Star => true,
            Token::Comma | Token::Operator => false,
        }
    }
}

/// Whether `word` is part of a keyword completion knows, so a bare word that
/// is not one reads as a name.
fn is_keyword_word(word: &str) -> bool {
    KEYWORDS
        .iter()
        .any(|keyword| keyword.split(' ').any(|part| part == word))
        || super::is_sql_keyword(word)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lexeme {
    Token(Token),
    Open,
    Close,
    Semicolon,
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

/// Index just past the quoted text starting at `start`, where a doubled quote
/// stands for the quote itself.
fn quoted_end(bytes: &[u8], start: usize, close: u8) -> usize {
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == close {
            if close != b']' && bytes.get(index + 1) == Some(&close) {
                index += 2;
                continue;
            }
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn lex(text: &str) -> Vec<Lexeme> {
    let bytes = text.as_bytes();
    let mut lexemes = Vec::new();
    let mut index = 0;
    // A `.` joins the name before it and the word after it into one name.
    let mut after_dot = false;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        let start = index;
        let token = match byte {
            _ if byte.is_ascii_whitespace() => {
                index += 1;
                continue;
            }
            b'-' if next == Some(b'-') => {
                index = bytes[index..]
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .map_or(bytes.len(), |offset| index + offset);
                continue;
            }
            b'/' if next == Some(b'*') => {
                index = text[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |offset| index + 2 + offset + 2);
                continue;
            }
            b'(' => {
                index += 1;
                after_dot = false;
                lexemes.push(Lexeme::Open);
                continue;
            }
            b')' => {
                index += 1;
                after_dot = false;
                lexemes.push(Lexeme::Close);
                continue;
            }
            b';' => {
                index += 1;
                after_dot = false;
                lexemes.push(Lexeme::Semicolon);
                continue;
            }
            b'.' if next.is_none_or(|next| !next.is_ascii_digit()) => {
                index += 1;
                after_dot = true;
                continue;
            }
            b'\'' => {
                index = quoted_end(bytes, index, b'\'');
                Token::Literal
            }
            b'"' | b'`' => {
                index = quoted_end(bytes, index, byte);
                Token::Name
            }
            b'[' => {
                index = quoted_end(bytes, index, b']');
                Token::Name
            }
            b',' => {
                index += 1;
                Token::Comma
            }
            b'*' => {
                index += 1;
                Token::Star
            }
            b'0'..=b'9' | b'.' => {
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric()
                        || bytes[index] == b'.'
                        || bytes[index] == b'_')
                {
                    index += 1;
                }
                Token::Literal
            }
            b'$' | b'?' | b'@' | b':' if next.is_some_and(is_word_byte) || byte == b'?' => {
                index += 1;
                while index < bytes.len() && is_word_byte(bytes[index]) {
                    index += 1;
                }
                Token::Literal
            }
            _ if is_word_byte(byte) => {
                while index < bytes.len() && is_word_byte(bytes[index]) {
                    index += 1;
                }
                Token::Word(text[start..index].to_uppercase())
            }
            _ => {
                index += 1;
                Token::Operator
            }
        };
        // `schema.table` or `alias.column` is one name, and `t.*` one star.
        let joins_previous = after_dot
            && matches!(token, Token::Word(_) | Token::Name | Token::Star)
            && matches!(
                lexemes.last(),
                Some(Lexeme::Token(Token::Word(_) | Token::Name))
            );
        if joins_previous {
            lexemes.pop();
        }
        let token = match token {
            Token::Word(_) if joins_previous => Token::Name,
            token => token,
        };
        after_dot = false;
        lexemes.push(Lexeme::Token(token));
    }
    lexemes
}

/// The tokens of one parenthesized group, or of the statement outside any.
#[derive(Debug, Default)]
struct Frame {
    tokens: Vec<Token>,
    /// The token right before the group's opening parenthesis.
    opener: Option<Token>,
}

/// Where the cursor is, as far as keywords are concerned.
#[derive(Debug, Default)]
pub(super) struct KeywordPosition {
    /// The tokens before the cursor in the group it is in.
    frame: Frame,
    /// Whether that group is the statement itself rather than a parenthesized
    /// group in it.
    top_level: bool,
    /// Bare words of the same group after the cursor.
    words_after: Vec<String>,
}

impl KeywordPosition {
    /// The position of a cursor at byte `cursor` of `text`, where
    /// `current_word` is the word being typed that ends at the cursor.
    pub(super) fn at(text: &str, cursor: usize, current_word: &str) -> Self {
        let cursor = cursor.min(text.len());
        let word_start = cursor.saturating_sub(current_word.len());
        let (Some(before), Some(after)) = (text.get(..word_start), text.get(cursor..)) else {
            return Self::default();
        };

        let mut frames = vec![Frame::default()];
        for lexeme in lex(before) {
            match lexeme {
                Lexeme::Token(token) => {
                    if let Some(frame) = frames.last_mut() {
                        frame.tokens.push(token);
                    }
                }
                Lexeme::Open => {
                    let opener = frames.last().and_then(|frame| frame.tokens.last().cloned());
                    frames.push(Frame {
                        tokens: Vec::new(),
                        opener,
                    });
                }
                Lexeme::Close => {
                    if frames.len() > 1 {
                        frames.pop();
                        if let Some(frame) = frames.last_mut() {
                            frame.tokens.push(Token::Group);
                        }
                    }
                }
                Lexeme::Semicolon => frames = vec![Frame::default()],
            }
        }
        let top_level = frames.len() == 1;
        let frame = frames.pop().unwrap_or_default();

        // The rest of a word the cursor is inside is not a word after it.
        let after = after.trim_start_matches(|character: char| {
            character.is_alphanumeric() || character == '_' || character == '$'
        });
        let mut words_after = Vec::new();
        let mut depth = 0usize;
        for lexeme in lex(after) {
            match lexeme {
                Lexeme::Token(Token::Word(word)) if depth == 0 => words_after.push(word),
                Lexeme::Token(_) => {}
                Lexeme::Open => depth += 1,
                Lexeme::Close if depth == 0 => break,
                Lexeme::Close => depth -= 1,
                Lexeme::Semicolon => break,
            }
        }

        Self {
            frame,
            top_level,
            words_after,
        }
    }

    /// The last clause keyword of the group with its index, folding `GROUP BY`
    /// and `ORDER BY` into one and reading `INTO` only as part of an insert.
    fn clause(&self) -> Option<(usize, &'static str)> {
        let tokens = &self.frame.tokens;
        tokens.iter().enumerate().rev().find_map(|(index, token)| {
            let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
            let clause = match token.word()? {
                "SELECT" => "SELECT",
                "FROM" => "FROM",
                "JOIN" => "JOIN",
                "ON" => "ON",
                "WHERE" => "WHERE",
                "HAVING" => "HAVING",
                "BY" if previous.is_some_and(|token| token.is("GROUP")) => "GROUP BY",
                "BY" if previous.is_some_and(|token| token.is("ORDER")) => "ORDER BY",
                "LIMIT" => "LIMIT",
                "OFFSET" => "OFFSET",
                "INSERT" | "UPSERT" => "INSERT",
                "INTO"
                    if previous.is_some_and(|token| token.is("INSERT") || token.is("UPSERT")) =>
                {
                    "INTO"
                }
                "VALUES" => "VALUES",
                "UPDATE" => "UPDATE",
                "SET" => "SET",
                "DELETE" => "DELETE",
                "USING" => "USING",
                "RETURNING" => "RETURNING",
                "OUTPUT" => "OUTPUT",
                "SETTINGS" => "SETTINGS",
                _ => return None,
            };
            Some((index, clause))
        })
    }

    /// The statement verb the clause belongs to: the last `SELECT`, `INSERT`,
    /// `UPDATE` or `DELETE` before it.
    fn verb(&self, before: usize) -> Option<&str> {
        self.frame.tokens[..before.min(self.frame.tokens.len())]
            .iter()
            .rev()
            .filter_map(Token::word)
            .find(|word| matches!(*word, "SELECT" | "INSERT" | "UPDATE" | "DELETE"))
    }

    /// The keywords this position expects, most likely first, before any
    /// backend filtering.
    fn expected_keywords(&self) -> Vec<&'static str> {
        let tokens = &self.frame.tokens;
        let Some(last) = tokens.last() else {
            return if self.top_level {
                vec!["SELECT", "INSERT", "UPSERT", "UPDATE", "DELETE", "WITH"]
            } else if self
                .frame
                .opener
                .as_ref()
                .is_none_or(|opener| matches!(opener, Token::Word(word) if is_keyword_word(word)))
            {
                vec!["SELECT"]
            } else {
                Vec::new()
            };
        };

        if let Some(case_keywords) = self.open_case_keywords() {
            return case_keywords;
        }

        if !last.ends_item() {
            return match last.word() {
                Some("INSERT" | "UPSERT") => vec!["INTO"],
                Some("DELETE") => vec!["FROM"],
                Some("GROUP" | "ORDER") => vec!["BY"],
                Some("INNER" | "CROSS") => vec!["JOIN"],
                Some("LEFT" | "RIGHT") => vec!["JOIN", "OUTER JOIN"],
                Some("IS") => vec!["NULL", "NOT NULL"],
                Some("NOT") => vec!["IN", "LIKE", "ILIKE", "BETWEEN", "EXISTS"],
                Some("UNION") => vec!["ALL", "SELECT"],
                _ => Vec::new(),
            };
        }

        let Some((clause_index, clause)) = self.clause() else {
            return Vec::new();
        };
        let verb = self.verb(clause_index);
        let since_clause = &tokens[clause_index + 1..];
        match clause {
            "SELECT" => vec!["FROM", "AS"],
            "FROM" if verb == Some("DELETE") => vec!["WHERE", "USING", "RETURNING", "OUTPUT"],
            "FROM" => AFTER_FROM.to_vec(),
            "JOIN" => vec!["ON", "USING", "AS"],
            "ON" | "WHERE" | "HAVING" => {
                if !has_predicate(since_clause) {
                    return COMPARISONS.to_vec();
                }
                let rest = match clause {
                    "ON" => AFTER_ON,
                    "HAVING" => AFTER_HAVING,
                    _ if matches!(verb, Some("UPDATE" | "DELETE")) => &["RETURNING"],
                    _ => AFTER_WHERE,
                };
                ["AND", "OR"]
                    .into_iter()
                    .chain(rest.iter().copied())
                    .collect()
            }
            "GROUP BY" => AFTER_GROUP_BY.to_vec(),
            "ORDER BY" if last.is("ASC") || last.is("DESC") => {
                vec!["LIMIT", "OFFSET", "SETTINGS"]
            }
            "ORDER BY" => vec!["ASC", "DESC", "LIMIT", "OFFSET", "SETTINGS"],
            "LIMIT" => vec!["OFFSET", "SETTINGS"],
            "INTO" => vec!["VALUES", "SELECT", "OUTPUT"],
            "VALUES" => vec!["RETURNING"],
            "UPDATE" => vec!["SET"],
            "SET" => vec!["WHERE", "FROM", "RETURNING", "OUTPUT"],
            "USING" => vec!["WHERE", "RETURNING"],
            _ => Vec::new(),
        }
    }

    /// Inside an unfinished `CASE`, the keywords that continue it.
    fn open_case_keywords(&self) -> Option<Vec<&'static str>> {
        let mut depth = 0usize;
        let mut last_branch = None;
        for word in self.frame.tokens.iter().filter_map(Token::word) {
            match word {
                "CASE" => depth += 1,
                "END" => depth = depth.saturating_sub(1),
                "WHEN" | "THEN" | "ELSE" if depth > 0 => last_branch = Some(word),
                _ => {}
            }
        }
        if depth == 0 || !self.frame.tokens.last().is_some_and(Token::ends_item) {
            return None;
        }
        Some(match last_branch {
            Some("WHEN") => vec!["THEN"],
            Some("ELSE") => vec!["END"],
            _ => vec!["WHEN", "ELSE", "END"],
        })
    }

    /// Whether the statement already holds `keyword` in the cursor's group.
    fn uses(&self, keyword: &str) -> bool {
        let parts: Vec<&str> = keyword.split(' ').collect();
        let before: Vec<&str> = self.frame.tokens.iter().filter_map(Token::word).collect();
        let after: Vec<&str> = self.words_after.iter().map(String::as_str).collect();
        [before, after].iter().any(|words| {
            words
                .windows(parts.len())
                .any(|window| window == parts.as_slice())
        })
    }

    /// The keywords starting with `current_word` the backend understands: the
    /// ones this position expects, most likely first, and then the rest of
    /// the list, with keywords the statement already holds at its end.
    pub(super) fn ranked(
        &self,
        dialect: Dialect,
        current_word: &str,
    ) -> (Vec<&'static str>, Vec<&'static str>) {
        let needle = current_word.to_uppercase();
        let offered =
            |keyword: &&str| dialect.offers_keyword(keyword) && keyword.starts_with(&needle);
        let used = |keyword: &&str| !REPEATABLE.contains(keyword) && self.uses(keyword);

        let expected_keywords = self.expected_keywords();
        let expected: Vec<&'static str> = expected_keywords
            .iter()
            .copied()
            .filter(offered)
            .filter(|keyword| !used(keyword))
            .collect();
        let (used_keywords, unused_keywords): (Vec<&'static str>, Vec<&'static str>) = KEYWORDS
            .iter()
            .copied()
            .filter(offered)
            .filter(|keyword| !expected.contains(keyword))
            .partition(used);
        let rest = unused_keywords.into_iter().chain(used_keywords).collect();
        (expected, rest)
    }
}

/// Whether a condition since its clause keyword, or since its last `AND` or
/// `OR`, already compares something.
fn has_predicate(tokens: &[Token]) -> bool {
    tokens
        .iter()
        .rev()
        .take_while(|token| !token.is("AND") && !token.is("OR"))
        .any(|token| {
            matches!(token, Token::Operator) || COMPARISONS.iter().any(|keyword| token.is(keyword))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::DatabaseType;

    /// The keywords expected at the `|` in `sql` on `db_type`.
    fn expected_on(db_type: DatabaseType, sql: &str) -> Vec<&'static str> {
        let cursor = sql.find('|').expect("cursor marker");
        let text = sql.replace('|', "");
        let current_word: String = text[..cursor]
            .chars()
            .rev()
            .take_while(|character| character.is_alphanumeric() || *character == '_')
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        KeywordPosition::at(&text, cursor, &current_word)
            .ranked(db_type.dialect(), &current_word)
            .0
    }

    fn expected(sql: &str) -> Vec<&'static str> {
        expected_on(DatabaseType::PostgreSQL, sql)
    }

    fn first(sql: &str) -> Option<&'static str> {
        expected(sql).first().copied()
    }

    #[test]
    fn statement_keywords_follow_their_verb() {
        assert_eq!(first("INSERT |"), Some("INTO"));
        assert_eq!(first("INSERT IN|"), Some("INTO"));
        assert_eq!(first("UPDATE users |"), Some("SET"));
        assert_eq!(first("UPDATE public.users |"), Some("SET"));
        assert_eq!(first("DELETE |"), Some("FROM"));
        assert_eq!(
            expected("INSERT INTO users (id, name) |"),
            vec!["VALUES", "SELECT"]
        );
        assert_eq!(expected("INSERT INTO users |"), vec!["VALUES", "SELECT"]);
        assert_eq!(
            expected("INSERT INTO users (id) VALUES (1) |"),
            vec!["RETURNING"]
        );
    }

    #[test]
    fn clause_keywords_follow_the_clause_they_can_come_after() {
        assert_eq!(
            expected("SELECT * FROM users |"),
            vec![
                "WHERE",
                "JOIN",
                "INNER JOIN",
                "LEFT JOIN",
                "GROUP BY",
                "ORDER BY",
                "LIMIT",
                "UNION",
                "AS"
            ]
        );
        assert_eq!(expected("SELECT id, name |"), vec!["FROM", "AS"]);
        assert_eq!(
            expected("SELECT * FROM users WHERE id = 1 |"),
            vec!["AND", "OR", "GROUP BY", "ORDER BY", "LIMIT", "UNION"]
        );
        assert_eq!(
            expected("SELECT * FROM users WHERE name |"),
            vec!["IS", "IN", "LIKE", "ILIKE", "BETWEEN", "NOT"]
        );
        assert_eq!(
            expected("SELECT count(*) FROM users GROUP BY name |"),
            vec!["HAVING", "ORDER BY", "LIMIT", "UNION"]
        );
        assert_eq!(
            expected("SELECT * FROM users ORDER BY name |"),
            vec!["ASC", "DESC", "LIMIT", "OFFSET"]
        );
        assert_eq!(
            expected("SELECT * FROM a JOIN b |"),
            vec!["ON", "USING", "AS"]
        );
        assert_eq!(first("SELECT * FROM users GROUP |"), Some("BY"));
        assert_eq!(first("SELECT * FROM a LEFT |"), Some("JOIN"));
        assert_eq!(
            expected("UPDATE users SET name = 'x' WHERE id = 1 |"),
            vec!["AND", "OR", "RETURNING"]
        );
    }

    #[test]
    fn a_position_that_wants_a_name_expects_no_keyword() {
        assert!(expected("SELECT |").is_empty());
        assert!(expected("SELECT * FROM |").is_empty());
        assert!(expected("SELECT * FROM users WHERE id = |").is_empty());
        assert!(expected("SELECT id, |").is_empty());
        assert!(expected("INSERT INTO users (|").is_empty());
        assert!(expected("SELECT lower(|").is_empty());
    }

    #[test]
    fn the_group_around_the_cursor_decides() {
        assert_eq!(
            first("SELECT * FROM (SELECT id FROM users WHERE id = 1 |"),
            Some("AND")
        );
        assert_eq!(
            first("SELECT * FROM (SELECT id FROM users) AS u |"),
            Some("WHERE")
        );
        assert_eq!(
            expected("SELECT * FROM users WHERE id IN (|"),
            vec!["SELECT"]
        );
        assert_eq!(first("SELECT 1; INSERT |"), Some("INTO"));
        assert_eq!(first("SELECT ';' FROM t /* ( */ |"), Some("WHERE"));
    }

    #[test]
    fn case_expressions_continue_with_their_branches() {
        assert_eq!(expected("SELECT CASE WHEN a = 1 |"), vec!["THEN"]);
        assert_eq!(
            expected("SELECT CASE WHEN a = 1 THEN 'x' |"),
            vec!["WHEN", "ELSE", "END"]
        );
        assert_eq!(first("SELECT CASE WHEN a THEN 1 END |"), Some("FROM"));
    }

    #[test]
    fn backend_keywords_are_offered_only_on_their_backend() {
        let sql = "SELECT * FROM users |";
        assert!(expected_on(DatabaseType::PostgreSQL, sql).contains(&"LIMIT"));
        assert!(!expected_on(DatabaseType::MsSql, sql).contains(&"LIMIT"));
        assert!(expected_on(DatabaseType::ClickHouse, sql).contains(&"FINAL"));
        assert!(expected_on(DatabaseType::ClickHouse, sql).contains(&"SETTINGS"));
        assert!(!expected_on(DatabaseType::MySQL, sql).contains(&"FINAL"));

        // CockroachDB's UPSERT starts a statement and takes INTO, as INSERT does.
        assert!(expected_on(DatabaseType::CockroachDb, "|").contains(&"UPSERT"));
        assert!(!expected_on(DatabaseType::PostgreSQL, "|").contains(&"UPSERT"));
        assert_eq!(
            expected_on(DatabaseType::CockroachDb, "UPSERT |").first(),
            Some(&"INTO")
        );
        assert_eq!(
            expected_on(DatabaseType::CockroachDb, "UPSERT INTO users (id) |").first(),
            Some(&"VALUES")
        );

        let insert = "INSERT INTO users (id) |";
        assert_eq!(
            expected_on(DatabaseType::MsSql, insert),
            vec!["VALUES", "SELECT", "OUTPUT"]
        );
        let values = "INSERT INTO users (id) VALUES (1) |";
        assert_eq!(expected_on(DatabaseType::SQLite, values), vec!["RETURNING"]);
        assert!(expected_on(DatabaseType::MySQL, values).is_empty());

        let (_, rest) = KeywordPosition::at("", 0, "").ranked(DatabaseType::MsSql.dialect(), "");
        assert!(rest.contains(&"TOP"));
        let (_, rest) =
            KeywordPosition::at("", 0, "").ranked(DatabaseType::PostgreSQL.dialect(), "");
        assert!(!rest.contains(&"TOP"));
    }

    #[test]
    fn used_keywords_rank_lower_and_stay_available() {
        let sql = "SELECT * FROM users  WHERE id = 1 ORDER BY id";
        let position = KeywordPosition::at(sql, "SELECT * FROM users ".len(), "");
        let (expected, rest) = position.ranked(DatabaseType::PostgreSQL.dialect(), "");
        assert_eq!(expected.first(), Some(&"JOIN"), "{expected:?}");
        assert!(!expected.contains(&"WHERE"), "{expected:?}");
        assert!(!expected.contains(&"ORDER BY"), "{expected:?}");
        assert_eq!(rest[rest.len() - 3..], ["FROM", "WHERE", "ORDER BY"]);

        let sql = "SELECT * FROM a WHERE x = 1 AND y = 2 ";
        let position = KeywordPosition::at(sql, sql.len(), "");
        let (expected, _) = position.ranked(DatabaseType::PostgreSQL.dialect(), "");
        assert_eq!(expected[..2], ["AND", "OR"]);
    }
}
