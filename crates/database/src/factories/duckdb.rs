//! DuckDB connection factory

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory};
use blanco_duckdb::DuckdbConnection;

/// DuckDB connection factory
pub struct DuckdbConnectionFactory;

#[async_trait]
impl ConnectionFactory for DuckdbConnectionFactory {
    async fn create_connection(&self, connection_string: &str) -> Result<Box<dyn Connection>> {
        let mut conn = DuckdbConnection::new(connection_string)?;
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
