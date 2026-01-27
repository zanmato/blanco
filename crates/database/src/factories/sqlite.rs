//! SQLite connection factory

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use sqlite::SqliteConnection;

/// SQLite connection factory
pub struct SqliteConnectionFactory;

#[async_trait]
impl ConnectionFactory for SqliteConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let mut conn = SqliteConnection::new(connection_string.to_string())?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}