pub mod agent;
pub mod app_database;
pub mod app_events;
pub mod sql_completion;
pub mod ssh_tunnel;
pub mod ssh_tunnel_manager;

// Re-export SSH tunnel types
pub use ssh_tunnel::{
    SshTunnelConfig, SshTunnelManager, TunnelStatus, TunnelInfo,
};

pub use ssh_tunnel_manager::{
    SshTunnelOperations, ssh_tunnel_operations, get_connection_string_with_tunnel,
};