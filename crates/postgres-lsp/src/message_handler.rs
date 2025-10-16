//! LSP message handler for processing stdout/stderr from PostgreSQL language server
//!
//! This module provides a continuous message processing system inspired by Zed's LspStdoutHandler.
//! It reads LSP messages from the process stdout and routes them through channels for proper
//! asynchronous processing without polling.

use anyhow::{Context as _, Result};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::io::{BufReader, AsyncReadExt};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

/// Content-Length header for LSP messages
const CONTENT_LENGTH_HEADER: &str = "Content-Length: ";
const HEADER_DELIMITER: &[u8; 4] = b"\r\n\r\n";

/// LSP message types that can be received from the server
#[derive(Debug, Clone)]
pub enum LspMessage {
    /// Request message (has ID and expects response)
    Request {
        id: Value,
        method: String,
        params: Option<Value>,
    },
    /// Response message (corresponds to a sent request)
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<Value>,
    },
    /// Notification message (no ID, no response expected)
    Notification {
        method: String,
        params: Option<Value>,
    },
}

/// Message handler for LSP server stdout
///
/// This continuously reads messages from the LSP process stdout and routes them
/// through channels for proper asynchronous processing.
pub struct LspMessageHandler {
    /// Channel for receiving incoming messages
    pub(crate) incoming_messages: mpsc::UnboundedReceiver<LspMessage>,
}

impl LspMessageHandler {
    /// Create a new message handler for the given stdout
    pub fn new<T>(stdout: T) -> Result<Self>
    where
        T: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let (message_tx, message_rx) = mpsc::unbounded_channel();

        tokio::spawn(Self::read_messages(stdout, message_tx));

        Ok(Self {
            incoming_messages: message_rx,
        })
    }

    /// Get the next incoming message
    pub async fn next_message(&mut self) -> Option<LspMessage> {
        self.incoming_messages.recv().await
    }

    /// Read messages from stdout in a background task
    async fn read_messages<T>(
        mut stdout: T,
        message_tx: mpsc::UnboundedSender<LspMessage>,
    ) -> Result<()>
    where
        T: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let mut stdout_reader = BufReader::new(&mut stdout);
        let mut buffer = Vec::new();

        info!("🔧 LspMessageHandler: Starting to read messages from stdout");

        loop {
            buffer.clear();

            // Read headers
            Self::read_headers(&mut stdout_reader, &mut buffer).await?;

            // Parse Content-Length
            let headers_str = std::str::from_utf8(&buffer)
                .context("Invalid UTF-8 in headers")?;

            let content_length = Self::parse_content_length(headers_str)
                .context("Failed to parse Content-Length header")?;

            trace!("📨 LspMessageHandler: Content-Length: {}", content_length);

            // Read the message content
            buffer.resize(content_length, 0);
            stdout_reader
                .read_exact(&mut buffer)
                .await
                .context("Failed to read message content")?;

            let message_str = std::str::from_utf8(&buffer)
                .context("Invalid UTF-8 in message content")?;

            trace!("📨 LspMessageHandler: Raw message: {}", message_str);

            // Parse and route the message
            if let Err(e) = Self::parse_and_route_message(message_str, &message_tx).await {
                error!("🔴 LspMessageHandler: Failed to process message: {}", e);
            }

            // Prevent CPU starvation
            tokio::task::yield_now().await;
        }
    }

    /// Read HTTP-style headers from the stream
    async fn read_headers<T>(
        reader: &mut BufReader<T>,
        buffer: &mut Vec<u8>,
    ) -> Result<()>
    where
        T: tokio::io::AsyncRead + Unpin,
    {
        loop {
            if buffer.len() >= HEADER_DELIMITER.len()
                && buffer[(buffer.len() - HEADER_DELIMITER.len())..] == HEADER_DELIMITER[..]
            {
                return Ok(());
            }

            let mut temp_buf = [0u8; 1];
            match reader.read(&mut temp_buf).await {
                Ok(0) => {
                    return Err(anyhow::anyhow!("EOF while reading headers"));
                }
                Ok(n) => {
                    buffer.extend_from_slice(&temp_buf[..n]);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Error reading headers: {}", e));
                }
            }
        }
    }

    /// Parse Content-Length from headers
    fn parse_content_length(headers: &str) -> Result<usize> {
        for line in headers.lines() {
            if let Some(length_str) = line.strip_prefix(CONTENT_LENGTH_HEADER) {
                return length_str.trim().parse::<usize>()
                    .context("Invalid Content-Length value");
            }
        }
        Err(anyhow::anyhow!("Content-Length header not found"))
    }

    /// Parse a JSON-RPC message and route it to the appropriate channel
    async fn parse_and_route_message(
        message_str: &str,
        message_tx: &mpsc::UnboundedSender<LspMessage>,
    ) -> Result<()> {
        let json_value: Value = serde_json::from_str(message_str)
            .context("Invalid JSON in message")?;

        // Check if it's a notification (no 'id' field) or request/response (has 'id')
        if json_value.get("id").is_some() {
            // Could be either a request or a response
            if let Some(method) = json_value.get("method").and_then(|m| m.as_str()) {
                // It's a request from the server
                let message = LspMessage::Request {
                    id: json_value.get("id").cloned()
                        .context("Request missing id")?,
                    method: method.to_string(),
                    params: json_value.get("params").cloned(),
                };

                debug!("📨 LspMessageHandler: Server request: {}", method);
                message_tx.send(message)
                    .context("Failed to send request message")?;
            } else {
                // It's a response from the server
                let message = LspMessage::Response {
                    id: json_value.get("id").cloned()
                        .context("Response missing id")?,
                    result: json_value.get("result").cloned(),
                    error: json_value.get("error").cloned(),
                };

                debug!("📨 LspMessageHandler: Server response for ID: {:?}", json_value.get("id"));
                message_tx.send(message)
                    .context("Failed to send response message")?;
            }
        } else {
            // It's a notification
            let method = json_value.get("method")
                .and_then(|m| m.as_str())
                .context("Notification missing method")?;

            let message = LspMessage::Notification {
                method: method.to_string(),
                params: json_value.get("params").cloned(),
            };

            debug!("📨 LspMessageHandler: Server notification: {}", method);
            message_tx.send(message)
                .context("Failed to send notification message")?;
        }

        Ok(())
    }
}

/// Diagnostic notification handler
pub type DiagnosticHandler = Arc<dyn Fn(lsp_types::PublishDiagnosticsParams) + Send + Sync>;

/// Health monitor for LSP connection
pub struct LspHealthMonitor {
    /// Last activity timestamp
    last_activity: Arc<Mutex<std::time::Instant>>,
    /// Activity receiver
    activity_rx: mpsc::UnboundedReceiver<()>,
    /// Health check interval
    check_interval: std::time::Duration,
    /// Timeout before considering connection unhealthy
    timeout: std::time::Duration,
}

impl LspHealthMonitor {
    /// Create a new health monitor
    pub fn new(
        activity_rx: mpsc::UnboundedReceiver<()>,
        check_interval: std::time::Duration,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            last_activity: Arc::new(Mutex::new(std::time::Instant::now())),
            activity_rx,
            check_interval,
            timeout,
        }
    }

    /// Start the health monitoring task
    pub async fn start_monitoring(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("🏥 LspHealthMonitor: Starting health monitoring");

        loop {
            tokio::select! {
                // Handle activity notifications
                _ = self.activity_rx.recv() => {
                    let mut last_activity = self.last_activity.lock().unwrap();
                    *last_activity = std::time::Instant::now();
                    debug!("🏥 LspHealthMonitor: Connection activity detected");
                }
                // Periodic health check
                _ = tokio::time::sleep(self.check_interval) => {
                    let last_activity = *self.last_activity.lock().unwrap();
                    let elapsed = last_activity.elapsed();

                    if elapsed > self.timeout {
                        warn!("🏥 LspHealthMonitor: Connection appears unhealthy (no activity for {:?})", elapsed);
                    } else {
                        debug!("🏥 LspHealthMonitor: Connection healthy (last activity {:?} ago)", elapsed);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{self, AsyncWriteExt};

    #[tokio::test]
    async fn test_parse_content_length() {
        let headers = "Content-Type: application/vscode-jsonrpc\r\nContent-Length: 1234\r\n\r\n";
        let length = LspMessageHandler::parse_content_length(headers).unwrap();
        assert_eq!(length, 1234);

        let headers = "Content-Length: 42\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n";
        let length = LspMessageHandler::parse_content_length(headers).unwrap();
        assert_eq!(length, 42);
    }

    #[tokio::test]
    async fn test_message_parsing() {
        // Test notification
        let notification_json = r#"
        {
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": "file:///test.sql",
                "diagnostics": []
            }
        }
        "#;

        let (tx, mut rx) = mpsc::unbounded_channel();
        LspMessageHandler::parse_and_route_message(
            notification_json,
            &tx,
        ).await.unwrap();

        let message = rx.recv().await.unwrap();
        match message {
            LspMessage::Notification { method, params } => {
                assert_eq!(method, "textDocument/publishDiagnostics");
                assert!(params.is_some());
            }
            _ => panic!("Expected notification"),
        }

        // Test response
        let response_json = r#"
        {
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "isIncomplete": false,
                "items": []
            }
        }
        "#;

        LspMessageHandler::parse_and_route_message(
            response_json,
            &tx,
        ).await.unwrap();

        let message = rx.recv().await.unwrap();
        match message {
            LspMessage::Response { id, result, error } => {
                assert_eq!(id, serde_json::json!(1));
                assert!(result.is_some());
                assert!(error.is_none());
            }
            _ => panic!("Expected response"),
        }
    }
}