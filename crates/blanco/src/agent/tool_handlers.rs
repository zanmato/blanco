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
use llm::{
    FunctionCall, ToolCall, chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema,
    chat::Tool,
};

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
                    arguments: serde_json::json!({
                        "error": "No database connection available. Please connect to a database first."
                    })
                    .to_string(),
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
                    Ok(result) => {
                        let json_result = serde_json::json!({
                            "connection_type": result.connection_type,
                            "display_name": result.display_name,
                            "tables": result.tables,
                            "pagination": result.pagination
                        });

                        ToolCall {
                            id: "list-tables".to_string(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: "list-tables".to_string(),
                                arguments: serde_json::to_string(&json_result)
                                    .unwrap_or_else(|_| "Invalid JSON result".to_string()),
                            },
                        }
                    }
                    Err(e) => ToolCall {
                        id: "list-tables".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "list-tables".to_string(),
                            arguments: serde_json::json!({"error": format!("Failed to query database schema: {}", e)}).to_string(),
                        },
                    },
                }
            }
            Err(e) => ToolCall {
                id: "list-tables".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "list-tables".to_string(),
                    arguments: serde_json::json!({"error": format!("Failed to get database connection: {}", e)}).to_string(),
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
                description: "List detailed schema information for user-created tables. Returns object type, columns, constraints, indexes, triggers, owner, and comment as JSON. Supports pagination with limit and offset parameters.".to_string(),
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
                description: "1-based starting line number to read from. Defaults to line 1.".to_string(),
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
                description: format!("Read the current query tab content with optional line range. Returns line-numbered content. If the tab has more than {} lines and no range is specified, returns only the first {} lines with a hint to read more.", READ_TAB_DEFAULT_LIMIT, READ_TAB_DEFAULT_LIMIT),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec![],
                }).unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolCall) -> String {
        let start = arguments
            .get("start_line")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let end = arguments
            .get("end_line")
            .and_then(|v| v.as_i64());
        match end {
            Some(end) => format!("Read Tab: lines {}-{}", start, end),
            None => "Read Tab".to_string(),
        }
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
                let new_text = apply_line_operation(&current, operation, &content, start_line, end_line)?;
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
                        arguments: serde_json::json!({"error": format!("Failed to write tab: {}", e)})
                            .to_string(),
                    },
                },
                Err(e) => ToolCall {
                    id: "write-tab".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "write-tab".to_string(),
                        arguments: serde_json::json!({"error": format!("Failed to write tab: {}", e)})
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

/// Tool mode for read/write operations
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum ToolMode {
    #[default]
    Read,
    Write,
}

/// Tool registry for agent
pub struct AgentToolRegistry {
    handlers: HashMap<String, Box<dyn AgentToolHandler>>,
}

impl AgentToolRegistry {
    pub fn new() -> Self {
        Self::with_mode(ToolMode::Read)
    }

    pub fn with_mode(mode: ToolMode) -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
        };

        // Register default tools (always available)
        registry.register(Box::new(ListTablesHandler));
        registry.register(Box::new(ReadTabHandler));

        // Register WriteTabHandler only in Write mode
        if mode == ToolMode::Write {
            registry.register(Box::new(WriteTabHandler));
        }

        registry
    }

    pub fn _set_mode(&mut self, mode: ToolMode) {
        // Remove or add write-tab handler based on mode
        if mode == ToolMode::Write {
            if !self.handlers.contains_key("write-tab") {
                self.register(Box::new(WriteTabHandler));
            }
        } else {
            self.handlers.remove("write-tab");
        }
    }

    pub fn register(&mut self, handler: Box<dyn AgentToolHandler>) {
        let name = handler.as_tool().function.name.clone();
        self.handlers.insert(name, handler);
    }

    /// Get llm tools for use with llm crate
    pub fn get_llm_tools(&self) -> Vec<Tool> {
        self.handlers.values().map(|h| h.as_tool()).collect()
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
