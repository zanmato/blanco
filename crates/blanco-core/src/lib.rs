//! Blanco Core Library
//!
//! This crate provides shared traits and types for the Blanco SQL editor.
//! It defines the core interfaces for database connections, completion providers,
//! and database service traits.

pub mod connection_trait;
pub mod database_service;

// Re-export main types for convenience
pub use connection_trait::{
    ColumnInfo, ColumnType, Connection, ConnectionFactory, DatabaseSchemaResult, DriverType,
    PaginationInfo, QueryResult, TableMetadata, TableSchemaInfo,
};

pub use database_service::{ConnectionStatus, DatabaseService};

// Re-export lsp-types Position for convenience
pub use lsp_types::Position;
