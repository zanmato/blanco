//! Connection configuration

use async_trait::async_trait;
use std::time::Duration;

pub use blanco_core::{DatabaseType, ParamStyles};

/// One of the per-connection secrets kept outside the app database.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecretKind {
    Password,
    SshPassword,
    SshPrivateKeyPassword,
}

impl SecretKind {
    pub const ALL: [SecretKind; 3] = [
        SecretKind::Password,
        SecretKind::SshPassword,
        SecretKind::SshPrivateKeyPassword,
    ];

    /// Stable name used as the last path segment of the stored credential.
    pub fn key(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::SshPassword => "ssh-password",
            Self::SshPrivateKeyPassword => "ssh-private-key-password",
        }
    }

    /// The kind whose [`SecretKind::key`] is `key`.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// What the secret is called in a prompt.
    pub fn label(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::SshPassword => "SSH password",
            Self::SshPrivateKeyPassword => "SSH key passphrase",
        }
    }
}

/// Where connection secrets are read from on demand. `DatabaseService` asks
/// the store only for the secrets a connection is missing at the moment it is
/// opened, so implementations may be slow or serialized without holding up
/// anything else.
#[async_trait]
pub trait ConnectionSecretStore: Send + Sync {
    async fn read(&self, connection_id: i64, kind: SecretKind) -> anyhow::Result<Option<String>>;
}

/// Store that never finds a secret, used until a real one is installed and by
/// tests that pass secrets inline on the config.
pub struct NoSecretStore;

#[async_trait]
impl ConnectionSecretStore for NoSecretStore {
    async fn read(&self, _connection_id: i64, _kind: SecretKind) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

/// Connection configuration loaded from app database
#[derive(Clone)]
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
    /// The database file of a file-based backend (SQLite, DuckDB). DuckDB
    /// also takes `:memory:`.
    pub path: Option<String>,
    // SSH tunnel configuration
    pub ssh_host: Option<String>,
    pub ssh_port: Option<i32>,
    pub ssh_user: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
    /// Jump hosts in `ProxyJump` form, `none`, or `None` to take them from
    /// `~/.ssh/config`.
    pub ssh_jump_hosts: Option<String>,
    pub is_active: bool,
    // SSL/TLS configuration
    pub ssl_mode: Option<String>,
    pub ssl_key_path: Option<String>,
    pub ssl_cert_path: Option<String>,
    pub ssl_ca_cert_path: Option<String>,
    pub trust_server_certificate: bool,
    /// Reject statements not recognised as reads before they reach the server.
    pub read_only: bool,
    /// Minutes without activity after which the connection is closed. `None`
    /// follows the global setting, `Some(0)` never closes it.
    pub idle_timeout_minutes: Option<u32>,
    /// Secrets that are never stored and are asked for when the connection
    /// opens instead.
    pub ask_secrets: Vec<SecretKind>,
    /// The name a Redis connection gives itself with `CLIENT SETNAME`.
    pub client_name: Option<String>,
}

impl std::fmt::Debug for ConnectionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectionConfig")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("db_type", &self.db_type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("path", &self.path)
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
            .field("ssh_jump_hosts", &self.ssh_jump_hosts)
            .field("is_active", &self.is_active)
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_key_path", &self.ssl_key_path)
            .field("ssl_cert_path", &self.ssl_cert_path)
            .field("ssl_ca_cert_path", &self.ssl_ca_cert_path)
            .field("trust_server_certificate", &self.trust_server_certificate)
            .field("read_only", &self.read_only)
            .field("idle_timeout_minutes", &self.idle_timeout_minutes)
            .field("ask_secrets", &self.ask_secrets)
            .field("client_name", &self.client_name)
            .finish()
    }
}

/// libpq's name for an SSL mode as the connection dialog stores it. The
/// dialog shares its names (`preferred`, `required`) with MySQL, while libpq,
/// and sqlx after it, accept only `prefer` and `require`.
pub fn postgres_ssl_mode(mode: &str) -> &str {
    match mode {
        "disabled" => "disable",
        "preferred" => "prefer",
        "required" => "require",
        other => other,
    }
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
            ssh_jump_hosts: None,
            is_active: true,
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
            trust_server_certificate: false,
            read_only: false,
            idle_timeout_minutes: None,
            ask_secrets: Vec::new(),
            client_name: None,
        }
    }

    /// Create a new SQLite connection config
    pub fn new_sqlite(id: i64, name: String, path: String) -> Self {
        Self::new_file(id, name, DatabaseType::SQLite, path)
    }

    /// Create a new DuckDB connection config for a file or `:memory:`.
    pub fn new_duckdb(id: i64, name: String, path: String) -> Self {
        Self::new_file(id, name, DatabaseType::DuckDb, path)
    }

    fn new_file(id: i64, name: String, db_type: DatabaseType, path: String) -> Self {
        Self {
            id,
            name,
            db_type,
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
            ssh_jump_hosts: None,
            is_active: true,
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
            trust_server_certificate: false,
            read_only: false,
            idle_timeout_minutes: None,
            ask_secrets: Vec::new(),
            client_name: None,
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

    pub fn with_ssh_jump_hosts(mut self, jump_hosts: Option<String>) -> Self {
        self.ssh_jump_hosts = jump_hosts.filter(|hosts| !hosts.trim().is_empty());
        self
    }

    /// The tunnel to `remote_host:remote_port` through this connection's
    /// SSH host, listening on `local_port`.
    pub fn ssh_tunnel_config(
        &self,
        remote_host: &str,
        remote_port: u16,
        local_port: u16,
    ) -> anyhow::Result<crate::SshTunnelConfig> {
        Ok(crate::SshTunnelConfig {
            ssh_host: self
                .ssh_host
                .clone()
                .filter(|host| !host.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("SSH host is required"))?,
            ssh_port: self.ssh_port(),
            // An empty user is taken from ~/.ssh/config or the local user.
            ssh_user: self.ssh_user.clone().unwrap_or_default(),
            ssh_password: self.ssh_password.clone(),
            // A pasted path often carries a space, which is never part of it.
            ssh_private_key_path: self
                .ssh_private_key_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string),
            ssh_private_key_password: self.ssh_private_key_password.clone(),
            jump_hosts: self.ssh_jump_hosts.clone(),
            remote_host: remote_host.to_string(),
            remote_port,
            local_port,
        })
    }

    pub fn with_trust_server_certificate(mut self, trust: bool) -> Self {
        self.trust_server_certificate = trust;
        self
    }

    pub fn with_client_name(mut self, client_name: Option<String>) -> Self {
        self.client_name = client_name;
        self
    }

    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn with_idle_timeout_minutes(mut self, minutes: Option<u32>) -> Self {
        self.idle_timeout_minutes = minutes;
        self
    }

    /// How long the connection may sit unused before it is closed, given the
    /// global default. `None` keeps it open.
    pub fn idle_timeout(&self, default: Option<Duration>) -> Option<Duration> {
        match self.idle_timeout_minutes {
            None => default,
            Some(0) => None,
            Some(minutes) => Some(Duration::from_secs(u64::from(minutes) * 60)),
        }
    }

    pub fn with_ask_secrets(mut self, ask_secrets: Vec<SecretKind>) -> Self {
        self.ask_secrets = ask_secrets;
        self
    }

    /// Whether `kind` is asked for on connect rather than stored.
    pub fn asks_for(&self, kind: SecretKind) -> bool {
        self.ask_secrets.contains(&kind)
    }

    /// Whether `kind` plays any part in opening this connection: a database
    /// file has no password, SSH secrets need a tunnel, and a key passphrase
    /// needs a key.
    pub fn uses_secret(&self, kind: SecretKind) -> bool {
        match kind {
            SecretKind::Password => !self.db_type.dialect().is_file_based(),
            SecretKind::SshPassword => self.requires_ssh_tunnel(),
            SecretKind::SshPrivateKeyPassword => {
                self.requires_ssh_tunnel()
                    && self
                        .ssh_private_key_path
                        .as_deref()
                        .is_some_and(|path| !path.is_empty())
            }
        }
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
            // Each in-memory DuckDB database belongs to its own connection,
            // and `database` picks one of its attached databases.
            DatabaseType::DuckDb => {
                let path = self.path.as_deref().unwrap_or(blanco_duckdb::MEMORY_PATH);
                let memory_id = self.id.to_string();
                blanco_duckdb::connection_string(
                    path,
                    self.read_only,
                    (path == blanco_duckdb::MEMORY_PATH).then_some(memory_id.as_str()),
                    database,
                )
            }
            DatabaseType::PostgreSQL | DatabaseType::CockroachDb => {
                // Through `url::Url`, so a user name, password, database or
                // certificate path with `@`, `/`, `#`, `&`, `+` or spaces
                // arrives as typed.
                let raw = format!("postgresql://{conn_host}:{conn_port}");
                let mut url = match url::Url::parse(&raw) {
                    Ok(url) => url,
                    Err(error) => {
                        tracing::warn!("Invalid PostgreSQL host {conn_host:?}: {error}");
                        return raw;
                    }
                };
                if url.set_username(&self.username).is_err() {
                    tracing::warn!("PostgreSQL URL cannot carry a user name");
                }
                if let Some(password) = self.password.as_deref().filter(|p| !p.is_empty()) {
                    if url.set_password(Some(password)).is_err() {
                        tracing::warn!("PostgreSQL URL cannot carry a password");
                    }
                }
                url.set_path(&format!("/{db_name}"));
                {
                    let mut query = url.query_pairs_mut();
                    query.append_pair("application_name", "Blanco");
                    if let Some(ssl_mode) = self.ssl_mode.as_deref().filter(|m| !m.is_empty()) {
                        query.append_pair("sslmode", postgres_ssl_mode(ssl_mode));
                    }
                    for (key, path) in [
                        ("sslkey", &self.ssl_key_path),
                        ("sslcert", &self.ssl_cert_path),
                        ("sslrootcert", &self.ssl_ca_cert_path),
                    ] {
                        if let Some(path) = path.as_deref().filter(|path| !path.is_empty()) {
                            query.append_pair(key, path);
                        }
                    }
                }
                url.to_string()
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
                        if !self.username.is_empty() && url.set_username(&self.username).is_err() {
                            tracing::warn!(
                                "ClickHouse URL cannot carry a username, connecting without one"
                            );
                        }
                        let password = self.password.as_deref().filter(|p| !p.is_empty());
                        if let Some(password) = password {
                            if url.set_password(Some(password)).is_err() {
                                tracing::warn!("ClickHouse URL cannot carry a password, connecting without one");
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
                    // The UI exposes Off/On/Required, but the driver only treats
                    // true/yes/mandatory as "encrypt"; translate so the choice
                    // actually takes effect (anything but "Off" enables it).
                    let encrypt = !ssl_mode.eq_ignore_ascii_case("off");
                    params.push(format!("encrypt={}", encrypt));
                }
                if self.trust_server_certificate {
                    params.push("trust_cert=true".to_string());
                }
                if !params.is_empty() {
                    conn_str = format!("{}?{}", conn_str, params.join("&"));
                }

                conn_str
            }
            DatabaseType::Redis => {
                // redis://[:password@]host:port/db-number?client_name=…. The
                // database name is a numeric DB index (e.g. "0"); default to 0
                // when not numeric. client_name is Blanco's parameter, which
                // the driver takes off and sends with CLIENT SETNAME.
                let db_index = db_name.parse::<u8>().unwrap_or(0);
                let client_name = self
                    .client_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("Blanco");
                let client_name: String =
                    url::form_urlencoded::byte_serialize(client_name.as_bytes()).collect();
                let url = {
                    let scheme = match self.ssl_mode.as_deref() {
                        Some("disabled") | None => "redis",
                        _ => "rediss",
                    };
                    if let Some(password) = &self.password {
                        if password.is_empty() {
                            format!("{}://{}:{}/{}", scheme, conn_host, conn_port, db_index)
                        } else if self.username.is_empty() {
                            format!(
                                "{}://:{}@{}:{}/{}",
                                scheme, password, conn_host, conn_port, db_index
                            )
                        } else {
                            format!(
                                "{}://{}:{}@{}:{}/{}",
                                scheme, self.username, password, conn_host, conn_port, db_index
                            )
                        }
                    } else {
                        format!("{}://{}:{}/{}", scheme, conn_host, conn_port, db_index)
                    }
                };
                format!("{url}?client_name={client_name}")
            }
        }
    }

    /// Check if this connection requires SSH tunneling
    pub fn requires_ssh_tunnel(&self) -> bool {
        self.ssh_host.is_some() && self.ssh_user.is_some()
    }

    /// The config with every secret cleared, the form kept in long-lived
    /// structures so secrets only exist while a connection is being opened.
    pub fn without_secrets(mut self) -> Self {
        self.password = None;
        self.ssh_password = None;
        self.ssh_private_key_password = None;
        self
    }

    pub fn secret(&self, kind: SecretKind) -> Option<&str> {
        match kind {
            SecretKind::Password => self.password.as_deref(),
            SecretKind::SshPassword => self.ssh_password.as_deref(),
            SecretKind::SshPrivateKeyPassword => self.ssh_private_key_password.as_deref(),
        }
    }

    pub fn set_secret(&mut self, kind: SecretKind, value: Option<String>) {
        match kind {
            SecretKind::Password => self.password = value,
            SecretKind::SshPassword => self.ssh_password = value,
            SecretKind::SshPrivateKeyPassword => self.ssh_private_key_password = value,
        }
    }

    /// Secrets this connection could use but does not carry yet. A database
    /// file has no password and SSH secrets only matter with a tunnel, so
    /// those never cause a store lookup.
    pub fn missing_secret_kinds(&self) -> Vec<SecretKind> {
        let mut kinds = Vec::new();
        if !self.db_type.dialect().is_file_based() && self.password.is_none() {
            kinds.push(SecretKind::Password);
        }
        if self.requires_ssh_tunnel() {
            if self.ssh_password.is_none() {
                kinds.push(SecretKind::SshPassword);
            }
            if self.ssh_private_key_password.is_none() {
                kinds.push(SecretKind::SshPrivateKeyPassword);
            }
        }
        kinds
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
    fn redis_urls_carry_the_client_name() {
        let config = ConnectionConfig::new(
            1,
            "cache".into(),
            DatabaseType::Redis,
            "localhost".into(),
            6379,
            "2".into(),
            String::new(),
            None,
        );
        assert_eq!(
            config.connection_string(None, None, None),
            "redis://localhost:6379/2?client_name=Blanco"
        );
        let named = config.with_client_name(Some("report worker".into()));
        assert_eq!(
            named.connection_string(None, None, None),
            "redis://localhost:6379/2?client_name=report+worker"
        );
    }

    #[test]
    fn editor_language_maps_sql_backends_and_redis() {
        for (db_type, language) in [
            (DatabaseType::SQLite, "sql_sqlite"),
            (DatabaseType::PostgreSQL, "sql_postgres"),
            (DatabaseType::MySQL, "sql_mariadb"),
            (DatabaseType::ClickHouse, "sql_clickhouse"),
            (DatabaseType::MsSql, "sql_tsql"),
            (DatabaseType::DuckDb, "sql_duckdb"),
        ] {
            assert_eq!(db_type.editor_language(), language, "{db_type:?}");
        }
        assert_eq!(DatabaseType::Redis.editor_language(), "redis");
    }

    #[test]
    fn ssh_tunnel_config_trims_the_key_path() {
        let config = ConnectionConfig::new(
            1,
            "tunnelled".to_string(),
            DatabaseType::PostgreSQL,
            "db".to_string(),
            5432,
            "app".to_string(),
            "user".to_string(),
            None,
        )
        .with_ssh_config(
            "bastion".to_string(),
            "deploy".to_string(),
            None,
            Some(" /home/deploy/.ssh/id_ed25519 ".to_string()),
            None,
            None,
        );
        let tunnel = config.ssh_tunnel_config("db", 5432, 15432).unwrap();
        assert_eq!(
            tunnel.ssh_private_key_path.as_deref(),
            Some("/home/deploy/.ssh/id_ed25519")
        );
    }

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
    fn mssql_only_trusts_server_certificate_when_explicitly_enabled() {
        let config = ConnectionConfig::new(
            1,
            "SQL Server".to_string(),
            DatabaseType::MsSql,
            "localhost".to_string(),
            1433,
            "master".to_string(),
            "sa".to_string(),
            Some("pass".to_string()),
        );

        assert_eq!(
            config.connection_string(None, None, None),
            "mssql://sa:pass@localhost:1433/master"
        );
        assert_eq!(
            config
                .with_trust_server_certificate(true)
                .connection_string(None, None, None),
            "mssql://sa:pass@localhost:1433/master?trust_cert=true"
        );
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let config = ConnectionConfig::new(
            1,
            "Postgres".to_string(),
            DatabaseType::PostgreSQL,
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "user".to_string(),
            Some("database-secret".to_string()),
        )
        .with_ssh_config(
            "bastion".to_string(),
            "ssh-user".to_string(),
            Some("ssh-secret".to_string()),
            None,
            Some("key-secret".to_string()),
            None,
        );

        let debug = format!("{config:?}");
        assert!(!debug.contains("database-secret"));
        assert!(!debug.contains("ssh-secret"));
        assert!(!debug.contains("key-secret"));
        assert!(debug.contains("[REDACTED]"));
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
            "postgresql://user:pass@localhost:5432/db?application_name=Blanco&sslmode=require&sslrootcert=%2Fpath%2Fto%2Fca.pem"
        );
    }

    /// Every SSL mode the dialog offers for PostgreSQL reaches sqlx in a
    /// spelling it parses, and to the mode it names.
    #[test]
    fn postgres_ssl_modes_from_the_dialog_parse() {
        use sqlx::postgres::{PgConnectOptions, PgSslMode};
        use std::str::FromStr as _;

        for (mode, expected) in [
            ("disabled", PgSslMode::Disable),
            ("allow", PgSslMode::Allow),
            ("preferred", PgSslMode::Prefer),
            ("required", PgSslMode::Require),
            ("verify-ca", PgSslMode::VerifyCa),
            ("verify-full", PgSslMode::VerifyFull),
        ] {
            let config = ConnectionConfig::new(
                1,
                "Test".to_string(),
                DatabaseType::PostgreSQL,
                "localhost".to_string(),
                5432,
                "db".to_string(),
                "user".to_string(),
                Some("pass".to_string()),
            )
            .with_ssl_config(Some(mode.to_string()), None, None, None);
            let options = PgConnectOptions::from_str(&config.connection_string(None, None, None))
                .unwrap_or_else(|error| panic!("{mode}: {error}"));
            // PgSslMode has no PartialEq.
            assert_eq!(
                format!("{:?}", options.get_ssl_mode()),
                format!("{expected:?}"),
                "{mode}"
            );
        }
    }

    /// Against the compose server with TLS (`postgrestestdb_ssl`, port 5488),
    /// each dialog mode connects, and encrypts exactly when it should.
    /// Skipped when the server is down, unless `BLANCO_RUN_DB_TESTS=1`.
    #[tokio::test]
    async fn postgres_ssl_modes_connect_to_a_tls_server() {
        use sqlx::Connection as _;
        use std::str::FromStr as _;

        let ca = format!(
            "{}/../postgres/test_certs/ca.crt",
            env!("CARGO_MANIFEST_DIR")
        );
        for (mode, encrypted) in [
            ("disabled", false),
            ("allow", false),
            ("preferred", true),
            ("required", true),
            ("verify-ca", true),
            ("verify-full", true),
        ] {
            let config = ConnectionConfig::new(
                1,
                "Test".to_string(),
                DatabaseType::PostgreSQL,
                "localhost".to_string(),
                5488,
                "blanco".to_string(),
                "blanco".to_string(),
                Some("blanco".to_string()),
            )
            .with_ssl_config(Some(mode.to_string()), None, None, Some(ca.clone()));
            let options = sqlx::postgres::PgConnectOptions::from_str(
                &config.connection_string(None, None, None),
            )
            .unwrap_or_else(|error| panic!("{mode}: {error}"));
            let mut connection = match sqlx::PgConnection::connect_with(&options).await {
                Ok(connection) => connection,
                Err(error) if mode == "disabled" => {
                    if std::env::var("BLANCO_RUN_DB_TESTS").as_deref() == Ok("1") {
                        panic!("the TLS test server is unreachable: {error}");
                    }
                    tracing::warn!("skip: the TLS test server on 5488 is unreachable ({error})");
                    return;
                }
                Err(error) => panic!("{mode}: {error}"),
            };
            let ssl: bool =
                sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
                    .fetch_one(&mut connection)
                    .await
                    .unwrap_or_else(|error| panic!("{mode}: {error}"));
            assert_eq!(ssl, encrypted, "{mode}");
        }
    }

    /// A password, user name, database and certificate path holding URL
    /// syntax come through to the driver unchanged.
    #[test]
    fn postgres_connection_string_encodes_its_parts() {
        use sqlx::postgres::PgConnectOptions;
        use std::str::FromStr as _;

        let config = ConnectionConfig::new(
            1,
            "Test".to_string(),
            DatabaseType::PostgreSQL,
            "db.example.com".to_string(),
            5432,
            "my db".to_string(),
            "andreas.project".to_string(),
            Some("p@ss/w:rd#1?%&+ ".to_string()),
        )
        .with_ssl_config(
            Some("verify-full".to_string()),
            None,
            None,
            Some("/home/me/certs & keys/ca+root.pem".to_string()),
        );
        let conn_str = config.connection_string(None, None, None);
        let options = PgConnectOptions::from_str(&conn_str).expect("parses");
        assert_eq!(options.get_host(), "db.example.com");
        assert_eq!(options.get_username(), "andreas.project");
        assert_eq!(options.get_database(), Some("my db"));
        let url = url::Url::parse(&conn_str).expect("url");
        let password = url.password().expect("password");
        assert_eq!(
            percent_encoding::percent_decode_str(password).decode_utf8_lossy(),
            "p@ss/w:rd#1?%&+ "
        );
        let root_cert: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| key == "sslrootcert")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(
            root_cert,
            [(
                "sslrootcert".to_string(),
                "/home/me/certs & keys/ca+root.pem".to_string()
            )]
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
