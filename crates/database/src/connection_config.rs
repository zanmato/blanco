//! Connection configuration and database type definitions

use serde::{Deserialize, Serialize};
use std::fmt;

/// Database type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseType {
    SQLite,
    PostgreSQL,
    MySQL,
    ClickHouse,
    MsSql,
}

/// Which placeholder syntaxes a SQL dialect recognizes as bind parameters.
///
/// The SQL grammar happily parses tokens like `?` as bind parameters, but those
/// same tokens are operators in some dialects (e.g. Postgres jsonb key-exists
/// `?`, `?|`, `?&`). Filtering parameter detection by dialect avoids prompting
/// the user for a value where no placeholder actually exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamStyles {
    /// `?` positional placeholder (JDBC / MySQL / SQLite).
    pub question_mark: bool,
    /// `$1`, `$2` positional placeholder (Postgres / SQLite).
    pub dollar_number: bool,
    /// `:name` named placeholder.
    pub colon_named: bool,
    /// `@name` named placeholder (T-SQL).
    pub at_named: bool,
}

impl ParamStyles {
    /// Every style enabled. Used by callers that only need statement extraction
    /// and ignore the detected parameters.
    pub const fn all() -> Self {
        Self {
            question_mark: true,
            dollar_number: true,
            colon_named: true,
            at_named: true,
        }
    }

    /// No style enabled.
    pub const fn none() -> Self {
        Self {
            question_mark: false,
            dollar_number: false,
            colon_named: false,
            at_named: false,
        }
    }
}

impl DatabaseType {
    /// Get the string representation of the database type
    pub fn as_str(&self) -> &'static str {
        match self {
            DatabaseType::SQLite => "SQLite",
            DatabaseType::PostgreSQL => "PostgreSQL",
            DatabaseType::MySQL => "MySQL",
            DatabaseType::ClickHouse => "ClickHouse",
            DatabaseType::MsSql => "SQL Server",
        }
    }

    /// Parse a string into a DatabaseType
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "SQLite" => Some(DatabaseType::SQLite),
            "PostgreSQL" => Some(DatabaseType::PostgreSQL),
            "MySQL" => Some(DatabaseType::MySQL),
            "ClickHouse" => Some(DatabaseType::ClickHouse),
            "SQL Server" => Some(DatabaseType::MsSql),
            _ => None,
        }
    }

    /// Check if this database type supports switching databases via connection string
    pub fn supports_database_switching(&self) -> bool {
        match self {
            DatabaseType::SQLite => false,
            DatabaseType::PostgreSQL
            | DatabaseType::MySQL
            | DatabaseType::ClickHouse
            | DatabaseType::MsSql => true,
        }
    }

    /// Get the Sqruff dialect string for this database type
    pub fn to_sqruff_dialect(&self) -> &'static str {
        match self {
            DatabaseType::SQLite => "sqlite",
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::MySQL => "mysql",
            DatabaseType::ClickHouse => "ansi",
            DatabaseType::MsSql => "tsql",
        }
    }

    /// The placeholder syntaxes this dialect recognizes as bind parameters.
    ///
    /// Note: `@name` in T-SQL is also used for local variable declarations
    /// (`DECLARE @x INT`), which share the parameter syntax. MySQL `@name` is a
    /// session variable, not a bind placeholder, so it is intentionally excluded.
    pub fn parameter_styles(&self) -> ParamStyles {
        match self {
            DatabaseType::SQLite => ParamStyles {
                question_mark: true,
                dollar_number: true,
                colon_named: true,
                at_named: true,
            },
            DatabaseType::PostgreSQL => ParamStyles {
                question_mark: false,
                dollar_number: true,
                colon_named: false,
                at_named: false,
            },
            DatabaseType::MySQL => ParamStyles {
                question_mark: true,
                dollar_number: false,
                colon_named: false,
                at_named: false,
            },
            DatabaseType::MsSql => ParamStyles {
                question_mark: false,
                dollar_number: false,
                colon_named: false,
                at_named: true,
            },
            DatabaseType::ClickHouse => ParamStyles::none(),
        }
    }

    /// Parse from the string representation stored in app_database
    pub fn from_db_type_str(s: &str) -> Option<Self> {
        match s {
            "SQLite" => Some(DatabaseType::SQLite),
            "PostgreSQL" => Some(DatabaseType::PostgreSQL),
            "MySQL" => Some(DatabaseType::MySQL),
            "ClickHouse" => Some(DatabaseType::ClickHouse),
            "SQL Server" => Some(DatabaseType::MsSql),
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
            DatabaseType::ClickHouse => blanco_core::DriverType::ClickHouse,
            DatabaseType::MsSql => blanco_core::DriverType::MsSql,
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
    // SSL/TLS configuration
    pub ssl_mode: Option<String>,
    pub ssl_key_path: Option<String>,
    pub ssl_cert_path: Option<String>,
    pub ssl_ca_cert_path: Option<String>,
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
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
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
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
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

    /// Set SSL configuration
    pub fn with_ssl_config(
        mut self,
        ssl_mode: Option<String>,
        ssl_key_path: Option<String>,
        ssl_cert_path: Option<String>,
        ssl_ca_cert_path: Option<String>,
    ) -> Self {
        self.ssl_mode = ssl_mode;
        self.ssl_key_path = ssl_key_path;
        self.ssl_cert_path = ssl_cert_path;
        self.ssl_ca_cert_path = ssl_ca_cert_path;
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
                let mut conn_str = if let Some(password) = &self.password {
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

                let mut params = vec!["application_name=Blanco".to_string()];
                if let Some(ssl_mode) = &self.ssl_mode {
                    let pg_ssl_mode = match ssl_mode.as_str() {
                        "disabled" => "disable",
                        _ => ssl_mode.as_str(),
                    };

                    params.push(format!("sslmode={}", pg_ssl_mode));
                }
                if let Some(ssl_key) = &self.ssl_key_path {
                    params.push(format!("sslkey={}", ssl_key));
                }
                if let Some(ssl_cert) = &self.ssl_cert_path {
                    params.push(format!("sslcert={}", ssl_cert));
                }
                if let Some(ssl_ca) = &self.ssl_ca_cert_path {
                    params.push(format!("sslrootcert={}", ssl_ca));
                }

                conn_str = format!("{}?{}", conn_str, params.join("&"));

                conn_str
            }
            DatabaseType::MySQL => {
                let mut conn_str = if let Some(password) = &self.password {
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

                // Append SSL parameters for MySQL
                let mut ssl_params = Vec::new();
                if let Some(ssl_mode) = &self.ssl_mode {
                    // MySQL uses different ssl-mode parameter names
                    let mysql_mode = match ssl_mode.as_str() {
                        "disabled" => "DISABLED",
                        "preferred" => "PREFERRED",
                        "required" => "REQUIRED",
                        "verify-ca" => "VERIFY_CA",
                        "verify-full" => "VERIFY_IDENTITY",
                        _ => ssl_mode.as_str(),
                    };
                    ssl_params.push(format!("ssl-mode={}", mysql_mode));
                }
                if let Some(ssl_key) = &self.ssl_key_path {
                    ssl_params.push(format!("ssl-key={}", ssl_key));
                }
                if let Some(ssl_cert) = &self.ssl_cert_path {
                    ssl_params.push(format!("ssl-cert={}", ssl_cert));
                }
                if let Some(ssl_ca) = &self.ssl_ca_cert_path {
                    ssl_params.push(format!("ssl-ca={}", ssl_ca));
                }

                if !ssl_params.is_empty() {
                    conn_str = format!("{}?{}", conn_str, ssl_params.join("&"));
                }

                conn_str
            }
            DatabaseType::ClickHouse => {
                // Allow the user to specify the scheme in the host field
                // (e.g. `https://ch.example.com`). If no scheme is present,
                // default to http. If the host already includes a port, don't
                // append the separate port field.
                let (scheme_prefix, host_rest) =
                    if let Some(rest) = conn_host.strip_prefix("https://") {
                        ("https://", rest)
                    } else if let Some(rest) = conn_host.strip_prefix("http://") {
                        ("http://", rest)
                    } else {
                        ("http://", conn_host)
                    };
                let host_clean = host_rest
                    .split(['/', '?'])
                    .next()
                    .unwrap_or(host_rest)
                    .trim_end_matches('/');
                let host_has_port = match host_clean.rfind(':') {
                    Some(idx) => {
                        // Guard against IPv6 literals like [::1]; only treat
                        // as a port separator when it follows a `]` or a
                        // non-bracketed hostname.
                        let after = &host_clean[idx + 1..];
                        !host_clean.starts_with('[')
                            || host_clean[..idx].ends_with(']')
                                && after.chars().all(|c| c.is_ascii_digit())
                    }
                    None => false,
                };
                let host_and_port = if host_has_port {
                    host_clean.to_string()
                } else {
                    format!("{}:{}", host_clean, conn_port)
                };

                let mut params = vec![format!("database={}", db_name)];
                if let Some(ssl_mode) = &self.ssl_mode {
                    params.push(format!("ssl_mode={}", ssl_mode));
                }
                if let Some(ssl_ca) = &self.ssl_ca_cert_path {
                    params.push(format!("sslrootcert={}", ssl_ca));
                }
                if let Some(ssl_cert) = &self.ssl_cert_path {
                    params.push(format!("sslcert={}", ssl_cert));
                }
                if let Some(ssl_key) = &self.ssl_key_path {
                    params.push(format!("sslkey={}", ssl_key));
                }

                // Build the URL via `url::Url` so that userinfo is properly
                // percent-encoded (passwords may contain '@', '/', ':' etc.).
                // The ClickHouse driver reads the username and password from
                // the URL's userinfo.
                let raw = format!("{}{}/?{}", scheme_prefix, host_and_port, params.join("&"));
                match url::Url::parse(&raw) {
                    Ok(mut url) => {
                        if !self.username.is_empty() {
                            let _ = url.set_username(&self.username);
                        }
                        if let Some(password) = &self.password {
                            if !password.is_empty() {
                                let _ = url.set_password(Some(password));
                            }
                        }
                        url.to_string()
                    }
                    Err(_) => raw,
                }
            }
            DatabaseType::MsSql => {
                let mut conn_str = if let Some(password) = &self.password {
                    if password.is_empty() {
                        format!(
                            "mssql://{}@{}:{}/{}",
                            self.username, conn_host, conn_port, db_name
                        )
                    } else {
                        format!(
                            "mssql://{}:{}@{}:{}/{}",
                            self.username, password, conn_host, conn_port, db_name
                        )
                    }
                } else {
                    format!(
                        "mssql://{}@{}:{}/{}",
                        self.username, conn_host, conn_port, db_name
                    )
                };

                let mut params = Vec::new();
                if let Some(ssl_mode) = &self.ssl_mode {
                    params.push(format!("encrypt={}", ssl_mode));
                }
                params.push("trust_cert=true".to_string());
                if !params.is_empty() {
                    conn_str = format!("{}?{}", conn_str, params.join("&"));
                }

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
        assert_eq!(
            conn_str,
            "postgresql://user:pass@localhost:5432/default_db?application_name=Blanco"
        );

        let override_db = config.connection_string(Some("new_db"), None, None);
        assert_eq!(
            override_db,
            "postgresql://user:pass@localhost:5432/new_db?application_name=Blanco"
        );
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
    fn test_postgres_connection_string_with_ssl() {
        let config = ConnectionConfig::new(
            1,
            "Test DB".to_string(),
            DatabaseType::PostgreSQL,
            "localhost".to_string(),
            5432,
            "db".to_string(),
            "user".to_string(),
            Some("pass".to_string()),
        )
        .with_ssl_config(
            Some("require".to_string()),
            None,
            None,
            Some("/path/to/ca.pem".to_string()),
        );

        let conn_str = config.connection_string(None, None, None);
        assert_eq!(
            conn_str,
            "postgresql://user:pass@localhost:5432/db?application_name=Blanco&sslmode=require&sslrootcert=/path/to/ca.pem"
        );
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
