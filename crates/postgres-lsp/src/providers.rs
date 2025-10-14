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
    InsertTextFormat, Documentation, Hover, HoverContents, MarkupContent, MarkupKind,
    CodeAction, CodeActionKind, WorkspaceEdit, Command,
    CompletionParams, TextDocumentIdentifier, Position,
    HoverParams, TextDocumentPositionParams,
    CodeActionParams, Range, Uri,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use async_io::Timer;

use tracing::{debug, info, error};
use anyhow::Result;

/// PostgreSQL completion provider
pub struct PostgresCompletionProvider {
    client: Arc<Mutex<Option<PostgresLspClient>>>,
    config: PostgresLspConfig,
    document_uri: Uri,
}

impl PostgresCompletionProvider {
    /// Create a new PostgreSQL completion provider
    pub fn new(client: Arc<Mutex<Option<PostgresLspClient>>>, config: PostgresLspConfig, document_uri: Uri) -> Self {
        Self { client, config, document_uri }
    }

    /// Set the LSP client
    pub async fn set_client(&self, client: PostgresLspClient) {
        let mut client_guard = self.client.lock().await;
        *client_guard = Some(client);
    }



    /// Convert byte offset to LSP position
    fn offset_to_position(&self, text: &str, offset: usize) -> Result<Position, anyhow::Error> {
        let lines: Vec<&str> = text.lines().collect();
        let mut current_offset = 0;
        
        for (line_num, line) in lines.iter().enumerate() {
            if current_offset + line.len() >= offset {
                let character = offset - current_offset;
                return Ok(Position::new(line_num as u32, character as u32));
            }
            current_offset += line.len() + 1; // +1 for newline
        }
        
        // If offset is beyond the text, return the last position
        Ok(Position::new(lines.len().saturating_sub(1) as u32, 0))
    }
}

impl CompletionProvider for PostgresCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> gpui::Task<Result<CompletionResponse>> {
        let text_str = text.to_string();
        let client = self.client.clone();
        let document_uri = self.document_uri.clone();

        info!("🧩 Completion Provider called at offset {}", offset);
        debug!("🧩 Text context: {}", &text_str[..text_str.len().min(100)]);

        cx.spawn(async move |_handle, _cx| {
            info!("🧩 Starting PostgreSQL completion request at offset {}", offset);

            // Try to get completions from LSP server
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                info!("🧩 LSP client available, requesting completions");
                // Use the correct document URI for this tab
                // Convert offset to LSP position (simplified)
                let position = Position::new(0, offset as u32);

                let params = CompletionParams {
                    text_document_position: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri: document_uri },
                        position,
                    },
                    context: Some(CompletionContext {
                        trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
                        trigger_character: None,
                    }),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };

                info!("🧩 Sending completion request with position: {:?}", position);
                match client.completion(params).await {
                    Ok(lsp_response) => {
                        info!("🧩 ✅ Got completion response from LSP");
                        return Ok(lsp_response);
                    }
                    Err(e) => {
                        info!("🧩 ❌ LSP completion request failed: {}", e);
                    }
                }
            } else {
                info!("🧩 ⚠️ LSP client not available, using fallback completions");
            }

            // Fallback to example completions if LSP is not available
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
    client: Arc<Mutex<Option<PostgresLspClient>>>,
    config: PostgresLspConfig,
    document_uri: Uri,
    pending_request: Arc<Mutex<Option<JoinHandle<Result<Option<lsp_types::Hover>, anyhow::Error>>>>>,
    last_request_time: Arc<Mutex<Instant>>,
}

impl PostgresHoverProvider {
    /// Create a new PostgreSQL hover provider
    pub fn new(client: Arc<Mutex<Option<PostgresLspClient>>>, config: PostgresLspConfig, document_uri: Uri) -> Self {
        Self {
            client,
            config,
            document_uri,
            pending_request: Arc::new(Mutex::new(None)),
            last_request_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Set the LSP client
    pub async fn set_client(&self, client: PostgresLspClient) {
        let mut client_guard = self.client.lock().await;
        *client_guard = Some(client);
    }



    /// Convert byte offset to LSP position
    fn offset_to_position(&self, text: &str, offset: usize) -> Result<Position, anyhow::Error> {
        let lines: Vec<&str> = text.lines().collect();
        let mut current_offset = 0;
        
        for (line_num, line) in lines.iter().enumerate() {
            if current_offset + line.len() >= offset {
                let character = offset - current_offset;
                return Ok(Position::new(line_num as u32, character as u32));
            }
            current_offset += line.len() + 1; // +1 for newline
        }
        
        // If offset is beyond the text, return the last position
        Ok(Position::new(lines.len().saturating_sub(1) as u32, 0))
    }
}

impl HoverProvider for PostgresHoverProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> gpui::Task<Result<Option<lsp_types::Hover>, anyhow::Error>> {
        let text_str = text.to_string();
        let client = self.client.clone();
        let document_uri = self.document_uri.clone();
        let pending_request = self.pending_request.clone();
        let last_request_time = self.last_request_time.clone();

        info!("🖱️ Hover Provider called at offset {}", offset);
        debug!("🖱️ Text context: {}", &text_str[..text_str.len().min(100)]);

        cx.spawn(async move |_cx| {
            info!("🖱️ Hover request received at offset {}", offset);

            // Update last request time first
            let request_start_time = {
                let mut last_time = last_request_time.lock().await;
                *last_time = Instant::now();
                last_time.clone()
            };

            // Cancel any pending request
            {
                let mut pending = pending_request.lock().await;
                if let Some(handle) = pending.take() {
                    handle.abort();
                    info!("🖱️ Cancelled previous hover request");
                }
            }

            // Debounce delay - wait 200ms before sending request
            Timer::after(Duration::from_millis(200)).await;

            // Check if this request is still the latest one
            let should_proceed = {
                let last_time = last_request_time.lock().await;
                // Compare the time - if another request came in while we were sleeping,
                // last_time will be newer than request_start_time
                *last_time == request_start_time
            };

            if !should_proceed {
                info!("🖱️ Hover request cancelled by newer request");
                return Ok(None);
            }

            info!("🖱️ Starting PostgreSQL hover request at offset {}", offset);

            // Try to get hover from LSP server
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                info!("🖱️ LSP client available, requesting hover");

                // Document content is now read from disk, no need to sync virtual content
                info!("🖱️ Using file-based document for hover request");

                // Convert offset to LSP position (simplified)
                let position = Position::new(0, offset as u32);

                let params = HoverParams {
                    text_document_position_params: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier { uri: document_uri },
                        position,
                    },
                    work_done_progress_params: Default::default(),
                };

                info!("🖱️ Sending hover request with position: {:?}", position);
                match client.hover(params).await {
                    Ok(Some(hover)) => {
                        info!("🖱️ ✅ Got hover response from LSP");
                        return Ok(Some(hover));
                    }
                    Ok(None) => {
                        info!("🖱️ ⚠️ LSP returned no hover information");
                    }
                    Err(e) => {
                        info!("🖱️ ❌ LSP hover request failed: {}", e);
                    }
                }
            } else {
                info!("🖱️ ⚠️ LSP client not available, using fallback hover");
            }

            // Fallback to example hover information
            let word = get_word_at_position_static(&text_str, offset);
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

/// Get the word at the current position (static helper)
fn get_word_at_position_static(text: &str, offset: usize) -> Option<String> {
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

/// PostgreSQL code action provider
pub struct PostgresCodeActionProvider {
    client: Arc<Mutex<Option<PostgresLspClient>>>,
    config: PostgresLspConfig,
    document_uri: Uri,
}

impl PostgresCodeActionProvider {
    /// Create a new PostgreSQL code action provider
    pub fn new(client: Arc<Mutex<Option<PostgresLspClient>>>, config: PostgresLspConfig, document_uri: Uri) -> Self {
        Self { client, config, document_uri }
    }

    /// Set the LSP client
    pub async fn set_client(&self, client: PostgresLspClient) {
        let mut client_guard = self.client.lock().await;
        *client_guard = Some(client);
    }


}

impl CodeActionProvider for PostgresCodeActionProvider {
    fn id(&self) -> SharedString {
        "postgres-lsp-code-actions".into()
    }

    fn code_actions(
        &self,
        _state: Entity<InputState>,
        range: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> gpui::Task<Result<Vec<lsp_types::CodeAction>, anyhow::Error>> {
        let client = self.client.clone();
        let document_uri = self.document_uri.clone();

        info!("⚡ Code Action Provider called for range: {:?}", range);

        cx.spawn(async move |_cx| {
            info!("⚡ Starting PostgreSQL code actions request for range: {:?}", range);

            // Convert byte range to LSP range (simplified)
            let lsp_range = Range::new(
                Position::new(0, range.start as u32),
                Position::new(0, range.end as u32),
            );

            // Try to get code actions from LSP server
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                info!("⚡ LSP client available, requesting code actions");
                let params = CodeActionParams {
                    text_document: TextDocumentIdentifier { uri: document_uri },
                    range: lsp_range,
                    context: lsp_types::CodeActionContext::default(),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                };

                info!("⚡ Sending code actions request with range: {:?}", lsp_range);
                match client.code_actions(params).await {
                    Ok(Some(actions)) => {
                        info!("⚡ ✅ Got {} code actions from LSP", actions.len());
                        return Ok(actions);
                    }
                    Ok(None) => {
                        info!("⚡ ⚠️ LSP returned no code actions");
                    }
                    Err(e) => {
                        info!("⚡ ❌ LSP code actions request failed: {}", e);
                    }
                }
            } else {
                info!("⚡ ⚠️ LSP client not available, using fallback code actions");
            }

            // Fallback to example code actions
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
            info!("⚡ 🎯 Performing PostgreSQL code action: {} (preview: {})", action.title, preview);
            
            // For now, just log the action
            // In a real implementation, we would apply the edits
            info!("⚡ 🎯 Code action completed (placeholder implementation)");
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_word_at_position() {
        let text = "SELECT * FROM table_name WHERE id = 1";
        
        // Test getting "SELECT"
        let word = get_word_at_position_static(text, 3);
        assert_eq!(word, Some("SELECT".to_string()));

        // Test getting "FROM"
        let word = get_word_at_position_static(text, 10);
        assert_eq!(word, Some("FROM".to_string()));

        // Test getting "table_name"
        let word = get_word_at_position_static(text, 15);
        assert_eq!(word, Some("table_name".to_string()));

        // Test getting "WHERE"
        let word = get_word_at_position_static(text, 25);
        assert_eq!(word, Some("WHERE".to_string()));
    }
}