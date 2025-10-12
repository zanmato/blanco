//! LSP provider implementations for PostgreSQL language server
//! 
//! This module provides GPUI-compatible LSP providers that integrate with the PostgreSQL language server.

use crate::client::PostgresLspClient;
use crate::config::PostgresLspConfig;
use gpui::{App, Context, Entity, Window, SharedString};
use gpui_component::input::{
    CompletionProvider, HoverProvider, CodeActionProvider, InputState, Rope,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionItemKind,
    InsertTextFormat, Documentation, MarkupKind, Hover, HoverContents, MarkupContent,
    CodeAction, CodeActionKind, WorkspaceEdit, Command,
};

use tracing::debug;
use anyhow::Result;

/// PostgreSQL completion provider
pub struct PostgresCompletionProvider {
    _client: (), // Placeholder - will be implemented with proper client sharing
    config: PostgresLspConfig,
}

impl PostgresCompletionProvider {
    /// Create a new PostgreSQL completion provider
    pub fn new(_client: (), config: PostgresLspConfig) -> Self {
        Self { _client, config }
    }
}

impl CompletionProvider for PostgresCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> gpui::Task<Result<CompletionResponse>> {
        let _text = text.to_string();
        
        cx.spawn(async move |_handle, cx| {
            debug!("Requesting PostgreSQL completions at offset {}", offset);

            // For now, return some example completions
            let example_completions = vec![
                CompletionItem {
                    label: "SELECT".to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("SELECT statement".to_string()),
                    documentation: Some(Documentation::String("Retrieve data from database".to_string())),
                    insert_text: Some("SELECT ${1:*} FROM ${2:table}".to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                },
                CompletionItem {
                    label: "FROM".to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("FROM clause".to_string()),
                    documentation: Some(Documentation::String("Specify table to query from".to_string())),
                    ..Default::default()
                },
                CompletionItem {
                    label: "WHERE".to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("WHERE clause".to_string()),
                    documentation: Some(Documentation::String("Filter query results".to_string())),
                    ..Default::default()
                },
                CompletionItem {
                    label: "INSERT".to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("INSERT statement".to_string()),
                    documentation: Some(Documentation::String("Insert data into table".to_string())),
                    insert_text: Some("INSERT INTO ${1:table} (${2:columns}) VALUES (${3:values})".to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                },
                CompletionItem {
                    label: "UPDATE".to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("UPDATE statement".to_string()),
                    documentation: Some(Documentation::String("Update existing data".to_string())),
                    insert_text: Some("UPDATE ${1:table} SET ${2:column} = ${3:value} WHERE ${4:condition}".to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                },
                CompletionItem {
                    label: "DELETE".to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("DELETE statement".to_string()),
                    documentation: Some(Documentation::String("Delete data from table".to_string())),
                    insert_text: Some("DELETE FROM ${1:table} WHERE ${2:condition}".to_string()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                },
                // PostgreSQL-specific functions
                CompletionItem {
                    label: "NOW()".to_string(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some("PostgreSQL function".to_string()),
                    documentation: Some(Documentation::String("Returns current timestamp".to_string())),
                    insert_text: Some("NOW()".to_string()),
                    ..Default::default()
                },
                CompletionItem {
                    label: "COUNT(*)".to_string(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some("Aggregate function".to_string()),
                    documentation: Some(Documentation::String("Count number of rows".to_string())),
                    insert_text: Some("COUNT(*)".to_string()),
                    ..Default::default()
                },
            ];

            Ok(CompletionResponse::Array(example_completions))
        })
    }

    fn is_completion_trigger(&self, offset: usize, text: &str, cx: &mut Context<InputState>) -> bool {
        // Check if the character at offset-1 is a trigger character
        if offset > 0 {
            if let Some(prev_char) = text.chars().nth(offset - 1) {
                return matches!(prev_char, '.' | ' ' | '(' | ',');
            }
        }
        false
    }
}

/// PostgreSQL hover provider
pub struct PostgresHoverProvider {
    _client: (), // Placeholder - will be implemented with proper client sharing
    config: PostgresLspConfig,
}

impl PostgresHoverProvider {
    /// Create a new PostgreSQL hover provider
    pub fn new(_client: (), config: PostgresLspConfig) -> Self {
        Self { _client, config }
    }
}

impl HoverProvider for PostgresHoverProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Task<Result<Option<lsp_types::Hover>, anyhow::Error>> {
        let text_str = text.to_string();
        
        // Get the word at the current position
        let word = self.get_word_at_position(&text_str, offset);
        
        cx.spawn(async move |cx| {
            debug!("Requesting PostgreSQL hover for word: {:?}", word);

            // For now, return some example hover information
            let hover_content = match word.as_deref() {
                Some("SELECT") => Some(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "**SELECT**\n\nRetrieve data from one or more tables.\n\n```sql\nSELECT column1, column2\nFROM table_name\nWHERE condition;\n```".to_string(),
                }),
                Some("INSERT") => Some(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "**INSERT**\n\nInsert new rows into a table.\n\n```sql\nINSERT INTO table_name (column1, column2)\nVALUES (value1, value2);\n```".to_string(),
                }),
                Some("UPDATE") => Some(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "**UPDATE**\n\nModify existing rows in a table.\n\n```sql\nUPDATE table_name\nSET column1 = value1\nWHERE condition;\n```".to_string(),
                }),
                Some("DELETE") => Some(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "**DELETE**\n\nRemove rows from a table.\n\n```sql\nDELETE FROM table_name\nWHERE condition;\n```".to_string(),
                }),
                Some("NOW") => Some(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "**NOW()**\n\nReturns the current timestamp.\n\n**Returns:** `timestamp`\n\n**Example:**\n```sql\nSELECT NOW();\n-- 2023-12-07 10:30:45.123456+00\n```".to_string(),
                }),
                Some("COUNT") => Some(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "**COUNT()**\n\nCount the number of rows.\n\n**Parameters:**\n- `*` or `expression`: Count all rows or non-null values\n\n**Returns:** `bigint`\n\n**Example:**\n```sql\nSELECT COUNT(*) FROM table_name;\nSELECT COUNT(column_name) FROM table_name;\n```".to_string(),
                }),
                _ => None,
            };

            if let Some(content) = hover_content {
                Ok(Some(Hover {
                    contents: HoverContents::Markup(content),
                    range: None,
                }))
            } else {
                Ok(None)
            }
        })
    }
}

impl PostgresHoverProvider {
    /// Get the word at the current position
    fn get_word_at_position(&self, text: &str, offset: usize) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        if offset >= chars.len() {
            return None;
        }

        // Find start of word
        let mut start = offset;
        while start > 0 {
            let c = chars[start - 1];
            if c.is_alphanumeric() || c == '_' {
                start -= 1;
            } else {
                break;
            }
        }

        // Find end of word
        let mut end = offset;
        while end < chars.len() {
            let c = chars[end];
            if c.is_alphanumeric() || c == '_' {
                end += 1;
            } else {
                break;
            }
        }

        if start < end {
            Some(chars[start..end].iter().collect())
        } else {
            None
        }
    }
}

/// PostgreSQL code action provider
pub struct PostgresCodeActionProvider {
    _client: (), // Placeholder - will be implemented with proper client sharing
    config: PostgresLspConfig,
}

impl PostgresCodeActionProvider {
    /// Create a new PostgreSQL code action provider
    pub fn new(_client: (), config: PostgresLspConfig) -> Self {
        Self { _client, config }
    }
}

impl CodeActionProvider for PostgresCodeActionProvider {
    fn id(&self) -> SharedString {
        "postgres-lsp-code-actions".into()
    }

    fn code_actions(
        &self,
        state: Entity<InputState>,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Task<Result<Vec<lsp_types::CodeAction>, anyhow::Error>> {
        cx.spawn(async move |cx| {
            debug!("Requesting PostgreSQL code actions for range: {:?}", range);

            // For now, return some example code actions
            let mut actions = Vec::new();

            // Add SQL formatting action
            actions.push(CodeAction {
                title: "Format SQL".to_string(),
                kind: Some(CodeActionKind::SOURCE),
                diagnostics: None,
                edit: Some(WorkspaceEdit {
                    document_changes: None,
                    changes: None,
                    change_annotations: None,
                }),
                is_preferred: Some(true),
                disabled: None,
                data: None,
                command: Some(Command {
                    title: "Format SQL".to_string(),
                    command: "sql.format".to_string(),
                    arguments: None,
                }),
            });

            // Add uppercase keywords action
            actions.push(CodeAction {
                title: "Convert Keywords to Uppercase".to_string(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: Some(WorkspaceEdit {
                    document_changes: None,
                    changes: None,
                    change_annotations: None,
                }),
                is_preferred: Some(false),
                disabled: None,
                data: None,
                command: Some(Command {
                    title: "Uppercase Keywords".to_string(),
                    command: "sql.uppercaseKeywords".to_string(),
                    arguments: None,
                }),
            });

            Ok(actions)
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<InputState>,
        action: CodeAction,
        preview: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Task<Result<(), anyhow::Error>> {
        cx.spawn(async move |cx| {
            debug!("Performing PostgreSQL code action: {}", action.title);
            
            // For now, just log the action
            // In a real implementation, we would apply the edits
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_word_at_position() {
        let provider = PostgresHoverProvider::new(
            unsafe { std::mem::zeroed() },
            PostgresLspConfig::default(),
        );

        let text = "SELECT * FROM table_name WHERE id = 1";
        
        // Test getting "SELECT"
        let word = provider.get_word_at_position(text, 3);
        assert_eq!(word, Some("SELECT".to_string()));

        // Test getting "FROM"
        let word = provider.get_word_at_position(text, 10);
        assert_eq!(word, Some("FROM".to_string()));

        // Test getting "table_name"
        let word = provider.get_word_at_position(text, 15);
        assert_eq!(word, Some("table_name".to_string()));

        // Test getting "WHERE"
        let word = provider.get_word_at_position(text, 25);
        assert_eq!(word, Some("WHERE".to_string()));
    }
}