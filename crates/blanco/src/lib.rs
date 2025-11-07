pub mod app_database;
pub mod ssh_tunnel;
pub mod ssh_tunnel_manager;

// Re-export SSH tunnel types
pub use ssh_tunnel::{
    SshTunnelConfig, SshTunnelManager, TunnelStatus, TunnelInfo,
};

pub use ssh_tunnel_manager::{
    SshTunnelOperations, ssh_tunnel_operations, get_connection_string_with_tunnel,
};