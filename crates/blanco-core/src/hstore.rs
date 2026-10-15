//! PostgreSQL's `hstore` text form: `"key"=>"value", "other"=>NULL`.
//!
//! Parsing accepts what the server accepts on input: keys and values quoted
//! with `"` and backslash escapes, or bare words, whitespace around the
//! tokens, and an unquoted `NULL` (any case) as a value. Formatting writes
//! the form the server returns, every key and value quoted.

/// A key and its value, `None` for SQL NULL. Order is kept as written.
pub type Pair = (String, Option<String>);

/// Why a text is not an hstore, with the byte offset it went wrong at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: &'static str,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at offset {}", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

pub fn parse(text: &str) -> Result<Vec<Pair>, ParseError> {
    let mut parser = Parser { text, offset: 0 };
    let mut pairs = Vec::new();
    parser.skip_whitespace();
    if parser.at_end() {
        return Ok(pairs);
    }
    loop {
        let key = parser.token()?.0;
        parser.skip_whitespace();
        parser.expect("=>")?;
        parser.skip_whitespace();
        let (value, quoted) = parser.token()?;
        let value = (quoted || !value.eq_ignore_ascii_case("null")).then_some(value);
        pairs.push((key, value));
        parser.skip_whitespace();
        if parser.at_end() {
            return Ok(pairs);
        }
        parser.expect(",")?;
        parser.skip_whitespace();
    }
}

/// `pairs` as the server writes them.
pub fn format(pairs: &[Pair]) -> String {
    let mut text = String::new();
    for (ix, (key, value)) in pairs.iter().enumerate() {
        if ix > 0 {
            text.push_str(", ");
        }
        push_quoted(key, &mut text);
        text.push_str("=>");
        match value {
            Some(value) => push_quoted(value, &mut text),
            None => text.push_str("NULL"),
        }
    }
    text
}

fn push_quoted(value: &str, text: &mut String) {
    text.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            text.push('\\');
        }
        text.push(character);
    }
    text.push('"');
}

struct Parser<'a> {
    text: &'a str,
    offset: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.offset..]
    }

    fn at_end(&self) -> bool {
        self.offset >= self.text.len()
    }

    fn error(&self, message: &'static str) -> ParseError {
        ParseError {
            offset: self.offset,
            message,
        }
    }

    fn skip_whitespace(&mut self) {
        let rest = self.rest();
        self.offset += rest.len() - rest.trim_start().len();
    }

    fn expect(&mut self, token: &str) -> Result<(), ParseError> {
        if self.rest().starts_with(token) {
            self.offset += token.len();
            Ok(())
        } else if token == "=>" {
            Err(self.error("expected =>"))
        } else {
            Err(self.error("expected , between pairs"))
        }
    }

    /// A quoted or bare key or value, and whether it was quoted.
    fn token(&mut self) -> Result<(String, bool), ParseError> {
        let mut value = String::new();
        let mut characters = self.rest().char_indices();
        match characters.next() {
            None => Err(self.error("expected a key or value")),
            Some((_, '"')) => {
                let mut escaped = false;
                for (ix, character) in characters {
                    match character {
                        _ if escaped => {
                            value.push(character);
                            escaped = false;
                        }
                        '\\' => escaped = true,
                        '"' => {
                            self.offset += ix + 1;
                            return Ok((value, true));
                        }
                        _ => value.push(character),
                    }
                }
                Err(self.error("unterminated quoted string"))
            }
            Some(_) => {
                // A bare word runs to whitespace, `=>` or `,`; backslashes
                // escape the next character as in quoted strings.
                let rest = self.rest();
                let mut end = rest.len();
                let mut escaped = false;
                for (ix, character) in rest.char_indices() {
                    if escaped {
                        value.push(character);
                        escaped = false;
                        continue;
                    }
                    if character == '\\' {
                        escaped = true;
                        continue;
                    }
                    if character.is_whitespace() || character == ',' || rest[ix..].starts_with("=>")
                    {
                        end = ix;
                        break;
                    }
                    value.push(character);
                }
                if end == 0 {
                    return Err(self.error("expected a key or value"));
                }
                self.offset += end;
                Ok((value, false))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Pair, format, parse};

    fn pairs(items: &[(&str, Option<&str>)]) -> Vec<Pair> {
        items
            .iter()
            .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
            .collect()
    }

    #[test]
    fn reads_the_servers_output() {
        assert_eq!(
            parse(r#""a"=>"1", "b c"=>NULL, "empty"=>"", "q\"uote"=>"x,y""#),
            Ok(pairs(&[
                ("a", Some("1")),
                ("b c", None),
                ("empty", Some("")),
                ("q\"uote", Some("x,y")),
            ]))
        );
        assert_eq!(parse(""), Ok(Vec::new()));
        assert_eq!(parse("  "), Ok(Vec::new()));
    }

    #[test]
    fn reads_what_the_server_accepts_as_input() {
        assert_eq!(
            parse(r#"a=>1,b=>NULL , c => "NULL", d=>null, e\ f=>back\\slash"#),
            Ok(pairs(&[
                ("a", Some("1")),
                ("b", None),
                ("c", Some("NULL")),
                ("d", None),
                ("e f", Some("back\\slash")),
            ]))
        );
    }

    #[test]
    fn says_where_a_text_is_not_an_hstore() {
        assert_eq!(
            parse("a").map_err(|error| error.message),
            Err("expected =>")
        );
        assert_eq!(
            parse(r#""a"=>"1" "b"=>"2""#).map_err(|error| error.offset),
            Err(9)
        );
        assert_eq!(
            parse(r#""a"=>"unterminated"#).map_err(|error| error.message),
            Err("unterminated quoted string")
        );
        assert_eq!(parse("=>1").map_err(|error| error.offset), Err(0));
    }

    #[test]
    fn writes_what_it_reads() {
        let written = pairs(&[
            ("a", Some("1")),
            ("with \"quotes\" and \\", Some("line\nbreak")),
            ("null value", None),
            ("", Some("")),
        ]);
        let text = format(&written);
        assert_eq!(
            text,
            "\"a\"=>\"1\", \"with \\\"quotes\\\" and \\\\\"=>\"line\nbreak\", \"null value\"=>NULL, \"\"=>\"\""
        );
        assert_eq!(parse(&text), Ok(written));
        assert_eq!(format(&[]), "");
    }
}
