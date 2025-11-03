//! Blanco Core Library
//!
//! This crate provides shared traits and types for the Blanco SQL editor.
//! It defines the core interfaces for database connections, completion providers,
//! and hover providers that are implemented by database-specific crates.

pub mod connection_trait;
pub mod lsp_traits;
pub mod table_operations;
pub mod chat_provider;

// Re-export main types for convenience
pub use connection_trait::{
    Connection, ConnectionFactory, ConnectionInfo, ConnectionRegistry, ConnectionUIMetadata,
    ColumnInfo, IconName, QueryResult, TableMetadata, TableChangeOperation,
};

pub use table_operations::{
    OperationType, RowIdentifier, ColumnChange,
};

pub use lsp_traits::{
    CompletionProvider, HoverProvider, LanguageProvider, LanguageProviderRegistry,
    Position, Range, CompletionContext, CompletionResponse, Hover,
};

pub use chat_provider::{
    ChatProvider, ProviderError, ChatCompletionRequest, ChatCompletionResponse,
    CompletionChoice, FinishReason, FunctionCall, FunctionDefinition, Message, StreamChunk,
    StreamChoice, StreamDelta, ToolCall, ToolChoice, ToolDefinition, ToolResult, UsageInfo,
};