//! Chat Provider Trait
//!
//! This module defines the ChatProvider trait for AI/chat integration.
//! It provides a provider-agnostic interface that can be implemented by different
//! AI providers (OpenAI, Anthropic, local LLMs, etc.).

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

// Error type that implements std::error::Error and can be created from anyhow::Error
#[derive(Debug)]
pub struct ProviderError {
    inner: anyhow::Error,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for ProviderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.inner.as_ref())
    }
}

impl From<anyhow::Error> for ProviderError {
    fn from(err: anyhow::Error) -> Self {
        Self { inner: err }
    }
}

/// A trait that defines the interface for chat providers
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// The error type for this provider
    type Error: std::error::Error + Send + Sync + 'static;

    /// Send a chat completion request
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, Self::Error>;

    /// Send a streaming chat completion request
    async fn stream_chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, Self::Error>> + Send>>, Self::Error>;

    /// Call a tool/function
    async fn call_tool(&self, tool_call: ToolCall) -> Result<ToolResult, Self::Error>;
}

/// Generic chat completion request that works across providers
#[derive(Clone, Debug, Serialize)]
pub struct ChatCompletionRequest {
    /// The model to use for completion
    pub model: String,
    /// The messages to send to the model
    pub messages: Vec<Message>,
    /// Whether to stream the response
    pub stream: bool,
    /// Temperature for sampling (0.0 to 2.0)
    pub temperature: f32,
    /// Maximum number of tokens to generate
    pub max_tokens: Option<u32>,
    /// Nucleus sampling parameter (top_p)
    pub top_p: Option<f32>,
    /// Frequency penalty (-2.0 to 2.0)
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0)
    pub presence_penalty: Option<f32>,
    /// Tools/functions the model can call
    pub tools: Option<Vec<ToolDefinition>>,
    /// Tool choice behavior
    pub tool_choice: Option<ToolChoice>,
    /// Additional provider-specific parameters
    pub additional_params: Option<serde_json::Value>,
}

/// Generic chat completion response
#[derive(Clone, Debug, Deserialize)]
pub struct ChatCompletionResponse {
    /// Unique identifier for the completion
    pub id: String,
    /// Object type (typically "chat.completion")
    pub object: String,
    /// Unix timestamp of creation
    pub created: u64,
    /// Model used for completion
    pub model: String,
    /// The completion choices
    pub choices: Vec<CompletionChoice>,
    /// Usage information
    pub usage: UsageInfo,
    /// Additional provider-specific data
    pub additional_data: Option<serde_json::Value>,
}

/// A single completion choice
#[derive(Clone, Debug, Deserialize)]
pub struct CompletionChoice {
    /// Index of the choice
    pub index: u32,
    /// The message content
    pub message: Message,
    /// The reason the completion finished
    pub finish_reason: FinishReason,
    /// Tool calls made by the model
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Streaming response chunk
#[derive(Clone, Debug, Deserialize)]
pub struct StreamChunk {
    /// Unique identifier for the chunk
    pub id: String,
    /// Object type (typically "chat.completion.chunk")
    pub object: String,
    /// Unix timestamp of creation
    pub created: u64,
    /// Model used for completion
    pub model: String,
    /// The streaming choices
    pub choices: Vec<StreamChoice>,
}

/// A streaming choice
#[derive(Clone, Debug, Deserialize)]
pub struct StreamChoice {
    /// Index of the choice
    pub index: u32,
    /// The delta content
    pub delta: StreamDelta,
    /// The reason the completion finished
    pub finish_reason: Option<FinishReason>,
}

/// Delta content for streaming responses
#[derive(Clone, Debug, Deserialize)]
pub struct StreamDelta {
    /// The message role
    pub role: Option<String>,
    /// The message content
    pub content: Option<String>,
    /// Tool calls in progress
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Chat message
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// Message role (system, user, assistant, tool)
    pub role: String,
    /// Message content
    pub content: String,
    /// Tool call ID (for tool responses)
    pub tool_call_id: Option<String>,
    /// Tool calls (for assistant messages)
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Additional message metadata
    pub additional_data: Option<serde_json::Value>,
}

/// Token usage information
#[derive(Clone, Debug, Deserialize)]
pub struct UsageInfo {
    /// Tokens used in the prompt
    pub prompt_tokens: u32,
    /// Tokens used in the completion
    pub completion_tokens: u32,
    /// Total tokens used
    pub total_tokens: u32,
}

/// Reason why a completion finished
#[derive(Clone, Debug, PartialEq)]
pub enum FinishReason {
    /// Hit the maximum token limit
    Length,
    /// Natural stop point
    Stop,
    /// Model called a tool/function
    ToolCalls,
    /// Content filtered by policy
    ContentFilter,
    /// Model's stop sequence
    StopSequence,
    /// Unknown or provider-specific reason
    Other(String),
}

/// Tool/function definition
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool type (typically "function")
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition
    pub function: FunctionDefinition,
}

/// Function definition for tools
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Function name
    pub name: String,
    /// Function description
    pub description: String,
    /// Function parameters (JSON schema)
    pub parameters: serde_json::Value,
}

/// Tool call made by the model
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool call ID
    pub id: String,
    /// Tool type (typically "function")
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function call details
    pub function: FunctionCall,
}

/// Function call details
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Function name
    pub name: String,
    /// Function arguments as JSON string
    pub arguments: String,
}

/// Tool choice behavior
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// Don't use tools
    None,
    /// Model can choose whether to use tools
    Auto,
    /// Model must use a specific tool
    Required,
    /// Use a specific function
    Function { name: String },
}

/// Result of a tool call
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    /// Tool call ID this result corresponds to
    pub tool_call_id: String,
    /// The result content
    pub content: String,
    /// Whether the tool call was successful
    pub success: bool,
    /// Error message if unsuccessful
    pub error: Option<String>,
}

impl Default for ChatCompletionRequest {
    fn default() -> Self {
        Self {
            model: "gpt-3.5-turbo".to_string(),
            messages: Vec::new(),
            stream: false,
            temperature: 0.7,
            max_tokens: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            tools: None,
            tool_choice: None,
            additional_params: None,
        }
    }
}

impl Message {
    /// Create a new user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
            additional_data: None,
        }
    }

    /// Create a new assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
            additional_data: None,
        }
    }

    /// Create a new system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
            additional_data: None,
        }
    }

    /// Create a new tool result message
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
            additional_data: None,
        }
    }

    /// Add tool calls to an assistant message
    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = Some(tool_calls);
        self
    }
}

impl ToolDefinition {
    /// Create a new tool definition
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: serde_json::Value) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

impl ToolResult {
    /// Create a successful tool result
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            success: true,
            error: None,
        }
    }

    /// Create a failed tool result
    pub fn error(tool_call_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            content: String::new(),
            success: false,
            error: Some(error.into()),
        }
    }
}

impl FinishReason {
    /// Create a finish reason from a string
    pub fn parse(s: &str) -> Self {
        match s {
            "length" => FinishReason::Length,
            "stop" => FinishReason::Stop,
            "tool_calls" => FinishReason::ToolCalls,
            "content_filter" => FinishReason::ContentFilter,
            "stop_sequence" => FinishReason::StopSequence,
            other => FinishReason::Other(other.to_string()),
        }
    }
}

impl std::fmt::Display for FinishReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinishReason::Length => write!(f, "length"),
            FinishReason::Stop => write!(f, "stop"),
            FinishReason::ToolCalls => write!(f, "tool_calls"),
            FinishReason::ContentFilter => write!(f, "content_filter"),
            FinishReason::StopSequence => write!(f, "stop_sequence"),
            FinishReason::Other(s) => write!(f, "{}", s),
        }
    }
}

impl<'de> Deserialize<'de> for FinishReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(FinishReason::parse(&s))
    }
}