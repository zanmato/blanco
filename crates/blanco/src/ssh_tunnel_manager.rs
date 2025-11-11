use anyhow::Result;
use gpui::Task;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::app_database::ConnectionData;
use crate::ssh_tunnel::{SshTunnelManager, TunnelInfo, TunnelStatus};

/// SSH tunnel operations manager with GPUI integration
pub struct SshTunnelOperations {
    manager: SshTunnelManager,
    active_tasks: Arc<Mutex<HashMap<String, Task<()>>>>,
}

impl SshTunnelOperations {
    pub fn new() -> Self {
        Self {
            manager: SshTunnelManager::new(),
            active_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create tunnel key for a connection
    fn create_tunnel_key(&self, conn: &ConnectionData) -> String {
        format!(
            "{}@{}:{}->{}:{}",
            conn.ssh_user.as_ref().unwrap_or(&"".to_string()),
            conn.ssh_host.as_ref().unwrap_or(&"".to_string()),
            conn.ssh_port.unwrap_or(22),
            conn.host.as_ref().unwrap_or(&"".to_string()),
            conn.port.unwrap_or(5432)
        )
    }

    /// Create SSH tunnel (simplified for now)
    pub async fn create_tunnel(&self, conn: &ConnectionData) -> Result<u16> {
        if !conn.uses_ssh_tunnel() {
            // Return the direct port for non-SSH connections
            return Ok(conn.port.unwrap_or(5432) as u16);
        }

        // Create the tunnel using the manager
        let local_port = self.manager.create_tunnel(conn).await?;

        // Note: In a full implementation, you would set up monitoring here
        // using GPUI's background_spawn for tunnel health checks

        Ok(local_port)
    }

    /// Close SSH tunnel
    pub fn close_tunnel(&self, conn: &ConnectionData) -> Result<()> {
        if !conn.uses_ssh_tunnel() {
            return Ok(());
        }

        let tunnel_key = self.create_tunnel_key(conn);

        // Cancel the monitor task
        {
            let mut active_tasks = self.active_tasks.lock().unwrap();
            if let Some(task) = active_tasks.remove(&tunnel_key) {
                task.detach(); // Cancel the background task
            }
        }

        // Close the tunnel
        self.manager.close_tunnel(conn)
    }

    /// Get tunnel status
    pub fn get_tunnel_status(&self, conn: &ConnectionData) -> TunnelStatus {
        if !conn.uses_ssh_tunnel() {
            return TunnelStatus::Connected; // Non-SSH connections are considered "connected"
        }

        self.manager.get_tunnel_status(conn)
    }

    /// Test SSH connection configuration
    pub async fn test_ssh_connection(&self, conn: &ConnectionData) -> Result<bool> {
        if !conn.uses_ssh_tunnel() {
            return Ok(true);
        }

        self.manager.test_ssh_connection(conn).await
    }

    /// List all active tunnels
    pub fn list_active_tunnels(&self) -> Vec<(String, TunnelInfo)> {
        self.manager.list_active_tunnels()
    }

    /// Get local tunnel port for a connection
    pub fn get_local_tunnel_port(&self, conn: &ConnectionData) -> Option<u16> {
        if !conn.uses_ssh_tunnel() {
            return Some(conn.port.unwrap_or(5432) as u16);
        }

        // Get the tunnel info and extract the local port
        let tunnel_key = self.create_tunnel_key(conn);
        let tunnels = self.manager.tunnels.lock().unwrap();
        tunnels.get(&tunnel_key).map(|info| info.config.local_port)
    }

    /// Check if connection uses SSH tunnel
    pub fn uses_ssh_tunnel(&self, conn: &ConnectionData) -> bool {
        conn.uses_ssh_tunnel()
    }
}

impl Clone for SshTunnelOperations {
    fn clone(&self) -> Self {
        Self {
            manager: self.manager.clone(),
            active_tasks: Arc::clone(&self.active_tasks),
        }
    }
}

impl Default for SshTunnelOperations {
    fn default() -> Self {
        Self::new()
    }
}

/// Global SSH tunnel operations instance
static SSH_TUNNEL_OPERATIONS: std::sync::OnceLock<SshTunnelOperations> = std::sync::OnceLock::new();

/// Get the global SSH tunnel operations instance
pub fn ssh_tunnel_operations() -> &'static SshTunnelOperations {
    SSH_TUNNEL_OPERATIONS.get_or_init(SshTunnelOperations::new)
}

/// Helper function to get connection string with SSH tunnel port
pub fn get_connection_string_with_tunnel(conn: &ConnectionData) -> Result<String> {
    let operations = ssh_tunnel_operations();

    if conn.uses_ssh_tunnel() {
        if let Some(local_port) = operations.get_local_tunnel_port(conn) {
            // Use the local tunnel port in the connection string
            let host = "127.0.0.1";
            let database = conn
                .database_name
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Database name required"))?;
            let username = conn
                .username
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Username required"))?;

            let connection_string = if let Some(password) = &conn.password {
                if password.is_empty() {
                    format!(
                        "postgresql://{}@{}:{}/{}",
                        username, host, local_port, database
                    )
                } else {
                    format!(
                        "postgresql://{}:{}@{}:{}/{}",
                        username, password, host, local_port, database
                    )
                }
            } else {
                format!(
                    "postgresql://{}@{}:{}/{}",
                    username, host, local_port, database
                )
            };

            return Ok(connection_string);
        }
    }

    // Fall back to the original connection string
    conn.connection_string
        .clone()
        .ok_or_else(|| anyhow::anyhow!("No connection string available"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_tunnel_operations_creation() {
        let operations = SshTunnelOperations::new();
        assert!(!operations
            .list_active_tunnels()
            .iter()
            .any(|(_, info)| matches!(info.status, TunnelStatus::Connected)));
    }

    #[test]
    fn test_tunnel_key_consistency() {
        let conn = ConnectionData::new_postgres_with_ssh(
            "test".to_string(),
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "testuser".to_string(),
            "testpass".to_string(),
            "ssh.example.com".to_string(),
            22,
            "sshuser".to_string(),
            Some("sshpass".to_string()),
            None,
            None,
        );

        let operations = SshTunnelOperations::new();
        let key1 = operations.create_tunnel_key(&conn);
        let key2 = operations.create_tunnel_key(&conn);

        assert_eq!(key1, key2);
        assert_eq!(key1, "sshuser@ssh.example.com:22->localhost:5432");
    }

    #[test]
    fn test_non_ssh_connection_handling() {
        let conn = ConnectionData::new_postgres(
            "test".to_string(),
            "localhost".to_string(),
            5432,
            "testdb".to_string(),
            "testuser".to_string(),
            "testpass".to_string(),
        );

        let operations = SshTunnelOperations::new();

        // Non-SSH connections should have status Connected by default
        let status = operations.get_tunnel_status(&conn);
        assert_eq!(status, TunnelStatus::Connected);

        // Should return the original port
        let port = operations.get_local_tunnel_port(&conn);
        assert_eq!(port, Some(5432));
    }
}
