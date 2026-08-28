//! Blanco Core Library
//!
//! This crate provides shared traits and types for the Blanco SQL editor.
//! It defines the core interfaces for database connections, completion providers,
//! and database service traits.

pub mod connection_trait;
pub mod database_service;
pub mod database_type;
pub mod ddl;
pub mod explain_plan;
pub mod table_extractor;
pub mod write_guard;

// Re-export main types for convenience
pub use connection_trait::{
    BatchFailure, BatchOutcome, ColumnInfo, ColumnType, Connection, ConnectionFactory,
    DatabaseSchemaResult, EntityType, ForeignKeyInfo, FunctionSignatureInfo, InboundForeignKey,
    IndexInfo, KeyValueResult, PaginationInfo, QueryResult, QueryableEntity, RedisType, RedisValue,
    ResultPayload, RoutineKind, TableMetadata, TableSchemaInfo, WriteOperation,
};

pub use database_service::{ConnectionStatus, DatabaseService};
pub use database_type::{DatabaseType, ParamStyles};
pub use table_extractor::TableExtractor;
pub use write_guard::StatementAccess;

// Re-export lsp-types Position for convenience
pub use lsp_types::Position;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Connection-establishment timeout in seconds, shared across the database
/// service and the individual drivers. It bounds SSH tunnel setup and the
/// initial database connect so an unreachable host (for example a conflicting
/// VPN route) fails fast instead of hanging indefinitely. Defaults to the same
/// 30s the settings UI defaults to; `set_connect_timeout_secs` keeps it in sync
/// with the user's configured value.
static CONNECT_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(30);

/// Update the global connection timeout. Values are clamped to a sane range so a
/// stray setting can neither disable the timeout nor make it absurdly long.
pub fn set_connect_timeout_secs(secs: u64) {
    CONNECT_TIMEOUT_SECS.store(secs.clamp(1, 600), Ordering::Relaxed);
}

/// The current connection-establishment timeout as a `Duration`.
pub fn connect_timeout() -> Duration {
    Duration::from_secs(CONNECT_TIMEOUT_SECS.load(Ordering::Relaxed))
}

/// Marker attached to an error chain by a driver when it determines, from its
/// own typed error, that a failure was caused by a lost or closed connection
/// (rather than an ordinary query error). The database service checks for this
/// with [`is_connection_lost`] to decide whether to evict and mark the
/// connection disconnected. Keeping the decision in the driver, where the
/// concrete error type is in scope, means the service never has to pattern-match
/// driver-specific error strings.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLost;

impl std::fmt::Display for ConnectionLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("database connection lost")
    }
}

impl std::error::Error for ConnectionLost {}

/// True if a [`ConnectionLost`] marker was attached anywhere in the error's
/// context chain. `anyhow`'s `downcast_ref` walks the context chain, so this
/// finds the marker even underneath later `.context(...)` wrapping.
pub fn is_connection_lost(error: &anyhow::Error) -> bool {
    error.downcast_ref::<ConnectionLost>().is_some()
}

/// Typed classification of `sqlx::Error` into the shared [`ConnectionLost`]
/// marker. Lives behind the `sqlx` feature so only the sqlx-based driver crates
/// pull it in.
#[cfg(feature = "sqlx")]
mod sqlx_support {
    use super::ConnectionLost;

    /// Classify a `sqlx::Error` as a lost/closed connection. Covers the socket
    /// level IO errors a dropped TCP connection or dead SSH tunnel produces, the
    /// pool giving up, and server-initiated closes that sqlx surfaces as
    /// protocol errors.
    pub fn sqlx_connection_lost(error: &sqlx::Error) -> bool {
        match error {
            sqlx::Error::Io(io) => matches!(
                io.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::NotConnected
            ),
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => true,
            sqlx::Error::Protocol(message) => {
                let message = message.to_lowercase();
                message.contains("closed") || message.contains("terminating")
            }
            _ => false,
        }
    }

    /// Convert a raw `sqlx::Error` into an `anyhow::Error`, attaching a
    /// [`ConnectionLost`] marker when it represents a dropped connection.
    pub fn tag_sqlx(error: sqlx::Error) -> anyhow::Error {
        if sqlx_connection_lost(&error) {
            anyhow::Error::new(error).context(ConnectionLost)
        } else {
            anyhow::Error::new(error)
        }
    }

    /// Attach a [`ConnectionLost`] marker to an existing `anyhow::Error` when its
    /// chain contains a dropped-connection `sqlx::Error`. Used at boundaries that
    /// already hold an `anyhow::Error` whose underlying `sqlx::Error` was
    /// preserved by `?`, rather than the raw driver error.
    pub fn tag_sqlx_error(error: anyhow::Error) -> anyhow::Error {
        let lost = error
            .downcast_ref::<sqlx::Error>()
            .is_some_and(sqlx_connection_lost);
        if lost {
            error.context(ConnectionLost)
        } else {
            error
        }
    }
}

/// A batch of write statements committed as a single transaction.
///
/// The sqlx-based drivers (PostgreSQL, MySQL, SQLite) each hold a
/// `sqlx::Transaction` whose concrete `QueryResult` type exposes
/// `rows_affected()` (sqlx has no generic trait for it), so this macro stamps
/// out the identical begin/execute/commit/rollback control flow against a
/// caller-provided `$transaction` while keeping the per-statement rows-affected
/// accounting. A failure on any statement rolls the whole batch back, so the
/// resulting [`BatchFailure`] always reports `applied == 0`.
#[cfg(feature = "sqlx")]
#[macro_export]
macro_rules! run_sqlx_transaction {
    ($pool:expr, $operations:expr) => {{
        let pool = $pool;
        let operations: &[$crate::WriteOperation] = $operations;
        match pool.begin().await {
            Ok(mut transaction) => {
                let mut outcome = $crate::BatchOutcome::default();
                let mut failed: Option<sqlx::Error> = None;
                for operation in operations {
                    let mut query = sqlx::query(&operation.sql);
                    for parameter in &operation.parameters {
                        query = match parameter {
                            Some(value) => query.bind(value.clone()),
                            None => query.bind(Option::<String>::None),
                        };
                    }
                    match query.execute(&mut *transaction).await {
                        Ok(result) => {
                            outcome.rows_affected += result.rows_affected();
                            outcome.operations_executed += 1;
                        }
                        Err(error) => {
                            failed = Some(error);
                            break;
                        }
                    }
                }
                // `transaction` is consumed exactly once here (rollback on
                // failure, commit otherwise), keeping the borrow checker happy.
                match failed {
                    Some(error) => {
                        if let Err(rollback_error) = transaction.rollback().await {
                            tracing::warn!("transaction rollback failed: {rollback_error}");
                        }
                        Err($crate::BatchFailure::atomic($crate::tag_sqlx(error)))
                    }
                    None => match transaction.commit().await {
                        Ok(()) => Ok(outcome),
                        Err(error) => Err($crate::BatchFailure::atomic($crate::tag_sqlx(error))),
                    },
                }
            }
            Err(error) => Err($crate::BatchFailure::atomic($crate::tag_sqlx(error))),
        }
    }};
}

#[cfg(feature = "sqlx")]
pub use sqlx_support::{sqlx_connection_lost, tag_sqlx, tag_sqlx_error};
