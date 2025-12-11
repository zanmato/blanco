//! Database Crate for Blanco
//!
//! This crate provides a clean, unified database service with opaque SSH tunnel handling.
//! It manages database connections, SSH tunnels, and connection pooling internally.

mod connection_config;
mod factories;
mod service;
mod ssh_tunnel;

pub use connection_config::{ConnectionConfig, DatabaseType};
pub use service::{DatabaseService, DatabaseConfigId, ConnectionId};
pub use ssh_tunnel::{SshTunnel, SshTunnelConfig, TunnelInfo};
pub use factories::{SqliteConnectionFactory, PostgresConnectionFactory, MysqlConnectionFactory};

// Re-export blanco_core traits for convenience
pub use blanco_core::{Connection, DatabaseService as DatabaseServiceTrait, ConnectionFactory};