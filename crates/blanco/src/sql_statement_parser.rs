use once_cell::sync::Lazy;
use std::ops::Range;
use tree_sitter::{Language, Node, Parser};

/// Information about an extracted SQL statement
#[derive(Debug, Clone)]
pub struct StatementInfo {
    pub text: String,
    pub byte_range: Range<usize>,
    pub is_complete: bool,
}

/// SQL statement parser using tree-sitter for accurate statement extraction
pub struct SqlStatementParser {
    parser: Parser,
}

static SQL_LANGUAGE: Lazy<Language> = Lazy::new(|| {
    tree_sitter_sequel::LANGUAGE.into()
});

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

        // Find the deepest node that contains the cursor
        let mut current_node = root_node;
        let mut target_node = current_node;

        // Walk down to find the deepest node containing cursor
        loop {
            let mut found_child = false;
            let mut child_count = 0;

            for i in 0.. {
                if let Some(child) = current_node.child(i) {
                    child_count += 1;
                    if child.byte_range().contains(&cursor_byte_pos) {
                        current_node = child;
                        target_node = child;
                        found_child = true;
                        break;
                    }
                } else {
                    break;
                }
            }

            if !found_child || child_count == 0 {
                break;
            }
        }

        // Navigate up to find the statement-level node
        let statement_node = self.find_statement_node(target_node)?;

        // Extract the statement text and range
        let byte_range = statement_node.byte_range();
        let text = text[byte_range.clone()].to_string();

        // Check if the statement is complete (has semicolon or is the last statement)
        let is_complete = self.is_statement_complete(&text);

        Some(StatementInfo {
            text,
            byte_range,
            is_complete,
        })
    }

    /// Find the nearest statement-level parent node
    fn find_statement_node<'a>(&self, node: Node<'a>) -> Option<Node<'a>> {
        let mut current = node;

        loop {
            let kind = current.kind();

            // Check if this is a statement-level node
            if self.is_statement_node(kind) {
                return Some(current);
            }

            // Move to parent
            match current.parent() {
                Some(parent) => current = parent,
                None => return None,
            }
        }
    }

    /// Check if a node kind represents a SQL statement
    fn is_statement_node(&self, kind: &str) -> bool {
        matches!(
            kind,
            "statement" |
            "select_statement" |
            "insert_statement" |
            "update_statement" |
            "delete_statement" |
            "create_statement" |
            "drop_statement" |
            "alter_statement" |
            "truncate_statement" |
            "merge_statement" |
            "transaction_statement" |
            "compound_statement"
        )
    }

    /// Check if a statement is complete (has proper termination)
    fn is_statement_complete(&self, statement: &str) -> bool {
        let trimmed = statement.trim();

        // Empty statements are not complete
        if trimmed.is_empty() {
            return false;
        }

        // Check if statement ends with semicolon (outside of comments/strings)
        // For now, simple check - tree-sitter parsing already handles structure
        trimmed.ends_with(';') || !trimmed.contains(';')
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

    // Convert character position to byte position
    let cursor_byte_pos = text
        .char_indices()
        .nth(cursor_pos)
        .map(|(byte_pos, _)| byte_pos)
        .unwrap_or(text.len());

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

    #[test]
    fn test_simple_select() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users;";
        let result = parser.extract_statement_at_cursor(text, 10);

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users;");
        assert!(info.is_complete);
    }

    #[test]
    fn test_insert_with_semicolon_in_string() {
        let mut parser = create_test_parser();
        let text = "INSERT INTO orders (a) VALUES ('hello;');";
        let result = parser.extract_statement_at_cursor(text, 25); // Position in the middle

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "INSERT INTO orders (a) VALUES ('hello;');");
        assert!(info.is_complete);
    }

    #[test]
    fn test_multiple_statements() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM table1; INSERT INTO table2 (col) VALUES ('test;'); UPDATE table3 SET col = 'value';";

        // Test cursor in first statement
        let result = parser.extract_statement_at_cursor(text, 10);
        assert!(result.is_some());
        assert!(result.unwrap().text.trim().starts_with("SELECT * FROM table1"));

        // Test cursor in second statement (the problematic one)
        let result = parser.extract_statement_at_cursor(text, 50);
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "INSERT INTO table2 (col) VALUES ('test;');");
        assert!(info.is_complete);

        // Test cursor in third statement
        let result = parser.extract_statement_at_cursor(text, 85);
        assert!(result.is_some());
        assert!(result.unwrap().text.trim().starts_with("UPDATE table3"));
    }

    #[test]
    fn test_comments_with_semicolons() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users -- This comment has a semicolon;";
        let result = parser.extract_statement_at_cursor(text, 15);

        assert!(result.is_some());
        let info = result.unwrap();
        // Should extract the entire SELECT statement, not treat comment semicolon as terminator
        assert!(info.text.trim().contains("SELECT * FROM users"));
        assert!(info.text.trim().contains("-- This comment has a semicolon;"));
    }

    #[test]
    fn test_incomplete_statement() {
        let mut parser = create_test_parser();
        let text = "SELECT * FROM users WHERE id = 1";
        let result = parser.extract_statement_at_cursor(text, 20);

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
    fn test_convenience_function() {
        let text = "SELECT * FROM users; INSERT INTO table (col) VALUES ('hello; world');";
        let result = extract_current_query(text, 50, false); // Cursor in the problematic INSERT
        assert_eq!(result.trim(), "INSERT INTO table (col) VALUES ('hello; world');");
    }
}