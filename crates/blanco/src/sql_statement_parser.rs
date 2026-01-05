use once_cell::sync::Lazy;
use ropey::Rope;
use std::cell::RefCell;
use std::ops::Range;
use tree_sitter::{Language, Node, Parser};

/// Parameter style in SQL query
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterStyle {
    Positional(usize), // $1, $2, etc.
    Named(String),     // :name, @name, etc.
}

/// A parameter found in a SQL query
#[derive(Clone, Debug)]
pub struct QueryParameter {
    pub style: ParameterStyle,
    pub raw_text: String,   // Original text from query (e.g., "$1", ":user_id")
    pub byte_offset: usize, // Byte offset of the parameter in the original query
}

/// Information about an extracted SQL statement
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StatementInfo {
    pub text: String,
    pub byte_range: Range<usize>,
    pub utf16_range: Range<usize>,
    pub is_complete: bool,
    pub parameters: Vec<QueryParameter>,
}

/// SQL statement parser using tree-sitter for accurate statement extraction
struct SqlStatementParser {
    parser: Parser,
}

static SQL_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_sequel::LANGUAGE.into());

// Thread-local parser instance for reuse.
// Each thread gets its own parser, avoiding contention and ensuring thread safety.
thread_local! {
    static TLS_PARSER: RefCell<SqlStatementParser> = {
        match SqlStatementParser::new() {
            Ok(parser) => RefCell::new(parser),
            Err(e) => {
                tracing::error!("Failed to create thread-local SQL parser: {}", e);
                // Panic in the unlikely case the parser cannot be created.
                // This is acceptable because a parser failure is a fatal error.
                panic!("Failed to initialize thread-local SQL parser: {}", e);
            }
        }
    };
}

impl SqlStatementParser {
    /// Create a new SQL statement parser
    fn new() -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&SQL_LANGUAGE)
            .map_err(|e| format!("Failed to set SQL language: {}", e))?;

        Ok(Self { parser })
    }

    /// Extract the SQL statement at the given cursor byte position
    pub fn extract_statement_at_cursor(
        &mut self,
        text: &str,
        cursor_byte_pos: usize,
    ) -> Option<StatementInfo> {
        // Parse the text
        let tree = self.parser.parse(text, None)?;
        let root_node = tree.root_node();

        // Find all statement nodes
        let mut statements = Vec::new();
        self.collect_statements(root_node, &mut statements);

        // Find the statement that contains or is closest to the cursor
        let mut best_statement = None;
        let mut best_distance = usize::MAX;

        for statement in statements {
            let range = statement.byte_range();

            // If cursor is inside the statement, return it immediately
            if range.contains(&cursor_byte_pos) {
                let rope = Rope::from_str(text);
                let utf16_range =
                    rope.byte_to_utf16_idx(range.start)..rope.byte_to_utf16_idx(range.end);
                let statement_text = text[range.clone()].to_string();
                let is_complete = self.is_statement_complete(&statement_text);
                // Extract parameters and adjust their offsets to be relative to the statement text
                let mut parameters = self.extract_parameters_from_node(statement, text);
                // Adjust byte offsets to be relative to the statement text (not the full text)
                for param in &mut parameters {
                    param.byte_offset = param.byte_offset.saturating_sub(range.start);
                }
                return Some(StatementInfo {
                    text: statement_text,
                    byte_range: range,
                    utf16_range,
                    is_complete,
                    parameters,
                });
            }

            // Calculate distance from cursor to this statement
            let distance = if cursor_byte_pos < range.start {
                range.start - cursor_byte_pos
            } else {
                cursor_byte_pos - range.end
            };

            // Keep track of the closest statement
            if distance < best_distance {
                best_distance = distance;
                best_statement = Some(statement);
            }
        }

        // Return the closest statement (if any)
        if let Some(statement) = best_statement {
            let range = statement.byte_range();
            let rope = Rope::from_str(text);
            let utf16_range =
                rope.byte_to_utf16_idx(range.start)..rope.byte_to_utf16_idx(range.end);
            let statement_text = text[range.clone()].to_string();
            let is_complete = self.is_statement_complete(&statement_text);
            // Extract parameters and adjust their offsets to be relative to the statement text
            let mut parameters = self.extract_parameters_from_node(statement, text);
            // Adjust byte offsets to be relative to the statement text (not the full text)
            for param in &mut parameters {
                param.byte_offset = param.byte_offset.saturating_sub(range.start);
            }
            Some(StatementInfo {
                text: statement_text,
                byte_range: range,
                utf16_range,
                is_complete,
                parameters,
            })
        } else {
            None
        }
    }

    /// Collect all statement nodes from the tree
    fn collect_statements<'a>(&self, node: Node<'a>, statements: &mut Vec<Node<'a>>) {
        // Check if this node is a statement
        if self.is_statement_node(node.kind()) {
            statements.push(node);
        }

        // Recursively check children
        for i in 0.. {
            if let Some(child) = node.child(i) {
                self.collect_statements(child, statements);
            } else {
                break;
            }
        }
    }

    /// Check if a node kind represents a SQL statement
    fn is_statement_node(&self, kind: &str) -> bool {
        matches!(
            kind,
            "statement"
                | "select_statement"
                | "insert_statement"
                | "update_statement"
                | "delete_statement"
                | "create_statement"
                | "drop_statement"
                | "alter_statement"
                | "truncate_statement"
                | "merge_statement"
                | "transaction_statement"
                | "compound_statement"
        )
    }

    /// Check if a statement is complete (has proper termination)
    fn is_statement_complete(&self, statement: &str) -> bool {
        let trimmed = statement.trim();

        // Empty statements are not complete
        if trimmed.is_empty() {
            return false;
        }

        // Since we're extracting statement nodes from tree-sitter,
        // they represent complete statements regardless of semicolon presence
        true
    }

    /// Extract parameters from a statement node
    fn extract_parameters_from_node(
        &self,
        node: tree_sitter::Node,
        text: &str,
    ) -> Vec<QueryParameter> {
        let mut parameters = Vec::new();
        self.collect_parameters(node, text, &mut parameters);

        // Sort by byte offset ascending for user display (in order of appearance)
        parameters.sort_by_key(|p| p.byte_offset);
        parameters
    }

    /// Recursively collect parameters from the AST
    fn collect_parameters<'a>(
        &self,
        node: tree_sitter::Node<'a>,
        text: &str,
        parameters: &mut Vec<QueryParameter>,
    ) {
        // Check if this node represents a parameter
        if let Some(param) = self.try_parse_parameter(node, text) {
            // Deduplicate by byte offset - each position can only have one parameter
            let already_seen = parameters
                .iter()
                .any(|p| p.byte_offset == param.byte_offset);

            if !already_seen {
                parameters.push(param);
            }
        }

        // Recursively check children
        for i in 0.. {
            if let Some(child) = node.child(i) {
                self.collect_parameters(child, text, parameters);
            } else {
                break;
            }
        }
    }

    /// Try to parse a node as a parameter
    fn try_parse_parameter(&self, node: tree_sitter::Node, text: &str) -> Option<QueryParameter> {
        let kind = node.kind();
        let byte_range = node.byte_range();

        // Clamp the byte range to the text length to avoid panic
        // This handles edge cases where tree-sitter returns ranges extending beyond text
        let end = byte_range.end.min(text.len());
        let start = byte_range.start.min(end);
        let node_text = &text[start..end];

        // Check for tree-sitter parameter node types
        if kind == "positional_parameter" || kind == "bind_parameter" {
            return self.parse_parameter_text(node_text, start);
        }

        // Fallback: check for dollar-number pattern or named parameter patterns in text
        if let Some(param) = self.parse_parameter_text(node_text, start) {
            return Some(param);
        }

        None
    }

    /// Parse parameter text into a QueryParameter
    fn parse_parameter_text(&self, text: &str, byte_offset: usize) -> Option<QueryParameter> {
        let trimmed = text.trim();

        // Standalone ? parameter (JDBC style)
        if trimmed == "?" {
            return Some(QueryParameter {
                style: ParameterStyle::Positional(0),
                raw_text: trimmed.to_string(),
                byte_offset,
            });
        }

        // Positional parameter: $1, $2, etc.
        if let Some(rest) = trimmed.strip_prefix('$')
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            && let Ok(index) = rest.parse::<usize>()
        {
            return Some(QueryParameter {
                style: ParameterStyle::Positional(index),
                raw_text: trimmed.to_string(),
                byte_offset,
            });
        }

        // Named parameter: :name, @name, etc. (?name is also supported as named)
        if trimmed.starts_with(':') || trimmed.starts_with('@') || trimmed.starts_with('?') {
            let name = trimmed[1..].to_string();
            if !name.is_empty()
                && name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_')
            {
                return Some(QueryParameter {
                    style: ParameterStyle::Named(name),
                    raw_text: trimmed.to_string(),
                    byte_offset,
                });
            }
        }

        None
    }
}

/// Extract statement info at cursor position using thread-local parser.
///
/// # Arguments
/// * `text` - Full SQL text
/// * `cursor_pos` - Cursor position in characters (not bytes)
///
/// # Returns
/// The statement info, or None if no statement found
pub fn extract_statement_info(text: &str, cursor_pos: usize) -> Option<StatementInfo> {
    TLS_PARSER.with_borrow_mut(|parser| {
        // Use Rope for efficient character to byte position conversion
        let rope = Rope::from_str(text);
        let cursor_byte_pos = if cursor_pos < rope.len_chars() {
            rope.char_to_byte_idx(cursor_pos)
        } else {
            text.len()
        };

        parser.extract_statement_at_cursor(text, cursor_byte_pos)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_parser() -> SqlStatementParser {
        SqlStatementParser::new().expect("Failed to create test parser")
    }

    /// Helper to convert character position to byte position for tests
    fn char_to_byte_pos(text: &str, char_pos: usize) -> usize {
        let rope = Rope::from_str(text);
        if char_pos < rope.len_chars() {
            rope.char_to_byte_idx(char_pos)
        } else {
            text.len()
        }
    }

    #[test]
    fn test_simple_select() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users;";
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 10));

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
        assert!(info.is_complete);
    }

    #[test]
    fn test_insert_with_semicolon_in_string() {
        let mut parser = create_test_parser();
        let text = "INSERT INTO orders (a) VALUES ('hello;');";
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 25));

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "INSERT INTO orders (a) VALUES ('hello;')");
        assert!(info.is_complete);
    }

    #[test]
    fn test_multiple_statements() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM table1; INSERT INTO table2 (col) VALUES ('test;'); UPDATE table3 SET col = 'value';";

        // Test cursor in first statement
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 10));
        assert!(result.is_some());
        assert!(
            result
                .unwrap()
                .text
                .trim()
                .starts_with("SELECT * FROM table1")
        );

        // Test cursor in second statement
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 50));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim(),
            "INSERT INTO table2 (col) VALUES ('test;')"
        );
        assert!(info.is_complete);

        // Test cursor in third statement
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 85));
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(info.text.trim().starts_with("UPDATE table3"));
        assert!(info.is_complete);
    }

    #[test]
    fn test_comments_with_semicolons() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users -- This comment has a semicolon;";
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 15));

        assert!(result.is_some());
        let info = result.unwrap();
        // Should extract the SELECT statement part
        assert!(info.text.trim().contains("SELECT * FROM users"));
        // Comment may or may not be included depending on tree-sitter parsing
        // The important part is that the comment semicolon doesn't break statement detection
    }

    #[test]
    fn test_incomplete_statement() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users WHERE id = 1";
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users WHERE id = 1");
        assert!(info.is_complete); // No semicolon but still a complete statement
    }

    #[test]
    fn test_empty_text() {
        let mut parser = create_test_parser();
        let result = parser.extract_statement_at_cursor("", 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_cursor_before_semicolon() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users;";

        // Position cursor just before the semicolon (after 'users')
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 18));
        assert!(
            result.is_some(),
            "Should extract statement when cursor is before semicolon"
        );
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
        assert!(info.is_complete);

        // Also test at the semicolon position
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 19));
        assert!(
            result.is_some(),
            "Should extract statement when cursor is at semicolon position"
        );
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
        assert!(info.is_complete);
    }

    #[test]
    fn test_cursor_before_semicolon_with_whitespace() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users   ;";

        // Position cursor at the first space after 'users'
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 16));
        assert!(
            result.is_some(),
            "Should extract statement when cursor is before whitespace and semicolon"
        );
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
        assert!(info.is_complete);
    }

    #[test]
    fn test_convenience_function() {
        let text = "SELECT * FROM users; INSERT INTO table (col) VALUES ('hello; world');";
        // Position 50 should be in the INSERT statement part
        // "SELECT * FROM users; " = 21 chars, so position 50 is well into the INSERT
        let result = extract_current_query(text, 50, false);
        assert_eq!(
            result.trim(),
            "INSERT INTO table (col) VALUES ('hello; world')"
        );
    }

    #[test]
    fn test_unicode_utf16_indices() {
        let mut parser = create_test_parser();
        // Test with Unicode characters (emoji and non-ASCII) in a valid SQL statement
        let text = "SELECT * FROM testing WHERE text_col = '🏠';";

        // Position cursor in the middle of the statement
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));

        assert!(
            result.is_some(),
            "Should extract statement with Unicode characters"
        );
        let info = result.unwrap();

        // Verify both byte and UTF-16 ranges are provided
        assert!(info.byte_range.start < info.byte_range.end);
        assert!(info.utf16_range.start < info.utf16_range.end);
        assert_eq!(
            info.text.trim(),
            "SELECT * FROM testing WHERE text_col = '🏠'"
        );
        assert!(info.is_complete);

        // Verify UTF-16 range exists and is reasonable
        assert!(info.utf16_range.end > info.utf16_range.start);
        // The house emoji 🏠 should be represented as a surrogate pair in UTF-16
        // So the UTF-16 length should be greater than the byte length / 4 (approximation)
        assert!(info.utf16_range.end > info.byte_range.end / 3);
    }

    #[test]
    fn test_multiline_statements() {
        let mut parser = create_test_parser();
        let text = "INSERT INTO orders (a)
VALUES ('hello;');

SELECT * FROM users; -- some comment

DELETE FROM users WHERE id = 1;";

        // Test cursor on first line (INSERT)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 5));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim().replace('\n', " "),
            "INSERT INTO orders (a) VALUES ('hello;')"
        );
        assert!(info.is_complete);

        // Test cursor on second line (VALUES part of INSERT)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 30));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim().replace('\n', " "),
            "INSERT INTO orders (a) VALUES ('hello;')"
        );
        assert!(info.is_complete);

        // Test cursor on third line (empty line after INSERT semicolon)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 40));
        assert!(result.is_some());
        let info = result.unwrap();
        // The INSERT statement is still the closest at this position
        assert!(info.text.trim().contains("INSERT INTO orders"));
        assert!(info.is_complete);

        // Test cursor on fourth line (SELECT)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 45));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
        assert!(info.is_complete);

        // Test cursor on fifth line (comment after SELECT)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 60));
        assert!(result.is_some());
        let info = result.unwrap();
        // Should still return the SELECT statement even with cursor in comment
        assert!(info.text.trim().contains("SELECT * FROM users"));
        assert!(info.is_complete);

        // Test cursor on sixth line (empty line before DELETE)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 80));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DELETE FROM users WHERE id = 1");
        assert!(info.is_complete);

        // Test cursor on seventh line (DELETE)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 85));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DELETE FROM users WHERE id = 1");
        assert!(info.is_complete);
    }

    #[test]
    fn test_mixed_formatting_statements() {
        let mut parser = create_test_parser();
        // Test statements with various formatting styles
        let text = "  CREATE TABLE test (
            id INTEGER PRIMARY KEY,
            name TEXT
        );  -- Create test table

        UPDATE test SET name = 'test' WHERE id = 1;

        DROP TABLE test;";

        // Test cursor in CREATE (first line)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 4));
        assert!(result.is_some());
        let info = result.unwrap();
        // Multi-line CREATE statement may not include the semicolon
        assert!(info.text.trim().starts_with("CREATE TABLE test"));
        assert!(info.is_complete);

        // Test cursor in CREATE (middle)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 30));
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(info.text.trim().starts_with("CREATE TABLE test"));
        assert!(info.is_complete);

        // Test cursor in UPDATE
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 120));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim(),
            "UPDATE test SET name = 'test' WHERE id = 1"
        );
        assert!(info.is_complete);

        // Test cursor in DROP
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 175));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DROP TABLE test");
        assert!(info.is_complete);
    }

    #[test]
    fn test_no_whitespace_between_statements() {
        let mut parser = create_test_parser();
        // Test statements with minimal whitespace
        let text = "SELECT a FROM b;INSERT INTO c VALUES (1);DELETE FROM d WHERE e = 2;";

        // Test cursor in SELECT
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 5));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT a FROM b");
        assert!(info.is_complete);

        // Test cursor in INSERT (position 20)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "INSERT INTO c VALUES (1)");
        assert!(info.is_complete);

        // Test cursor in DELETE (position 45, well into DELETE)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 45));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DELETE FROM d WHERE e = 2");
        assert!(info.is_complete);
    }

    #[test]
    fn test_update_with_uuid_string() {
        let mut parser = create_test_parser();
        // This query caused a panic: byte index 177 is out of bounds
        // The issue was that tree-sitter returned a node with byte range extending beyond text length
        let text = "UPDATE alternative_images ai SET updated_at = NOW() WHERE id = 'f893fd7a-45a2-4747-a726-5561bb4735ec'";

        // Test cursor at various positions in the statement
        for cursor_pos in [0, 10, 30, 50, 70, 90, text.len().saturating_sub(1)] {
            let result =
                parser.extract_statement_at_cursor(text, char_to_byte_pos(text, cursor_pos));
            assert!(
                result.is_some(),
                "Should extract statement at cursor position {}",
                cursor_pos
            );
            let info = result.unwrap();
            assert_eq!(
                info.text.trim(),
                "UPDATE alternative_images ai SET updated_at = NOW() WHERE id = 'f893fd7a-45a2-4747-a726-5561bb4735ec'"
            );
            assert!(info.is_complete);
            // Should have no parameters
            assert!(info.parameters.is_empty());
        }
    }

    #[test]
    fn test_parameter_extraction_with_special_characters() {
        let mut parser = create_test_parser();
        // Test that single character named params don't panic
        // Note: :a is NOT recognized as a parameter by tree-sitter SQL grammar
        let text = "SELECT * FROM users WHERE name = :a";

        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users WHERE name = :a");
        // tree-sitter doesn't classify :a as a parameter node
        assert_eq!(info.parameters.len(), 0);

        // Test with positional parameter (PostgreSQL style)
        // Note: $1 IS recognized as a positional_parameter by tree-sitter
        let text = "SELECT * FROM users WHERE id = $1";
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));
        assert!(result.is_some());
        let info = result.unwrap();
        // PostgreSQL $1 IS recognized
        assert_eq!(info.parameters.len(), 1);
        assert_eq!(info.parameters[0].raw_text, "$1");
        matches!(info.parameters[0].style, ParameterStyle::Positional(1));

        // Test with multiple parameters
        let text = "SELECT * FROM users WHERE id = $1 AND name = $2";
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.parameters.len(), 2);
        // Parameters are sorted in ascending byte offset order (for user display)
        assert_eq!(info.parameters[0].raw_text, "$1");
        assert_eq!(info.parameters[1].raw_text, "$2");
        // Verify byte offsets are in ascending order
        assert!(info.parameters[0].byte_offset < info.parameters[1].byte_offset);
    }

    #[test]
    fn test_parameter_extraction_bound_parameter() {
        let mut parser = create_test_parser();
        // Test with ? style parameter
        // Note: ? IS recognized as a bind_parameter by tree-sitter SQL grammar
        let text = "SELECT * FROM users WHERE id = ?";

        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));
        assert!(result.is_some());
        let info = result.unwrap();
        // ? IS recognized as a bind_parameter
        assert_eq!(info.parameters.len(), 1);
        assert_eq!(info.parameters[0].raw_text, "?");
        // Note: ? is parsed as a positional parameter with index 0 (no number in it)
        matches!(info.parameters[0].style, ParameterStyle::Positional(0));

        // Test with multiple ? parameters
        let text = "SELECT * FROM users WHERE id = ? AND name = ?";
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 20));
        assert!(result.is_some());
        let info = result.unwrap();
        // Multiple ? are recognized as separate bind_parameter nodes
        assert_eq!(info.parameters.len(), 2);
    }
}
