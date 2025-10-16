//! LSP client for PostgreSQL language server communication
//!
//! This module handles JSON-RPC communication with the PostgreSQL language server,
//! including request/response handling and notification processing using a background
//! message processing system inspired by Zed's LSP implementation.

use crate::config::PostgresLspConfig;
use crate::process::PostgresLspProcess;
use anyhow::Result;
use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use smol::{
    io::{AsyncReadExt, AsyncWriteExt},
};

use futures::{
    FutureExt,
    channel::oneshot,
    select,
};

use futures::lock::Mutex;
use gpui::BackgroundExecutor;
use tracing::{debug, error, info, trace, warn};

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
    pending_requests: Arc<Mutex<HashMap<Value, futures::channel::oneshot::Sender<Result<Value, ClientError>>>>>,
    diagnostic_handler: Option<DiagnosticHandler>,
    executor: BackgroundExecutor,
    /// Buffer for accumulating partial message data
    message_buffer: Vec<u8>,
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
        executor: BackgroundExecutor,
    ) -> Result<Self, ClientError> {
        info!(
            "🚀 Creating PostgreSQL LSP client with workspace: {:?}",
            workspace_path
        );

        // Create the client structure first so we can use it for initialization
        let mut client = Self {
            process,
            next_id: AtomicU64::new(1),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            diagnostic_handler: None,
            executor,
            message_buffer: Vec::new(),
        };

        // Initialize the LSP connection using the actual client
        info!("🔧 Initializing LSP connection");
        client.initialize_process(config, workspace_path).await?;

        info!("✅ PostgreSQL LSP client created successfully");
        Ok(client)
    }

    /// Initialize the LSP connection (separate from client construction)
    async fn initialize_process(
        &mut self,
        _config: &PostgresLspConfig,
        workspace_path: &std::path::Path,
    ) -> Result<(), ClientError> {
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

        // Simplified client capabilities that match the lsp-types version
        let capabilities = ClientCapabilities::default();

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
            capabilities,
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

        let _response = self
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

    /// Process any pending incoming messages with proper buffering
    async fn process_pending_messages(&mut self) -> Result<()> {
        if let Some(stdout) = self.process.stdout() {
            // Use futures::select! for non-blocking I/O
            let (result, data) = select! {
                // Read data from stdout in the background
                data = async {
                    let mut buffer = vec![0u8; 8192];
                    let result = stdout.read(&mut buffer).await;
                    (result, buffer)
                }.fuse() => {
                    match data {
                        (Ok(0), _buffer) => {
                            // EOF - connection closed
                            warn!("🔴 LSP process stdout closed");
                            (Ok(()), Vec::new())
                        }
                        (Ok(n), buffer) => {
                            // Add new data to our buffer
                            if n > 0 {
                                debug!("📥 Read {} bytes from LSP stdout", n);
                                (Ok(()), buffer[..n].to_vec())
                            } else {
                                (Ok(()), Vec::new())
                            }
                        }
                        (Err(e), _buffer) => {
                            error!("🔴 Error reading from stdout: {}", e);
                            (Err(anyhow::anyhow!(e)), Vec::new())
                        }
                    }
                }
                _ = smol::Timer::after(std::time::Duration::from_millis(10)).fuse() => {
                    // Timeout - no data available, which is normal
                    (Ok(()), Vec::new())
                }
            };

            result?;

            // Process the data outside the select context
            if !data.is_empty() {
                self.message_buffer.extend_from_slice(&data);

                // Process all complete messages in the buffer
                while let Some(message) = self.extract_next_message()? {
                    if let Err(e) = self.handle_json_message(message).await {
                        error!("🔴 Error handling message: {}", e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Extract the next complete message from the buffer
    fn extract_next_message(&mut self) -> Result<Option<Value>> {
        if self.message_buffer.is_empty() {
            return Ok(None);
        }

        // Convert buffer to string for header parsing
        let buffer_str = std::str::from_utf8(&self.message_buffer)
            .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in buffer: {}", e))?;

        debug!("📦 Current buffer size: {} bytes", self.message_buffer.len());
        if !self.message_buffer.is_empty() {
            debug!("📦 Buffer preview: {}", &buffer_str[..buffer_str.len().min(200)]);
        }

        // Look for Content-Length header
        if let Some(header_start) = buffer_str.find("Content-Length:") {
            let header_part = &buffer_str[header_start..];
            if let Some(header_end) = header_part.find("\r\n\r\n") {
                let header = &header_part[..header_end];
                if let Some(length_str) = header.split(':').nth(1) {
                    if let Ok(content_length) = length_str.trim().parse::<usize>() {
                        let header_total_length = header_start + header_end + 4;
                        let message_end = header_total_length + content_length;

                        if message_end <= self.message_buffer.len() {
                            // We have a complete message
                            let message_data = self.message_buffer[header_total_length..message_end].to_vec();

                            // Parse the JSON
                            let message = serde_json::from_slice::<Value>(&message_data)?;

                            // Remove the processed message from buffer
                            self.message_buffer.drain(0..message_end);

                            debug!("📨 Extracted complete message ({} bytes)", message_data.len());
                            return Ok(Some(message));
                        }
                    }
                }
            }
        }

        // No complete message available
        Ok(None)
    }

    
    /// Handle a parsed JSON message
    async fn handle_json_message(&mut self, json_value: Value) -> Result<()> {
        debug!("📨 Handling message: {}", serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "Invalid JSON".to_string()));

        if let Some(id) = json_value.get("id") {
            if json_value.get("method").is_some() {
                // Server request
                info!("📨 Server request: ID={:?}, method={:?}",
                      id,
                      json_value.get("method").and_then(|m| m.as_str()).unwrap_or("unknown"));
                // TODO: Handle server requests properly
            } else {
                // Response to our request
                info!("📨 Received response for request ID: {:?}", id);

                let response_result = if let Some(error) = json_value.get("error") {
                    let error_msg = error.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");
                    let error_data = error.get("data");
                    error!("🔴 LSP Server Error (ID: {:?}): {} - {:?}", id, error_msg, error_data);
                    Err(ClientError::Server(format!("LSP error: {} - {:?}", error_msg, error_data)))
                } else if let Some(result) = json_value.get("result") {
                    info!("✅ LSP Success Response (ID: {:?})", id);
                    debug!("✅ Result: {}", serde_json::to_string_pretty(result).unwrap_or_else(|_| "Cannot serialize".to_string()));
                    Ok(result.clone())
                } else {
                    warn!("⚠️ Response has no result or error (ID: {:?})", id);
                    Err(ClientError::InvalidResponse)
                };

                // Find and remove the pending request
                let sender = {
                    let mut pending = self.pending_requests.lock().await;
                    pending.remove(id)
                };

                if let Some(sender) = sender {
                    if sender.send(response_result).is_err() {
                        warn!("📨 Failed to send response to waiting task - request may have been cancelled");
                    }
                } else {
                    warn!("📨 Received response for unknown request ID: {:?}", id);
                }
            }
        } else if let Some(method) = json_value.get("method").and_then(|m| m.as_str()) {
            // Notification
            debug!("📨 Received notification: {}", method);
            match method {
                "textDocument/publishDiagnostics" => {
                    if let Some(ref handler) = self.diagnostic_handler {
                        if let Some(params) = json_value.get("params") {
                            match serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
                                Ok(diagnostic_params) => {
                                    info!("🔍 Received {} diagnostics for {:?}",
                                        diagnostic_params.diagnostics.len(),
                                        diagnostic_params.uri);
                                    handler(diagnostic_params);
                                }
                                Err(e) => {
                                    error!("🔴 Failed to parse diagnostics: {}", e);
                                    debug!("🔴 Diagnostic params: {}", serde_json::to_string_pretty(params).unwrap_or_else(|_| "Invalid".to_string()));
                                }
                            }
                        }
                    } else {
                        debug!("📨 No diagnostic handler set, ignoring diagnostics");
                    }
                }
                "window/logMessage" => {
                    if let Some(params) = json_value.get("params") {
                        info!("📝 LSP Log Message: {}", serde_json::to_string_pretty(params).unwrap_or_else(|_| "Invalid".to_string()));
                    }
                }
                "window/showMessage" => {
                    if let Some(params) = json_value.get("params") {
                        info!("💬 LSP Show Message: {}", serde_json::to_string_pretty(params).unwrap_or_else(|_| "Invalid".to_string()));
                    }
                }
                _ => {
                    debug!("📄 Unhandled notification: {}", method);
                    if let Some(params) = json_value.get("params") {
                        trace!("📄 Notification params: {}", serde_json::to_string_pretty(params).unwrap_or_else(|_| "Invalid".to_string()));
                    }
                }
            }
        } else {
            warn!("📨 Malformed message with no ID or method: {}", serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| "Invalid".to_string()));
        }

        Ok(())
    }

    
    /// Send a request to the LSP server with proper async response handling
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let id_value = Value::Number(serde_json::Number::from(id));

        // Create a channel for the response
        let (response_tx, response_rx) = oneshot::channel();

        // Register the pending request
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(id_value.clone(), response_tx);
        }

        // Create and send the request
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
        info!("🔵 Raw Request JSON: {}", request_json);

        // Check if process is still running before attempting to write
        if !self.is_process_running().await {
            error!("🔴 Failed to send request: LSP process is not running");
            // Clean up the pending request
            let mut pending = self.pending_requests.lock().await;
            pending.remove(&id_value);
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "LSP process is not running",
            )));
        }

        // Send request
        if let Some(stdin) = self.process.stdin() {
            let header = format!("Content-Length: {}\r\n\r\n", request_json.len());
            debug!("🔵 Sending header: {}", header.trim());

            // Additional check: ensure stdin is still writable
            match stdin.write_all(header.as_bytes()).await {
                Ok(()) => {
                    debug!("🔵 Header written successfully");
                }
                Err(e) => {
                    error!("🔴 Failed to write header to LSP process: {}", e);
                    let mut pending = self.pending_requests.lock().await;
                    pending.remove(&id_value);
                    return Err(ClientError::Io(e));
                }
            }

            match stdin.write_all(request_json.as_bytes()).await {
                Ok(()) => {
                    debug!("🔵 Request body written successfully");
                }
                Err(e) => {
                    error!("🔴 Failed to write request body to LSP process: {}", e);
                    let mut pending = self.pending_requests.lock().await;
                    pending.remove(&id_value);
                    return Err(ClientError::Io(e));
                }
            }

            match stdin.flush().await {
                Ok(()) => {
                    debug!("🔵 Request sent successfully");
                }
                Err(e) => {
                    error!("🔴 Failed to flush request to LSP process: {}", e);
                    let mut pending = self.pending_requests.lock().await;
                    pending.remove(&id_value);
                    return Err(ClientError::Io(e));
                }
            }
        } else {
            error!("🔴 Failed to send request: stdin not available");

            // Clean up the pending request
            let mut pending = self.pending_requests.lock().await;
            pending.remove(&id_value);

            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process stdin not available",
            )));
        }

        // Wait for the response with timeout while processing incoming messages
        let timeout_duration = std::time::Duration::from_secs(30);
        let start_time = std::time::Instant::now();

        // Pin the response receiver outside the loop to avoid moving it
        use futures::FutureExt;
        let mut response_rx = Box::pin(response_rx);

        loop {
            // Process any pending messages
            if let Err(e) = self.process_pending_messages().await {
                error!("🔴 Error processing pending messages: {}", e);
            }

            // Try to receive the response with a short timeout using GPUI executor timer
            let timer = self.executor.timer(std::time::Duration::from_millis(10)).fuse();

            match futures::future::select(response_rx.as_mut(), timer).await {
                futures::future::Either::Left((response_result, _)) => {
                    match response_result {
                        Ok(response) => {
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
                            return response;
                        }
                        Err(_) => {
                            error!("🔴 Response channel was closed for request {} (ID: {})", method, id);
                            return Err(ClientError::Timeout);
                        }
                    }
                }
                futures::future::Either::Right((_, _)) => {
                    // Check if we've exceeded the overall timeout
                    if start_time.elapsed() > timeout_duration {
                        error!("🔴 Timeout waiting for response to {} (ID: {})", method, id);

                        // Clean up the pending request
                        let mut pending = self.pending_requests.lock().await;
                        pending.remove(&id_value);

                        return Err(ClientError::Timeout);
                    }

                    // Continue the loop to process more messages
                    continue;
                }
            }
        }
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

        // Check if process is still running before attempting to write
        if !self.is_process_running().await {
            error!("🔴 Failed to send notification: LSP process is not running");
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "LSP process is not running",
            )));
        }

        // Send notification
        if let Some(stdin) = self.process.stdin() {
            let header = format!("Content-Length: {}\r\n\r\n", notification_json.len());
            debug!("📤 Sending notification header: {}", header.trim());

            match stdin.write_all(header.as_bytes()).await {
                Ok(()) => {
                    debug!("📤 Notification header written successfully");
                }
                Err(e) => {
                    error!("🔴 Failed to write notification header to LSP process: {}", e);
                    return Err(ClientError::Io(e));
                }
            }

            match stdin.write_all(notification_json.as_bytes()).await {
                Ok(()) => {
                    debug!("📤 Notification body written successfully");
                }
                Err(e) => {
                    error!("🔴 Failed to write notification body to LSP process: {}", e);
                    return Err(ClientError::Io(e));
                }
            }

            match stdin.flush().await {
                Ok(()) => {
                    debug!("📤 Notification sent successfully");
                }
                Err(e) => {
                    error!("🔴 Failed to flush notification to LSP process: {}", e);
                    return Err(ClientError::Io(e));
                }
            }
        } else {
            error!("🔴 Failed to send notification: stdin not available");
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process stdin not available",
            )));
        }

        Ok(())
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
