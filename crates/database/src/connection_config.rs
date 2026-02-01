//! Connection configuration and database type definitions

use serde::{Deserialize, Serialize};
use std::fmt;

/// Database type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseType {
    SQLite,
    PostgreSQL,
    MySQL,
}

impl DatabaseType {
    /// Get the string representation of the database type
    pub fn as_str(&self) -> &'static str {
        match self {
            DatabaseType::SQLite => "SQLite",
            DatabaseType::PostgreSQL => "PostgreSQL",
            DatabaseType::MySQL => "MySQL",
        }
    }

    /// Parse a string into a DatabaseType
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "SQLite" => Some(DatabaseType::SQLite),
            "PostgreSQL" => Some(DatabaseType::PostgreSQL),
            "MySQL" => Some(DatabaseType::MySQL),
            _ => None,
        }
    }

    /// Check if this database type supports switching databases via connection string
    pub fn supports_database_switching(&self) -> bool {
        match self {
            DatabaseType::SQLite => false,
            DatabaseType::PostgreSQL => true,
            DatabaseType::MySQL => true,
        }
    }

    /// Get the Sqruff dialect string for this database type
    pub fn to_sqruff_dialect(&self) -> &'static str {
        match self {
            DatabaseType::SQLite => "sqlite",
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::MySQL => "mysql",
        }
    }

    /// Parse from the string representation stored in app_database
    pub fn from_db_type_str(s: &str) -> Option<Self> {
        match s {
            "SQLite" => Some(DatabaseType::SQLite),
            "PostgreSQL" => Some(DatabaseType::PostgreSQL),
            "MySQL" => Some(DatabaseType::MySQL),
            _ => None,
        }
    }
}

impl fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<DatabaseType> for blanco_core::DriverType {
    fn from(db_type: DatabaseType) -> Self {
        match db_type {
            DatabaseType::SQLite => blanco_core::DriverType::SQLite,
            DatabaseType::PostgreSQL => blanco_core::DriverType::PostgreSQL,
            DatabaseType::MySQL => blanco_core::DriverType::MySQL,
        }
    }
}

/// Connection configuration loaded from app database
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub id: i64,
    pub name: String,
    pub db_type: DatabaseType,
    // Database connection parameters
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
    // SQLite specific
    pub path: Option<String>,
    // SSH tunnel configuration
    pub ssh_host: Option<String>,
    pub ssh_port: Option<i32>,
    pub ssh_user: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
    pub is_active: bool,
}

impl ConnectionConfig {
    /// Create a new connection config from individual parameters
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: i64,
        name: String,
        db_type: DatabaseType,
        host: String,
        port: u16,
        database: String,
        username: String,
        password: Option<String>,
    ) -> Self {
        Self {
            id,
            name,
            db_type,
            host,
            port,
            database,
            username,
            password,
            path: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            is_active: true,
        }
    }

    /// Create a new SQLite connection config
    pub fn new_sqlite(id: i64, name: String, path: String) -> Self {
        Self {
            id,
            name,
            db_type: DatabaseType::SQLite,
            host: String::new(),
            port: 0,
            database: String::new(),
            username: String::new(),
            password: None,
            path: Some(path),
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            is_active: true,
        }
    }

    /// Set SSH configuration
    pub fn with_ssh_config(
        mut self,
        ssh_host: String,
        ssh_user: String,
        ssh_password: Option<String>,
        ssh_private_key_path: Option<String>,
        ssh_private_key_password: Option<String>,
        ssh_port: Option<i32>,
    ) -> Self {
        self.ssh_host = Some(ssh_host);
        self.ssh_user = Some(ssh_user);
        self.ssh_password = ssh_password;
        self.ssh_private_key_path = ssh_private_key_path;
        self.ssh_private_key_password = ssh_private_key_password;
        self.ssh_port = ssh_port;
        self
    }

    /// Get a connection string with optional overrides for database, host, and port
    pub fn connection_string(
        &self,
        database: Option<&str>,
        host: Option<&str>,
        port: Option<u16>,
    ) -> String {
        let db_name = database.unwrap_or(&self.database);
        let conn_host = host.unwrap_or(&self.host);
        let conn_port = port.unwrap_or(self.port);

        match self.db_type {
            DatabaseType::SQLite => {
                // For SQLite, use the path if available, otherwise construct from database name
                // SQLite ignores host/port overrides
                if let Some(path) = &self.path {
                    format!("sqlite:{}", path)
                } else {
                    format!("sqlite:{}.db", db_name)
                }
            }
            DatabaseType::PostgreSQL => {
                let conn_str = if let Some(password) = &self.password {
                    if password.is_empty() {
                        format!(
                            "postgresql://{}@{}:{}/{}",
                            self.username, conn_host, conn_port, db_name
                        )
                    } else {
                        format!(
                            "postgresql://{}:{}@{}:{}/{}",
                            self.username, password, conn_host, conn_port, db_name
                        )
                    }
                } else {
                    format!(
                        "postgresql://{}@{}:{}/{}",
                        self.username, conn_host, conn_port, db_name
                    )
                };
                conn_str
            }
            DatabaseType::MySQL => {
                let conn_str = if let Some(password) = &self.password {
                    if password.is_empty() {
                        format!(
                            "mysql://{}@{}:{}/{}",
                            self.username, conn_host, conn_port, db_name
                        )
                    } else {
                        format!(
                            "mysql://{}:{}@{}:{}/{}",
                            self.username, password, conn_host, conn_port, db_name
                        )
                    }
                } else {
                    format!(
                        "mysql://{}@{}:{}/{}",
                        self.username, conn_host, conn_port, db_name
                    )
                };
                conn_str
            }
        }
    }

    /// Check if this connection requires SSH tunneling
    pub fn requires_ssh_tunnel(&self) -> bool {
        self.ssh_host.is_some() && self.ssh_user.is_some()
    }

    /// Get SSH port (default to 22 if not specified)
    pub fn ssh_port(&self) -> u16 {
        self.ssh_port.unwrap_or(22) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_connection_string_generation() {
        let config = ConnectionConfig::new(
            1,
            "Test DB".to_string(),
            DatabaseType::PostgreSQL,
            "localhost".to_string(),
            5432,
            "default_db".to_string(),
            "user".to_string(),
            Some("pass".to_string()),
        );

        let conn_str = config.connection_string(None, None, None);
        assert_eq!(conn_str, "postgresql://user:pass@localhost:5432/default_db");

        let override_db = config.connection_string(Some("new_db"), None, None);
        assert_eq!(override_db, "postgresql://user:pass@localhost:5432/new_db");
    }

    #[test]
    fn test_mysql_connection_string_generation() {
        let config = ConnectionConfig::new(
            1,
            "Test DB".to_string(),
            DatabaseType::MySQL,
            "localhost".to_string(),
            3306,
            "default_db".to_string(),
            "user".to_string(),
            Some("pass".to_string()),
        );

        let conn_str = config.connection_string(None, None, None);
        assert_eq!(conn_str, "mysql://user:pass@localhost:3306/default_db");

        let override_db = config.connection_string(Some("new_db"), None, None);
        assert_eq!(override_db, "mysql://user:pass@localhost:3306/new_db");
    }

    #[test]
    fn test_sqlite_connection_string_generation() {
        let config = ConnectionConfig::new_sqlite(
            1,
            "Test DB".to_string(),
            "/path/to/db.sqlite".to_string(),
        );

        let conn_str = config.connection_string(None, None, None);
        assert_eq!(conn_str, "sqlite:/path/to/db.sqlite");

        // SQLite should ignore database override and use path
        let override_db = config.connection_string(Some("ignored"), None, None);
        assert_eq!(override_db, "sqlite:/path/to/db.sqlite");
    }

    #[test]
    fn test_ssh_config() {
        let config = ConnectionConfig::new(
            1,
            "Test DB".to_string(),
            DatabaseType::PostgreSQL,
            "localhost".to_string(),
            5432,
            "db".to_string(),
            "user".to_string(),
            None,
        )
        .with_ssh_config(
            "ssh.example.com".to_string(),
            "sshuser".to_string(),
            Some("sshpass".to_string()),
            None,
            None,
            Some(2222),
        );

        assert_eq!(config.ssh_host, Some("ssh.example.com".to_string()));
        assert_eq!(config.ssh_user, Some("sshuser".to_string()));
        assert_eq!(config.ssh_password, Some("sshpass".to_string()));
        assert_eq!(config.ssh_port(), 2222);
        assert!(config.requires_ssh_tunnel());
    }
}
