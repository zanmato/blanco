//! SQL Auto-completion System
//!
//! This module provides intelligent SQL auto-completion that works with both SQLite and PostgreSQL
//! through the unified connection trait. It includes table completion, column completion, and
//! context-aware suggestions based on the current query position.

use blanco_core::{Connection, ColumnInfo, TableMetadata};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

/// Completion item types for SQL suggestions
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionItemKind {
    Table,
    Column,
    Schema,
    Keyword,
    Function,
    Alias,
}

/// A single completion item that can be suggested to the user
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    /// The label shown to the user
    pub label: String,
    /// The kind of completion item
    pub kind: CompletionItemKind,
    /// Detailed information about the item (shown in hover)
    pub detail: Option<String>,
    /// The text that will be inserted
    pub insert_text: String,
    /// Additional text to insert (for example, table alias)
    pub additional_text: Option<String>,
    /// The range of text that this completion will replace
    pub replace_range: Option<CompletionRange>,
    /// Priority for sorting (higher = more relevant)
    pub priority: i32,
}

impl CompletionItem {
    pub fn table(name: String, detail: Option<String>) -> Self {
        Self {
            label: name.clone(),
            kind: CompletionItemKind::Table,
            detail,
            insert_text: name,
            additional_text: None,
            replace_range: None,
            priority: 100,
        }
    }

    pub fn table_with_alias(name: String, alias: String, detail: Option<String>) -> Self {
        Self {
            label: alias.clone(),
            kind: CompletionItemKind::Table,
            detail,
            insert_text: format!("{} {}", name, alias),
            additional_text: Some(alias),
            replace_range: None,
            priority: 110,
        }
    }

    pub fn column(name: String, table_name: Option<String>, detail: Option<String>) -> Self {
        Self {
            label: name.clone(),
            kind: CompletionItemKind::Column,
            detail,
            insert_text: name,
            additional_text: table_name,
            replace_range: None,
            priority: 90,
        }
    }

    pub fn schema(name: String) -> Self {
        Self {
            label: name.clone(),
            kind: CompletionItemKind::Schema,
            detail: Some("Schema".to_string()),
            insert_text: name,
            additional_text: None,
            replace_range: None,
            priority: 80,
        }
    }

    pub fn keyword(name: String) -> Self {
        Self {
            label: name.clone(),
            kind: CompletionItemKind::Keyword,
            detail: Some("SQL Keyword".to_string()),
            insert_text: name,
            additional_text: None,
            replace_range: None,
            priority: 50,
        }
    }

    pub fn function(name: String) -> Self {
        Self {
            label: name.clone(),
            kind: CompletionItemKind::Function,
            detail: Some("SQL Function".to_string()),
            insert_text: format!("{}()", name),
            additional_text: None,
            replace_range: None,
            priority: 60,
        }
    }

    pub fn alias(name: String) -> Self {
        Self {
            label: name.clone(),
            kind: CompletionItemKind::Alias,
            detail: Some("Table Alias".to_string()),
            insert_text: name,
            additional_text: None,
            replace_range: None,
            priority: 70,
        }
    }
}

/// Represents a range in the text that should be replaced by a completion
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRange {
    pub start: Position,
    pub end: Position,
}

/// Position in a text document (0-indexed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// Context for completion requests
#[derive(Debug, Clone)]
pub struct CompletionContext {
    /// The full text of the document
    pub text: String,
    /// The current cursor position
    pub position: Position,
    /// The character directly before the cursor (for trigger detection)
    pub trigger_character: Option<char>,
    /// The current word being typed
    pub current_word: Option<String>,
}

/// The result of a completion request
#[derive(Debug, Clone)]
pub struct CompletionResult {
    /// List of completion items
    pub items: Vec<CompletionItem>,
    /// Whether the completion list is incomplete (more items could be available)
    pub is_incomplete: bool,
}

/// SQL completion context detection
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionKind {
    /// Table name completion (after FROM, JOIN, INTO, UPDATE)
    Table,
    /// Column name completion (after SELECT, WHERE, ORDER BY, GROUP BY, HAVING)
    Column,
    /// Column name completion with table qualification (table.column)
    QualifiedColumn,
    /// Schema name completion
    Schema,
    /// Alias name completion (after AS)
    Alias,
    /// General keyword completion
    Keyword,
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
    /// Available schemas for completion
    pub available_schemas: Vec<String>,
}

/// Hover information for SQL elements
#[derive(Debug, Clone)]
pub struct HoverInfo {
    /// The content to show in the hover tooltip
    pub content: String,
    /// The range that the hover applies to
    pub range: CompletionRange,
    /// The type of content (markdown, plain text, etc.)
    pub content_type: HoverContentType,
}

#[derive(Debug, Clone)]
pub enum HoverContentType {
    PlainText,
    Markdown,
}

impl HoverInfo {
    pub fn column(column: &ColumnInfo, table_name: &str) -> Self {
        let mut content = format!("**{}**\n\n", column.name);
        content.push_str(&format!("**Type:** `{}`\n", column.data_type));

        if column.is_primary_key {
            content.push_str("**Primary Key** ✅\n");
        }

        if !column.is_nullable {
            content.push_str("**NOT NULL** ❌\n");
        } else {
            content.push_str("**Nullable** ✅\n");
        }

        if let Some(ref default) = column.default_value {
            content.push_str(&format!("**Default:** `{}`\n", default));
        }

        if let Some(max_len) = column.character_maximum_length {
            content.push_str(&format!("**Max Length:** {}\n", max_len));
        }

        content.push_str(&format!("\n**Table:** `{}`", table_name));

        Self {
            content,
            range: CompletionRange {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
            content_type: HoverContentType::Markdown,
        }
    }

    pub fn table(metadata: &TableMetadata) -> Self {
        let mut content = format!("**{}**\n\n", metadata.name);

        if let Some(ref schema) = metadata.schema {
            content.push_str(&format!("**Schema:** `{}`\n", schema));
        }

        content.push_str(&format!("**Columns:** {}\n", metadata.columns.len()));

        if let Some(row_count) = metadata.row_count {
            content.push_str(&format!("**Rows:** {}\n", row_count));
        }

        if !metadata.primary_keys.is_empty() {
            let pk_list = metadata.primary_keys.join(", ");
            content.push_str(&format!("**Primary Keys:** `{}`\n", pk_list));
        }

        content.push_str("\n**Columns:**\n");
        for column in &metadata.columns {
            let pk_indicator = if column.is_primary_key { " 🔑" } else { "" };
            let nullable_indicator = if column.is_nullable { " ✅" } else { " ❌" };
            content.push_str(&format!(
                "- `{}` `{}`{}{}\n",
                column.name, column.data_type, pk_indicator, nullable_indicator
            ));
        }

        Self {
            content,
            range: CompletionRange {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
            content_type: HoverContentType::Markdown,
        }
    }
}

/// Cache for metadata to avoid repeated database queries
#[derive(Debug)]
pub struct MetadataCache {
    /// Cached table metadata
    table_metadata: HashMap<String, Arc<TableMetadata>>,
    /// Cached column lists
    table_columns: HashMap<String, Arc<Vec<ColumnInfo>>>,
    /// Cached table lists
    schema_tables: HashMap<String, Arc<Vec<String>>>,
}

impl MetadataCache {
    pub fn new() -> Self {
        Self {
            table_metadata: HashMap::new(),
            table_columns: HashMap::new(),
            schema_tables: HashMap::new(),
        }
    }

    pub fn get_table_metadata(&self, key: &str) -> Option<Arc<TableMetadata>> {
        self.table_metadata.get(key).cloned()
    }

    pub fn set_table_metadata(&mut self, key: String, metadata: TableMetadata) {
        self.table_metadata.insert(key, Arc::new(metadata));
    }

    pub fn get_table_columns(&self, key: &str) -> Option<Arc<Vec<ColumnInfo>>> {
        self.table_columns.get(key).cloned()
    }

    pub fn set_table_columns(&mut self, key: String, columns: Vec<ColumnInfo>) {
        self.table_columns.insert(key, Arc::new(columns));
    }

    pub fn get_schema_tables(&self, key: &str) -> Option<Arc<Vec<String>>> {
        self.schema_tables.get(key).cloned()
    }

    pub fn set_schema_tables(&mut self, key: String, tables: Vec<String>) {
        self.schema_tables.insert(key, Arc::new(tables));
    }

    pub fn clear(&mut self) {
        self.table_metadata.clear();
        self.table_columns.clear();
        self.schema_tables.clear();
    }
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self::new()
    }
}

/// SQL keywords that can trigger completion
pub static SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER",
    "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "OUTER", "ON", "AS", "GROUP", "BY",
    "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION", "ALL", "DISTINCT", "INTO", "VALUES",
    "SET", "TABLE", "INDEX", "VIEW", "DATABASE", "SCHEMA", "PRIMARY", "KEY", "FOREIGN",
    "REFERENCES", "NOT", "NULL", "DEFAULT", "UNIQUE", "CHECK", "CONSTRAINT", "CASCADE",
    "RESTRICT", "AND", "OR", "NOT", "IN", "EXISTS", "BETWEEN", "LIKE", "ILIKE", "IS",
    "CASE", "WHEN", "THEN", "ELSE", "END", "IF", "COALESCE", "NULLIF", "CAST", "CONVERT",
];

/// Common SQL functions
pub static SQL_FUNCTIONS: &[&str] = &[
    "COUNT", "SUM", "AVG", "MIN", "MAX", "FIRST", "LAST",
    "UPPER", "LOWER", "LENGTH", "TRIM", "LTRIM", "RTRIM", "SUBSTRING", "CONCAT",
    "ROUND", "CEIL", "FLOOR", "ABS", "SQRT", "POWER", "MOD",
    "NOW", "CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP",
    "EXTRACT", "DATE_PART", "DATE_TRUNC",
    "COALESCE", "NULLIF", "ISNULL", "IFNULL",
    "CAST", "CONVERT",
];

/// Characters that can trigger auto-completion
pub static TRIGGER_CHARACTERS: &[char] = &[' ', '.', ',', '(', ')', '[', ']', '"', '\'', '`'];

/// SQL completion engine
pub struct SqlCompletionEngine {
    /// Connection to the database
    connection: Arc<dyn Connection>,
    /// Metadata cache
    cache: Arc<std::sync::Mutex<MetadataCache>>,
}

impl SqlCompletionEngine {
    pub fn new(connection: Arc<dyn Connection>) -> Self {
        Self {
            connection,
            cache: Arc::new(std::sync::Mutex::new(MetadataCache::new())),
        }
    }

    /// Get completion suggestions for the given context
    pub async fn get_completions(&self, context: CompletionContext) -> Result<CompletionResult> {
        // Parse the query to understand the context
        let parsed = self.parse_query_context(&context).await?;

        let mut items = Vec::new();

        match parsed.completion_context {
            CompletionKind::Table => {
                items.extend(self.get_table_completions(&parsed).await?);
            }
            CompletionKind::Column | CompletionKind::QualifiedColumn => {
                items.extend(self.get_column_completions(&parsed).await?);
            }
            CompletionKind::Schema => {
                items.extend(self.get_schema_completions().await?);
            }
            CompletionKind::Alias => {
                items.extend(self.get_alias_completions(&parsed).await?);
            }
            CompletionKind::Keyword | CompletionKind::Unknown => {
                items.extend(self.get_keyword_completions());
                items.extend(self.get_function_completions());
            }
        }

        // Filter by current word if present
        if let Some(ref word) = parsed.current_word {
            items.retain(|item| {
                item.label.to_lowercase().starts_with(&word.to_lowercase())
            });
        }

        // Sort by priority (descending) and then alphabetically
        items.sort_by(|a, b| {
            b.priority.cmp(&a.priority)
                .then_with(|| a.label.cmp(&b.label))
        });

        Ok(CompletionResult {
            items,
            is_incomplete: false,
        })
    }

    /// Get hover information for the given position
    pub async fn get_hover(&self, context: CompletionContext) -> Result<Option<HoverInfo>> {
        let parsed = self.parse_query_context(&context).await?;

        // This is a simplified implementation - in a real scenario we'd need
        // to identify what token is at the cursor position
        if let Some(ref word) = parsed.current_word {
            // Check if it's a column name
            for table_name in &parsed.tables {
                let cache_key = format!("{}:{}",
                    parsed.available_schemas.first().unwrap_or(&"public".to_string()),
                    table_name
                );

                if let Some(metadata) = self.get_cached_table_metadata(&cache_key).await? {
                    if let Some(column) = metadata.get_column(word) {
                        return Ok(Some(HoverInfo::column(column, table_name)));
                    }
                }
            }

            // Check if it's a table name
            for table_name in &parsed.tables {
                if table_name.eq_ignore_ascii_case(word) {
                    let cache_key = format!("{}:{}",
                        parsed.available_schemas.first().unwrap_or(&"public".to_string()),
                        table_name
                    );

                    if let Some(metadata) = self.get_cached_table_metadata(&cache_key).await? {
                        return Ok(Some(HoverInfo::table(&*metadata)));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Parse the query to understand completion context
    async fn parse_query_context(&self, context: &CompletionContext) -> Result<ParsedQuery> {
        // This is a simplified implementation - we'll enhance this later
        // with proper SQL parsing using the existing sql-parse crate

        let lines: Vec<&str> = context.text.lines().collect();
        let current_line = if (context.position.line as usize) < lines.len() {
            lines[context.position.line as usize]
        } else {
            ""
        };

        // Extract the current word
        let current_word = self.extract_current_word(current_line, context.position.character as usize);

        // Detect completion context based on surrounding text
        let completion_context = self.detect_completion_context(current_line, context.position.character as usize);

        // Get available schemas
        let available_schemas = self.connection.get_schemas().await.unwrap_or_else(|_| vec!["public".to_string()]);

        Ok(ParsedQuery {
            statement_type: "SELECT".to_string(), // Simplified
            tables: Vec::new(), // Will be populated by proper parsing
            aliases: Vec::new(),
            completion_context,
            current_word,
            available_schemas,
        })
    }

    /// Extract the current word at the cursor position
    fn extract_current_word(&self, line: &str, cursor_pos: usize) -> Option<String> {
        if cursor_pos >= line.len() {
            return None;
        }

        let start = line[..cursor_pos]
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);

        let end = line[cursor_pos..]
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| cursor_pos + i)
            .unwrap_or(line.len());

        if start < end && start <= cursor_pos && cursor_pos <= end {
            Some(line[start..end].to_string())
        } else {
            None
        }
    }

    /// Detect the completion context based on the surrounding text
    fn detect_completion_context(&self, line: &str, cursor_pos: usize) -> CompletionKind {
        let before_cursor = &line[..cursor_pos];
        let before_cursor_lower = before_cursor.to_lowercase();

        // Check for qualified column completion (table.column)
        if before_cursor.contains('.') {
            if let Some(dot_pos) = before_cursor.rfind('.') {
                let before_dot = &before_cursor[..dot_pos];
                if before_dot.trim_end().split_whitespace().last().map_or(false, |token| {
                    !["FROM", "JOIN", "INTO", "UPDATE"].contains(&token.to_uppercase().as_str())
                }) {
                    return CompletionKind::QualifiedColumn;
                }
            }
        }

        // Check for table completion contexts
        let tokens: Vec<&str> = before_cursor_lower.split_whitespace().collect();
        if let Some(last_token) = tokens.last() {
            match *last_token {
                "from" | "join" | "into" | "update" => return CompletionKind::Table,
                "as" => return CompletionKind::Alias,
                "select" | "where" | "order" | "group" | "having" => return CompletionKind::Column,
                _ => {}
            }
        }

        // Check for "order by" and "group by"
        if tokens.len() >= 2 {
            let last_two = &tokens[tokens.len()-2..];
            if (last_two[0] == "order" && last_two[1] == "by") ||
               (last_two[0] == "group" && last_two[1] == "by") {
                return CompletionKind::Column;
            }
        }

        // Default to keyword completion
        CompletionKind::Keyword
    }

    /// Get table name completions
    async fn get_table_completions(&self, parsed: &ParsedQuery) -> Result<Vec<CompletionItem>> {
        let mut items = Vec::new();

        for schema in &parsed.available_schemas {
            let cache_key = format!("{}:*", schema);

            let tables = if let Some(cached_tables) = {
                let cache = self.cache.lock().unwrap();
                cache.get_schema_tables(&cache_key)
            } {
                cached_tables.clone()
            } else {
                let tables = self.connection.get_tables(Some(schema)).await?;
                let mut cache = self.cache.lock().unwrap();
                cache.set_schema_tables(cache_key, tables.clone());
                Arc::new(tables)
            };

            for table_name in &*tables {
                let detail = Some(format!("Table in schema {}", schema));
                items.push(CompletionItem::table(table_name.clone(), detail));
            }
        }

        Ok(items)
    }

    /// Get column name completions
    async fn get_column_completions(&self, parsed: &ParsedQuery) -> Result<Vec<CompletionItem>> {
        let mut items = Vec::new();

        for table_name in &parsed.tables {
            let cache_key = format!("{}:{}",
                parsed.available_schemas.first().unwrap_or(&"public".to_string()),
                table_name
            );

            let columns = if let Some(cached_columns) = {
                let cache = self.cache.lock().unwrap();
                cache.get_table_columns(&cache_key)
            } {
                cached_columns.clone()
            } else {
                let columns = self.connection.get_columns_for_table(
                    table_name,
                    parsed.available_schemas.first().map(|s| s.as_str())
                ).await?;
                let mut cache = self.cache.lock().unwrap();
                cache.set_table_columns(cache_key, columns.clone());
                Arc::new(columns)
            };

            for column in &*columns {
                let detail = Some(format!("{} ({})", column.name, column.data_type));
                items.push(CompletionItem::column(
                    column.name.clone(),
                    Some(table_name.clone()),
                    detail
                ));
            }
        }

        Ok(items)
    }

    /// Get schema name completions
    async fn get_schema_completions(&self) -> Result<Vec<CompletionItem>> {
        let schemas = self.connection.get_schemas().await?;
        Ok(schemas.into_iter()
            .map(CompletionItem::schema)
            .collect())
    }

    /// Get alias completions
    async fn get_alias_completions(&self, parsed: &ParsedQuery) -> Result<Vec<CompletionItem>> {
        // Generate reasonable aliases based on table names
        let mut items = Vec::new();

        for table_name in &parsed.tables {
            // Generate common alias patterns
            if table_name.len() >= 1 {
                let single_char = table_name.chars().next().unwrap().to_string();
                items.push(CompletionItem::alias(single_char));
            }

            if table_name.len() >= 3 {
                let three_char = table_name.chars().take(3).collect::<String>();
                items.push(CompletionItem::alias(three_char));
            }

            // Snake_case to camelCase
            let camel_case = table_name.split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect::<String>();

            if !camel_case.is_empty() && camel_case != *table_name {
                items.push(CompletionItem::alias(camel_case));
            }
        }

        Ok(items)
    }

    /// Get keyword completions
    fn get_keyword_completions(&self) -> Vec<CompletionItem> {
        SQL_KEYWORDS.iter()
            .map(|&keyword| CompletionItem::keyword(keyword.to_string()))
            .collect()
    }

    /// Get function completions
    fn get_function_completions(&self) -> Vec<CompletionItem> {
        SQL_FUNCTIONS.iter()
            .map(|&function| CompletionItem::function(function.to_string()))
            .collect()
    }

    /// Get cached table metadata, loading it if necessary
    async fn get_cached_table_metadata(&self, key: &str) -> Result<Option<Arc<TableMetadata>>> {
        // First check cache
        {
            let cache = self.cache.lock().unwrap();
            if let Some(metadata) = cache.get_table_metadata(key) {
                return Ok(Some(metadata));
            }
        }

        // Parse key to get schema and table name
        let mut parts = key.split(':');
        let schema = parts.next().unwrap_or("public");
        let table_name = parts.next().unwrap_or("");

        if !table_name.is_empty() {
            // Load from database
            match self.connection.get_table_metadata(
                table_name,
                if schema == "public" { None } else { Some(schema) }
            ).await {
                Ok(metadata) => {
                    let arc_metadata = Arc::new(metadata);
                    let mut cache = self.cache.lock().unwrap();
                    cache.set_table_metadata(key.to_string(), (*arc_metadata).clone());
                    return Ok(Some(arc_metadata));
                }
                Err(_) => {
                    // Table might not exist or we don't have permission
                    return Ok(None);
                }
            }
        }

        Ok(None)
    }

    /// Clear the metadata cache
    pub fn clear_cache(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.clear();
    }
}