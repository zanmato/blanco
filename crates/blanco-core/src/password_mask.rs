//! Passwords in statements that create or change an account, masked for the
//! query history and the SQL log. What runs on the server is never masked:
//! only the text kept afterwards.

use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

use crate::{Dialect, statement_stream};

/// What a masked password reads as.
pub const MASK: &str = "'********'";

/// `sql` with the password literals of its account statements replaced by
/// [`MASK`]: PostgreSQL's and CockroachDB's `CREATE`/`ALTER` `USER`/`ROLE`
/// with `PASSWORD`, MySQL's and MariaDB's `CREATE`/`ALTER USER`, `GRANT` and
/// `SET PASSWORD` with `IDENTIFIED BY` and the like, and SQL Server's
/// `CREATE`/`ALTER LOGIN` with `PASSWORD` and `OLD_PASSWORD`. Each statement
/// of a script is looked at on its own, and everything else is kept as typed.
pub fn mask_passwords(dialect: Dialect, sql: &str) -> String {
    if !dialect.supports_sql() {
        return sql.to_string();
    }
    let mut masked = String::with_capacity(sql.len());
    let mut copied = 0;
    let mut searched = 0;
    for statement in statement_stream::split_script(dialect, sql) {
        // Statements are trimmed slices of the script, in order.
        let Some(found) = sql.get(searched..).and_then(|rest| rest.find(&statement)) else {
            continue;
        };
        let start = searched + found;
        searched = start + statement.len();
        for range in password_ranges(dialect, &statement) {
            masked.push_str(&sql[copied..start + range.start]);
            masked.push_str(MASK);
            copied = start + range.end;
        }
    }
    masked.push_str(&sql[copied..]);
    masked
}

/// Which statements hold a password, by their first words.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `CREATE`/`ALTER` `USER`/`ROLE`/`LOGIN`, and MySQL's `GRANT`.
    Account,
    /// MySQL's `SET PASSWORD [FOR user] = 'secret'`.
    SetPassword,
}

/// The words a password literal follows, possibly after `=` or `(`:
/// `PASSWORD 'x'`, `PASSWORD = 'x'`, `IDENTIFIED BY 'x'`,
/// `IDENTIFIED VIA ed25519 USING PASSWORD('x')`, `IDENTIFIED WITH p AS 'hash'`,
/// `REPLACE 'current'` and SQL Server's `OLD_PASSWORD = 'x'`.
const PASSWORD_WORDS: [&str; 6] = ["PASSWORD", "OLD_PASSWORD", "BY", "USING", "AS", "REPLACE"];

/// The byte ranges in `statement` of the password literals to mask.
fn password_ranges(dialect: Dialect, statement: &str) -> Vec<std::ops::Range<usize>> {
    let parser_dialect = dialect.sqlparser_dialect();
    let tokens = match Tokenizer::new(parser_dialect.as_ref(), statement).tokenize_with_location() {
        Ok(tokens) => tokens,
        Err(_) => return unlexed_password_range(statement).into_iter().collect(),
    };
    let ranges = token_ranges(statement, &tokens);
    let code: Vec<(&Token, std::ops::Range<usize>)> = tokens
        .iter()
        .zip(ranges)
        .filter(|(token, _)| !matches!(token.token, Token::Whitespace(_)))
        .map(|(token, range)| (&token.token, range))
        .collect();

    let Some(kind) = statement_kind(code.iter().map(|(token, _)| *token)) else {
        return Vec::new();
    };
    let mut masked = Vec::new();
    let mut after_password_word = false;
    for (token, range) in code {
        if is_string_literal(token) {
            if after_password_word {
                masked.push(range);
            }
            after_password_word = false;
            continue;
        }
        after_password_word = match token {
            Token::Word(word) if word.quote_style.is_none() => PASSWORD_WORDS
                .iter()
                .any(|keyword| word.value.eq_ignore_ascii_case(keyword)),
            Token::Eq if kind == Kind::SetPassword => true,
            Token::Eq | Token::LParen => after_password_word,
            _ => false,
        };
    }
    masked
}

fn statement_kind<'a>(mut words: impl Iterator<Item = &'a Token>) -> Option<Kind> {
    let mut next_word = || match words.next() {
        Some(Token::Word(word)) if word.quote_style.is_none() => {
            Some(word.value.to_ascii_uppercase())
        }
        _ => None,
    };
    match next_word()?.as_str() {
        "GRANT" => Some(Kind::Account),
        "SET" => (next_word()? == "PASSWORD").then_some(Kind::SetPassword),
        "CREATE" | "ALTER" => {
            let mut object = next_word()?;
            // MariaDB's CREATE OR REPLACE USER.
            if object == "OR" {
                next_word()?;
                object = next_word()?;
            }
            matches!(object.as_str(), "USER" | "ROLE" | "LOGIN").then_some(Kind::Account)
        }
        _ => None,
    }
}

fn is_string_literal(token: &Token) -> bool {
    matches!(
        token,
        Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::TripleSingleQuotedString(_)
            | Token::TripleDoubleQuotedString(_)
            | Token::DollarQuotedString(_)
            | Token::NationalStringLiteral(_)
            | Token::EscapedStringLiteral(_)
            | Token::UnicodeStringLiteral(_)
            | Token::HexStringLiteral(_)
    )
}

/// The byte range of each token. A token ends where the next one starts:
/// whitespace and comments are tokens too, so together they cover the text.
fn token_ranges(statement: &str, tokens: &[TokenWithSpan]) -> Vec<std::ops::Range<usize>> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(statement.match_indices('\n').map(|(index, _)| index + 1))
        .collect();
    let offset = |token: &TokenWithSpan| {
        let location = token.span.start;
        let line_start = usize::try_from(location.line)
            .ok()
            .and_then(|line| line.checked_sub(1))
            .and_then(|line| line_starts.get(line))
            .copied()?;
        let column = usize::try_from(location.column).ok()?.checked_sub(1)?;
        let line = statement.get(line_start..)?;
        Some(
            line.char_indices()
                .nth(column)
                .map_or(statement.len(), |(index, _)| line_start + index),
        )
    };
    let starts: Vec<usize> = tokens
        .iter()
        .map(|token| offset(token).unwrap_or(statement.len()))
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).copied().unwrap_or(statement.len());
            start..end.max(start)
        })
        .collect()
}

/// A statement the tokenizer gives up on, such as one whose password quote
/// is never closed, still must not keep its password: from the first quote
/// after `PASSWORD` or `IDENTIFIED` to the end of the statement is masked.
fn unlexed_password_range(statement: &str) -> Option<std::ops::Range<usize>> {
    let upper = statement.to_ascii_uppercase();
    let keyword = ["PASSWORD", "IDENTIFIED"]
        .iter()
        .filter_map(|keyword| upper.find(keyword))
        .min()?;
    let quote = statement[keyword..].find(['\'', '"', '$'])? + keyword;
    Some(quote..statement.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatabaseType;

    fn mask(db_type: DatabaseType, sql: &str) -> String {
        mask_passwords(db_type.dialect(), sql)
    }

    #[test]
    fn postgres_roles_lose_their_passwords() {
        for (sql, expected) in [
            (
                "CREATE USER app WITH PASSWORD 'hunter2'",
                "CREATE USER app WITH PASSWORD '********'",
            ),
            (
                "create role app login encrypted password 'md5abc' valid until '2027-01-01'",
                "create role app login encrypted password '********' valid until '2027-01-01'",
            ),
            (
                "ALTER USER \"App\" PASSWORD E'it''s\\'s'",
                "ALTER USER \"App\" PASSWORD '********'",
            ),
            (
                "ALTER ROLE app WITH PASSWORD $pw$s3cr;et$pw$",
                "ALTER ROLE app WITH PASSWORD '********'",
            ),
            (
                "ALTER ROLE app PASSWORD NULL",
                "ALTER ROLE app PASSWORD NULL",
            ),
        ] {
            assert_eq!(mask(DatabaseType::PostgreSQL, sql), expected, "{sql}");
        }
        assert_eq!(
            mask(
                DatabaseType::CockroachDb,
                "CREATE USER app WITH PASSWORD 'x'"
            ),
            "CREATE USER app WITH PASSWORD '********'"
        );
    }

    #[test]
    fn mysql_accounts_lose_their_passwords_but_keep_their_names() {
        for (sql, expected) in [
            (
                "CREATE USER 'app'@'%' IDENTIFIED BY 'hunter2'",
                "CREATE USER 'app'@'%' IDENTIFIED BY '********'",
            ),
            (
                "CREATE OR REPLACE USER app IDENTIFIED BY \"hunter2\"",
                "CREATE OR REPLACE USER app IDENTIFIED BY '********'",
            ),
            (
                "ALTER USER 'app'@'%' IDENTIFIED WITH caching_sha2_password BY 'new' REPLACE 'old'",
                "ALTER USER 'app'@'%' IDENTIFIED WITH caching_sha2_password BY '********' REPLACE '********'",
            ),
            (
                "CREATE USER app IDENTIFIED BY PASSWORD '*94BDCEBE19083CE2A1F959FD02F964C7AF4CFC29'",
                "CREATE USER app IDENTIFIED BY PASSWORD '********'",
            ),
            (
                "CREATE USER app IDENTIFIED VIA ed25519 USING PASSWORD('hunter2')",
                "CREATE USER app IDENTIFIED VIA ed25519 USING PASSWORD('********')",
            ),
            (
                "SET PASSWORD FOR 'app'@'%' = 'hunter2'",
                "SET PASSWORD FOR 'app'@'%' = '********'",
            ),
            (
                "SET PASSWORD = PASSWORD('hunter2')",
                "SET PASSWORD = PASSWORD('********')",
            ),
            (
                "GRANT ALL ON db.* TO 'app'@'%' IDENTIFIED BY 'hunter2'",
                "GRANT ALL ON db.* TO 'app'@'%' IDENTIFIED BY '********'",
            ),
            (
                "CREATE USER app IDENTIFIED BY 'it\\'s'",
                "CREATE USER app IDENTIFIED BY '********'",
            ),
        ] {
            assert_eq!(mask(DatabaseType::MySQL, sql), expected, "{sql}");
        }
    }

    #[test]
    fn sql_server_logins_lose_their_passwords() {
        assert_eq!(
            mask(
                DatabaseType::MsSql,
                "ALTER LOGIN [app] WITH PASSWORD = N'new' OLD_PASSWORD = 'old', CHECK_POLICY = OFF"
            ),
            "ALTER LOGIN [app] WITH PASSWORD = '********' OLD_PASSWORD = '********', CHECK_POLICY = OFF"
        );
        assert_eq!(
            mask(
                DatabaseType::MsSql,
                "SELECT 1\nGO\nCREATE LOGIN app WITH PASSWORD = 'hunter2', DEFAULT_DATABASE = [db]\nGO"
            ),
            "SELECT 1\nGO\nCREATE LOGIN app WITH PASSWORD = '********', DEFAULT_DATABASE = [db]\nGO"
        );
    }

    #[test]
    fn scripts_are_masked_statement_by_statement_and_otherwise_kept() {
        let script = "-- set up\nSELECT 'PASSWORD' AS by_word;\n\
                      CREATE USER app WITH PASSWORD 'one';\n\n  \
                      UPDATE t SET password = 'kept' WHERE name = 'by';\n\
                      ALTER USER app PASSWORD 'two' -- trailing\n";
        assert_eq!(
            mask(DatabaseType::PostgreSQL, script),
            "-- set up\nSELECT 'PASSWORD' AS by_word;\n\
             CREATE USER app WITH PASSWORD '********';\n\n  \
             UPDATE t SET password = 'kept' WHERE name = 'by';\n\
             ALTER USER app PASSWORD '********' -- trailing\n"
        );
    }

    #[test]
    fn statements_without_passwords_and_other_backends_are_untouched() {
        for sql in [
            "SELECT * FROM users WHERE password = 'x'",
            "CREATE TABLE users (password text DEFAULT 'x')",
            "ALTER TABLE users ALTER COLUMN password SET DEFAULT 'x'",
            "CREATE USER app",
            "",
        ] {
            assert_eq!(mask(DatabaseType::PostgreSQL, sql), sql);
        }
        assert_eq!(mask(DatabaseType::Redis, "AUTH hunter2"), "AUTH hunter2");
    }

    #[test]
    fn an_unclosed_password_is_masked_to_the_end() {
        assert_eq!(
            mask(
                DatabaseType::PostgreSQL,
                "SELECT 1; CREATE USER app PASSWORD 'hunter2"
            ),
            "SELECT 1; CREATE USER app PASSWORD '********'"
        );
    }

    #[test]
    fn non_ascii_text_keeps_its_place() {
        assert_eq!(
            mask(
                DatabaseType::PostgreSQL,
                "SELECT 'åäö';\nCREATE USER \"Jörg\" PASSWORD 'pässword'"
            ),
            "SELECT 'åäö';\nCREATE USER \"Jörg\" PASSWORD '********'"
        );
    }
}
