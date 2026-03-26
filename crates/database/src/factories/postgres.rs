//! PostgreSQL connection factory

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use postgres::PostgresConnection;

/// PostgreSQL connection factory
pub struct PostgresConnectionFactory;

impl PostgresConnectionFactory {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for PostgresConnectionFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionFactory for PostgresConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let mut conn = PostgresConnection::from_connection_string(connection_string)?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
