//! OpenAI-specific provider implementation
//!
//! This module contains the OpenAI implementation of the ChatProvider trait
//! from blanco-core and OpenAI-specific type conversions.

use blanco_core::chat_provider::{
    ChatCompletionRequest, ChatCompletionResponse, CompletionChoice, FinishReason, FunctionCall,
    FunctionDefinition, Message, StreamChoice, StreamChunk, StreamDelta, ToolCall, ToolChoice,
    ToolDefinition, UsageInfo,
};
use serde::{Deserialize, Serialize};

/// OpenAI-specific request structure
#[derive(Clone, Debug, Serialize)]
pub struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    pub stream: bool,
    pub temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OpenAIToolChoice>,
}

/// OpenAI message format
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// OpenAI response structure
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIResponse {
    pub id: String,
    #[serde(default = "default_object")]
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub system_fingerprint: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
}

/// Default value for object field
fn default_object() -> String {
    "chat.completion".to_string()
}

/// OpenAI choice structure
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIChoice {
    pub index: u32,
    pub message: OpenAIResponseMessage,
    pub finish_reason: String,
    #[serde(default)]
    pub logprobs: Option<serde_json::Value>,
}

/// OpenAI response message
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIResponseMessage {
    pub role: String,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub content: String,
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub annotations: Option<Vec<serde_json::Value>>,
}

/// Deserialize function that converts null or missing to empty string
fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// OpenAI usage information
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

/// Details about prompt tokens
#[derive(Clone, Debug, Deserialize)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u32,
    #[serde(default)]
    pub audio_tokens: u32,
}

/// Details about completion tokens
#[derive(Clone, Debug, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u32,
    #[serde(default)]
    pub audio_tokens: u32,
    #[serde(default)]
    pub accepted_prediction_tokens: u32,
    #[serde(default)]
    pub rejected_prediction_tokens: u32,
}

/// OpenAI streaming response
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIStreamChoice>,
}

/// OpenAI streaming choice
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamChoice {
    pub index: u32,
    pub delta: OpenAIStreamDelta,
    pub finish_reason: Option<String>,
}

/// OpenAI streaming delta
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<OpenAIStreamToolCall>>,
}

/// OpenAI streaming tool call
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamToolCall {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: Option<OpenAIStreamFunction>,
}

/// OpenAI streaming function
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIStreamFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

/// OpenAI tool definition
#[derive(Clone, Debug, Serialize)]
pub struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionDefinition,
}

/// OpenAI function definition
#[derive(Clone, Debug, Serialize)]
pub struct OpenAIFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// OpenAI tool call
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionCall,
}

/// OpenAI function call
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// OpenAI tool choice
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum OpenAIToolChoice {
    None,
    Auto,
    Required,
    Function { function: OpenAIFunctionChoice },
}

/// OpenAI function choice
#[derive(Clone, Debug, Serialize)]
pub struct OpenAIFunctionChoice {
    pub name: String,
}

/// OpenAI error response
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIErrorResponse {
    pub error: OpenAIErrorDetail,
}

/// OpenAI error details
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAIErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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
            function: tool_call.function.map_or_else(
                || FunctionCall {
                    name: String::new(),
                    arguments: String::new(),
                },
                Into::into,
            ),
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

impl From<OpenAIStreamFunction> for OpenAIFunctionCall {
    fn from(function: OpenAIStreamFunction) -> Self {
        Self {
            name: function.name.unwrap_or_default(),
            arguments: function.arguments.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpenAIRequest;

    #[test]
    fn test_serialization_omits_null_values() {
        let request = OpenAIRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                }
            ],
            stream: false,
            temperature: 0.7,
            max_tokens: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            tools: None,
            tool_choice: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        // The JSON should not contain null values
        assert!(!json.contains("null"));

        // Should contain the required fields
        assert!(json.contains("model"));
        assert!(json.contains("gpt-4"));
        assert!(json.contains("temperature"));
        assert!(json.contains("0.7"));

        // Should not contain the optional fields that are None
        assert!(!json.contains("max_tokens"));
        assert!(!json.contains("top_p"));
        assert!(!json.contains("frequency_penalty"));
        assert!(!json.contains("presence_penalty"));
        assert!(!json.contains("tools"));
        assert!(!json.contains("tool_choice"));
    }

    #[test]
    fn test_serialization_includes_some_values() {
        let request = OpenAIRequest {
            model: "gpt-4".to_string(),
            messages: vec![
                OpenAIMessage {
                    role: "user".to_string(),
                    content: "Hello".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                }
            ],
            stream: false,
            temperature: 0.7,
            max_tokens: Some(1000),
            top_p: None,
            frequency_penalty: Some(0.1),
            presence_penalty: None,
            tools: None,
            tool_choice: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        // Should contain the optional fields that have Some values
        assert!(json.contains("max_tokens"));
        assert!(json.contains("1000"));
        assert!(json.contains("frequency_penalty"));
        assert!(json.contains("0.1"));

        // Should not contain the optional fields that are None
        assert!(!json.contains("top_p"));
        assert!(!json.contains("presence_penalty"));
        assert!(!json.contains("tools"));
        assert!(!json.contains("tool_choice"));
    }

    #[test]
    fn test_message_serialization_omits_null_values() {
        let message = OpenAIMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        };

        let json = serde_json::to_string(&message).unwrap();

        // Should not contain null fields
        assert!(!json.contains("null"));

        // Should contain required fields
        assert!(json.contains("role"));
        assert!(json.contains("user"));
        assert!(json.contains("content"));
        assert!(json.contains("Hello"));

        // Should not contain optional fields that are None
        assert!(!json.contains("tool_calls"));
        assert!(!json.contains("tool_call_id"));
    }

    #[test]
    fn test_response_parsing_with_request_id() {
        let response_json = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 9,
                "completion_tokens": 12,
                "total_tokens": 21
            },
            "request_id": "req_12345"
        }"#;

        let response: OpenAIResponse = serde_json::from_str(response_json).unwrap();

        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.request_id, Some("req_12345".to_string()));
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].message.content, "Hello!");
        assert_eq!(response.usage.prompt_tokens, 9);
        assert_eq!(response.usage.completion_tokens, 12);
        assert_eq!(response.usage.total_tokens, 21);
    }

    #[test]
    fn test_response_parsing_with_usage_details() {
        let response_json = r#"{
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Test response"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 81,
                "completion_tokens": 186,
                "total_tokens": 267,
                "prompt_tokens_details": {
                    "cached_tokens": 6
                }
            }
        }"#;

        let response: OpenAIResponse = serde_json::from_str(response_json).unwrap();

        assert_eq!(response.id, "chatcmpl-456");
        assert_eq!(response.choices[0].message.content, "Test response");
        assert_eq!(response.usage.prompt_tokens, 81);
        assert_eq!(response.usage.completion_tokens, 186);
        assert_eq!(response.usage.total_tokens, 267);

        // Test the new optional usage details
        assert_eq!(response.usage.prompt_tokens_details.as_ref().unwrap().cached_tokens, 6);
        assert_eq!(response.usage.prompt_tokens_details.as_ref().unwrap().audio_tokens, 0);
        assert!(response.usage.completion_tokens_details.is_none());
    }

    #[test]
    fn test_response_parsing_real_world_example() {
        // This is based on the actual response you received
        let response_json = r#"{
            "choices": [{
                "finish_reason": "stop",
                "index": 0,
                "message": {
                    "content": "\nHello! I'm here to help you with your PostgreSQL queries.\n\nI see you have two queries ready:\n\n1.  **`SELECT * FROM stores;`**: This will retrieve all columns and rows from your `stores` table. It's a good way to get a quick overview of the data.\n\n2.  **`UPDATE stores SET available_currencies = '{1}' WHERE id = 1;`**: This will update the row in the `stores` table where `id` is `1`. Specifically, it sets the `available_currencies` column to a PostgreSQL array containing the single integer element `1`.\n\nHow can I assist you with these queries? For example, I can:\n*   Explain them in more detail.\n*   Help you modify them (e.g., update multiple rows, add more currencies).\n*   Suggest optimizations.\n*   Help debug any potential issues.",
                    "role": "assistant"
                }
            }],
            "created": 1762158376,
            "id": "20251103162614ab25f9b375f8437a",
            "model": "glm-4.6",
            "object": "chat.completion",
            "request_id": "20251103162614ab25f9b375f8437a",
            "usage": {
                "completion_tokens": 186,
                "prompt_tokens": 81,
                "prompt_tokens_details": {
                    "cached_tokens": 6
                },
                "total_tokens": 267
            }
        }"#;

        let response: OpenAIResponse = serde_json::from_str(response_json).unwrap();

        assert_eq!(response.id, "20251103162614ab25f9b375f8437a");
        assert_eq!(response.model, "glm-4.6");
        assert_eq!(response.request_id, Some("20251103162614ab25f9b375f8437a".to_string()));
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].finish_reason, "stop");
        assert_eq!(response.choices[0].index, 0);
        assert_eq!(response.choices[0].message.role, "assistant");
        assert!(response.choices[0].message.content.starts_with("\nHello! I'm here to help you"));
        assert_eq!(response.usage.prompt_tokens, 81);
        assert_eq!(response.usage.completion_tokens, 186);
        assert_eq!(response.usage.total_tokens, 267);
        assert_eq!(response.usage.prompt_tokens_details.unwrap().cached_tokens, 6);
    }

    #[test]
    fn test_response_parsing_minimal() {
        let response_json = r#"{
            "id": "minimal",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-3.5-turbo",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Minimal response"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        }"#;

        let response: OpenAIResponse = serde_json::from_str(response_json).unwrap();

        assert_eq!(response.id, "minimal");
        assert_eq!(response.model, "gpt-3.5-turbo");
        assert!(response.request_id.is_none());
        assert!(response.system_fingerprint.is_none());
        assert_eq!(response.choices[0].message.content, "Minimal response");
        assert!(response.usage.prompt_tokens_details.is_none());
        assert!(response.usage.completion_tokens_details.is_none());
    }

    #[test]
    fn test_response_parsing_with_tool_calls() {
        let response_json = r#"{
            "id": "tool-test",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "test_function",
                            "arguments": "{\"param\": \"value\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 10,
                "total_tokens": 30
            }
        }"#;

        let response: OpenAIResponse = serde_json::from_str(response_json).unwrap();

        assert_eq!(response.id, "tool-test");
        assert_eq!(response.choices[0].finish_reason, "tool_calls");
        assert!(response.choices[0].message.tool_calls.is_some());
        let tool_calls = response.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_123");
        assert_eq!(tool_calls[0].tool_type, "function");
        assert_eq!(tool_calls[0].function.name, "test_function");
    }

    #[test]
    fn test_response_parsing_exact_user_example() {
        // Test the exact response you received that was causing parsing errors
        let response_json = r#"{"choices":[{"finish_reason":"stop","index":0,"message":{"content":"\nHello! I'm here to help you with your PostgreSQL queries.\n\nI see you have two queries ready:\n\n1.  **`SELECT * FROM stores;`**: This will retrieve all columns and rows from your `stores` table. It's a good way to get a quick overview of the data.\n\n2.  **`UPDATE stores SET available_currencies = '{1}' WHERE id = 1;`**: This will update the row in the `stores` table where `id` is `1`. Specifically, it sets the `available_currencies` column to a PostgreSQL array containing the single integer element `1`.\n\nHow can I assist you with these queries? For example, I can:\n*   Explain them in more detail.\n*   Help you modify them (e.g., update multiple rows, add more currencies).\n*   Suggest optimizations.\n*   Help debug any potential issues.","role":"assistant"}}],"created":1762158376,"id":"20251103162614ab25f9b375f8437a","model":"glm-4.6","object":"chat.completion","request_id":"20251103162614ab25f9b375f8437a","usage":{"completion_tokens":186,"prompt_tokens":81,"prompt_tokens_details":{"cached_tokens":6},"total_tokens":267}}"#;

        let response: OpenAIResponse = serde_json::from_str(response_json).unwrap();

        assert_eq!(response.id, "20251103162614ab25f9b375f8437a");
        assert_eq!(response.model, "glm-4.6");
        assert_eq!(response.request_id, Some("20251103162614ab25f9b375f8437a".to_string()));
        assert_eq!(response.created, 1762158376);
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].finish_reason, "stop");
        assert_eq!(response.choices[0].index, 0);
        assert_eq!(response.choices[0].message.role, "assistant");
        assert!(response.choices[0].message.content.starts_with("\nHello! I'm here to help you"));
        assert!(response.choices[0].message.content.contains("PostgreSQL queries"));
        assert_eq!(response.usage.prompt_tokens, 81);
        assert_eq!(response.usage.completion_tokens, 186);
        assert_eq!(response.usage.total_tokens, 267);

        // Test the usage details
        assert!(response.usage.prompt_tokens_details.is_some());
        assert_eq!(response.usage.prompt_tokens_details.as_ref().unwrap().cached_tokens, 6);
        assert_eq!(response.usage.prompt_tokens_details.as_ref().unwrap().audio_tokens, 0);
        assert!(response.usage.completion_tokens_details.is_none());

        // Test the new service_tier field
        assert!(response.service_tier.is_none()); // This wasn't in your original response
    }

    #[test]
    fn test_response_parsing_missing_object_field() {
        // Test response from a model that doesn't include the object field (like glm-4.6)
        let response_json = r#"{
            "id": "test-no-object",
            "created": 1762160542,
            "model": "glm-4.6",
            "choices": [{
                "finish_reason": "stop",
                "index": 0,
                "message": {
                    "content": "Hello world",
                    "role": "assistant"
                }
            }],
            "request_id": "test-request-id",
            "usage": {
                "completion_tokens": 10,
                "prompt_tokens": 5,
                "total_tokens": 15
            }
        }"#;

        let response: OpenAIResponse = serde_json::from_str(response_json).unwrap();

        assert_eq!(response.id, "test-no-object");
        assert_eq!(response.model, "glm-4.6");
        // The object field should default to "chat.completion" when missing
        assert_eq!(response.object, "chat.completion");
        assert_eq!(response.request_id, Some("test-request-id".to_string()));
        assert_eq!(response.choices.len(), 1);
        assert_eq!(response.choices[0].message.content, "Hello world");
    }
}
