use async_trait::async_trait;
use blanco_core::chat_provider::{FunctionDefinition, ToolCall, ToolDefinition, ToolResult};
use blanco_core::connection_trait::{Connection, ConnectionRegistry};
use blanco_core::DatabaseService;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// A registry of available tools/functions
#[derive(Clone, Debug)]
pub struct ToolRegistry {
    tools: HashMap<String, ToolDefinition>,
}

impl ToolRegistry {
    /// Create a new empty tool registry
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Add a tool to the registry
    pub fn add_tool(&mut self, tool: ToolDefinition) {
        let name = tool.function.name.clone();
        self.tools.insert(name, tool);
    }

    /// Add multiple tools to the registry
    pub fn add_tools(&mut self, tools: Vec<ToolDefinition>) {
        for tool in tools {
            self.add_tool(tool);
        }
    }

    /// Get a tool by name
    pub fn get_tool(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.get(name)
    }

    /// Get all tools
    pub fn get_all_tools(&self) -> Vec<&ToolDefinition> {
        self.tools.values().collect()
    }

    /// Get all tools as owned values (for API calls)
    pub fn get_tools(&self) -> Vec<ToolDefinition> {
        self.tools.values().cloned().collect()
    }

    /// Remove a tool by name
    pub fn remove_tool(&mut self, name: &str) -> Option<ToolDefinition> {
        self.tools.remove(name)
    }

    /// Check if a tool exists
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Get the number of registered tools
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Clear all tools
    pub fn clear(&mut self) {
        self.tools.clear();
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A trait for tool handlers
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    /// Execute the tool with given arguments and optional database service
    async fn execute_with_db(
        &self,
        arguments: Value,
        database_service: Option<Arc<dyn DatabaseService>>,
    ) -> ToolResult;

    /// Get the tool definition
    fn definition(&self) -> ToolDefinition;
}

/// A tool executor that can run tools
pub struct ToolExecutor {
    handlers: HashMap<String, Box<dyn ToolHandler>>,
    database_service: Option<Arc<dyn DatabaseService>>,
}

impl ToolExecutor {
    /// Create a new tool executor
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            database_service: None,
        }
    }

    /// Create a new tool executor with a database service
    pub fn with_database_service(database_service: Arc<dyn DatabaseService>) -> Self {
        Self {
            handlers: HashMap::new(),
            database_service: Some(database_service),
        }
    }

    /// Register a tool handler
    pub fn register_tool(&mut self, tool: Box<dyn ToolHandler>) {
        let name = tool.definition().function.name.clone();
        self.handlers.insert(name, tool);
    }

    /// Register a tool handler by name
    pub fn register_tool_with_name(&mut self, name: String, tool: Box<dyn ToolHandler>) {
        self.handlers.insert(name, tool);
    }

    /// Execute a tool call
    pub async fn execute_tool_call(&self, tool_call: &ToolCall) -> ToolResult {
        let handler = self.handlers.get(&tool_call.function.name);

        match handler {
            Some(handler) => {
                // Parse the arguments
                let arguments: Value = match serde_json::from_str(&tool_call.function.arguments) {
                    Ok(args) => args,
                    Err(err) => {
                        return ToolResult::error(
                            tool_call.id.clone(),
                            format!("Invalid JSON arguments: {}", err),
                        );
                    }
                };

                // Execute the tool with database service if available
                handler
                    .execute_with_db(arguments, self.database_service.clone())
                    .await
            }
            None => ToolResult::error(
                tool_call.id.clone(),
                format!("Unknown tool: {}", tool_call.function.name),
            ),
        }
    }

    /// Execute multiple tool calls
    pub async fn execute_tool_calls(&self, tool_calls: &[ToolCall]) -> Vec<ToolResult> {
        let mut results = Vec::with_capacity(tool_calls.len());

        for tool_call in tool_calls {
            let result = self.execute_tool_call(tool_call).await;
            results.push(result);
        }

        results
    }

    /// Get all registered tool definitions
    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.handlers
            .values()
            .map(|handler| handler.definition())
            .collect()
    }

    /// Check if a tool is registered
    pub fn has_tool(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Get the number of registered tools
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the executor is empty
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// List-tables tool for database schema exploration
pub struct ListTablesTool {
    connection_id_resolver: Option<Box<dyn Fn() -> Option<i64> + Send + Sync>>,
}

impl ListTablesTool {
    /// Create a new list-tables tool
    pub fn new() -> Self {
        Self {
            connection_id_resolver: None,
        }
    }

    /// Create a new list-tables tool with a connection od
    pub fn with_connection_id(connection_id: i64) -> Self {
        Self {
            connection_id_resolver: Some(Box::new(move || Some(connection_id))),
        }
    }

    /// Create a new list-tables tool with connection id resolver
    pub fn with_connection_resolver<F>(resolver: F) -> Self
    where
        F: Fn() -> Option<i64> + Send + Sync + 'static,
    {
        Self {
            connection_id_resolver: Some(Box::new(resolver)),
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for ListTablesTool {
    async fn execute_with_db(
        &self,
        arguments: Value,
        database_service: Option<Arc<dyn DatabaseService>>,
    ) -> ToolResult {
        log::debug!(
            "ListTablesTool execute_with_db called with arguments: {}",
            arguments
        );

        // Extract parameters
        let table_names = arguments
            .get("table_names")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let output_format = arguments
            .get("output_format")
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

        log::debug!(
            "Extracted parameters - table_names: {:?}, output_format: {:?}, limit: {}, offset: {}",
            table_names,
            output_format,
            limit,
            offset
        );

        // Try to get connection string from various sources
        let connection_id = if let Some(resolver) = &self.connection_id_resolver {
            resolver()
        } else {
            None
        };

        if let Some(conn_id) = connection_id {
            if let Some(db_service) = &database_service {
                log::debug!(
                    "Using database service for ListTablesTool with connection: {}",
                    conn_id
                );

                // Use the database service to query actual database schema with pagination
                let table_names_ref = table_names.as_deref();
                match db_service
                    .get_database_schema_paginated(
                        conn_id,
                        table_names_ref,
                        Some(limit),
                        Some(offset),
                    )
                    .await
                {
                    Ok(result) => ToolResult::success(
                        "list-tables",
                        serde_json::to_string_pretty(&result)
                            .unwrap_or_else(|_| "Invalid JSON result".to_string()),
                    ),
                    Err(e) => {
                        log::error!(
                            "Failed to query database schema via database service: {}",
                            e
                        );
                        ToolResult::error(
                            "list-tables",
                            format!("Failed to query database schema: {}", e),
                        )
                    }
                }
            } else {
                ToolResult::error(
                    "list-tables",
                    "No database connection available. Please connect to a database first.",
                )
            }
        } else {
            log::warn!("No connection string available for ListTablesTool");
            ToolResult::error(
                "list-tables",
                "No database connection available. Please connect to a database first.",
            )
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "list-tables".to_string(),
                description: "List detailed schema information for user-created tables. Returns object type, columns, constraints, indexes, triggers, owner, and comment as JSON. Supports pagination with limit and offset parameters.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "table_names": {
                            "type": "string",
                            "description": "Optional comma-separated list of table names to filter. If not provided, lists all tables in user schemas. Wildcard (%) is not supported."
                        },
                        "output_format": {
                            "type": "string",
                            "enum": ["simple", "detailed"],
                            "description": "Optional output format. 'simple' returns only table names, 'detailed' returns full table information. Default: 'detailed'."
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
            },
        }
    }
}

impl ListTablesTool {
    /// Query database schema using the connection string (fallback method)
    async fn query_database_schema(
        &self,
        connection_string: String,
    ) -> Result<serde_json::Value, anyhow::Error> {
        log::warn!(
            "Fallback method used - no database service available for connection: {}",
            connection_string
        );

        // Return a simple error result since we can't create connections without a database service
        Err(anyhow::anyhow!(
            "Database service not available - cannot query schema for: {}",
            connection_string
        ))
    }
}
