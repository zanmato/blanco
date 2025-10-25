use anyhow::Result;
use async_trait::async_trait;
use lsp_types::{Diagnostic, Uri};

/// Trait for Language Server Protocol managers
/// Different database types can implement LSP support differently
#[async_trait]
pub trait LspManager: Send + Sync {
    /// Initialize LSP for a connection
    async fn initialize(&mut self, connection: &dyn crate::connection_trait::Connection) -> Result<()>;

    /// Send document change notification to LSP
    async fn did_change(&mut self, uri: Uri, version: i32, content: String) -> Result<()>;

    /// Get diagnostics from LSP
    async fn get_diagnostics(&self) -> Result<Vec<Diagnostic>>;

    /// Check if LSP is currently active and healthy
    fn is_active(&self) -> bool;

    /// Shutdown LSP gracefully
    async fn shutdown(&mut self) -> Result<()>;
}

/// Factory for creating appropriate LSP managers for different connection types
pub struct LspManagerFactory;

impl LspManagerFactory {
    /// Create an LSP manager for the given connection, or return a no-op manager
    pub async fn create_for_connection(
        connection: &dyn crate::connection_trait::Connection,
    ) -> Result<Box<dyn LspManager>> {
        if connection.supports_lsp() {
            // Create PostgreSQL-specific LSP manager
            if connection.get_connection_type() == "PostgreSQL" {
                let config = connection.get_lsp_config()
                    .ok_or_else(|| anyhow::anyhow!("LSP config required but not provided"))?;

                let manager = crate::lsp_postgres::PostgresLspManagerWrapper::new(config).await?;
                Ok(Box::new(manager))
            } else {
                // Return no-op manager for databases that claim LSP support but don't have implementation
                Ok(Box::new(NoOpLspManager))
            }
        } else {
            // Return no-op manager for databases that don't support LSP
            Ok(Box::new(NoOpLspManager))
        }
    }
}

/// No-op LSP manager implementation for databases that don't support LSP
pub struct NoOpLspManager;

#[async_trait]
impl LspManager for NoOpLspManager {
    async fn initialize(&mut self, _connection: &dyn crate::connection_trait::Connection) -> Result<()> {
        log::debug!("No-op LSP manager initialized (connection does not support LSP)");
        Ok(())
    }

    async fn did_change(&mut self, _uri: Uri, _version: i32, _content: String) -> Result<()> {
        // Do nothing - no LSP support
        Ok(())
    }

    async fn get_diagnostics(&self) -> Result<Vec<Diagnostic>> {
        Ok(Vec::new()) // No diagnostics without LSP
    }

    fn is_active(&self) -> bool {
        false // No LSP server running
    }

    async fn shutdown(&mut self) -> Result<()> {
        // Nothing to shut down
        Ok(())
    }
}