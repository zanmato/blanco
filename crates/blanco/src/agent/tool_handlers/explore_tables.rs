use async_trait::async_trait;
use gpui::AsyncWindowContext;
use std::collections::HashMap;

use llm::{
    FunctionCall, ToolCall, chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema,
    chat::Tool,
};

use super::markdown::format_explore_markdown;
use super::{AgentToolHandler, ToolContext};

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
