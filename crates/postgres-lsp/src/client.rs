//! LSP client for PostgreSQL language server communication
//! 
//! This module handles JSON-RPC communication with the PostgreSQL language server,
//! including request/response handling and notification processing.

use crate::config::PostgresLspConfig;
use crate::process::PostgresLspProcess;
use anyhow::Result;
use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use url::Url;

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

/// LSP client for PostgreSQL language server
pub struct PostgresLspClient {
    process: Option<PostgresLspProcess>,
    next_id: AtomicU64,
    pending_requests: HashMap<Value, mpsc::Sender<Result<Value, ClientError>>>,
    config: PostgresLspConfig,
}

impl PostgresLspClient {
    /// Create a new LSP client
    /// 
    /// # Arguments
    /// * `process` - The LSP process to communicate with
    /// * `config` - LSP configuration
    /// 
    /// # Returns
    /// * `Result<Self, ClientError>` - LSP client or error
    pub async fn new(process: PostgresLspProcess, config: &PostgresLspConfig) -> Result<Self, ClientError> {
        let mut client = Self {
            process: Some(process),
            next_id: AtomicU64::new(1),
            pending_requests: HashMap::new(),
            config: config.clone(),
        };

        // Initialize the LSP connection
        client.initialize().await?;

        Ok(client)
    }

    /// Initialize the LSP connection
    async fn initialize(&mut self) -> Result<(), ClientError> {
        info!("Initializing PostgreSQL LSP client");

        // Send initialize request with basic capabilities
        let init_params = InitializeParams::default();

        let response = self.request("initialize", Some(serde_json::to_value(init_params)?)).await?;
        
        // Send initialized notification
        self.notification("initialized", Some(serde_json::json!({}))).await?;

        info!("PostgreSQL LSP client initialized successfully");
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
            params,
        };

        let request_json = serde_json::to_string(&request)?;
        
        debug!("Sending LSP request: {} (ID: {})", method, id);

        // Send request
        if let Some(process) = &mut self.process {
            if let Some(stdin) = process.stdin() {
                let header = format!("Content-Length: {}\r\n\r\n", request_json.len());
                stdin.write_all(header.as_bytes()).await?;
                stdin.write_all(request_json.as_bytes()).await?;
                stdin.flush().await?;
            } else {
                return Err(ClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Process stdin not available"
                )));
            }
        } else {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process not available"
            )));
        }

        // Wait for response (simplified for now - in a real implementation, 
        // we'd need to handle responses asynchronously)
        self.wait_for_response(&id_value).await
    }

    /// Send a notification to the LSP server
    async fn notification(&mut self, method: &str, params: Option<Value>) -> Result<(), ClientError> {
        let notification = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };

        let notification_json = serde_json::to_string(&notification)?;
        
        debug!("Sending LSP notification: {}", method);

        // Send notification
        if let Some(process) = &mut self.process {
            if let Some(stdin) = process.stdin() {
                let header = format!("Content-Length: {}\r\n\r\n", notification_json.len());
                stdin.write_all(header.as_bytes()).await?;
                stdin.write_all(notification_json.as_bytes()).await?;
                stdin.flush().await?;
            } else {
                return Err(ClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Process stdin not available"
                )));
            }
        } else {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process not available"
            )));
        }

        Ok(())
    }

    /// Wait for a response with the given ID
    async fn wait_for_response(&mut self, expected_id: &Value) -> Result<Value, ClientError> {
        // This is a simplified implementation
        // In a real implementation, we'd need to read responses asynchronously
        // and match them to pending requests
        
        if let Some(process) = &mut self.process {
            if let Some(stdout) = process.stdout() {
                let mut buffer = vec![0u8; 8192];
                let mut response_data = Vec::new();

                loop {
                    match stdout.read(&mut buffer).await {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            response_data.extend_from_slice(&buffer[..n]);
                            
                            // Try to parse a complete response
                            let response_str = PostgresLspClient::extract_complete_response_static(&response_data);
                            
                            if let Some(response_str) = response_str {
                                let response: JsonRpcResponse = serde_json::from_str(response_str)?;
                                
                                if let Some(response_id) = &response.id {
                                    if response_id == expected_id {
                                        if let Some(error) = response.error {
                                            return Err(ClientError::Server(error.message));
                                        }
                                        if let Some(result) = response.result {
                                            return Ok(result);
                                        }
                                        return Err(ClientError::InvalidResponse);
                                    }
                                }
                            }
                        }
                        Err(e) => return Err(ClientError::Io(e)),
                    }
                }
            } else {
                return Err(ClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Process stdout not available"
                )));
            }
        } else {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Process not available"
            )));
        }

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
    pub async fn completion(&mut self, params: CompletionParams) -> Result<CompletionResponse, ClientError> {
        let response = self.request("textDocument/completion", Some(serde_json::to_value(params)?)).await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Get hover information
    pub async fn hover(&mut self, params: HoverParams) -> Result<Option<Hover>, ClientError> {
        let response = self.request("textDocument/hover", Some(serde_json::to_value(params)?)).await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Get diagnostics (this would typically come from notifications)
    pub async fn request_diagnostics(&mut self, params: DocumentDiagnosticParams) -> Result<DocumentDiagnosticReport, ClientError> {
        let response = self.request("textDocument/diagnostic", Some(serde_json::to_value(params)?)).await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Get code actions
    pub async fn code_actions(&mut self, params: CodeActionParams) -> Result<Option<Vec<CodeAction>>, ClientError> {
        let response = self.request("textDocument/codeAction", Some(serde_json::to_value(params)?)).await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Open a document
    pub async fn did_open(&mut self, params: DidOpenTextDocumentParams) -> Result<(), ClientError> {
        self.notification("textDocument/didOpen", Some(serde_json::to_value(params)?)).await
    }

    /// Change a document
    pub async fn did_change(&mut self, params: DidChangeTextDocumentParams) -> Result<(), ClientError> {
        self.notification("textDocument/didChange", Some(serde_json::to_value(params)?)).await
    }

    /// Close a document
    pub async fn did_close(&mut self, params: DidCloseTextDocumentParams) -> Result<(), ClientError> {
        self.notification("textDocument/didClose", Some(serde_json::to_value(params)?)).await
    }

    /// Save a document
    pub async fn did_save(&mut self, params: DidSaveTextDocumentParams) -> Result<(), ClientError> {
        self.notification("textDocument/didSave", Some(serde_json::to_value(params)?)).await
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
        assert_eq!(response.id, Some(Value::Number(serde_json::Number::from(1))));
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }
}