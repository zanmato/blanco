mod cache;
mod context;
mod fetch;

pub use cache::{CacheEntry, MetadataCache};
pub use context::{ParsedSqlContext, SqlContextParser};
pub use fetch::{fetch_columns, fetch_tables};

use anyhow::Result;
use database::DatabaseServiceTrait;
use gpui::{AppContext, Context, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    TextEdit,
};
use std::sync::Arc;
use std::time::Duration;

const CACHE_TTL_SECONDS: u64 = 300; // 5 minutes cache TTL

/// SQL keywords for completion and keyword detection
const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP", "TABLE",
    "INDEX", "VIEW", "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "ON", "GROUP", "BY", "ORDER",
    "HAVING", "LIMIT", "OFFSET", "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "IS",
    "NULL", "TRUE", "FALSE", "ASC", "DESC", "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX",
    "UNION", "ALL", "AS", "CASE", "WHEN", "THEN", "ELSE", "END",
];

/// SQL Completion Provider that implements gpui-component's CompletionProvider trait
#[derive(Clone)]
pub struct SqlCompletionProvider {
    pub connection_id: i64,
    pub database_name: String,
    db_service: Arc<dyn DatabaseServiceTrait>,
    cache: Arc<std::sync::Mutex<MetadataCache>>,
}

impl SqlCompletionProvider {
    pub fn new(
        connection_id: i64,
        database_name: String,
        db_service: Arc<dyn DatabaseServiceTrait>,
    ) -> Self {
        Self {
            connection_id,
            database_name,
            db_service,
            cache: Arc::new(std::sync::Mutex::new(MetadataCache::new())),
        }
    }

    /// Get cached tables or fetch them if not cached/expired
    pub async fn get_cached_tables(&self) -> Result<Vec<String>> {
        // First, check if we have valid cached data
        if let Ok(cache) = self.cache.lock()
            && let Some(cached_tables) = &cache.tables
            && !cached_tables.is_expired(CACHE_TTL_SECONDS)
        {
            tracing::debug!("Using cached tables for database '{}'", self.database_name);
            return Ok(cached_tables.data.clone());
        } // Lock released here

        // No valid cache, fetch fresh data
        tracing::debug!(
            "Fetching fresh tables for database '{}'",
            self.database_name
        );
        let tables =
            fetch_tables(&*self.db_service, self.connection_id, &self.database_name).await?;

        // Update cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.tables = Some(CacheEntry::new(tables.clone()));
        }

        Ok(tables)
    }

    /// Get cached columns for a table or fetch them if not cached/expired
    pub async fn get_cached_columns(&self, table_name: &str) -> Result<Vec<String>> {
        // First, check if we have valid cached data
        if let Ok(cache) = self.cache.lock()
            && let Some(cached_columns) = cache.columns.get(table_name)
            && !cached_columns.is_expired(CACHE_TTL_SECONDS)
        {
            tracing::debug!(
                "Using cached columns for table '{}', database '{}'",
                table_name,
                self.database_name
            );
            return Ok(cached_columns.data.clone());
        } // Lock released here

        // No valid cache, fetch fresh data
        tracing::debug!(
            "Fetching fresh columns for table '{}', database '{}'",
            table_name,
            self.database_name
        );
        let columns = fetch_columns(
            &*self.db_service,
            self.connection_id,
            table_name,
            &self.database_name,
        )
        .await?;

        // Update cache
        if let Ok(mut cache) = self.cache.lock() {
            cache
                .columns
                .insert(table_name.to_string(), CacheEntry::new(columns.clone()));
        }

        Ok(columns)
    }

    /// Extract table name from context for column completion using full text for better alias resolution
    async fn extract_table_for_columns_with_full_text(
        &self,
        cursor_context: &ParsedSqlContext,
        full_context: &ParsedSqlContext,
        full_text: &str,
    ) -> Option<String> {
        // Use aliases from full text for better resolution
        let table_aliases = &full_context.table_aliases;

        tracing::debug!(
            "SQL Completion: Using full text for alias extraction: '{}'",
            full_text
        );
        tracing::debug!(
            "SQL Completion: Extracted aliases from full text: {:?}",
            table_aliases
        );

        // Handle dot notation: "table.column" or "alias.column"
        if cursor_context.is_dot_notation
            && let Some(table_name) = &cursor_context.dot_table_name
        {
            tracing::debug!(
                "SQL Completion: Dot notation detected, table_name='{}'",
                table_name
            );
            tracing::debug!(
                "SQL Completion: Parsed aliases from full text: {:?}",
                table_aliases
            );

            // First try to resolve as alias
            if let Some(resolved_table) =
                SqlContextParser::resolve_table_alias(table_aliases, table_name)
            {
                tracing::debug!(
                    "SQL Completion: Resolved alias '{}' to table '{}'",
                    table_name,
                    resolved_table
                );
                return Some(resolved_table);
            }

            tracing::debug!(
                "SQL Completion: Alias resolution failed, using table_name='{}' directly",
                table_name
            );
            // If alias resolution fails and table_name is likely an alias (single letter),
            // we could try common table names or return None to avoid invalid table queries
            if is_valid_identifier(table_name) && !is_sql_keyword(table_name) {
                // For now, return the table_name as-is, but in a real implementation,
                // we might want to maintain alias history or provide better fallbacks
                return Some(table_name.clone());
            }
        }

        // For non-dot notation, find table from context
        match cursor_context.last_keyword.as_deref() {
            Some("FROM") | Some("JOIN") | Some("INNER JOIN") | Some("LEFT JOIN")
            | Some("RIGHT JOIN") | Some("OUTER JOIN") | Some("UPDATE") | Some("INTO") => {
                // Look for table name after the keyword
                if let Some(table_name) = SqlContextParser::find_table_after_keyword(
                    &cursor_context.text,
                    cursor_context.last_keyword.as_ref().unwrap(),
                ) {
                    // Try to resolve through aliases
                    if let Some(resolved_table) =
                        SqlContextParser::resolve_table_alias(table_aliases, &table_name)
                    {
                        return Some(resolved_table);
                    }
                    return Some(table_name);
                }
            }
            Some("SELECT") | Some("WHERE") | Some("SET") | Some("ORDER BY") | Some("GROUP BY")
            | Some("HAVING") => {
                // For these contexts, find the last table mentioned in the query
                if let Some(table_name) =
                    SqlContextParser::find_last_table_mentioned(full_text, table_aliases)
                {
                    return Some(table_name);
                }
            }
            _ => {}
        }

        None
    }
}

// Helper functions moved to module level
fn is_valid_identifier(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !word.chars().next().is_none_or(|c| c.is_ascii_digit())
}

fn is_sql_keyword(word: &str) -> bool {
    SQL_KEYWORDS.contains(&word.to_uppercase().as_str())
}

impl CompletionProvider for SqlCompletionProvider {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        // Clone values needed for background task
        let rope_clone = rope.clone();
        let provider_clone = self.clone();

        // Get the background executor before entering the async block
        // Context is not Send, so we can't move it into the async block
        let executor = cx.background_executor().clone();

        // Spawn a single background task for all completion logic
        // This ensures all text processing and context parsing happens off the UI thread
        cx.background_spawn(async move {
            // Debounce - wait before doing any work to avoid excessive completion requests
            executor.timer(Duration::from_millis(100)).await;

            // All synchronous work now happens off the main thread
            // Find the last semicolon before cursor using the rope (avoids full string conversion)
            let slice_before_offset = rope_clone.slice(0..offset.min(rope_clone.len()));
            let text_before_offset = slice_before_offset.to_string();
            let text_before_cursor_start = if let Some(pos) = text_before_offset.rfind(';') {
                pos + 1
            } else {
                0
            };

            // Get the text before cursor as a string slice (only this portion, not the full text)
            let text_before_cursor = rope_clone
                .slice(text_before_cursor_start..offset.min(rope_clone.len()))
                .to_string();

            // Parse SQL context once for all completion decisions
            let context = SqlContextParser::parse(&text_before_cursor);

            // Determine what to show based on parsed context
            let should_show_columns = context.is_dot_notation || matches!(
                context.last_keyword.as_deref(),
                Some("SELECT")
                    | Some("WHERE")
                    | Some("SET")
                    | Some("ORDER BY")
                    | Some("GROUP BY")
                    | Some("HAVING")
            );
            let should_show_tables = matches!(
                context.last_keyword.as_deref(),
                Some("FROM")
                    | Some("JOIN")
                    | Some("INNER JOIN")
                    | Some("LEFT JOIN")
                    | Some("RIGHT JOIN")
                    | Some("OUTER JOIN")
                    | Some("INTO")
                    | Some("UPDATE")
            );

            // Calculate positions for text replacement
            let start_pos = rope_clone
                .offset_to_position(offset.saturating_sub(context.current_word.len()));
            let end_pos = rope_clone.offset_to_position(offset);

            // Priority: Column completion > Table completion > Keywords
            if should_show_columns {
                // Only convert full rope to string when we need it (for alias resolution)
                let full_text = rope_clone.to_string();
                let full_context = SqlContextParser::parse(&full_text);

                // Extract table name and fetch columns using cache with current query context
                // This ensures we only parse the current query, not previous ones
                if let Some(table_name) = provider_clone
                    .extract_table_for_columns_with_full_text(&context, &full_context, &full_text)
                    .await
                {
                    match provider_clone.get_cached_columns(&table_name).await {
                        Ok(columns) => {
                            // Filter columns based on current input
                            let filtered_columns: Vec<String> = if context.current_word.is_empty() {
                                columns
                            } else {
                                columns
                                    .into_iter()
                                    .filter(|column| {
                                        column
                                            .to_lowercase()
                                            .starts_with(&context.current_word.to_lowercase())
                                    })
                                    .collect()
                            };

                            // Sort by shortest first to prioritize shorter names
                            let mut filtered_columns = filtered_columns;
                            filtered_columns.sort_by_key(|a| a.len());

                            // Convert to LSP completion items
                            let completion_items = filtered_columns
                                .into_iter()
                                .map(|column_name| CompletionItem {
                                    label: column_name.clone(),
                                    kind: Some(CompletionItemKind::FIELD),
                                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                                        lsp_types::Range::new(start_pos, end_pos),
                                        column_name.clone(),
                                    ))),
                                    detail: Some(format!("Column from {}", table_name)),
                                    insert_text: Some(column_name),
                                    ..Default::default()
                                })
                                .collect::<Vec<_>>();

                            return Ok(CompletionResponse::Array(completion_items));
                        }
                        Err(_) => {
                            // If error fetching columns, return empty response
                            return Ok(CompletionResponse::Array(Vec::new()));
                        }
                    }
                } else {
                    // Could not extract table name, return empty response
                    return Ok(CompletionResponse::Array(Vec::new()));
                }
            }

            if should_show_tables {
                // Fetch tables using cache
                tracing::debug!("SQL Completion: Fetching tables...");
                match provider_clone.get_cached_tables().await {
                    Ok(tables) => {
                        tracing::debug!(
                            "SQL Completion: Fetched {} tables: {:?}",
                            tables.len(),
                            tables
                        );
                        // Filter tables based on current input
                        let mut filtered_tables: Vec<String> = if context.current_word.is_empty() {
                            tables.clone()
                        } else {
                            tables
                                .into_iter()
                                .filter(|table| {
                                    table
                                        .to_lowercase()
                                        .starts_with(&context.current_word.to_lowercase())
                                })
                                .collect()
                        };

                        // Sort by shortest first to prioritize shorter names
                        filtered_tables.sort_by_key(|a| a.len());

                        tracing::debug!(
                            "SQL Completion: Filter logic - context.current_word_is_empty: {}, filtered_tables: {:?}",
                            context.current_word.is_empty(),
                            filtered_tables
                        );

                        // Convert to LSP completion items
                        let completion_items = filtered_tables
                            .into_iter()
                            .take(20)
                            .map(|table_name| {
                                let insert_text_with_alias =
                                    SqlContextParser::generate_table_abbreviation(&table_name);
                                CompletionItem {
                                    label: table_name.clone(),
                                    kind: Some(CompletionItemKind::CLASS),
                                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                                        lsp_types::Range::new(start_pos, end_pos),
                                        insert_text_with_alias.clone(),
                                    ))),
                                    detail: Some("Table".to_string()),
                                    insert_text: Some(insert_text_with_alias),
                                    ..Default::default()
                                }
                            })
                            .collect::<Vec<_>>();

                        tracing::debug!(
                            "SQL Completion: Returning {} table completion items",
                            completion_items.len()
                        );
                        return Ok(CompletionResponse::Array(completion_items));
                    }
                    Err(_) => {
                        // If error fetching tables, return empty response
                        return Ok(CompletionResponse::Array(Vec::new()));
                    }
                }
            }

            // Show SQL keywords when not in table context
            // Filter keywords based on current input
            let filtered_keywords: Vec<&str> = if context.current_word.is_empty() {
                SQL_KEYWORDS.to_vec()
            } else {
                SQL_KEYWORDS
                    .iter()
                    .filter(|keyword| {
                        keyword
                            .to_lowercase()
                            .starts_with(&context.current_word.to_lowercase())
                    })
                    .copied()
                    .collect()
            };

            // Convert keywords to LSP completion items
            let lsp_items: Vec<CompletionItem> = filtered_keywords
                .iter()
                .map(|keyword| {
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
                })
                .collect();

            Ok(CompletionResponse::Array(lsp_items))
        })
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        // Trigger completion on space, dot, comma, and opening parenthesis
        // or when typing alphanumeric characters that could start a keyword
        matches!(new_text, " " | "." | "," | "(")
            || new_text.chars().all(|c| c.is_alphanumeric() || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock DatabaseService for tests
    #[derive(Clone)]
    struct MockDatabaseService;

    #[async_trait::async_trait]
    impl DatabaseServiceTrait for MockDatabaseService {
        async fn get_or_create_connection_by_id(
            &self,
            _connection_id: i64,
            _database: Option<&str>,
        ) -> std::result::Result<Arc<dyn blanco_core::Connection>, anyhow::Error> {
            unimplemented!("Mock database service not implemented for unit tests")
        }
    }

    fn create_test_db_service() -> Arc<dyn DatabaseServiceTrait> {
        Arc::new(MockDatabaseService)
    }

    // Test-only constructor that uses a mock service
    pub fn create_test_provider() -> SqlCompletionProvider {
        SqlCompletionProvider::new(1, "default".to_string(), create_test_db_service())
    }

    #[test]
    fn test_find_last_keyword() {
        // Test basic keyword detection
        assert_eq!(
            SqlContextParser::find_last_keyword("SELECT * FROM users"),
            Some("FROM".to_string())
        );
        assert_eq!(
            SqlContextParser::find_last_keyword("SELECT * FROM users WHERE"),
            Some("WHERE".to_string())
        );
        assert_eq!(
            SqlContextParser::find_last_keyword("UPDATE users SET name"),
            Some("SET".to_string())
        );
        assert_eq!(
            SqlContextParser::find_last_keyword("INSERT INTO users"),
            Some("INTO".to_string())
        );

        // Test case-insensitive
        assert_eq!(
            SqlContextParser::find_last_keyword("select * from users"),
            Some("FROM".to_string())
        );
        assert_eq!(
            SqlContextParser::find_last_keyword("Select * From Users"),
            Some("FROM".to_string())
        );

        // Test multi-word keywords
        // Note: The current implementation finds "JOIN" instead of "LEFT JOIN" in "LEFT JOIN users"
        // This is because "JOIN" appears later in the string than "LEFT JOIN"
        // This is actually acceptable behavior for our use case
        assert_eq!(
            SqlContextParser::find_last_keyword("LEFT JOIN users"),
            Some("JOIN".to_string())
        );
        assert_eq!(
            SqlContextParser::find_last_keyword("ORDER BY name"),
            Some("ORDER BY".to_string())
        );
        assert_eq!(
            SqlContextParser::find_last_keyword("GROUP BY category"),
            Some("GROUP BY".to_string())
        );
    }
}
