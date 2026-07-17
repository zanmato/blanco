use database::ParamStyles;
use once_cell::sync::Lazy;
use ropey::Rope;
use std::cell::RefCell;
use std::ops::Range;
use tree_sitter::{Language, Node, Parser};

/// Parameter style in SQL query
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterStyle {
    Positional(usize), // $1, $2, etc.
    Named(String),     // :name, @name, etc.
}

/// A parameter found in a SQL query
#[derive(Clone, Debug)]
pub struct QueryParameter {
    pub style: ParameterStyle,
    pub raw_text: String,   // Original text from query (e.g., "$1", ":user_id")
    pub byte_offset: usize, // Byte offset of the parameter in the original query
}

/// Information about an extracted SQL statement
#[derive(Debug, Clone)]
pub struct StatementInfo {
    pub text: String,
    pub byte_range: Range<usize>,
    pub parameters: Vec<QueryParameter>,
}

/// SQL statement parser using tree-sitter for accurate statement extraction
struct SqlStatementParser {
    parser: Parser,
}

static SQL_LANGUAGE: Lazy<Language> = Lazy::new(|| tree_sitter_sequel::LANGUAGE.into());

// Thread-local parser instance for reuse.
// Each thread gets its own parser, avoiding contention and ensuring thread safety.
thread_local! {
    static TLS_PARSER: RefCell<SqlStatementParser> = {
        match SqlStatementParser::new() {
            Ok(parser) => RefCell::new(parser),
            Err(e) => {
                tracing::error!("Failed to create thread-local SQL parser: {}", e);
                // Panic in the unlikely case the parser cannot be created.
                // This is acceptable because a parser failure is a fatal error.
                panic!("Failed to initialize thread-local SQL parser: {}", e);
            }
        }
    };
}

impl SqlStatementParser {
    /// Create a new SQL statement parser
    fn new() -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&SQL_LANGUAGE)
            .map_err(|e| format!("Failed to set SQL language: {}", e))?;

        Ok(Self { parser })
    }

    /// Extract the SQL statement at the given cursor byte position
    pub fn extract_statement_at_cursor(
        &mut self,
        text: &Rope,
        cursor_byte_pos: usize,
        styles: ParamStyles,
    ) -> Option<StatementInfo> {
        // Convert Rope to string. as_str() only works if the rope is contiguous,
        // so we need to iterate over chunks for non-contiguous ropes.
        let text_str: String = text.chunks().collect();

        let tree = self.parser.parse(&text_str, None)?;
        let root_node = tree.root_node();

        // Find all statement nodes
        let mut statements = Vec::new();
        self.collect_statements(root_node, &mut statements);

        // Find the statement that contains or is closest to the cursor
        let mut best_statement = None;
        let mut best_distance = usize::MAX;

        for statement in statements {
            let range = statement.byte_range();

            // If cursor is inside the statement, return it immediately
            if range.contains(&cursor_byte_pos) {
                let statement_text = text.slice(range.clone()).to_string();
                // Extract parameters and adjust their offsets to be relative to the statement text
                let mut parameters =
                    self.extract_parameters_from_node(statement, &text_str, styles);
                // Adjust byte offsets to be relative to the statement text (not the full text)
                for param in &mut parameters {
                    param.byte_offset = param.byte_offset.saturating_sub(range.start);
                }
                return Some(StatementInfo {
                    text: statement_text,
                    byte_range: range,
                    parameters,
                });
            }

            // Calculate distance from cursor to this statement
            let distance = if cursor_byte_pos < range.start {
                range.start - cursor_byte_pos
            } else {
                cursor_byte_pos - range.end
            };

            // Keep track of the closest statement
            if distance < best_distance {
                best_distance = distance;
                best_statement = Some(statement);
            }
        }

        // Return the closest statement (if any)
        if let Some(statement) = best_statement {
            let range = statement.byte_range();
            let statement_text = text.slice(range.clone()).to_string();
            // Extract parameters and adjust their offsets to be relative to the statement text
            let mut parameters = self.extract_parameters_from_node(statement, &text_str, styles);
            // Adjust byte offsets to be relative to the statement text (not the full text)
            for param in &mut parameters {
                param.byte_offset = param.byte_offset.saturating_sub(range.start);
            }
            Some(StatementInfo {
                text: statement_text,
                byte_range: range,
                parameters,
            })
        } else {
            None
        }
    }

    /// Collect all statement nodes from the tree
    fn collect_statements<'a>(&self, node: Node<'a>, statements: &mut Vec<Node<'a>>) {
        // Check if this node is a statement
        if self.is_statement_node(node.kind()) {
            statements.push(node);
        }

        // Recursively check children
        for i in 0.. {
            if let Some(child) = node.child(i) {
                self.collect_statements(child, statements);
            } else {
                break;
            }
        }
    }

    /// Check if a node kind represents a SQL statement
    fn is_statement_node(&self, kind: &str) -> bool {
        matches!(
            kind,
            "statement"
                | "select_statement"
                | "insert_statement"
                | "update_statement"
                | "delete_statement"
                | "create_statement"
                | "drop_statement"
                | "alter_statement"
                | "truncate_statement"
                | "merge_statement"
                | "transaction_statement"
                | "compound_statement"
        )
    }

    /// Extract parameters from a statement node
    fn extract_parameters_from_node(
        &self,
        node: tree_sitter::Node,
        text: &str,
        styles: ParamStyles,
    ) -> Vec<QueryParameter> {
        let mut parameters = Vec::new();
        self.collect_parameters(node, text, styles, &mut parameters);

        // Sort by byte offset ascending for user display (in order of appearance)
        parameters.sort_by_key(|p| p.byte_offset);

        // Assign sequential positional indices to bare `?` parameters (Positional(0)).
        // Each `?` should get a unique index ($1, $2, ...) so the parameter form
        // creates a separate input for each one.
        let mut question_mark_index = 1usize;
        for param in &mut parameters {
            if param.style == ParameterStyle::Positional(0) {
                param.style = ParameterStyle::Positional(question_mark_index);
                question_mark_index += 1;
            }
        }

        parameters
    }

    /// Recursively collect parameters from the AST
    fn collect_parameters<'a>(
        &self,
        node: tree_sitter::Node<'a>,
        text: &str,
        styles: ParamStyles,
        parameters: &mut Vec<QueryParameter>,
    ) {
        // Check if this node represents a parameter
        if let Some(param) = self.try_parse_parameter(node, text, styles) {
            // Deduplicate by byte offset, each position can only have one parameter
            let already_seen = parameters
                .iter()
                .any(|p| p.byte_offset == param.byte_offset);

            if !already_seen {
                parameters.push(param);
            }
        }

        // Recursively check children
        for i in 0.. {
            if let Some(child) = node.child(i) {
                self.collect_parameters(child, text, styles, parameters);
            } else {
                break;
            }
        }
    }

    /// Try to parse a node as a parameter
    fn try_parse_parameter(
        &self,
        node: tree_sitter::Node,
        text: &str,
        styles: ParamStyles,
    ) -> Option<QueryParameter> {
        let kind = node.kind();
        let byte_range = node.byte_range();

        // Clamp the byte range to the text length to avoid panic
        // This handles edge cases where tree-sitter returns ranges extending beyond text
        let end = byte_range.end.min(text.len());
        let start = byte_range.start.min(end);
        let node_text = &text[start..end];

        // Check for tree-sitter parameter node types
        if kind == "positional_parameter" || kind == "bind_parameter" {
            return self.parse_parameter_text(node_text, start, styles);
        }

        // Fallback: check for dollar-number pattern or named parameter patterns in text
        if let Some(param) = self.parse_parameter_text(node_text, start, styles) {
            return Some(param);
        }

        None
    }

    /// Parse parameter text into a QueryParameter, honoring the dialect's
    /// enabled placeholder styles. A token like `?` is a bind parameter in
    /// MySQL/SQLite but a jsonb operator in Postgres, so styles the dialect does
    /// not use are rejected here rather than prompting the user for a value.
    fn parse_parameter_text(
        &self,
        text: &str,
        byte_offset: usize,
        styles: ParamStyles,
    ) -> Option<QueryParameter> {
        let trimmed = text.trim();

        // Standalone ? parameter (JDBC style)
        if trimmed == "?" {
            if !styles.question_mark {
                return None;
            }
            return Some(QueryParameter {
                style: ParameterStyle::Positional(0),
                raw_text: trimmed.to_string(),
                byte_offset,
            });
        }

        // Positional parameter: $1, $2, etc.
        if let Some(rest) = trimmed.strip_prefix('$')
            && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            && let Ok(index) = rest.parse::<usize>()
        {
            if !styles.dollar_number {
                return None;
            }
            return Some(QueryParameter {
                style: ParameterStyle::Positional(index),
                raw_text: trimmed.to_string(),
                byte_offset,
            });
        }

        // Named parameter: :name, @name, etc. (?name is also supported as named)
        if let Some(prefix) = trimmed.chars().next()
            && (prefix == ':' || prefix == '@' || prefix == '?')
        {
            let allowed = match prefix {
                ':' => styles.colon_named,
                '@' => styles.at_named,
                '?' => styles.question_mark,
                _ => false,
            };
            if !allowed {
                return None;
            }
            let name = trimmed[1..].to_string();
            if !name.is_empty()
                && name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_')
            {
                return Some(QueryParameter {
                    style: ParameterStyle::Named(name),
                    raw_text: trimmed.to_string(),
                    byte_offset,
                });
            }
        }

        None
    }
}

/// Extract statement info at cursor position using thread-local parser.
///
/// Recognizes every placeholder style. Use [`extract_statement_info_with_styles`]
/// when the active dialect is known, so dialect operators (e.g. Postgres jsonb
/// `?`) are not mistaken for bind parameters.
///
/// # Arguments
/// * `text` - Full SQL text
/// * `cursor_pos` - Cursor position in characters (not bytes)
///
/// # Returns
/// The statement info, or None if no statement found
pub fn extract_statement_info(text: &Rope, cursor_pos: usize) -> Option<StatementInfo> {
    extract_statement_info_with_styles(text, cursor_pos, ParamStyles::all())
}

/// Extract statement info at cursor position, recognizing only the placeholder
/// styles enabled in `styles`.
pub fn extract_statement_info_with_styles(
    text: &Rope,
    cursor_pos: usize,
    styles: ParamStyles,
) -> Option<StatementInfo> {
    TLS_PARSER
        .with_borrow_mut(|parser| parser.extract_statement_at_cursor(text, cursor_pos, styles))
}

/// Extract the single line-oriented command at the cursor for non-SQL backends
/// (e.g. Redis), where statements are one-per-line rather than `;`-delimited and
/// there is no grammar to parse. Returns the trimmed text of the line containing
/// `cursor_byte_pos`, or `None` if that line is blank.
pub fn extract_command_at_cursor(text: &str, cursor_byte_pos: usize) -> Option<String> {
    let cursor = cursor_byte_pos.min(text.len());

    // Walk line spans (including their trailing newline) until we find the one
    // that contains the cursor. A cursor sitting exactly on a newline belongs to
    // the line it terminates.
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let line_end = line_start + line.len();
        if cursor < line_end || line_end == text.len() {
            let trimmed = line.trim();
            return if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
        line_start = line_end;
    }
    None
}

/// The SQL clause the cursor is currently in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlClause {
    Select,
    From,
    Where,
    Join,
    Set,
    OrderBy,
    GroupBy,
    Having,
    Insert,
    Update,
    Delete,
    Values,
    On,
}

/// Table alias information extracted from the AST
#[derive(Debug, Clone)]
pub struct TableAlias {
    pub table_name: String,
    pub alias: String,
}

/// Completion context extracted from tree-sitter AST
#[derive(Debug, Clone)]
pub struct CompletionContext {
    pub current_word: String,
    pub clause: Option<SqlClause>,
    pub table_aliases: Vec<TableAlias>,
    pub is_dot_notation: bool,
    pub dot_table_name: Option<String>,
    /// Columns of CTEs and derived tables (subqueries) referenced in the
    /// statement's FROM clauses, keyed by the name used to reference them (the
    /// CTE name, or the relation alias). Ordered by appearance. Only populated
    /// when the projection is statically determinable (no `*`).
    pub cte_columns: Vec<(String, Vec<String>)>,
}

/// Extract completion context at cursor position using tree-sitter.
/// This replaces the hand-rolled parser in `completion/context.rs` with
/// an AST-based approach that correctly handles strings, comments, and
/// nested queries.
pub fn extract_completion_context(
    text: &Rope,
    cursor_byte_pos: usize,
) -> Option<CompletionContext> {
    TLS_PARSER.with_borrow_mut(|parser| {
        let text_str: String = text.chunks().collect();
        let tree = parser.parser.parse(&text_str, None)?;
        let root_node = tree.root_node();

        // Find the statement containing the cursor
        let mut statements = Vec::new();
        parser.collect_statements(root_node, &mut statements);

        let statement_node = find_statement_at_cursor(&statements, cursor_byte_pos, root_node);

        let statement_range = if let Some(node) = statement_node {
            node.byte_range()
        } else {
            0..text_str.len()
        };

        // Extract table aliases from all `relation` nodes within the statement
        let search_root = statement_node.unwrap_or(root_node);
        let table_aliases = extract_table_aliases_from_ast(search_root, &text_str);

        // Detect dot notation by checking if cursor is right after or within "identifier."
        let (is_dot_notation, dot_table_name) = detect_dot_notation(&text_str, cursor_byte_pos);

        // Extract current word by scanning backwards from cursor
        let current_word = extract_current_word_from_text(&text_str, cursor_byte_pos);

        // Determine clause. Strategies in priority order:
        // 1. Check ERROR nodes near cursor for trailing keywords (incomplete SQL like
        //    "SELECT * FROM users WHERE " where WHERE is in an ERROR node)
        // 2. Walk the AST within the statement (works for complete SQL and also
        //    when cursor is just past the statement end in the same clause)
        // 3. Keyword scan as a last resort (scoped to statement text to avoid
        //    picking up keywords from string literals in other statements)
        let clamped_pos = cursor_byte_pos.min(search_root.byte_range().end);
        let clause = find_clause_in_error_nodes(root_node, &text_str, cursor_byte_pos)
            .or_else(|| determine_clause(search_root, &text_str, clamped_pos))
            .or_else(|| {
                let scan_start = statement_range.start;
                let scan_end = cursor_byte_pos.min(text_str.len());
                if scan_end > scan_start {
                    determine_clause_from_keywords(
                        &text_str[scan_start..scan_end],
                        scan_end - scan_start,
                    )
                } else {
                    None
                }
            });

        let cte_columns = extract_derived_columns(search_root, &text_str);

        Some(CompletionContext {
            current_word,
            clause,
            table_aliases,
            is_dot_notation,
            dot_table_name,
            cte_columns,
        })
    })
}

/// Find the statement node containing the cursor, or the closest one
fn find_statement_at_cursor<'a>(
    statements: &[Node<'a>],
    cursor_byte_pos: usize,
    root_node: Node<'a>,
) -> Option<Node<'a>> {
    // First check if cursor is directly inside a statement
    for &statement in statements {
        let range = statement.byte_range();
        if range.start <= cursor_byte_pos && cursor_byte_pos <= range.end {
            return Some(statement);
        }
    }

    // Check ERROR nodes at the root level that might contain partial statements
    for i in 0..root_node.child_count() as u32 {
        if let Some(child) = root_node.child(i) {
            let range = child.byte_range();
            if range.start <= cursor_byte_pos && cursor_byte_pos <= range.end {
                // If the cursor is in an ERROR node, find the nearest statement before it
                if child.kind() == "ERROR" {
                    return statements
                        .iter()
                        .filter(|s| s.byte_range().end <= cursor_byte_pos)
                        .max_by_key(|s| s.byte_range().end)
                        .copied();
                }
            }
        }
    }

    // Find closest statement
    let mut best: Option<Node<'a>> = None;
    let mut best_distance = usize::MAX;
    for &statement in statements {
        let range = statement.byte_range();
        let distance = if cursor_byte_pos < range.start {
            range.start - cursor_byte_pos
        } else {
            cursor_byte_pos - range.end
        };
        if distance < best_distance {
            best_distance = distance;
            best = Some(statement);
        }
    }
    best
}

/// Extract table aliases from `relation` nodes in the AST.
/// A `relation` with an `object_reference` child and an `identifier` sibling = table with alias.
fn extract_table_aliases_from_ast(node: Node, source: &str) -> Vec<TableAlias> {
    let mut aliases = Vec::new();
    collect_table_aliases(node, source, &mut aliases);
    aliases
}

/// Text of a node.
fn node_text<'a>(node: Node, source: &'a str) -> &'a str {
    let range = node.byte_range();
    source.get(range).unwrap_or("")
}

/// First direct child of the given kind.
fn first_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    (0..node.child_count() as u32)
        .filter_map(|i| node.child(i))
        .find(|child| child.kind() == kind)
}

/// Whether `node` has any descendant matching `kind`.
fn has_descendant_of_kind(node: Node, kind: &str) -> bool {
    if node.kind() == kind {
        return true;
    }
    (0..node.child_count() as u32)
        .filter_map(|i| node.child(i))
        .any(|child| has_descendant_of_kind(child, kind))
}

/// The text of the last `identifier` descendant in document order. For a column
/// projection this is the column name (`a`), the qualified name's tail (`t.a` ->
/// `a`), or the alias (`x AS y` -> `y`).
fn last_identifier_text(node: Node, source: &str) -> Option<String> {
    let mut found: Option<String> = None;
    collect_last_identifier(node, source, &mut found);
    found
}

fn collect_last_identifier(node: Node, source: &str, found: &mut Option<String>) {
    if node.kind() == "identifier" {
        *found = Some(node_text(node, source).to_string());
    }
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_last_identifier(child, source, found);
        }
    }
}

/// Find the `select` clause node belonging directly to `node`'s query, without
/// descending into nested subqueries or CTEs.
fn find_select_clause<'a>(node: Node<'a>) -> Option<Node<'a>> {
    for i in 0..node.child_count() as u32 {
        let Some(child) = node.child(i) else { continue };
        match child.kind() {
            "select" => return Some(child),
            "subquery" | "cte" => continue,
            _ => {
                if let Some(found) = find_select_clause(child) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// The projected column names of the query rooted at `node` (a `cte` or
/// `subquery` node). Returns `None` if the projection is not statically
/// enumerable (e.g. it uses `*`), so callers can fall back to a database lookup.
fn projected_columns(node: Node, source: &str) -> Option<Vec<String>> {
    let select = find_select_clause(node)?;
    let select_expression = first_child_of_kind(select, "select_expression")?;

    let mut columns = Vec::new();
    for i in 0..select_expression.child_count() as u32 {
        let Some(term) = select_expression.child(i) else {
            continue;
        };
        if term.kind() != "term" {
            continue;
        }
        // `SELECT *` / `t.*` can't be enumerated without the source schema.
        if has_descendant_of_kind(term, "all_fields") {
            return None;
        }
        columns.push(last_identifier_text(term, source)?);
    }

    if columns.is_empty() {
        None
    } else {
        Some(columns)
    }
}

/// Extract the statically-known columns of CTEs and derived tables referenced in
/// `search_root`'s FROM clauses. See [`CompletionContext::cte_columns`].
fn extract_derived_columns(search_root: Node, source: &str) -> Vec<(String, Vec<String>)> {
    let mut cte_defs: Vec<(String, Vec<String>)> = Vec::new();
    collect_cte_defs(search_root, source, &mut cte_defs);

    let mut result: Vec<(String, Vec<String>)> = Vec::new();
    collect_derived_relations(search_root, source, &cte_defs, &mut result);
    result
}

/// Map CTE definition names to their projected columns.
fn collect_cte_defs(node: Node, source: &str, out: &mut Vec<(String, Vec<String>)>) {
    if node.kind() == "cte"
        && let Some(name_node) = first_child_of_kind(node, "identifier")
        && let Some(columns) = projected_columns(node, source)
    {
        out.push((node_text(name_node, source).to_string(), columns));
    }
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_cte_defs(child, source, out);
        }
    }
}

/// Walk `relation` nodes; for each derived table (subquery) or CTE reference,
/// record its reference name and columns.
fn collect_derived_relations(
    node: Node,
    source: &str,
    cte_defs: &[(String, Vec<String>)],
    out: &mut Vec<(String, Vec<String>)>,
) {
    if node.kind() == "relation" {
        if let Some(subquery) = first_child_of_kind(node, "subquery") {
            // `FROM (SELECT ...) alias` -> columns keyed by the alias.
            if let (Some(alias), Some(columns)) = (
                first_child_of_kind(node, "identifier"),
                projected_columns(subquery, source),
            ) {
                push_named(out, node_text(alias, source).to_string(), columns);
            }
        } else if let Some(object_reference) = first_child_of_kind(node, "object_reference") {
            // `FROM cte [alias]` -> columns of the matching CTE, keyed by the
            // alias if present, else the CTE name.
            let table_name = node_text(object_reference, source);
            if let Some((_, columns)) = cte_defs.iter().find(|(name, _)| name == table_name) {
                let key = first_child_of_kind(node, "identifier")
                    .map(|id| node_text(id, source).to_string())
                    .unwrap_or_else(|| table_name.to_string());
                push_named(out, key, columns.clone());
            }
        }
    }
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_derived_relations(child, source, cte_defs, out);
        }
    }
}

fn push_named(out: &mut Vec<(String, Vec<String>)>, name: String, columns: Vec<String>) {
    if !out.iter().any(|(existing, _)| *existing == name) {
        out.push((name, columns));
    }
}

/// Normalize a possibly-quoted, possibly schema-qualified object reference by
/// quote-stripping each dot-separated part (`[dbo].[Order]` -> `dbo.Order`,
/// `"public"."Users"` -> `public.Users`) so it matches the cache's unquoted,
/// dotted keys.
fn normalize_object_reference(reference: &str) -> String {
    reference
        .split('.')
        .map(strip_identifier_quotes)
        .collect::<Vec<_>>()
        .join(".")
}

fn collect_table_aliases(node: Node, source: &str, aliases: &mut Vec<TableAlias>) {
    if node.kind() == "relation" {
        let mut table_name: Option<String> = None;
        let mut alias: Option<String> = None;

        for i in 0..node.child_count() as u32 {
            if let Some(child) = node.child(i) {
                match child.kind() {
                    "object_reference" => {
                        // The table name is the text of the object_reference.
                        // Strip quotes so `"Order"`/`` `t` ``/`[t]` match the
                        // unquoted cache keys and per-part quoting in a
                        // schema-qualified name is normalized.
                        let range = child.byte_range();
                        if range.end <= source.len() {
                            table_name = Some(normalize_object_reference(&source[range]));
                        }
                    }
                    "identifier" => {
                        // The alias is a bare identifier after the object_reference
                        let range = child.byte_range();
                        if range.end <= source.len() {
                            alias = Some(strip_identifier_quotes(&source[range]).to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        if let (Some(table), Some(al)) = (table_name, alias) {
            aliases.push(TableAlias {
                table_name: table,
                alias: al,
            });
        }
    }

    // Recurse into children
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            collect_table_aliases(child, source, aliases);
        }
    }
}

/// Strip a single pair of surrounding SQL quote characters from an identifier:
/// double quotes (ANSI/PostgreSQL), backticks (MySQL), or square brackets
/// (MSSQL, `[dbo]`). Quoted identifiers preserve case and may contain characters
/// the bare-word scanner stops on, so callers normalize by removing the quotes
/// before matching against the metadata cache (whose keys are stored unquoted).
pub fn strip_identifier_quotes(ident: &str) -> &str {
    let bytes = ident.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if matches!((first, last), (b'"', b'"') | (b'`', b'`') | (b'[', b']')) {
            return &ident[1..ident.len() - 1];
        }
    }
    ident
}

/// Case-insensitive equality for SQL identifiers. Unquoted identifiers are
/// case-folded by every supported engine, so alias/CTE/schema matching must
/// ignore case (`FROM Users u ... u.` should resolve). Surrounding quotes are
/// stripped first so `"Users"` and `users` compare equal at the boundary where
/// the cache already holds unquoted names.
pub fn ident_eq(a: &str, b: &str) -> bool {
    strip_identifier_quotes(a).eq_ignore_ascii_case(strip_identifier_quotes(b))
}

/// Scan backwards from `end` over the bytes to the start of the identifier that
/// ends there, understanding the three quoting styles. A trailing closing quote
/// (`"`, `` ` ``, `]`) consumes back to its matching opening quote; otherwise a
/// bare run of `[A-Za-z0-9_]` is consumed. Returns the start index (== `end`
/// when there is no identifier immediately before `end`).
fn identifier_start(bytes: &[u8], end: usize) -> usize {
    if end == 0 {
        return 0;
    }
    let closing = bytes[end - 1];
    let opening = match closing {
        b'"' => Some(b'"'),
        b'`' => Some(b'`'),
        b']' => Some(b'['),
        _ => None,
    };
    if let Some(open) = opening {
        // Walk back to the matching opening quote.
        let mut pos = end - 1;
        while pos > 0 && bytes[pos - 1] != open {
            pos -= 1;
        }
        // Include the opening quote itself if found.
        return pos.saturating_sub(1);
    }

    let mut pos = end;
    while pos > 0 && (bytes[pos - 1].is_ascii_alphanumeric() || bytes[pos - 1] == b'_') {
        pos -= 1;
    }
    pos
}

/// Detect dot notation by scanning backwards from cursor.
/// Returns (is_dot, optional_table_name). The returned table name is
/// quote-stripped so quoted identifiers (`"Order".`, `` `tbl`. ``, `[dbo].`)
/// resolve against the unquoted cache keys.
fn detect_dot_notation(text: &str, cursor_byte_pos: usize) -> (bool, Option<String>) {
    let before_cursor = &text[..cursor_byte_pos.min(text.len())];
    let trimmed = before_cursor.as_bytes();

    // Skip the partial word being typed after the dot (bare or quoted).
    let pos = identifier_start(trimmed, trimmed.len());

    // Check if there's a dot immediately before it.
    if pos > 0 && trimmed[pos - 1] == b'.' {
        let dot_pos = pos - 1;
        // Extract the (possibly quoted) identifier before the dot.
        let ident_start = identifier_start(trimmed, dot_pos);
        if ident_start < dot_pos {
            let table_name = std::str::from_utf8(&trimmed[ident_start..dot_pos])
                .ok()
                .map(|s| strip_identifier_quotes(s).to_string());
            return (true, table_name);
        }
        return (true, None);
    }

    (false, None)
}

/// Extract the current word being typed at cursor position.
fn extract_current_word_from_text(text: &str, cursor_byte_pos: usize) -> String {
    let before = &text[..cursor_byte_pos.min(text.len())];
    for (byte_offset, ch) in before.char_indices().rev() {
        if !(ch.is_alphanumeric() || ch == '_') {
            let start = byte_offset + ch.len_utf8();
            return before[start..].to_string();
        }
    }
    before.to_string()
}

/// Determine the SQL clause at the cursor position by walking the AST.
fn determine_clause(root: Node, source: &str, cursor_byte_pos: usize) -> Option<SqlClause> {
    // Find the deepest named node at (or just before) the cursor position
    let node = find_deepest_node_at(root, cursor_byte_pos);

    // Walk up from that node to find the enclosing clause
    let mut current = Some(node);
    while let Some(node) = current {
        let clause = match node.kind() {
            "select" => Some(SqlClause::Select),
            "from" => {
                // "from" contains "where", "join", etc. as children.
                // Check if cursor is in a more specific child clause.
                if let Some(specific) = find_specific_clause_in_from(node, source, cursor_byte_pos)
                {
                    Some(specific)
                } else {
                    Some(SqlClause::From)
                }
            }
            "where" => Some(SqlClause::Where),
            "join" => {
                // Check if we're in the ON part
                if is_cursor_in_on_clause(node, cursor_byte_pos) {
                    Some(SqlClause::On)
                } else {
                    Some(SqlClause::Join)
                }
            }
            "order_by" => Some(SqlClause::OrderBy),
            "group_by" => Some(SqlClause::GroupBy),
            "having" => Some(SqlClause::Having),
            "insert" => Some(SqlClause::Insert),
            "update" => {
                // Check if we're in the SET part
                if is_cursor_after_keyword(source, node, "SET", cursor_byte_pos) {
                    Some(SqlClause::Set)
                } else {
                    Some(SqlClause::Update)
                }
            }
            "delete" => Some(SqlClause::Delete),
            "values" => Some(SqlClause::Values),
            "set" => Some(SqlClause::Set),
            _ => None,
        };

        if clause.is_some() {
            return clause;
        }
        current = node.parent();
    }

    None
}

/// Check ERROR nodes at the root level for trailing SQL keywords.
/// When typing incomplete SQL like "SELECT * FROM users WHERE ", tree-sitter
/// puts trailing keywords in ERROR nodes separate from the statement node.
fn find_clause_in_error_nodes(
    root_node: Node,
    source: &str,
    cursor_byte_pos: usize,
) -> Option<SqlClause> {
    let mut best_clause: Option<SqlClause> = None;
    let mut best_pos: usize = 0;

    for i in 0..root_node.child_count() as u32 {
        if let Some(child) = root_node.child(i) {
            // Only look at ERROR nodes that are at or before the cursor
            if child.kind() != "ERROR" || child.byte_range().start > cursor_byte_pos {
                continue;
            }
            // Scan keyword children within the ERROR node
            for j in 0..child.child_count() as u32 {
                if let Some(kw_node) = child.child(j) {
                    let range = kw_node.byte_range();
                    if range.start > cursor_byte_pos || range.end > source.len() {
                        continue;
                    }
                    if range.start >= best_pos {
                        let clause = match kw_node.kind() {
                            "keyword_select" => Some(SqlClause::Select),
                            "keyword_from" => Some(SqlClause::From),
                            "keyword_where" => Some(SqlClause::Where),
                            "keyword_join" => Some(SqlClause::Join),
                            "keyword_inner" | "keyword_left" | "keyword_right" => {
                                Some(SqlClause::Join)
                            }
                            "keyword_on" => Some(SqlClause::On),
                            "keyword_having" => Some(SqlClause::Having),
                            "keyword_set" => Some(SqlClause::Set),
                            "keyword_and" | "keyword_or" => {
                                // AND/OR don't change the clause, skip
                                None
                            }
                            _ => None,
                        };
                        if let Some(c) = clause {
                            best_pos = range.start;
                            best_clause = Some(c);
                        }
                    }
                }
            }
        }
    }

    best_clause
}

/// Find the deepest named node at or just before the cursor position.
fn find_deepest_node_at(node: Node, cursor_byte_pos: usize) -> Node {
    let mut best = node;
    for i in 0..node.child_count() as u32 {
        if let Some(child) = node.child(i) {
            let range = child.byte_range();
            // Allow nodes that start at or before cursor and end at or after cursor,
            // or nodes that end just before cursor (for trailing whitespace cases)
            if range.start <= cursor_byte_pos && cursor_byte_pos <= range.end {
                best = find_deepest_node_at(child, cursor_byte_pos);
            } else if range.end <= cursor_byte_pos && range.end + 1 >= cursor_byte_pos {
                // Cursor is right after this node
                if child.is_named() {
                    best = child;
                }
            }
        }
    }
    best
}

/// Within a "from" node, check for more specific child clauses.
fn find_specific_clause_in_from(
    from_node: Node,
    _source: &str,
    cursor_byte_pos: usize,
) -> Option<SqlClause> {
    for i in 0..from_node.child_count() as u32 {
        if let Some(child) = from_node.child(i) {
            let range = child.byte_range();
            if range.start <= cursor_byte_pos && cursor_byte_pos <= range.end {
                match child.kind() {
                    "where" => return Some(SqlClause::Where),
                    "join" => {
                        if is_cursor_in_on_clause(child, cursor_byte_pos) {
                            return Some(SqlClause::On);
                        }
                        return Some(SqlClause::Join);
                    }
                    "order_by" => return Some(SqlClause::OrderBy),
                    "group_by" => return Some(SqlClause::GroupBy),
                    "having" => return Some(SqlClause::Having),
                    _ => {
                        // Recurse for nested structures
                        if let Some(clause) =
                            find_specific_clause_in_from(child, _source, cursor_byte_pos)
                        {
                            return Some(clause);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Check if cursor is within the ON part of a JOIN
fn is_cursor_in_on_clause(join_node: Node, cursor_byte_pos: usize) -> bool {
    for i in 0..join_node.child_count() as u32 {
        if let Some(child) = join_node.child(i)
            && child.kind() == "keyword_on"
            && cursor_byte_pos > child.byte_range().end
        {
            return true;
        }
    }
    false
}

/// Check if cursor is after a specific keyword within a node
fn is_cursor_after_keyword(
    source: &str,
    node: Node,
    keyword: &str,
    cursor_byte_pos: usize,
) -> bool {
    let node_text = &source[node.byte_range()];
    let node_upper = node_text.to_uppercase();
    if let Some(kw_offset) = node_upper.find(keyword) {
        let absolute_offset = node.byte_range().start + kw_offset + keyword.len();
        return cursor_byte_pos >= absolute_offset;
    }
    false
}

/// Fallback clause detection from raw text when AST has ERROR nodes.
/// Scans for the last SQL keyword before cursor position.
fn determine_clause_from_keywords(source: &str, cursor_byte_pos: usize) -> Option<SqlClause> {
    let text = &source[..cursor_byte_pos.min(source.len())];
    let text_upper = text.to_uppercase();

    // Keywords to search for, ordered by multi-word first
    const CLAUSE_KEYWORDS: &[(&str, SqlClause)] = &[
        ("ORDER BY", SqlClause::OrderBy),
        ("GROUP BY", SqlClause::GroupBy),
        ("INNER JOIN", SqlClause::Join),
        ("LEFT JOIN", SqlClause::Join),
        ("RIGHT JOIN", SqlClause::Join),
        ("OUTER JOIN", SqlClause::Join),
        ("SELECT", SqlClause::Select),
        ("FROM", SqlClause::From),
        ("WHERE", SqlClause::Where),
        ("JOIN", SqlClause::Join),
        ("HAVING", SqlClause::Having),
        ("SET", SqlClause::Set),
        ("UPDATE", SqlClause::Update),
        ("INSERT", SqlClause::Insert),
        ("DELETE", SqlClause::Delete),
        ("INTO", SqlClause::Insert),
        ("VALUES", SqlClause::Values),
        ("ON", SqlClause::On),
    ];

    let mut best_pos: Option<usize> = None;
    let mut best_clause: Option<SqlClause> = None;

    for (keyword, clause) in CLAUSE_KEYWORDS {
        if let Some(pos) = text_upper.rfind(keyword) {
            // Verify word boundary
            let before_ok = pos == 0 || !text.as_bytes()[pos - 1].is_ascii_alphanumeric();
            let after_pos = pos + keyword.len();
            let after_ok =
                after_pos >= text.len() || !text.as_bytes()[after_pos].is_ascii_alphanumeric();

            if before_ok && after_ok && best_pos.is_none_or(|bp| pos > bp) {
                best_pos = Some(pos);
                best_clause = Some(*clause);
            }
        }
    }

    best_clause
}

#[cfg(test)]
mod tests {
    use super::*;
    use database::DatabaseType;

    fn create_test_parser() -> SqlStatementParser {
        SqlStatementParser::new().expect("Failed to create test parser")
    }

    #[test]
    fn strip_identifier_quotes_handles_all_dialects() {
        assert_eq!(strip_identifier_quotes("\"Order\""), "Order");
        assert_eq!(strip_identifier_quotes("`tbl`"), "tbl");
        assert_eq!(strip_identifier_quotes("[dbo]"), "dbo");
        assert_eq!(strip_identifier_quotes("plain"), "plain");
        // Mismatched or single char left untouched.
        assert_eq!(strip_identifier_quotes("\""), "\"");
        assert_eq!(strip_identifier_quotes("[t\""), "[t\"");
    }

    #[test]
    fn ident_eq_is_case_and_quote_insensitive() {
        assert!(ident_eq("Users", "users"));
        assert!(ident_eq("\"Users\"", "users"));
        assert!(ident_eq("u", "U"));
        assert!(!ident_eq("users", "orders"));
    }

    #[test]
    fn detect_dot_notation_handles_quoted_identifiers() {
        // Double-quoted (ANSI/Postgres)
        let text = "SELECT * FROM \"Order\".";
        assert_eq!(
            detect_dot_notation(text, text.len()),
            (true, Some("Order".to_string()))
        );
        // Backtick (MySQL) with a partial column typed after the dot
        let text = "SELECT `tbl`.na";
        assert_eq!(
            detect_dot_notation(text, text.len()),
            (true, Some("tbl".to_string()))
        );
        // Bracketed (MSSQL)
        let text = "SELECT [T].";
        assert_eq!(
            detect_dot_notation(text, text.len()),
            (true, Some("T".to_string()))
        );
        // Bare identifier still works
        let text = "SELECT u.";
        assert_eq!(
            detect_dot_notation(text, text.len()),
            (true, Some("u".to_string()))
        );
        // No dot
        let text = "SELECT foo";
        assert_eq!(detect_dot_notation(text, text.len()), (false, None));
    }

    #[test]
    fn command_at_cursor_returns_line_under_cursor() {
        let text = "GET foo\nHGETALL bar\nLRANGE q 0 -1\n";
        // Cursor on the second line (byte offset within "HGETALL bar").
        let cursor = text.find("bar").unwrap();
        assert_eq!(
            extract_command_at_cursor(text, cursor),
            Some("HGETALL bar".to_string())
        );
    }

    #[test]
    fn command_at_cursor_trims_and_skips_blank_lines() {
        let text = "  GET foo  \n\nSET k v\n";
        assert_eq!(
            extract_command_at_cursor(text, 0),
            Some("GET foo".to_string())
        );
        // Cursor on the blank middle line yields nothing.
        let blank = text.find("\n\n").unwrap() + 1;
        assert_eq!(extract_command_at_cursor(text, blank), None);
    }

    #[test]
    fn command_at_cursor_handles_cursor_past_end() {
        let text = "PING";
        assert_eq!(
            extract_command_at_cursor(text, 999),
            Some("PING".to_string())
        );
    }

    /// Helper to convert character position to byte position for tests
    fn char_to_byte_pos(text: &Rope, char_pos: usize) -> usize {
        if char_pos < text.len_chars() {
            text.char_to_byte_idx(char_pos)
        } else {
            text.len()
        }
    }

    #[test]
    fn test_simple_select() {
        let mut parser = create_test_parser();
        let text = Rope::from_str("SELECT * FROM users;");
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 10),
            ParamStyles::all(),
        );

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
    }

    #[test]
    fn test_insert_with_semicolon_in_string() {
        let mut parser = create_test_parser();
        let text = Rope::from_str("INSERT INTO orders (a) VALUES ('hello;');");
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 25),
            ParamStyles::all(),
        );

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "INSERT INTO orders (a) VALUES ('hello;')");
    }

    #[test]
    fn test_multiple_statements() {
        let mut parser = create_test_parser();
        let text = Rope::from_str(
            "SELECT * FROM table1; INSERT INTO table2 (col) VALUES ('test;'); UPDATE table3 SET col = 'value';",
        );

        // Test cursor in first statement
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 10),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        assert!(
            result
                .unwrap()
                .text
                .trim()
                .starts_with("SELECT * FROM table1")
        );

        // Test cursor in second statement
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 50),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim(),
            "INSERT INTO table2 (col) VALUES ('test;')"
        );

        // Test cursor in third statement
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 85),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(info.text.trim().starts_with("UPDATE table3"));
    }

    #[test]
    fn test_comments_with_semicolons() {
        let mut parser = create_test_parser();
        let text = Rope::from_str("SELECT * FROM users -- This comment has a semicolon;");
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 15),
            ParamStyles::all(),
        );

        assert!(result.is_some());
        let info = result.unwrap();
        // Should extract the SELECT statement part
        assert!(info.text.trim().contains("SELECT * FROM users"));
        // Comment may or may not be included depending on tree-sitter parsing
        // The important part is that the comment semicolon doesn't break statement detection
    }

    #[test]
    fn test_incomplete_statement() {
        let mut parser = create_test_parser();
        let text = Rope::from_str("SELECT * FROM users WHERE id = 1");
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );

        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users WHERE id = 1");
    }

    #[test]
    fn test_empty_text() {
        let mut parser = create_test_parser();
        let text = Rope::from_str("");
        let result = parser.extract_statement_at_cursor(&text, 0, ParamStyles::all());
        assert!(result.is_none());
    }

    #[test]
    fn test_cursor_before_semicolon() {
        let mut parser = create_test_parser();
        let text = Rope::from_str("SELECT * FROM users;");

        // Position cursor just before the semicolon (after 'users')
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 18),
            ParamStyles::all(),
        );
        assert!(
            result.is_some(),
            "Should extract statement when cursor is before semicolon"
        );
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");

        // Also test at the semicolon position
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 19),
            ParamStyles::all(),
        );
        assert!(
            result.is_some(),
            "Should extract statement when cursor is at semicolon position"
        );
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
    }

    #[test]
    fn test_cursor_before_semicolon_with_whitespace() {
        let mut parser = create_test_parser();
        let text = Rope::from_str("SELECT * FROM users   ;");

        // Position cursor at the first space after 'users'
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 16),
            ParamStyles::all(),
        );
        assert!(
            result.is_some(),
            "Should extract statement when cursor is before whitespace and semicolon"
        );
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");
    }

    #[test]
    fn test_unicode_statement() {
        let mut parser = create_test_parser();
        // Test with Unicode characters (emoji and non-ASCII) in a valid SQL statement
        let text = Rope::from_str("SELECT * FROM testing WHERE text_col = '🏠';");

        // Position cursor in the middle of the statement
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );

        assert!(
            result.is_some(),
            "Should extract statement with Unicode characters"
        );
        let info = result.unwrap();

        assert!(info.byte_range.start < info.byte_range.end);
        assert_eq!(
            info.text.trim(),
            "SELECT * FROM testing WHERE text_col = '🏠'"
        );
    }

    #[test]
    fn test_multiline_statements() {
        let mut parser = create_test_parser();
        let text = Rope::from_str(
            "INSERT INTO orders (a)
VALUES ('hello;');

SELECT * FROM users; -- some comment

DELETE FROM users WHERE id = 1;",
        );

        // Test cursor on first line (INSERT)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 5),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim().replace('\n', " "),
            "INSERT INTO orders (a) VALUES ('hello;')"
        );

        // Test cursor on second line (VALUES part of INSERT)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 30),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim().replace('\n', " "),
            "INSERT INTO orders (a) VALUES ('hello;')"
        );

        // Test cursor on third line (empty line after INSERT semicolon)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 40),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        // The INSERT statement is still the closest at this position
        assert!(info.text.trim().contains("INSERT INTO orders"));

        // Test cursor on fourth line (SELECT)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 45),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users");

        // Test cursor on fifth line (comment after SELECT)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 60),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        // Should still return the SELECT statement even with cursor in comment
        assert!(info.text.trim().contains("SELECT * FROM users"));

        // Test cursor on sixth line (empty line before DELETE)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 80),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DELETE FROM users WHERE id = 1");

        // Test cursor on seventh line (DELETE)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 85),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DELETE FROM users WHERE id = 1");
    }

    #[test]
    fn test_mixed_formatting_statements() {
        let mut parser = create_test_parser();
        // Test statements with various formatting styles
        let text = Rope::from_str(
            "  CREATE TABLE test (
            id INTEGER PRIMARY KEY,
            name TEXT
        );  -- Create test table

        UPDATE test SET name = 'test' WHERE id = 1;

        DROP TABLE test;",
        );

        // Test cursor in CREATE (first line)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 4),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        // Multi-line CREATE statement may not include the semicolon
        assert!(info.text.trim().starts_with("CREATE TABLE test"));

        // Test cursor in CREATE (middle)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 30),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert!(info.text.trim().starts_with("CREATE TABLE test"));

        // Test cursor in UPDATE
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 120),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.text.trim(),
            "UPDATE test SET name = 'test' WHERE id = 1"
        );

        // Test cursor in DROP
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 175),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DROP TABLE test");
    }

    #[test]
    fn test_no_whitespace_between_statements() {
        let mut parser = create_test_parser();
        // Test statements with minimal whitespace
        let text =
            Rope::from_str("SELECT a FROM b;INSERT INTO c VALUES (1);DELETE FROM d WHERE e = 2;");

        // Test cursor in SELECT
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 5),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT a FROM b");

        // Test cursor in INSERT (position 20)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "INSERT INTO c VALUES (1)");

        // Test cursor in DELETE (position 45, well into DELETE)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 45),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "DELETE FROM d WHERE e = 2");
    }

    #[test]
    fn test_update_with_uuid_string() {
        let mut parser = create_test_parser();
        // This query caused a panic: byte index 177 is out of bounds
        // The issue was that tree-sitter returned a node with byte range extending beyond text length
        let text = Rope::from_str(
            "UPDATE alternative_images ai SET updated_at = NOW() WHERE id = 'f893fd7a-45a2-4747-a726-5561bb4735ec'",
        );

        // Test cursor at various positions in the statement
        for cursor_pos in [0, 10, 30, 50, 70, 90, text.len().saturating_sub(1)] {
            let result = parser.extract_statement_at_cursor(
                &text,
                char_to_byte_pos(&text, cursor_pos),
                ParamStyles::all(),
            );
            assert!(
                result.is_some(),
                "Should extract statement at cursor position {}",
                cursor_pos
            );
            let info = result.unwrap();
            assert_eq!(
                info.text.trim(),
                "UPDATE alternative_images ai SET updated_at = NOW() WHERE id = 'f893fd7a-45a2-4747-a726-5561bb4735ec'"
            );

            // Should have no parameters
            assert!(info.parameters.is_empty());
        }
    }

    #[test]
    fn test_parameter_extraction_with_special_characters() {
        let mut parser = create_test_parser();
        // Test that single character named params don't panic
        // Note: :a is NOT recognized as a parameter by tree-sitter SQL grammar
        let text = Rope::from_str("SELECT * FROM users WHERE name = :a");

        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.text.trim(), "SELECT * FROM users WHERE name = :a");
        // tree-sitter doesn't classify :a as a parameter node
        assert_eq!(info.parameters.len(), 0);

        // Test with positional parameter (PostgreSQL style)
        // Note: $1 IS recognized as a positional_parameter by tree-sitter
        let text = Rope::from_str("SELECT * FROM users WHERE id = $1");
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        // PostgreSQL $1 IS recognized
        assert_eq!(info.parameters.len(), 1);
        assert_eq!(info.parameters[0].raw_text, "$1");
        matches!(info.parameters[0].style, ParameterStyle::Positional(1));

        // Test with multiple parameters
        let text = Rope::from_str("SELECT * FROM users WHERE id = $1 AND name = $2");
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.parameters.len(), 2);
        // Parameters are sorted in ascending byte offset order (for user display)
        assert_eq!(info.parameters[0].raw_text, "$1");
        assert_eq!(info.parameters[1].raw_text, "$2");
        // Verify byte offsets are in ascending order
        assert!(info.parameters[0].byte_offset < info.parameters[1].byte_offset);
    }

    #[test]
    fn test_parameter_extraction_bound_parameter() {
        let mut parser = create_test_parser();
        // Test with ? style parameter
        // Note: ? IS recognized as a bind_parameter by tree-sitter SQL grammar
        let text = Rope::from_str("SELECT * FROM users WHERE id = ?");

        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        // ? IS recognized as a bind_parameter
        assert_eq!(info.parameters.len(), 1);
        assert_eq!(info.parameters[0].raw_text, "?");
        // ? parameters get sequential positional indices starting at 1
        assert_eq!(info.parameters[0].style, ParameterStyle::Positional(1));

        // Test with multiple ? parameters
        let text = Rope::from_str("SELECT * FROM users WHERE id = ? AND name = ?");
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 20),
            ParamStyles::all(),
        );
        assert!(result.is_some());
        let info = result.unwrap();
        // Multiple ? are recognized as separate bind_parameter nodes with sequential indices
        assert_eq!(info.parameters.len(), 2);
        assert_eq!(info.parameters[0].style, ParameterStyle::Positional(1));
        assert_eq!(info.parameters[1].style, ParameterStyle::Positional(2));
    }

    #[test]
    fn test_select_above_cte() {
        let mut parser = create_test_parser();
        let text = Rope::from_str(
            "SELECT * FROM ps_customer
WHERE id_shop = 3
ORDER BY date_add DESC
LIMIT 100;


WITH customer_addresses AS (
    SELECT
        c.id_customer,
        JSON_ARRAYAGG(
            JSON_OBJECT(
                'ID', UUID_v7(),
                'OrganizationName', a.company
            )
        ) AS addresses
    FROM ps_customer c
    INNER JOIN ps_orders o ON o.id_customer = c.id_customer
    LEFT JOIN ps_address a ON a.id_customer = c.id_customer AND a.deleted = 0
    WHERE c.email IS NOT NULL AND c.email != ''
    GROUP BY c.id_customer
)",
        );

        // Test cursor inside the first SELECT statement (position 10)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 10),
            ParamStyles::all(),
        );
        assert!(
            result.is_some(),
            "Should extract first SELECT when cursor is inside it, even with CTE below"
        );
        let info = result.unwrap();
        assert!(info.text.trim().contains("SELECT * FROM ps_customer"));

        // Test cursor at different positions in first statement
        for cursor_pos in [30, 50, 70] {
            let result = parser.extract_statement_at_cursor(
                &text,
                char_to_byte_pos(&text, cursor_pos),
                ParamStyles::all(),
            );
            assert!(
                result.is_some(),
                "Should extract first SELECT at cursor position {}",
                cursor_pos
            );
            let info = result.unwrap();
            assert!(info.text.trim().contains("SELECT * FROM ps_customer"));
        }

        // Test cursor inside the WITH/CTE statement (position around 150)
        let result = parser.extract_statement_at_cursor(
            &text,
            char_to_byte_pos(&text, 150),
            ParamStyles::all(),
        );
        assert!(
            result.is_some(),
            "Should extract CTE statement when cursor is inside it"
        );
        let info = result.unwrap();
        assert!(info.text.trim().contains("WITH customer_addresses"));
    }

    #[test]
    fn test_postgres_jsonb_question_mark_not_a_parameter() {
        let mut parser = create_test_parser();
        // In Postgres, `?` is the jsonb key-exists operator, not a placeholder.
        let text = Rope::from_str("SELECT * FROM events WHERE data ? 'key'");

        let pg = DatabaseType::PostgreSQL.parameter_styles();
        let result = parser.extract_statement_at_cursor(&text, char_to_byte_pos(&text, 10), pg);
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(
            info.parameters.len(),
            0,
            "Postgres `?` jsonb operator must not be treated as a parameter"
        );

        // The same text under a `?`-using dialect (e.g. MySQL) still detects it.
        let mysql = DatabaseType::MySQL.parameter_styles();
        let result = parser.extract_statement_at_cursor(&text, char_to_byte_pos(&text, 10), mysql);
        let info = result.unwrap();
        assert_eq!(info.parameters.len(), 1);
        assert_eq!(info.parameters[0].raw_text, "?");
    }

    #[test]
    fn test_postgres_dollar_parameter_still_detected() {
        let mut parser = create_test_parser();
        // `$1` remains a valid Postgres placeholder even with `?` disabled.
        let text = Rope::from_str("SELECT * FROM users WHERE id = $1");

        let pg = DatabaseType::PostgreSQL.parameter_styles();
        let result = parser.extract_statement_at_cursor(&text, char_to_byte_pos(&text, 20), pg);
        let info = result.unwrap();
        assert_eq!(info.parameters.len(), 1);
        assert_eq!(info.parameters[0].raw_text, "$1");
    }

    #[test]
    fn test_mysql_dollar_number_not_a_parameter() {
        let mut parser = create_test_parser();
        // MySQL has no `$1` positional placeholders, so it must not be detected.
        let text = Rope::from_str("SELECT * FROM users WHERE id = $1");

        let mysql = DatabaseType::MySQL.parameter_styles();
        let result = parser.extract_statement_at_cursor(&text, char_to_byte_pos(&text, 20), mysql);
        let info = result.unwrap();
        assert_eq!(info.parameters.len(), 0);
    }

    #[test]
    fn test_postgres_jsonb_question_mark_with_dollar_parameter() {
        let mut parser = create_test_parser();
        // Mixing a jsonb `?` operator with a real `$1` placeholder: only `$1`
        // should be surfaced as a parameter.
        let text = Rope::from_str("SELECT * FROM events WHERE data ? 'key' AND id = $1");

        let pg = DatabaseType::PostgreSQL.parameter_styles();
        let result = parser.extract_statement_at_cursor(&text, char_to_byte_pos(&text, 10), pg);
        let info = result.unwrap();
        assert_eq!(info.parameters.len(), 1);
        assert_eq!(info.parameters[0].raw_text, "$1");
    }
}
