mod cache;
mod context;
mod fetch;

use blanco_core::QueryableEntity;
pub use cache::{CacheEntry, MetadataCache};
pub use context::{generate_table_abbreviation, resolve_table_alias};
pub use fetch::{fetch_columns, fetch_queryable_entities, fetch_schemas};

use cache::columns_key;

use crate::sql::statement_parser::{self, CompletionContext as TsCompletionContext, SqlClause};

use anyhow::Result;
use database::DatabaseServiceTrait;
use gpui::{AppContext as _, Context, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range, TextEdit,
};
use std::sync::Arc;
use std::time::Duration;

const CACHE_TTL_SECONDS: u64 = 300; // 5 minutes cache TTL

/// Default schema used when a tab has no explicit schema (postgres).
const DEFAULT_SCHEMA: &str = "public";

/// SQL keywords for completion and keyword detection
const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP", "TABLE",
    "INDEX", "VIEW", "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "ON", "GROUP", "BY", "ORDER",
    "HAVING", "LIMIT", "OFFSET", "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "IS",
    "NULL", "TRUE", "FALSE", "ASC", "DESC", "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX",
    "UNION", "ALL", "AS", "CASE", "WHEN", "THEN", "ELSE", "END",
];

/// What the completion request resolves to once the SQL context has been
/// analysed. Computed by [`plan_completion`], which is pure so the routing
/// rules can be tested without a database.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionPlan {
    /// Show columns of `schema.table`.
    Columns { schema: String, table: String },
    /// Show the tables of `schema` (drilling into `schema.` in a FROM/JOIN).
    SchemaTables { schema: String },
    /// Show the current schema's tables plus the other schema names.
    TablesAndSchemas,
    /// Show SQL keywords.
    Keywords,
    /// Nothing to complete.
    Nothing,
}

/// SQL Completion Provider that implements gpui-component's CompletionProvider trait
#[derive(Clone)]
pub struct SqlCompletionProvider {
    pub connection_id: i64,
    pub database_name: String,
    /// The schema the tab is connected to ("current schema"). Tables here are
    /// offered unqualified; other schemas appear as drill-in namespaces.
    pub current_schema: String,
    db_service: Arc<dyn DatabaseServiceTrait>,
    cache: Arc<std::sync::Mutex<MetadataCache>>,
}

impl SqlCompletionProvider {
    pub fn new(
        connection_id: i64,
        database_name: String,
        schema_name: Option<String>,
        db_service: Arc<dyn DatabaseServiceTrait>,
    ) -> Self {
        let current_schema = schema_name
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_SCHEMA.to_string());
        Self {
            connection_id,
            database_name,
            current_schema,
            db_service,
            cache: Arc::new(std::sync::Mutex::new(MetadataCache::new())),
        }
    }

    /// Return cached (schemas, supports_schemas) only if a fresh entry exists. No IO.
    pub fn try_get_cached_schemas(&self) -> Option<(Arc<Vec<String>>, bool)> {
        let cache = self.cache.lock().ok()?;
        let supports = cache.supports_schemas?;
        let cached = cache.schemas.as_ref()?;
        if cached.is_expired(CACHE_TTL_SECONDS) {
            return None;
        }
        Some((Arc::clone(&cached.data), supports))
    }

    /// Get cached schemas or fetch them. Returns the user schemas and whether
    /// the backend supports schemas at all.
    pub async fn get_cached_schemas(&self) -> Result<(Arc<Vec<String>>, bool)> {
        if let Some(cached) = self.try_get_cached_schemas() {
            return Ok(cached);
        }

        let (schemas, supports) =
            fetch_schemas(&*self.db_service, self.connection_id, &self.database_name).await?;

        let arc = if let Ok(mut cache) = self.cache.lock() {
            cache.supports_schemas = Some(supports);
            cache.schemas = Some(CacheEntry::new(schemas));
            Arc::clone(&cache.schemas.as_ref().expect("just inserted").data)
        } else {
            Arc::new(schemas)
        };

        Ok((arc, supports))
    }

    /// Return cached tables for a schema only if a fresh entry exists. No IO.
    pub fn try_get_cached_tables(&self, schema: &str) -> Option<Arc<Vec<QueryableEntity>>> {
        let cache = self.cache.lock().ok()?;
        let cached = cache.tables_by_schema.get(schema)?;
        if cached.is_expired(CACHE_TTL_SECONDS) {
            return None;
        }
        Some(Arc::clone(&cached.data))
    }

    /// Return cached columns for `schema.table` only if a fresh entry exists. No IO.
    pub fn try_get_cached_columns(&self, schema: &str, table: &str) -> Option<Arc<Vec<String>>> {
        let key = columns_key(schema, table);
        let cache = self.cache.lock().ok()?;
        let cached = cache.columns.get(&key)?;
        if cached.is_expired(CACHE_TTL_SECONDS) {
            return None;
        }
        Some(Arc::clone(&cached.data))
    }

    /// Get cached tables for a schema or fetch them if not cached/expired
    pub async fn get_cached_tables(&self, schema: &str) -> Result<Arc<Vec<QueryableEntity>>> {
        if let Some(cached) = self.try_get_cached_tables(schema) {
            return Ok(cached);
        }

        let entities = fetch_queryable_entities(
            &*self.db_service,
            self.connection_id,
            &self.database_name,
            Some(schema),
        )
        .await?;

        let arc = if let Ok(mut cache) = self.cache.lock() {
            cache
                .tables_by_schema
                .insert(schema.to_string(), CacheEntry::new(entities));
            Arc::clone(
                &cache
                    .tables_by_schema
                    .get(schema)
                    .expect("just inserted")
                    .data,
            )
        } else {
            Arc::new(entities)
        };

        Ok(arc)
    }

    /// Get cached columns for `schema.table` or fetch them if not cached/expired
    pub async fn get_cached_columns(&self, schema: &str, table: &str) -> Result<Arc<Vec<String>>> {
        if let Some(cached) = self.try_get_cached_columns(schema, table) {
            return Ok(cached);
        }

        let columns = fetch_columns(
            &*self.db_service,
            self.connection_id,
            table,
            &self.database_name,
            Some(schema),
        )
        .await?;

        let key = columns_key(schema, table);
        let arc = if let Ok(mut cache) = self.cache.lock() {
            cache.columns.insert(key.clone(), CacheEntry::new(columns));
            Arc::clone(&cache.columns.get(&key).expect("just inserted").data)
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
}

fn is_valid_identifier(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !word.chars().next().is_none_or(|c| c.is_ascii_digit())
}

fn is_sql_keyword(word: &str) -> bool {
    SQL_KEYWORDS.contains(&word.to_uppercase().as_str())
}

/// Clauses where a table/relation name is expected.
fn is_table_clause(clause: Option<SqlClause>) -> bool {
    matches!(
        clause,
        Some(SqlClause::From)
            | Some(SqlClause::Join)
            | Some(SqlClause::Insert)
            | Some(SqlClause::Update)
    )
}

/// Clauses where a column name is expected.
fn is_column_clause(clause: Option<SqlClause>) -> bool {
    matches!(
        clause,
        Some(SqlClause::Select)
            | Some(SqlClause::Where)
            | Some(SqlClause::Set)
            | Some(SqlClause::OrderBy)
            | Some(SqlClause::GroupBy)
            | Some(SqlClause::Having)
            | Some(SqlClause::On)
    )
}

/// Split a possibly schema-qualified name into `(schema, table)`. Bare names
/// fall back to `default_schema`. `"sales.orders"` -> `("sales", "orders")`.
fn split_schema_qualified(name: &str, default_schema: &str) -> (String, String) {
    match name.split_once('.') {
        Some((schema, table)) if !schema.is_empty() && !table.is_empty() => {
            (schema.to_string(), table.to_string())
        }
        _ => (default_schema.to_string(), name.to_string()),
    }
}

/// Decide what to complete from the parsed SQL context. Pure: all database
/// state (`known_schemas`, `supports_schemas`) is passed in, so the routing
/// rules are unit-testable without IO.
fn plan_completion(
    context: &TsCompletionContext,
    current_schema: &str,
    known_schemas: &[String],
    supports_schemas: bool,
) -> CompletionPlan {
    if context.is_dot_notation {
        let Some(name) = context.dot_table_name.as_deref() else {
            return CompletionPlan::Nothing;
        };

        // An alias always refers to a table -> show its columns.
        if let Some(table) = resolve_table_alias(&context.table_aliases, name) {
            let (schema, table) = split_schema_qualified(&table, current_schema);
            return CompletionPlan::Columns { schema, table };
        }

        // `schema.` in a table position drills into that schema's tables.
        if supports_schemas
            && is_table_clause(context.clause)
            && known_schemas.iter().any(|s| s == name)
        {
            return CompletionPlan::SchemaTables {
                schema: name.to_string(),
            };
        }

        // Otherwise treat it as a table in the current schema -> its columns.
        if is_valid_identifier(name) && !is_sql_keyword(name) {
            return CompletionPlan::Columns {
                schema: current_schema.to_string(),
                table: name.to_string(),
            };
        }

        return CompletionPlan::Nothing;
    }

    if is_column_clause(context.clause) {
        // Resolve the table from the FROM clause (first relation with an alias).
        if let Some(first) = context.table_aliases.first() {
            let (schema, table) = split_schema_qualified(&first.table_name, current_schema);
            return CompletionPlan::Columns { schema, table };
        }
        return CompletionPlan::Nothing;
    }

    if is_table_clause(context.clause) {
        return CompletionPlan::TablesAndSchemas;
    }

    CompletionPlan::Keywords
}

/// Filter items case-insensitively by the partial word, then sort shortest first.
fn filter_and_sort(items: &[String], current_word: &str) -> Vec<String> {
    let mut filtered: Vec<String> = if current_word.is_empty() {
        items.to_vec()
    } else {
        let needle = current_word.to_lowercase();
        items
            .iter()
            .filter(|item| item.to_lowercase().starts_with(&needle))
            .cloned()
            .collect()
    };
    filtered.sort_by_key(|a| a.len());
    filtered
}

fn build_column_items(
    columns: &[String],
    current_word: &str,
    range: Range,
    table: &str,
) -> Vec<CompletionItem> {
    filter_and_sort(columns, current_word)
        .into_iter()
        .map(|column_name| CompletionItem {
            label: column_name.clone(),
            kind: Some(CompletionItemKind::FIELD),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                range,
                column_name.clone(),
            ))),
            detail: Some(format!("Column from {table}")),
            insert_text: Some(column_name),
            ..Default::default()
        })
        .collect()
}

/// Build table completion items. Each inserts `name <abbrev>` so the user gets
/// a ready-made alias. Limited to 20 to keep the popup snappy.
fn build_table_items(
    entities: &[QueryableEntity],
    current_word: &str,
    range: Range,
) -> Vec<CompletionItem> {
    let mut filtered: Vec<&QueryableEntity> = if current_word.is_empty() {
        entities.iter().collect()
    } else {
        let needle = current_word.to_lowercase();
        entities
            .iter()
            .filter(|e| e.name.to_lowercase().starts_with(&needle))
            .collect()
    };
    filtered.sort_by_key(|e| e.name.len());

    filtered
        .into_iter()
        .take(20)
        .map(|entity| {
            let insert_text = generate_table_abbreviation(&entity.name);
            CompletionItem {
                label: entity.name.clone(),
                kind: Some(CompletionItemKind::CLASS),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    insert_text.clone(),
                ))),
                detail: Some(entity.entity_type.display_name().to_string()),
                insert_text: Some(insert_text),
                ..Default::default()
            }
        })
        .collect()
}

/// Build schema-namespace completion items. Inserting one yields the bare
/// schema name; the user then types `.` to drill into it.
fn build_schema_items(schemas: &[String], current_word: &str, range: Range) -> Vec<CompletionItem> {
    filter_and_sort(schemas, current_word)
        .into_iter()
        .map(|schema| CompletionItem {
            label: schema.clone(),
            kind: Some(CompletionItemKind::MODULE),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                range,
                schema.clone(),
            ))),
            detail: Some("Schema".to_string()),
            insert_text: Some(schema),
            ..Default::default()
        })
        .collect()
}

fn build_keyword_items(current_word: &str, range: Range) -> Vec<CompletionItem> {
    let filtered: Vec<&str> = if current_word.is_empty() {
        SQL_KEYWORDS.to_vec()
    } else {
        let needle = current_word.to_lowercase();
        SQL_KEYWORDS
            .iter()
            .filter(|keyword| keyword.to_lowercase().starts_with(&needle))
            .copied()
            .collect()
    };

    filtered
        .iter()
        .map(|keyword| {
            let label = keyword.to_string();
            CompletionItem {
                label: label.clone(),
                kind: Some(CompletionItemKind::KEYWORD),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    label.clone(),
                ))),
                detail: Some("SQL Keyword".to_string()),
                insert_text: Some(label),
                ..Default::default()
            }
        })
        .collect()
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
        let provider = self.clone();
        let background_executor = cx.background_executor().clone();

        // Spawn on the foreground thread so the debounce timer is cancelled when
        // a new completion request replaces this task (via _context_menu_task).
        cx.spawn_in(window, async move |_handle, cx| {
            cx.background_spawn(async move {
                let context = statement_parser::extract_completion_context(
                    &rope_clone,
                    offset.min(rope_clone.len()),
                );
                let Some(context) = context else {
                    return Ok(CompletionResponse::Array(Vec::new()));
                };

                // Debounce before any DB roundtrip, but only if something we need
                // is not already cached. We may sleep at most once.
                let mut debounced = false;

                // Schemas drive dot-notation disambiguation and the FROM listing.
                let (schemas, supports_schemas) = match provider.try_get_cached_schemas() {
                    Some(cached) => cached,
                    None => {
                        background_executor.timer(Duration::from_millis(300)).await;
                        debounced = true;
                        provider
                            .get_cached_schemas()
                            .await
                            .unwrap_or_else(|_| (Arc::new(Vec::new()), false))
                    }
                };

                let plan = plan_completion(
                    &context,
                    &provider.current_schema,
                    &schemas,
                    supports_schemas,
                );

                let start_pos = rope_clone
                    .offset_to_position(offset.saturating_sub(context.current_word.len()));
                let end_pos = rope_clone.offset_to_position(offset);
                let range = Range::new(start_pos, end_pos);

                // Debounce table/column fetches that are not yet cached.
                let needs_fetch = match &plan {
                    CompletionPlan::Columns { schema, table } => {
                        provider.try_get_cached_columns(schema, table).is_none()
                    }
                    CompletionPlan::SchemaTables { schema } => {
                        provider.try_get_cached_tables(schema).is_none()
                    }
                    CompletionPlan::TablesAndSchemas => provider
                        .try_get_cached_tables(&provider.current_schema)
                        .is_none(),
                    CompletionPlan::Keywords | CompletionPlan::Nothing => false,
                };
                if needs_fetch && !debounced {
                    background_executor.timer(Duration::from_millis(300)).await;
                }

                let items = match plan {
                    CompletionPlan::Columns { schema, table } => {
                        match provider.get_cached_columns(&schema, &table).await {
                            Ok(columns) => {
                                build_column_items(&columns, &context.current_word, range, &table)
                            }
                            Err(_) => Vec::new(),
                        }
                    }
                    CompletionPlan::SchemaTables { schema } => {
                        match provider.get_cached_tables(&schema).await {
                            Ok(entities) => {
                                build_table_items(&entities, &context.current_word, range)
                            }
                            Err(_) => Vec::new(),
                        }
                    }
                    CompletionPlan::TablesAndSchemas => {
                        match provider.get_cached_tables(&provider.current_schema).await {
                            Ok(entities) => {
                                let mut items =
                                    build_table_items(&entities, &context.current_word, range);
                                let other_schemas: Vec<String> = schemas
                                    .iter()
                                    .filter(|s| *s != &provider.current_schema)
                                    .cloned()
                                    .collect();
                                items.extend(build_schema_items(
                                    &other_schemas,
                                    &context.current_word,
                                    range,
                                ));
                                items
                            }
                            Err(_) => Vec::new(),
                        }
                    }
                    CompletionPlan::Keywords => build_keyword_items(&context.current_word, range),
                    CompletionPlan::Nothing => Vec::new(),
                };

                Ok(CompletionResponse::Array(items))
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
    use crate::sql::statement_parser::TableAlias;

    fn schemas(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn ctx(rope: &str, cursor: usize) -> TsCompletionContext {
        let rope = Rope::from_str(rope);
        statement_parser::extract_completion_context(&rope, cursor).unwrap()
    }

    fn ctx_end(rope: &str) -> TsCompletionContext {
        let r = Rope::from_str(rope);
        statement_parser::extract_completion_context(&r, r.len()).unwrap()
    }

    // ---- statement parser context (kept from before) ----

    #[test]
    fn test_completion_context_basic_select() {
        let c = ctx_end("SELECT * FROM users WHERE ");
        assert_eq!(c.clause, Some(SqlClause::Where));
        assert!(!c.is_dot_notation);
    }

    #[test]
    fn test_completion_context_from_clause() {
        assert_eq!(ctx_end("SELECT * FROM ").clause, Some(SqlClause::From));
    }

    #[test]
    fn test_completion_context_dot_notation() {
        let c = ctx("SELECT u. FROM users u", 9);
        assert!(c.is_dot_notation);
        assert_eq!(c.dot_table_name, Some("u".to_string()));
    }

    #[test]
    fn test_completion_context_aliases() {
        let c = ctx_end("SELECT * FROM users u JOIN orders o ON u.id = o.user_id WHERE ");
        assert_eq!(c.table_aliases.len(), 2);
        assert_eq!(c.table_aliases[0].table_name, "users");
        assert_eq!(c.table_aliases[0].alias, "u");
        assert_eq!(c.table_aliases[1].table_name, "orders");
        assert_eq!(c.table_aliases[1].alias, "o");
    }

    #[test]
    fn test_completion_context_keyword_in_string() {
        let c = ctx_end("SELECT * FROM users WHERE name = 'FROM orders' AND ");
        assert_eq!(c.clause, Some(SqlClause::Where));
        assert_eq!(c.table_aliases.len(), 0);
    }

    #[test]
    fn test_completion_context_semicolon_in_string() {
        let c = ctx_end("INSERT INTO orders (a) VALUES ('hello;'); SELECT * FROM ");
        assert_eq!(c.clause, Some(SqlClause::From));
    }

    #[test]
    fn test_completion_context_join() {
        assert_eq!(
            ctx_end("SELECT * FROM users u INNER JOIN ").clause,
            Some(SqlClause::Join)
        );
    }

    #[test]
    fn test_completion_context_update_set() {
        assert_eq!(ctx_end("UPDATE users SET ").clause, Some(SqlClause::Set));
    }

    #[test]
    fn test_completion_context_current_word() {
        assert_eq!(ctx_end("SELECT * FROM use").current_word, "use");
    }

    // ---- split_schema_qualified ----

    #[test]
    fn test_split_schema_qualified() {
        assert_eq!(
            split_schema_qualified("sales.orders", "public"),
            ("sales".to_string(), "orders".to_string())
        );
        assert_eq!(
            split_schema_qualified("orders", "public"),
            ("public".to_string(), "orders".to_string())
        );
        // Degenerate forms fall back to default schema + the literal name.
        assert_eq!(
            split_schema_qualified(".orders", "public"),
            ("public".to_string(), ".orders".to_string())
        );
        assert_eq!(
            split_schema_qualified("sales.", "public"),
            ("public".to_string(), "sales.".to_string())
        );
    }

    // ---- plan_completion: namespace drill-down behavior ----

    #[test]
    fn test_plan_from_lists_tables_and_schemas() {
        let c = ctx_end("SELECT * FROM ");
        let plan = plan_completion(&c, "public", &schemas(&["public", "sales"]), true);
        assert_eq!(plan, CompletionPlan::TablesAndSchemas);
    }

    #[test]
    fn test_plan_schema_dot_drills_into_schema() {
        // `FROM sales.` -> list sales tables, not columns of a table called "sales".
        let sql = "SELECT * FROM sales.";
        let plan = plan_completion(
            &ctx(sql, sql.len()),
            "public",
            &schemas(&["public", "sales", "analytics"]),
            true,
        );
        assert_eq!(
            plan,
            CompletionPlan::SchemaTables {
                schema: "sales".to_string()
            }
        );
    }

    #[test]
    fn test_plan_unknown_dot_name_treated_as_table_columns() {
        // `FROM foo.` where foo is neither alias nor schema -> columns of foo.
        let sql = "SELECT * FROM foo.";
        let plan = plan_completion(
            &ctx(sql, sql.len()),
            "public",
            &schemas(&["public", "sales"]),
            true,
        );
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                schema: "public".to_string(),
                table: "foo".to_string()
            }
        );
    }

    #[test]
    fn test_plan_alias_resolves_to_qualified_table_columns() {
        // `FROM sales.orders o ... o.` -> columns of sales.orders. Uses an
        // intact FROM (dot in WHERE) so the relation/alias parses cleanly.
        let sql = "SELECT * FROM sales.orders o WHERE o.";
        let plan = plan_completion(
            &ctx(sql, sql.len()),
            "public",
            &schemas(&["public", "sales"]),
            true,
        );
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                schema: "sales".to_string(),
                table: "orders".to_string()
            }
        );
    }

    #[test]
    fn test_plan_alias_wins_over_schema_name() {
        // A relation aliased "sales" must resolve as a table, not the schema.
        let aliases = vec![TableAlias {
            table_name: "shipments".to_string(),
            alias: "sales".to_string(),
        }];
        let context = TsCompletionContext {
            current_word: String::new(),
            clause: Some(SqlClause::Select),
            table_aliases: aliases,
            is_dot_notation: true,
            dot_table_name: Some("sales".to_string()),
        };
        let plan = plan_completion(&context, "public", &schemas(&["public", "sales"]), true);
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                schema: "public".to_string(),
                table: "shipments".to_string()
            }
        );
    }

    #[test]
    fn test_plan_schema_dot_ignored_when_schemas_unsupported() {
        // MySQL/SQLite: `db.` should not be treated as a schema drill-down.
        let sql = "SELECT * FROM sales.";
        let plan = plan_completion(&ctx(sql, sql.len()), "public", &schemas(&["sales"]), false);
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                schema: "public".to_string(),
                table: "sales".to_string()
            }
        );
    }

    #[test]
    fn test_plan_columns_in_where_use_first_alias() {
        let sql = "SELECT * FROM sales.orders o WHERE ";
        let plan = plan_completion(
            &ctx_end(sql),
            "public",
            &schemas(&["public", "sales"]),
            true,
        );
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                schema: "sales".to_string(),
                table: "orders".to_string()
            }
        );
    }

    #[test]
    fn test_plan_keywords_when_no_clause() {
        let context = TsCompletionContext {
            current_word: "SEL".to_string(),
            clause: None,
            table_aliases: Vec::new(),
            is_dot_notation: false,
            dot_table_name: None,
        };
        assert_eq!(
            plan_completion(&context, "public", &[], false),
            CompletionPlan::Keywords
        );
    }

    // ---- builders ----

    fn dummy_range() -> Range {
        Range::default()
    }

    #[test]
    fn test_build_table_items_inserts_alias_abbreviation() {
        let entities = vec![QueryableEntity {
            name: "orders".to_string(),
            entity_type: blanco_core::EntityType::Table,
        }];
        let items = build_table_items(&entities, "", dummy_range());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "orders");
        assert_eq!(items[0].insert_text.as_deref(), Some("orders o"));
        assert_eq!(items[0].kind, Some(CompletionItemKind::CLASS));
    }

    #[test]
    fn test_build_schema_items_are_namespaces() {
        let items = build_schema_items(&schemas(&["sales", "analytics"]), "", dummy_range());
        assert_eq!(items.len(), 2);
        assert!(
            items
                .iter()
                .all(|i| i.kind == Some(CompletionItemKind::MODULE))
        );
        assert_eq!(items[0].insert_text.as_deref(), Some("sales"));
        assert_eq!(items[0].detail.as_deref(), Some("Schema"));
    }

    #[test]
    fn test_build_schema_items_filter_by_word() {
        let items = build_schema_items(&schemas(&["sales", "analytics"]), "an", dummy_range());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "analytics");
    }

    #[test]
    fn test_build_column_items_filter_and_detail() {
        let cols = schemas(&["id", "company", "region"]);
        let items = build_column_items(&cols, "co", dummy_range(), "customers");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "company");
        assert_eq!(items[0].detail.as_deref(), Some("Column from customers"));
        assert_eq!(items[0].kind, Some(CompletionItemKind::FIELD));
    }
}
