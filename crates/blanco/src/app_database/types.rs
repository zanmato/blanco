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

/// What an editor buffer holds: statements for the connection's own dialect, or
/// a JavaScript program that drives the connection through the injected `db`
/// object. Persisted in `query_tabs.tab_kind` and `snippets.kind`; anything
/// unrecognised (an older or newer profile) reads back as [`EditorKind::Query`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorKind {
    #[default]
    Query,
    Script,
}

impl EditorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EditorKind::Query => "query",
            EditorKind::Script => "script",
        }
    }

    pub fn from_stored(value: &str) -> Self {
        match value {
            "script" => EditorKind::Script,
            _ => EditorKind::Query,
        }
    }

    /// Highlighter language for a buffer with no connection behind it (a
    /// snippet). Connection-backed tabs use `DatabaseType::editor_language()`
    /// instead, which distinguishes SQL from line-oriented backends.
    pub fn standalone_language(self) -> &'static str {
        match self {
            EditorKind::Query => "sql",
            EditorKind::Script => "javascript",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EditorKind::Query => "Query",
            EditorKind::Script => "Script",
        }
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
    pub tab_kind: EditorKind,
    /// Unix seconds of the last query/script run in this tab.
    pub last_run_at: Option<i64>,
}

/// A single recorded query execution, stored in the `query_history` table.
#[derive(Debug, Clone)]
pub struct QueryHistoryData {
    pub id: Option<i64>,
    pub query_text: String,
    /// Unix timestamp (seconds) of when the query finished executing.
    pub executed_at: i64,
    pub duration_ms: Option<i64>,
    pub rows_affected: Option<i64>,
    pub row_count: Option<i64>,
    pub success: bool,
    pub error_message: Option<String>,
    pub connection_id: Option<i64>,
    pub connection_name: Option<String>,
    pub database_name: Option<String>,
}

/// Data for a code snippet
#[derive(Debug, Clone)]
pub struct SnippetData {
    pub id: Option<i64>,
    pub name: String,
    pub content: String,
    /// Groups are containers and carry no content, so their kind is unused and
    /// stored as the default.
    pub kind: EditorKind,
    pub parent_id: Option<i64>,
    pub is_group: bool,
    pub position: i32,
}

/// Data for a database connection
#[derive(Clone, PartialEq)]
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
    pub trust_server_certificate: bool,
    /// When set, every statement the classifier does not recognise as a read
    /// is rejected before it reaches the server.
    pub read_only: bool,
}

impl std::fmt::Debug for ConnectionData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectionData")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("db_type", &self.db_type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database_name", &self.database_name)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("database_path", &self.database_path)
            .field("last_used_at", &self.last_used_at)
            .field("is_active", &self.is_active)
            .field("environment_type", &self.environment_type)
            .field("ssh_host", &self.ssh_host)
            .field("ssh_port", &self.ssh_port)
            .field("ssh_user", &self.ssh_user)
            .field(
                "ssh_password",
                &self.ssh_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("ssh_private_key_path", &self.ssh_private_key_path)
            .field(
                "ssh_private_key_password",
                &self.ssh_private_key_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_key_path", &self.ssl_key_path)
            .field("ssl_cert_path", &self.ssl_cert_path)
            .field("ssl_ca_cert_path", &self.ssl_ca_cert_path)
            .field("trust_server_certificate", &self.trust_server_certificate)
            .field("read_only", &self.read_only)
            .finish()
    }
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
            trust_server_certificate: false,
            read_only: false,
        }
    }

    /// A host/port/user/password connection of any server backend. The
    /// backend specific constructors below delegate here.
    pub fn new_server(
        db_type: DatabaseType,
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
            db_type,
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
            trust_server_certificate: false,
            read_only: false,
        }
    }

    pub fn with_ssh(
        mut self,
        ssh_host: String,
        ssh_port: i32,
        ssh_user: String,
        ssh_password: Option<String>,
        ssh_private_key_path: Option<String>,
        ssh_private_key_password: Option<String>,
    ) -> Self {
        self.ssh_host = Some(ssh_host);
        self.ssh_port = Some(ssh_port);
        self.ssh_user = Some(ssh_user);
        self.ssh_password = ssh_password;
        self.ssh_private_key_path = ssh_private_key_path;
        self.ssh_private_key_password = ssh_private_key_password;
        self
    }

    pub fn new_mssql(
        name: String,
        host: String,
        port: i32,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        Self::new_server(
            DatabaseType::MsSql,
            name,
            host,
            port,
            database,
            username,
            password,
        )
    }

    pub fn new_redis(
        name: String,
        host: String,
        port: i32,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        Self::new_server(
            DatabaseType::Redis,
            name,
            host,
            port,
            database,
            username,
            password,
        )
    }

    #[cfg(test)]
    pub fn new_postgres(
        name: String,
        host: String,
        port: i32,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        Self::new_server(
            DatabaseType::PostgreSQL,
            name,
            host,
            port,
            database,
            username,
            password,
        )
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
                    DatabaseType::ClickHouse => 8123,
                    DatabaseType::MsSql => 1433,
                    DatabaseType::Redis => 6379,
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
                config = config.with_trust_server_certificate(self.trust_server_certificate);

                config
            }
        };

        Some(config.with_read_only(self.read_only))
    }
}
