use database::{ConnectionConfig, DatabaseType};
use gpui_component::ActiveTheme;

/// Environment type for database connections
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnvironmentType {
    #[default]
    Dev = 1,
    Test = 2,
    Prod = 3,
}

impl EnvironmentType {
    pub fn from_i32(value: i32) -> Self {
        match value {
            1 => EnvironmentType::Dev,
            2 => EnvironmentType::Test,
            3 => EnvironmentType::Prod,
            _ => EnvironmentType::Dev, // Default to Dev for invalid values
        }
    }

    pub fn to_i32(self) -> i32 {
        self as i32
    }

    pub fn display_name(self) -> &'static str {
        match self {
            EnvironmentType::Dev => "DEV",
            EnvironmentType::Test => "TEST",
            EnvironmentType::Prod => "PROD",
        }
    }

    pub fn get_color(self, cx: &gpui::App) -> gpui::Rgba {
        match self {
            EnvironmentType::Dev => cx.theme().blue.into(),
            EnvironmentType::Test => cx.theme().green.into(),
            EnvironmentType::Prod => cx.theme().red.into(),
        }
    }
}

impl std::fmt::Display for EnvironmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Data for a query tab stored in the database
#[derive(Debug, Clone)]
pub struct QueryTabData {
    pub id: Option<i64>,
    pub title: String,
    pub content: String,
    pub position: i32,
    pub connection_id: Option<i64>,
    pub connection_type: Option<String>,
    pub connection_name: Option<String>,
    pub database_name: Option<String>,
    pub schema_name: Option<String>,
    pub environment_type: Option<EnvironmentType>,
}

/// Data for a code snippet
#[derive(Debug, Clone)]
pub struct SnippetData {
    pub id: Option<i64>,
    pub name: String,
    pub content: String,
    pub parent_id: Option<i64>,
    pub is_group: bool,
    pub position: i32,
}

/// Data for a database connection
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionData {
    pub id: Option<i64>,
    pub name: String,
    pub db_type: DatabaseType,
    pub host: Option<String>,
    pub port: Option<i32>,
    pub database_name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database_path: Option<String>,
    pub last_used_at: Option<i64>,
    pub is_active: Option<bool>,
    pub environment_type: EnvironmentType,
    // SSH tunnel configuration
    pub ssh_host: Option<String>,
    pub ssh_port: Option<i32>,
    pub ssh_user: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
    // SSL/TLS configuration
    pub ssl_mode: Option<String>,
    pub ssl_key_path: Option<String>,
    pub ssl_cert_path: Option<String>,
    pub ssl_ca_cert_path: Option<String>,
}

impl ConnectionData {
    pub fn new_sqlite(name: String, database_path: String) -> Self {
        Self {
            id: None,
            name,
            db_type: DatabaseType::SQLite,
            host: None,
            port: None,
            database_name: None,
            username: None,
            password: None,
            database_path: Some(database_path),
            is_active: Some(true),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
        }
    }

    pub fn new_postgres(
        name: String,
        host: String,
        port: i32,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        Self {
            id: None,
            name,
            db_type: DatabaseType::PostgreSQL,
            host: Some(host),
            port: Some(port),
            database_name: Some(database),
            username: Some(username),
            password: Some(password),
            database_path: None,
            is_active: Some(true),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_postgres_with_ssh(
        name: String,
        host: String,
        port: i32,
        database: String,
        username: String,
        password: String,
        ssh_host: String,
        ssh_port: i32,
        ssh_user: String,
        ssh_password: Option<String>,
        ssh_private_key_path: Option<String>,
        ssh_private_key_password: Option<String>,
    ) -> Self {
        Self {
            id: None,
            name,
            db_type: DatabaseType::PostgreSQL,
            host: Some(host),
            port: Some(port),
            database_name: Some(database),
            username: Some(username),
            password: Some(password),
            database_path: None,
            is_active: Some(true),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: Some(ssh_host),
            ssh_port: Some(ssh_port),
            ssh_user: Some(ssh_user),
            ssh_password,
            ssh_private_key_path,
            ssh_private_key_password,
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
        }
    }

    pub fn new_mysql(
        name: String,
        host: String,
        port: i32,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        Self {
            id: None,
            name,
            db_type: DatabaseType::MySQL,
            host: Some(host),
            port: Some(port),
            database_name: Some(database),
            username: Some(username),
            password: Some(password),
            database_path: None,
            is_active: Some(true),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
        }
    }

    /// Check if this connection uses SSH tunnel
    pub fn uses_ssh_tunnel(&self) -> bool {
        self.ssh_host.as_ref().is_some_and(|h| !h.trim().is_empty())
    }

    /// Convert to a ConnectionConfig for use with the database service.
    /// Returns None if the connection has no ID.
    pub fn to_connection_config(&self) -> Option<ConnectionConfig> {
        let connection_id = self.id?;

        let config = match self.db_type {
            DatabaseType::SQLite => ConnectionConfig::new_sqlite(
                connection_id,
                self.name.clone(),
                self.database_path
                    .clone()
                    .unwrap_or_else(|| format!("{}.db", self.name)),
            ),
            _ => {
                let default_port = match self.db_type {
                    DatabaseType::PostgreSQL => 5432,
                    DatabaseType::MySQL => 3306,
                    DatabaseType::SQLite => 0,
                };

                let mut config = ConnectionConfig::new(
                    connection_id,
                    self.name.clone(),
                    self.db_type,
                    self.host.clone().unwrap_or_else(|| "localhost".to_string()),
                    self.port.unwrap_or(default_port) as u16,
                    self.database_name.clone().unwrap_or_default(),
                    self.username.clone().unwrap_or_default(),
                    self.password.clone(),
                );

                if let (Some(ssh_host), Some(ssh_user)) = (&self.ssh_host, &self.ssh_user) {
                    config = config.with_ssh_config(
                        ssh_host.clone(),
                        ssh_user.clone(),
                        self.ssh_password.clone(),
                        self.ssh_private_key_path.clone(),
                        self.ssh_private_key_password.clone(),
                        self.ssh_port,
                    );
                }

                config = config.with_ssl_config(
                    self.ssl_mode.clone(),
                    self.ssl_key_path.clone(),
                    self.ssl_cert_path.clone(),
                    self.ssl_ca_cert_path.clone(),
                );

                config
            }
        };

        Some(config)
    }
}
