//! Tool handlers for agent-level tool execution with GPUI context access.
//!
//! This module provides tool handlers that can access GPUI entities and globals,
//! decoupling tool execution from the LLM provider implementations.

use async_trait::async_trait;
use gpui::{AsyncWindowContext, WeakEntity};
use gpui_component::input::{InputState, RopeExt};
use std::collections::HashMap;
use std::sync::Arc;

use blanco_core::DatabaseService;
use blanco_core::{DatabaseSchemaResult, TableSchemaInfo};
use llm::{
    FunctionCall, ToolCall, chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema,
    chat::Tool,
};
use std::fmt::Write as _;

/// Context for executing tools with GPUI/database access
pub struct ToolContext {
    pub db_service: Arc<dyn DatabaseService>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
    pub input_state: Option<WeakEntity<InputState>>,
}

/// Async tool handler for agent-level tools
#[async_trait(?Send)]
pub trait AgentToolHandler: Send + Sync {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolCall;

    /// Convert to llm Tool for use with llm crate
    fn as_tool(&self) -> Tool;

    fn call_summary(&self, arguments: &serde_json::Value, result: &ToolCall) -> String;

    fn always_allow(&self) -> bool {
        false
    }
}

/// List tables tool handler
pub struct ListTablesHandler;

#[async_trait(?Send)]
impl AgentToolHandler for ListTablesHandler {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        _cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        // Extract parameters
        let table_names = arguments
            .get("table_names")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20); // Default limit of 20

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_i64())
            .unwrap_or(0); // Default offset of 0

        // Get connection from DatabaseService
        let conn_result = if let Some(conn_id) = context.connection_id {
            let database_name_ref = context.database_name.as_deref();
            context
                .db_service
                .get_or_create_connection_by_id(conn_id, database_name_ref)
                .await
        } else {
            return ToolCall {
                id: "list-tables".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "list-tables".to_string(),
                    arguments: "Error: no database connection available. Please connect to a database first.".to_string(),
                },
            };
        };

        match conn_result {
            Ok(conn) => {
                // Query schema
                match conn
                    .get_database_schema_paginated(
                        context.database_name.as_deref(),
                        table_names.as_deref(),
                        Some(limit),
                        Some(offset),
                    )
                    .await
                {
                    Ok(result) => ToolCall {
                        id: "list-tables".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "list-tables".to_string(),
                            arguments: format_schema_markdown(&result),
                        },
                    },
                    Err(e) => ToolCall {
                        id: "list-tables".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "list-tables".to_string(),
                            arguments: format!("Error: failed to query database schema: {}", e),
                        },
                    },
                }
            }
            Err(e) => ToolCall {
                id: "list-tables".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "list-tables".to_string(),
                    arguments: format!("Error: failed to get database connection: {}", e),
                },
            },
        }
    }

    fn as_tool(&self) -> Tool {
        // Build parameters schema
        let mut properties = HashMap::new();
        properties.insert(
            "table_names".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "Optional comma-separated list of table name patterns to filter. Supports SQL LIKE wildcards: % matches any characters, _ matches a single character. Example: '%user%' finds tables containing 'user', 'order%' finds tables starting with 'order'. If not provided, lists all tables in user schemas.".to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "limit".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "Maximum number of tables to return. Default: 20.".to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "offset".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "Number of tables to skip for pagination. Default: 0.".to_string(),
                items: None,
                enum_list: None,
            },
        );

        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "list-tables".to_string(),
                description: "List detailed schema information for user-created tables. Returns a Markdown document with one section per table containing the columns (name, type, nullable, primary key, default), a `Foreign keys` section listing outgoing references as `column -> referenced_table.referenced_column`, and a `Referenced by` section listing inbound references from other tables as `from_table.from_column -> column`. Use both sections to reason about how tables relate to each other in either direction. Supports pagination with limit and offset parameters.".to_string(),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec![],
                }).unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolCall) -> String {
        let table_names = arguments
            .get("table_names")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if table_names.is_empty() {
            "List Tables".to_string()
        } else {
            // Parse comma-separated table names and truncate if too long
            let names: Vec<&str> = table_names.split(',').map(|s| s.trim()).collect();
            if names.len() > 3 {
                format!("List Tables: {}, ...", names[0..3].join(", "))
            } else {
                format!("List Tables: {}", names.join(", "))
            }
        }
    }
}

/// Lightweight discovery tool: returns table names + relationships, no column detail
pub struct ExploreTablesHandler;

#[async_trait(?Send)]
impl AgentToolHandler for ExploreTablesHandler {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        _cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        let table_names = arguments
            .get("table_names")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100);

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        let conn_result = if let Some(conn_id) = context.connection_id {
            let database_name_ref = context.database_name.as_deref();
            context
                .db_service
                .get_or_create_connection_by_id(conn_id, database_name_ref)
                .await
        } else {
            return ToolCall {
                id: "explore-tables".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "explore-tables".to_string(),
                    arguments: "Error: no database connection available. Please connect to a database first.".to_string(),
                },
            };
        };

        match conn_result {
            Ok(conn) => match conn
                .get_database_schema_paginated(
                    context.database_name.as_deref(),
                    table_names.as_deref(),
                    Some(limit),
                    Some(offset),
                )
                .await
            {
                Ok(result) => ToolCall {
                    id: "explore-tables".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "explore-tables".to_string(),
                        arguments: format_explore_markdown(&result),
                    },
                },
                Err(e) => ToolCall {
                    id: "explore-tables".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "explore-tables".to_string(),
                        arguments: format!("Error: failed to query database schema: {}", e),
                    },
                },
            },
            Err(e) => ToolCall {
                id: "explore-tables".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "explore-tables".to_string(),
                    arguments: format!("Error: failed to get database connection: {}", e),
                },
            },
        }
    }

    fn as_tool(&self) -> Tool {
        let mut properties = HashMap::new();
        properties.insert(
            "table_names".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "Optional comma-separated list of table name patterns to filter, with SQL LIKE wildcards (% and _). If omitted, lists all user tables.".to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "limit".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "Maximum number of tables to return. Default: 100.".to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "offset".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "Number of tables to skip for pagination. Default: 0.".to_string(),
                items: None,
                enum_list: None,
            },
        );

        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "explore-tables".to_string(),
                description: "Lightweight discovery tool. Returns a compact Markdown list of tables with only their outgoing foreign keys (`-> other_table.col`) and inbound foreign keys (`<- from_table.col`). Use this FIRST when exploring an unfamiliar schema to find which tables are relevant and how they relate. Once you know which specific tables you need, call `list-tables` for full column-level detail. This tool returns no column names, types, or defaults.".to_string(),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec![],
                }).unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolCall) -> String {
        let table_names = arguments
            .get("table_names")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if table_names.is_empty() {
            "Explore Tables".to_string()
        } else {
            let names: Vec<&str> = table_names.split(',').map(|s| s.trim()).collect();
            if names.len() > 3 {
                format!("Explore Tables: {}, ...", names[0..3].join(", "))
            } else {
                format!("Explore Tables: {}", names.join(", "))
            }
        }
    }
}

fn format_explore_markdown(result: &DatabaseSchemaResult) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Tables in {}", result.display_name);
    let pagination = &result.pagination;
    let _ = writeln!(
        out,
        "_Pagination: limit={}, offset={}, has_more={}_",
        pagination
            .limit
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination
            .offset
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination.has_more,
    );

    if result.tables.is_empty() {
        out.push_str("\nNo tables matched.\n");
        return out;
    }

    for table in &result.tables {
        out.push('\n');
        let _ = writeln!(out, "## {}.{}", table.schema, table.name);
        for col in &table.columns {
            if let Some(fk) = &col.foreign_key {
                let _ = writeln!(
                    out,
                    "- {} -> {}.{}",
                    col.name, fk.foreign_table_name, fk.foreign_column_name
                );
            }
        }
        for inbound in &table.referenced_by {
            let _ = writeln!(
                out,
                "- {} <- {}.{}",
                inbound.to_column, inbound.from_table, inbound.from_column
            );
        }
    }
    out
}

fn format_schema_markdown(result: &DatabaseSchemaResult) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Tables in {}", result.display_name);
    let _ = writeln!(out, "_Connection type: {}_", result.connection_type);
    let pagination = &result.pagination;
    let _ = writeln!(
        out,
        "_Pagination: limit={}, offset={}, has_more={}_",
        pagination
            .limit
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination
            .offset
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        pagination.has_more,
    );

    if result.tables.is_empty() {
        out.push_str("\nNo tables matched.\n");
        return out;
    }

    for table in &result.tables {
        out.push('\n');
        format_table_markdown(&mut out, table);
    }
    out
}

fn format_table_markdown(out: &mut String, table: &TableSchemaInfo) {
    let _ = writeln!(
        out,
        "## {}.{} ({})",
        table.schema, table.name, table.object_type
    );
    out.push_str("| column | type | nullable | pk | default |\n");
    out.push_str("|--------|------|----------|----|---------|\n");
    for col in &table.columns {
        let default = col
            .default_value
            .as_deref()
            .map(|d| d.replace('|', "\\|").replace('\n', " "))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            col.name,
            col.data_type,
            if col.is_nullable { "yes" } else { "no" },
            if col.is_primary_key { "yes" } else { "no" },
            default,
        );
    }

    let fks: Vec<_> = table
        .columns
        .iter()
        .filter_map(|c| c.foreign_key.as_ref().map(|fk| (c.name.as_str(), fk)))
        .collect();
    if !fks.is_empty() {
        out.push_str("\nForeign keys:\n");
        for (column, fk) in fks {
            match &fk.constraint_name {
                Some(name) => {
                    let _ = writeln!(
                        out,
                        "- {} -> {}.{} ({})",
                        column, fk.foreign_table_name, fk.foreign_column_name, name
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "- {} -> {}.{}",
                        column, fk.foreign_table_name, fk.foreign_column_name
                    );
                }
            }
        }
    }

    if !table.referenced_by.is_empty() {
        out.push_str("\nReferenced by:\n");
        for inbound in &table.referenced_by {
            match &inbound.constraint_name {
                Some(name) => {
                    let _ = writeln!(
                        out,
                        "- {}.{} -> {} ({})",
                        inbound.from_table, inbound.from_column, inbound.to_column, name
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "- {}.{} -> {}",
                        inbound.from_table, inbound.from_column, inbound.to_column
                    );
                }
            }
        }
    }
}

/// Maximum lines to return when no range is specified
const READ_TAB_DEFAULT_LIMIT: usize = 200;

/// Read tab tool handler with optional line range support
pub struct ReadTabHandler;

#[async_trait(?Send)]
impl AgentToolHandler for ReadTabHandler {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        if let Some(input_state) = &context.input_state {
            let requested_start = arguments
                .get("start_line")
                .and_then(|v| v.as_i64())
                .map(|v| v.max(1) as usize);

            let requested_end = arguments
                .get("end_line")
                .and_then(|v| v.as_i64())
                .map(|v| v.max(1) as usize);

            match input_state.read_with(cx, |input, _cx| {
                let rope = input.text();
                let total_lines = rope.lines_len();

                if total_lines == 0 {
                    return (String::new(), 0usize, 0usize, 0usize);
                }

                let (start, end) = match (requested_start, requested_end) {
                    (Some(s), Some(e)) => (s, e),
                    (Some(s), None) => (s, total_lines),
                    (None, Some(e)) => (1, e),
                    (None, None) if total_lines > READ_TAB_DEFAULT_LIMIT => {
                        (1, READ_TAB_DEFAULT_LIMIT)
                    }
                    (None, None) => (1, total_lines),
                };

                let start = start.min(total_lines);
                let end = end.min(total_lines).max(start);

                let slice = rope.slice_lines((start - 1)..end);
                let content = slice.to_string();
                let numbered = content
                    .lines()
                    .enumerate()
                    .map(|(i, line)| format!("{}: {}", start + i, line))
                    .collect::<Vec<_>>()
                    .join("\n");

                (numbered, total_lines, start, end)
            }) {
                Ok((content, total_lines, start, end)) => {
                    let mut json = serde_json::json!({
                        "content": content,
                        "total_lines": total_lines,
                        "start_line": start,
                        "end_line": end,
                    });

                    if total_lines > end {
                        json["truncated"] = serde_json::json!(true);
                        json["hint"] = serde_json::json!(format!(
                            "Showing lines {}-{} of {}. Use start_line/end_line to read more.",
                            start, end, total_lines
                        ));
                    }

                    ToolCall {
                        id: "read-tab".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "read-tab".to_string(),
                            arguments: json.to_string(),
                        },
                    }
                }
                Err(e) => ToolCall {
                    id: "read-tab".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "read-tab".to_string(),
                        arguments: serde_json::json!({"error": format!("Failed to read tab content: {}", e)}).to_string(),
                    },
                },
            }
        } else {
            ToolCall {
                id: "read-tab".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "read-tab".to_string(),
                    arguments: serde_json::json!({"error": "No input state available. Please ensure the chat session is properly connected to a query tab."}).to_string(),
                },
            }
        }
    }

    fn as_tool(&self) -> Tool {
        let mut properties = HashMap::new();
        properties.insert(
            "start_line".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "1-based starting line number to read from. Defaults to line 1."
                    .to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "end_line".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: format!("1-based ending line number (inclusive). Defaults to the last line, or {} if the file is large and no range is specified.", READ_TAB_DEFAULT_LIMIT),
                items: None,
                enum_list: None,
            },
        );

        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "read-tab".to_string(),
                description: format!(
                    "Read the current query tab content with optional line range. Returns line-numbered content. If the tab has more than {} lines and no range is specified, returns only the first {} lines with a hint to read more.",
                    READ_TAB_DEFAULT_LIMIT, READ_TAB_DEFAULT_LIMIT
                ),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec![],
                })
                .unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolCall) -> String {
        let start = arguments
            .get("start_line")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let end = arguments.get("end_line").and_then(|v| v.as_i64());
        match end {
            Some(end) => format!("Read Tab: lines {}-{}", start, end),
            None => "Read Tab".to_string(),
        }
    }

    fn always_allow(&self) -> bool {
        true
    }
}

/// Write tab tool handler with multiple operation modes
pub struct WriteTabHandler;

#[derive(Clone, Copy, Debug, PartialEq)]
enum WriteOperation {
    ReplaceAll,
    InsertBeforeLine,
    ReplaceLines,
}

fn apply_line_operation(
    current_text: &str,
    operation: WriteOperation,
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, String> {
    match operation {
        WriteOperation::ReplaceAll => Ok(content.to_string()),
        WriteOperation::InsertBeforeLine => {
            let target = start_line.ok_or("start_line is required for insert_before_line")?;
            if target == 0 {
                return Err("start_line must be 1 or greater".to_string());
            }
            let mut lines: Vec<String> = current_text.lines().map(|l| l.to_string()).collect();
            let insert_at = (target - 1).min(lines.len());
            let new_lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            for (i, new_line) in new_lines.into_iter().enumerate() {
                lines.insert(insert_at + i, new_line);
            }
            // Preserve trailing newline if original had one
            let mut result = lines.join("\n");
            if current_text.ends_with('\n') {
                result.push('\n');
            }
            Ok(result)
        }
        WriteOperation::ReplaceLines => {
            let start = start_line.ok_or("start_line is required for replace_lines")?;
            let end = end_line.ok_or("end_line is required for replace_lines")?;
            if start == 0 || end == 0 {
                return Err("start_line and end_line must be 1 or greater".to_string());
            }
            if start > end {
                return Err("start_line must be <= end_line".to_string());
            }
            let mut lines: Vec<String> = current_text.lines().map(|l| l.to_string()).collect();
            let replace_start = (start - 1).min(lines.len());
            let replace_end = end.min(lines.len());
            let new_lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            lines.splice(replace_start..replace_end, new_lines);
            let mut result = lines.join("\n");
            if current_text.ends_with('\n') {
                result.push('\n');
            }
            Ok(result)
        }
    }
}

#[async_trait(?Send)]
impl AgentToolHandler for WriteTabHandler {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        if let Some(input_state) = &context.input_state {
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let operation_str = arguments
                .get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("replace_all");

            let operation = match operation_str {
                "insert_before_line" => WriteOperation::InsertBeforeLine,
                "replace_lines" => WriteOperation::ReplaceLines,
                _ => WriteOperation::ReplaceAll,
            };

            let start_line = arguments
                .get("start_line")
                .and_then(|v| v.as_i64())
                .map(|v| v as usize);

            let end_line = arguments
                .get("end_line")
                .and_then(|v| v.as_i64())
                .map(|v| v as usize);

            match input_state.update_in(cx, |input_state, window, cx| {
                let current = input_state.text().to_string();
                let new_text =
                    apply_line_operation(&current, operation, &content, start_line, end_line)?;
                input_state.set_value(new_text, window, cx);
                let total_lines = input_state.text().lines_len();
                Ok::<usize, String>(total_lines)
            }) {
                Ok(Ok(total_lines)) => {
                    let message = match operation {
                        WriteOperation::ReplaceAll => "Content written to tab".to_string(),
                        WriteOperation::InsertBeforeLine => {
                            format!("Inserted before line {}", start_line.unwrap_or(0))
                        }
                        WriteOperation::ReplaceLines => {
                            format!(
                                "Replaced lines {}-{}",
                                start_line.unwrap_or(0),
                                end_line.unwrap_or(0)
                            )
                        }
                    };

                    ToolCall {
                        id: "write-tab".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "write-tab".to_string(),
                            arguments: serde_json::json!({
                                "success": true,
                                "message": message,
                                "total_lines": total_lines,
                            })
                            .to_string(),
                        },
                    }
                }
                Ok(Err(e)) => ToolCall {
                    id: "write-tab".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "write-tab".to_string(),
                        arguments:
                            serde_json::json!({"error": format!("Failed to write tab: {}", e)})
                                .to_string(),
                    },
                },
                Err(e) => ToolCall {
                    id: "write-tab".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "write-tab".to_string(),
                        arguments:
                            serde_json::json!({"error": format!("Failed to write tab: {}", e)})
                                .to_string(),
                    },
                },
            }
        } else {
            ToolCall {
                id: "write-tab".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "write-tab".to_string(),
                    arguments: serde_json::json!({"error": "No input state available. Please ensure the chat session is properly connected to a query tab."}).to_string(),
                },
            }
        }
    }

    fn as_tool(&self) -> Tool {
        let mut properties = HashMap::new();
        properties.insert(
            "operation".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "The write operation to perform. \"replace_all\" (default): replaces the entire tab content. \"insert_before_line\": inserts content before the specified line. \"replace_lines\": replaces the specified line range with new content.".to_string(),
                items: None,
                enum_list: Some(vec![
                    "replace_all".to_string(),
                    "insert_before_line".to_string(),
                    "replace_lines".to_string(),
                ]),
            },
        );
        properties.insert(
            "content".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "The SQL query content to write/insert.".to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "start_line".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "1-based line number. Required for \"insert_before_line\" and \"replace_lines\" operations.".to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "end_line".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "1-based ending line number (inclusive). Required for \"replace_lines\" operation.".to_string(),
                items: None,
                enum_list: None,
            },
        );

        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "write-tab".to_string(),
                description: "Write content to the current query tab. Supports three modes: replace all content (default), insert before a specific line, or replace a specific range of lines.".to_string(),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec!["content".to_string()],
                }).unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolCall) -> String {
        let operation = arguments
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("replace_all");
        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let preview = if content.len() > 40 {
            format!("{}...", &content[..40])
        } else {
            content.to_string()
        };

        match operation {
            "insert_before_line" => {
                let line = arguments
                    .get("start_line")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                format!("Insert before line {}: {}", line, preview)
            }
            "replace_lines" => {
                let start = arguments
                    .get("start_line")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let end = arguments
                    .get("end_line")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                format!("Replace lines {}-{}: {}", start, end, preview)
            }
            _ => format!("Write Tab: {}", preview),
        }
    }
}

const EXECUTE_SQL_MAX_ROWS: usize = 100;
const EXECUTE_SQL_AUTO_LIMIT: usize = 100;

fn contains_drop_statement(sql: &str) -> bool {
    sql.to_uppercase().contains("DROP ")
}

fn needs_auto_limit(sql: &str) -> bool {
    let upper = sql.to_uppercase();
    let trimmed = upper.trim();
    if !trimmed.starts_with("SELECT") {
        return false;
    }
    !upper.contains("LIMIT")
}

pub struct ExecuteSqlHandler;

#[async_trait(?Send)]
impl AgentToolHandler for ExecuteSqlHandler {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        _cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        let sql = match arguments.get("sql").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return ToolCall {
                    id: "execute-sql".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "execute-sql".to_string(),
                        arguments: serde_json::json!({"error": "Missing required parameter: sql"})
                            .to_string(),
                    },
                };
            }
        };

        if contains_drop_statement(&sql) {
            return ToolCall {
                id: "execute-sql".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "execute-sql".to_string(),
                    arguments: serde_json::json!({
                        "error": "DROP statements are not allowed. This tool is for querying and modifying data, not altering database structure."
                    })
                    .to_string(),
                },
            };
        }

        let connection_id = match context.connection_id {
            Some(id) => id,
            None => {
                return ToolCall {
                    id: "execute-sql".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "execute-sql".to_string(),
                        arguments: serde_json::json!({
                            "error": "No database connection available. Please connect to a database first."
                        })
                        .to_string(),
                    },
                };
            }
        };

        let mut sql = sql;
        if needs_auto_limit(&sql) {
            sql = format!(
                "{}\nLIMIT {}",
                sql.trim_end().trim_end_matches(';'),
                EXECUTE_SQL_AUTO_LIMIT
            );
        }

        match context
            .db_service
            .execute_query(connection_id, context.database_name.as_deref(), &sql)
            .await
        {
            Ok(result) => {
                let truncated = result.rows.len() > EXECUTE_SQL_MAX_ROWS;
                let error_message = if result.is_error {
                    result.rows.first().map(|first_row| {
                        first_row
                            .iter()
                            .filter_map(|v| v.as_deref())
                            .collect::<Vec<&str>>()
                            .join(" ")
                    })
                } else {
                    None
                };
                let display_rows: Vec<_> =
                    result.rows.into_iter().take(EXECUTE_SQL_MAX_ROWS).collect();

                let mut json_result = serde_json::json!({
                    "columns": result.columns,
                    "rows": display_rows,
                    "rows_affected": result.rows_affected,
                    "execution_time_ms": result.execution_time_ms,
                    "truncated": truncated,
                });

                if truncated {
                    json_result["hint"] = serde_json::json!(format!(
                        "Result truncated to {} rows. Add a LIMIT clause or WHERE filter to narrow results.",
                        EXECUTE_SQL_MAX_ROWS
                    ));
                }

                if result.is_error {
                    json_result["error"] = serde_json::json!(true);
                    if let Some(msg) = error_message {
                        json_result["error_message"] = serde_json::json!(msg);
                    }
                }

                ToolCall {
                    id: "execute-sql".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "execute-sql".to_string(),
                        arguments: json_result.to_string(),
                    },
                }
            }
            Err(e) => ToolCall {
                id: "execute-sql".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "execute-sql".to_string(),
                    arguments:
                        serde_json::json!({"error": format!("Query execution failed: {}", e)})
                            .to_string(),
                },
            },
        }
    }

    fn as_tool(&self) -> Tool {
        let mut properties = HashMap::new();
        properties.insert(
            "sql".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "The SQL query to execute. SELECT queries without a LIMIT clause will have LIMIT 100 appended automatically. DROP statements are not allowed.".to_string(),
                items: None,
                enum_list: None,
            },
        );

        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "execute-sql".to_string(),
                description: "Execute a SQL query against the connected database and return results as JSON. Results are truncated to 100 rows. DROP statements are blocked. SELECT queries without LIMIT will have LIMIT 100 added automatically.".to_string(),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec!["sql".to_string()],
                })
                .unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, _arguments: &serde_json::Value, _result: &ToolCall) -> String {
        "Execute SQL".to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ToolMode {
    #[default]
    Ask,
    AllowAll,
}

/// Tool registry for agent
pub struct AgentToolRegistry {
    handlers: HashMap<String, Box<dyn AgentToolHandler>>,
}

impl AgentToolRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
        };

        registry.register(Box::new(ExploreTablesHandler));
        registry.register(Box::new(ListTablesHandler));
        registry.register(Box::new(ReadTabHandler));
        registry.register(Box::new(WriteTabHandler));
        registry.register(Box::new(ExecuteSqlHandler));

        registry
    }

    pub fn register(&mut self, handler: Box<dyn AgentToolHandler>) {
        let name = handler.as_tool().function.name.clone();
        self.handlers.insert(name, handler);
    }

    /// Get llm tools for use with llm crate
    pub fn get_llm_tools(&self) -> Vec<Tool> {
        self.handlers.values().map(|h| h.as_tool()).collect()
    }

    pub fn call_preview(&self, tool_call: &ToolCall) -> String {
        let handler = match self.handlers.get(&tool_call.function.name) {
            Some(h) => h,
            None => return format!("Unknown tool: {}", tool_call.function.name),
        };

        let arguments: serde_json::Value = match serde_json::from_str(&tool_call.function.arguments)
        {
            Ok(args) => args,
            Err(_) => return format!("{} (invalid arguments)", tool_call.function.name),
        };

        let placeholder_result = ToolCall {
            id: tool_call.id.clone(),
            call_type: tool_call.call_type.clone(),
            function: FunctionCall {
                name: tool_call.function.name.clone(),
                arguments: "{}".to_string(),
            },
        };

        handler.call_summary(&arguments, &placeholder_result)
    }

    pub fn always_allow(&self, tool_name: &str) -> bool {
        self.handlers
            .get(tool_name)
            .map(|h| h.always_allow())
            .unwrap_or(false)
    }

    /// Execute a tool and return both the result and a human-readable summary
    pub async fn execute_tool_with_summary(
        &self,
        tool_call: &ToolCall,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> (ToolCall, String) {
        let handler = self.handlers.get(&tool_call.function.name);

        match handler {
            Some(handler) => {
                // Parse arguments
                let arguments: serde_json::Value = match serde_json::from_str(
                    &tool_call.function.arguments,
                ) {
                    Ok(args) => args,
                    Err(err) => {
                        let error_result = ToolCall {
                                id: tool_call.id.clone(),
                                call_type: tool_call.call_type.clone(),
                                function: FunctionCall {
                                    name: tool_call.function.name.clone(),
                                    arguments: serde_json::json!({"error": format!("Invalid JSON arguments: {}", err)}).to_string(),
                                },
                            };
                        return (error_result, format!("Error: {}", err));
                    }
                };

                // Execute tool
                let mut result = handler.execute(arguments.clone(), context, cx).await;

                // Preserve the original tool call id for result matching
                result.id = tool_call.id.clone();

                // Generate summary
                let summary = handler.call_summary(&arguments, &result);

                (result, summary)
            }
            None => {
                let error_msg = format!("Unknown tool: {}", tool_call.function.name);
                let error_result = ToolCall {
                    id: tool_call.id.clone(),
                    call_type: tool_call.call_type.clone(),
                    function: FunctionCall {
                        name: tool_call.function.name.clone(),
                        arguments: serde_json::json!({"error": &error_msg}).to_string(),
                    },
                };
                (error_result, error_msg)
            }
        }
    }

    pub async fn _execute_tool(
        &self,
        tool_call: &ToolCall,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        self.execute_tool_with_summary(tool_call, context, cx)
            .await
            .0
    }
}

impl Default for AgentToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
