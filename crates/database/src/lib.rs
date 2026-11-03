//! Database Crate for Blanco
//!
//! This crate provides a clean, unified database service with opaque SSH tunnel handling.
//! It manages database connections, SSH tunnels, and connection pooling internally.

#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

mod connection_config;
mod factories;
mod pgpass;
mod secret_prompt;
mod service;
pub mod ssh_config;
mod ssh_tunnel;
mod tokio_connection;

pub use connection_config::{
    postgres_ssl_mode, ConnectionConfig, ConnectionSecretStore, DatabaseType, NoSecretStore,
    ParamStyles, SecretKind,
};
pub use factories::{MysqlConnectionFactory, PostgresConnectionFactory, SqliteConnectionFactory};
pub use secret_prompt::{NoSecretPrompt, SecretPrompt, SecretPromptCancelled, SecretPromptRequest};
pub use service::{
    ClientToolEndpoint, ConnectionId, DatabaseConfigId, DatabaseConnectedMessage,
    DatabaseDisconnectedMessage, DatabaseService, DatabaseServiceMessage, DedicatedConnection,
    ShortLivedConnection,
};
pub use ssh_tunnel::{SshTunnel, SshTunnelConfig, TunnelInfo};

// Re-export blanco_core traits for convenience
pub use blanco_core::{Connection, ConnectionFactory, DatabaseService as DatabaseServiceTrait};
pub use blanco_duckdb::MEMORY_PATH;

#[cfg(test)]
mod locale_tests {
    use std::collections::{BTreeMap, BTreeSet};

    fn messages(locale: &str) -> BTreeMap<String, String> {
        crate::_rust_i18n_backend()
            .messages_for_locale(locale)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split("%{")
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .collect()
    }

    /// Every string has every translation, with the same placeholders.
    #[test]
    fn every_locale_translates_every_string() {
        let english = messages("en");
        assert!(!english.is_empty(), "no English strings");
        let locales = crate::_rust_i18n_backend().available_locales();
        assert!(
            locales.iter().any(|locale| locale == "sv"),
            "no Swedish strings"
        );
        let mut failures = Vec::new();
        for locale in locales {
            if locale == "en" {
                continue;
            }
            let translation = messages(&locale);
            for (key, text) in &english {
                match translation.get(key) {
                    None => failures.push(format!("{locale}: missing {key}")),
                    Some(translated) if placeholders(translated) != placeholders(text) => {
                        failures.push(format!("{locale}: {key} has other %{{}} placeholders"))
                    }
                    Some(_) => {}
                }
            }
            for key in translation.keys().filter(|key| !english.contains_key(*key)) {
                failures.push(format!("en: missing {key}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
