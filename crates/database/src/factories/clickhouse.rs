use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use clickhouse::ClickhouseConnection;

pub struct ClickhouseConnectionFactory;

impl ClickhouseConnectionFactory {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for ClickhouseConnectionFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionFactory for ClickhouseConnectionFactory {
    async fn create_connection(
        &self,
        connection_string: &str,
        session_setup: &[String],
    ) -> Result<Box<dyn Connection>> {
        let mut conn = ClickhouseConnection::from_connection_string(connection_string)?;
        conn.set_session_setup(session_setup.to_vec())?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
