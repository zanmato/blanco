use async_trait::async_trait;
use gpui::AsyncWindowContext;
use gpui_component::input::RopeExt;
use std::collections::HashMap;

use llm::{
    FunctionCall, ToolCall, chat::FunctionTool, chat::ParameterProperty, chat::ParametersSchema,
    chat::Tool,
};

use super::{AgentToolHandler, ToolContext};

/// Maximum lines to return when no range is specified
const READ_TAB_DEFAULT_LIMIT: usize = 200;

/// Read tab tool handler with optional line range support
pub struct ReadTabHandler {
    /// What the tab holds ("SQL query", "JavaScript script", …).
    pub content_noun: &'static str,
}

#[async_trait(?Send)]
impl AgentToolHandler for ReadTabHandler {
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
        cx: &mut AsyncWindowContext,
    ) -> ToolCall {
        if let Some(input_state) = &context.input_state {
            let requested_start = arguments
                .get("start_line")
                .and_then(|v| v.as_i64())
                .map(|v| v.max(1) as usize);

            let requested_end = arguments
                .get("end_line")
                .and_then(|v| v.as_i64())
                .map(|v| v.max(1) as usize);

            match input_state.read_with(cx, |input, _cx| {
                let rope = input.text();
                let total_lines = rope.lines_len();

                if total_lines == 0 {
                    return (String::new(), 0usize, 0usize, 0usize);
                }

                let (start, end) = match (requested_start, requested_end) {
                    (Some(s), Some(e)) => (s, e),
                    (Some(s), None) => (s, total_lines),
                    (None, Some(e)) => (1, e),
                    (None, None) if total_lines > READ_TAB_DEFAULT_LIMIT => {
                        (1, READ_TAB_DEFAULT_LIMIT)
                    }
                    (None, None) => (1, total_lines),
                };

                let start = start.min(total_lines);
                let end = end.min(total_lines).max(start);

                let slice = rope.slice_lines((start - 1)..end);
                let content = slice.to_string();
                let numbered = content
                    .lines()
                    .enumerate()
                    .map(|(i, line)| format!("{}: {}", start + i, line))
                    .collect::<Vec<_>>()
                    .join("\n");

                (numbered, total_lines, start, end)
            }) {
                Ok((content, total_lines, start, end)) => {
                    let mut json = serde_json::json!({
                        "content": content,
                        "total_lines": total_lines,
                        "start_line": start,
                        "end_line": end,
                    });

                    if total_lines > end {
                        json["truncated"] = serde_json::json!(true);
                        json["hint"] = serde_json::json!(format!(
                            "Showing lines {}-{} of {}. Use start_line/end_line to read more.",
                            start, end, total_lines
                        ));
                    }

                    ToolCall {
                        id: "read-tab".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "read-tab".to_string(),
                            arguments: json.to_string(),
                        },
                    }
                }
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
        let mut properties = HashMap::new();
        properties.insert(
            "start_line".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: "1-based starting line number to read from. Defaults to line 1."
                    .to_string(),
                items: None,
                enum_list: None,
            },
        );
        properties.insert(
            "end_line".to_string(),
            ParameterProperty {
                property_type: "integer".to_string(),
                description: format!("1-based ending line number (inclusive). Defaults to the last line, or {} if the file is large and no range is specified.", READ_TAB_DEFAULT_LIMIT),
                items: None,
                enum_list: None,
            },
        );

        Tool {
            tool_type: "function".to_string(),
            function: FunctionTool {
                name: "read-tab".to_string(),
                description: format!(
                    "Read the current tab's content (the {}) with optional line range. Returns line-numbered content. If the tab has more than {} lines and no range is specified, returns only the first {} lines with a hint to read more.",
                    self.content_noun, READ_TAB_DEFAULT_LIMIT, READ_TAB_DEFAULT_LIMIT
                ),
                parameters: serde_json::to_value(ParametersSchema {
                    schema_type: "object".to_string(),
                    properties,
                    required: vec![],
                })
                .unwrap_or_default(),
            },
            cache_control: None,
        }
    }

    fn call_summary(&self, arguments: &serde_json::Value, _result: &ToolCall) -> String {
        let start = arguments
            .get("start_line")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let end = arguments.get("end_line").and_then(|v| v.as_i64());
        match end {
            Some(end) => format!("Read Tab: lines {}-{}", start, end),
            None => "Read Tab".to_string(),
        }
    }

    fn always_allow(&self) -> bool {
        true
    }
}
