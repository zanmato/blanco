//! PostgreSQL connection factory

use anyhow::Result;
use async_trait::async_trait;
use blanco_core::{Connection, ConnectionFactory, DatabaseType};
use postgres::PostgresConnection;

/// Opens PostgreSQL connections, or connections to a server speaking its
/// protocol (CockroachDB) through the same driver.
pub struct PostgresConnectionFactory {
    database_type: DatabaseType,
}

impl PostgresConnectionFactory {
    pub fn new() -> Self {
        Self {
            database_type: DatabaseType::PostgreSQL,
        }
    }

    pub fn cockroachdb() -> Self {
        Self {
            database_type: DatabaseType::CockroachDb,
        }
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
        let mut conn = PostgresConnection::from_connection_string(connection_string)?
            .with_database_type(self.database_type);
        Connection::connect(&mut conn, connection_string).await?;
        Ok(Box::new(conn))
    }
}
