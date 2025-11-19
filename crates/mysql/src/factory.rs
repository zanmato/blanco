use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};

use crate::connection::MysqlConnection;

/// Factory for creating MySQL connections
pub struct MysqlConnectionFactory;

#[async_trait]
impl ConnectionFactory for MysqlConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let mut conn = MysqlConnection::from_connection_string(connection_string)?;
        conn.connect(connection_string).await?;
        Ok(Box::new(conn))
    }

    fn parse_connection_string(&self, connection_string: &str) -> Result<String> {
        let key = crate::connection::MysqlConnectionKey::from_connection_string(connection_string)?;
        Ok(key.to_connection_string())
    }

    fn get_connection_type(&self) -> &'static str {
        "MySQL"
    }
}