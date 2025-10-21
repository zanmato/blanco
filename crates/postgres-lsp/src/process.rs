//! Process management for PostgreSQL language server
//!
//! This module handles the lifecycle of the PostgreSQL language server process,
//! including startup, communication, and shutdown.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;
use smol::process::{Child, Command, Stdio};
use futures::io::{AsyncBufReadExt, BufReader};
use futures::{StreamExt, FutureExt};
use tracing::{debug, error, info, warn};
use gpui::BackgroundExecutor;

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

/// RAII guard for child processes that automatically kills them on drop
///
/// This ensures that postgrestools processes are properly cleaned up
/// when the application exits, even during panics.
pub struct ProcessGuard {
    child: Child,
    pid: u32,
    binary_name: String,
}

impl ProcessGuard {
    /// Create a new process guard
    pub fn new(child: Child, pid: u32, binary_name: String) -> Self {
        Self {
            child,
            pid,
            binary_name,
        }
    }

    /// Get the process ID
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Get the child process (for normal operations)
    pub fn child(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Check if the process is still running
    pub fn is_running(&mut self) -> bool {
        // smol::process::Child doesn't have try_wait, so we'll use a different approach
        // We can check the process status by trying to kill it with signal 0
        #[cfg(unix)]
        {
            use std::process::Command;
            match Command::new("kill").arg("-0").arg(self.pid.to_string()).output() {
                Ok(output) => output.status.success(),
                Err(_) => false,
            }
        }
        #[cfg(not(unix))]
        {
            // On non-Unix platforms, we'll assume it's running
            // In practice, you might want a different approach
            true
        }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        info!("🛡️ ProcessGuard dropping - automatically cleaning up {} (PID: {})",
              self.binary_name, self.pid);

        // Check if we're in a panic situation
        let is_panicking = std::thread::panicking();
        if is_panicking {
            warn!("🚨 ProcessGuard dropped during panic - force killing {} (PID: {})",
                  self.binary_name, self.pid);
        }

        // Kill the process asynchronously - we need to spawn a task since we're in Drop
        let pid = self.pid;
        let binary_name = self.binary_name.clone();

        // In Drop, we can only use synchronous operations
        // smol doesn't have runtime detection like tokio
        warn!("Using synchronous kill during ProcessGuard::drop for {} (PID: {})",
              binary_name, pid);
        Self::kill_process_sync(pid, &binary_name);
    }
}

impl ProcessGuard {

    fn kill_process_sync(pid: u32, binary_name: &str) {
        info!("🔫 Killing {} process (PID: {}) synchronously", binary_name, pid);

        #[cfg(unix)]
        {
            match std::process::Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .output()
            {
                Ok(output) => {
                    if output.status.success() {
                        info!("✅ Successfully sent SIGTERM to {} (PID: {})", binary_name, pid);
                    } else {
                        error!("❌ Failed to send SIGTERM to {} (PID: {}): {}",
                               binary_name, pid, String::from_utf8_lossy(&output.stderr));
                    }
                }
                Err(e) => error!("❌ Error executing kill command for {} (PID: {}): {}",
                               binary_name, pid, e),
            }
        }

        #[cfg(windows)]
        {
            match std::process::Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output()
            {
                Ok(output) => {
                    if output.status.success() {
                        info!("✅ Successfully killed {} process on Windows (PID: {})", binary_name, pid);
                    } else {
                        error!("❌ Failed to kill {} process on Windows (PID: {}): {}",
                               binary_name, pid, String::from_utf8_lossy(&output.stderr));
                    }
                }
                Err(e) => error!("❌ Error executing taskkill for {} (PID: {}): {}",
                               binary_name, pid, e),
            }
        }
    }
}

/// PostgreSQL language server process
pub struct PostgresLspProcess {
    pub(crate) process_guard: ProcessGuard,
    binary_path: PathBuf,
    workspace_path: PathBuf,
    executor: BackgroundExecutor,
}

impl PostgresLspProcess {
    /// Create a dummy placeholder process (for testing/temporary use)
    pub fn new_dummy(executor: BackgroundExecutor) -> Self {
        // This is a hack - we need to create a valid process structure
        // For now, let's just create a process that will fail when used
        let child = smol::process::Command::new("echo")
            .arg("dummy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to create dummy process");

        let pid = child.id();
        let process_guard = ProcessGuard::new(child, pid, "dummy".to_string());

        Self {
            process_guard,
            binary_path: PathBuf::from("dummy"),
            workspace_path: PathBuf::from("."),
            executor,
        }
    }
    /// Create a new PostgreSQL language server process
    ///
    /// # Arguments
    /// * `binary_path` - Path to the postgrestools binary
    /// * `workspace_path` - Path to the workspace directory
    /// * `executor` - GPUI background executor for timers
    ///
    /// # Returns
    /// * `Result<Self, ProcessError>` - Process handle or error
    pub async fn new(
        binary_path: PathBuf,
        workspace_path: &Path,
        executor: BackgroundExecutor,
    ) -> Result<Self, ProcessError> {
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

        // Start the process without --config-path argument
        // The LSP server will use workspace folder detection instead
        info!("🔧 Spawning LSP process with cwd: {:?}", workspace_path);
        info!("🔧 Binary path: {:?}", binary_path);

        let child = Command::new(&binary_path)
            .arg("lsp-proxy")
            .current_dir(workspace_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ProcessError::Start(format!("Failed to spawn process: {}", e)))?;

        let pid = child.id();

        info!("✅ Started PostgreSQL LSP process with PID: {}", pid);
        info!("🔧 Checking if process is immediately responsive...");

        // Small delay to let the process start
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Check if the process is still running after the short delay
        #[cfg(unix)]
        {
            use std::process::Command;
            match Command::new("kill").arg("-0").arg(pid.to_string()).output() {
                Ok(output) => {
                    if output.status.success() {
                        info!("✅ Process {} is running after startup", pid);
                    } else {
                        warn!("⚠️ Process {} is not running immediately after startup", pid);
                        warn!("⚠️ Exit status: {}", output.status);
                        if !output.stderr.is_empty() {
                            warn!("⚠️ stderr: {}", String::from_utf8_lossy(&output.stderr));
                        }
                    }
                }
                Err(e) => {
                    warn!("⚠️ Failed to check process {} status: {}", pid, e);
                }
            }
        }

        // Wrap the child process in a ProcessGuard for automatic cleanup
        let process_guard = ProcessGuard::new(child, pid, "postgrestools".to_string());

        let process = Self {
            process_guard,
            binary_path,
            workspace_path: workspace_path.to_path_buf(),
            executor,
        };

        Ok(process)
    }

    /// Wait for the process to be ready
    ///
    /// This method waits for the LSP server to initialize and be ready to accept requests.
    pub async fn wait_for_ready(&mut self) -> Result<(), ProcessError> {
        info!("Waiting for PostgreSQL LSP process to be ready...");

        // Take stderr to monitor startup messages
        let stderr =
            self.process_guard.child().stderr.take().ok_or_else(|| {
                ProcessError::Communication("Failed to capture stderr".to_string())
            })?;

        let mut reader = BufReader::new(stderr).lines();
        let mut ready = false;

        // Wait up to 30 seconds for the process to be ready
        let timeout_duration = Duration::from_secs(30);

        // Use GPUI executor timer for timeout (following Zed's pattern)
        loop {
            let next_line = reader.next();
            let timer = self.executor.timer(timeout_duration).fuse();

            match futures::future::select(next_line, timer).await {
                futures::future::Either::Left((line_result, _)) => {
                    if let Some(Ok(line)) = line_result {
                        debug!("LSP stderr: {}", line);

                        // Look for ready indicators
                        if line.contains("LSP server started")
                            || line.contains("Server initialized")
                            || line.contains("Listening")
                        {
                            ready = true;
                            break;
                        }

                        // Look for error indicators
                        if line.contains("error") || line.contains("Error") || line.contains("failed") {
                            warn!("LSP process reported error: {}", line);
                        }
                    } else {
                        // EOF or error
                        break;
                    }
                }
                futures::future::Either::Right((_, _)) => {
                    // Timeout
                    warn!("Timeout waiting for LSP process to be ready");
                    return Err(ProcessError::Timeout);
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
        self.process_guard.pid()
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
    pub fn is_running(&mut self) -> bool {
        self.process_guard.is_running()
    }

    /// Get the stdin handle for sending LSP messages
    pub fn stdin(&mut self) -> Option<&mut smol::process::ChildStdin> {
        self.process_guard.child().stdin.as_mut()
    }

    /// Get the stdout handle for receiving LSP messages
    pub fn stdout(&mut self) -> Option<&mut smol::process::ChildStdout> {
        self.process_guard.child().stdout.as_mut()
    }

    /// Shutdown the process gracefully
    pub async fn shutdown(mut self) -> Result<(), ProcessError> {
        info!("Shutting down PostgreSQL LSP process (PID: {})", self.pid());

        // Try to send SIGTERM first for graceful shutdown
        #[cfg(unix)]
        {
            use std::process::Command;
            let result = Command::new("kill")
                .arg("-TERM")
                .arg(self.pid().to_string())
                .output();

            match result {
                Ok(output) => {
                    if output.status.success() {
                        debug!("Sent SIGTERM to LSP process");

                        // Wait up to 5 seconds for graceful shutdown
                        use futures::FutureExt;
                        let status_future = self.process_guard.child().status();
                        let shutdown_timer = self.executor.timer(Duration::from_secs(5)).fuse();

                        match futures::future::select(
                            Box::pin(status_future),
                            shutdown_timer
                        ).await {
                            futures::future::Either::Left((Ok(status), _)) => {
                                info!("LSP process shut down gracefully with status: {}", status);
                                return Ok(());
                            }
                            futures::future::Either::Left((Err(e), _)) => {
                                warn!("Error waiting for LSP process shutdown: {}", e);
                            }
                            futures::future::Either::Right((_, _)) => {
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
        if let Err(e) = self.process_guard.child().kill() {
            error!("Failed to kill LSP process during shutdown: {}", e);
        }
        Ok(())
    }

    /// Force kill the process
    pub fn kill(&mut self) {
        warn!("Force killing PostgreSQL LSP process (PID: {})", self.pid());

        if let Err(e) = self.process_guard.child().kill() {
            error!("Failed to kill LSP process: {}", e);
        }
    }

    /// Restart the process
    pub async fn restart(&mut self) -> Result<(), ProcessError> {
        info!("Restarting PostgreSQL LSP process");

        // Kill current process
        self.kill();

        // Start new process without config path
        let mut new_process =
            Self::new(self.binary_path.clone(), &self.workspace_path, self.executor.clone()).await?;

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
    pub(crate) processes: Vec<PostgresLspProcess>,
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
            if !process.is_running() {
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

    #[test]
    fn test_process_creation() {
        // This test requires a real binary, so we'll just test the structure
        let binary_path = PathBuf::from("/nonexistent/binary");

        // This should fail because the binary doesn't exist
        let result = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { PostgresLspProcess::new(binary_path, binary_path.parent().unwrap()).await })
        })
        .join()
        .unwrap();

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
