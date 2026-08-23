//! Streaming chat abstraction.
//!
//! The `llm` crate exposes chat through `LLMProvider`, but its OpenAI-compatible response
//! and stream types only surface `content` + `tool_calls`. Reasoning models (z.ai GLM,
//! DeepSeek, ...) deliver their thinking in a separate `reasoning_content` field that those
//! types silently drop, and the blocking `chat_with_tools` call gives no incremental output
//! at all (so a long reasoning turn shows nothing until it finishes).
//!
//! This module defines a richer streaming interface the chat session drives instead. The
//! OpenAI-compatible provider implements it with a real SSE parse (see
//! [`crate::agent::openai_compatible`]); native backends (Anthropic, Google, Ollama) reuse
//! their blocking `LLMProvider::chat_with_tools` through [`NonStreamingAdapter`], emitting the
//! whole response as a single batch of events so the session loop stays uniform.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use llm::{
    LLMProvider, ToolCall, chat::ChatMessage as LlmChatMessage, chat::Tool, error::LLMError,
};

/// An incremental piece of a streaming chat response.
#[derive(Debug, Clone)]
pub enum ChatStreamEvent {
    /// A chunk of the model's reasoning/thinking (`reasoning_content`).
    Reasoning(String),
    /// A chunk of the user-visible answer (`content`).
    Content(String),
    /// The fully assembled tool calls for this turn, emitted once the stream completes.
    ToolCalls(Vec<ToolCall>),
    /// Token usage for this turn, emitted when the provider reports it.
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
}

pub type ChatEventStream =
    Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, LLMError>> + Send + 'static>>;

/// A chat provider that yields its response incrementally.
///
/// The system prompt is passed per request (not at provider construction) because providers
/// are cached across tabs whose prompts differ.
#[async_trait]
pub trait StreamingChatProvider: Send + Sync {
    async fn stream_chat(
        &self,
        system_prompt: &str,
        messages: &[LlmChatMessage],
        tools: Option<&[Tool]>,
    ) -> Result<ChatEventStream, LLMError>;
}

/// Adapts a blocking [`LLMProvider`] (the native Anthropic/Google/Ollama backends) to the
/// streaming interface by issuing a single `chat_with_tools` call and replaying the response
/// as a short burst of events. There is no token-level streaming here, but the session loop
/// can treat every provider identically.
pub struct NonStreamingAdapter {
    inner: Arc<Box<dyn LLMProvider>>,
}

impl NonStreamingAdapter {
    pub fn new(inner: Arc<Box<dyn LLMProvider>>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl StreamingChatProvider for NonStreamingAdapter {
    async fn stream_chat(
        &self,
        system_prompt: &str,
        messages: &[LlmChatMessage],
        tools: Option<&[Tool]>,
    ) -> Result<ChatEventStream, LLMError> {
        // The llm crate has no per-request system slot (`LLMBuilder::system` is fixed at
        // provider construction, and providers are cached across tabs with different
        // prompts), so the system prompt leads the conversation as a user message.
        let mut request_messages = Vec::with_capacity(messages.len() + 1);
        request_messages.push(LlmChatMessage::user().content(system_prompt).build());
        request_messages.extend_from_slice(messages);

        let response = self.inner.chat_with_tools(&request_messages, tools).await?;

        let mut events: Vec<Result<ChatStreamEvent, LLMError>> = Vec::new();
        if let Some(text) = response.text().filter(|t| !t.is_empty()) {
            events.push(Ok(ChatStreamEvent::Content(text)));
        }
        if let Some(tool_calls) = response.tool_calls().filter(|c| !c.is_empty()) {
            events.push(Ok(ChatStreamEvent::ToolCalls(tool_calls)));
        }
        if let Some(usage) = response.usage() {
            events.push(Ok(ChatStreamEvent::Usage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
            }));
        }

        Ok(Box::pin(futures::stream::iter(events)))
    }
}
