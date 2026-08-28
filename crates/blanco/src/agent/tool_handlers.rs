//! Tool handlers for agent-level tool execution with GPUI context access.
//!
//! This module provides tool handlers that can access GPUI entities and globals,
//! decoupling tool execution from the LLM provider implementations.

use async_trait::async_trait;
use gpui::{AsyncWindowContext, WeakEntity};
use gpui_component::input::EditorState;
use std::collections::HashMap;
use std::sync::Arc;

use blanco_core::DatabaseService;
use database::DatabaseType;
use llm::{FunctionCall, ToolCall, chat::Tool};

use super::chat_session::TabLanguage;

mod execute_sql;
mod explore_tables;
mod list_tables;
mod markdown;
mod read_tab;
mod write_tab;

use execute_sql::ExecuteSqlHandler;
use explore_tables::ExploreTablesHandler;
use list_tables::ListTablesHandler;
use read_tab::ReadTabHandler;
use write_tab::WriteTabHandler;

/// What the tab the tools read and write actually holds, in the words the
/// tool descriptions use. A script tab is JavaScript regardless of the
/// connection; a query tab follows the backend.
pub fn tab_content_noun(tab_language: TabLanguage, db_type: Option<DatabaseType>) -> &'static str {
    match tab_language {
        TabLanguage::Script => "JavaScript script",
        TabLanguage::Query => match db_type {
            Some(db_type) if !db_type.supports_sql() => "Redis command",
            _ => "SQL query",
        },
    }
}

/// Context for executing tools with GPUI/database access
pub struct ToolContext {
    pub db_service: Arc<dyn DatabaseService>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
    /// Backend of the tab's connection, `None` when the tab has none yet.
    pub db_type: Option<DatabaseType>,
    pub input_state: Option<WeakEntity<EditorState>>,
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
    pub fn new(tab_language: TabLanguage, db_type: Option<DatabaseType>) -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
        };

        let content_noun = tab_content_noun(tab_language, db_type);

        registry.register(Box::new(ExploreTablesHandler));
        registry.register(Box::new(ListTablesHandler));
        registry.register(Box::new(ReadTabHandler { content_noun }));
        registry.register(Box::new(WriteTabHandler { content_noun }));
        registry.register(Box::new(ExecuteSqlHandler));

        registry
    }

    pub fn register(&mut self, handler: Box<dyn AgentToolHandler>) {
        let name = handler.as_tool().function.name;
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
        Self::new(TabLanguage::default(), None)
    }
}
