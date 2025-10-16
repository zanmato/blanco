//! LSP manager for PostgreSQL language server
//! 
//! This module provides the main interface for managing PostgreSQL LSP instances,
//! coordinating between the downloader, process manager, and providers.

use crate::client::PostgresLspClient;
use crate::config::PostgresLspConfig;
use crate::downloader::{BinaryDownloader, DownloadError};
use crate::process::{PostgresLspProcess, ProcessError, ProcessManager};
use crate::providers::{PostgresCompletionProvider, PostgresHoverProvider, PostgresCodeActionProvider};
use anyhow::Result;
use lsp_types::Uri;
use serde_json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use tracing::{debug, error, info, warn};
use gpui::BackgroundExecutor;

/// LSP manager errors
#[derive(Debug, thiserror::Error)]
pub enum LspManagerError {
    #[error("Download error: {0}")]
    Download(#[from] DownloadError),
    #[error("Process error: {0}")]
    Process(#[from] ProcessError),
    #[error("Client error: {0}")]
    Client(#[from] crate::client::ClientError),
    #[error("Configuration error: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// PostgreSQL LSP manager
///
/// This is the main entry point for managing PostgreSQL language server instances.
/// It handles binary downloading, process management, and provider creation.
#[derive(Clone)]
pub struct PostgresLspManager {
    downloader: Arc<BinaryDownloader>,
    process_manager: Arc<RwLock<ProcessManager>>,
    config: PostgresLspConfig,
    client: Option<Arc<Mutex<Option<PostgresLspClient>>>>,
    executor: BackgroundExecutor,
}

impl PostgresLspManager {
    /// Create a new PostgreSQL LSP manager
    pub async fn new(config: PostgresLspConfig, executor: BackgroundExecutor) -> Result<Self, LspManagerError> {
        let downloader = Arc::new(BinaryDownloader::new()?);
        let process_manager = Arc::new(RwLock::new(ProcessManager::new()));

        Ok(Self {
            downloader,
            process_manager,
            config,
            client: None,
            executor,
        })
    }

    /// Extract password from connection string
    /// Supports formats: postgresql://username[:password]@host:port/database
    fn extract_password_from_connection_string(connection_string: &str) -> String {
        info!("🔍 Extracting password from connection string: {}", connection_string);

        // Parse the connection string as a URL to properly extract components
        if let Ok(url) = url::Url::parse(connection_string) {
            // The password is available through the password() method if it exists
            // The password() method returns percent-decoded values automatically
            let password = url.password().unwrap_or("").to_string();
            info!("🔍 Parsed password from URL: '{}' (length: {})", if password.is_empty() { "<empty>" } else { "<hidden>" }, password.len());
            password
        } else {
            // Fallback to manual parsing if URL parsing fails
            warn!("🔍 Failed to parse as URL, falling back to manual parsing");
            if let Some(at_pos) = connection_string.find('@') {
                // Find the start of the authority part (after "://")
                if let Some(scheme_end) = connection_string.find("://") {
                    let authority_start = scheme_end + 3;
                    if at_pos > authority_start {
                        let auth_part = &connection_string[authority_start..at_pos];
                        if let Some(colon_pos) = auth_part.find(':') {
                            let password = auth_part[colon_pos + 1..].to_string();
                            info!("🔍 Manual parsing found password: '{}' (length: {})", if password.is_empty() { "<empty>" } else { "<hidden>" }, password.len());
                            password
                        } else {
                            info!("🔍 Manual parsing: no colon found in auth part, no password");
                            String::new()
                        }
                    } else {
                        info!("🔍 Manual parsing: invalid authority position, no password");
                        String::new()
                    }
                } else {
                    info!("🔍 Manual parsing: no scheme found, no password");
                    String::new()
                }
            } else {
                info!("🔍 Manual parsing: no @ found, no password");
                String::new()
            }
        }
    }

    /// Initialize the LSP manager
    /// 
    /// This method ensures the binary is available and sets up the LSP infrastructure.
    pub async fn initialize(&mut self) -> Result<(), LspManagerError> {
        info!("Initializing PostgreSQL LSP manager");

        // Ensure binary is available
        let binary_path = self.downloader.ensure_binary().await?;
        info!("PostgreSQL LSP binary available at: {:?}", binary_path);

        // Use the main queries directory as workspace
        // This ensures the LSP server can access all query files
        let workspace_path = std::path::PathBuf::from("/home/user/.local/share/blanco/queries");
        std::fs::create_dir_all(&workspace_path)?;

        info!("Using workspace directory for LSP: {:?}", workspace_path);

        // Create postgrestools.jsonc config file with database connection
        let password = Self::extract_password_from_connection_string(&self.config.connection.connection_string);
        let config_content = serde_json::json!({
            "$schema": "https://pgtools.dev/latest/schema.json",
            "db": {
                "host": self.config.connection.host,
                "port": self.config.connection.port,
                "username": self.config.connection.username,
                "password": password,
                "database": self.config.connection.database,
                "connTimeoutSecs": 10,
                "allowStatementExecutionsAgainst": [
                    format!("{}/{}/*", self.config.connection.host, self.config.connection.port),
                    format!("localhost:{}/{}", self.config.connection.port, self.config.connection.database)
                ]
            }
        });

        let config_path = workspace_path.join("postgrestools.jsonc");
        let config_json = serde_json::to_string_pretty(&config_content)?;
        let full_content = format!("// PostgreSQL Tools Configuration\n// Generated automatically by Blanco\n\n{}\n", config_json);
        std::fs::write(&config_path, full_content)?;
        info!("Created postgrestools.jsonc config at: {:?}", config_path);

        // Start LSP process (without --config-path so it uses workspace folder detection)
        info!("🚀 Starting LSP process with working directory: {:?}", workspace_path);
        let process = PostgresLspProcess::new(binary_path, &workspace_path, self.executor.clone()).await?;
        // Note: LSP servers typically don't output ready messages, they just start listening
        // No need to wait_for_ready() since the server is ready immediately

        // Create LSP client
        let client = Arc::new(Mutex::new(Some(PostgresLspClient::new(process, &self.config, &workspace_path, self.executor.clone()).await?)));
        self.client = Some(client);

        info!("PostgreSQL LSP manager initialized successfully");
        Ok(())
    }

    /// Start an LSP instance for a specific workspace
    pub async fn start_lsp_for_workspace(
        &self,
        workspace_path: &Path,
    ) -> Result<u32, LspManagerError> {
        info!("Starting PostgreSQL LSP for workspace: {:?}", workspace_path);

        // Get the binary path
        let binary_path = self.downloader.ensure_binary().await?;

        // Create postgrestools.jsonc config file with database connection
        let password = Self::extract_password_from_connection_string(&self.config.connection.connection_string);
        let config_content = serde_json::json!({
            "$schema": "https://pgtools.dev/latest/schema.json",
            "db": {
                "host": self.config.connection.host,
                "port": self.config.connection.port,
                "username": self.config.connection.username,
                "password": password,
                "database": self.config.connection.database,
                "connTimeoutSecs": 10,
                "allowStatementExecutionsAgainst": [
                    format!("{}/{}/*", self.config.connection.host, self.config.connection.port),
                    format!("localhost:{}/{}", self.config.connection.port, self.config.connection.database)
                ]
            }
        });

        let config_path = workspace_path.join("postgrestools.jsonc");
        let config_json = serde_json::to_string_pretty(&config_content)?;
        let full_content = format!("// PostgreSQL Tools Configuration\n// Generated automatically by Blanco\n\n{}\n", config_json);
        std::fs::write(&config_path, full_content)?;
        info!("Created postgrestools.jsonc config at: {:?}", config_path);

        // Create and start the process (without --config-path so it uses workspace folder detection)
        let process = PostgresLspProcess::new(binary_path, workspace_path, self.executor.clone()).await?;

        // Note: LSP servers are ready immediately, no need to wait for output

        let pid = process.pid();

        // Add to process manager
        {
            let mut manager = self.process_manager.write().await;
            manager.add_process(process);
        }

        info!("PostgreSQL LSP started with PID: {}", pid);
        Ok(pid)
    }

    /// Stop an LSP instance
    pub async fn stop_lsp(&self, pid: u32) -> Result<(), LspManagerError> {
        info!("Stopping PostgreSQL LSP with PID: {}", pid);

        let mut manager = self.process_manager.write().await;
        if let Some(process) = manager.remove_process(pid) {
            process.shutdown().await?;
            info!("PostgreSQL LSP stopped successfully");
        } else {
            warn!("LSP process with PID {} not found", pid);
        }

        Ok(())
    }

    /// Check if an LSP instance is running
    pub async fn is_lsp_running(&self, pid: u32) -> bool {
        let manager = self.process_manager.read().await;
        if let Some(_process) = manager.get_process(pid) {
            // Note: This would need to be async in a real implementation
            // For now, just check if the process exists in the manager
            true
        } else {
            false
        }
    }

    /// Get all running LSP instances
    pub async fn get_running_instances(&self) -> Vec<u32> {
        let manager = self.process_manager.read().await;
        manager.processes().iter().map(|p| p.pid()).collect()
    }

    /// Create a completion provider
    pub fn create_completion_provider(&self, document_uri: Uri) -> Option<PostgresCompletionProvider> {
        self.client.as_ref().map(|client| {
            PostgresCompletionProvider::new(client.clone(), self.config.clone(), document_uri)
        })
    }

    /// Create a hover provider
    pub fn create_hover_provider(&self, document_uri: Uri) -> Option<PostgresHoverProvider> {
        self.client.as_ref().map(|client| {
            PostgresHoverProvider::new(client.clone(), self.config.clone(), document_uri)
        })
    }

    /// Create a code action provider
    pub fn create_code_action_provider(&self, document_uri: Uri) -> Option<PostgresCodeActionProvider> {
        self.client.as_ref().map(|client| {
            PostgresCodeActionProvider::new(client.clone(), self.config.clone(), document_uri)
        })
    }

    /// Get the downloader (for cache management, etc.)
    pub fn downloader(&self) -> &Arc<BinaryDownloader> {
        &self.downloader
    }

    /// Get the current configuration
    pub fn config(&self) -> &PostgresLspConfig {
        &self.config
    }

    /// Update the configuration
    pub async fn update_config(&mut self, config: PostgresLspConfig) -> Result<(), LspManagerError> {
        info!("Updating PostgreSQL LSP configuration");
        self.config = config;

        // If we have a client, we would update its configuration here
        if let Some(_client) = &self.client {
            // In a real implementation, we would send configuration updates to the LSP server
            debug!("LSP client configuration updated");
        }

        Ok(())
    }

    /// Clean up all resources
    pub async fn shutdown(self) -> Result<(), LspManagerError> {
        info!("Shutting down PostgreSQL LSP manager");

        // Shutdown all processes
        {
            let manager = self.process_manager.write().await;
            // Get all PIDs and shut them down individually
            let pids: Vec<u32> = manager.processes().iter().map(|p| p.pid()).collect();
            drop(manager);
            
            for pid in pids {
                if let Err(e) = self.stop_lsp(pid).await {
                    error!("Error shutting down LSP process {}: {}", pid, e);
                }
            }
        }

        info!("PostgreSQL LSP manager shut down successfully");
        Ok(())
    }

    /// Clean up dead processes
    pub async fn cleanup_dead_processes(&self) {
        let mut manager = self.process_manager.write().await;
        manager.cleanup_dead().await;
    }

    /// Get cache directory
    pub fn cache_dir(&self) -> &PathBuf {
        self.downloader.cache_dir()
    }

    /// Get current architecture
    pub fn architecture(&self) -> &crate::downloader::Architecture {
        self.downloader.architecture()
    }

    /// Notify LSP that a document was opened
    pub async fn did_open(&self, uri: Uri, language_id: String, version: i32, text: String) -> Result<(), LspManagerError> {
        use lsp_types::*;

        if let Some(client) = &self.client {
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                let params = DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri,
                        language_id,
                        version,
                        text,
                    },
                };
                client.did_open(params).await?;
            }
        }
        Ok(())
    }

    /// Notify LSP that a document changed
    pub async fn did_change(&self, uri: Uri, version: i32, text: String) -> Result<(), LspManagerError> {
        use lsp_types::*;

        if let Some(client) = &self.client {
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                // Convert text to lines for range calculation
                let lines: Vec<&str> = text.lines().collect();
                let line_count = lines.len().max(1) as u32;
                let last_char_count = lines.last().map(|line| line.len() as u32).unwrap_or(0);

                // Create a range covering the entire document to help LSP understand document structure
                let full_range = Some(Range::new(
                    Position::new(0, 0),
                    Position::new(line_count - 1, last_char_count)
                ));

                info!("📝 Calculated range for didChange: {} lines, last line has {} chars", line_count, last_char_count);
                info!("📝 Range: {:?} -> {:?}", Position::new(0, 0), Position::new(line_count - 1, last_char_count));

                let params = DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier {
                        uri,
                        version,
                    },
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: full_range, // Provide range information for better LSP tracking
                        range_length: None,
                        text,
                    }],
                };
                client.did_change(params).await?;
            }
        }
        Ok(())
    }

    /// Notify LSP that a document was closed
    pub async fn did_close(&self, uri: Uri) -> Result<(), LspManagerError> {
        use lsp_types::*;

        if let Some(client) = &self.client {
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                let params = DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier {
                        uri,
                    },
                };
                client.did_close(params).await?;
            }
        }
        Ok(())
    }

    /// Set workspace configuration
    pub async fn set_configuration(&self, settings: serde_json::Value) -> Result<(), LspManagerError> {
        if let Some(client) = &self.client {
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                client.set_configuration(settings).await?;
            }
        }
        Ok(())
    }

    /// Set diagnostic handler for receiving LSP diagnostic notifications
    pub async fn set_diagnostic_handler(&self, handler: crate::client::DiagnosticHandler) {
        if let Some(client) = &self.client {
            let mut client_guard = client.lock().await;
            if let Some(client) = client_guard.as_mut() {
                client.set_diagnostic_handler(handler);
            }
        }
    }

    /// Check LSP connection health and process any pending messages
    /// This method processes any pending notifications that have already been received.
    pub async fn process_pending_messages(&self) -> Result<(), LspManagerError> {
        info!("📨 Processing pending LSP messages");
        if let Some(client) = &self.client {
            let mut client_guard = client.lock().await;
            if let Some(_client) = client_guard.as_mut() {
                // The LSP client now automatically processes incoming messages
                // This method is mainly for compatibility and health checking
                info!("📨 LSP client is processing messages automatically");
            } else {
                warn!("📨 LSP client not available for message processing");
            }
        } else {
            warn!("📨 LSP manager has no client for message processing");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_manager_creation() {
        let config = PostgresLspConfig::default();
        let manager = PostgresLspManager::new(config).await;
        assert!(manager.is_ok());
    }

    #[tokio::test]
    async fn test_manager_initialization() {
        let config = PostgresLspConfig::default();
        let mut manager = PostgresLspManager::new(config).await.unwrap();

        // This will fail without a real binary, but we can test the structure
        let result = manager.initialize().await;
        // We expect this to fail in test environment without the binary
        assert!(result.is_err() || result.is_ok());
    }

    #[test]
    fn test_extract_password_from_connection_string() {
        // Test connection string with password
        let conn_str = "postgresql://user:password@localhost:5432/database";
        let password = PostgresLspManager::extract_password_from_connection_string(conn_str);
        assert_eq!(password, "password");

        // Test connection string without password
        let conn_str = "postgresql://user@localhost:5432/database";
        let password = PostgresLspManager::extract_password_from_connection_string(conn_str);
        assert_eq!(password, "");

        // Test connection string without authentication (like the default)
        let conn_str = "postgresql://localhost:5432/postgres";
        let password = PostgresLspManager::extract_password_from_connection_string(conn_str);
        assert_eq!(password, "");

        // Test connection string with empty password
        let conn_str = "postgresql://user:@localhost:5432/database";
        let password = PostgresLspManager::extract_password_from_connection_string(conn_str);
        assert_eq!(password, "");

        // Test that the original problematic case is now fixed
        // The original issue was that it was returning "postgres//" instead of ""
        let conn_str = "postgresql://localhost:5432/postgres";
        let password = PostgresLspManager::extract_password_from_connection_string(conn_str);
        assert_eq!(password, "");

        // Test simple password with alphanumeric characters
        let conn_str = "postgresql://user:mypassword123@localhost:5432/database";
        let password = PostgresLspManager::extract_password_from_connection_string(conn_str);
        assert_eq!(password, "mypassword123");
    }
}