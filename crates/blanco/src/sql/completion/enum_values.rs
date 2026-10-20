//! Where the cursor is typing a value for a column: `status = '…`,
//! `status <> …`, `status IN ('open', '…` or `SET status = '…`. Completion
//! offers the labels of the column's enum type there. Read from the text
//! before the cursor alone, since a half typed string leaves the syntax tree
//! in error.

/// A value being typed for a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValueTarget {
    /// Where the column reference ends, for resolving it like a column being
    /// completed there.
    pub column_end: usize,
    /// The column's name, without quotes.
    pub column: String,
    /// Where the value's text starts: just past the opening quote, or where
    /// the word typed without one starts.
    pub value_start: usize,
    /// The value is typed inside quotes.
    pub quoted: bool,
}

/// The value the cursor at `offset` is typing in `text`, if it is one.
pub(super) fn value_target(text: &str, offset: usize) -> Option<ValueTarget> {
    let text = text.get(..offset)?;
    let (value_start, quoted) = match open_quote(text)? {
        Some(quote) => (quote + 1, true),
        None => {
            let word_start = text
                .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                .map_or(0, |index| {
                    index + text[index..].chars().next().map_or(1, char::len_utf8)
                });
            (word_start, false)
        }
    };
    // Where the value starts, the opening quote included.
    let value_begin = if quoted { value_start - 1 } else { value_start };
    let before = text[..value_begin].trim_end();

    let column_text = if let Some(rest) = strip_comparison(before) {
        rest
    } else {
        strip_in_list(before)?
    };
    let column_text = column_text.trim_end();
    let column = column_before(column_text)?;
    Some(ValueTarget {
        column_end: column_text.len(),
        column,
        value_start,
        quoted,
    })
}

/// `Some(Some(index))` when the cursor is inside a single quoted string that
/// opens at `index`, `Some(None)` outside strings, and `None` inside a
/// comment or quoted identifier, where nothing is offered.
fn open_quote(text: &str) -> Option<Option<usize>> {
    #[derive(PartialEq)]
    enum State {
        Code,
        String(usize),
        Identifier,
        LineComment,
        BlockComment,
    }
    let bytes = text.as_bytes();
    let mut state = State::Code;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            State::Code => match (byte, next) {
                (b'\'', _) => state = State::String(index),
                (b'"', _) => state = State::Identifier,
                (b'-', Some(b'-')) => state = State::LineComment,
                (b'/', Some(b'*')) => {
                    state = State::BlockComment;
                    index += 1;
                }
                _ => {}
            },
            State::String(_) => {
                if byte == b'\'' {
                    if next == Some(b'\'') {
                        index += 1;
                    } else {
                        state = State::Code;
                    }
                }
            }
            State::Identifier => {
                if byte == b'"' {
                    state = State::Code;
                }
            }
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Code;
                }
            }
            State::BlockComment => {
                if byte == b'*' && next == Some(b'/') {
                    state = State::Code;
                    index += 1;
                }
            }
        }
        index += 1;
    }
    match state {
        State::Code => Some(None),
        State::String(start) => Some(Some(start)),
        State::Identifier | State::LineComment | State::BlockComment => None,
    }
}

/// The text before `=`, `<>` or `!=` that `before` ends with.
fn strip_comparison(before: &str) -> Option<&str> {
    ["<>", "!=", "="]
        .iter()
        .find_map(|operator| before.strip_suffix(operator))
        // `<=` and `>=` compare, but offering labels there is not useful.
        .filter(|rest| !rest.ends_with(['<', '>']))
}

/// The text before `IN (` when `before` ends inside an `IN` list, after
/// `(` or after a complete value and its `,`.
fn strip_in_list(before: &str) -> Option<&str> {
    let mut rest = before;
    loop {
        if let Some(open) = rest.strip_suffix('(') {
            let open = open.trim_end();
            let keyword_start = open.len().checked_sub(2)?;
            let keyword = open.get(keyword_start..)?;
            let separated = open[..keyword_start]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            return (keyword.eq_ignore_ascii_case("in") && separated)
                .then(|| open[..keyword_start].trim_end())
                .map(|column| {
                    // `NOT IN` compares the same column.
                    column
                        .len()
                        .checked_sub(3)
                        .filter(|start| {
                            column.get(*start..).is_some_and(|word| {
                                word.eq_ignore_ascii_case("not")
                                    && column[..*start]
                                        .chars()
                                        .next_back()
                                        .is_none_or(char::is_whitespace)
                            })
                        })
                        .map_or(column, |start| column[..start].trim_end())
                });
        }
        let item = rest.strip_suffix(',')?.trim_end();
        rest = skip_value_back(item)?.trim_end();
    }
}

/// `text` without the value it ends with: a quoted string or a bare word.
fn skip_value_back(text: &str) -> Option<&str> {
    if let Some(inside) = text.strip_suffix('\'') {
        // Back to the opening quote, stepping over doubled quotes.
        let bytes = inside.as_bytes();
        let mut index = bytes.len();
        while index > 0 {
            index -= 1;
            if bytes[index] == b'\'' {
                if index > 0 && bytes[index - 1] == b'\'' {
                    index -= 1;
                    continue;
                }
                return Some(&text[..index]);
            }
        }
        return None;
    }
    let start = text
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '-'))
        .map_or(0, |index| index + 1);
    (start < text.len()).then(|| &text[..start])
}

/// The unquoted last name part of the column reference `text` ends with:
/// `status` of `status`, `t.status` or `"status"`.
fn column_before(text: &str) -> Option<String> {
    let name = if let Some(inside) = text.strip_suffix('"') {
        let open = inside.rfind('"')?;
        inside[open + 1..].to_string()
    } else {
        let start = text
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .map_or(0, |index| index + 1);
        let name = &text[start..];
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        name.to_string()
    };
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text_with_cursor: &str) -> Option<(String, String, bool)> {
        let offset = text_with_cursor.find('|').expect("cursor");
        let text = text_with_cursor.replace('|', "");
        value_target(&text, offset).map(|target| {
            (
                text[..target.column_end].to_string(),
                target.column,
                target.quoted,
            )
        })
    }

    #[test]
    fn values_compared_with_a_column_are_found() {
        assert_eq!(
            target("SELECT * FROM t WHERE status = 'op|"),
            Some(("SELECT * FROM t WHERE status".into(), "status".into(), true))
        );
        assert_eq!(
            target("SELECT * FROM t x WHERE x.status<>|"),
            Some((
                "SELECT * FROM t x WHERE x.status".into(),
                "status".into(),
                false
            ))
        );
        assert_eq!(
            target("UPDATE t SET \"Status\" = 'on ho|"),
            Some(("UPDATE t SET \"Status\"".into(), "Status".into(), true))
        );
        assert_eq!(
            target("SELECT 1 WHERE status != cl|"),
            Some(("SELECT 1 WHERE status".into(), "status".into(), false))
        );
    }

    #[test]
    fn values_in_an_in_list_are_found() {
        assert_eq!(
            target("WHERE status IN ('open', 'it''s, x', '|"),
            Some(("WHERE status".into(), "status".into(), true))
        );
        assert_eq!(
            target("WHERE t.status NOT IN (|"),
            Some(("WHERE t.status".into(), "status".into(), false))
        );
        assert_eq!(target("WHERE status_in (|"), None);
        assert_eq!(target("SELECT coalesce(|"), None);
    }

    #[test]
    fn other_places_are_not_values() {
        assert_eq!(target("SELECT 'a = b|"), None, "the = is inside a string");
        assert_eq!(target("SELECT status FROM t WHERE |"), None);
        assert_eq!(target("WHERE total >= |"), None);
        assert_eq!(target("WHERE status = 1 -- = '|"), None);
        assert_eq!(target("WHERE \"a = |"), None);
        assert_eq!(target("WHERE 1 = '|"), None, "a number is no column");
    }
}
