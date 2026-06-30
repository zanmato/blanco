//! Redis command tokenizing and reply-to-`QueryResult` shaping.

use anyhow::{Result, anyhow};
use blanco_core::{ColumnType, QueryResult};
use fred::types::{Key, Value};

/// Split a single Redis command line into its arguments, honoring single and
/// double quotes (redis-cli style). Quotes group whitespace-separated tokens;
/// a backslash inside double quotes escapes the next character.
pub fn tokenize(line: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                has_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                has_token = true;
            }
            '\\' if in_double => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }

    if in_single || in_double {
        return Err(anyhow!("unbalanced quotes in command"));
    }
    if has_token {
        args.push(current);
    }
    Ok(args)
}

/// Render a Redis map key (always a byte string) as a display string.
pub fn key_to_string(key: &Key) -> String {
    String::from_utf8_lossy(key.as_bytes()).into_owned()
}

/// Render a single Redis value as a display string. Nested containers are
/// rendered compactly; `Null` becomes `None` (SQL NULL).
pub fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Integer(i) => Some(i.to_string()),
        Value::Double(d) => Some(d.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        Value::String(s) => Some(s.to_string()),
        Value::Bytes(b) => Some(String::from_utf8_lossy(b).to_string()),
        Value::Queued => Some("QUEUED".to_string()),
        Value::Array(items) => Some(
            items
                .iter()
                .map(|v| value_to_string(v).unwrap_or_else(|| "(nil)".to_string()))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Value::Map(map) => Some(
            map.iter()
                .map(|(k, v)| {
                    format!(
                        "{}={}",
                        key_to_string(k),
                        value_to_string(v).unwrap_or_else(|| "(nil)".to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        ),
    }
}

/// Shape an arbitrary Redis reply into a tabular `QueryResult` for the console.
/// Scalars become a single `value` cell, arrays one `value` row per element,
/// and maps a two-column `field`/`value` grid.
pub fn reply_to_query_result(command: &str, value: Value) -> QueryResult {
    match value {
        Value::Array(items) => {
            let rows = items
                .into_iter()
                .map(|item| vec![value_to_string(&item)])
                .collect::<Vec<_>>();
            QueryResult {
                columns: vec!["value".to_string()],
                column_types: vec![ColumnType::Text],
                rows,
                query_text: Some(command.to_string()),
                ..Default::default()
            }
        }
        Value::Map(map) => {
            let rows = map
                .iter()
                .map(|(k, v)| vec![Some(key_to_string(k)), value_to_string(v)])
                .collect::<Vec<_>>();
            QueryResult {
                columns: vec!["field".to_string(), "value".to_string()],
                column_types: vec![ColumnType::Text, ColumnType::Text],
                rows,
                query_text: Some(command.to_string()),
                ..Default::default()
            }
        }
        scalar => QueryResult {
            columns: vec!["value".to_string()],
            column_types: vec![ColumnType::Text],
            rows: vec![vec![value_to_string(&scalar)]],
            query_text: Some(command.to_string()),
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple() {
        assert_eq!(tokenize("GET foo").unwrap(), vec!["GET", "foo"]);
    }

    #[test]
    fn tokenize_quotes() {
        assert_eq!(
            tokenize(r#"SET key "hello world""#).unwrap(),
            vec!["SET", "key", "hello world"]
        );
        assert_eq!(
            tokenize("HSET h field 'a b'").unwrap(),
            vec!["HSET", "h", "field", "a b"]
        );
    }

    #[test]
    fn tokenize_empty_quoted_arg() {
        assert_eq!(tokenize(r#"SET k """#).unwrap(), vec!["SET", "k", ""]);
    }

    #[test]
    fn tokenize_unbalanced_errors() {
        assert!(tokenize(r#"SET k "open"#).is_err());
    }

    #[test]
    fn scalar_reply_single_cell() {
        let result = reply_to_query_result("GET k", Value::String("v".into()));
        assert_eq!(result.columns, vec!["value"]);
        assert_eq!(result.rows, vec![vec![Some("v".to_string())]]);
    }

    #[test]
    fn array_reply_one_row_each() {
        let value = Value::Array(vec![Value::String("a".into()), Value::String("b".into())]);
        let result = reply_to_query_result("LRANGE l 0 -1", value);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0], vec![Some("a".to_string())]);
    }
}
