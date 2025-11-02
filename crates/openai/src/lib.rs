//! # Blanco OpenAI Client
//!
//! A Rust client for the OpenAI API that implements a provider-agnostic `ChatProvider` trait.
//! This crate provides:
//!
//! - A clean `ChatProvider` trait for AI provider abstraction
//! - Full OpenAI chat completions API support
//! - Streaming chat completions
//! - Tool calling support
//! - Robust error handling
//! - Configuration management
//! - Support for `zed-http-client`
//!
//! ## Quick Start
//!
//! ```rust
//! use blanco_openai::{OpenAIClient, OpenAIConfig, ChatProvider, Message};
//! use std::sync::Arc;
//!
//! #[async_std::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create configuration
//!     let config = OpenAIConfig::new("your-api-key-here");
//!
//!     // Create HTTP client (using zed-http-client)
//!     let http_client = Arc::new(/* your HTTP client implementation */);
//!
//!     // Create OpenAI client
//!     let client = OpenAIClient::new(http_client, config)?;
//!
//!     // Send a chat completion request
//!     let request = blanco_openai::ChatCompletionRequest {
//!         model: "gpt-4".to_string(),
//!         messages: vec![
//!             Message::user("Hello, world!")
//!         ],
//!         ..Default::default()
//!     };
//!
//!     let response = client.chat_completion(request).await?;
//!     println!("Response: {}", response.choices[0].message.content);
//!
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod provider;
pub mod tools;
pub mod types;

// Re-export the main types for convenience
pub use client::OpenAIClient;
pub use config::{ConfigError, OpenAIConfig};
pub use error::{OpenAIError, OpenAIResult};
pub use provider::{
    ChatCompletionRequest, ChatCompletionResponse, ChatProvider, CompletionChoice, FinishReason,
    FunctionCall, FunctionDefinition, Message, StreamChoice, StreamChunk, StreamDelta, ToolCall,
    ToolChoice, ToolDefinition, ToolResult, UsageInfo,
};
pub use tools::{ClosureTool, ToolExecutor, ToolHandler, ToolRegistry};
pub use types::*;

/// Current version of the crate
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn test_provider_trait_send_sync() {
        // Ensure that ChatProvider is Send + Sync
        fn _assert_send_sync<T: Send + Sync>() {}
        _assert_send_sync::<dyn ChatProvider<Error = OpenAIError>>();
    }
}
