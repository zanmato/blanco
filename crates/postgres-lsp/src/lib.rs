//! PostgreSQL Language Server Integration
//! 
//! This crate provides PostgreSQL language server integration for GPUI applications,
//! including binary downloading, process management, and LSP provider implementations.

pub mod client;
pub mod config;
pub mod gpui_tokio;
pub mod manager;
pub mod message_handler;
pub mod process;
pub mod providers;
pub mod downloader;

// Re-export main types for convenience
pub use client::PostgresLspClient;
pub use config::{PostgresLspConfig, CompletionConfig, DiagnosticsConfig, FormattingConfig};
pub use manager::{PostgresLspManager, LspManagerError};
pub use process::PostgresLspProcess;
pub use providers::{
    PostgresCompletionProvider, PostgresHoverProvider, 
    PostgresCodeActionProvider
};
pub use downloader::{BinaryDownloader, Architecture, DownloadError};

/// LSP status enumeration for backward compatibility
#[derive(Debug, Clone, PartialEq)]
pub enum LspStatus {
    Starting,
    Ready,
    Error(String),
    Disconnected,
}

impl From<LspManagerError> for LspStatus {
    fn from(err: LspManagerError) -> Self {
        LspStatus::Error(err.to_string())
    }
}