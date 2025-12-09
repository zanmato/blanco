//! # Blanco OpenAI Client
//!
//! A Rust client for the OpenAI API that implements the `ChatProvider` trait from blanco-core.
//! This crate provides:
//!
//! - Full OpenAI chat completions API support
//! - Streaming chat completions
//! - Tool calling support
//! - Robust error handling
//! - Configuration management
//! - Support for `zed-reqwest`
//!
//! ## Quick Start
//!
//! ```ignore
//! use blanco_openai::{OpenAIClient, OpenAIConfig};
//! use blanco_core::chat_provider::*;
//! use zed_reqwest as reqwest;
//! use std::sync::Arc;
//!
//! #[async_std::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create configuration
//!     let config = OpenAIConfig::new("your-api-key-here");
//!
//!     // Create HTTP client using zed-reqwest
//!     let http_client = Arc::new(reqwest::Client::new());
//!
//!     // Create OpenAI client
//!     let client = OpenAIClient::new(http_client, config)?;
//!
//!     // Send a chat completion request
//!     let request = ChatCompletionRequest {
//!         model: "gpt-4".to_string(),
//!         messages: vec![
//!             Message {
//!                 role: "user".to_string(),
//!                 content: "Hello, world!".to_string(),
//!                 tool_call_id: None,
//!                 tool_calls: None,
//!                 additional_data: None,
//!             }
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
pub use tools::{ListTablesTool, ReadTabTool, ToolExecutor, ToolHandler, ToolRegistry};
pub use types::*;

/// Current version of the crate
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;
    use blanco_core::ChatProvider;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn test_provider_trait_send_sync() {
        // Ensure that ChatProvider is Send + Sync
        fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
        _assert_send_sync::<dyn ChatProvider<Error = OpenAIError>>();
    }
}
