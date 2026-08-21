use async_trait::async_trait;
use gpui::AsyncWindowContext;
use std::collections::HashMap;

use llm::{
    FunctionCall, ToolCall, chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema,
    chat::Tool,
};

use super::{AgentToolHandler, ToolContext};

const EXECUTE_SQL_MAX_ROWS: usize = 100;
const EXECUTE_SQL_AUTO_LIMIT: usize = 100;

/// Uses the SQL grammar so a `DROP` inside a string literal or comment does not
/// trip the guard, while `DROP\tTABLE` / `DROP\nTABLE` still do. When the text
/// does not parse cleanly the tree cannot be trusted, so fall back to a
/// word-boundary scan and err on the side of blocking.
fn contains_drop_statement(sql: &str) -> bool {
    match sql_parser::statement_parser::contains_node_kind(sql, &["keyword_drop"]) {
        Some(found) => found,
        None => {
            let upper = sql.to_uppercase();
            upper
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .any(|word| word == "DROP")
        }
    }
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
            .execute_script(connection_id, context.database_name.as_deref(), &sql)
            .await
            // The tool runs a single statement; surface the final result-set,
            // matching the script path the editor uses (and its OID resolution).
            .map(|results| results.into_iter().last().unwrap_or_default())
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
            cache_control: None,
        }
    }

    fn call_summary(&self, _arguments: &serde_json::Value, _result: &ToolCall) -> String {
        "Execute SQL".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::contains_drop_statement;

    #[test]
    fn drop_guard_matches_any_whitespace() {
        assert!(contains_drop_statement("DROP TABLE users"));
        assert!(contains_drop_statement("drop\ttable users"));
        assert!(contains_drop_statement("DROP\nTABLE users;"));
        assert!(contains_drop_statement("SELECT 1; DROP TABLE users"));
    }

    #[test]
    fn drop_guard_ignores_literals_and_identifiers() {
        assert!(!contains_drop_statement(
            "SELECT 'please DROP this' AS note"
        ));
        assert!(!contains_drop_statement("SELECT drop_count FROM stats"));
        assert!(!contains_drop_statement("SELECT * FROM users"));
    }
}
