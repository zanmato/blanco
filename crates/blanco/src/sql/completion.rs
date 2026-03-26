mod cache;
mod context;
mod fetch;

use blanco_core::connection_trait::QueryableEntity;
pub use cache::{CacheEntry, MetadataCache};
pub use context::{generate_table_abbreviation, resolve_table_alias};
pub use fetch::{fetch_columns, fetch_queryable_entities};

use crate::sql::statement_parser::{self, CompletionContext as TsCompletionContext, SqlClause};

use anyhow::Result;
use database::DatabaseServiceTrait;
use gpui::{AppContext as _, Context, Task, Window};
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
    pub async fn get_cached_tables(&self) -> Result<Arc<Vec<QueryableEntity>>> {
        if let Ok(cache) = self.cache.lock()
            && let Some(cached_tables) = &cache.tables
            && !cached_tables.is_expired(CACHE_TTL_SECONDS)
        {
            tracing::debug!("Using cached tables for database '{}'", self.database_name);
            return Ok(Arc::clone(&cached_tables.data));
        }

        tracing::debug!(
            "Fetching fresh tables for database '{}'",
            self.database_name
        );
        let entities =
            fetch_queryable_entities(&*self.db_service, self.connection_id, &self.database_name)
                .await?;

        let arc = if let Ok(mut cache) = self.cache.lock() {
            cache.tables = Some(CacheEntry::new(entities));
            Arc::clone(&cache.tables.as_ref().expect("just inserted").data)
        } else {
            Arc::new(entities)
        };

        Ok(arc)
    }

    /// Get cached columns for a table or fetch them if not cached/expired
    pub async fn get_cached_columns(&self, table_name: &str) -> Result<Arc<Vec<String>>> {
        if let Ok(cache) = self.cache.lock()
            && let Some(cached_columns) = cache.columns.get(table_name)
            && !cached_columns.is_expired(CACHE_TTL_SECONDS)
        {
            tracing::debug!(
                "Using cached columns for table '{}', database '{}'",
                table_name,
                self.database_name
            );
            return Ok(Arc::clone(&cached_columns.data));
        }

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

        let arc = if let Ok(mut cache) = self.cache.lock() {
            cache
                .columns
                .insert(table_name.to_string(), CacheEntry::new(columns));
            Arc::clone(&cache.columns.get(table_name).expect("just inserted").data)
        } else {
            Arc::new(columns)
        };

        Ok(arc)
    }

    /// Invalidate all cached metadata (call after DDL statements)
    pub fn invalidate_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.invalidate_all();
        }
    }

    /// Extract table name for column completion from the tree-sitter context
    async fn extract_table_for_columns(&self, context: &TsCompletionContext) -> Option<String> {
        let table_aliases = &context.table_aliases;

        // Handle dot notation: "table.column" or "alias.column"
        if context.is_dot_notation {
            if let Some(table_name) = &context.dot_table_name {
                tracing::debug!(
                    "SQL Completion: Dot notation detected, table_name='{}'",
                    table_name
                );

                // First try to resolve as alias
                if let Some(resolved_table) = resolve_table_alias(table_aliases, table_name) {
                    tracing::debug!(
                        "SQL Completion: Resolved alias '{}' to table '{}'",
                        table_name,
                        resolved_table
                    );
                    return Some(resolved_table);
                }

                // Use directly if it's a valid identifier
                if is_valid_identifier(table_name) && !is_sql_keyword(table_name) {
                    return Some(table_name.clone());
                }
            }
        }

        // For non-dot notation, find a table from the aliases in the statement.
        // When cursor is in SELECT/WHERE/SET/ORDER BY/GROUP BY/HAVING,
        // use the first table in the FROM clause.
        if !table_aliases.is_empty() {
            return Some(table_aliases[0].table_name.clone());
        }

        // Try fetching from cached tables as a last resort: if there's only one
        // table in the database connection, use that.
        None
    }
}

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
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<Result<CompletionResponse>> {
        let rope_clone = rope.clone();
        let provider_clone = self.clone();

        let debounce_timer = cx.background_executor().timer(Duration::from_millis(300));

        // Spawn on the foreground thread so the debounce timer is cancelled when
        // a new completion request replaces this task (via _context_menu_task).
        cx.spawn_in(window, async move |_handle, cx| {
            debounce_timer.await;

            cx.background_spawn(async move {
                // Use tree-sitter to extract completion context (replaces rfind(';')
                // and the hand-rolled parser). This correctly handles semicolons
                // inside strings, keywords in string literals, comments, etc.
                let context = statement_parser::extract_completion_context(
                    &rope_clone,
                    offset.min(rope_clone.len()),
                );

                let Some(context) = context else {
                    return Ok(CompletionResponse::Array(Vec::new()));
                };

                // Determine what to show based on the AST-derived clause
                let should_show_columns = context.is_dot_notation
                    || matches!(
                        context.clause,
                        Some(SqlClause::Select)
                            | Some(SqlClause::Where)
                            | Some(SqlClause::Set)
                            | Some(SqlClause::OrderBy)
                            | Some(SqlClause::GroupBy)
                            | Some(SqlClause::Having)
                            | Some(SqlClause::On)
                    );
                let should_show_tables = matches!(
                    context.clause,
                    Some(SqlClause::From)
                        | Some(SqlClause::Join)
                        | Some(SqlClause::Insert)
                        | Some(SqlClause::Update)
                );

                // Calculate positions for text replacement
                let start_pos = rope_clone
                    .offset_to_position(offset.saturating_sub(context.current_word.len()));
                let end_pos = rope_clone.offset_to_position(offset);

                // Priority: Column completion > Table completion > Keywords
                if should_show_columns {
                    if let Some(table_name) =
                        provider_clone.extract_table_for_columns(&context).await
                    {
                        match provider_clone.get_cached_columns(&table_name).await {
                            Ok(columns) => {
                                let mut filtered_columns: Vec<String> =
                                    if context.current_word.is_empty() {
                                        columns.as_ref().clone()
                                    } else {
                                        columns
                                            .iter()
                                            .filter(|column| {
                                                column.to_lowercase().starts_with(
                                                    &context.current_word.to_lowercase(),
                                                )
                                            })
                                            .cloned()
                                            .collect()
                                    };

                                filtered_columns.sort_by_key(|a| a.len());

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
                                return Ok(CompletionResponse::Array(Vec::new()));
                            }
                        }
                    } else {
                        return Ok(CompletionResponse::Array(Vec::new()));
                    }
                }

                if should_show_tables {
                    match provider_clone.get_cached_tables().await {
                        Ok(entities) => {
                            let mut filtered_entities: Vec<QueryableEntity> =
                                if context.current_word.is_empty() {
                                    entities.as_ref().clone()
                                } else {
                                    entities
                                        .iter()
                                        .filter(|entity| {
                                            entity
                                                .name
                                                .to_lowercase()
                                                .starts_with(&context.current_word.to_lowercase())
                                        })
                                        .cloned()
                                        .collect()
                                };

                            filtered_entities.sort_by_key(|e| e.name.len());

                            tracing::debug!(
                                "SQL Completion: filtered_entities: {:?}",
                                filtered_entities
                                    .iter()
                                    .map(|e| &e.name)
                                    .collect::<Vec<_>>()
                            );

                            let completion_items = filtered_entities
                                .into_iter()
                                .take(20)
                                .map(|entity| {
                                    let insert_text_with_alias =
                                        generate_table_abbreviation(&entity.name);
                                    CompletionItem {
                                        label: entity.name.clone(),
                                        kind: Some(CompletionItemKind::CLASS),
                                        text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                                            lsp_types::Range::new(start_pos, end_pos),
                                            insert_text_with_alias.clone(),
                                        ))),
                                        detail: Some(entity.entity_type.display_name().to_string()),
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
                            return Ok(CompletionResponse::Array(Vec::new()));
                        }
                    }
                }

                // Show SQL keywords when not in table/column context
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
            .await
        })
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        matches!(new_text, " " | "." | "," | "(")
            || new_text.chars().all(|c| c.is_alphanumeric() || c == '_')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    pub fn create_test_provider() -> SqlCompletionProvider {
        SqlCompletionProvider::new(1, "default".to_string(), create_test_db_service())
    }

    #[test]
    fn test_completion_context_basic_select() {
        let rope = Rope::from_str("SELECT * FROM users WHERE ");
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        assert_eq!(ctx.clause, Some(SqlClause::Where));
        assert!(!ctx.is_dot_notation);
    }

    #[test]
    fn test_completion_context_from_clause() {
        let rope = Rope::from_str("SELECT * FROM ");
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        assert_eq!(ctx.clause, Some(SqlClause::From));
    }

    #[test]
    fn test_completion_context_dot_notation() {
        let rope = Rope::from_str("SELECT u. FROM users u");
        // cursor at position 9 (right after "u.")
        let ctx = statement_parser::extract_completion_context(&rope, 9).unwrap();
        assert!(ctx.is_dot_notation);
        assert_eq!(ctx.dot_table_name, Some("u".to_string()));
    }

    #[test]
    fn test_completion_context_aliases() {
        let rope = Rope::from_str("SELECT * FROM users u JOIN orders o ON u.id = o.user_id WHERE ");
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        assert_eq!(ctx.table_aliases.len(), 2);
        assert_eq!(ctx.table_aliases[0].table_name, "users");
        assert_eq!(ctx.table_aliases[0].alias, "u");
        assert_eq!(ctx.table_aliases[1].table_name, "orders");
        assert_eq!(ctx.table_aliases[1].alias, "o");
    }

    #[test]
    fn test_completion_context_keyword_in_string() {
        // The hand-rolled parser would be fooled by FROM inside a string literal
        let rope = Rope::from_str("SELECT * FROM users WHERE name = 'FROM orders' AND ");
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        // Should detect WHERE clause, not FROM (which is inside a string)
        assert_eq!(ctx.clause, Some(SqlClause::Where));
        assert_eq!(ctx.table_aliases.len(), 0); // "users" has no alias
    }

    #[test]
    fn test_completion_context_semicolon_in_string() {
        // The rfind(';') approach would break on semicolons in strings
        let rope = Rope::from_str("INSERT INTO orders (a) VALUES ('hello;'); SELECT * FROM ");
        // cursor at end, after "FROM "
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        assert_eq!(ctx.clause, Some(SqlClause::From));
        // The statement text should be the SELECT, not the INSERT
        assert!(ctx.statement_text.contains("SELECT"));
    }

    #[test]
    fn test_completion_context_join() {
        let rope = Rope::from_str("SELECT * FROM users u INNER JOIN ");
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        assert_eq!(ctx.clause, Some(SqlClause::Join));
    }

    #[test]
    fn test_completion_context_update_set() {
        let rope = Rope::from_str("UPDATE users SET ");
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        assert_eq!(ctx.clause, Some(SqlClause::Set));
    }

    #[test]
    fn test_completion_context_current_word() {
        let rope = Rope::from_str("SELECT * FROM use");
        let ctx = statement_parser::extract_completion_context(&rope, rope.len()).unwrap();
        assert_eq!(ctx.current_word, "use");
    }
}
