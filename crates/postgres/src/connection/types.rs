use anyhow::Result;

/// Typed parameter for PostgreSQL queries
#[derive(Debug, Clone)]
pub enum QueryParam {
    String(String),
    StringArray(Vec<String>),
    I64(i64),
    I32(i32),
    F64(f64),
    Bool(bool),
}

impl From<String> for QueryParam {
    fn from(s: String) -> Self {
        QueryParam::String(s)
    }
}

impl From<&str> for QueryParam {
    fn from(s: &str) -> Self {
        QueryParam::String(s.to_string())
    }
}

impl From<Vec<String>> for QueryParam {
    fn from(v: Vec<String>) -> Self {
        QueryParam::StringArray(v)
    }
}

impl From<i64> for QueryParam {
    fn from(n: i64) -> Self {
        QueryParam::I64(n)
    }
}

impl From<i32> for QueryParam {
    fn from(n: i32) -> Self {
        QueryParam::I32(n)
    }
}

impl From<f64> for QueryParam {
    fn from(n: f64) -> Self {
        QueryParam::F64(n)
    }
}

impl From<bool> for QueryParam {
    fn from(b: bool) -> Self {
        QueryParam::Bool(b)
    }
}

/// SSH configuration for PostgreSQL connections
#[derive(Debug, Clone)]
pub struct PostgresSshConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub ssh_password: Option<String>,
    pub ssh_private_key_path: Option<String>,
    pub ssh_private_key_password: Option<String>,
}

/// Server-level connection key for PostgreSQL connections (no database)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PgServerKey {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    // SSL/TLS configuration
    pub ssl_mode: Option<String>,
    pub ssl_key_path: Option<String>,
    pub ssl_cert_path: Option<String>,
    pub ssl_ca_cert_path: Option<String>,
}

/// Connection key for PostgreSQL connections
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PgConnectionKey {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
}

impl PgServerKey {
    pub fn new(host: String, port: u16, username: String, password: Option<String>) -> Self {
        Self {
            host,
            port,
            username,
            password,
            ssl_mode: None,
            ssl_key_path: None,
            ssl_cert_path: None,
            ssl_ca_cert_path: None,
        }
    }

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
}

impl PgConnectionKey {
    pub fn new(
        host: String,
        port: u16,
        database: String,
        username: String,
        password: Option<String>,
    ) -> Self {
        Self {
            host,
            port,
            database,
            username,
            password,
        }
    }

    /// Convert to server key (removing database)
    pub fn to_server_key(&self) -> PgServerKey {
        PgServerKey::new(
            self.host.clone(),
            self.port,
            self.username.clone(),
            self.password.clone(),
        )
    }

    /// Extract connection key from a PostgreSQL connection string
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let url = if connection_string.starts_with("postgresql://")
            || connection_string.starts_with("postgres://")
        {
            connection_string
        } else {
            return Err(anyhow::anyhow!(
                "Invalid PostgreSQL connection string format"
            ));
        };

        let parsed = url::Url::parse(url)?;

        let host = parsed.host_str().unwrap_or("localhost").to_string();
        let port = parsed.port().unwrap_or(5432);
        let database = parsed.path().trim_start_matches('/').to_string();
        let username = parsed.username().to_string();
        let password = parsed.password().map(|p| p.to_string());

        if database.is_empty() {
            return Err(anyhow::anyhow!(
                "Database name is required in connection string"
            ));
        }

        if username.is_empty() {
            return Err(anyhow::anyhow!("Username is required in connection string"));
        }

        Ok(PgConnectionKey {
            host,
            port,
            database,
            username,
            password,
        })
    }

    /// Generate a connection string from this key
    pub fn to_connection_string(&self) -> String {
        if let Some(ref password) = self.password {
            format!(
                "postgresql://{}:{}@{}:{}/{}?application_name=Blanco",
                self.username, password, self.host, self.port, self.database
            )
        } else {
            format!(
                "postgresql://{}@{}:{}/{}?application_name=Blanco",
                self.username, self.host, self.port, self.database
            )
        }
    }
}
