#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

pub mod connection_transfer;
mod connections;
mod dashboards;
mod environment_tags;
mod query_history;
mod query_tabs;
mod schema;
mod settings;
mod snippets;
mod types;
mod virtual_foreign_keys;
mod windows;

pub use connections::LegacyConnectionCredentials;
pub use dashboards::{DASHBOARD_COLUMNS, DashboardData, DashboardPanelData, PanelConnection};
pub use schema::{app_db_path, init_schema};
pub use types::{
    ConnectionData, EditorKind, EnvironmentType, QueryHistoryData, QueryTabData, SnippetData,
    WindowData, WindowGeometry,
};
pub use virtual_foreign_keys::VirtualForeignKey;

use gpui::{App, Global};
use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use std::future::Future;
use std::str::FromStr;
use tokio::runtime::Handle;

/// Application database for persisting query tabs, history, and connections.
///
/// sqlx is built with the `runtime-tokio` feature, so all of its work must be
/// driven on the shared tokio runtime, GPUI's smol executor would otherwise
/// panic with "this functionality requires a Tokio context". `run` shuttles
/// each call onto the runtime via `Handle::spawn` so that callers from
/// `cx.spawn` / `cx.background_spawn` (smol) keep working unchanged.
#[derive(Clone)]
pub struct AppDatabase {
    pool: SqlitePool,
    runtime: Handle,
}

impl Global for AppDatabase {}

impl AppDatabase {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub(crate) fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    pub(crate) async fn run<F, T>(&self, fut: F) -> Result<T, sqlx::Error>
    where
        F: Future<Output = Result<T, sqlx::Error>> + Send + 'static,
        T: Send + 'static,
    {
        match self.runtime.spawn(fut).await {
            Ok(result) => result,
            Err(e) => Err(sqlx::Error::Configuration(
                format!("tokio task join failed: {e}").into(),
            )),
        }
    }
}

impl AppDatabase {
    pub async fn new(runtime: Handle) -> Result<Self, sqlx::Error> {
        let db_path = app_db_path();

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = runtime
            .spawn(async move { SqlitePool::connect_with(options).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;
        let pool_for_init = pool.clone();
        runtime
            .spawn(async move { init_schema(&pool_for_init).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;

        Ok(Self { pool, runtime })
    }

    /// In-memory database for tests in this and downstream crates.
    pub async fn new_in_memory(runtime: Handle) -> Result<Self, sqlx::Error> {
        let options =
            SqliteConnectOptions::from_str("sqlite::memory:")?.disable_statement_logging();

        let pool = runtime
            .spawn(async move { SqlitePool::connect_with(options).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;
        let pool_for_init = pool.clone();
        runtime
            .spawn(async move { init_schema(&pool_for_init).await })
            .await
            .map_err(|e| {
                sqlx::Error::Configuration(format!("tokio task join failed: {e}").into())
            })??;

        Ok(Self { pool, runtime })
    }
}

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
