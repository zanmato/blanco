//! Blanco Core Library
//!
//! This crate provides shared traits and types for the Blanco SQL editor.
//! It defines the core interfaces for database connections, completion providers,
//! and chat providers that are implemented by database-specific crates.

pub mod connection_trait;
pub mod chat_provider;
pub mod database_service;

// Re-export main types for convenience
pub use connection_trait::{
    Connection, ConnectionFactory, ConnectionInfo, ConnectionRegistry, ConnectionUIMetadata,
    ColumnInfo, IconName, QueryResult, TableMetadata, TableChangeOperation,
    DatabaseSchemaResult, PaginationInfo, TableSchemaInfo,
};

pub use database_service::DatabaseService;

// Re-export lsp-types Position for convenience
pub use lsp_types::Position;

pub use chat_provider::{
    ChatProvider, ProviderError, ChatCompletionRequest, ChatCompletionResponse,
    CompletionChoice, FinishReason, FunctionCall, FunctionDefinition, Message, StreamChunk,
    StreamChoice, StreamDelta, ToolCall, ToolChoice, ToolDefinition, ToolResult, UsageInfo,
};