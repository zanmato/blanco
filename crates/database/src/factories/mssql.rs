use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use mssql::MssqlConnection;

pub struct MssqlConnectionFactory;

impl MssqlConnectionFactory {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ConnectionFactory for MssqlConnectionFactory {
    async fn create_connection(
        &self,
        connection_string: &str,
        session_setup: &[String],
    ) -> Result<Box<dyn Connection>> {
        let mut conn = MssqlConnection::from_connection_string(connection_string)?;
        conn.set_session_setup(session_setup.to_vec())?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
