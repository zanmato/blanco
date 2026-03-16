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
use crate::result_ext::ResultExt;
use llm::{chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema, chat::Tool, FunctionCall, ToolCall};

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

/// Read tab tool handler
pub struct ReadTabHandler;

#[async_trait(?Send)]
impl AgentToolHandler for ReadTabHandler {
    async fn execute(
        &self,
        _arguments: serde_json::Value,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        if let Some(input_state) = &context.input_state {
            // Read from input state entity
            match input_state.read_with(cx, |input, _cx| input.text().to_string()) {
                Ok(content) => ToolCall {
                    id: "read-tab".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "read-tab".to_string(),
                        arguments: serde_json::json!({"content": content}).to_string(),
                    },
                },
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
        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "read-tab".to_string(),
                description: "Read the current query tab content including the SQL query text. Returns the SQL content from the active tab connected to this chat session.".to_string(),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties: HashMap::new(),
                    required: vec![],
                }).unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, _arguments: &serde_json::Value, _result: &ToolCall) -> String {
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
    ) -> ToolCall {
        if let Some(input_state) = &context.input_state {
            // Extract content from arguments
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            input_state.update_in(cx, |input_state, window, cx| {
                input_state.set_value(content.clone(), window, cx);
            }).log_err();

            ToolCall {
                id: "write-tab".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "write-tab".to_string(),
                    arguments: serde_json::json!({"success": true, "message": "Content written to tab"}).to_string(),
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
            "content".to_string(),
            ParameterProperty {
                property_type: "string".to_string(),
                description: "The SQL query content to write to the tab.".to_string(),
                items: None,
                enum_list: None,
            },
        );

        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "write-tab".to_string(),
                description: "Write/update the current query tab content with the provided SQL query. This replaces the entire tab content with the new query.".to_string(),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec!["content".to_string()],
                }).unwrap_or_default(),
            },
        }
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolCall) -> String {
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
#[derive(Default)]
pub enum ToolMode {
    #[default]
    Read,
    Write,
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

    pub fn _set_mode(&mut self, mode: ToolMode) {
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

    pub fn _mode(&self) -> ToolMode {
        self.mode
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
                let arguments: serde_json::Value =
                    match serde_json::from_str(&tool_call.function.arguments) {
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
