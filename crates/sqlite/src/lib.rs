//! SQLite Database Implementation for Blanco SQL Editor
//!
//! This crate provides SQLite-specific functionality including:
//! - Database connection management
//! - SQL parsing with SQLite dialect
//! - Auto-completion for SQLite queries
//! - Hover information for tables and columns

pub mod connection;
pub mod sql_parser;
pub mod completion;
pub mod hover;
pub mod factory;

// Re-export main types for convenience
pub use connection::{
    SqliteConnection, SqliteConnectionKey, RowIdentifier
};

pub use sql_parser::{
    SqliteTableExtractor, CompletionKind, ParsedQuery, TableAlias
};

pub use factory::SqliteConnectionFactory;

// Import completion and hover implementations to make them available
use completion::*;
use hover::*;