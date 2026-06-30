use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use redis::RedisConnection;

pub struct RedisConnectionFactory;

impl RedisConnectionFactory {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RedisConnectionFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ConnectionFactory for RedisConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let mut conn = RedisConnection::from_connection_string(connection_string)?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
