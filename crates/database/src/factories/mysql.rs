//! MySQL connection factory

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use mysql::MysqlConnection;

/// MySQL connection factory
pub struct MysqlConnectionFactory;

impl MysqlConnectionFactory {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for MysqlConnectionFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionFactory for MysqlConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let mut conn = MysqlConnection::from_connection_string(connection_string)?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}