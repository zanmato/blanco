//! OpenAI-compatible streaming chat provider.
//!
//! The `llm` crate's native `LLMBackend::OpenAI` switched to OpenAI's `/responses` endpoint
//! in 1.3.x, which the endpoints we target (OpenAI's own `/chat/completions`, z.AI, local
//! proxies, ...) do not implement. The crate ships an `OpenAICompatibleProvider`, but both its
//! blocking response type and its stream delta only expose `content` + `tool_calls`: a
//! reasoning model's answer in `reasoning_content` is silently dropped, and the blocking call
//! gives no incremental output.
//!
//! So this provider talks to `/chat/completions` directly and parses the SSE stream itself,
//! surfacing `content`, `reasoning_content`, tool-call deltas, and usage as
//! [`ChatStreamEvent`]s. It implements [`StreamingChatProvider`] rather than the `llm` crate's
//! `LLMProvider`.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::channel::mpsc;
use llm::{
    FunctionCall, ToolCall, chat::ChatMessage as LlmChatMessage, chat::ChatRole, chat::MessageType,
    chat::Tool, error::LLMError,
};
use serde::{Deserialize, Serialize};

use super::streaming::{ChatEventStream, ChatStreamEvent, StreamingChatProvider};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Initial requests are retried on transient failures (connection errors, 429, 5xx) with
/// exponential backoff. Kept in sync with the native backend's resilience policy.
const MAX_ATTEMPTS: usize = 3;
const BASE_BACKOFF_MS: u64 = 200;
const MAX_BACKOFF_MS: u64 = 2000;

/// An OpenAI-compatible provider that streams `/chat/completions` responses.
pub struct CompatibleProvider {
    client: reqwest::Client,
    api_key: String,
    completions_url: String,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

impl CompatibleProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: Option<String>,
        model: Option<String>,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        connect_timeout_seconds: Option<u64>,
    ) -> Self {
        let base = base_url
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let completions_url = format!("{}/chat/completions", base.trim_end_matches('/'));

        // No overall request timeout: a reasoning turn can stream for minutes. We only bound
        // how long we wait to establish the connection.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(connect_timeout_seconds.unwrap_or(30)))
            .build()
            .unwrap_or_default();

        Self {
            client,
            api_key: api_key.into(),
            completions_url,
            model: model.unwrap_or_default(),
            max_tokens,
            temperature,
        }
    }

    fn build_body<'a>(
        &'a self,
        system_prompt: &'a str,
        messages: &'a [LlmChatMessage],
        tools: Option<&'a [Tool]>,
    ) -> ChatRequest<'a> {
        let mut request_messages = Vec::with_capacity(messages.len() + 1);
        request_messages.push(RequestMessage {
            role: "system",
            content: Some(system_prompt),
            tool_calls: None,
            tool_call_id: None,
        });

        for msg in messages {
            match &msg.message_type {
                MessageType::Text => request_messages.push(RequestMessage {
                    role: match msg.role {
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                    },
                    content: Some(&msg.content),
                    tool_calls: None,
                    tool_call_id: None,
                }),
                MessageType::ToolUse(calls) => request_messages.push(RequestMessage {
                    role: "assistant",
                    content: (!msg.content.is_empty()).then_some(msg.content.as_str()),
                    tool_calls: Some(calls),
                    tool_call_id: None,
                }),
                // One `tool` message per result: the result payload travels in
                // `function.arguments`, linked to the request by the call id.
                MessageType::ToolResult(results) => {
                    for result in results {
                        request_messages.push(RequestMessage {
                            role: "tool",
                            content: Some(&result.function.arguments),
                            tool_calls: None,
                            tool_call_id: Some(&result.id),
                        });
                    }
                }
                MessageType::Image(_)
                | MessageType::Pdf(_)
                | MessageType::Audio(_)
                | MessageType::ImageURL(_) => {}
            }
        }

        ChatRequest {
            model: &self.model,
            messages: request_messages,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            tools: tools.filter(|t| !t.is_empty()),
        }
    }

    async fn send_with_retry(&self, body: &ChatRequest<'_>) -> Result<reqwest::Response, LLMError> {
        if self.api_key.is_empty() {
            return Err(LLMError::AuthError("Missing API key".to_string()));
        }

        let mut attempt = 0;
        loop {
            let result = self
                .client
                .post(&self.completions_url)
                .bearer_auth(&self.api_key)
                .header("Accept", "text/event-stream")
                .json(body)
                .send()
                .await;

            let retryable = match &result {
                Ok(response) => {
                    let status = response.status();
                    status == 429 || status.is_server_error()
                }
                Err(error) => error.is_connect() || error.is_timeout() || error.is_request(),
            };

            match result {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    if retryable && attempt + 1 < MAX_ATTEMPTS {
                        backoff(attempt).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(LLMError::ResponseFormatError {
                        message: format!("Chat completions request failed: HTTP {status}"),
                        raw_response: body,
                    });
                }
                Err(error) => {
                    if retryable && attempt + 1 < MAX_ATTEMPTS {
                        backoff(attempt).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(LLMError::HttpError(error.to_string()));
                }
            }
        }
    }
}

async fn backoff(attempt: usize) {
    let delay = (BASE_BACKOFF_MS.saturating_mul(1 << attempt.min(16))).min(MAX_BACKOFF_MS);
    tokio::time::sleep(Duration::from_millis(delay)).await;
}

#[async_trait]
impl StreamingChatProvider for CompatibleProvider {
    async fn stream_chat(
        &self,
        system_prompt: &str,
        messages: &[LlmChatMessage],
        tools: Option<&[Tool]>,
    ) -> Result<ChatEventStream, LLMError> {
        let body = self.build_body(system_prompt, messages, tools);
        let response = self.send_with_retry(&body).await?;

        let (sender, receiver) = mpsc::unbounded::<Result<ChatStreamEvent, LLMError>>();

        // Read and parse the SSE body on the Tokio runtime (reqwest's byte stream must be
        // polled there). Parsed events are pushed onto the channel, which the chat session
        // drains on the GPUI foreground to update the UI incrementally.
        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut tool_calls: BTreeMap<usize, ToolCallAccumulator> = BTreeMap::new();

            'read: while let Some(chunk) = byte_stream.next().await {
                let bytes = match chunk {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let _ = sender.unbounded_send(Err(LLMError::HttpError(error.to_string())));
                        break;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(newline) = buffer.find('\n') {
                    let line: String = buffer.drain(..=newline).collect();
                    let line = line.trim();
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        break 'read;
                    }
                    let chunk: StreamChunk = match serde_json::from_str(data) {
                        Ok(chunk) => chunk,
                        // Skip keep-alive comments or fragments we cannot parse rather than
                        // killing the whole stream.
                        Err(_) => continue,
                    };

                    if sender_closed(&sender) {
                        break 'read;
                    }

                    for choice in chunk.choices {
                        if let Some(reasoning) = choice.delta.reasoning_content {
                            if !reasoning.is_empty() {
                                let _ = sender
                                    .unbounded_send(Ok(ChatStreamEvent::Reasoning(reasoning)));
                            }
                        }
                        if let Some(content) = choice.delta.content {
                            if !content.is_empty() {
                                let _ =
                                    sender.unbounded_send(Ok(ChatStreamEvent::Content(content)));
                            }
                        }
                        for delta in choice.delta.tool_calls {
                            let entry = tool_calls.entry(delta.index).or_default();
                            if let Some(id) = delta.id {
                                entry.id = id;
                            }
                            if let Some(function) = delta.function {
                                if let Some(name) = function.name {
                                    entry.name.push_str(&name);
                                }
                                if let Some(arguments) = function.arguments {
                                    entry.arguments.push_str(&arguments);
                                }
                            }
                        }
                    }

                    if let Some(usage) = chunk.usage {
                        let _ = sender.unbounded_send(Ok(ChatStreamEvent::Usage {
                            prompt_tokens: usage.prompt_tokens,
                            completion_tokens: usage.completion_tokens,
                        }));
                    }
                }
            }

            if !tool_calls.is_empty() {
                let assembled = tool_calls
                    .into_values()
                    .map(|accumulator| ToolCall {
                        id: accumulator.id,
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: accumulator.name,
                            arguments: accumulator.arguments,
                        },
                    })
                    .collect();
                let _ = sender.unbounded_send(Ok(ChatStreamEvent::ToolCalls(assembled)));
            }
        });

        Ok(Box::pin(receiver))
    }
}

fn sender_closed(sender: &mpsc::UnboundedSender<Result<ChatStreamEvent, LLMError>>) -> bool {
    sender.is_closed()
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<RequestMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
    stream_options: StreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [Tool]>,
}

#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<&'a [ToolCall]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    usage: Option<UsageDelta>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: Delta,
}

#[derive(Deserialize, Default)]
struct Delta {
    content: Option<String>,
    #[serde(alias = "reasoning")]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: usize,
    id: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Deserialize)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct UsageDelta {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    #[test]
    fn request_body_carries_system_tool_calls_and_tool_results() {
        let provider =
            CompatibleProvider::new("key", None, Some("gpt-test".into()), None, None, None);

        let messages = vec![
            LlmChatMessage::user().content("count the users").build(),
            LlmChatMessage::assistant()
                .content("Counting now.")
                .tool_use(vec![tool_call(
                    "call_1",
                    "execute-sql",
                    r#"{"sql":"SELECT COUNT(*) FROM users"}"#,
                )])
                .build(),
            LlmChatMessage::user()
                .tool_result(vec![tool_call(
                    "call_1",
                    "execute-sql",
                    r#"{"rows":[["42"]]}"#,
                )])
                .build(),
            LlmChatMessage::assistant()
                .content("There are 42 users.")
                .build(),
        ];

        let body = provider.build_body("You are a SQL assistant.", &messages, None);
        let value = serde_json::to_value(&body).expect("request body should serialize");
        let wire_messages = value["messages"]
            .as_array()
            .expect("messages should be an array");

        assert_eq!(wire_messages.len(), 5);
        assert_eq!(wire_messages[0]["role"], "system");
        assert_eq!(wire_messages[0]["content"], "You are a SQL assistant.");
        assert_eq!(wire_messages[1]["role"], "user");
        assert_eq!(wire_messages[2]["role"], "assistant");
        assert_eq!(wire_messages[2]["content"], "Counting now.");
        assert_eq!(wire_messages[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(wire_messages[2]["tool_calls"][0]["type"], "function");
        assert_eq!(
            wire_messages[2]["tool_calls"][0]["function"]["name"],
            "execute-sql"
        );
        assert_eq!(wire_messages[3]["role"], "tool");
        assert_eq!(wire_messages[3]["tool_call_id"], "call_1");
        assert_eq!(wire_messages[3]["content"], r#"{"rows":[["42"]]}"#);
        assert!(wire_messages[3].get("tool_calls").is_none());
        assert_eq!(wire_messages[4]["role"], "assistant");
        assert!(wire_messages[4].get("tool_call_id").is_none());
    }

    #[test]
    fn tool_use_without_text_omits_content() {
        let provider = CompatibleProvider::new("key", None, None, None, None, None);
        let messages = vec![
            LlmChatMessage::assistant()
                .tool_use(vec![tool_call("call_2", "read-tab", "{}")])
                .build(),
        ];

        let body = provider.build_body("prompt", &messages, None);
        let value = serde_json::to_value(&body).expect("request body should serialize");

        assert!(value["messages"][1].get("content").is_none());
        assert_eq!(value["messages"][1]["tool_calls"][0]["id"], "call_2");
    }
}
