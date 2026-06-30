//! This crate provides Redis language support for the [tree-sitter][] parsing library.
//!
//! Typically, you will use the [LANGUAGE][] constant to add this language to a
//! tree-sitter [Parser][], and then use the parser to parse some code:
//!
//! ```
//! let code = r#"
//! SET key "hello world"
//! CONFIG GET maxmemory
//! "#;
//! let mut parser = tree_sitter::Parser::new();
//! let language = tree_sitter_redis::LANGUAGE;
//! parser
//!     .set_language(&language.into())
//!     .expect("Error loading Redis parser");
//! let tree = parser.parse(code, None).unwrap();
//! assert!(!tree.root_node().has_error());
//! ```
//!
//! [Parser]: https://docs.rs/tree-sitter/*/tree_sitter/struct.Parser.html
//! [tree-sitter]: https://tree-sitter.github.io/

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_redis() -> *const ();
}

/// The tree-sitter [`LanguageFn`][LanguageFn] for this grammar.
///
/// [LanguageFn]: https://docs.rs/tree-sitter-language/*/tree_sitter_language/struct.LanguageFn.html
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_redis) };

/// The content of the [`node-types.json`][] file for this grammar.
///
/// [`node-types.json`]: https://tree-sitter.github.io/tree-sitter/using-parsers#static-node-types
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

/// The syntax highlighting query for this grammar.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

pub mod commands;

/// Subcommands for a container command (e.g. `CONFIG`, `CLIENT`), looked up
/// case-insensitively. Returns `None` for plain commands and unknown names.
pub fn subcommands_for(command: &str) -> Option<&'static [&'static str]> {
    let command = command.to_ascii_uppercase();
    commands::SUBCOMMANDS
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, subs)| *subs)
}

#[cfg(test)]
mod tests {
    use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

    fn parse(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("Error loading Redis parser");
        parser.parse(source, None).expect("parse should succeed")
    }

    /// Collect `(kind, text)` for every named node, depth-first.
    fn named_nodes<'a>(node: Node<'a>, source: &'a str, out: &mut Vec<(String, String)>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() && child.child_count() == 0 {
                out.push((
                    child.kind().to_string(),
                    child.utf8_text(source.as_bytes()).unwrap().to_string(),
                ));
            }
            named_nodes(child, source, out);
        }
    }

    fn tokens(source: &str) -> Vec<(String, String)> {
        let tree = parse(source);
        let mut out = Vec::new();
        named_nodes(tree.root_node(), source, &mut out);
        out
    }

    #[test]
    fn test_can_load_grammar() {
        let mut parser = Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("Error loading Redis parser");
    }

    #[test]
    fn highlights_query_is_valid() {
        Query::new(&super::LANGUAGE.into(), super::HIGHLIGHTS_QUERY)
            .expect("highlights.scm should compile against the grammar");
    }

    #[test]
    fn simple_command_and_argument() {
        assert_eq!(
            tokens("GET foo"),
            vec![
                ("command_name".into(), "GET".into()),
                ("bare_word".into(), "foo".into()),
            ]
        );
    }

    #[test]
    fn commands_are_case_insensitive() {
        // Lowercase command and container/subcommand still classify as keywords.
        assert_eq!(tokens("get foo")[0].0, "command_name");
        let config = tokens("config get maxmemory");
        assert_eq!(config[0], ("container_command".into(), "config".into()));
        assert_eq!(config[1], ("subcommand".into(), "get".into()));
    }

    #[test]
    fn double_quoted_string_with_escapes() {
        // Mirrors crates/redis/src/command.rs::tokenize quoting rules.
        assert_eq!(
            tokens(r#"SET key "hello \"world\"""#),
            vec![
                ("command_name".into(), "SET".into()),
                ("bare_word".into(), "key".into()),
                ("string".into(), r#""hello \"world\"""#.into()),
            ]
        );
    }

    #[test]
    fn single_quoted_string_is_literal() {
        let toks = tokens("HSET h field 'a b'");
        assert_eq!(
            toks.last().unwrap(),
            &("string".to_string(), "'a b'".to_string())
        );
    }

    #[test]
    fn empty_quoted_argument() {
        assert_eq!(tokens(r#"SET k """#).last().unwrap().0, "string");
    }

    #[test]
    fn integers_and_floats() {
        let toks = tokens("LRANGE mylist 0 -1");
        assert_eq!(toks[2], ("integer".into(), "0".into()));
        assert_eq!(toks[3], ("integer".into(), "-1".into()));
        assert_eq!(tokens("SET counter 3.14").last().unwrap().0, "float");
    }

    #[test]
    fn container_command_with_arguments() {
        assert_eq!(
            tokens("CLIENT KILL ID 42"),
            vec![
                ("container_command".into(), "CLIENT".into()),
                ("subcommand".into(), "KILL".into()),
                ("bare_word".into(), "ID".into()),
                ("integer".into(), "42".into()),
            ]
        );
    }

    #[test]
    fn unknown_command_still_parses() {
        let toks = tokens("TOTALLYNOTACOMMAND arg1 arg2");
        assert_eq!(
            toks[0],
            ("unknown_command".into(), "TOTALLYNOTACOMMAND".into())
        );
        assert!(!parse("TOTALLYNOTACOMMAND arg1 arg2")
            .root_node()
            .has_error());
    }

    #[test]
    fn multiline_script_has_no_errors() {
        let script = "SET a 1\nGET a\nCONFIG GET maxmemory\n\nDEL a\n";
        assert!(!parse(script).root_node().has_error());
        // Each command keeps its own command node.
        let tree = parse(script);
        let mut cursor = tree.root_node().walk();
        let commands = tree
            .root_node()
            .children(&mut cursor)
            .filter(|n| n.kind() == "command")
            .count();
        assert_eq!(commands, 4);
    }

    #[test]
    fn highlights_capture_command_as_keyword() {
        let source = "CONFIG GET maxmemory";
        let tree = parse(source);
        let query = Query::new(&super::LANGUAGE.into(), super::HIGHLIGHTS_QUERY).unwrap();
        let mut cursor = QueryCursor::new();
        let mut captures = Vec::new();
        let mut it = cursor.matches(&query, tree.root_node(), source.as_bytes());
        while let Some(m) = it.next() {
            for cap in m.captures {
                captures.push((
                    query.capture_names()[cap.index as usize].to_string(),
                    cap.node.utf8_text(source.as_bytes()).unwrap().to_string(),
                ));
            }
        }
        assert!(captures.contains(&("keyword".into(), "CONFIG".into())));
        assert!(captures.contains(&("function.call".into(), "GET".into())));
        assert!(captures.contains(&("variable".into(), "maxmemory".into())));
    }
}
