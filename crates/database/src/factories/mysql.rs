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
    async fn create_connection(
        &self,
        connection_string: &str,
        session_setup: &[String],
    ) -> Result<Box<dyn Connection>> {
        let mut conn = MysqlConnection::from_connection_string(connection_string)?;
        conn.set_session_setup(session_setup.to_vec())?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
