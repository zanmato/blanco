use chrono::{DateTime, Utc};
use gpui::SharedString;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
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
    pub model: String,
    pub execution_time: Option<std::time::Duration>,
}

impl Default for MessageMetadata {
    fn default() -> Self {
        Self {
            tokens_used: None,
            model: "unknown".to_string(),
            execution_time: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoadingState {
    Idle,
    Connecting,
    Streaming,
    ProcessingTools,
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
            LoadingState::Error(e) => e.clone(),
        }
    }

    pub fn show_spinner(&self) -> bool {
        matches!(
            self,
            LoadingState::Connecting | LoadingState::Streaming | LoadingState::ProcessingTools
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
                tokens_used: None,
                model,
                execution_time: None,
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
                tokens_used: None,
                model,
                execution_time: None,
            },
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
        }
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCallData>) -> Self {
        self.tool_calls = Some(tool_calls);
        self
    }
}
