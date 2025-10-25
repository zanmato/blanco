use anyhow::Result;
use async_trait::async_trait;
use lsp_types::{Diagnostic, Uri};
use log::{debug, info};

use crate::connection_trait::{Connection, LspConfig};
use crate::lsp_manager::LspManager;

/// PostgreSQL-specific LSP manager that wraps the existing PostgresLspManager
pub struct PostgresLspManagerWrapper {
    inner: Option<postgres_lsp::PostgresLspManager>,
    config: LspConfig,
    is_initialized: bool,
}

impl PostgresLspManagerWrapper {
    /// Create a new PostgreSQL LSP manager wrapper
    pub async fn new(config: LspConfig) -> Result<Self> {
        debug!("Creating PostgreSQL LSP manager wrapper with config: {:?}", config);

        Ok(Self {
            inner: None,
            config,
            is_initialized: false,
        })
    }

    /// Initialize the inner PostgreSQL LSP manager
    async fn initialize_inner(&mut self) -> Result<()> {
        if self.inner.is_none() {
            info!("Initializing PostgreSQL LSP manager");

            // Create the actual PostgreSQL LSP manager
            // TODO: Update this when we know the correct parameters for PostgresLspManager::new()
            // For now, this is a placeholder to fix compilation
            return Err(anyhow::anyhow!("PostgreSQL LSP manager initialization needs proper parameters"));
        }

        Ok(())
    }
}

#[async_trait]
impl LspManager for PostgresLspManagerWrapper {
    async fn initialize(&mut self, connection: &dyn crate::connection_trait::Connection) -> Result<()> {
        debug!("Initializing PostgreSQL LSP manager wrapper");

        if connection.get_connection_type() != "PostgreSQL" {
            return Err(anyhow::anyhow!("PostgreSQL LSP manager can only be used with PostgreSQL connections"));
        }

        self.initialize_inner().await?;

        if let Some(ref _manager) = self.inner {
            // The existing PostgreSQL LSP manager might need additional setup
            debug!("PostgreSQL LSP manager wrapper initialization complete");
        }

        Ok(())
    }

    async fn did_change(&mut self, uri: Uri, version: i32, content: String) -> Result<()> {
        if let Some(ref mut manager) = self.inner {
            debug!("📤 Sending didChange to PostgreSQL LSP: {:?} (version {})", uri, version);

            manager.did_change(uri, version, content).await
                .map_err(|e| anyhow::anyhow!("PostgreSQL LSP did_change failed: {}", e))?;

            debug!("✅ PostgreSQL LSP didChange completed successfully");
        } else {
            debug!("PostgreSQL LSP manager not initialized, skipping did_change");
        }

        Ok(())
    }

    async fn get_diagnostics(&self) -> Result<Vec<Diagnostic>> {
        if let Some(ref _manager) = self.inner {
            // Get diagnostics from the PostgreSQL LSP manager
            // Note: The existing PostgresLspManager may not have a get_diagnostics method
            // This might need to be added to the original implementation
            debug!("Getting diagnostics from PostgreSQL LSP");
            Ok(Vec::new()) // Placeholder - would need to be implemented
        } else {
            debug!("PostgreSQL LSP manager not initialized, returning empty diagnostics");
            Ok(Vec::new())
        }
    }

    fn is_active(&self) -> bool {
        self.is_initialized && self.inner.is_some()
    }

    async fn shutdown(&mut self) -> Result<()> {
        if let Some(_manager) = self.inner.take() {
            info!("Shutting down PostgreSQL LSP manager");

            // The existing PostgreSQL LSP manager might need a shutdown method
            // This might need to be added to the original implementation
            debug!("PostgreSQL LSP manager shutdown complete");

            self.is_initialized = false;
        }

        Ok(())
    }
}

// Temporary trait implementation for raw PostgresLspManager to fix compilation
// TODO: Replace this with proper wrapper implementation
#[async_trait]
impl LspManager for postgres_lsp::PostgresLspManager {
    async fn initialize(&mut self, _connection: &dyn Connection) -> Result<()> {
        // TODO: Implement proper initialization
        Ok(())
    }

    async fn did_change(&mut self, uri: Uri, version: i32, content: String) -> Result<()> {
        self.did_change(uri, version, content).await
    }

    async fn get_diagnostics(&self) -> Result<Vec<Diagnostic>> {
        self.get_diagnostics().await
    }

    fn is_active(&self) -> bool {
        // TODO: Implement proper check
        true
    }

    async fn shutdown(&mut self) -> Result<()> {
        // TODO: Implement proper shutdown
        Ok(())
    }
}