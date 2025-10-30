use std::sync::Arc;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use gpui::{
    AppContext,
    Task, Window, Context,
};
use gpui_component::input::{
    CompletionProvider, Rope, RopeExt, InputState,
};
use blanco_core::{HoverProvider};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse,
    CompletionTextEdit, TextEdit, Hover, HoverContents, MarkupKind, MarkupContent,
    Range,
};
use crate::db_service::DbService;

/// Cache entry with timestamp
#[derive(Debug, Clone)]
struct CacheEntry<T> {
    data: T,
    timestamp: u64,
}

impl<T> CacheEntry<T> {
    fn new(data: T) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self { data, timestamp }
    }

    fn is_expired(&self, ttl_seconds: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.timestamp) > ttl_seconds
    }
}

/// Metadata cache for tables and columns
#[derive(Debug, Clone)]
struct MetadataCache {
    tables: Option<CacheEntry<Vec<String>>>,
    columns: HashMap<String, CacheEntry<Vec<String>>>,
    table_info: HashMap<String, CacheEntry<String>>,
    column_info: HashMap<String, CacheEntry<String>>,
}

impl MetadataCache {
    fn new() -> Self {
        Self {
            tables: None,
            columns: HashMap::new(),
            table_info: HashMap::new(),
            column_info: HashMap::new(),
        }
    }

    fn clear(&mut self) {
        self.tables = None;
        self.columns.clear();
        self.table_info.clear();
        self.column_info.clear();
    }

    fn is_empty(&self) -> bool {
        self.tables.is_none() && self.columns.is_empty()
    }
}

/// Fetch table names using the DbService
async fn fetch_tables(db_service: &DbService, connection_string: &str) -> Result<Vec<String>> {
    if let Some(connection) = db_service.get_or_create_unified_connection(connection_string).await.ok() {
        connection.get_tables(None).await
    } else {
        Ok(Vec::new())
    }
}

/// Fetch column names for a specific table using the DbService
async fn fetch_columns(db_service: &DbService, connection_string: &str, table_name: &str) -> Result<Vec<String>> {
    if let Some(connection) = db_service.get_or_create_unified_connection(connection_string).await.ok() {
        {
            let columns = connection.get_columns_for_table(table_name, None).await?;
            Ok(columns.into_iter().map(|col| col.name).collect())
        }
    } else {
        Ok(Vec::new())
    }
}

/// SQL Completion Provider that implements gpui-component's CompletionProvider trait
#[derive(Clone)]
pub struct SqlCompletionProvider {
    connection_string: String,
    db_service: DbService,
    cache: Arc<std::sync::Mutex<MetadataCache>>,
}

impl SqlCompletionProvider {
    pub fn new(connection_string: String, db_service: DbService) -> Self {
        Self {
            connection_string,
            db_service,
            cache: Arc::new(std::sync::Mutex::new(MetadataCache::new())),
        }
    }

    const CACHE_TTL_SECONDS: u64 = 300; // 5 minutes cache TTL

    fn invalidate_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Pre-populate the cache with all tables and columns
    /// This should be called when the provider is created or when database schema changes
    pub async fn warm_cache(&self) -> Result<()> {
        // Pre-fetch all tables
        let tables = self.get_cached_tables().await?;

        // Pre-fetch columns for each table
        for table in tables {
            let _ = self.get_cached_columns(&table).await;
        }

        Ok(())
    }

    /// Get cached tables or fetch them if not cached/expired
    async fn get_cached_tables(&self) -> Result<Vec<String>> {
        // First, check if we have valid cached data
        if let Ok(cache) = self.cache.lock() {
            if let Some(cached_tables) = &cache.tables {
                if !cached_tables.is_expired(Self::CACHE_TTL_SECONDS) {
                    return Ok(cached_tables.data.clone());
                }
            }
        } // Lock released here

        // No valid cache, fetch fresh data
        let tables = fetch_tables(&self.db_service, &self.connection_string).await?;

        // Update cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.tables = Some(CacheEntry::new(tables.clone()));
        }

        Ok(tables)
    }

    /// Get cached columns for a table or fetch them if not cached/expired
    async fn get_cached_columns(&self, table_name: &str) -> Result<Vec<String>> {
        // First, check if we have valid cached data
        if let Ok(cache) = self.cache.lock() {
            if let Some(cached_columns) = cache.columns.get(table_name) {
                if !cached_columns.is_expired(Self::CACHE_TTL_SECONDS) {
                    return Ok(cached_columns.data.clone());
                }
            }
        } // Lock released here

        // No valid cache, fetch fresh data
        let columns = fetch_columns(&self.db_service, &self.connection_string, table_name).await?;

        // Update cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.columns.insert(
                table_name.to_string(),
                CacheEntry::new(columns.clone())
            );
        }

        Ok(columns)
    }

    /// Get cached table info or fetch it if not cached/expired
    async fn get_cached_table_info(&self, table_name: &str) -> Result<String> {
        let cache_key = table_name.to_string();

        // First, check if we have valid cached data
        if let Ok(cache) = self.cache.lock() {
            if let Some(cached_info) = cache.table_info.get(&cache_key) {
                if !cached_info.is_expired(Self::CACHE_TTL_SECONDS) {
                    return Ok(cached_info.data.clone());
                }
            }
        } // Lock released here

        // No valid cache, fetch fresh data using connection trait
        let info = self.get_table_info(table_name).await?;

        // Update cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.table_info.insert(cache_key, CacheEntry::new(info.clone()));
        }

        Ok(info)
    }

    /// Get cached column info or fetch it if not cached/expired
    async fn get_cached_column_info(&self, table_name: &str, column_name: &str) -> Result<String> {
        let cache_key = format!("{}.{}", table_name, column_name);

        // First, check if we have valid cached data
        if let Ok(cache) = self.cache.lock() {
            if let Some(cached_info) = cache.column_info.get(&cache_key) {
                if !cached_info.is_expired(Self::CACHE_TTL_SECONDS) {
                    return Ok(cached_info.data.clone());
                }
            }
        } // Lock released here

        // No valid cache, fetch fresh data using connection trait
        let info = self.get_column_info(table_name, column_name).await?;

        // Update cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.column_info.insert(cache_key, CacheEntry::new(info.clone()));
        }

        Ok(info)
    }

    /// Get table information using the DbService
    async fn get_table_info(&self, table_name: &str) -> Result<String> {
        let columns = if let Some(connection) = self.db_service.get_or_create_unified_connection(&self.connection_string).await.ok() {
            connection.get_columns_for_table(table_name, None).await?
        } else {
            Vec::new()
        };
        let mut info = format!("**Table**: `{}`\n\n**Columns**:\n", table_name);

        for column in columns {
            info.push_str(&format!(
                "- **`{}`**: {}{}{}\n",
                column.name,
                column.data_type,
                if !column.is_nullable { " NOT NULL" } else { "" },
                if column.is_primary_key { " **PRIMARY KEY**" } else { "" }
            ));
        }

        Ok(info)
    }

    /// Get column information using the DbService
    async fn get_column_info(&self, table_name: &str, column_name: &str) -> Result<String> {
        let columns = if let Some(connection) = self.db_service.get_or_create_unified_connection(&self.connection_string).await.ok() {
            connection.get_columns_for_table(table_name, None).await?
        } else {
            Vec::new()
        };

        if let Some(column) = columns.iter().find(|c| c.name.eq_ignore_ascii_case(column_name)) {
            Ok(format!(
                "**Column**: `{}` in table `{}`\n\n- **Type**: {}\n- **Nullable**: {}\n- **Primary Key**: {}",
                column.name,
                table_name,
                column.data_type,
                if column.is_nullable { "Yes" } else { "No" },
                if column.is_primary_key { "Yes" } else { "No" }
            ))
        } else {
            Ok(format!("Column `{}` not found in table `{}`.", column_name, table_name))
        }
    }

    /// Determine if we should show table completions based on context
    fn should_show_tables(&self, text_before_cursor: &str) -> bool {
        let text_upper = text_before_cursor.to_uppercase();

        // Check if we're after FROM, JOIN, INTO, or UPDATE keywords
        text_upper.ends_with("FROM") ||
        text_upper.ends_with("JOIN") ||
        text_upper.ends_with("INNER JOIN") ||
        text_upper.ends_with("LEFT JOIN") ||
        text_upper.ends_with("RIGHT JOIN") ||
        text_upper.ends_with("OUTER JOIN") ||
        text_upper.ends_with("INTO") ||
        text_upper.ends_with("UPDATE") ||
        text_upper.contains("FROM ") ||
        text_upper.contains(" JOIN ") ||
        text_upper.contains(" INTO ") ||
        text_upper.contains("UPDATE ")
    }

    /// Extract table name from context for column completion
    async fn extract_table_for_columns(&self, text_before_cursor: &str) -> Option<String> {
        // Use the connection trait's table parsing method
        if let Some(_connection) = self.db_service.get_or_create_unified_connection(&self.connection_string).await.ok() {
            // Use a simple regex to extract table name from SQL query
            use regex::Regex;
            let re = Regex::new(r"(?i)(?:FROM|JOIN|UPDATE|INTO)\s+([a-zA-Z_][a-zA-Z0-9_]*)").ok()?;

            if let Some(captures) = re.captures(text_before_cursor) {
                captures.get(1).map(|m| m.as_str().to_string())
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Extract table name from a clause fragment
    fn extract_table_from_clause(&self, clause: &str) -> Option<String> {
        // Split on whitespace and look for the first word that could be a table name
        let trimmed = clause.trim();

        // Split on common SQL keywords to isolate table name
        let keywords = ["WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION", "AND", "OR"];

        for keyword in &keywords {
            if let Some(pos) = trimmed.to_uppercase().find(keyword) {
                let before_keyword = &trimmed[..pos].trim();
                if !before_keyword.is_empty() {
                    // Get the last word before this keyword
                    let words: Vec<&str> = before_keyword.split_whitespace().collect();
                    if let Some(last_word) = words.last() {
                        // Clean up the word (remove commas, semicolons, etc.)
                        let table_name = last_word.trim_end_matches(',').trim_end_matches(';');
                        if !table_name.is_empty() {
                            return Some(table_name.to_string());
                        }
                    }
                }
            }
        }

        // If no keywords found, just take the first word
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if let Some(first_word) = words.first() {
            let table_name = first_word.trim_end_matches(',').trim_end_matches(';');
            if !table_name.is_empty() {
                return Some(table_name.to_string());
            }
        }

        None
    }

    /// Determine if we should show column completions
    fn should_show_columns(&self, text_before_cursor: &str) -> bool {
        let text_upper = text_before_cursor.to_uppercase();

        // Column completion is appropriate in SELECT, WHERE, ORDER BY, GROUP BY contexts
        // Look for these patterns after we've already specified a table
        text_upper.contains("SELECT ") &&
        (text_upper.contains("FROM ") || text_upper.contains(" JOIN ")) ||
        text_upper.contains("WHERE ") ||
        text_upper.contains("ORDER BY ") ||
        text_upper.contains("GROUP BY ") ||
        text_upper.contains("HAVING ")
    }
}

impl CompletionProvider for SqlCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        // Get the current text before cursor to determine context
        let full_text = rope.to_string();
        let text_before_cursor = full_text[..offset].to_string();

        // Extract current word for filtering
        let current_word = extract_current_word(&text_before_cursor);

        // Check if we should show column or table completions
        let should_show_columns = self.should_show_columns(&text_before_cursor);
        let should_show_tables = self.should_show_tables(&text_before_cursor);

        // Calculate positions for text replacement
        let start_pos = rope.offset_to_position(offset.saturating_sub(current_word.len()));
        let end_pos = rope.offset_to_position(offset);

        // Priority: Column completion > Table completion > Keywords
        if should_show_columns {
            // Clone values for the background task
            let provider_clone = self.clone();
            let current_word_clone = current_word.clone();
            let start_pos_clone = start_pos;
            let end_pos_clone = end_pos;
            let text_before_cursor_clone = text_before_cursor.clone();

            // Spawn background task to extract table name and fetch columns
            let task = cx.background_spawn(async move {
                // Extract table name and fetch columns using cache
                if let Some(table_name) = provider_clone.extract_table_for_columns(&text_before_cursor_clone).await {
                    match provider_clone.get_cached_columns(&table_name).await {
                        Ok(columns) => {
                            // Filter columns based on current input
                            let filtered_columns: Vec<String> = if current_word_clone.is_empty() {
                                columns
                            } else {
                                columns.into_iter()
                                    .filter(|column| column.to_lowercase().starts_with(&current_word_clone.to_lowercase()))
                                    .collect()
                            };

                            // Convert to LSP completion items
                            let completion_items = filtered_columns.into_iter().map(|column_name| {
                                CompletionItem {
                                    label: column_name.clone(),
                                    kind: Some(CompletionItemKind::FIELD),
                                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                                        lsp_types::Range::new(start_pos_clone, end_pos_clone),
                                        column_name.clone(),
                                    ))),
                                    detail: Some(format!("Column from {}", table_name)),
                                    insert_text: Some(column_name),
                                    ..Default::default()
                                }
                            }).collect::<Vec<_>>();

                            Ok(CompletionResponse::Array(completion_items))
                        }
                        Err(_) => {
                            // If error fetching columns, return empty response
                            Ok(CompletionResponse::Array(Vec::new()))
                        }
                    }
                } else {
                    // Could not extract table name, return empty response
                    Ok(CompletionResponse::Array(Vec::new()))
                }
            });

            return task;
        }

        if should_show_tables {
            // Clone values for the background task
            let provider_clone = self.clone();
            let current_word_clone = current_word.clone();
            let start_pos_clone = start_pos;
            let end_pos_clone = end_pos;

            // Spawn background task to fetch tables using cache
            let task = cx.background_spawn(async move {
                // Fetch tables using cache
                match provider_clone.get_cached_tables().await {
                    Ok(tables) => {
                        // Filter tables based on current input
                        let filtered_tables: Vec<String> = if current_word_clone.is_empty() {
                            tables
                        } else {
                            tables.into_iter()
                                .filter(|table| table.to_lowercase().starts_with(&current_word_clone.to_lowercase()))
                                .collect()
                        };

                        // Convert to LSP completion items
                        let completion_items = filtered_tables.into_iter().map(|table_name| {
                            CompletionItem {
                                label: table_name.clone(),
                                kind: Some(CompletionItemKind::CLASS),
                                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                                    lsp_types::Range::new(start_pos_clone, end_pos_clone),
                                    table_name.clone(),
                                ))),
                                detail: Some("Table".to_string()),
                                insert_text: Some(table_name),
                                ..Default::default()
                            }
                        }).collect::<Vec<_>>();

                        Ok(CompletionResponse::Array(completion_items))
                    }
                    Err(_) => {
                        // If error fetching tables, return empty response
                        Ok(CompletionResponse::Array(Vec::new()))
                    }
                }
            });

            return task;
        }

        // Show SQL keywords when not in table context
        let sql_keywords = vec![
            "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE",
            "CREATE", "ALTER", "DROP", "TABLE", "INDEX", "VIEW",
            "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "ON",
            "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET",
            "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN",
            "LIKE", "IS", "NULL", "TRUE", "FALSE", "ASC", "DESC",
            "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX",
            "UNION", "ALL", "AS", "CASE", "WHEN", "THEN", "ELSE", "END",
        ];

        // Filter keywords based on current input
        let filtered_keywords: Vec<&str> = if current_word.is_empty() {
            sql_keywords
        } else {
            sql_keywords.iter()
                .filter(|keyword| keyword.to_lowercase().starts_with(&current_word.to_lowercase()))
                .copied()
                .collect()
        };

        // Convert keywords to LSP completion items
        let lsp_items: Vec<CompletionItem> = filtered_keywords.into_iter().map(|keyword| {
            let label = keyword.to_string();
            CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::KEYWORD),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    lsp_types::Range::new(start_pos, end_pos),
                    label.clone(),
                ))),
                detail: Some("SQL Keyword".to_string()),
                insert_text: Some(label),
                ..Default::default()
            }
        }).collect();

        Task::ready(Ok(CompletionResponse::Array(lsp_items)))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        // Trigger completion on space, dot, comma, and opening parenthesis
        // or when typing alphanumeric characters that could start a keyword
        matches!(new_text, " " | "." | "," | "(") || new_text.chars().all(|c| c.is_alphanumeric() || c == '_')
    }
}

impl HoverProvider for SqlCompletionProvider {
    fn hover(
        &self,
        rope: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut gpui::Context<InputState>,
    ) -> Task<Result<Option<Hover>>> {
        // Get the current text before cursor to determine context
        let full_text = rope.to_string();
        let text_before_cursor = full_text[..offset].to_string();

        // Extract current word to check for hover
        let current_word = extract_current_word(&text_before_cursor);

        if current_word.is_empty() {
            return Task::ready(Ok(None));
        }

        // Calculate position for hover range
        let start_pos = rope.offset_to_position(offset.saturating_sub(current_word.len()));
        let end_pos = rope.offset_to_position(offset);

        // Clone values for the background task
        let provider_clone = self.clone();
        let current_word_clone = current_word.clone();
        let start_pos_clone = start_pos;
        let end_pos_clone = end_pos;

        // Spawn background task to fetch hover information using cache
        let task = cx.background_spawn(async move {
            // Try to determine if this is a table or column and get appropriate info using cache
            if let Some(hover_info) = get_cached_hover_info(&provider_clone, &current_word_clone, &text_before_cursor).await {
                let range = Range::new(start_pos_clone, end_pos_clone);
                let hover = Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: hover_info,
                    }),
                    range: Some(range),
                };
                Ok(Some(hover))
            } else {
                Ok(None)
            }
        });

        task
    }
}

/// Get hover information for a table or column using cache
async fn get_cached_hover_info(provider: &SqlCompletionProvider, word: &str, text_before_cursor: &str) -> Option<String> {
    // First, try to determine if this is a column name by checking table context
    let provider_clone = provider.clone();
    let text_before_cursor_clone = text_before_cursor.to_string();
    if let Some(table_name) = provider_clone.extract_table_for_columns(&text_before_cursor_clone).await {
        if let Ok(column_info) = provider.get_cached_column_info(&table_name, word).await {
            return Some(column_info);
        }
    }

    // If no column context found, try table lookup
    if let Ok(table_info) = provider.get_cached_table_info(word).await {
        return Some(table_info);
    }

    // Try to find this column in any table using cached tables
    if let Ok(tables) = provider.get_cached_tables().await {
        for table in tables {
            if let Ok(column_info) = provider.get_cached_column_info(&table, word).await {
                return Some(column_info);
            }
        }
    }

    None
}

/// Extract the current word being typed based on text before cursor
fn extract_current_word(text_before_cursor: &str) -> String {
    let mut word_chars = Vec::new();

    for c in text_before_cursor.chars().rev() {
        if c.is_alphanumeric() || c == '_' {
            word_chars.push(c);
        } else {
            break;
        }
    }

    word_chars.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_current_word() {
        assert_eq!(extract_current_word("SELECT * FROM"), "FROM");
        assert_eq!(extract_current_word("SELECT * F"), "F");
        assert_eq!(extract_current_word("SELECT * "), "");
        assert_eq!(extract_current_word("user_name"), "user_name");
        assert_eq!(extract_current_word("123"), "123");
    }
}