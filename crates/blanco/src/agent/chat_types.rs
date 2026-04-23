use chrono::{DateTime, Utc};
use gpui::SharedString;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
    ToolRequest,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: SharedString,
    pub timestamp: DateTime<Utc>,
    pub metadata: MessageMetadata,
    pub tool_calls: Option<Vec<ToolCallData>>,
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallData {
    pub id: String,
    pub tool_name: String,
    pub arguments: String,
    pub result: Option<String>,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub tokens_used: Option<u32>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub model: String,
    pub execution_time: Option<std::time::Duration>,
    pub sql_query: Option<String>,
    pub approval: Option<PendingApproval>,
}

impl Default for MessageMetadata {
    fn default() -> Self {
        Self {
            tokens_used: None,
            prompt_tokens: None,
            completion_tokens: None,
            model: "unknown".to_string(),
            execution_time: None,
            sql_query: None,
            approval: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ApprovalState {
    Pending,
    Approved,
    Denied,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingApproval {
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments_preview: String,
    pub state: ApprovalState,
    pub result: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoadingState {
    Idle,
    Connecting,
    Streaming,
    ProcessingTools,
    AwaitingApproval,
    Error(String),
}

impl LoadingState {
    pub fn is_loading(&self) -> bool {
        !matches!(self, LoadingState::Idle)
    }

    pub fn message(&self) -> String {
        match self {
            LoadingState::Idle => String::new(),
            LoadingState::Connecting => "Connecting...".to_string(),
            LoadingState::Streaming => "Thinking".to_string(),
            LoadingState::ProcessingTools => "Processing tools...".to_string(),
            LoadingState::AwaitingApproval => "Awaiting approval...".to_string(),
            LoadingState::Error(e) => e.clone(),
        }
    }

    pub fn show_spinner(&self) -> bool {
        matches!(
            self,
            LoadingState::Connecting
                | LoadingState::Streaming
                | LoadingState::ProcessingTools
                | LoadingState::AwaitingApproval
        )
    }
}

// Chat session events
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ChatEvent {
    MessageAdded {
        message: ChatMessage,
    },
    SessionCleared,
    LoadingStateChanged {
        _old_state: LoadingState,
        new_state: LoadingState,
    },
    ToolResultReady {
        tool_call_id: String,
        result_summary: String,
    },
}

// Chat commands
#[derive(Clone, Debug, PartialEq)]
pub enum ChatCommand {
    New,
    Help,
}

impl ChatCommand {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if !text.starts_with('/') {
            return None;
        }

        let command = text.to_lowercase();

        match command.as_str() {
            "/new" => Some(Self::New),
            "/help" => Some(Self::Help),
            _ => None,
        }
    }

    pub fn help_text() -> &'static str {
        r#"
Available commands:
/new - Start a new chat session by clearing the message history
"#
    }
}

// Utility functions
impl ChatMessage {
    pub fn new(role: MessageRole, content: String, model: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: MessageMetadata {
                model,
                ..Default::default()
            },
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self::new(MessageRole::User, content, "user".to_string())
    }

    pub fn assistant(content: String, model: String) -> Self {
        Self::new(MessageRole::Assistant, content, model)
    }

    pub fn tool(content: String, tool_call_id: String, model: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Tool,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: MessageMetadata {
                model,
                ..Default::default()
            },
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        }
    }

    pub fn tool_request(
        tool_call_id: String,
        tool_name: String,
        arguments_preview: String,
        code_block: Option<String>,
        model: String,
    ) -> Self {
        let content = match code_block {
            Some(code) => format!("```sql\n{}\n```", code),
            None => arguments_preview.clone(),
        };

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::ToolRequest,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: MessageMetadata {
                model,
                approval: Some(PendingApproval {
                    tool_call_id,
                    tool_name,
                    arguments_preview,
                    state: ApprovalState::Pending,
                    result: None,
                }),
                ..Default::default()
            },
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCallData>) -> Self {
        self.tool_calls = Some(tool_calls);
        self
    }

    pub fn with_usage(mut self, prompt_tokens: u32, completion_tokens: u32) -> Self {
        self.metadata.prompt_tokens = Some(prompt_tokens);
        self.metadata.completion_tokens = Some(completion_tokens);
        self.metadata.tokens_used = Some(prompt_tokens + completion_tokens);
        self
    }
}
