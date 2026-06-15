//! OpenAI-compatible chat provider.
//!
//! The `llm` crate's native `LLMBackend::OpenAI` switched to OpenAI's `/responses`
//! endpoint in 1.3.x. That endpoint is OpenAI-proprietary and is not implemented by
//! the endpoints we actually target (OpenAI's own `/chat/completions`, z.AI, and other
//! OpenAI-compatible servers), so requests fail against them.
//!
//! The crate still ships a `/chat/completions` implementation in
//! `OpenAICompatibleProvider<T>`, but it only implements `ChatProvider`; the full
//! `LLMProvider` trait is implemented per concrete provider alias (Groq, Mistral, ...).
//! The orphan rule prevents us from adding `impl LLMProvider for
//! OpenAICompatibleProvider<OurConfig>` from this crate, so we wrap it in a local
//! newtype and delegate, mirroring how the `llm` crate's own backends do it.

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use llm::{
    LLMProvider,
    chat::{ChatMessage, ChatProvider, ChatResponse, StreamChunk, StreamResponse, Tool},
    completion::{CompletionProvider, CompletionRequest, CompletionResponse},
    embedding::EmbeddingProvider,
    error::LLMError,
    models::ModelsProvider,
    providers::openai_compatible::{OpenAICompatibleProvider, OpenAIProviderConfig},
    stt::SpeechToTextProvider,
    tts::TextToSpeechProvider,
};

/// Marker config selecting OpenAI-compatible chat-completions behaviour.
///
/// `base_url` and `model` are always supplied from settings at construction time, so
/// the `DEFAULT_*` constants only act as a last-resort fallback when a user leaves the
/// base URL blank.
struct CompatibleConfig;

impl OpenAIProviderConfig for CompatibleConfig {
    const PROVIDER_NAME: &'static str = "OpenAI Compatible";
    const DEFAULT_BASE_URL: &'static str = "https://api.openai.com/v1/";
    const DEFAULT_MODEL: &'static str = "gpt-4o-mini";
    const SUPPORTS_REASONING_EFFORT: bool = true;
    const SUPPORTS_STRUCTURED_OUTPUT: bool = true;
    // Left disabled so we never force a `parallel_tool_calls` field onto endpoints that
    // reject it. Tool calling itself works regardless of this flag.
    const SUPPORTS_PARALLEL_TOOL_CALLS: bool = false;
    const SUPPORTS_STREAM_OPTIONS: bool = true;
}

/// An OpenAI-compatible provider that talks to the `/chat/completions` endpoint.
///
/// Point `base_url` at any OpenAI-compatible server (OpenAI, z.AI, a local proxy, ...).
pub struct CompatibleProvider {
    inner: OpenAICompatibleProvider<CompatibleConfig>,
}

impl CompatibleProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: Option<String>,
        model: Option<String>,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        timeout_seconds: Option<u64>,
    ) -> Self {
        let inner = OpenAICompatibleProvider::<CompatibleConfig>::new(
            api_key,
            base_url,
            model,
            max_tokens,
            temperature,
            timeout_seconds,
            None, // system
            None, // top_p
            None, // top_k
            None, // tools
            None, // tool_choice
            None, // reasoning_effort
            None, // json_schema
            None, // voice
            None, // extra_body
            None, // parallel_tool_calls
            None, // normalize_response (defaults to true)
            None, // embedding_encoding_format
            None, // embedding_dimensions
        );
        Self { inner }
    }
}

#[async_trait]
impl ChatProvider for CompatibleProvider {
    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
    ) -> Result<Box<dyn ChatResponse>, LLMError> {
        self.inner.chat_with_tools(messages, tools).await
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, LLMError>> + Send>>, LLMError> {
        self.inner.chat_stream(messages).await
    }

    async fn chat_stream_struct(
        &self,
        messages: &[ChatMessage],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamResponse, LLMError>> + Send>>, LLMError> {
        self.inner.chat_stream_struct(messages).await
    }

    async fn chat_stream_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, LLMError>> + Send>>, LLMError> {
        self.inner.chat_stream_with_tools(messages, tools).await
    }
}

// The remaining `LLMProvider` supertraits are not applicable to a chat-only provider.
// They return clear errors rather than silently succeeding, matching how the `llm`
// crate's own OpenAI-compatible backends handle these capabilities.

#[async_trait]
impl CompletionProvider for CompatibleProvider {
    async fn complete(&self, _req: &CompletionRequest) -> Result<CompletionResponse, LLMError> {
        Err(LLMError::ProviderError(
            "Text completion is not supported by the OpenAI-compatible chat provider".into(),
        ))
    }
}

#[async_trait]
impl EmbeddingProvider for CompatibleProvider {
    async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
        Err(LLMError::ProviderError(
            "Embeddings are not supported by the OpenAI-compatible chat provider".into(),
        ))
    }
}

#[async_trait]
impl SpeechToTextProvider for CompatibleProvider {
    async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
        Err(LLMError::ProviderError(
            "Speech-to-text is not supported by the OpenAI-compatible chat provider".into(),
        ))
    }
}

#[async_trait]
impl TextToSpeechProvider for CompatibleProvider {}

#[async_trait]
impl ModelsProvider for CompatibleProvider {}

impl LLMProvider for CompatibleProvider {}
