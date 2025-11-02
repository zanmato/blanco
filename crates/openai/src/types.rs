use crate::provider::*;
use serde::{Deserialize, Serialize};

/// OpenAI-specific request structure
#[derive(Clone, Debug, Serialize)]
#[allow(missing_docs)]
pub struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    pub stream: bool,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub tools: Option<Vec<OpenAITool>>,
    pub tool_choice: Option<OpenAIToolChoice>,
}

/// OpenAI message format
#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    pub tool_call_id: Option<String>,
}

/// OpenAI response structure
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

/// OpenAI choice structure
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIChoice {
    pub index: u32,
    pub message: OpenAIResponseMessage,
    pub finish_reason: String,
}

/// OpenAI response message
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIResponseMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
}

/// OpenAI usage information
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// OpenAI streaming response
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIStreamResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIStreamChoice>,
}

/// OpenAI streaming choice
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIStreamChoice {
    pub index: u32,
    pub delta: OpenAIStreamDelta,
    pub finish_reason: Option<String>,
}

/// OpenAI streaming delta
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIStreamDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAIStreamToolCall>>,
}

/// OpenAI streaming tool call
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIStreamToolCall {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: Option<OpenAIStreamFunction>,
}

/// OpenAI streaming function
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIStreamFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// OpenAI tool definition
#[derive(Clone, Debug, Serialize)]
#[allow(missing_docs)]
pub struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionDefinition,
}

/// OpenAI function definition
#[derive(Clone, Debug, Serialize)]
#[allow(missing_docs)]
pub struct OpenAIFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// OpenAI tool call
#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionCall,
}

/// OpenAI function call
#[derive(Clone, Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// OpenAI tool choice
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
#[allow(missing_docs)]
pub enum OpenAIToolChoice {
    None,
    Auto,
    Required,
    Function { function: OpenAIFunctionChoice },
}

/// OpenAI function choice
#[derive(Clone, Debug, Serialize)]
#[allow(missing_docs)]
pub struct OpenAIFunctionChoice {
    pub name: String,
}

/// OpenAI error response
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIErrorResponse {
    pub error: OpenAIError,
}

/// OpenAI error details
#[derive(Clone, Debug, Deserialize)]
#[allow(missing_docs)]
pub struct OpenAIError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: Option<String>,
}

/// Conversion implementations
impl From<ChatCompletionRequest> for OpenAIRequest {
    fn from(request: ChatCompletionRequest) -> Self {
        Self {
            model: request.model,
            messages: request.messages.into_iter().map(Into::into).collect(),
            stream: request.stream,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            top_p: request.top_p,
            frequency_penalty: request.frequency_penalty,
            presence_penalty: request.presence_penalty,
            tools: request
                .tools
                .map(|tools| tools.into_iter().map(Into::into).collect()),
            tool_choice: request.tool_choice.map(Into::into),
        }
    }
}

impl From<OpenAIResponse> for ChatCompletionResponse {
    fn from(response: OpenAIResponse) -> Self {
        Self {
            id: response.id,
            object: response.object,
            created: response.created,
            model: response.model,
            choices: response.choices.into_iter().map(Into::into).collect(),
            usage: response.usage.into(),
            additional_data: None,
        }
    }
}

impl From<OpenAIChoice> for CompletionChoice {
    fn from(choice: OpenAIChoice) -> Self {
        let tool_calls = choice
            .message
            .tool_calls
            .clone()
            .map(|calls| calls.into_iter().map(Into::into).collect());
        Self {
            index: choice.index,
            message: choice.message.into(),
            finish_reason: FinishReason::parse(&choice.finish_reason),
            tool_calls,
        }
    }
}

impl From<OpenAIResponseMessage> for Message {
    fn from(message: OpenAIResponseMessage) -> Self {
        Self {
            role: message.role,
            content: message.content,
            tool_call_id: None,
            tool_calls: message
                .tool_calls
                .map(|calls| calls.into_iter().map(Into::into).collect()),
            additional_data: None,
        }
    }
}

impl From<OpenAIUsage> for UsageInfo {
    fn from(usage: OpenAIUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

impl From<OpenAIStreamResponse> for StreamChunk {
    fn from(response: OpenAIStreamResponse) -> Self {
        Self {
            id: response.id,
            object: response.object,
            created: response.created,
            model: response.model,
            choices: response.choices.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<OpenAIStreamChoice> for StreamChoice {
    fn from(choice: OpenAIStreamChoice) -> Self {
        Self {
            index: choice.index,
            delta: choice.delta.into(),
            finish_reason: choice
                .finish_reason
                .map(|reason| FinishReason::parse(&reason)),
        }
    }
}

impl From<OpenAIStreamDelta> for StreamDelta {
    fn from(delta: OpenAIStreamDelta) -> Self {
        Self {
            role: delta.role,
            content: delta.content,
            tool_calls: delta
                .tool_calls
                .map(|calls| calls.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<Message> for OpenAIMessage {
    fn from(message: Message) -> Self {
        Self {
            role: message.role,
            content: message.content,
            tool_calls: message
                .tool_calls
                .map(|calls| calls.into_iter().map(Into::into).collect()),
            tool_call_id: message.tool_call_id,
        }
    }
}

impl From<ToolDefinition> for OpenAITool {
    fn from(tool: ToolDefinition) -> Self {
        Self {
            tool_type: tool.tool_type,
            function: tool.function.into(),
        }
    }
}

impl From<FunctionDefinition> for OpenAIFunctionDefinition {
    fn from(function: FunctionDefinition) -> Self {
        Self {
            name: function.name,
            description: function.description,
            parameters: function.parameters,
        }
    }
}

impl From<ToolCall> for OpenAIToolCall {
    fn from(tool_call: ToolCall) -> Self {
        Self {
            id: tool_call.id,
            tool_type: tool_call.tool_type,
            function: tool_call.function.into(),
        }
    }
}

impl From<FunctionCall> for OpenAIFunctionCall {
    fn from(function: FunctionCall) -> Self {
        Self {
            name: function.name,
            arguments: function.arguments,
        }
    }
}

impl From<ToolChoice> for OpenAIToolChoice {
    fn from(choice: ToolChoice) -> Self {
        match choice {
            ToolChoice::None => OpenAIToolChoice::None,
            ToolChoice::Auto => OpenAIToolChoice::Auto,
            ToolChoice::Required => OpenAIToolChoice::Required,
            ToolChoice::Function { name } => OpenAIToolChoice::Function {
                function: OpenAIFunctionChoice { name },
            },
        }
    }
}

impl From<OpenAIToolCall> for ToolCall {
    fn from(tool_call: OpenAIToolCall) -> Self {
        Self {
            id: tool_call.id,
            tool_type: tool_call.tool_type,
            function: tool_call.function.into(),
        }
    }
}

impl From<OpenAIFunctionCall> for FunctionCall {
    fn from(function: OpenAIFunctionCall) -> Self {
        Self {
            name: function.name,
            arguments: function.arguments,
        }
    }
}

impl From<OpenAIStreamToolCall> for ToolCall {
    fn from(tool_call: OpenAIStreamToolCall) -> Self {
        Self {
            id: tool_call.id.unwrap_or_default(),
            tool_type: tool_call.tool_type.unwrap_or_default(),
            function: tool_call.function.map(Into::into).unwrap_or(FunctionCall {
                name: String::new(),
                arguments: String::new(),
            }),
        }
    }
}

impl From<OpenAIStreamFunction> for FunctionCall {
    fn from(function: OpenAIStreamFunction) -> Self {
        Self {
            name: function.name.unwrap_or_default(),
            arguments: function.arguments.unwrap_or_default(),
        }
    }
}
