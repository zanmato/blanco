pub mod agent;
pub mod app_database;
pub mod app_events;
pub mod ssh_tunnel;
pub mod ssh_tunnel_manager;

// Re-export SSH tunnel types
pub use ssh_tunnel::{SshTunnelConfig, SshTunnelManager, TunnelInfo, TunnelStatus};

pub use ssh_tunnel_manager::{
    SshTunnelOperations, get_connection_string_with_tunnel, ssh_tunnel_operations,
};
