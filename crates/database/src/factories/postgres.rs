//! PostgreSQL connection factory

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use postgres::{PostgresConnection, PgConnectionKey};

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
        let key = PgConnectionKey::from_connection_string(connection_string)?;
        let mut conn = PostgresConnection::from_key(key);
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String> {
        Ok(connection_string.to_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }
}