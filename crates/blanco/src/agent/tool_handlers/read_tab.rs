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
pub(crate) const READ_TAB_DEFAULT_LIMIT: usize = 200;

/// A line-numbered window of a tab's text. Line numbers are 1-based and
/// inclusive; an empty tab reports zeros.
pub(crate) struct TabSlice {
    pub content: String,
    pub total_lines: usize,
    pub start_line: usize,
    pub end_line: usize,
}

impl TabSlice {
    /// Whether lines after `end_line` were left out.
    pub(crate) fn truncated(&self) -> bool {
        self.total_lines > self.end_line
    }
}

/// Cut `rope` to the requested 1-based inclusive range, numbering each line.
/// Without a range the first `READ_TAB_DEFAULT_LIMIT` lines are returned so a
/// huge buffer does not flood the model; a half-open range extends to the
/// buffer's edge. Out-of-range bounds are clamped rather than rejected. Shared
/// by the agent's `read-tab` tool and the MCP `read_tab` tool.
pub(crate) fn slice_tab_text(
    rope: &ropey::Rope,
    requested_start: Option<usize>,
    requested_end: Option<usize>,
) -> TabSlice {
    let total_lines = rope.lines_len();
    if total_lines == 0 {
        return TabSlice {
            content: String::new(),
            total_lines: 0,
            start_line: 0,
            end_line: 0,
        };
    }

    let (start, end) = match (requested_start, requested_end) {
        (Some(start), Some(end)) => (start, end),
        (Some(start), None) => (start, total_lines),
        (None, Some(end)) => (1, end),
        (None, None) if total_lines > READ_TAB_DEFAULT_LIMIT => (1, READ_TAB_DEFAULT_LIMIT),
        (None, None) => (1, total_lines),
    };

    let start = start.clamp(1, total_lines);
    let end = end.min(total_lines).max(start);

    let slice = rope.slice_lines((start - 1)..end);
    let content = slice
        .to_string()
        .lines()
        .enumerate()
        .map(|(offset, line)| format!("{}: {}", start + offset, line))
        .collect::<Vec<_>>()
        .join("\n");

    TabSlice {
        content,
        total_lines,
        start_line: start,
        end_line: end,
    }
}

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
                slice_tab_text(input.text(), requested_start, requested_end)
            }) {
                Ok(TabSlice {
                    content,
                    total_lines,
                    start_line: start,
                    end_line: end,
                }) => {
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
