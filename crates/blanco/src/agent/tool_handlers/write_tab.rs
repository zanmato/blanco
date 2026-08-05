use async_trait::async_trait;
use gpui::AsyncWindowContext;
use gpui_component::input::RopeExt;
use std::collections::HashMap;

use llm::{
    FunctionCall, ToolCall, chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema,
    chat::Tool,
};

use super::{AgentToolHandler, ToolContext};

/// Write tab tool handler with multiple operation modes
pub struct WriteTabHandler {
    /// What the tab holds ("SQL query", "JavaScript script", …), so the model
    /// is told what kind of content it is expected to write.
    pub content_noun: &'static str,
}

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
                let new_text =
                    apply_line_operation(&current, operation, &content, start_line, end_line)?;
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
                        arguments:
                            serde_json::json!({"error": format!("Failed to write tab: {}", e)})
                                .to_string(),
                    },
                },
                Err(e) => ToolCall {
                    id: "write-tab".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "write-tab".to_string(),
                        arguments:
                            serde_json::json!({"error": format!("Failed to write tab: {}", e)})
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
                description: format!("The {} content to write/insert.", self.content_noun),
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
                description: format!(
                    "Write content to the current tab, which holds the {}. Supports three modes: replace all content (default), insert before a specific line, or replace a specific range of lines.",
                    self.content_noun
                ),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec!["content".to_string()],
                })
                .unwrap_or_default(),
            },
            cache_control: None,
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
