use anyhow::{anyhow, Result};
use blanco_core::Position;
use sql_parse::{parse_statement, ParseOptions, Statement, TableReference};

/// Completion context types for SQLite
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionKind {
    /// Table name completion (after FROM, JOIN, INTO, UPDATE)
    Table,
    /// Column name completion (after SELECT, WHERE, ORDER BY, GROUP BY, HAVING)
    Column,
    /// Column name completion with table qualification (table.column)
    QualifiedColumn,
    /// Schema name completion (main, temp, attached databases)
    Schema,
    /// Alias name completion (after AS)
    Alias,
    /// General keyword completion
    Keyword,
    /// SQLite-specific pragma completion
    Pragma,
    /// No specific context detected
    Unknown,
}

/// Information about table aliases in the current query
#[derive(Debug, Clone)]
pub struct TableAlias {
    /// The alias name
    pub alias: String,
    /// The actual table name it refers to
    pub table_name: String,
    /// The position where the alias was defined
    pub position: Position,
}

/// Parsed information about the current SQL query
#[derive(Debug, Clone)]
pub struct ParsedQuery {
    /// The type of statement (SELECT, INSERT, UPDATE, DELETE, etc.)
    pub statement_type: String,
    /// Tables referenced in the query
    pub tables: Vec<String>,
    /// Table aliases used in the query
    pub aliases: Vec<TableAlias>,
    /// The context for completion at the current position
    pub completion_context: CompletionKind,
    /// The word at the current position
    pub current_word: Option<String>,
    /// Available databases/schemas for completion (main, temp, attached)
    pub available_databases: Vec<String>,
}

/// SQLite-specific SQL table extractor
pub struct SqliteTableExtractor {
}

impl SqliteTableExtractor {
    pub fn new() -> Self {
        Self { }
    }

    /// Extract the primary table name from a SELECT query
    pub fn extract_primary_table(&self, sql: &str) -> Result<String> {
        let options = ParseOptions::new();
        let mut issues = Vec::new();

        if let Some(statement) = parse_statement(sql, &mut issues, &options) {
            if let Some(table_name) = self.extract_from_statement(&statement) {
                return Ok(table_name);
            }
        }

        Err(anyhow!("Could not extract table name from query"))
    }

    /// Parse the query to understand completion and hover context
    pub fn parse_query_context(&self, text: &str, position: Position) -> Result<ParsedQuery> {
        let lines: Vec<&str> = text.lines().collect();

        if position.line as usize >= lines.len() {
            return Err(anyhow!("Position out of bounds"));
        }

        let current_line = lines[position.line as usize];
        let current_word = self.extract_word_at_position(current_line, position.character);

        // Get text up to current position for context analysis
        let text_up_to_position = self.get_text_up_to_position(text, position);

        // Parse the main statement
        let options = ParseOptions::new();
        let mut issues = Vec::new();

        let (statement_type, tables, aliases) = if let Some(statement) = parse_statement(&text_up_to_position, &mut issues, &options) {
            (
                self.get_statement_type(&statement),
                self.extract_tables_from_statement(&statement),
                self.extract_aliases_from_statement(&statement)
            )
        } else {
            // Fallback for incomplete or invalid SQL
            ("UNKNOWN".to_string(), Vec::new(), Vec::new())
        };

        // Detect completion context
        let completion_context = self.detect_completion_context(text, position, &current_word);

        // Get available databases (main, temp, and any attached databases)
        let available_databases = vec!["main".to_string(), "temp".to_string()];

        Ok(ParsedQuery {
            statement_type,
            tables,
            aliases,
            completion_context,
            current_word,
            available_databases,
        })
    }

    /// Get the word at the given position
    pub fn extract_word_at_position(&self, text: &str, character: u32) -> Option<String> {
        let char_pos = character as usize;

        if char_pos >= text.len() {
            return None;
        }

        // Find word boundaries including dots for qualified names
        let start = text[..char_pos]
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map(|i| i + 1)
            .unwrap_or(0);

        let end = text[char_pos..]
            .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map(|i| char_pos + i)
            .unwrap_or(text.len());

        if start < end && start <= char_pos && char_pos <= end {
            Some(text[start..end].to_string())
        } else {
            None
        }
    }

    /// Get text up to the given position
    fn get_text_up_to_position(&self, text: &str, position: Position) -> String {
        let lines: Vec<&str> = text.lines().collect();
        let mut result = String::new();

        for (i, line) in lines.iter().enumerate() {
            if i < position.line as usize {
                result.push_str(line);
                result.push(' ');
            } else if i == position.line as usize {
                let char_pos = position.character as usize;
                if char_pos <= line.len() {
                    result.push_str(&line[..char_pos]);
                } else {
                    result.push_str(line);
                }
                break;
            }
        }

        result
    }

    /// Extract table name from parsed statement
    fn extract_from_statement(&self, statement: &Statement) -> Option<String> {
        match statement {
            Statement::Select(select) => self.extract_from_select(select),
            Statement::InsertReplace(insert) => {
                // Handle SQLite-specific INSERT syntax
                insert.table.last().map(|last_id| last_id.value.to_string())
            }
            Statement::Update(update) => {
                // Handle SQLite-specific UPDATE syntax
                if let Some(first_table) = update.tables.first() {
                    self.extract_from_table_reference(first_table)
                } else {
                    None
                }
            }
            Statement::Delete(delete) => {
                // Handle SQLite-specific DELETE syntax
                if let Some(first_table_vec) = delete.tables.first() {
                    first_table_vec
                        .last()
                        .map(|last_id| last_id.value.to_string())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Extract table name from a SELECT statement
    fn extract_from_select(&self, select: &sql_parse::Select) -> Option<String> {
        if let Some(table_references) = &select.table_references {
            if let Some(first_table) = table_references.first() {
                return self.extract_from_table_reference(first_table);
            }
        }
        None
    }

    /// Extract table name from a table reference
    fn extract_from_table_reference(&self, table_ref: &TableReference) -> Option<String> {
        match table_ref {
            TableReference::Table {
                identifier, as_, ..
            } => {
                if let Some(alias) = as_ {
                    Some(alias.value.to_string())
                } else {
                    identifier.last().map(|last_id| last_id.value.to_string())
                }
            }
            TableReference::Query { query, as_, .. } => {
                if let Some(alias) = as_ {
                    Some(alias.value.to_string())
                } else {
                    self.extract_from_statement(query)
                }
            }
            TableReference::Join { left, right, .. } => {
                self.extract_from_table_reference(left)
                    .or_else(|| self.extract_from_table_reference(right))
            }
        }
    }

    /// Extract all tables from a statement
    fn extract_tables_from_statement(&self, statement: &Statement) -> Vec<String> {
        let mut tables = Vec::new();

        match statement {
            Statement::Select(select) => {
                if let Some(table_references) = &select.table_references {
                    for table_ref in table_references {
                        self.extract_tables_from_reference(table_ref, &mut tables);
                    }
                }
            }
            Statement::InsertReplace(insert) => {
                if let Some(table_name) = insert.table.last() {
                    tables.push(table_name.value.to_string());
                }
            }
            Statement::Update(_update) => {
                // Skip UPDATE for now - table extraction is complex
            }
            Statement::Delete(_delete) => {
                // Skip DELETE for now - table extraction is complex
            }
            _ => {}
        }

        tables
    }

    /// Extract tables from a table reference
    fn extract_tables_from_reference(&self, table_ref: &TableReference, tables: &mut Vec<String>) {
        match table_ref {
            TableReference::Table { identifier, .. } => {
                if let Some(table_name) = identifier.last() {
                    tables.push(table_name.value.to_string());
                }
            }
            TableReference::Query { query, .. } => {
                // Extract tables from subquery
                let sub_tables = self.extract_tables_from_statement(query);
                tables.extend(sub_tables);
            }
            TableReference::Join { left, right, .. } => {
                self.extract_tables_from_reference(left, tables);
                self.extract_tables_from_reference(right, tables);
            }
        }
    }

    /// Extract table aliases from a statement
    fn extract_aliases_from_statement(&self, statement: &Statement) -> Vec<TableAlias> {
        let mut aliases = Vec::new();

        if let Statement::Select(select) = statement {
            if let Some(table_references) = &select.table_references {
                for table_ref in table_references {
                    self.extract_aliases_from_reference(table_ref, &mut aliases);
                }
            }
        }

        aliases
    }

    /// Extract aliases from a table reference
    #[allow(clippy::only_used_in_recursion)]
    fn extract_aliases_from_reference(&self, table_ref: &TableReference, aliases: &mut Vec<TableAlias>) {
        match table_ref {
            TableReference::Table { identifier, as_, .. } => {
                if let Some(alias) = as_ {
                    let table_name = identifier
                        .last()
                        .map(|id| id.value.to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    aliases.push(TableAlias {
                        alias: alias.value.to_string(),
                        table_name,
                        position: Position::new(0, 0), // Would need proper position tracking
                    });
                }
            }
            TableReference::Query { as_, .. } => {
                if let Some(alias) = as_ {
                    aliases.push(TableAlias {
                        alias: alias.value.to_string(),
                        table_name: "subquery".to_string(),
                        position: Position::new(0, 0),
                    });
                }
            }
            TableReference::Join { left, right, .. } => {
                self.extract_aliases_from_reference(left, aliases);
                self.extract_aliases_from_reference(right, aliases);
            }
        }
    }

    /// Get the statement type as a string
    fn get_statement_type(&self, statement: &Statement) -> String {
        match statement {
            Statement::Select(_) => "SELECT",
            Statement::InsertReplace(_) => "INSERT",
            Statement::Update(_) => "UPDATE",
            Statement::Delete(_) => "DELETE",
            // Statement::Create(_) => "CREATE",  // Not supported in current sql-parse version
            // Statement::Drop(_) => "DROP",    // Not supported in current sql-parse version
            // Statement::Alter(_) => "ALTER",  // Not supported in current sql-parse version
            _ => "OTHER",
        }.to_string()
    }

    /// Detect completion context using SQLite-specific parsing
    fn detect_completion_context(&self, text: &str, position: Position, _current_word: &Option<String>) -> CompletionKind {
        let lines: Vec<&str> = text.lines().collect();

        if position.line as usize >= lines.len() {
            return CompletionKind::Unknown;
        }

        let current_line = lines[position.line as usize];
        let before_cursor = &current_line[..(position.character as usize).min(current_line.len())];
        let before_cursor_lower = before_cursor.to_lowercase();

        // Check for PRAGMA completion (SQLite-specific)
        if before_cursor_lower.trim().starts_with("pragma") {
            return CompletionKind::Pragma;
        }

        // Check for qualified column completion (table.column)
        if let Some(dot_pos) = before_cursor.rfind('.') {
            let before_dot = &before_cursor[..dot_pos];
            let potential_table = before_dot.split_whitespace().last().unwrap_or("");

            // Check if it's a table name or alias (simplified)
            if !potential_table.is_empty() && !["from", "join", "into", "update"].contains(&potential_table.to_lowercase().as_str()) {
                return CompletionKind::QualifiedColumn;
            }
        }

        // Get text up to current position for context analysis
        let text_up_to_position = self.get_text_up_to_position(text, position);
        let tokens: Vec<&str> = text_up_to_position.split_whitespace().collect();

        // Check for completion contexts based on last tokens
        if let Some(last_token) = tokens.last() {
            match last_token.to_lowercase().as_str() {
                "from" | "join" | "inner" | "left" | "right" | "full" | "into" | "update" => {
                    return CompletionKind::Table;
                }
                "as" => return CompletionKind::Alias,
                "select" | "where" | "having" => return CompletionKind::Column,
                "pragma" => return CompletionKind::Pragma,
                _ => {}
            }
        }

        // Check for multi-word contexts
        if tokens.len() >= 2 {
            let last_two = &tokens[tokens.len()-2..];
            if (last_two[0].to_lowercase() == "order" && last_two[1].to_lowercase() == "by") ||
               (last_two[0].to_lowercase() == "group" && last_two[1].to_lowercase() == "by") {
                return CompletionKind::Column;
            }
        }

        if tokens.len() >= 3 {
            let last_three = &tokens[tokens.len()-3..];
            if (last_three[0].to_lowercase() == "inner" && last_three[1].to_lowercase() == "join") ||
               (last_three[0].to_lowercase() == "left" && last_three[1].to_lowercase() == "join") ||
               (last_three[0].to_lowercase() == "right" && last_three[1].to_lowercase() == "join") ||
               (last_three[0].to_lowercase() == "full" && last_three[1].to_lowercase() == "join") {
                return CompletionKind::Table;
            }
        }

        CompletionKind::Keyword
    }
}

impl Default for SqliteTableExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqlite_table_extraction() {
        let extractor = SqliteTableExtractor::new();

        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM users").unwrap(),
            "users"
        );

        assert_eq!(
            extractor.extract_primary_table("SELECT * FROM sqlite_master").unwrap(),
            "sqlite_master"
        );
    }

    #[test]
    fn test_pragma_detection() {
        let extractor = SqliteTableExtractor::new();
        let position = Position::new(0, 10);
        let parsed = extractor.parse_query_context("PRAGMA table_", position).unwrap();

        assert_eq!(parsed.completion_context, CompletionKind::Pragma);
    }

    #[test]
    fn test_qualified_column_detection() {
        let extractor = SqliteTableExtractor::new();
        let position = Position::new(0, 20);
        let parsed = extractor.parse_query_context("SELECT users. FROM users", position).unwrap();

        assert_eq!(parsed.completion_context, CompletionKind::QualifiedColumn);
    }
}