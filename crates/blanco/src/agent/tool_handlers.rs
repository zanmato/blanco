//! Tool handlers for agent-level tool execution with GPUI context access.
//!
//! This module provides tool handlers that can access GPUI entities and globals,
//! decoupling tool execution from the LLM provider implementations.

use async_trait::async_trait;
use gpui::{AsyncWindowContext, WeakEntity};
use gpui_component::input::InputState;
use std::collections::HashMap;
use std::sync::Arc;

use blanco_core::DatabaseService;
use blanco_core::chat_provider::{ToolCall, ToolDefinition, ToolResult};

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
    ) -> ToolResult;

    fn definition(&self) -> ToolDefinition;

    fn call_summary(&self, arguments: &serde_json::Value, result: &ToolResult) -> String;
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
    ) -> ToolResult {
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
            return ToolResult::error(
                "list-tables",
                "No database connection available. Please connect to a database first.",
            );
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

                        ToolResult::success(
                            "list-tables",
                            serde_json::to_string_pretty(&json_result)
                                .unwrap_or_else(|_| "Invalid JSON result".to_string()),
                        )
                    }
                    Err(e) => {
                        tracing::info!("YOLOOOOOOOOOOOOOO {}", e);
                        ToolResult::error(
                            "list-tables",
                            format!("Failed to query database schema: {}", e),
                        )
                    }
                }
            }
            Err(e) => ToolResult::error(
                "list-tables",
                format!("Failed to get database connection: {}", e),
            ),
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "list-tables",
            "List detailed schema information for user-created tables. Returns object type, columns, constraints, indexes, triggers, owner, and comment as JSON. Supports pagination with limit and offset parameters.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "table_names": {
                        "type": "string",
                        "description": "Optional comma-separated list of table names to filter. If not provided, lists all tables in user schemas. Wildcard (%) is not supported."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of tables to return. Default: 20.",
                        "minimum": 1,
                        "maximum": 100
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Number of tables to skip for pagination. Default: 0.",
                        "minimum": 0
                    }
                },
                "required": []
            }),
        )
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolResult) -> String {
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

/// Read tab tool handler
pub struct ReadTabHandler;

#[async_trait(?Send)]
impl AgentToolHandler for ReadTabHandler {
    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolResult {
        if let Some(input_state) = &context.input_state {
            // Read from input state entity
            match input_state.read_with(cx, |input, _cx| input.text().to_string()) {
                Ok(content) => ToolResult::success("read-tab", content),
                Err(e) => {
                    ToolResult::error("read-tab", format!("Failed to read tab content: {}", e))
                }
            }
        } else {
            ToolResult::error(
                "read-tab",
                "No input state available. Please ensure the chat session is properly connected to a query tab.",
            )
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "read-tab",
            "Read the current query tab content including the SQL query text. Returns the SQL content from the active tab connected to this chat session.",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        )
    }

    fn call_summary(&self, _arguments: &serde_json::Value, _result: &ToolResult) -> String {
        "Read Tab".to_string()
    }
}

/// Write tab tool handler
pub struct WriteTabHandler;

#[async_trait(?Send)]
impl AgentToolHandler for WriteTabHandler {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolResult {
        if let Some(input_state) = &context.input_state {
            // Extract content from arguments
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let _ = input_state.update_in(cx, |input_state, window, cx| {
                input_state.set_value(content, window, cx);
            });

            ToolResult::success("write-tab", "Content written to tab")
        } else {
            ToolResult::error(
                "write-tab",
                "No input state available. Please ensure the chat session is properly connected to a query tab.",
            )
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "write-tab",
            "Write/update the current query tab content with the provided SQL query. This replaces the entire tab content with the new query.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "The SQL query content to write to the tab."
                    }
                },
                "required": ["content"]
            }),
        )
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolResult) -> String {
        if let Some(content) = arguments.get("content").and_then(|v| v.as_str()) {
            let preview = if content.len() > 50 {
                format!("{}...", &content[..50])
            } else {
                content.to_string()
            };
            format!("Write Tab: {}", preview)
        } else {
            "Write Tab".to_string()
        }
    }
}

/// Tool mode for read/write operations
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolMode {
    Read,
    Write,
}

impl Default for ToolMode {
    fn default() -> Self {
        Self::Read
    }
}

/// Tool registry for agent
pub struct AgentToolRegistry {
    handlers: HashMap<String, Box<dyn AgentToolHandler>>,
    #[allow(dead_code)]
    mode: ToolMode,
}

impl AgentToolRegistry {
    pub fn new() -> Self {
        Self::with_mode(ToolMode::Read)
    }

    pub fn with_mode(mode: ToolMode) -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
            mode,
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

    pub fn set_mode(&mut self, mode: ToolMode) {
        self.mode = mode;

        // Remove or add write-tab handler based on mode
        if mode == ToolMode::Write {
            if !self.handlers.contains_key("write-tab") {
                self.register(Box::new(WriteTabHandler));
            }
        } else {
            self.handlers.remove("write-tab");
        }
    }

    pub fn mode(&self) -> ToolMode {
        self.mode
    }

    pub fn register(&mut self, handler: Box<dyn AgentToolHandler>) {
        let name = handler.definition().function.name.clone();
        self.handlers.insert(name, handler);
    }

    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.handlers
            .values()
            .map(|handler| handler.definition())
            .collect()
    }

    pub async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolResult {
        let handler = self.handlers.get(&tool_call.function.name);

        match handler {
            Some(handler) => {
                // Parse arguments
                let arguments: serde_json::Value =
                    match serde_json::from_str(&tool_call.function.arguments) {
                        Ok(args) => args,
                        Err(err) => {
                            return ToolResult::error(
                                tool_call.id.clone(),
                                format!("Invalid JSON arguments: {}", err),
                            );
                        }
                    };

                // Execute tool
                let mut result = handler.execute(arguments, context, cx).await;

                // Add summary
                result.summary = Some(handler.call_summary(
                    &serde_json::from_str(&tool_call.function.arguments).unwrap_or_default(),
                    &result,
                ));

                result
            }
            None => ToolResult::error(
                tool_call.id.clone(),
                format!("Unknown tool: {}", tool_call.function.name),
            ),
        }
    }
}

impl Default for AgentToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
