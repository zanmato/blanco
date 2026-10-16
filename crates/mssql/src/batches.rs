//! `GO` batch separators. SQL Server never sees `GO`: SSMS and sqlcmd split a
//! script on lines holding only `GO` (optionally `GO <count>`) and send each
//! batch on its own. Some statements must be alone in their batch, such as
//! `SET SHOWPLAN_ALL` and `CREATE PROCEDURE`.

/// One batch of a script and how many times `GO <count>` asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub sql: String,
    pub count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Code,
    SingleQuoted,
    DoubleQuoted,
    Bracketed,
    /// T-SQL block comments nest.
    BlockComment(u32),
}

/// Split `sql` on its `GO` lines. A `GO` inside a string, quoted identifier
/// or block comment does not count. Batches holding only whitespace are left
/// out, and a script without `GO` is one batch.
pub fn split_batches(sql: &str) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut state = State::Code;
    for line in sql.split_inclusive('\n') {
        if state == State::Code
            && let Some(count) = go_separator(line)
        {
            push_batch(&mut batches, &mut current, count);
            continue;
        }
        state = scan_line(line, state);
        current.push_str(line);
    }
    push_batch(&mut batches, &mut current, 1);
    batches
}

fn push_batch(batches: &mut Vec<Batch>, current: &mut String, count: u32) {
    let sql = std::mem::take(current);
    if !sql.trim().is_empty() {
        batches.push(Batch { sql, count });
    }
}

/// The repeat count of a `GO` line, or `None` when `line` is not one.
fn go_separator(line: &str) -> Option<u32> {
    let code = match line.find("--") {
        Some(comment) => &line[..comment],
        None => line,
    };
    let mut words = code.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("GO") {
        return None;
    }
    let count = match words.next() {
        Some(count) => count.parse().ok().filter(|count| *count > 0)?,
        None => 1,
    };
    words.next().is_none().then_some(count)
}

/// The state at the end of `line` when it starts in `state`.
fn scan_line(line: &str, mut state: State) -> State {
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        state = match state {
            State::Code => match c {
                '\'' => State::SingleQuoted,
                '"' => State::DoubleQuoted,
                '[' => State::Bracketed,
                '-' if chars.peek() == Some(&'-') => return State::Code,
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    State::BlockComment(1)
                }
                _ => State::Code,
            },
            // A doubled quote is an escaped one and reads as leaving and
            // entering the string again, which ends in the same state.
            State::SingleQuoted if c == '\'' => State::Code,
            State::DoubleQuoted if c == '"' => State::Code,
            State::Bracketed if c == ']' => State::Code,
            State::BlockComment(depth) => match c {
                '*' if chars.peek() == Some(&'/') => {
                    chars.next();
                    if depth == 1 {
                        State::Code
                    } else {
                        State::BlockComment(depth - 1)
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    State::BlockComment(depth + 1)
                }
                _ => state,
            },
            _ => state,
        };
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqls(sql: &str) -> Vec<(String, u32)> {
        split_batches(sql)
            .into_iter()
            .map(|batch| (batch.sql.trim().to_string(), batch.count))
            .collect()
    }

    #[test]
    fn script_without_go_is_one_batch() {
        assert_eq!(
            sqls("SELECT 1; SELECT 2"),
            [("SELECT 1; SELECT 2".into(), 1)]
        );
        assert_eq!(sqls("  \n"), []);
    }

    #[test]
    fn go_lines_separate_batches() {
        assert_eq!(
            sqls("SET SHOWPLAN_ALL ON\nGO\nSELECT 1\n  go  \nSET SHOWPLAN_ALL OFF"),
            [
                ("SET SHOWPLAN_ALL ON".into(), 1),
                ("SELECT 1".into(), 1),
                ("SET SHOWPLAN_ALL OFF".into(), 1),
            ]
        );
        assert_eq!(
            sqls("INSERT INTO t DEFAULT VALUES\nGO 3 -- three rows\nGO\n"),
            [("INSERT INTO t DEFAULT VALUES".into(), 3)]
        );
    }

    #[test]
    fn go_that_is_not_a_separator() {
        for sql in [
            "SELECT 'a\nGO\nb'",
            "SELECT [a\nGO\nb]",
            "/* outer /* inner */\nGO\n*/ SELECT 1",
            "SELECT 1 GO",
            "GOTO done",
            "GO 0",
            "GO x",
        ] {
            assert_eq!(split_batches(sql).len(), 1, "{sql}");
        }
        assert_eq!(
            sqls("SELECT 'it''s' -- GO\nGO\nSELECT 2"),
            [("SELECT 'it''s' -- GO".into(), 1), ("SELECT 2".into(), 1)]
        );
    }
}
