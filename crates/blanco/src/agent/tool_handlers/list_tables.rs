use async_trait::async_trait;
use gpui::AsyncWindowContext;
use std::collections::HashMap;

use llm::{
    FunctionCall, ToolCall, chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema,
    chat::Tool,
};

use super::markdown::format_schema_markdown;
use super::{AgentToolHandler, ToolContext};

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
