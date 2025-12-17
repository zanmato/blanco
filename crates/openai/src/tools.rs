use blanco_core::chat_provider::{FunctionDefinition, ToolCall, ToolDefinition, ToolResult};
use blanco_core::{Connection, DatabaseService};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Connection context for tool execution
#[derive(Clone)]
pub struct ConnectionContext {
    /// Database service for connection resolution
    pub database_service: Option<Arc<dyn DatabaseService>>,
    /// The database connection ID
    pub connection_id: Option<i64>,
    /// The current database name (for connections that support multiple databases)
    pub database_name: Option<String>,
}

impl ConnectionContext {
    /// Create a new empty connection context
    pub fn new() -> Self {
        Self {
            database_service: None,
            connection_id: None,
            database_name: None,
        }
    }

    /// Create a connection context with database service and connection ID
    pub fn with_database_service(
        database_service: Arc<dyn DatabaseService>,
        connection_id: i64,
    ) -> Self {
        Self {
            database_service: Some(database_service),
            connection_id: Some(connection_id),
            database_name: None,
        }
    }

    /// Create a connection context with database service, connection ID, and database name
    pub fn with_database_service_and_database(
        database_service: Arc<dyn DatabaseService>,
        connection_id: i64,
        database_name: String,
    ) -> Self {
        Self {
            database_service: Some(database_service),
            connection_id: Some(connection_id),
            database_name: Some(database_name),
        }
    }

    /// Check if the context has a database service
    pub fn has_database_service(&self) -> bool {
        self.database_service.is_some()
    }

    /// Get a reference to the database service
    pub fn database_service(&self) -> Option<&Arc<dyn DatabaseService>> {
        self.database_service.as_ref()
    }

    /// Get the connection ID
    pub fn connection_id(&self) -> Option<i64> {
        self.connection_id
    }

    /// Get the database name
    pub fn database_name(&self) -> Option<&str> {
        self.database_name.as_deref()
    }

    /// Get a connection asynchronously
    pub async fn get_connection(&self) -> Result<Option<Arc<dyn Connection>>, anyhow::Error> {
        if let (Some(db_service), Some(conn_id)) = (&self.database_service, self.connection_id) {
            let database_name_ref = self.database_name.as_deref();
            Ok(Some(db_service.get_or_create_connection_by_id(conn_id, database_name_ref).await?))
        } else {
            Ok(None)
        }
    }
}

impl Default for ConnectionContext {
    fn default() -> Self {
        Self::new()
    }
}

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
    /// Execute the tool with given arguments and connection context
    async fn execute_with_context(
        &self,
        arguments: Value,
        context: ConnectionContext,
    ) -> ToolResult;

    /// Get the tool definition
    fn definition(&self) -> ToolDefinition;

    /// Generate a human-readable summary of the tool call
    fn call_summary(&self, arguments: &Value, result: &ToolResult) -> String;
}

/// A tool executor that can run tools
pub struct ToolExecutor {
    handlers: HashMap<String, Box<dyn ToolHandler>>,
    context_provider: Option<Box<dyn Fn() -> ConnectionContext + Send + Sync>>,
}

impl ToolExecutor {
    /// Create a new tool executor
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            context_provider: None,
        }
    }

    /// Create a new tool executor with a context provider
    pub fn with_context_provider<F>(provider: F) -> Self
    where
        F: Fn() -> ConnectionContext + Send + Sync + 'static,
    {
        Self {
            handlers: HashMap::new(),
            context_provider: Some(Box::new(provider)),
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

                // Get the context if provider is available
                let context = if let Some(provider) = &self.context_provider {
                    provider()
                } else {
                    ConnectionContext::new()
                };

                // Execute the tool with context
                let mut result = handler
                    .execute_with_context(arguments.clone(), context)
                    .await;

                // Add human-readable summary to the result
                result.summary = Some(handler.call_summary(&arguments, &result));

                result
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
    // This tool now relies on the ToolExecutor's connection resolver
    // We can remove the connection_id_resolver since the connection is provided
    // through the execute_with_connection method
}

impl Default for ListTablesTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ListTablesTool {
    /// Create a new list-tables tool
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl ToolHandler for ListTablesTool {
    async fn execute_with_context(
        &self,
        arguments: Value,
        context: ConnectionContext,
    ) -> ToolResult {
        tracing::debug!(
            "ListTablesTool execute_with_context called with arguments: {}",
            arguments
        );

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

        tracing::debug!(
            "Extracted parameters - table_names: {:?}, limit: {}, offset: {}",
            table_names,
            limit,
            offset
        );

        // Try to get connection from context
        match context.get_connection().await {
            Ok(Some(conn)) => {
                tracing::debug!(
                    "Using connection for ListTablesTool: {} ({})",
                    conn.get_display_name(),
                    conn.get_connection_type()
                );

                // Use the connection to query actual database schema with pagination
                let table_names_ref = table_names.as_deref();
                let database_name = context.database_name();

                match conn
                    .get_database_schema_paginated(
                        table_names_ref,
                        Some(limit),
                        Some(offset),
                    )
                    .await
                {
                    Ok(result) => {
                        // Convert the DatabaseSchemaResult to JSON with database context
                        let json_result = serde_json::json!({
                            "connection_type": result.connection_type,
                            "display_name": result.display_name,
                            "database_name": database_name,
                            "tables": result.tables,
                            "pagination": result.pagination
                        });

                        ToolResult::success(
                            "list-tables",
                            serde_json::to_string_pretty(&json_result)
                                .unwrap_or_else(|_| "Invalid JSON result".to_string()),
                        )
                    },
                    Err(e) => {
                        tracing::error!(
                            "Failed to query database schema: {}",
                            e
                        );
                        ToolResult::error(
                            "list-tables",
                            format!("Failed to query database schema: {}", e),
                        )
                    }
                }
            },
            Ok(None) => {
                tracing::warn!("No connection available for ListTablesTool");
                ToolResult::error(
                    "list-tables",
                    "No database connection available. Please connect to a database first.",
                )
            },
            Err(e) => {
                tracing::error!(
                    "Failed to get database connection: {}",
                    e
                );
                ToolResult::error(
                    "list-tables",
                    format!("Failed to get database connection: {}", e),
                )
            }
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

    /// Generate a human-readable summary of the tool call
    fn call_summary(&self, arguments: &Value, _result: &ToolResult) -> String {
        // Extract table_names from arguments for summary
        let table_names = arguments
            .get("table_names")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if table_names.is_empty() {
            "List Tables".to_string()
        } else {
            // Parse comma-separated table names and truncate if too long
            let names: Vec<&str> = table_names.split(',').map(|s| s.trim()).collect();
            let summary = if names.len() > 3 {
                format!("List Tables: {}, ...", names[0..3].join(", "))
            } else {
                format!("List Tables: {}", names.join(", "))
            };
            summary
        }
    }
}

/// Read tab tool for reading current query tab content
pub struct ReadTabTool {
    tab_content_resolver: Option<Box<dyn Fn() -> String + Send + Sync>>,
}

impl Default for ReadTabTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadTabTool {
    pub fn new() -> Self {
        Self {
            tab_content_resolver: None,
        }
    }

    /// Create a new read tab tool with a tab content resolver
    pub fn with_tab_resolver<F>(resolver: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        Self {
            tab_content_resolver: Some(Box::new(resolver)),
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for ReadTabTool {
    async fn execute_with_context(
        &self,
        _arguments: Value,
        _context: ConnectionContext,
    ) -> ToolResult {
        tracing::debug!("ReadTabTool execute_with_context called");

        // Get tab content using the resolver or return a default message
        let tab_content = if let Some(resolver) = &self.tab_content_resolver {
            resolver()
        } else {
            "No tab content resolver available. Please ensure the chat session is properly connected to a query tab.".to_string()
        };

        ToolResult {
            tool_call_id: "read-tab".to_string(),
            success: true,
            content: tab_content,
            error: None,
            summary: None,
        }
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read-tab".to_string(),
                description: "Read the current query tab content including the SQL query text. Returns the SQL content from the active tab connected to this chat session.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        }
    }

    /// Generate a human-readable summary of the tool call
    fn call_summary(&self, _arguments: &Value, _result: &ToolResult) -> String {
        "Read Tab".to_string()
    }
}
