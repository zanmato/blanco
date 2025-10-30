use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};

use crate::connection::PostgresConnection;

/// Factory for creating PostgreSQL connections
pub struct PostgresConnectionFactory;

#[async_trait]
impl ConnectionFactory for PostgresConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let mut conn = PostgresConnection::from_connection_string(connection_string)?;
        conn.connect(connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String> {
        let key = crate::connection::PgConnectionKey::from_connection_string(connection_string)?;
        Ok(key.to_connection_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "PostgreSQL"
    }
}