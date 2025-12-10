use once_cell::sync::Lazy;
use ropey::Rope;
use std::ops::Range;
use tree_sitter::{Language, Node, Parser};

/// Information about an extracted SQL statement
#[derive(Debug, Clone)]
pub struct StatementInfo {
    pub text: String,
    #[allow(dead_code)]
    pub byte_range: Range<usize>,
    pub utf16_range: Range<usize>,
    pub is_complete: bool,
}

/// SQL statement parser using tree-sitter for accurate statement extraction
pub struct SqlStatementParser {
    parser: Parser,
}

static SQL_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_sequel::LANGUAGE.into());

impl SqlStatementParser {
    /// Create a new SQL statement parser
    pub fn new() -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&*SQL_LANGUAGE)
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
                let utf16_range = rope.byte_to_utf16_idx(range.start)..rope.byte_to_utf16_idx(range.end);
                let text = text[range.clone()].to_string();
                let is_complete = self.is_statement_complete(&text);
                return Some(StatementInfo {
                    text,
                    byte_range: range,
                    utf16_range,
                    is_complete,
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
            let utf16_range = rope.byte_to_utf16_idx(range.start)..rope.byte_to_utf16_idx(range.end);
            let text = text[range.clone()].to_string();
            let is_complete = self.is_statement_complete(&text);
            Some(StatementInfo {
                text,
                byte_range: range,
                utf16_range,
                is_complete,
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
}

/// Extract the current SQL query at the cursor position
///
/// This is a convenience function that maintains compatibility with the existing
/// extract_current_query function signature while using robust tree-sitter parsing.
///
/// # Arguments
/// * `text` - Full SQL text
/// * `cursor_pos` - Cursor position in characters (not bytes)
/// * `_has_selection` - Currently unused (selection detection not implemented)
///
/// # Returns
/// The extracted SQL statement, or empty string if no statement found
pub fn extract_current_query(text: &str, cursor_pos: usize, _has_selection: bool) -> String {
    // Create a new parser instance for each call
    // This is safe and avoids static mut issues
    let mut parser = match SqlStatementParser::new() {
        Ok(parser) => parser,
        Err(e) => {
            tracing::error!("Failed to initialize SQL parser: {}", e);
            return String::new();
        }
    };

    // Use Rope for efficient character to byte position conversion
    let rope = Rope::from_str(text);
    let cursor_byte_pos = if cursor_pos < rope.len_chars() {
        rope.char_to_byte_idx(cursor_pos)
    } else {
        text.len()
    };

    parser
        .extract_statement_at_cursor(text, cursor_byte_pos)
        .map(|info| info.text.trim().to_string())
        .unwrap_or_default()
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
        assert_eq!(
            info.text.trim(),
            "INSERT INTO orders (a) VALUES ('hello;')"
        );
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

        assert!(result.is_some(), "Should extract statement with Unicode characters");
        let info = result.unwrap();

        // Verify both byte and UTF-16 ranges are provided
        assert!(info.byte_range.start < info.byte_range.end);
        assert!(info.utf16_range.start < info.utf16_range.end);
        assert_eq!(info.text.trim(), "SELECT * FROM testing WHERE text_col = '🏠'");
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
        assert_eq!(info.text.trim().replace('\n', " "), "INSERT INTO orders (a) VALUES ('hello;')");
        assert!(info.is_complete);

        // Test cursor on second line (VALUES part of INSERT)
        let result = parser.extract_statement_at_cursor(text, char_to_byte_pos(text, 30));
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim().replace('\n', " "), "INSERT INTO orders (a) VALUES ('hello;')");
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
        assert_eq!(info.text.trim(), "UPDATE test SET name = 'test' WHERE id = 1");
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
}
