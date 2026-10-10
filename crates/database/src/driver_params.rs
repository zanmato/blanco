//! Extra driver parameters: connection options the connection dialog does not
//! expose, added to the driver's connection URL as `key=value` pairs.
//!
//! Each backend accepts only the keys its driver reads, so a typo or an option
//! the driver lacks fails with a message naming it instead of being dropped.

use crate::DatabaseType;

/// Keys the dialog's own fields write. Naming one here would set it twice.
const DIALOG_KEYS: &[(&str, &str)] = &[
    ("sslmode", "SSL Mode"),
    ("ssl-mode", "SSL Mode"),
    ("ssl_mode", "SSL Mode"),
    ("sslkey", "SSL Key Path"),
    ("ssl-key", "SSL Key Path"),
    ("sslcert", "SSL Cert Path"),
    ("ssl-cert", "SSL Cert Path"),
    ("sslrootcert", "SSL CA Cert Path"),
    ("ssl-root-cert", "SSL CA Cert Path"),
    ("sslca", "SSL CA Cert Path"),
    ("ssl-ca", "SSL CA Cert Path"),
    ("host", "Host"),
    ("hostaddr", "Host"),
    ("port", "Port"),
    ("database", "Database name"),
    ("dbname", "Database name"),
    ("user", "User"),
    ("password", "Password"),
];

/// Query parameters the PostgreSQL driver (sqlx) reads besides the dialog's.
const POSTGRES_KEYS: &[&str] = &["application_name", "options", "statement-cache-capacity"];

/// Query parameters the MySQL driver (sqlx) reads besides the dialog's.
const MYSQL_KEYS: &[&str] = &[
    "charset",
    "collation",
    "socket",
    "statement-cache-capacity",
    "time-zone",
    "timezone",
];

/// ClickHouse HTTP parameters that are not settings.
const CLICKHOUSE_RESERVED: &[&str] = &["query", "default_format", "query_id", "session_id"];

/// Check `params` against what `db_type`'s driver reads. The error names the
/// offending key and what is accepted instead.
pub fn validate_driver_params(
    db_type: DatabaseType,
    params: &[(String, String)],
) -> Result<(), String> {
    let backend = db_type.as_str();
    for (ix, (key, _)) in params.iter().enumerate() {
        if key.is_empty() {
            return Err("A driver parameter has a value but no name".to_string());
        }
        if params[..ix].iter().any(|(earlier, _)| earlier == key) {
            return Err(format!("The driver parameter \"{key}\" is listed twice"));
        }
        if let Some((_, field)) = DIALOG_KEYS
            .iter()
            .find(|(owned, _)| owned.eq_ignore_ascii_case(key))
        {
            return Err(format!(
                "\"{key}\" is set by the {field} field, not as a driver parameter"
            ));
        }
        let accepted = match db_type {
            DatabaseType::PostgreSQL => {
                POSTGRES_KEYS.contains(&key.as_str())
                    || key
                        .strip_prefix("options[")
                        .and_then(|rest| rest.strip_suffix(']'))
                        .is_some_and(|setting| !setting.is_empty())
            }
            DatabaseType::MySQL => MYSQL_KEYS.contains(&key.as_str()),
            // Every other HTTP parameter is a ClickHouse setting, which the
            // server checks and names when unknown.
            DatabaseType::ClickHouse => {
                !CLICKHOUSE_RESERVED.contains(&key.as_str())
                    && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            }
            DatabaseType::SQLite | DatabaseType::MsSql | DatabaseType::Redis => {
                return Err(format!("{backend} connections take no driver parameters"));
            }
        };
        if !accepted {
            return Err(format!(
                "{backend} does not support the driver parameter \"{key}\". {}",
                accepted_keys(db_type)
            ));
        }
    }
    Ok(())
}

fn accepted_keys(db_type: DatabaseType) -> String {
    match db_type {
        DatabaseType::PostgreSQL => "It accepts application_name, options, \
            options[<setting>] and statement-cache-capacity"
            .to_string(),
        DatabaseType::MySQL => "It accepts charset, collation, socket, \
            statement-cache-capacity and time-zone"
            .to_string(),
        DatabaseType::ClickHouse => {
            "It accepts ClickHouse settings such as max_execution_time".to_string()
        }
        _ => String::new(),
    }
}

/// `params` as `key=value` pairs joined by `&`, values percent-encoded.
pub(crate) fn encode_driver_params(params: &[(String, String)]) -> Vec<String> {
    params
        .iter()
        .map(|(key, value)| {
            let value: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
            format!("{key}={value}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn accepts_what_each_driver_reads() {
        assert_eq!(
            validate_driver_params(
                DatabaseType::PostgreSQL,
                &params(&[
                    ("options", "-c statement_timeout=5000"),
                    ("options[search_path]", "app"),
                    ("application_name", "reports"),
                ])
            ),
            Ok(())
        );
        assert_eq!(
            validate_driver_params(
                DatabaseType::MySQL,
                &params(&[("charset", "utf8mb4"), ("time-zone", "+00:00")])
            ),
            Ok(())
        );
        assert_eq!(
            validate_driver_params(
                DatabaseType::ClickHouse,
                &params(&[("max_execution_time", "30")])
            ),
            Ok(())
        );
        assert_eq!(validate_driver_params(DatabaseType::MsSql, &[]), Ok(()));
    }

    #[test]
    fn rejects_unknown_keys_by_name() {
        let error = validate_driver_params(
            DatabaseType::PostgreSQL,
            &params(&[("sslnegotiation", "direct")]),
        )
        .unwrap_err();
        assert!(
            error
                .starts_with("PostgreSQL does not support the driver parameter \"sslnegotiation\""),
            "{error}"
        );

        let error = validate_driver_params(DatabaseType::MySQL, &params(&[("local_infile", "1")]))
            .unwrap_err();
        assert!(error.contains("\"local_infile\""), "{error}");

        let error =
            validate_driver_params(DatabaseType::ClickHouse, &params(&[("max time", "30")]))
                .unwrap_err();
        assert!(error.contains("\"max time\""), "{error}");

        let error = validate_driver_params(DatabaseType::MsSql, &params(&[("encrypt", "true")]))
            .unwrap_err();
        assert_eq!(error, "SQL Server connections take no driver parameters");
    }

    #[test]
    fn rejects_dialog_keys_empty_names_and_duplicates() {
        assert_eq!(
            validate_driver_params(DatabaseType::PostgreSQL, &params(&[("sslmode", "require")])),
            Err("\"sslmode\" is set by the SSL Mode field, not as a driver parameter".to_string())
        );
        assert_eq!(
            validate_driver_params(DatabaseType::MySQL, &params(&[("", "x")])),
            Err("A driver parameter has a value but no name".to_string())
        );
        assert_eq!(
            validate_driver_params(
                DatabaseType::MySQL,
                &params(&[("charset", "utf8"), ("charset", "latin1")])
            ),
            Err("The driver parameter \"charset\" is listed twice".to_string())
        );
    }

    #[test]
    fn encodes_values() {
        assert_eq!(
            encode_driver_params(&params(&[("options", "-c statement_timeout=5000")])),
            ["options=-c+statement_timeout%3D5000"]
        );
    }
}
