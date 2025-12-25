//! Blanco Core Library
//!
//! This crate provides shared traits and types for the Blanco SQL editor.
//! It defines the core interfaces for database connections, completion providers,
//! and chat providers that are implemented by database-specific crates.

pub mod chat_provider;
pub mod connection_trait;
pub mod database_service;

// Re-export main types for convenience
pub use connection_trait::{
    ColumnInfo, Connection, ConnectionFactory, ConnectionInfo, ConnectionRegistry,
    ConnectionUIMetadata, DatabaseSchemaResult, PaginationInfo, QueryResult, TableChangeOperation,
    TableMetadata, TableSchemaInfo,
};

pub use database_service::DatabaseService;

// Re-export lsp-types Position for convenience
pub use lsp_types::Position;

pub use chat_provider::{
    ChatCompletionRequest, ChatCompletionResponse, ChatProvider, CompletionChoice, FinishReason,
    FunctionCall, FunctionDefinition, Message, ProviderError, StreamChoice, StreamChunk,
    StreamDelta, ToolCall, ToolChoice, ToolDefinition, ToolResult, UsageInfo,
};
