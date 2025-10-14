//! LSP client for PostgreSQL language server communication
//!
//! This module handles JSON-RPC communication with the PostgreSQL language server,
//! including request/response handling and notification processing.

use crate::config::PostgresLspConfig;
use crate::process::PostgresLspProcess;
use anyhow::Result;
use lsp_types::*;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};
use url::Url;
use std::sync::Arc;

/// LSP client errors
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("JSON-RPC error: {0}")]
    JsonRpc(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Request timeout")]
    Timeout,
    #[error("Server error: {0}")]
    Server(String),
    #[error("Invalid response")]
    InvalidResponse,
}

/// JSON-RPC request
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

/// JSON-RPC response
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    data: Option<Value>,
}

/// JSON-RPC notification
#[derive(Debug, Deserialize, serde::Serialize)]
struct JsonRpcNotification {
    jsonrpc: String,
    method: String,
    params: Option<Value>,
}

/// Workspace configuration parameters
#[derive(Debug, Serialize)]
struct ConfigurationParams {
    items: Vec<ConfigurationItem>,
}

/// Configuration item
#[derive(Debug, Serialize)]
struct ConfigurationItem {
    scope_uri: Option<String>,
    section: Option<String>,
}

/// Diagnostic handler callback type
pub type DiagnosticHandler = Arc<dyn Fn(PublishDiagnosticsParams) + Send + Sync>;

/// LSP client for PostgreSQL language server
pub struct PostgresLspClient {
    process: PostgresLspProcess,
    next_id: AtomicU64,
    pending_requests: HashMap<Value, mpsc::Sender<Result<Value, ClientError>>>,
    config: PostgresLspConfig,
    diagnostic_handler: Option<DiagnosticHandler>,
}

impl PostgresLspClient {
    /// Create a new LSP client
    ///
    /// # Arguments
    /// * `process` - The LSP process to communicate with
    /// * `config` - LSP configuration
    /// * `workspace_path` - Path to the workspace directory
    ///
    /// # Returns
    /// * `Result<Self, ClientError>` - LSP client or error
    pub async fn new(
        process: PostgresLspProcess,
        config: &PostgresLspConfig,
        workspace_path: &std::path::Path,
    ) -> Result<Self, ClientError> {
        let mut client = Self {
            process,
            next_id: AtomicU64::new(1),
            pending_requests: HashMap::new(),
            config: config.clone(),
            diagnostic_handler: None,
        };

        // Initialize the LSP connection
        client.initialize(workspace_path).await?;

        Ok(client)
    }

    /// Initialize the LSP connection
    async fn initialize(&mut self, workspace_path: &std::path::Path) -> Result<(), ClientError> {
        info!(
            "🚀 Initializing PostgreSQL LSP client with workspace: {:?}",
            workspace_path
        );

        // Convert workspace path to URI
        let workspace_uri = match url::Url::from_directory_path(workspace_path) {
            Ok(uri) => {
                info!("🔗 Converted workspace path to URI: {}", uri);
                uri
            }
            Err(e) => {
                error!("🔴 Failed to convert workspace path to URI: {:?}", e);
                return Err(ClientError::Server(format!(
                    "Invalid workspace path: {:?}",
                    e
                )));
            }
        };

        // Convert url::Url to lsp_types::Uri
        let workspace_lsp_uri = workspace_uri
            .to_string()
            .parse::<Uri>()
            .map_err(|e| ClientError::Server(format!("Failed to parse workspace URI: {:?}", e)))?;

        info!("🎯 Final LSP URI: {:?}", workspace_lsp_uri);

        let capabilities = ClientCapabilities {
            workspace: Some(WorkspaceClientCapabilities {
                workspace_folders: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

        // Create workspace folder
        let workspace_folder = WorkspaceFolder {
            uri: workspace_lsp_uri.clone(),
            name: "Blanco SQL Workspace".to_string(),
        };

        info!(
            "📁 Workspace folder: {} -> {:?}",
            workspace_folder.name, workspace_folder.uri
        );

        // Create initialize parameters with workspace root URI
        let init_params = InitializeParams {
            root_uri: Some(workspace_lsp_uri.clone()),
            root_path: None, // deprecated, use root_uri instead
            workspace_folders: Some(vec![workspace_folder]),
            initialization_options: None,
            capabilities: capabilities,
            trace: None,
            client_info: None,
            locale: None,
            work_done_progress_params: WorkDoneProgressParams::default(),
            process_id: Some(std::process::id()),
        };

        info!("📤 Sending initialize request with workspace folders:");
        if let Some(ref folders) = init_params.workspace_folders {
            for folder in folders {
                info!("  📂 Folder: {} -> {:?}", folder.name, folder.uri);
            }
        }
        info!("  🌍 Root URI: {:?}", init_params.root_uri);

        let response = self
            .request("initialize", Some(serde_json::to_value(init_params)?))
            .await?;

        // Send initialized notification
        self.notification("initialized", Some(serde_json::json!({})))
            .await?;

        // Send workspace folder change notification to ensure workspace folders are registered
        let workspace_folders_params = serde_json::json!({
            "event": {
                "added": [
                    {
                        "uri": workspace_lsp_uri,
                        "name": "Blanco SQL Workspace"
                    }
                ],
                "removed": []
            }
        });
        self.notification(
            "workspace/didChangeWorkspaceFolders",
            Some(workspace_folders_params),
        )
        .await?;

        info!("✅ PostgreSQL LSP client initialized successfully");
        Ok(())
    }

    /// Send a request to the LSP server
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let id_value = Value::Number(serde_json::Number::from(id));

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id_value.clone()),
            method: method.to_string(),
            params: params.clone(),
        };

        let request_json = serde_json::to_string(&request)?;

        // Enhanced tracing for outgoing request
        info!("🔵 LSP Request -> {} (ID: {})", method, id);
        if let Some(params) = &params {
            debug!(
                "🔵 Request Params: {}",
                serde_json::to_string_pretty(params)?
            );
        } else {
            debug!("🔵 Request Params: <none>");
        }
        trace!("🔵 Raw Request JSON: {}", request_json);

        // Send request
        if let Some(stdin) = self.process.stdin() {
            let header = format!("Content-Length: {}\r\n\r\n", request_json.len());
            debug!("🔵 Sending header: {}", header.trim());
            stdin.write_all(header.as_bytes()).await?;
            stdin.write_all(request_json.as_bytes()).await?;
            stdin.flush().await?;
            debug!("🔵 Request sent successfully");
        } else {
            error!("🔴 Failed to send request: stdin not available");
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process stdin not available",
            )));
        }

        // Wait for response (simplified for now - in a real implementation,
        // we'd need to handle responses asynchronously)
        let response = self.wait_for_response(&id_value).await;

        // Enhanced tracing for response
        match &response {
            Ok(response_value) => {
                info!("🟢 LSP Response <- {} (ID: {})", method, id);
                debug!(
                    "🟢 Response: {}",
                    serde_json::to_string_pretty(response_value)?
                );
            }
            Err(e) => {
                error!("🔴 LSP Error <- {} (ID: {}): {:?}", method, id, e);
            }
        }

        response
    }

    /// Send a notification to the LSP server
    async fn notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), ClientError> {
        let notification = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params.clone(),
        };

        let notification_json = serde_json::to_string(&notification)?;

        // Enhanced tracing for outgoing notification
        info!("📤 LSP Notification -> {}", method);
        if let Some(params) = &params {
            debug!(
                "📤 Notification Params: {}",
                serde_json::to_string_pretty(params)?
            );
        } else {
            debug!("📤 Notification Params: <none>");
        }
        trace!("📤 Raw Notification JSON: {}", notification_json);

        // Send notification
        if let Some(stdin) = self.process.stdin() {
            let header = format!("Content-Length: {}\r\n\r\n", notification_json.len());
            debug!("📤 Sending notification header: {}", header.trim());
            stdin.write_all(header.as_bytes()).await?;
            stdin.write_all(notification_json.as_bytes()).await?;
            stdin.flush().await?;
            debug!("📤 Notification sent successfully");
        } else {
            error!("🔴 Failed to send notification: stdin not available");
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process stdin not available",
            )));
        }

        Ok(())
    }

    /// Wait for a response with the given ID
    async fn wait_for_response(&mut self, expected_id: &Value) -> Result<Value, ClientError> {
        debug!("⏳ Waiting for LSP response (ID: {})", expected_id);

        // This is a simplified implementation
        // In a real implementation, we'd need to read responses asynchronously
        // and match them to pending requests

        if let Some(stdout) = self.process.stdout() {
            let mut buffer = vec![0u8; 8192];
            let mut response_data = Vec::new();
            let mut attempts = 0;

            loop {
                attempts += 1;
                debug!("🔍 Reading from stdout (attempt {})", attempts);

                match stdout.read(&mut buffer).await {
                    Ok(0) => {
                        warn!(
                            "🔴 EOF reached while waiting for response (ID: {})",
                            expected_id
                        );
                        break;
                    }
                    Ok(n) => {
                        debug!("📥 Read {} bytes from LSP server", n);
                        response_data.extend_from_slice(&buffer[..n]);
                        trace!(
                            "📥 Raw response data: {}",
                            String::from_utf8_lossy(&response_data)
                        );

                        // Try to parse a complete response
                        let response_str =
                            PostgresLspClient::extract_complete_response_static(&response_data);

                        if let Some(response_str) = response_str {
                            debug!("📨 Complete response received: {}", response_str);

                            // Check if this might be a workspace/configuration request
                            if response_str.contains("workspace/configuration") {
                                warn!("🟡 LSP server is requesting workspace configuration!");
                                warn!("🟡 This might indicate the server isn't detecting workspace folders properly");
                            }

                            // Check if this might be a workspace/workspaceFolders request
                            if response_str.contains("workspace/workspaceFolders") {
                                warn!("🟡 LSP server is requesting workspace folders!");
                                warn!("🟡 This indicates the server doesn't have workspace folders registered");
                            }

                            // First try to parse as a notification (no ID)
                        let is_notification = match serde_json::from_str::<JsonRpcNotification>(response_str) {
                            Ok(_) => {
                                info!("📨 Parsed JSON-RPC notification");
                                true
                            }
                            Err(_) => {
                                // Not a notification, try parsing as a response
                                false
                            }
                        };

                        if is_notification {
                            // Handle notifications separately to avoid borrow issues
                            let notifications_to_process = Vec::from([response_str]);
                            for notification_str in notifications_to_process {
                                match serde_json::from_str::<JsonRpcNotification>(notification_str) {
                                    Ok(notification) => {
                                        // Clone the parts we need for the handler
                                        let handler = self.diagnostic_handler.clone();
                                        let method = notification.method.clone();
                                        let params = notification.params.clone();

                                        // Handle the notification directly (no tokio::spawn)
                                        info!("📨 Processing notification: {}", method);
                                        if method == "textDocument/publishDiagnostics" {
                                            if let Some(params) = params {
                                                match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                                                    Ok(diagnostic_params) => {
                                                        info!("🔍 Received {} diagnostics for {:?}",
                                                            diagnostic_params.diagnostics.len(),
                                                            diagnostic_params.uri);

                                                        if let Some(handler) = handler {
                                                            handler(diagnostic_params);
                                                        } else {
                                                            warn!("🟡 No diagnostic handler set, ignoring diagnostics");
                                                        }
                                                    }
                                                    Err(e) => {
                                                        error!("🔴 Failed to parse diagnostics: {}", e);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("🔴 Failed to parse notification: {}", e);
                                    }
                                }
                            }

                            // Remove the processed notification from the buffer
                            response_data.drain(0..response_str.len());
                            continue;
                        }

                        // Try to parse as a response
                        match serde_json::from_str::<JsonRpcResponse>(response_str) {
                            Ok(response) => {
                                debug!("📨 Parsed JSON-RPC response: {:?}", response);

                                if let Some(response_id) = &response.id {
                                    if response_id == expected_id {
                                        info!(
                                            "✅ Response ID matches expected ID: {}",
                                            expected_id
                                        );

                                        if let Some(error) = response.error {
                                            error!(
                                                "🔴 LSP Server Error (ID: {}): {} - {:?}",
                                                expected_id, error.message, error.data
                                            );
                                            return Err(ClientError::Server(error.message));
                                        }

                                        if let Some(result) = response.result {
                                            info!(
                                                "✅ LSP Success Response (ID: {})",
                                                expected_id
                                            );
                                            debug!(
                                                "✅ Result: {}",
                                                serde_json::to_string_pretty(&result)?
                                            );
                                            return Ok(result);
                                        }

                                        warn!(
                                            "⚠️ Response has no result or error (ID: {})",
                                            expected_id
                                        );
                                        return Err(ClientError::InvalidResponse);
                                    } else {
                                        debug!(
                                            "⏭️ Response ID {} doesn't match expected ID {}",
                                            response_id, expected_id
                                        );
                                        // Continue waiting for the correct response
                                    }
                                } else {
                                    warn!("⚠️ Response has no ID field - treating as notification");
                                    // Try to parse as notification again for safety
                                    drop(response);
                                    match serde_json::from_str::<JsonRpcNotification>(response_str) {
                                        Ok(notification) => {
                                            let handler = self.diagnostic_handler.clone();
                                            let method = notification.method.clone();
                                            let params = notification.params.clone();

                                            // Handle the notification directly (no tokio::spawn)
                                            info!("📨 Processing notification: {}", method);
                                            if method == "textDocument/publishDiagnostics" {
                                                if let Some(params) = params {
                                                    match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                                                        Ok(diagnostic_params) => {
                                                            info!("🔍 Received {} diagnostics for {:?}",
                                                                diagnostic_params.diagnostics.len(),
                                                                diagnostic_params.uri);

                                                            if let Some(handler) = handler {
                                                                handler(diagnostic_params);
                                                            } else {
                                                                warn!("🟡 No diagnostic handler set, ignoring diagnostics");
                                                            }
                                                        }
                                                        Err(e) => {
                                                            error!("🔴 Failed to parse diagnostics: {}", e);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!("🔴 Failed to parse as notification: {}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("🔴 Failed to parse JSON-RPC response: {}", e);
                                trace!("🔴 Invalid JSON: {}", response_str);
                            }
                        }
                        } else {
                            debug!("⏳ Incomplete response, continuing to read...");
                        }
                    }
                    Err(e) => {
                        error!("🔴 Error reading from stdout: {}", e);
                        return Err(ClientError::Io(e));
                    }
                }

                // Prevent infinite loop
                if attempts > 100 {
                    error!(
                        "🔴 Timeout waiting for response (ID: {}) after {} attempts",
                        expected_id, attempts
                    );
                    return Err(ClientError::Timeout);
                }
            }
        } else {
            error!("🔴 Process stdout not available");
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process stdout not available",
            )));
        }

        error!("🔴 No response received (ID: {})", expected_id);
        Err(ClientError::Timeout)
    }

    /// Extract a complete JSON-RPC response from the buffer
    fn extract_complete_response<'a>(&self, buffer: &'a [u8]) -> Option<&'a str> {
        Self::extract_complete_response_static(buffer)
    }

    fn extract_complete_response_static<'a>(buffer: &'a [u8]) -> Option<&'a str> {
        let response_str = std::str::from_utf8(buffer).ok()?;

        // Look for Content-Length header
        if let Some(header_start) = response_str.find("Content-Length:") {
            let header_part = &response_str[header_start..];
            if let Some(header_end) = header_part.find("\r\n\r\n") {
                let header = &header_part[..header_end];
                if let Some(length_str) = header.split(':').nth(1) {
                    if let Ok(length) = length_str.trim().parse::<usize>() {
                        let content_start = header_start + header_end + 4;
                        let content_end = content_start + length;

                        if content_end <= response_str.len() {
                            return Some(&response_str[content_start..content_end]);
                        }
                    }
                }
            }
        }

        None
    }

    /// Get completion items
    pub async fn completion(
        &mut self,
        params: CompletionParams,
    ) -> Result<CompletionResponse, ClientError> {
        let response = self
            .request(
                "textDocument/completion",
                Some(serde_json::to_value(params)?),
            )
            .await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Get hover information
    pub async fn hover(&mut self, params: HoverParams) -> Result<Option<Hover>, ClientError> {
        let response = self
            .request("textDocument/hover", Some(serde_json::to_value(params)?))
            .await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Get diagnostics (this would typically come from notifications)
    pub async fn request_diagnostics(
        &mut self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReport, ClientError> {
        let response = self
            .request(
                "textDocument/diagnostic",
                Some(serde_json::to_value(params)?),
            )
            .await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Get code actions
    pub async fn code_actions(
        &mut self,
        params: CodeActionParams,
    ) -> Result<Option<Vec<CodeAction>>, ClientError> {
        let response = self
            .request(
                "textDocument/codeAction",
                Some(serde_json::to_value(params)?),
            )
            .await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Get workspace configuration
    pub async fn get_configuration(
        &mut self,
        scope_uri: Option<String>,
        section: Option<String>,
    ) -> Result<Vec<Value>, ClientError> {
        let params = ConfigurationParams {
            items: vec![ConfigurationItem { scope_uri, section }],
        };

        let response = self
            .request(
                "workspace/configuration",
                Some(serde_json::to_value(params)?),
            )
            .await?;

        // Configuration returns an array of values
        match response {
            Value::Array(values) => Ok(values),
            _ => Err(ClientError::InvalidResponse),
        }
    }

    /// Set workspace configuration via notification
    pub async fn set_configuration(&mut self, settings: Value) -> Result<(), ClientError> {
        self.notification("workspace/didChangeConfiguration", Some(settings))
            .await
    }

    /// Open a document
    pub async fn did_open(&mut self, params: DidOpenTextDocumentParams) -> Result<(), ClientError> {
        self.notification("textDocument/didOpen", Some(serde_json::to_value(params)?))
            .await
    }

    /// Change a document
    pub async fn did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Result<(), ClientError> {
        self.notification(
            "textDocument/didChange",
            Some(serde_json::to_value(params)?),
        )
        .await
    }

    /// Close a document
    pub async fn did_close(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> Result<(), ClientError> {
        self.notification("textDocument/didClose", Some(serde_json::to_value(params)?))
            .await
    }

    /// Save a document
    pub async fn did_save(&mut self, params: DidSaveTextDocumentParams) -> Result<(), ClientError> {
        self.notification("textDocument/didSave", Some(serde_json::to_value(params)?))
            .await
    }

    /// Get a reference to the process (for providers that need direct access)
    pub fn process(&self) -> &PostgresLspProcess {
        &self.process
    }

    /// Get a mutable reference to the process
    pub fn process_mut(&mut self) -> &mut PostgresLspProcess {
        &mut self.process
    }

    /// Set the diagnostic handler callback
    pub fn set_diagnostic_handler(&mut self, handler: DiagnosticHandler) {
        self.diagnostic_handler = Some(handler);
    }

    /// Handle incoming notifications
    async fn handle_notification(&self, notification: JsonRpcNotification) -> Result<(), ClientError> {
        info!("📨 LSP Notification <- {}", notification.method);

        match notification.method.as_str() {
            "textDocument/publishDiagnostics" => {
                if let Some(params) = notification.params {
                    match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                        Ok(diagnostic_params) => {
                            info!("🔍 Received {} diagnostics for {:?}",
                                diagnostic_params.diagnostics.len(),
                                diagnostic_params.uri);

                            if let Some(ref handler) = self.diagnostic_handler {
                                handler(diagnostic_params);
                            } else {
                                warn!("🟡 No diagnostic handler set, ignoring diagnostics");
                            }
                        }
                        Err(e) => {
                            error!("🔴 Failed to parse diagnostics: {}", e);
                        }
                    }
                }
            }
            "window/logMessage" => {
                if let Some(ref params) = notification.params {
                    info!("📝 LSP Log Message: {}", serde_json::to_string_pretty(params)?);
                }
            }
            "window/showMessage" => {
                if let Some(ref params) = notification.params {
                    info!("💬 LSP Show Message: {}", serde_json::to_string_pretty(params)?);
                }
            }
            _ => {
                debug!("📄 Unhandled notification: {}", notification.method);
            }
        }

        Ok(())
    }

    /// Check if the process is still running
    pub async fn is_process_running(&mut self) -> bool {
        self.process.is_running().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::Number(serde_json::Number::from(1))),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({"test": "value"})),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("initialize"));
        assert!(json.contains("2.0"));
    }

    #[test]
    fn test_json_rpc_response_deserialization() {
        let json = r#"
        {
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"test": "value"}
        }
        "#;

        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            response.id,
            Some(Value::Number(serde_json::Number::from(1)))
        );
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }
}
