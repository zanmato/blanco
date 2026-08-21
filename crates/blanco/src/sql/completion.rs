mod cache;
mod context;
mod fetch;

use blanco_core::{ColumnInfo, DatabaseType, QueryableEntity};
pub use cache::{CacheEntry, MetadataCache};
pub use context::{generate_table_abbreviation, resolve_table_alias};
pub use fetch::{fetch_columns, fetch_queryable_entities, fetch_schemas};

use cache::columns_key;

use crate::sql::statement_parser::{
    self, CompletionContext as TsCompletionContext, SqlClause, TableAlias, ident_eq,
};

use anyhow::Result;
use database::DatabaseServiceTrait;
use gpui::{App, AppContext as _, Task, Window};
use gpui_component::input::{CompletionProvider, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Range, TextEdit,
};
use std::sync::Arc;
use std::time::Duration;

const CACHE_TTL_SECONDS: u64 = 300; // 5 minutes cache TTL

/// Default schema used when a tab has no explicit schema, keyed by backend:
/// PostgreSQL uses `public`, SQL Server uses `dbo`, and the schema-less backends
/// (MySQL, SQLite, ClickHouse) fall back to the database name since their
/// "schema" is effectively the database.
fn default_schema_for(driver: DatabaseType, database_name: &str) -> String {
    match driver {
        DatabaseType::PostgreSQL => "public".to_string(),
        DatabaseType::MsSql => "dbo".to_string(),
        DatabaseType::MySQL
        | DatabaseType::SQLite
        | DatabaseType::ClickHouse
        | DatabaseType::Redis => database_name.to_string(),
    }
}

/// SQL keywords for completion and keyword detection
const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP", "TABLE",
    "INDEX", "VIEW", "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "ON", "GROUP", "BY", "ORDER",
    "HAVING", "LIMIT", "OFFSET", "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "IS",
    "NULL", "TRUE", "FALSE", "ASC", "DESC", "DISTINCT", "COUNT", "SUM", "AVG", "MIN", "MAX",
    "UNION", "ALL", "AS", "CASE", "WHEN", "THEN", "ELSE", "END",
];

/// A resolved table reference (schema-qualified) whose columns should be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableRef {
    schema: String,
    table: String,
}

/// A column offered for completion, with its data type when known (real table
/// columns have a type; CTE/derived-table projection columns do not).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ColumnCandidate {
    name: String,
    data_type: Option<String>,
}

/// Where a set of completable columns comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ColumnSource {
    /// Columns fetched (and cached) from a real database table.
    Table(TableRef),
    /// Columns known statically from a CTE or derived-table (subquery)
    /// projection, so no database lookup is needed.
    Static(Vec<String>),
}

/// What the completion request resolves to once the SQL context has been
/// analysed. Computed by [`plan_completion`], which is pure so the routing
/// rules can be tested without a database.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionPlan {
    /// Show the union of columns from one or more sources. A single entry is the
    /// dot-notation / single-relation case; multiple entries cover columns from
    /// all relations in scope (e.g. every table in a JOIN).
    Columns { sources: Vec<ColumnSource> },
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
        driver: DatabaseType,
        db_service: Arc<dyn DatabaseServiceTrait>,
    ) -> Self {
        let current_schema = schema_name
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_schema_for(driver, &database_name));
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
    pub fn try_get_cached_columns(
        &self,
        schema: &str,
        table: &str,
    ) -> Option<Arc<Vec<ColumnInfo>>> {
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
    pub async fn get_cached_columns(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<Arc<Vec<ColumnInfo>>> {
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

/// Look up the static columns of a CTE / derived table referenced by `name`,
/// either directly (`FROM cte`) or via an alias that maps to such a relation.
fn lookup_derived_columns(context: &TsCompletionContext, name: &str) -> Option<Vec<String>> {
    if let Some((_, columns)) = context
        .cte_columns
        .iter()
        .find(|(key, _)| ident_eq(key, name))
    {
        return Some(columns.clone());
    }
    // `name` may be a table alias that resolves to a CTE name.
    let resolved = resolve_table_alias(&context.table_aliases, name)?;
    context
        .cte_columns
        .iter()
        .find(|(key, _)| ident_eq(key, &resolved))
        .map(|(_, columns)| columns.clone())
}

/// Push a source unless an equal one is already present (de-dupes repeated
/// relations without disturbing order).
fn push_unique(sources: &mut Vec<ColumnSource>, source: ColumnSource) {
    if !sources.contains(&source) {
        sources.push(source);
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

        // A CTE or derived-table referenced by this name -> its static columns.
        if let Some(columns) = lookup_derived_columns(context, name) {
            return CompletionPlan::Columns {
                sources: vec![ColumnSource::Static(columns)],
            };
        }

        // An alias always refers to a table -> show its columns.
        if let Some(table) = resolve_table_alias(&context.table_aliases, name) {
            let (schema, table) = split_schema_qualified(&table, current_schema);
            return CompletionPlan::Columns {
                sources: vec![ColumnSource::Table(TableRef { schema, table })],
            };
        }

        // `schema.` in a table position drills into that schema's tables.
        if supports_schemas
            && is_table_clause(context.clause)
            && let Some(schema) = known_schemas.iter().find(|s| ident_eq(s, name))
        {
            return CompletionPlan::SchemaTables {
                schema: schema.to_string(),
            };
        }

        // Otherwise treat it as a table in the current schema -> its columns.
        if is_valid_identifier(name) && !is_sql_keyword(name) {
            return CompletionPlan::Columns {
                sources: vec![ColumnSource::Table(TableRef {
                    schema: current_schema.to_string(),
                    table: name.to_string(),
                })],
            };
        }

        return CompletionPlan::Nothing;
    }

    if is_column_clause(context.clause) {
        // Offer columns from every relation in scope (all FROM/JOIN relations),
        // not just the first. Real tables are fetched from the database; CTEs and
        // derived tables contribute their statically-known projection columns.
        let mut sources: Vec<ColumnSource> = Vec::new();
        let mut covered_derived: Vec<&str> = Vec::new();

        for alias in &context.table_aliases {
            // A relation aliased to a CTE/derived name uses its static columns
            // (e.g. `FROM cte c` or `FROM (subquery) c`).
            if let Some(columns) = lookup_derived_columns(context, &alias.alias) {
                covered_derived.push(&alias.alias);
                push_unique(&mut sources, ColumnSource::Static(columns));
                continue;
            }
            let (schema, table) = split_schema_qualified(&alias.table_name, current_schema);
            push_unique(
                &mut sources,
                ColumnSource::Table(TableRef { schema, table }),
            );
        }

        // Derived relations referenced without a separate alias (e.g. `FROM cte`
        // or `FROM (subquery) sub`) are not in `table_aliases`, so add them here.
        for (name, columns) in &context.cte_columns {
            if !covered_derived
                .iter()
                .any(|covered| ident_eq(covered, name.as_str()))
            {
                push_unique(&mut sources, ColumnSource::Static(columns.clone()));
            }
        }

        // Even with no resolvable tables (e.g. `SELECT ` before a FROM), this is
        // still an expression context, so return `Columns` so functions can be
        // offered. The source list may be empty.
        return CompletionPlan::Columns { sources };
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
    columns: &[ColumnCandidate],
    current_word: &str,
    range: Range,
    table: &str,
) -> Vec<CompletionItem> {
    let needle = current_word.to_lowercase();
    let mut filtered: Vec<&ColumnCandidate> = if current_word.is_empty() {
        columns.iter().collect()
    } else {
        columns
            .iter()
            .filter(|column| column.name.to_lowercase().starts_with(&needle))
            .collect()
    };
    filtered.sort_by_key(|column| column.name.len());

    filtered
        .into_iter()
        .map(|column| {
            let column_name = column.name.clone();
            // Prefer the data type as the detail hint; fall back to the source
            // table when the type is unknown (CTE / derived-table columns).
            let detail = column
                .data_type
                .clone()
                .unwrap_or_else(|| format!("Column from {table}"));
            CompletionItem {
                label: column_name.clone(),
                kind: Some(CompletionItemKind::FIELD),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    column_name.clone(),
                ))),
                detail: Some(detail),
                insert_text: Some(column_name),
                ..Default::default()
            }
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

/// A built-in SQL function offered in expression contexts, with a human-readable
/// signature shown as the completion detail.
struct SqlFunction {
    name: &'static str,
    signature: &'static str,
}

/// Common cross-dialect SQL functions. Not exhaustive or dialect-specific, but
/// covers the functions users reach for most often so completion can offer them
/// with a signature hint alongside columns.
const SQL_FUNCTIONS: &[SqlFunction] = &[
    // Aggregates
    SqlFunction {
        name: "COUNT",
        signature: "COUNT(expr)",
    },
    SqlFunction {
        name: "SUM",
        signature: "SUM(expr)",
    },
    SqlFunction {
        name: "AVG",
        signature: "AVG(expr)",
    },
    SqlFunction {
        name: "MIN",
        signature: "MIN(expr)",
    },
    SqlFunction {
        name: "MAX",
        signature: "MAX(expr)",
    },
    SqlFunction {
        name: "ARRAY_AGG",
        signature: "ARRAY_AGG(expr)",
    },
    SqlFunction {
        name: "STRING_AGG",
        signature: "STRING_AGG(expr, delimiter)",
    },
    // Conditional / null handling
    SqlFunction {
        name: "COALESCE",
        signature: "COALESCE(value [, ...])",
    },
    SqlFunction {
        name: "NULLIF",
        signature: "NULLIF(value1, value2)",
    },
    SqlFunction {
        name: "GREATEST",
        signature: "GREATEST(value [, ...])",
    },
    SqlFunction {
        name: "LEAST",
        signature: "LEAST(value [, ...])",
    },
    SqlFunction {
        name: "CAST",
        signature: "CAST(expr AS type)",
    },
    // String
    SqlFunction {
        name: "UPPER",
        signature: "UPPER(string)",
    },
    SqlFunction {
        name: "LOWER",
        signature: "LOWER(string)",
    },
    SqlFunction {
        name: "LENGTH",
        signature: "LENGTH(string)",
    },
    SqlFunction {
        name: "TRIM",
        signature: "TRIM(string)",
    },
    SqlFunction {
        name: "SUBSTRING",
        signature: "SUBSTRING(string FROM start FOR count)",
    },
    SqlFunction {
        name: "REPLACE",
        signature: "REPLACE(string, from, to)",
    },
    SqlFunction {
        name: "CONCAT",
        signature: "CONCAT(value [, ...])",
    },
    // Math
    SqlFunction {
        name: "ROUND",
        signature: "ROUND(numeric [, decimals])",
    },
    SqlFunction {
        name: "ABS",
        signature: "ABS(numeric)",
    },
    SqlFunction {
        name: "CEIL",
        signature: "CEIL(numeric)",
    },
    SqlFunction {
        name: "FLOOR",
        signature: "FLOOR(numeric)",
    },
    // Date / time
    SqlFunction {
        name: "NOW",
        signature: "NOW()",
    },
    SqlFunction {
        name: "CURRENT_DATE",
        signature: "CURRENT_DATE",
    },
    SqlFunction {
        name: "CURRENT_TIMESTAMP",
        signature: "CURRENT_TIMESTAMP",
    },
    SqlFunction {
        name: "DATE_TRUNC",
        signature: "DATE_TRUNC(field, source)",
    },
    SqlFunction {
        name: "EXTRACT",
        signature: "EXTRACT(field FROM source)",
    },
    // Window
    SqlFunction {
        name: "ROW_NUMBER",
        signature: "ROW_NUMBER() OVER (...)",
    },
    SqlFunction {
        name: "RANK",
        signature: "RANK() OVER (...)",
    },
    SqlFunction {
        name: "DENSE_RANK",
        signature: "DENSE_RANK() OVER (...)",
    },
];

/// Build completion items for built-in SQL functions whose name starts with
/// `current_word`. Inserts `NAME()` so the parentheses are balanced.
fn build_function_items(current_word: &str, range: Range) -> Vec<CompletionItem> {
    let needle = current_word.to_lowercase();
    let mut filtered: Vec<&SqlFunction> = SQL_FUNCTIONS
        .iter()
        .filter(|function| function.name.to_lowercase().starts_with(&needle))
        .collect();
    filtered.sort_by_key(|function| function.name.len());

    filtered
        .into_iter()
        .map(|function| {
            let insert_text = format!("{}()", function.name);
            CompletionItem {
                label: function.name.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    insert_text.clone(),
                ))),
                detail: Some(function.signature.to_string()),
                insert_text: Some(insert_text),
                ..Default::default()
            }
        })
        .collect()
}

/// A real table participating in a join, with its alias and column metadata,
/// used to derive foreign-key join conditions.
struct JoinTable<'a> {
    alias: &'a str,
    table_name: &'a str,
    columns: &'a [ColumnInfo],
}

/// Build `alias.col = other_alias.ref_col` strings for every foreign key on an
/// in-scope table that references another in-scope table.
fn build_join_conditions(tables: &[JoinTable]) -> Vec<String> {
    let mut conditions = Vec::new();
    for table in tables {
        for column in table.columns {
            let Some(foreign_key) = &column.foreign_key else {
                continue;
            };
            for other in tables {
                if other.table_name == foreign_key.foreign_table_name {
                    conditions.push(format!(
                        "{}.{} = {}.{}",
                        table.alias, column.name, other.alias, foreign_key.foreign_column_name
                    ));
                }
            }
        }
    }
    conditions
}

/// Build foreign-key join-condition completion items for the in-scope relations,
/// filtered by `current_word`. Each item inserts a full `a.col = b.col` clause.
fn build_join_condition_items(
    table_aliases: &[TableAlias],
    fetched_tables: &[(TableRef, Arc<Vec<ColumnInfo>>)],
    current_schema: &str,
    current_word: &str,
    range: Range,
) -> Vec<CompletionItem> {
    // Pair each alias with its fetched column metadata.
    let owned: Vec<(String, String, Arc<Vec<ColumnInfo>>)> = table_aliases
        .iter()
        .filter_map(|alias| {
            let (schema, table) = split_schema_qualified(&alias.table_name, current_schema);
            let columns = fetched_tables
                .iter()
                .find(|(table_ref, _)| table_ref.schema == schema && table_ref.table == table)
                .map(|(_, columns)| Arc::clone(columns))?;
            Some((alias.alias.clone(), table, columns))
        })
        .collect();
    let tables: Vec<JoinTable> = owned
        .iter()
        .map(|(alias, table, columns)| JoinTable {
            alias,
            table_name: table,
            columns,
        })
        .collect();

    let needle = current_word.to_lowercase();
    build_join_conditions(&tables)
        .into_iter()
        .filter(|condition| condition.to_lowercase().starts_with(&needle))
        .map(|condition| CompletionItem {
            label: condition.clone(),
            kind: Some(CompletionItemKind::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                range,
                condition.clone(),
            ))),
            detail: Some("Join condition".to_string()),
            insert_text: Some(condition),
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
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let rope_clone = rope.clone();
        let provider = self.clone();
        let background_executor = cx.background_executor().clone();

        // Spawn on the foreground thread so the debounce timer is cancelled when
        // a new completion request replaces this task (via _context_menu_task).
        window.spawn(cx, async move |cx| {
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
                    CompletionPlan::Columns { sources } => {
                        sources.iter().any(|source| match source {
                            ColumnSource::Table(t) => provider
                                .try_get_cached_columns(&t.schema, &t.table)
                                .is_none(),
                            ColumnSource::Static(_) => false,
                        })
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
                    CompletionPlan::Columns { sources } => {
                        // Union columns across all in-scope sources, keeping the
                        // first occurrence so columns from earlier relations win
                        // on name collisions. Keep the full column metadata of
                        // real tables so foreign-key join conditions can be built.
                        let mut all_columns: Vec<ColumnCandidate> = Vec::new();
                        let mut seen = std::collections::HashSet::new();
                        let mut fetched_tables: Vec<(TableRef, Arc<Vec<ColumnInfo>>)> = Vec::new();
                        for source in &sources {
                            match source {
                                ColumnSource::Table(table_ref) => {
                                    if let Ok(columns) = provider
                                        .get_cached_columns(&table_ref.schema, &table_ref.table)
                                        .await
                                    {
                                        for column in columns.iter() {
                                            if seen.insert(column.name.clone()) {
                                                all_columns.push(ColumnCandidate {
                                                    name: column.name.clone(),
                                                    data_type: Some(column.data_type.clone()),
                                                });
                                            }
                                        }
                                        fetched_tables.push((table_ref.clone(), columns));
                                    }
                                }
                                ColumnSource::Static(columns) => {
                                    for column in columns {
                                        if seen.insert(column.clone()) {
                                            all_columns.push(ColumnCandidate {
                                                name: column.clone(),
                                                data_type: None,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        let detail = match sources.as_slice() {
                            [ColumnSource::Table(single)] => single.table.clone(),
                            _ => "query columns".to_string(),
                        };

                        let mut items = Vec::new();
                        // In an ON clause, offer ready-made foreign-key join
                        // conditions first so they rank above raw columns.
                        if context.clause == Some(SqlClause::On) {
                            items.extend(build_join_condition_items(
                                &context.table_aliases,
                                &fetched_tables,
                                &provider.current_schema,
                                &context.current_word,
                                range,
                            ));
                        }
                        items.extend(build_column_items(
                            &all_columns,
                            &context.current_word,
                            range,
                            &detail,
                        ));
                        // Expression clauses also accept function calls, so offer
                        // built-in functions after the columns.
                        items.extend(build_function_items(&context.current_word, range));
                        items
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

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
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

    /// A single-table `Columns` plan, the common case in these tests.
    fn columns_plan(schema: &str, table: &str) -> CompletionPlan {
        CompletionPlan::Columns {
            sources: vec![ColumnSource::Table(TableRef {
                schema: schema.to_string(),
                table: table.to_string(),
            })],
        }
    }

    /// A `Columns` plan made of statically-known columns (CTE / derived table).
    fn static_columns_plan(columns: &[&str]) -> CompletionPlan {
        CompletionPlan::Columns {
            sources: vec![ColumnSource::Static(
                columns.iter().map(|c| c.to_string()).collect(),
            )],
        }
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
        assert_eq!(plan, columns_plan("public", "foo"));
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
        assert_eq!(plan, columns_plan("sales", "orders"));
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
            cte_columns: Vec::new(),
        };
        let plan = plan_completion(&context, "public", &schemas(&["public", "sales"]), true);
        assert_eq!(plan, columns_plan("public", "shipments"));
    }

    #[test]
    fn test_plan_schema_dot_ignored_when_schemas_unsupported() {
        // MySQL/SQLite: `db.` should not be treated as a schema drill-down.
        let sql = "SELECT * FROM sales.";
        let plan = plan_completion(&ctx(sql, sql.len()), "public", &schemas(&["sales"]), false);
        assert_eq!(plan, columns_plan("public", "sales"));
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
        assert_eq!(plan, columns_plan("sales", "orders"));
    }

    #[test]
    fn test_plan_columns_in_join_union_all_tables() {
        // In a JOIN, columns from every relation in scope should be offered,
        // not just the first table.
        let aliases = vec![
            TableAlias {
                table_name: "users".to_string(),
                alias: "u".to_string(),
            },
            TableAlias {
                table_name: "sales.orders".to_string(),
                alias: "o".to_string(),
            },
        ];
        let context = TsCompletionContext {
            current_word: String::new(),
            clause: Some(SqlClause::Where),
            table_aliases: aliases,
            is_dot_notation: false,
            dot_table_name: None,
            cte_columns: Vec::new(),
        };
        let plan = plan_completion(&context, "public", &schemas(&["public", "sales"]), true);
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                sources: vec![
                    ColumnSource::Table(TableRef {
                        schema: "public".to_string(),
                        table: "users".to_string(),
                    }),
                    ColumnSource::Table(TableRef {
                        schema: "sales".to_string(),
                        table: "orders".to_string(),
                    }),
                ]
            }
        );
    }

    #[test]
    fn test_plan_cte_columns_in_where() {
        // Columns of a CTE referenced unqualified should be offered statically.
        let sql = "WITH cte AS (SELECT a, b FROM users) SELECT * FROM cte WHERE ";
        let plan = plan_completion(&ctx_end(sql), "public", &schemas(&["public"]), true);
        assert_eq!(plan, static_columns_plan(&["a", "b"]));
    }

    #[test]
    fn test_plan_cte_columns_via_alias_dot() {
        // `c.` where `c` aliases the CTE should resolve to the CTE's columns.
        let sql = "WITH cte AS (SELECT a, b FROM users) SELECT * FROM cte c WHERE c.";
        let plan = plan_completion(&ctx(sql, sql.len()), "public", &schemas(&["public"]), true);
        assert_eq!(plan, static_columns_plan(&["a", "b"]));
    }

    #[test]
    fn test_plan_derived_table_columns() {
        // A derived table (subquery in FROM) exposes its projection columns.
        let sql = "SELECT * FROM (SELECT a, b FROM users) sub WHERE ";
        let plan = plan_completion(&ctx_end(sql), "public", &schemas(&["public"]), true);
        assert_eq!(plan, static_columns_plan(&["a", "b"]));
    }

    #[test]
    fn test_plan_derived_table_columns_via_dot() {
        let sql = "SELECT * FROM (SELECT a, b FROM users) sub WHERE sub.";
        let plan = plan_completion(&ctx(sql, sql.len()), "public", &schemas(&["public"]), true);
        assert_eq!(plan, static_columns_plan(&["a", "b"]));
    }

    #[test]
    fn test_plan_cte_aliased_column_uses_alias_name() {
        // `SELECT x AS y` projects `y`, not `x`.
        let sql = "WITH cte AS (SELECT x AS y, z FROM users) SELECT * FROM cte WHERE ";
        let plan = plan_completion(&ctx_end(sql), "public", &schemas(&["public"]), true);
        assert_eq!(plan, static_columns_plan(&["y", "z"]));
    }

    #[test]
    fn test_plan_cte_joined_with_real_table_unions_both() {
        // A CTE joined with a real table contributes static columns alongside
        // the real table's fetched columns.
        let sql = "WITH cte AS (SELECT a FROM users) SELECT * FROM cte c JOIN orders o ON c.a = o.a WHERE ";
        let plan = plan_completion(&ctx_end(sql), "public", &schemas(&["public"]), true);
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                sources: vec![
                    ColumnSource::Static(vec!["a".to_string()]),
                    ColumnSource::Table(TableRef {
                        schema: "public".to_string(),
                        table: "orders".to_string(),
                    }),
                ]
            }
        );
    }

    #[test]
    fn test_plan_column_clause_without_tables_still_columns() {
        // `SELECT ` with no FROM yet is still a column/expression context, so the
        // plan is `Columns` (with no sources) and functions can be offered.
        let sql = "SELECT ";
        let plan = plan_completion(&ctx_end(sql), "public", &schemas(&["public"]), true);
        assert_eq!(
            plan,
            CompletionPlan::Columns {
                sources: Vec::new()
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
            cte_columns: Vec::new(),
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

    fn typed_candidate(name: &str, data_type: &str) -> ColumnCandidate {
        ColumnCandidate {
            name: name.to_string(),
            data_type: Some(data_type.to_string()),
        }
    }

    #[test]
    fn test_build_column_items_filter_and_detail() {
        let cols = vec![
            typed_candidate("id", "integer"),
            typed_candidate("company", "text"),
            typed_candidate("region", "text"),
        ];
        let items = build_column_items(&cols, "co", dummy_range(), "customers");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "company");
        // The data type is shown as the detail hint.
        assert_eq!(items[0].detail.as_deref(), Some("text"));
        assert_eq!(items[0].kind, Some(CompletionItemKind::FIELD));
    }

    #[test]
    fn test_build_column_items_type_hint_falls_back_to_table() {
        // Columns without a known type (CTE / derived) fall back to the table.
        let cols = vec![ColumnCandidate {
            name: "total".to_string(),
            data_type: None,
        }];
        let items = build_column_items(&cols, "", dummy_range(), "summary");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].detail.as_deref(), Some("Column from summary"));
    }

    #[test]
    fn test_build_function_items_filter_and_signature() {
        let items = build_function_items("co", dummy_range());
        // COALESCE, CONCAT, COUNT all start with "co".
        assert!(items.iter().any(|i| i.label == "COALESCE"));
        assert!(items.iter().any(|i| i.label == "COUNT"));
        assert!(
            items
                .iter()
                .all(|i| i.kind == Some(CompletionItemKind::FUNCTION))
        );
        let coalesce = items
            .iter()
            .find(|i| i.label == "COALESCE")
            .expect("COALESCE present");
        assert!(
            coalesce
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("COALESCE(")
        );
        assert_eq!(coalesce.insert_text.as_deref(), Some("COALESCE()"));
    }

    #[test]
    fn test_build_function_items_empty_word_lists_all() {
        let items = build_function_items("", dummy_range());
        assert!(items.iter().any(|i| i.label == "NOW"));
        assert!(items.iter().any(|i| i.label == "COUNT"));
        assert!(items.iter().any(|i| i.label == "ROW_NUMBER"));
    }

    #[test]
    fn test_build_function_items_filter_excludes_non_matching() {
        let items = build_function_items("now", dummy_range());
        assert!(items.iter().any(|i| i.label == "NOW"));
        assert!(
            items
                .iter()
                .all(|i| i.label.to_lowercase().starts_with("now"))
        );
    }

    fn plain_column(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "integer".to_string(),
            is_nullable: false,
            is_primary_key: false,
            default_value: None,
            character_maximum_length: None,
            foreign_key: None,
        }
    }

    fn fk_column(name: &str, foreign_table: &str, foreign_column: &str) -> ColumnInfo {
        ColumnInfo {
            foreign_key: Some(blanco_core::ForeignKeyInfo {
                foreign_table_name: foreign_table.to_string(),
                foreign_column_name: foreign_column.to_string(),
                constraint_name: None,
            }),
            ..plain_column(name)
        }
    }

    #[test]
    fn test_build_join_conditions_from_fk() {
        let orders = vec![plain_column("id"), fk_column("user_id", "users", "id")];
        let users = vec![plain_column("id")];
        let tables = vec![
            JoinTable {
                alias: "o",
                table_name: "orders",
                columns: &orders,
            },
            JoinTable {
                alias: "u",
                table_name: "users",
                columns: &users,
            },
        ];
        assert_eq!(
            build_join_conditions(&tables),
            vec!["o.user_id = u.id".to_string()]
        );
    }

    #[test]
    fn test_build_join_conditions_ignores_out_of_scope_target() {
        // FK references a table that is not part of the join -> no suggestion.
        let orders = vec![fk_column("user_id", "users", "id")];
        let tables = vec![JoinTable {
            alias: "o",
            table_name: "orders",
            columns: &orders,
        }];
        assert!(build_join_conditions(&tables).is_empty());
    }

    #[test]
    fn test_build_join_condition_items_filtered_by_word() {
        let orders = vec![fk_column("user_id", "users", "id")];
        let users = vec![plain_column("id")];
        let aliases = vec![
            TableAlias {
                table_name: "orders".to_string(),
                alias: "o".to_string(),
            },
            TableAlias {
                table_name: "users".to_string(),
                alias: "u".to_string(),
            },
        ];
        let fetched = vec![
            (
                TableRef {
                    schema: "public".to_string(),
                    table: "orders".to_string(),
                },
                Arc::new(orders),
            ),
            (
                TableRef {
                    schema: "public".to_string(),
                    table: "users".to_string(),
                },
                Arc::new(users),
            ),
        ];
        let items = build_join_condition_items(&aliases, &fetched, "public", "o.", dummy_range());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "o.user_id = u.id");
        assert_eq!(items[0].kind, Some(CompletionItemKind::SNIPPET));

        // A non-matching prefix filters it out.
        let none = build_join_condition_items(&aliases, &fetched, "public", "zzz", dummy_range());
        assert!(none.is_empty());
    }
}
