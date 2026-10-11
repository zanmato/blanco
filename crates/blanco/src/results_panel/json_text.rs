//! Formatting and validating JSON cell text without parsing it into values.
//!
//! Going through `serde_json::Value` would sort object keys and round large
//! numbers through `f64`, so formatting works on the tokens instead and only
//! ever changes whitespace outside strings: keys keep their stored order and
//! numbers their exact digits.

/// `text` re-indented with two spaces, or `None` when it is not valid JSON.
pub fn format_json(text: &str) -> Option<String> {
    json_error(text).is_none().then(|| reflow(text, true))
}

/// `text` without whitespace outside strings, or `None` when it is not
/// valid JSON.
pub fn minify_json(text: &str) -> Option<String> {
    json_error(text).is_none().then(|| reflow(text, false))
}

/// Why `text` is not valid JSON, with its line and column, or `None` when it
/// is.
pub fn json_error(text: &str) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text)
        .err()
        .map(|error| error.to_string())
}

/// Whether a text value is a JSON object or array, which the cell editor
/// then treats like a JSON column.
pub fn is_json_document(text: &str) -> bool {
    let trimmed = text.trim_start();
    (trimmed.starts_with('{') || trimmed.starts_with('[')) && json_error(text).is_none()
}

fn reflow(text: &str, pretty: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut chars = text.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('"');
                while let Some(c) = chars.next() {
                    out.push(c);
                    match c {
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                out.push(escaped);
                            }
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                out.push(c);
                let close = if c == '{' { '}' } else { ']' };
                // Keep an empty object or array on one line.
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if chars.peek() == Some(&close) {
                    out.push(close);
                    chars.next();
                } else {
                    depth += 1;
                    if pretty {
                        newline(&mut out, depth);
                    }
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if pretty {
                    newline(&mut out, depth);
                }
                out.push(c);
            }
            ',' => {
                out.push(',');
                if pretty {
                    newline(&mut out, depth);
                }
            }
            ':' => {
                out.push(':');
                if pretty {
                    out.push(' ');
                }
            }
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_keeps_key_order_and_numbers() {
        let stored = r#"{"zeta":1,"alpha":{"big":12345678901234567890,"f":1.50},"list":[1, "a,b:{}", []],"e":{}}"#;
        assert_eq!(
            format_json(stored).unwrap(),
            r#"{
  "zeta": 1,
  "alpha": {
    "big": 12345678901234567890,
    "f": 1.50
  },
  "list": [
    1,
    "a,b:{}",
    []
  ],
  "e": {}
}"#
        );
    }

    #[test]
    fn test_minify_undoes_format() {
        let stored = r#"{"a":[1,2,{"b":"x \"q\" y"}],"c":null}"#;
        let formatted = format_json(stored).unwrap();
        assert_eq!(minify_json(&formatted).unwrap(), stored);
        assert_eq!(minify_json("  [ 1 ,\n 2 ]  ").unwrap(), "[1,2]");
    }

    #[test]
    fn test_invalid_json_is_reported() {
        assert!(format_json(r#"{"a": }"#).is_none());
        let error = json_error("{\n  \"a\": 1,\n}").unwrap();
        assert!(error.contains("line 3"), "{error}");
        assert!(json_error(r#""just a string""#).is_none());
    }

    #[test]
    fn test_is_json_document() {
        assert!(is_json_document(r#" {"a": 1}"#));
        assert!(is_json_document("[1]"));
        assert!(!is_json_document("42"), "a scalar stays plain text");
        assert!(!is_json_document("{not json"));
    }
}
