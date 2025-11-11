use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// SSH tunnel configuration
#[derive(Debug, Clone)]
pub struct SshTunnelConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
}

impl SshTunnelConfig {
    pub fn from_connection_data(
        conn: &crate::app_database::ConnectionData,
        assigned_local_port: u16,
    ) -> Result<Self> {
        if conn.ssh_host.is_none() || conn.ssh_user.is_none() {
            anyhow::bail!("SSH configuration is incomplete");
        }

        Ok(Self {
            ssh_host: conn.ssh_host.as_ref().unwrap().clone(),
            ssh_port: conn.ssh_port.unwrap_or(22) as u16,
            ssh_user: conn.ssh_user.as_ref().unwrap().clone(),
            ssh_password: conn.ssh_password.clone(),
            ssh_private_key_path: conn.ssh_private_key_path.clone(),
            ssh_private_key_password: conn.ssh_private_key_password.clone(),
            remote_host: conn.host.as_ref().unwrap().clone(),
            remote_port: conn.port.unwrap_or(5432) as u16,
            local_port: assigned_local_port,
        })
    }
}

/// SSH tunnel status
#[derive(Debug, Clone, PartialEq)]
pub enum TunnelStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// SSH tunnel information
#[derive(Debug, Clone)]
pub struct TunnelInfo {
    pub config: SshTunnelConfig,
    pub status: TunnelStatus,
    pub created_at: Instant,
    pub last_activity: Instant,
}

/// SSH tunnel manager
pub struct SshTunnelManager {
    pub(super) tunnels: Arc<Mutex<HashMap<String, TunnelInfo>>>,
    port_counter: Arc<AtomicU32>,
}

impl SshTunnelManager {
    pub fn new() -> Self {
        Self {
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            port_counter: Arc::new(AtomicU32::new(15432)), // Start from 15432
        }
    }

    /// Assign a unique local port for a tunnel
    pub fn assign_local_port(&self) -> u16 {
        let port = self.port_counter.fetch_add(1, Ordering::SeqCst);
        // Check if we need to wrap around
        if port > 65535 {
            self.port_counter.store(15432, Ordering::SeqCst);
            return 15432;
        }
        port as u16
    }

    /// Create a tunnel key for a connection
    fn create_tunnel_key(&self, conn: &crate::app_database::ConnectionData) -> String {
        format!(
            "{}@{}:{}->{}:{}",
            conn.ssh_user.as_ref().unwrap_or(&"".to_string()),
            conn.ssh_host.as_ref().unwrap_or(&"".to_string()),
            conn.ssh_port.unwrap_or(22),
            conn.host.as_ref().unwrap_or(&"".to_string()),
            conn.port.unwrap_or(5432)
        )
    }

    /// Create a new SSH tunnel (simplified implementation for now)
    pub async fn create_tunnel(&self, conn: &crate::app_database::ConnectionData) -> Result<u16> {
        let tunnel_key = self.create_tunnel_key(conn);
        let local_port = self.assign_local_port();

        let config = SshTunnelConfig::from_connection_data(conn, local_port)?;

        // Store tunnel info
        {
            let mut tunnels = self.tunnels.lock().unwrap();
            tunnels.insert(
                tunnel_key.clone(),
                TunnelInfo {
                    config,
                    status: TunnelStatus::Connected, // Simplified - assume it works for now
                    created_at: Instant::now(),
                    last_activity: Instant::now(),
                },
            );
        }

        // Note: This is a simplified implementation. In a real scenario,
        // you would need to implement the actual SSH tunneling logic here
        // with proper port forwarding using russh

        Ok(local_port)
    }

    /// Get tunnel status
    pub fn get_tunnel_status(&self, conn: &crate::app_database::ConnectionData) -> TunnelStatus {
        let tunnel_key = self.create_tunnel_key(conn);
        let tunnels = self.tunnels.lock().unwrap();
        tunnels
            .get(&tunnel_key)
            .map(|info| info.status.clone())
            .unwrap_or(TunnelStatus::Disconnected)
    }

    /// Close a tunnel
    pub fn close_tunnel(&self, conn: &crate::app_database::ConnectionData) -> Result<()> {
        let tunnel_key = self.create_tunnel_key(conn);
        let mut tunnels = self.tunnels.lock().unwrap();
        tunnels.remove(&tunnel_key);
        Ok(())
    }

    /// List all active tunnels
    pub fn list_active_tunnels(&self) -> Vec<(String, TunnelInfo)> {
        let tunnels = self.tunnels.lock().unwrap();
        tunnels
            .iter()
            .filter(|(_, info)| matches!(info.status, TunnelStatus::Connected))
            .map(|(key, info)| (key.clone(), info.clone()))
            .collect()
    }

    /// Test SSH connection configuration
    pub async fn test_ssh_connection(
        &self,
        conn: &crate::app_database::ConnectionData,
    ) -> Result<bool> {
        if conn.ssh_host.is_none() || conn.ssh_user.is_none() {
            return Ok(false);
        }

        // Simplified test - in a real implementation, you would
        // attempt to establish an actual SSH connection here
        Ok(true)
    }

    /// Create tunnel using GPUI background_spawn (for integration with GPUI)
    pub async fn create_tunnel_gpui(
        &self,
        conn: &crate::app_database::ConnectionData,
    ) -> Result<u16> {
        // Simplified GPUI integration - will be expanded later
        self.create_tunnel(conn).await
    }
}

impl Clone for SshTunnelManager {
    fn clone(&self) -> Self {
        Self {
            tunnels: Arc::clone(&self.tunnels),
            port_counter: Arc::clone(&self.port_counter),
        }
    }
}

impl Default for SshTunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assign_local_port() {
        let manager = SshTunnelManager::new();
        let port1 = manager.assign_local_port();
        let port2 = manager.assign_local_port();

        assert_eq!(port1, 15432);
        assert_eq!(port2, 15433);
    }

    #[test]
    fn test_tunnel_key_creation() {
        let conn = crate::app_database::ConnectionData::new_postgres_with_ssh(
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

        let manager = SshTunnelManager::new();
        let key = manager.create_tunnel_key(&conn);

        assert_eq!(key, "sshuser@ssh.example.com:22->localhost:5432");
    }
}
