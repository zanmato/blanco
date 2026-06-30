use blanco_core::DriverType;

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

impl From<DriverType> for ConnectorType {
    fn from(driver_type: DriverType) -> Self {
        match driver_type {
            DriverType::SQLite => ConnectorType::SQLite,
            DriverType::PostgreSQL => ConnectorType::PostgreSQL,
            DriverType::MySQL => ConnectorType::MySQL,
            DriverType::ClickHouse => ConnectorType::ClickHouse,
            DriverType::MsSql => ConnectorType::MsSql,
            DriverType::Redis => ConnectorType::Redis,
        }
    }
}

impl From<ConnectorType> for DriverType {
    fn from(connector_type: ConnectorType) -> Self {
        match connector_type {
            ConnectorType::SQLite => DriverType::SQLite,
            ConnectorType::PostgreSQL => DriverType::PostgreSQL,
            ConnectorType::MySQL => DriverType::MySQL,
            ConnectorType::ClickHouse => DriverType::ClickHouse,
            ConnectorType::MsSql => DriverType::MsSql,
            ConnectorType::Redis => DriverType::Redis,
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
