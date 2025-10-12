//! PostgreSQL Language Server Integration
//! 
//! This crate provides PostgreSQL language server integration for GPUI applications,
//! including binary downloading, process management, and LSP provider implementations.

pub mod client;
pub mod config;
pub mod manager;
pub mod process;
pub mod providers;
pub mod downloader;

pub use client::PostgresLspClient;
pub use config::{PostgresLspConfig, CompletionConfig, DiagnosticsConfig, FormattingConfig};
pub use process::PostgresLspProcess;
pub use providers::{
    PostgresCompletionProvider, PostgresHoverProvider, 
    PostgresCodeActionProvider
};
pub use downloader::{BinaryDownloader, Architecture, DownloadError};

use gpui::Context;
use std::path::PathBuf;

/// Main PostgreSQL LSP integration point
/// 
/// This struct manages the lifecycle of a PostgreSQL language server process,
/// handles configuration, and provides LSP providers for integration with GPUI editors.
pub struct PostgresLspManager {
    client: Option<PostgresLspClient>,
    process: Option<PostgresLspProcess>,
    config: PostgresLspConfig,
    workspace_dir: Option<tempfile::TempDir>,
    status: LspStatus,
    _subscriptions: Vec<gpui::Subscription>,
}

/// LSP status enumeration
#[derive(Debug, Clone, PartialEq)]
pub enum LspStatus {
    Starting,
    Ready,
    Error(String),
    Disconnected,
}

/// LSP error types
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("Failed to start LSP process: {0}")]
    ProcessStart(String),
    #[error("LSP communication error: {0}")]
    Communication(String),
    #[error("Configuration error: {0}")]
    Configuration(String),
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Binary download error: {0}")]
    BinaryDownload(#[from] DownloadError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Process error: {0}")]
    Process(String),
    #[error("Client error: {0}")]
    Client(String),
    #[error("Config error: {0}")]
    Config(String),
}

impl From<crate::config::ConfigError> for LspError {
    fn from(err: crate::config::ConfigError) -> Self {
        LspError::Config(err.to_string())
    }
}

impl From<crate::process::ProcessError> for LspError {
    fn from(err: crate::process::ProcessError) -> Self {
        LspError::Process(err.to_string())
    }
}

impl From<crate::client::ClientError> for LspError {
    fn from(err: crate::client::ClientError) -> Self {
        LspError::Client(err.to_string())
    }
}

impl PostgresLspManager {
    /// Create new LSP manager for PostgreSQL connection
    /// 
    /// # Arguments
    /// * `connection_string` - PostgreSQL connection string
    /// * `database` - Database name
    /// * `schema` - Optional schema name (defaults to "public")
    /// * `workspace_root` - Optional custom workspace directory
    /// * `cx` - GPUI context
    /// 
    /// # Returns
    /// * `Result<Self, LspError>` - LSP manager or error
    pub async fn new(
        connection_string: String,
        database: String,
        schema: Option<String>,
        workspace_root: Option<PathBuf>,
        cx: &mut Context<'_, Self>,
    ) -> Result<Self, LspError> {
        // Create workspace directory
        let workspace_dir = if let Some(root) = workspace_root {
            Some(tempfile::TempDir::new_in(root)?)
        } else {
            Some(tempfile::TempDir::new()?)
        };

        // Parse connection string to extract connection details
        let connection_config = Self::parse_connection_string(&connection_string)?;
        
        // Create configuration
        let config = PostgresLspConfig {
            connection: connection_config,
            database: Default::default(),
            sql: Default::default(),
            workspace: Default::default(),
        };

        // Create workspace configuration file
        if let Some(ref workspace) = workspace_dir {
            config.create_workspace_config(&workspace.path().to_path_buf())?;
        }

        let mut manager = Self {
            client: None,
            process: None,
            config,
            workspace_dir,
            status: LspStatus::Starting,
            _subscriptions: Vec::new(),
        };

        // Start LSP process
        manager.start_lsp_process(cx).await?;

        Ok(manager)
    }

    /// Start the PostgreSQL language server process
    async fn start_lsp_process(&mut self, cx: &mut Context<'_, Self>) -> Result<(), LspError> {
        let workspace_path = self.workspace_dir
            .as_ref()
            .ok_or_else(|| LspError::Configuration("No workspace directory".to_string()))?
            .path()
            .to_path_buf();

        // Ensure binary is available
        let downloader = BinaryDownloader::new()?;
        let binary_path = downloader.ensure_binary().await?;

        // Start postgrestools LSP process
        let mut process = PostgresLspProcess::new(binary_path, &workspace_path).await?;
        
        // Wait for process to be ready
        process.wait_for_ready().await?;

        // Create LSP client
        let client = PostgresLspClient::new(process, &self.config).await?;

        // Process is moved into client, so we can't store it separately
        // We'll need to redesign this to handle process ownership properly
        self.client = Some(client);
        self.status = LspStatus::Ready;

        Ok(())
    }

    /// Parse PostgreSQL connection string
    fn parse_connection_string(conn_str: &str) -> Result<config::ConnectionConfig, LspError> {
        use url::Url;
        
        let url = Url::parse(conn_str)
            .map_err(|e| LspError::Connection(format!("Invalid connection string: {}", e)))?;

        if url.scheme() != "postgresql" && url.scheme() != "postgres" {
            return Err(LspError::Connection("Invalid scheme, expected postgresql or postgres".to_string()));
        }

        let host = url.host_str()
            .ok_or_else(|| LspError::Connection("No host specified".to_string()))?
            .to_string();

        let port = url.port().unwrap_or(5432);

        let database = url.path()
            .trim_start_matches('/')
            .to_string();

        if database.is_empty() {
            return Err(LspError::Connection("No database specified".to_string()));
        }

        let username = url.username()
            .to_string();

        if username.is_empty() {
            return Err(LspError::Connection("No username specified".to_string()));
        }

        Ok(config::ConnectionConfig {
            connection_string: conn_str.to_string(),
            host,
            port,
            database,
            username,
            schema: None, // Will be set separately
        })
    }

    /// Update connection configuration and restart LSP
    pub async fn update_connection(
        &mut self, 
        connection_string: String,
        cx: &mut Context<'_, Self>
    ) -> Result<(), LspError> {
        // Update configuration and restart LSP
        self.config.connection = Self::parse_connection_string(&connection_string)?;
        
        // Update workspace config
        if let Some(ref workspace) = self.workspace_dir {
            self.config.create_workspace_config(&workspace.path().to_path_buf())?;
        }

        // Restart LSP with new configuration
        self.restart(cx).await
    }

    /// Restart the LSP process
    pub async fn restart(&mut self, cx: &mut Context<'_, Self>) -> Result<(), LspError> {
        // Shutdown existing process
        if let Some(process) = self.process.take() {
            process.shutdown().await?;
        }

        // Start new process
        self.start_lsp_process(cx).await
    }

    /// Shutdown the LSP process
    pub async fn shutdown(&mut self, cx: &mut Context<'_, Self>) -> Result<(), LspError> {
        if let Some(process) = self.process.take() {
            process.shutdown().await?;
        }
        self.status = LspStatus::Disconnected;
        Ok(())
    }

    /// Get current LSP status
    pub fn status(&self) -> LspStatus {
        self.status.clone()
    }

    /// Get completion provider for gpui-component
    pub fn completion_provider(&self) -> PostgresCompletionProvider {
        PostgresCompletionProvider::new(
            (),
            self.config.clone()
        )
    }

    /// Get hover provider
    pub fn hover_provider(&self) -> PostgresHoverProvider {
        PostgresHoverProvider::new(
            (),
            self.config.clone()
        )
    }



    /// Get code action provider
    pub fn code_action_provider(&self) -> PostgresCodeActionProvider {
        PostgresCodeActionProvider::new(
            (),
            self.config.clone()
        )
    }
}

impl Drop for PostgresLspManager {
    fn drop(&mut self) {
        // Ensure process is cleaned up
        if let Some(mut process) = self.process.take() {
            // Note: In drop, we can't use async, so we force kill synchronously
            // This is a best-effort cleanup
            let _ = process.child.kill();
        }
    }
}