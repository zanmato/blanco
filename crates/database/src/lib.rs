//! Database Crate for Blanco
//!
//! This crate provides a clean, unified database service with opaque SSH tunnel handling.
//! It manages database connections, SSH tunnels, and connection pooling internally.

mod connection_config;
mod factories;
mod secret_prompt;
mod service;
pub mod ssh_config;
mod ssh_tunnel;
mod tokio_connection;

pub use connection_config::{
    ConnectionConfig, ConnectionSecretStore, DatabaseType, NoSecretStore, ParamStyles, SecretKind,
};
pub use factories::{MysqlConnectionFactory, PostgresConnectionFactory, SqliteConnectionFactory};
pub use secret_prompt::{NoSecretPrompt, SecretPrompt, SecretPromptCancelled, SecretPromptRequest};
pub use service::{
    ClientToolEndpoint, ConnectionId, DatabaseConfigId, DatabaseConnectedMessage,
    DatabaseDisconnectedMessage, DatabaseService, DatabaseServiceMessage,
};
pub use ssh_tunnel::{SshTunnel, SshTunnelConfig, TunnelInfo};

// Re-export blanco_core traits for convenience
pub use blanco_core::{Connection, ConnectionFactory, DatabaseService as DatabaseServiceTrait};
