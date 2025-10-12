//! Process management for PostgreSQL language server
//! 
//! This module handles the lifecycle of the PostgreSQL language server process,
//! including startup, communication, and shutdown.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child as AsyncChild, Command as AsyncCommand};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

/// Process management errors
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("Failed to start process: {0}")]
    Start(String),
    #[error("Process communication error: {0}")]
    Communication(String),
    #[error("Process exited with code: {0}")]
    Exit(i32),
    #[error("Process timeout")]
    Timeout,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// PostgreSQL language server process
pub struct PostgresLspProcess {
    pub(crate) child: AsyncChild,
    binary_path: PathBuf,
    workspace_path: PathBuf,
    pid: u32,
}

impl PostgresLspProcess {
    /// Create a new PostgreSQL language server process
    /// 
    /// # Arguments
    /// * `binary_path` - Path to the postgrestools binary
    /// * `workspace_path` - Path to the workspace directory
    /// 
    /// # Returns
    /// * `Result<Self, ProcessError>` - Process handle or error
    pub async fn new(binary_path: PathBuf, workspace_path: &Path) -> Result<Self, ProcessError> {
        info!("Starting PostgreSQL LSP process: {:?}", binary_path);
        debug!("Workspace path: {:?}", workspace_path);

        // Validate binary exists
        if !binary_path.exists() {
            return Err(ProcessError::Start(format!(
                "Binary not found: {:?}",
                binary_path
            )));
        }

        // Validate workspace exists
        if !workspace_path.exists() {
            return Err(ProcessError::Start(format!(
                "Workspace not found: {:?}",
                workspace_path
            )));
        }

        // Start the process
        let mut child = AsyncCommand::new(&binary_path)
            .arg("lsp-proxy")
            .arg("--config-path")
            .arg(workspace_path.join("postgrestools.jsonc"))
            .current_dir(workspace_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ProcessError::Start(format!("Failed to spawn process: {}", e)))?;

        let pid = child.id().ok_or_else(|| {
            ProcessError::Start("Failed to get process ID".to_string())
        })?;

        info!("Started PostgreSQL LSP process with PID: {}", pid);

        let process = Self {
            child,
            binary_path,
            workspace_path: workspace_path.to_path_buf(),
            pid,
        };

        Ok(process)
    }

    /// Wait for the process to be ready
    /// 
    /// This method waits for the LSP server to initialize and be ready to accept requests.
    pub async fn wait_for_ready(&mut self) -> Result<(), ProcessError> {
        info!("Waiting for PostgreSQL LSP process to be ready...");

        // Take stderr to monitor startup messages
        let stderr = self.child.stderr.take()
            .ok_or_else(|| ProcessError::Communication("Failed to capture stderr".to_string()))?;

        let mut reader = BufReader::new(stderr).lines();
        let mut ready = false;

        // Wait up to 30 seconds for the process to be ready
        let timeout_duration = Duration::from_secs(30);

        while let Ok(result) = timeout(timeout_duration, reader.next_line()).await {
            if let Ok(Some(line)) = result {
            debug!("LSP stderr: {}", line);
            
            // Look for ready indicators
            if line.contains("LSP server started") || 
               line.contains("Server initialized") ||
               line.contains("Listening") {
                ready = true;
                break;
            }
            
            // Look for error indicators
            if line.contains("error") || line.contains("Error") || line.contains("failed") {
                warn!("LSP process reported error: {}", line);
            }
        }
        }

        if !ready {
            return Err(ProcessError::Timeout);
        }

        info!("PostgreSQL LSP process is ready");
        Ok(())
    }

    /// Get the process ID
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Get the binary path
    pub fn binary_path(&self) -> &PathBuf {
        &self.binary_path
    }

    /// Get the workspace path
    pub fn workspace_path(&self) -> &PathBuf {
        &self.workspace_path
    }

    /// Check if the process is still running
    pub async fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                debug!("LSP process exited with status: {}", status);
                false
            }
            Ok(None) => true,
            Err(e) => {
                error!("Error checking LSP process status: {}", e);
                false
            }
        }
    }

    /// Get the stdin handle for sending LSP messages
    pub fn stdin(&mut self) -> Option<&mut tokio::process::ChildStdin> {
        self.child.stdin.as_mut()
    }

    /// Get the stdout handle for receiving LSP messages
    pub fn stdout(&mut self) -> Option<&mut tokio::process::ChildStdout> {
        self.child.stdout.as_mut()
    }

    /// Shutdown the process gracefully
    pub async fn shutdown(mut self) -> Result<(), ProcessError> {
        info!("Shutting down PostgreSQL LSP process (PID: {})", self.pid);

        // Try to send SIGTERM first for graceful shutdown
        #[cfg(unix)]
        {
            use std::process::Command;
            let result = Command::new("kill")
                .arg("-TERM")
                .arg(self.pid.to_string())
                .output();
                
            match result {
                Ok(output) => {
                    if output.status.success() {
                        debug!("Sent SIGTERM to LSP process");
                        
                        // Wait up to 5 seconds for graceful shutdown
                        match timeout(Duration::from_secs(5), self.child.wait()).await {
                            Ok(Ok(status)) => {
                                info!("LSP process shut down gracefully with status: {}", status);
                                return Ok(());
                            }
                            Ok(Err(e)) => {
                                warn!("Error waiting for LSP process shutdown: {}", e);
                            }
                            Err(_) => {
                                warn!("Timeout waiting for LSP process shutdown");
                            }
                        }
                    } else {
                        warn!("Failed to send SIGTERM to LSP process");
                    }
                }
                Err(e) => {
                    warn!("Failed to execute kill command: {}", e);
                }
            }
        }

        #[cfg(windows)]
        {
            // On Windows, we can't easily send signals, so we'll just kill the process
            warn!("On Windows, using forceful shutdown for LSP process");
        }

        // Force kill if graceful shutdown failed
        let mut process = std::mem::replace(&mut self.child, AsyncCommand::new("echo").spawn().unwrap());
        process.kill().await;
        Ok(())
    }

    /// Force kill the process
    pub async fn kill(&mut self) {
        warn!("Force killing PostgreSQL LSP process (PID: {})", self.pid);
        
        if let Err(e) = self.child.kill().await {
            error!("Failed to kill LSP process: {}", e);
        }
    }

    /// Restart the process
    pub async fn restart(&mut self) -> Result<(), ProcessError> {
        info!("Restarting PostgreSQL LSP process");

        // Kill current process
        self.kill();

        // Start new process
        let mut new_process = Self::new(
            self.binary_path.clone(),
            &self.workspace_path,
        ).await?;

        // Wait for new process to be ready
        new_process.wait_for_ready().await?;

        // Replace current process
        *self = new_process;

        info!("PostgreSQL LSP process restarted successfully");
        Ok(())
    }
}

/// Process manager for handling multiple LSP processes
pub struct ProcessManager {
    processes: Vec<PostgresLspProcess>,
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new() -> Self {
        Self {
            processes: Vec::new(),
        }
    }

    /// Add a new process to manage
    pub fn add_process(&mut self, process: PostgresLspProcess) {
        self.processes.push(process);
    }

    /// Remove a process by PID
    pub fn remove_process(&mut self, pid: u32) -> Option<PostgresLspProcess> {
        let index = self.processes.iter().position(|p| p.pid() == pid)?;
        Some(self.processes.remove(index))
    }

    /// Get a process by PID
    pub fn get_process(&self, pid: u32) -> Option<&PostgresLspProcess> {
        self.processes.iter().find(|p| p.pid() == pid)
    }

    /// Get a mutable process by PID
    pub fn get_process_mut(&mut self, pid: u32) -> Option<&mut PostgresLspProcess> {
        self.processes.iter_mut().find(|p| p.pid() == pid)
    }

    /// Get all processes
    pub fn processes(&self) -> &[PostgresLspProcess] {
        &self.processes
    }

    /// Shutdown all processes
    pub async fn shutdown_all(mut self) -> Result<(), ProcessError> {
        info!("Shutting down all PostgreSQL LSP processes");

        for process in self.processes.drain(..) {
            if let Err(e) = process.shutdown().await {
                error!("Error shutting down LSP process: {}", e);
            }
        }

        Ok(())
    }

    /// Clean up dead processes
    pub async fn cleanup_dead(&mut self) {
        let mut to_remove = Vec::new();

        for (i, process) in self.processes.iter_mut().enumerate() {
            if !process.is_running().await {
                warn!("Removing dead LSP process (PID: {})", process.pid());
                to_remove.push(i);
            }
        }

        // Remove dead processes (in reverse order to maintain indices)
        for &i in to_remove.iter().rev() {
            self.processes.remove(i);
        }
    }

    /// Get the number of managed processes
    pub fn len(&self) -> usize {
        self.processes.len()
    }

    /// Check if there are any managed processes
    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_process_creation() {
        // This test requires a real binary, so we'll just test the structure
        let temp_dir = TempDir::new().unwrap();
        let binary_path = PathBuf::from("/nonexistent/binary");
        
        // This should fail because the binary doesn't exist
        let result = std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                PostgresLspProcess::new(binary_path, temp_dir.path()).await
            })
        }).join().unwrap();

        assert!(result.is_err());
    }

    #[test]
    fn test_process_manager() {
        let manager = ProcessManager::new();
        assert_eq!(manager.len(), 0);
        assert!(manager.is_empty());
    }

    #[tokio::test]
    async fn test_process_manager_cleanup() {
        let mut manager = ProcessManager::new();
        
        // Add a mock process (we can't actually start one without a binary)
        // This test just verifies the structure
        manager.cleanup_dead().await;
        assert_eq!(manager.len(), 0);
    }
}