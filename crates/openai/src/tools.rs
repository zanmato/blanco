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
    /// Execute the tool with given arguments
    async fn execute(&self, arguments: Value) -> ToolResult;

    /// Execute the tool with given arguments and optional database service
    async fn execute_with_db(
        &self,
        arguments: Value,
        database_service: Option<Arc<dyn DatabaseService>>,
    ) -> ToolResult {
        // Default implementation falls back to execute without database service
        self.execute(arguments).await
    }

    /// Get the tool definition
    fn definition(&self) -> ToolDefinition;
}

/// A simple tool implementation that uses a closure
pub struct ClosureTool {
    definition: ToolDefinition,
    handler: Box<dyn Fn(Value) -> ToolResult + Send + Sync>,
}

impl ClosureTool {
    /// Create a new tool from a closure
    pub fn new<F>(definition: ToolDefinition, handler: F) -> Self
    where
        F: Fn(Value) -> ToolResult + Send + Sync + 'static,
    {
        Self {
            definition,
            handler: Box::new(handler),
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for ClosureTool {
    async fn execute(&self, arguments: Value) -> ToolResult {
        (self.handler)(arguments)
    }

    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
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
    connection_string_resolver: Option<Box<dyn Fn() -> Option<String> + Send + Sync>>,
}

impl ListTablesTool {
    /// Create a new list-tables tool
    pub fn new() -> Self {
        Self {
            connection_string_resolver: None,
        }
    }

    /// Create a new list-tables tool with a connection string
    pub fn with_connection_string(connection_string: String) -> Self {
        let conn_str = connection_string.clone();
        Self {
            connection_string_resolver: Some(Box::new(move || Some(conn_str.clone()))),
        }
    }

    /// Create a new list-tables tool with connection string resolver
    pub fn with_connection_resolver<F>(resolver: F) -> Self
    where
        F: Fn() -> Option<String> + Send + Sync + 'static,
    {
        Self {
            connection_string_resolver: Some(Box::new(resolver)),
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for ListTablesTool {
    async fn execute(&self, arguments: Value) -> ToolResult {
        log::debug!(
            "ListTablesTool execute called with arguments: {}",
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

        let param_connection_string = arguments.get("connection_string").and_then(|v| v.as_str());

        log::debug!("Extracted parameters - table_names: {:?}, output_format: {:?}, connection_string: {:?}",
                   table_names, output_format, param_connection_string);

        // Try to get connection string from various sources
        let connection_string = if let Some(conn_str) = param_connection_string {
            Some(conn_str.to_string())
        } else if let Some(resolver) = &self.connection_string_resolver {
            resolver()
        } else {
            None
        };

        if let Some(conn_str) = connection_string {
            log::debug!("Using connection string for ListTablesTool: {}", conn_str);

            // Use the connection to query actual database schema
            match self.query_database_schema(conn_str).await {
                Ok(result) => ToolResult::success(
                    "list-tables",
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|_| "Invalid JSON result".to_string()),
                ),
                Err(e) => {
                    log::error!("Failed to query database schema: {}", e);
                    ToolResult::error(
                        "list-tables",
                        format!("Failed to query database schema: {}", e),
                    )
                }
            }
        } else {
            log::warn!("No connection string available for ListTablesTool");
            ToolResult::error(
                "list-tables",
                "No database connection available. Please connect to a database first.",
            )
        }
    }

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

        let param_connection_string = arguments.get("connection_string").and_then(|v| v.as_str());

        log::debug!("Extracted parameters - table_names: {:?}, output_format: {:?}, limit: {}, offset: {}, connection_string: {:?}",
                   table_names, output_format, limit, offset, param_connection_string);

        // Try to get connection string from various sources
        let connection_string = if let Some(conn_str) = param_connection_string {
            Some(conn_str.to_string())
        } else if let Some(resolver) = &self.connection_string_resolver {
            resolver()
        } else {
            None
        };

        if let Some(conn_str) = &connection_string {
            if let Some(db_service) = &database_service {
                log::debug!(
                    "Using database service for ListTablesTool with connection: {}",
                    conn_str
                );

                // Use the database service to query actual database schema with pagination
                let table_names_ref = table_names.as_deref();
                match db_service
                    .get_database_schema_paginated(
                        conn_str,
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
                log::debug!(
                    "No database service available, falling back to direct connection: {}",
                    conn_str
                );
                // Fall back to the original method for backward compatibility
                match self.query_database_schema(conn_str.clone()).await {
                    Ok(result) => ToolResult::success(
                        "list-tables",
                        serde_json::to_string_pretty(&result)
                            .unwrap_or_else(|_| "Invalid JSON result".to_string()),
                    ),
                    Err(e) => {
                        log::error!("Failed to query database schema: {}", e);
                        ToolResult::error(
                            "list-tables",
                            format!("Failed to query database schema: {}", e),
                        )
                    }
                }
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
                        "connection_string": {
                            "type": "string",
                            "description": "Database connection string to use for the query. This is automatically provided by the system."
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

/// Built-in tools for common operations
/// Create a calculator tool that can evaluate mathematical expressions
pub fn create_calculator_tool() -> ToolDefinition {
    ToolDefinition::new(
        "calculator",
        "Evaluate mathematical expressions",
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Mathematical expression to evaluate (e.g., '2 + 3 * 4')"
                }
            },
            "required": ["expression"]
        }),
    )
}

/// Execute a calculator operation
pub fn execute_calculator(arguments: Value) -> ToolResult {
    let expression = match arguments.get("expression").and_then(|v| v.as_str()) {
        Some(expr) => expr,
        None => {
            return ToolResult::error("calculator", "Missing 'expression' parameter");
        }
    };

    // Simple expression evaluation (for demonstration)
    // In a real implementation, you might use a proper expression parser
    let result = match simple_evaluate_expression(expression) {
        Ok(result) => result,
        Err(err) => {
            return ToolResult::error("calculator", format!("Evaluation error: {}", err));
        }
    };

    ToolResult::success("calculator", format!("Result: {}", result))
}

/// Simple expression evaluator (supports basic arithmetic)
fn simple_evaluate_expression(expr: &str) -> Result<f64, String> {
    // This is a very simple implementation - in practice, you'd want a proper parser
    let expr = expr.trim().replace(' ', "");

    // Handle basic operations
    if let Some(pos) = expr.find('+') {
        let left = &expr[..pos];
        let right = &expr[pos + 1..];
        return Ok(left.parse::<f64>().unwrap_or(0.0) + right.parse::<f64>().unwrap_or(0.0));
    }

    if let Some(pos) = expr.find('-') {
        let left = &expr[..pos];
        let right = &expr[pos + 1..];
        return Ok(left.parse::<f64>().unwrap_or(0.0) - right.parse::<f64>().unwrap_or(0.0));
    }

    if let Some(pos) = expr.find('*') {
        let left = &expr[..pos];
        let right = &expr[pos + 1..];
        return Ok(left.parse::<f64>().unwrap_or(1.0) * right.parse::<f64>().unwrap_or(1.0));
    }

    if let Some(pos) = expr.find('/') {
        let left = &expr[..pos];
        let right = &expr[pos + 1..];
        let right_val = right.parse::<f64>().unwrap_or(1.0);
        if right_val == 0.0 {
            return Err("Division by zero".to_string());
        }
        return Ok(left.parse::<f64>().unwrap_or(0.0) / right_val);
    }

    // Try to parse as a single number
    expr.parse::<f64>()
        .map_err(|_| format!("Cannot evaluate expression: {}", expr))
}

/// Create a date/time tool
pub fn create_datetime_tool() -> ToolDefinition {
    ToolDefinition::new(
        "datetime",
        "Get current date and time or format dates",
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["current", "format"],
                    "description": "Operation to perform"
                },
                "format": {
                    "type": "string",
                    "description": "Format string for dates (used with 'format' operation)"
                },
                "date": {
                    "type": "string",
                    "description": "Date to format (used with 'format' operation, ISO 8601 format)"
                }
            },
            "required": ["operation"]
        }),
    )
}

/// Execute a datetime operation
pub fn execute_datetime(arguments: Value) -> ToolResult {
    let operation = match arguments.get("operation").and_then(|v| v.as_str()) {
        Some(op) => op,
        None => {
            return ToolResult::error("datetime", "Missing 'operation' parameter");
        }
    };

    match operation {
        "current" => {
            let now = chrono::Utc::now();
            ToolResult::success(
                "datetime",
                format!("Current UTC time: {}", now.to_rfc3339()),
            )
        }
        "format" => {
            let date_str = match arguments.get("date").and_then(|v| v.as_str()) {
                Some(date) => date,
                None => {
                    return ToolResult::error(
                        "datetime",
                        "Missing 'date' parameter for format operation",
                    );
                }
            };

            let format_str = match arguments.get("format").and_then(|v| v.as_str()) {
                Some(format) => format,
                None => {
                    return ToolResult::error(
                        "datetime",
                        "Missing 'format' parameter for format operation",
                    );
                }
            };

            let date = match chrono::DateTime::parse_from_rfc3339(date_str) {
                Ok(date) => date,
                Err(err) => {
                    return ToolResult::error("datetime", format!("Invalid date format: {}", err));
                }
            };

            let formatted = date.format(format_str).to_string();
            ToolResult::success("datetime", format!("Formatted date: {}", formatted))
        }
        _ => ToolResult::error("datetime", format!("Unknown operation: {}", operation)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::{FunctionCall, ToolCall};

    #[test]
    fn test_tool_registry() {
        let mut registry = ToolRegistry::new();
        assert!(registry.is_empty());

        let tool = create_calculator_tool();
        registry.add_tool(tool.clone());
        assert_eq!(registry.len(), 1);
        assert!(registry.has_tool("calculator"));

        let retrieved = registry.get_tool("calculator");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().function.name, "calculator");

        registry.remove_tool("calculator");
        assert!(registry.is_empty());
    }

    #[test]
    fn test_calculator_tool() {
        let tool = create_calculator_tool();
        assert_eq!(tool.function.name, "calculator");

        // Test simple calculations
        let args = json!({"expression": "2+3"});
        let result = execute_calculator(args);
        assert!(result.success);
        assert!(result.content.contains("5"));

        let args = json!({"expression": "10*5"});
        let result = execute_calculator(args);
        assert!(result.success);
        assert!(result.content.contains("50"));

        // Test error case
        let args = json!({});
        let result = execute_calculator(args);
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_datetime_tool() {
        let tool = create_datetime_tool();
        assert_eq!(tool.function.name, "datetime");

        // Test current time
        let args = json!({"operation": "current"});
        let result = execute_datetime(args);
        assert!(result.success);
        assert!(result.content.contains("Current UTC time"));

        // Test format operation
        let args = json!({
            "operation": "format",
            "date": "2023-01-01T12:00:00Z",
            "format": "%Y-%m-%d"
        });
        let result = execute_datetime(args);
        assert!(result.success);
        assert!(result.content.contains("2023-01-01"));

        // Test error case
        let args = json!({"operation": "invalid"});
        let result = execute_datetime(args);
        assert!(!result.success);
    }

    #[async_std::test]
    async fn test_tool_executor() {
        let mut executor = ToolExecutor::new();

        // Register a calculator tool
        let calc_tool = Box::new(ClosureTool::new(create_calculator_tool(), |args| {
            execute_calculator(args)
        }));
        executor.register_tool(calc_tool);

        // Test tool execution
        let tool_call = ToolCall {
            id: "call_1".to_string(),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: "calculator".to_string(),
                arguments: r#"{"expression": "5+3"}"#.to_string(),
            },
        };

        let result = executor.execute_tool_call(&tool_call).await;
        assert!(result.success);
        assert_eq!(result.tool_call_id, "calculator");
        assert!(result.content.contains("8"));

        // Test unknown tool
        let unknown_tool_call = ToolCall {
            id: "call_2".to_string(),
            tool_type: "function".to_string(),
            function: FunctionCall {
                name: "unknown_tool".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let result = executor.execute_tool_call(&unknown_tool_call).await;
        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("Unknown tool"));
    }

    #[test]
    fn test_simple_expression_evaluator() {
        assert_eq!(simple_evaluate_expression("2+3").unwrap(), 5.0);
        assert_eq!(simple_evaluate_expression("10*5").unwrap(), 50.0);
        assert_eq!(simple_evaluate_expression("10/2").unwrap(), 5.0);
        assert_eq!(simple_evaluate_expression("10-3").unwrap(), 7.0);
        assert_eq!(simple_evaluate_expression("42").unwrap(), 42.0);

        // Test division by zero
        assert!(simple_evaluate_expression("10/0").is_err());

        // Test invalid expression
        assert!(simple_evaluate_expression("invalid").is_err());
    }
}
