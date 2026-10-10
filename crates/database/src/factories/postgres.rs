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
    async fn create_connection(
        &self,
        connection_string: &str,
        session_setup: &[String],
    ) -> Result<Box<dyn Connection>> {
        let mut conn = PostgresConnection::from_connection_string(connection_string)?;
        conn.set_session_setup(session_setup.to_vec())?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
