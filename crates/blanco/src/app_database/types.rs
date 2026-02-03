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

/// Data for a query history entry
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct QueryHistoryData {
    #[allow(dead_code)]
    pub id: Option<i64>,
    pub query_text: String,
    pub executed_at: i64,
    pub duration_ms: Option<i64>,
    pub rows_affected: Option<i64>,
    pub row_count: Option<i64>,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Data for a code snippet
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SnippetData {
    pub id: Option<i64>,
    pub name: String,
    pub content: String,
    pub parent_id: Option<i64>,
    pub is_group: bool,
    pub position: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Data for a database connection
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionData {
    pub id: Option<i64>,
    pub name: String,
    pub db_type: String,
    pub host: Option<String>,
    pub port: Option<i32>,
    pub database_name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database_path: Option<String>,
    #[allow(dead_code)]
    pub last_used_at: Option<i64>,
    // Additional fields for unified connection management
    pub connection_string: Option<String>,
    pub is_active: Option<bool>,
    pub connection_params: Option<serde_json::Value>, // For extensible parameters
    pub environment_type: EnvironmentType,            // Environment type (Dev/Test/Prod)
    // SSH tunnel configuration
    pub ssh_host: Option<String>,
    pub ssh_port: Option<i32>,
    pub ssh_user: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
    pub local_tunnel_port: Option<i32>, // Auto-assigned local port for the tunnel
}

impl ConnectionData {
    pub fn new_sqlite(name: String, database_path: String) -> Self {
        let connection_string = format!("sqlite://{}", database_path);
        Self {
            id: None,
            name,
            db_type: "SQLite".to_string(),
            host: None,
            port: None,
            database_name: None,
            username: None,
            password: None,
            database_path: Some(database_path.clone()),
            connection_string: Some(connection_string),
            is_active: Some(true),
            connection_params: Some(serde_json::json!({
                "database_path": database_path
            })),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            local_tunnel_port: None,
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
        let connection_string = if password.is_empty() {
            format!("postgresql://{}@{}:{}/{}", username, host, port, database)
        } else {
            format!(
                "postgresql://{}:{}@{}:{}/{}",
                username, password, host, port, database
            )
        };

        Self {
            id: None,
            name,
            db_type: "PostgreSQL".to_string(),
            host: Some(host.clone()),
            port: Some(port),
            database_name: Some(database.clone()),
            username: Some(username.clone()),
            password: Some(password),
            database_path: None,
            connection_string: Some(connection_string),
            is_active: Some(true),
            connection_params: Some(serde_json::json!({
                "host": host,
                "port": port,
                "database": database,
                "username": username
            })),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            local_tunnel_port: None,
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
        let connection_string = if password.is_empty() {
            format!("postgresql://{}@{}:{}/{}", username, host, port, database)
        } else {
            format!(
                "postgresql://{}:{}@{}:{}/{}",
                username, password, host, port, database
            )
        };

        Self {
            id: None,
            name,
            db_type: "PostgreSQL".to_string(),
            host: Some(host.clone()),
            port: Some(port),
            database_name: Some(database.clone()),
            username: Some(username.clone()),
            password: Some(password),
            database_path: None,
            connection_string: Some(connection_string),
            is_active: Some(true),
            connection_params: Some(serde_json::json!({
                "host": host,
                "port": port,
                "database": database,
                "username": username,
                "ssh": {
                    "ssh_host": ssh_host,
                    "ssh_port": ssh_port,
                    "ssh_user": ssh_user
                }
            })),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: Some(ssh_host),
            ssh_port: Some(ssh_port),
            ssh_user: Some(ssh_user),
            ssh_password,
            ssh_private_key_path,
            ssh_private_key_password,
            local_tunnel_port: Some(15432), // Default port, will be auto-assigned
        }
    }

    /// Check if this connection uses SSH tunnel
    #[allow(dead_code)]
    pub fn uses_ssh_tunnel(&self) -> bool {
        self.ssh_host.is_some() && !self.ssh_host.as_ref().unwrap().trim().is_empty()
    }

    /// Check if SSH tunnel is properly configured
    #[allow(dead_code)]
    pub fn has_valid_ssh_config(&self) -> bool {
        if let (Some(host), Some(user)) = (&self.ssh_host, &self.ssh_user) {
            !host.trim().is_empty() && !user.trim().is_empty()
        } else {
            false
        }
    }

    /// Get SSH display string for UI
    #[allow(dead_code)]
    pub fn ssh_display_string(&self) -> Option<String> {
        if let (Some(host), Some(port), Some(user)) =
            (&self.ssh_host, &self.ssh_port, &self.ssh_user)
        {
            if self.has_valid_ssh_config() {
                Some(format!("{}@{}:{}", user, host.trim(), port))
            } else {
                None
            }
        } else {
            None
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
        let connection_string = if password.is_empty() {
            format!("mysql://{}@{}:{}/{}", username, host, port, database)
        } else {
            format!(
                "mysql://{}:{}@{}:{}/{}",
                username, password, host, port, database
            )
        };

        Self {
            id: None,
            name,
            db_type: "MySQL".to_string(),
            host: Some(host.clone()),
            port: Some(port),
            database_name: Some(database.clone()),
            username: Some(username.clone()),
            password: Some(password),
            database_path: None,
            connection_string: Some(connection_string),
            is_active: Some(true),
            connection_params: Some(serde_json::json!({
                "host": host,
                "port": port,
                "database": database,
                "username": username
            })),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            ssh_password: None,
            ssh_private_key_path: None,
            ssh_private_key_password: None,
            local_tunnel_port: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_mysql_with_ssh(
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
        let connection_string = if password.is_empty() {
            format!("mysql://{}@{}:{}/{}", username, host, port, database)
        } else {
            format!(
                "mysql://{}:{}@{}:{}/{}",
                username, password, host, port, database
            )
        };

        Self {
            id: None,
            name,
            db_type: "MySQL".to_string(),
            host: Some(host.clone()),
            port: Some(port),
            database_name: Some(database.clone()),
            username: Some(username.clone()),
            password: Some(password),
            database_path: None,
            connection_string: Some(connection_string),
            is_active: Some(true),
            connection_params: Some(serde_json::json!({
                "host": host,
                "port": port,
                "database": database,
                "username": username,
                "ssh_enabled": true
            })),
            environment_type: EnvironmentType::default(),
            last_used_at: None,
            ssh_host: Some(ssh_host),
            ssh_port: Some(ssh_port),
            ssh_user: Some(ssh_user),
            ssh_password,
            ssh_private_key_path,
            ssh_private_key_password,
            local_tunnel_port: Some(13306), // Default port for MySQL, will be auto-assigned
        }
    }
}
