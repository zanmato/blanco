mod cache;
mod context;
mod fetch;

use blanco_core::{ColumnInfo, DatabaseType, FunctionSignatureInfo, QueryableEntity};
pub use cache::{CacheEntry, MetadataCache};
pub use context::{generate_table_abbreviation, resolve_table_alias};
pub use fetch::{
    fetch_columns, fetch_function_signatures, fetch_queryable_entities, fetch_schemas,
};

use crate::sql::functions;

use cache::columns_key;

use sql_parser::statement_parser::{
    self, CompletionContext as TsCompletionContext, DerivedRelation, SqlClause, TableAlias,
    ident_eq,
};

use anyhow::Result;
use database::DatabaseServiceTrait;
use futures::FutureExt as _;
use futures::future::Shared;
use gpui::{App, AppContext as _, BackgroundExecutor, Pixels, Task, Window, px};
use gpui_component::input::{CompletionProvider, Rope, RopeExt};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    InsertTextFormat, Range, TextEdit,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

const CACHE_TTL_SECONDS: u64 = 300; // 5 minutes cache TTL

/// Widest the completion menu grows. The default fits short identifiers but
/// cuts off long table and column names together with their type.
pub const COMPLETION_MENU_MAX_WIDTH: Pixels = px(480.);

/// Labels longer than this are shortened in the middle, so both the prefix
/// being typed and the distinguishing end of a long name stay visible.
const MAX_LABEL_CHARS: usize = 48;

/// Default schema used when a tab has no explicit schema, keyed by backend:
/// PostgreSQL uses `public`, SQL Server uses `dbo`, and the schema-less backends
/// (MySQL, SQLite, ClickHouse) fall back to the database name since their
/// "schema" is effectively the database.
fn default_schema_for(driver: DatabaseType, database_name: &str) -> String {
    driver
        .dialect()
        .default_schema()
        .map(str::to_string)
        .unwrap_or_else(|| database_name.to_string())
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

/// What a completion item offers. Items are listed in this order, so where a
/// name is expected the names of the query and the current schema come before
/// other schemas, functions and keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ItemGroup {
    JoinCondition,
    Column,
    /// CTEs and the tables of the current schema.
    Relation,
    /// Other schemas, whose tables are one `.` away.
    Schema,
    Function,
    Keyword,
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

/// One piece of metadata the provider caches and fetches as a unit.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MetadataKey {
    Schemas,
    Tables { schema: String },
    Columns { schema: String, table: String },
    Functions { schema: String },
}

impl std::fmt::Display for MetadataKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetadataKey::Schemas => write!(formatter, "schemas"),
            MetadataKey::Tables { schema } => write!(formatter, "tables of {schema}"),
            MetadataKey::Columns { schema, table } => {
                write!(formatter, "columns of {schema}.{table}")
            }
            MetadataKey::Functions { schema } => write!(formatter, "functions of {schema}"),
        }
    }
}

/// A running metadata fetch. Every request that needs the same metadata awaits
/// the same fetch, and the provider keeps it alive when those requests go away.
type SharedFetch = Shared<Task<Result<(), Arc<anyhow::Error>>>>;

/// SQL Completion Provider that implements gpui-component's CompletionProvider trait
#[derive(Clone)]
pub struct SqlCompletionProvider {
    pub connection_id: i64,
    pub database_name: String,
    /// The schema the tab is connected to ("current schema"). Tables here are
    /// offered unqualified; other schemas appear as drill-in namespaces.
    pub current_schema: String,
    /// The connection's backend, which selects the built-in function catalog.
    pub driver: DatabaseType,
    db_service: Arc<dyn DatabaseServiceTrait>,
    cache: Arc<Mutex<MetadataCache>>,
    /// Fetches that have not finished yet. The editor drops a completion
    /// request as soon as the next keystroke replaces it, so a fetch owned by
    /// the request would restart on every key and never finish on a connection
    /// slower than the typing. Owned here, it runs to completion and fills the
    /// cache for the requests that follow.
    in_flight: Arc<Mutex<HashMap<MetadataKey, SharedFetch>>>,
    executor: BackgroundExecutor,
    /// Upper bound for one metadata fetch, `None` for unlimited. Without it a
    /// fetch that hangs on a dead connection would stay in flight forever and
    /// block every later request for the same metadata.
    fetch_timeout: Option<Duration>,
}

impl SqlCompletionProvider {
    pub fn new(
        connection_id: i64,
        database_name: String,
        schema_name: Option<String>,
        driver: DatabaseType,
        db_service: Arc<dyn DatabaseServiceTrait>,
        executor: BackgroundExecutor,
    ) -> Self {
        let current_schema = schema_name
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_schema_for(driver, &database_name));
        Self {
            connection_id,
            database_name,
            current_schema,
            driver,
            db_service,
            cache: Arc::new(Mutex::new(MetadataCache::new())),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            executor,
            fetch_timeout: None,
        }
    }

    /// The same provider (sharing its cache and running fetches) with the
    /// timeout applied to fetches it starts from now on. Callers pass the
    /// user's query timeout, read where an `App` is at hand.
    pub fn with_fetch_timeout(&self, fetch_timeout: Option<Duration>) -> Self {
        Self {
            fetch_timeout,
            ..self.clone()
        }
    }

    /// Read a cache entry without waiting. An expired entry is still returned,
    /// with a refresh started behind it, so suggestions do not go blank for the
    /// length of a round trip every time the TTL runs out.
    fn lookup<T>(
        &self,
        key: MetadataKey,
        entry: impl FnOnce(&MetadataCache) -> Option<&CacheEntry<T>>,
    ) -> Option<Arc<T>> {
        let (data, expired) = {
            let cache = self.cache.lock().ok()?;
            let entry = entry(&cache)?;
            (Arc::clone(&entry.data), entry.is_expired(CACHE_TTL_SECONDS))
        };
        if expired {
            // Not awaited: `in_flight` keeps the refresh running.
            drop(self.start_fetch(key));
        }
        Some(data)
    }

    /// Join the running fetch for `key`, starting one when there is none.
    fn start_fetch(&self, key: MetadataKey) -> SharedFetch {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(fetch) = in_flight.get(&key) {
            return fetch.clone();
        }

        let fetch = self
            .executor
            .spawn({
                let provider = self.clone();
                let key = key.clone();
                async move {
                    let result = provider.fetch_into_cache(&key).await.map_err(Arc::new);
                    if let Err(error) = &result {
                        tracing::warn!("Failed to load {key} for completion: {error:#}");
                    }
                    provider
                        .in_flight
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&key);
                    result
                }
            })
            .shared();
        in_flight.insert(key, fetch.clone());
        fetch
    }

    /// Wait for `key` to be fetched and cached.
    async fn load(&self, key: MetadataKey) -> Result<()> {
        self.start_fetch(key)
            .await
            .map_err(|error| anyhow::anyhow!("{error:#}"))
    }

    async fn fetch_into_cache(&self, key: &MetadataKey) -> Result<()> {
        let fetch = self.fetch_and_store(key);
        match self.fetch_timeout {
            Some(timeout) => {
                let deadline = async {
                    self.executor.timer(timeout).await;
                    Err(anyhow::anyhow!(
                        "loading {key} exceeded the configured {}s query timeout",
                        timeout.as_secs()
                    ))
                };
                smol::future::or(fetch, deadline).await
            }
            None => fetch.await,
        }
    }

    async fn fetch_and_store(&self, key: &MetadataKey) -> Result<()> {
        let db_service = &*self.db_service;
        let generation = self.cache_generation();
        match key {
            MetadataKey::Schemas => {
                let (schemas, supports) =
                    fetch_schemas(db_service, self.connection_id, &self.database_name).await?;
                self.store(generation, |cache| {
                    cache.supports_schemas = Some(supports);
                    cache.schemas = Some(CacheEntry::new(schemas));
                });
            }
            MetadataKey::Tables { schema } => {
                let entities = fetch_queryable_entities(
                    db_service,
                    self.connection_id,
                    &self.database_name,
                    Some(schema),
                )
                .await?;
                self.store(generation, |cache| {
                    cache
                        .tables_by_schema
                        .insert(schema.clone(), CacheEntry::new(entities));
                });
            }
            MetadataKey::Columns { schema, table } => {
                let columns = fetch_columns(
                    db_service,
                    self.connection_id,
                    table,
                    &self.database_name,
                    Some(schema),
                )
                .await?;
                self.store(generation, |cache| {
                    cache
                        .columns
                        .insert(columns_key(schema, table), CacheEntry::new(columns));
                });
            }
            MetadataKey::Functions { schema } => {
                let functions = fetch_function_signatures(
                    db_service,
                    self.connection_id,
                    &self.database_name,
                    Some(schema),
                )
                .await?;
                self.store(generation, |cache| {
                    cache
                        .functions_by_schema
                        .insert(schema.clone(), CacheEntry::new(functions));
                });
            }
        }
        Ok(())
    }

    fn cache_generation(&self) -> u64 {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generation
    }

    /// Write a fetched result, unless the cache was invalidated while the
    /// fetch ran: what it read may predate the DDL that invalidated it.
    fn store(&self, generation: u64, write: impl FnOnce(&mut MetadataCache)) {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.generation == generation {
            write(&mut cache);
        }
    }

    /// Return cached (schemas, supports_schemas) if there is an entry. No waiting.
    pub fn try_get_cached_schemas(&self) -> Option<(Arc<Vec<String>>, bool)> {
        let supports = self.cache.lock().ok()?.supports_schemas?;
        let schemas = self.lookup(MetadataKey::Schemas, |cache| cache.schemas.as_ref())?;
        Some((schemas, supports))
    }

    /// Get cached schemas or fetch them. Returns the user schemas and whether
    /// the backend supports schemas at all.
    pub async fn get_cached_schemas(&self) -> Result<(Arc<Vec<String>>, bool)> {
        if let Some(cached) = self.try_get_cached_schemas() {
            return Ok(cached);
        }
        let key = MetadataKey::Schemas;
        self.load(key.clone()).await?;
        self.try_get_cached_schemas()
            .ok_or_else(|| not_cached(&key))
    }

    /// Return cached tables for a schema if there is an entry. No waiting.
    pub fn try_get_cached_tables(&self, schema: &str) -> Option<Arc<Vec<QueryableEntity>>> {
        let key = MetadataKey::Tables {
            schema: schema.to_string(),
        };
        self.lookup(key, |cache| cache.tables_by_schema.get(schema))
    }

    /// Return cached columns for `schema.table` if there is an entry. No waiting.
    pub fn try_get_cached_columns(
        &self,
        schema: &str,
        table: &str,
    ) -> Option<Arc<Vec<ColumnInfo>>> {
        let key = MetadataKey::Columns {
            schema: schema.to_string(),
            table: table.to_string(),
        };
        self.lookup(key, |cache| cache.columns.get(&columns_key(schema, table)))
    }

    /// Get cached tables for a schema or fetch them if not cached
    pub async fn get_cached_tables(&self, schema: &str) -> Result<Arc<Vec<QueryableEntity>>> {
        if let Some(cached) = self.try_get_cached_tables(schema) {
            return Ok(cached);
        }
        let key = MetadataKey::Tables {
            schema: schema.to_string(),
        };
        self.load(key.clone()).await?;
        self.try_get_cached_tables(schema)
            .ok_or_else(|| not_cached(&key))
    }

    /// Get cached columns for `schema.table` or fetch them if not cached
    pub async fn get_cached_columns(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<Arc<Vec<ColumnInfo>>> {
        if let Some(cached) = self.try_get_cached_columns(schema, table) {
            return Ok(cached);
        }
        let key = MetadataKey::Columns {
            schema: schema.to_string(),
            table: table.to_string(),
        };
        self.load(key.clone()).await?;
        self.try_get_cached_columns(schema, table)
            .ok_or_else(|| not_cached(&key))
    }

    /// Return cached user-defined functions for a schema if there is an entry.
    /// No waiting.
    pub fn try_get_cached_functions(
        &self,
        schema: &str,
    ) -> Option<Arc<Vec<FunctionSignatureInfo>>> {
        let key = MetadataKey::Functions {
            schema: schema.to_string(),
        };
        self.lookup(key, |cache| cache.functions_by_schema.get(schema))
    }

    /// Get cached user-defined functions for a schema or fetch them if not
    /// cached
    pub async fn get_cached_functions(
        &self,
        schema: &str,
    ) -> Result<Arc<Vec<FunctionSignatureInfo>>> {
        if let Some(cached) = self.try_get_cached_functions(schema) {
            return Ok(cached);
        }
        let key = MetadataKey::Functions {
            schema: schema.to_string(),
        };
        self.load(key.clone()).await?;
        self.try_get_cached_functions(schema)
            .ok_or_else(|| not_cached(&key))
    }

    /// Invalidate all cached metadata (call after DDL statements)
    pub fn invalidate_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.invalidate_all();
        }
        // Fetches still running read the catalog before the DDL. Their results
        // are refused by `store`; forgetting them here makes the next request
        // start a fresh one instead of joining them.
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// The cache was invalidated between a fetch finishing and its result being
/// read back, so there is nothing to return for this request.
fn not_cached(key: &MetadataKey) -> anyhow::Error {
    anyhow::anyhow!("{key} were invalidated while loading")
}

fn is_valid_identifier(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !word.chars().next().is_none_or(|c| c.is_ascii_digit())
}

/// A word that starts with a digit is a number being typed, never a name.
fn is_number_literal(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_digit())
}

pub(crate) fn is_sql_keyword(word: &str) -> bool {
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

/// Look up the CTE or derived table referenced by `name`, either directly
/// (`FROM cte`) or via an alias that maps to such a relation.
fn lookup_derived_relation<'a>(
    context: &'a TsCompletionContext,
    name: &str,
) -> Option<&'a DerivedRelation> {
    if let Some(relation) = context
        .derived_relations
        .iter()
        .find(|relation| ident_eq(&relation.name, name))
    {
        return Some(relation);
    }
    let resolved = resolve_table_alias(&context.table_aliases, name)?;
    context
        .derived_relations
        .iter()
        .find(|relation| ident_eq(&relation.name, &resolved))
}

/// The column sources of a CTE or derived table: the columns it names, and the
/// tables it selects `*` from.
fn derived_sources(relation: &DerivedRelation, current_schema: &str) -> Vec<ColumnSource> {
    let mut sources = Vec::new();
    if !relation.columns.is_empty() {
        sources.push(ColumnSource::Static(relation.columns.clone()));
    }
    for table_name in &relation.all_columns_of {
        let (schema, table) = split_schema_qualified(table_name, current_schema);
        push_unique(
            &mut sources,
            ColumnSource::Table(TableRef { schema, table }),
        );
    }
    sources
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
    if is_number_literal(&context.current_word) {
        return CompletionPlan::Nothing;
    }

    if context.is_dot_notation {
        let Some(name) = context.dot_table_name.as_deref() else {
            return CompletionPlan::Nothing;
        };

        // A CTE or derived-table referenced by this name -> its columns.
        if let Some(relation) = lookup_derived_relation(context, name) {
            return CompletionPlan::Columns {
                sources: derived_sources(relation, current_schema),
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
        // derived tables contribute their projection columns.
        let mut sources: Vec<ColumnSource> = Vec::new();
        let mut covered_derived: Vec<&str> = Vec::new();

        for alias in &context.table_aliases {
            // A relation aliased to a CTE/derived name uses its columns
            // (e.g. `FROM cte c` or `FROM (subquery) c`).
            if let Some(relation) = lookup_derived_relation(context, &alias.alias) {
                covered_derived.push(&relation.name);
                for source in derived_sources(relation, current_schema) {
                    push_unique(&mut sources, source);
                }
                continue;
            }
            let (schema, table) = split_schema_qualified(&alias.table_name, current_schema);
            push_unique(
                &mut sources,
                ColumnSource::Table(TableRef { schema, table }),
            );
        }

        // Derived tables (`FROM (subquery) sub`) have no table name, so they
        // are not in `table_aliases`.
        for relation in &context.derived_relations {
            if !covered_derived
                .iter()
                .any(|covered| ident_eq(covered, &relation.name))
            {
                for source in derived_sources(relation, current_schema) {
                    push_unique(&mut sources, source);
                }
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

/// Build completion items for the CTEs the statement defines. Like tables,
/// each inserts `name <abbrev>`.
fn build_cte_items(cte_names: &[String], current_word: &str, range: Range) -> Vec<CompletionItem> {
    filter_and_sort(cte_names, current_word)
        .into_iter()
        .map(|name| {
            let insert_text = generate_table_abbreviation(&name);
            CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::CLASS),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    insert_text.clone(),
                ))),
                detail: Some("CTE".to_string()),
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

/// Completion items shown when nothing has been typed yet. The dialect catalogs
/// hold up to a couple of thousand functions, far more than a menu can show.
const UNFILTERED_FUNCTION_LIMIT: usize = 200;

/// Build completion items for the dialect's built-in functions and the
/// schema's user-defined functions whose name starts with `current_word`.
///
/// Inserts `NAME($0)` as a snippet so the parentheses are balanced and the
/// cursor lands between them, where signature help takes over. Overloads
/// collapse into one item per name.
fn build_function_items(
    driver: DatabaseType,
    user_functions: &[FunctionSignatureInfo],
    current_word: &str,
    range: Range,
) -> Vec<CompletionItem> {
    let needle = current_word.to_lowercase();
    let matches = |name: &str| name.to_lowercase().starts_with(&needle);

    let make_item =
        |name: &str, detail: String, documentation: Option<lsp_types::Documentation>| {
            let insert_text = format!("{name}($0)");
            CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    range,
                    insert_text.clone(),
                ))),
                detail: Some(detail),
                documentation,
                insert_text: Some(insert_text),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            }
        };

    let mut items: Vec<CompletionItem> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // User-defined functions rank first: they are what this schema offers
    // beyond the dialect, and they are usually what the user is looking for.
    for function in user_functions {
        if !matches(&function.name) || !seen.insert(function.name.to_lowercase()) {
            continue;
        }
        let overloads = user_functions
            .iter()
            .filter(|other| other.name.eq_ignore_ascii_case(&function.name))
            .count();
        let mut detail = format!("{}({})", function.name, function.parameters.join(", "));
        if let Some(return_type) = function.return_type.as_deref().filter(|t| !t.is_empty()) {
            detail.push_str(" -> ");
            detail.push_str(return_type);
        }
        if overloads > 1 {
            detail.push_str(&format!(" (+{} overloads)", overloads - 1));
        }
        let documentation = function
            .comment
            .as_deref()
            .filter(|c| !c.is_empty())
            .map(|comment| {
                lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent {
                    kind: lsp_types::MarkupKind::Markdown,
                    value: comment.to_string(),
                })
            });
        items.push(make_item(&function.name, detail, documentation));
    }

    let mut builtins: Vec<&functions::FunctionGroup> = functions::function_groups(driver)
        .iter()
        .filter(|group| matches(group.name) && !seen.contains(&group.name.to_lowercase()))
        .collect();
    if needle.is_empty() {
        builtins.sort_by_key(|group| (group.name.len(), group.name));
        builtins.truncate(UNFILTERED_FUNCTION_LIMIT);
    }
    for group in builtins {
        let Some(first) = group.overloads.first() else {
            continue;
        };
        let mut detail = first.label();
        if group.overloads.len() > 1 {
            detail.push_str(&format!(" (+{} overloads)", group.overloads.len() - 1));
        }
        items.push(make_item(group.name, detail, first.lsp_documentation()));
    }
    items
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

/// Shorten `text` to `max_chars` by replacing its middle with an ellipsis,
/// keeping at least `min_head` leading characters.
fn elide_middle(text: &str, max_chars: usize, min_head: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let head = (max_chars.saturating_sub(1) / 2).max(min_head);
    let tail = max_chars.saturating_sub(head + 1);
    if head + tail >= count {
        return text.to_string();
    }
    let mut elided: String = text.chars().take(head).collect();
    elided.push('…');
    elided.extend(text.chars().skip(count - tail));
    elided
}

/// Flatten the item groups into the order they are listed in, and number the
/// items in `sort_text` so a client that sorts keeps that order.
///
/// The menu highlights as many leading bytes of a label as the user has typed,
/// so an elided label keeps the typed prefix ahead of the ellipsis.
fn rank_items(
    mut groups: Vec<(ItemGroup, Vec<CompletionItem>)>,
    current_word: &str,
) -> Vec<CompletionItem> {
    groups.sort_by_key(|(group, _)| *group);
    let min_head = current_word.chars().count() + 1;
    groups
        .into_iter()
        .flat_map(|(_, items)| items)
        .enumerate()
        .map(|(position, item)| CompletionItem {
            label: elide_middle(&item.label, MAX_LABEL_CHARS, min_head),
            sort_text: Some(format!("{position:05}")),
            ..item
        })
        .collect()
}

/// Keyword items for a position where a name is expected. They follow the
/// names, only once a word is being typed (an empty word would bury the names
/// under every keyword), and not when a function item already offers the word.
fn keyword_items_after_names(
    current_word: &str,
    range: Range,
    function_items: &[CompletionItem],
) -> Vec<CompletionItem> {
    if current_word.is_empty() {
        return Vec::new();
    }
    build_keyword_items(current_word, range)
        .into_iter()
        .filter(|keyword| {
            !function_items
                .iter()
                .any(|function| function.label.eq_ignore_ascii_case(&keyword.label))
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
        let provider = self.with_fetch_timeout(
            crate::app_settings::AppSettings::global(cx)
                .settings
                .database
                .query_timeout(),
        );
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
                if is_number_literal(&context.current_word) {
                    return Ok(CompletionResponse::Array(Vec::new()));
                }

                // Debounce before any DB roundtrip, but only if something we need
                // is not already cached. We may sleep at most once.
                let mut debounced = false;

                // Schemas drive dot-notation disambiguation and the FROM listing.
                let (schemas, supports_schemas) = match provider.try_get_cached_schemas() {
                    Some(cached) => cached,
                    None => {
                        background_executor.timer(Duration::from_millis(300)).await;
                        debounced = true;
                        provider.get_cached_schemas().await.unwrap_or_else(|error| {
                            tracing::warn!("Completing without schemas: {error:#}");
                            (Arc::new(Vec::new()), false)
                        })
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

                let groups = match plan {
                    CompletionPlan::Columns { sources } => {
                        // Column and function fetches go out together: on a
                        // slow connection the round trips overlap instead of
                        // adding up.
                        let column_fetches =
                            futures::future::join_all(sources.iter().map(|source| async {
                                match source {
                                    ColumnSource::Table(table_ref) => provider
                                        .get_cached_columns(&table_ref.schema, &table_ref.table)
                                        .await
                                        .inspect_err(|error| {
                                            tracing::warn!(
                                                "Completing without columns of {}.{}: {error:#}",
                                                table_ref.schema,
                                                table_ref.table
                                            );
                                        })
                                        .ok(),
                                    ColumnSource::Static(_) => None,
                                }
                            }));
                        let (fetched_columns, user_functions) = futures::join!(
                            column_fetches,
                            provider.get_cached_functions(&provider.current_schema)
                        );

                        // Union columns across all in-scope sources, keeping the
                        // first occurrence so columns from earlier relations win
                        // on name collisions. Keep the full column metadata of
                        // real tables so foreign-key join conditions can be built.
                        let mut all_columns: Vec<ColumnCandidate> = Vec::new();
                        let mut seen = std::collections::HashSet::new();
                        let mut fetched_tables: Vec<(TableRef, Arc<Vec<ColumnInfo>>)> = Vec::new();
                        for (source, fetched) in sources.iter().zip(fetched_columns) {
                            match source {
                                ColumnSource::Table(table_ref) => {
                                    if let Some(columns) = fetched {
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

                        let mut groups = vec![(
                            ItemGroup::Column,
                            build_column_items(&all_columns, &context.current_word, range, &detail),
                        )];
                        // After `name.` only that relation's columns fit.
                        if !context.is_dot_notation {
                            // In an ON clause, offer ready-made foreign-key
                            // join conditions above raw columns.
                            if context.clause == Some(SqlClause::On) {
                                groups.push((
                                    ItemGroup::JoinCondition,
                                    build_join_condition_items(
                                        &context.table_aliases,
                                        &fetched_tables,
                                        &provider.current_schema,
                                        &context.current_word,
                                        range,
                                    ),
                                ));
                            }
                            let user_functions = user_functions.unwrap_or_else(|error| {
                                tracing::warn!("Completing without user functions: {error:#}");
                                Arc::new(Vec::new())
                            });
                            let function_items = build_function_items(
                                provider.driver,
                                &user_functions,
                                &context.current_word,
                                range,
                            );
                            groups.push((
                                ItemGroup::Keyword,
                                keyword_items_after_names(
                                    &context.current_word,
                                    range,
                                    &function_items,
                                ),
                            ));
                            groups.push((ItemGroup::Function, function_items));
                        }
                        groups
                    }
                    CompletionPlan::SchemaTables { schema } => {
                        let tables = match provider.get_cached_tables(&schema).await {
                            Ok(entities) => {
                                build_table_items(&entities, &context.current_word, range)
                            }
                            Err(_) => Vec::new(),
                        };
                        vec![(ItemGroup::Relation, tables)]
                    }
                    CompletionPlan::TablesAndSchemas => {
                        let tables =
                            match provider.get_cached_tables(&provider.current_schema).await {
                                Ok(entities) => {
                                    build_table_items(&entities, &context.current_word, range)
                                }
                                Err(_) => Vec::new(),
                            };
                        let other_schemas: Vec<String> = schemas
                            .iter()
                            .filter(|s| *s != &provider.current_schema)
                            .cloned()
                            .collect();
                        vec![
                            (
                                ItemGroup::Relation,
                                build_cte_items(&context.cte_names, &context.current_word, range),
                            ),
                            (ItemGroup::Relation, tables),
                            (
                                ItemGroup::Schema,
                                build_schema_items(&other_schemas, &context.current_word, range),
                            ),
                            (
                                ItemGroup::Keyword,
                                keyword_items_after_names(&context.current_word, range, &[]),
                            ),
                        ]
                    }
                    CompletionPlan::Keywords => vec![(
                        ItemGroup::Keyword,
                        build_keyword_items(&context.current_word, range),
                    )],
                    CompletionPlan::Nothing => Vec::new(),
                };

                Ok(CompletionResponse::Array(rank_items(
                    groups,
                    &context.current_word,
                )))
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
    use sql_parser::statement_parser::TableAlias;

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

    // ---- metadata fetching ----

    /// A database service whose connections never arrive until the test lets
    /// them fail, standing in for a connection slower than the typing.
    struct StalledService {
        attempts: std::sync::atomic::AtomicUsize,
        release: smol::channel::Receiver<()>,
    }

    impl StalledService {
        fn new() -> (Arc<Self>, smol::channel::Sender<()>) {
            let (sender, release) = smol::channel::unbounded();
            let service = Arc::new(Self {
                attempts: std::sync::atomic::AtomicUsize::new(0),
                release,
            });
            (service, sender)
        }

        fn attempts(&self) -> usize {
            self.attempts.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl DatabaseServiceTrait for StalledService {
        async fn get_or_create_connection_by_id(
            &self,
            _connection_id: i64,
            _database: Option<&str>,
        ) -> Result<Arc<dyn blanco_core::Connection>> {
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.release.recv().await?;
            anyhow::bail!("connection lost")
        }

        async fn get_connection_status(
            &self,
            _connection_id: i64,
            _database_name: Option<&str>,
        ) -> Result<blanco_core::ConnectionStatus> {
            anyhow::bail!("not connected")
        }

        async fn get_active_connection_statuses(
            &self,
        ) -> Result<HashMap<(i64, String), blanco_core::ConnectionStatus>> {
            Ok(HashMap::new())
        }
    }

    fn stalled_provider(
        cx: &gpui::TestAppContext,
    ) -> (
        SqlCompletionProvider,
        Arc<StalledService>,
        smol::channel::Sender<()>,
    ) {
        let (service, release) = StalledService::new();
        let provider = SqlCompletionProvider::new(
            1,
            "app".to_string(),
            None,
            DatabaseType::PostgreSQL,
            service.clone(),
            cx.executor(),
        );
        (provider, service, release)
    }

    fn request_tables(
        provider: &SqlCompletionProvider,
        cx: &gpui::TestAppContext,
    ) -> Task<Result<Arc<Vec<QueryableEntity>>>> {
        let provider = provider.clone();
        cx.background_spawn(async move { provider.get_cached_tables("public").await })
    }

    /// The editor drops a completion request on the next keystroke. The fetch
    /// it started must keep running and serve the request that replaces it,
    /// instead of starting over.
    #[gpui::test]
    async fn metadata_fetch_outlives_the_request_that_started_it(cx: &mut gpui::TestAppContext) {
        let (provider, service, release) = stalled_provider(cx);

        let first = request_tables(&provider, cx);
        cx.run_until_parked();
        assert_eq!(service.attempts(), 1);

        drop(first);
        cx.run_until_parked();
        let second = request_tables(&provider, cx);
        cx.run_until_parked();
        assert_eq!(
            service.attempts(),
            1,
            "the replacing request should join the running fetch"
        );

        release.send(()).await.expect("service is waiting");
        let error = second.await.expect_err("the connection was lost");
        assert!(
            format!("{error:#}").contains("connection lost"),
            "{error:#}"
        );

        // A failure is not cached: the next request tries again.
        let _third = request_tables(&provider, cx);
        cx.run_until_parked();
        assert_eq!(service.attempts(), 2);
    }

    #[gpui::test]
    async fn stalled_metadata_fetch_gives_up_at_the_query_timeout(cx: &mut gpui::TestAppContext) {
        let (provider, service, _release) = stalled_provider(cx);
        let provider = provider.with_fetch_timeout(Some(Duration::from_secs(30)));

        let request = request_tables(&provider, cx);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(31));
        let error = request.await.expect_err("the fetch never answered");
        assert!(
            format!("{error:#}").contains("30s query timeout"),
            "{error:#}"
        );

        // The stalled fetch is gone, so the next request is not stuck behind it.
        let _retry = request_tables(&provider, cx);
        cx.run_until_parked();
        assert_eq!(service.attempts(), 2);
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
        assert_eq!(c.table_aliases.len(), 1);
        assert_eq!(c.table_aliases[0].table_name, "users");
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
            derived_relations: Vec::new(),
            cte_names: Vec::new(),
            enclosing_call: None,
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
            derived_relations: Vec::new(),
            cte_names: Vec::new(),
            enclosing_call: None,
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

    /// Plan at the `|` in `sql`, the way the editor asks while the user types.
    fn plan_at_marker(sql: &str) -> CompletionPlan {
        let cursor = sql.find('|').expect("sql has a cursor marker");
        let text = sql.replacen('|', "", 1);
        plan_completion(
            &ctx(&text, cursor),
            "public",
            &schemas(&["public", "sales"]),
            true,
        )
    }

    fn table_source(schema: &str, table: &str) -> ColumnSource {
        ColumnSource::Table(TableRef {
            schema: schema.to_string(),
            table: table.to_string(),
        })
    }

    #[test]
    fn test_plan_derived_columns_after_name_in_select_list() {
        // An unfinished `x.` in the select list breaks the statement apart;
        // the FROM clause after it must still resolve.
        let expected = static_columns_plan(&["a", "b"]);
        assert_eq!(
            plan_at_marker("WITH cte AS (SELECT a, b FROM users) SELECT c.| FROM cte c"),
            expected
        );
        assert_eq!(
            plan_at_marker("WITH cte AS (SELECT a, b FROM users) SELECT cte.| FROM cte"),
            expected
        );
        assert_eq!(
            plan_at_marker("SELECT sub.| FROM (SELECT a, b FROM users) sub"),
            expected
        );
        assert_eq!(
            plan_at_marker(
                "SELECT o.id, sub.| FROM orders o JOIN (SELECT a, b FROM users) AS sub ON true"
            ),
            expected
        );
    }

    #[test]
    fn test_plan_table_alias_in_select_list() {
        assert_eq!(
            plan_at_marker("SELECT u.| FROM sales.users u"),
            columns_plan("sales", "users")
        );
    }

    #[test]
    fn test_plan_columns_of_select_list_before_from() {
        assert_eq!(
            plan_at_marker("WITH cte AS (SELECT a, b FROM users) SELECT | FROM cte"),
            static_columns_plan(&["a", "b"])
        );
    }

    #[test]
    fn test_plan_cte_column_list_names_the_columns() {
        assert_eq!(
            plan_at_marker(
                "WITH cte(x, y) AS (SELECT a, b FROM users) SELECT * FROM cte WHERE cte.|"
            ),
            static_columns_plan(&["x", "y"])
        );
    }

    #[test]
    fn test_plan_cte_star_resolves_to_its_tables() {
        assert_eq!(
            plan_at_marker("WITH recent AS (SELECT * FROM sales.orders) SELECT r.| FROM recent r"),
            columns_plan("sales", "orders")
        );
        assert_eq!(
            plan_at_marker(
                "WITH paid AS (SELECT o.*, u.name FROM orders o JOIN users u ON u.id = o.user_id) \
                 SELECT * FROM paid WHERE paid.|"
            ),
            CompletionPlan::Columns {
                sources: vec![
                    ColumnSource::Static(vec!["name".to_string()]),
                    table_source("public", "orders"),
                ]
            }
        );
        // `*` of an earlier CTE takes that CTE's columns.
        assert_eq!(
            plan_at_marker(
                "WITH a AS (SELECT x FROM t), b AS (SELECT * FROM a) SELECT * FROM b WHERE b.|"
            ),
            static_columns_plan(&["x"])
        );
    }

    #[test]
    fn test_plan_cte_columns_skip_unnamed_expressions() {
        // Function calls are named after the function, as PostgreSQL does;
        // a bare literal has no usable name.
        assert_eq!(
            plan_at_marker(
                "WITH totals AS (SELECT user_id, count(*), sum(total) AS spent, 1 FROM orders GROUP BY user_id) \
                 SELECT * FROM totals t WHERE t.|"
            ),
            static_columns_plan(&["user_id", "count", "spent"])
        );
    }

    #[test]
    fn test_plan_inside_cte_body_uses_its_own_tables() {
        assert_eq!(
            plan_at_marker("WITH a AS (SELECT | FROM users u) SELECT * FROM orders o"),
            columns_plan("public", "users")
        );
        // The outer query does not see the tables inside a CTE body.
        assert_eq!(
            plan_at_marker("WITH a AS (SELECT x FROM t) SELECT * FROM a WHERE |"),
            static_columns_plan(&["x"])
        );
    }

    #[test]
    fn test_plan_correlated_subquery_sees_outer_tables() {
        assert_eq!(
            plan_at_marker(
                "SELECT * FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id AND |)"
            ),
            CompletionPlan::Columns {
                sources: vec![
                    table_source("public", "orders"),
                    table_source("public", "users"),
                ]
            }
        );
    }

    #[test]
    fn test_plan_columns_of_unaliased_tables() {
        assert_eq!(
            plan_at_marker("SELECT * FROM users WHERE |"),
            columns_plan("public", "users")
        );
        assert_eq!(
            plan_at_marker("UPDATE users SET |"),
            columns_plan("public", "users")
        );
        assert_eq!(
            plan_at_marker("DELETE FROM sales.orders WHERE |"),
            columns_plan("sales", "orders")
        );
        assert_eq!(
            plan_at_marker("SELECT * FROM users WHERE users.|"),
            columns_plan("public", "users")
        );
    }

    #[test]
    fn test_completion_context_lists_ctes_for_from() {
        let context =
            ctx_end("WITH recent AS (SELECT 1 AS x), paid AS (SELECT 2 AS y) SELECT * FROM ");
        assert_eq!(context.cte_names, vec!["recent", "paid"]);
        // The placeholder standing in for the word being typed never shows up.
        assert!(context.table_aliases.is_empty());
        assert_eq!(
            plan_completion(&context, "public", &schemas(&["public"]), true),
            CompletionPlan::TablesAndSchemas
        );
    }

    #[test]
    fn test_plan_nothing_while_typing_a_number() {
        assert_eq!(
            plan_at_marker("SELECT * FROM users WHERE id = 12|"),
            CompletionPlan::Nothing
        );
        assert_eq!(
            plan_at_marker("SELECT * FROM users WHERE price > 1.5|"),
            CompletionPlan::Nothing
        );
        assert_eq!(
            plan_at_marker("SELECT * FROM users LIMIT 1|"),
            CompletionPlan::Nothing
        );
        assert_eq!(plan_at_marker("SELECT 1e|"), CompletionPlan::Nothing);
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
            derived_relations: Vec::new(),
            cte_names: Vec::new(),
            enclosing_call: None,
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
        let items = build_function_items(DatabaseType::PostgreSQL, &[], "split_", dummy_range());
        let split_part = items
            .iter()
            .find(|item| item.label == "split_part")
            .expect("split_part offered for postgres");
        assert_eq!(split_part.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(
            split_part.detail.as_deref(),
            Some("split_part(text, text, integer) -> text")
        );
        assert_eq!(split_part.insert_text.as_deref(), Some("split_part($0)"));
        assert_eq!(
            split_part.insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
        assert!(split_part.documentation.is_some());
        assert!(items.iter().all(|item| item.label.starts_with("split_")));
        assert!(
            items
                .iter()
                .all(|item| item.label.to_lowercase().starts_with("split_"))
        );
    }

    #[test]
    fn test_build_function_items_are_dialect_specific() {
        let mysql = build_function_items(DatabaseType::MySQL, &[], "substring", dummy_range());
        assert!(mysql.iter().any(|item| item.label == "SUBSTRING_INDEX"));
        assert!(mysql.iter().all(|item| item.label != "split_part"));
        let sqlite = build_function_items(DatabaseType::SQLite, &[], "split", dummy_range());
        assert!(sqlite.is_empty());
        let mssql = build_function_items(DatabaseType::MsSql, &[], "date", dummy_range());
        assert!(mssql.iter().any(|item| item.label == "DATEDIFF"));
        let clickhouse =
            build_function_items(DatabaseType::ClickHouse, &[], "splitBy", dummy_range());
        assert!(clickhouse.iter().any(|item| item.label == "splitByChar"));
    }

    #[test]
    fn test_build_function_items_empty_word_is_capped_and_groups_overloads() {
        let items = build_function_items(DatabaseType::PostgreSQL, &[], "", dummy_range());
        assert!(!items.is_empty());
        assert!(items.len() <= UNFILTERED_FUNCTION_LIMIT);
        let labels: std::collections::HashSet<&str> =
            items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels.len(), items.len(), "one item per function name");

        let round = build_function_items(DatabaseType::PostgreSQL, &[], "round", dummy_range());
        let round = round
            .iter()
            .find(|item| item.label == "round")
            .expect("round");
        assert!(round.detail.as_deref().unwrap_or("").contains("overloads"));
    }

    #[test]
    fn test_build_function_items_user_functions_rank_first() {
        let udf = FunctionSignatureInfo {
            schema: Some("public".into()),
            name: "split_words".into(),
            parameters: vec!["text text".into()],
            return_type: Some("text[]".into()),
            comment: Some("Splits on whitespace".into()),
        };
        let items = build_function_items(
            DatabaseType::PostgreSQL,
            std::slice::from_ref(&udf),
            "split",
            dummy_range(),
        );
        assert_eq!(items[0].label, "split_words");
        assert_eq!(
            items[0].detail.as_deref(),
            Some("split_words(text text) -> text[]")
        );
        assert!(items.iter().skip(1).any(|item| item.label == "split_part"));
    }

    fn labelled(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_rank_items_lists_names_before_functions_and_keywords() {
        let items = rank_items(
            vec![
                (ItemGroup::Keyword, vec![labelled("WHERE")]),
                (ItemGroup::Function, vec![labelled("whatever_fn")]),
                (ItemGroup::Schema, vec![labelled("warehouse")]),
                (
                    ItemGroup::Relation,
                    vec![labelled("wcte"), labelled("widgets")],
                ),
                (ItemGroup::Column, vec![labelled("weight")]),
                (ItemGroup::JoinCondition, vec![labelled("w.id = o.w_id")]),
            ],
            "w",
        );
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "w.id = o.w_id",
                "weight",
                "wcte",
                "widgets",
                "warehouse",
                "whatever_fn",
                "WHERE"
            ]
        );
        let sort_texts: Vec<&str> = items
            .iter()
            .filter_map(|item| item.sort_text.as_deref())
            .collect();
        assert_eq!(sort_texts.len(), items.len());
        assert!(sort_texts.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn test_keywords_after_names_only_for_a_typed_word() {
        assert!(keyword_items_after_names("", dummy_range(), &[]).is_empty());
        let keywords = keyword_items_after_names("wh", dummy_range(), &[]);
        assert_eq!(keywords.len(), 2);
        assert!(keywords.iter().any(|item| item.label == "WHERE"));
        // `count` is offered as a function, so the COUNT keyword is not.
        let functions = build_function_items(DatabaseType::PostgreSQL, &[], "cou", dummy_range());
        assert!(functions.iter().any(|item| item.label == "count"));
        assert!(keyword_items_after_names("cou", dummy_range(), &functions).is_empty());
    }

    #[test]
    fn test_build_cte_items_insert_alias_abbreviation() {
        let items = build_cte_items(
            &["recent_orders".to_string(), "paid".to_string()],
            "re",
            dummy_range(),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "recent_orders");
        assert_eq!(items[0].insert_text.as_deref(), Some("recent_orders ro"));
        assert_eq!(items[0].detail.as_deref(), Some("CTE"));
    }

    #[test]
    fn test_elide_middle_keeps_both_ends() {
        assert_eq!(elide_middle("orders", 10, 1), "orders");
        let name = "customer_subscription_billing_history_archive_2024_q3";
        let elided = elide_middle(name, 20, 1);
        assert_eq!(elided.chars().count(), 20);
        assert_eq!(elided, "customer_…ve_2024_q3");
        // The typed prefix stays ahead of the ellipsis.
        let elided = elide_middle(name, 20, 14);
        assert!(elided.starts_with("customer_subsc…"), "{elided}");
        assert_eq!(elided.chars().count(), 20);
        // Multibyte characters are cut on character boundaries.
        assert_eq!(elide_middle("åäöåäöåäöåäö", 5, 1), "åä…äö");
    }

    #[test]
    fn test_rank_items_elides_long_labels_but_inserts_the_full_name() {
        let name = "customer_subscription_billing_history_archive_2024_q3_rollup";
        let columns = vec![ColumnCandidate {
            name: name.to_string(),
            data_type: Some("text".to_string()),
        }];
        let items = rank_items(
            vec![(
                ItemGroup::Column,
                build_column_items(&columns, "cust", dummy_range(), "t"),
            )],
            "cust",
        );
        assert_eq!(items[0].label.chars().count(), MAX_LABEL_CHARS);
        assert!(items[0].label.starts_with("cust"));
        assert!(items[0].label.ends_with("q3_rollup"));
        assert_eq!(items[0].insert_text.as_deref(), Some(name));
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
