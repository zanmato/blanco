//! PostgreSQL Database Implementation for Blanco SQL Editor
//!
//! This crate provides PostgreSQL-specific functionality including:
//! - Database connection management
//! - SQL parsing with PostgreSQL dialect
//! - Auto-completion for PostgreSQL queries (TODO)
//! - Hover information for tables and columns (TODO)

pub mod connection;
pub mod sql_parser;
pub mod completion;
pub mod hover;
pub mod factory;

// Re-export main types for convenience
pub use connection::{
    PostgresConnection, PgConnectionKey
};

pub use sql_parser::{
    PostgresTableExtractor, CompletionKind, ParsedQuery, TableAlias
};

pub use factory::PostgresConnectionFactory;

// Import completion and hover implementations to make them available
use completion::*;
use hover::*;