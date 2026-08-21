use blanco_core::DatabaseType;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ConnectorType {
    SQLite,
    PostgreSQL,
    MySQL,
    ClickHouse,
    MsSql,
    Redis,
}

impl ConnectorType {
    pub(super) fn from_str(s: &str) -> Self {
        match s {
            "PostgreSQL" => ConnectorType::PostgreSQL,
            "MySQL" => ConnectorType::MySQL,
            "ClickHouse" => ConnectorType::ClickHouse,
            "SQL Server" => ConnectorType::MsSql,
            "Redis" => ConnectorType::Redis,
            _ => ConnectorType::SQLite,
        }
    }
}

impl From<DatabaseType> for ConnectorType {
    fn from(driver_type: DatabaseType) -> Self {
        match driver_type {
            DatabaseType::SQLite => ConnectorType::SQLite,
            DatabaseType::PostgreSQL => ConnectorType::PostgreSQL,
            DatabaseType::MySQL => ConnectorType::MySQL,
            DatabaseType::ClickHouse => ConnectorType::ClickHouse,
            DatabaseType::MsSql => ConnectorType::MsSql,
            DatabaseType::Redis => ConnectorType::Redis,
        }
    }
}

impl From<ConnectorType> for DatabaseType {
    fn from(connector_type: ConnectorType) -> Self {
        match connector_type {
            ConnectorType::SQLite => DatabaseType::SQLite,
            ConnectorType::PostgreSQL => DatabaseType::PostgreSQL,
            ConnectorType::MySQL => DatabaseType::MySQL,
            ConnectorType::ClickHouse => DatabaseType::ClickHouse,
            ConnectorType::MsSql => DatabaseType::MsSql,
            ConnectorType::Redis => DatabaseType::Redis,
        }
    }
}

/// Returns true if the host string (possibly containing a scheme and/or path)
/// specifies an explicit port like `host:1234` or `https://host:1234`.
pub(super) fn host_contains_explicit_port(host: &str) -> bool {
    let rest = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))
        .unwrap_or(host);
    let host_clean = rest.split(['/', '?']).next().unwrap_or(rest);
    match host_clean.rfind(':') {
        Some(idx) => {
            if host_clean.starts_with('[') {
                host_clean[..idx].ends_with(']')
                    && host_clean[idx + 1..].chars().all(|c| c.is_ascii_digit())
            } else {
                host_clean[idx + 1..].chars().all(|c| c.is_ascii_digit())
            }
        }
        None => false,
    }
}
