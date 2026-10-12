//! Backup and restore with the native client tools (`pg_dump`, `pg_restore`,
//! `psql`, `mysqldump`, `mysql`), which produce dumps the app's own export
//! cannot match.

pub mod command;
pub mod dialog;
pub mod runner;
pub mod tools;

pub use dialog::{open_backup, open_restore};

/// Tag of the system notification that reports a finished backup or restore.
pub const NOTIFICATION_TAG: &str = "backup-finished";

#[cfg(test)]
mod integration_test;
