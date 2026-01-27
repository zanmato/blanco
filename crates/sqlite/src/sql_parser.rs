use anyhow::{anyhow, Result};
use blanco_core::Position;
use sql_parse::{parse_statement, Issues, ParseOptions, Statement, TableReference};

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
pub struct SqliteTableExtractor {}

impl SqliteTableExtractor {
    pub fn new() -> Self {
        Self {}
    }

    /// Extract the table name from a SELECT query
    pub fn extract_table(&self, sql: &str, alias: bool) -> Result<String> {
        let options = ParseOptions::new();
        let mut issues = Issues::new(sql);

        if let Some(statement) = parse_statement(sql, &mut issues, &options) {
            if let Some(table_name) = self.extract_from_statement(&statement, alias) {
                return Ok(table_name);
            }
        }

        Err(anyhow!("Could not extract table name from query"))
    }

    /// Extract table aliases with their actual table names for resolution
    /// Returns Vec<(table_name, alias)> for easy lookup
    pub fn extract_table_aliases_with_names(&self, sql: &str) -> Result<Vec<(String, String)>> {
        let options = ParseOptions::new();
        let mut issues = Issues::new(sql);

        if let Some(statement) = parse_statement(sql, &mut issues, &options) {
            let aliases = self.extract_aliases_with_names_from_statement(&statement);
            return Ok(aliases);
        }

        Err(anyhow!("Could not extract table aliases from query"))
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

    /// Extract table name from parsed statement
    fn extract_from_statement(&self, statement: &Statement, alias: bool) -> Option<String> {
        match statement {
            Statement::Select(select) => self.extract_from_select(select, alias),
            Statement::InsertReplace(insert) => {
                // Handle SQLite-specific INSERT syntax
                Some(insert.table.identifier.to_string())
            }
            Statement::Update(update) => {
                // Handle SQLite-specific UPDATE syntax
                if let Some(first_table) = update.tables.first() {
                    self.extract_from_table_reference(first_table, alias)
                } else {
                    None
                }
            }
            Statement::Delete(delete) => {
                // Handle SQLite-specific DELETE syntax
                delete.tables.first().map(|first_table| first_table.identifier.to_string())
            }
            _ => None,
        }
    }

    /// Extract table name from a SELECT statement
    fn extract_from_select(&self, select: &sql_parse::Select, alias: bool) -> Option<String> {
        if let Some(table_references) = &select.table_references {
            if let Some(first_table) = table_references.first() {
                return self.extract_from_table_reference(first_table, alias);
            }
        }
        None
    }

    /// Extract table name from a table reference
    fn extract_from_table_reference(
        &self,
        table_ref: &TableReference,
        alias: bool,
    ) -> Option<String> {
        match table_ref {
            TableReference::Table {
                identifier, as_, ..
            } => {
                if alias {
                    if let Some(alias) = as_ {
                        return Some(alias.to_string());
                    }
                }

                Some(identifier.identifier.to_string())
            }
            TableReference::Query { query, as_, .. } => {
                if let Some(alias) = as_ {
                    Some(alias.to_string())
                } else {
                    self.extract_from_statement(query, true)
                }
            }
            TableReference::Join { left, right, .. } => self
                .extract_from_table_reference(left, alias)
                .or_else(|| self.extract_from_table_reference(right, alias)),
        }
    }

    /// Extract aliases with actual table names from a statement
    fn extract_aliases_with_names_from_statement(
        &self,
        statement: &Statement,
    ) -> Vec<(String, String)> {
        let mut aliases = Vec::new();

        if let Statement::Select(select) = statement {
            if let Some(table_references) = &select.table_references {
                for table_ref in table_references {
                    Self::extract_aliases_with_names_from_reference(table_ref, &mut aliases);
                }
            }
        }

        aliases
    }

    /// Extract aliases with actual table names from a table reference
    fn extract_aliases_with_names_from_reference(
        table_ref: &TableReference,
        aliases: &mut Vec<(String, String)>,
    ) {
        match table_ref {
            TableReference::Table {
                identifier, as_, ..
            } => {
                let table_name = identifier.identifier.to_string();

                if let Some(alias) = as_ {
                    aliases.push((table_name, alias.value.to_string()));
                }
            }
            TableReference::Query { as_, .. } => {
                if let Some(alias) = as_ {
                    aliases.push(("subquery".to_string(), alias.to_string()));
                }
            }
            TableReference::Join { left, right, .. } => {
                Self::extract_aliases_with_names_from_reference(left, aliases);
                Self::extract_aliases_with_names_from_reference(right, aliases);
            }
        }
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
            extractor
                .extract_table("SELECT * FROM users", false)
                .unwrap(),
            "users"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM users u", false)
                .unwrap(),
            "users"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM users u", true)
                .unwrap(),
            "u"
        );

        assert_eq!(
            extractor
                .extract_table("SELECT * FROM sqlite_master", false)
                .unwrap(),
            "sqlite_master"
        );
    }
}
