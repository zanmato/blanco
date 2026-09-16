//! Connection secrets live in the OS keyring, one entry per connection and
//! secret kind. Nothing reads them in bulk: `DatabaseService` asks
//! [`KeyringSecretStore`] for a connection's secrets when that connection is
//! opened, and the edit dialog hydrates the single connection it shows. The
//! store funnels every read through one foreground task so the keyring daemon
//! sees one Secret Service session at a time, however many connections open
//! at once (gnome-keyring falls over under a burst of concurrent sessions).

use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use gpui::{App, Task};

use app_database::{ConnectionData, LegacyConnectionCredentials};
use database::{ConnectionSecretStore, SecretKind};

fn credential_path(connection_id: i64, kind: SecretKind) -> String {
    format!("blanco://connections/{connection_id}/{}", kind.key())
}

fn start_writing_value(
    connection_id: i64,
    kind: SecretKind,
    value: Option<&str>,
    cx: &App,
) -> Task<Result<()>> {
    let path = credential_path(connection_id, kind);
    match value.filter(|value| !value.is_empty()) {
        Some(value) => cx.write_credentials(&path, kind.key(), value.as_bytes()),
        None => cx.delete_credentials(&path),
    }
}

fn secret_of(connection: &ConnectionData, kind: SecretKind) -> Option<&str> {
    match kind {
        SecretKind::Password => connection.password.as_deref(),
        SecretKind::SshPassword => connection.ssh_password.as_deref(),
        SecretKind::SshPrivateKeyPassword => connection.ssh_private_key_password.as_deref(),
    }
}

fn set_secret(connection: &mut ConnectionData, kind: SecretKind, value: Option<String>) {
    match kind {
        SecretKind::Password => connection.password = value,
        SecretKind::SshPassword => connection.ssh_password = value,
        SecretKind::SshPrivateKeyPassword => connection.ssh_private_key_password = value,
    }
}

pub fn start_writing_connection(
    connection_id: i64,
    connection: &ConnectionData,
    cx: &App,
) -> Vec<Task<Result<()>>> {
    SecretKind::ALL
        .into_iter()
        .map(|kind| start_writing_value(connection_id, kind, secret_of(connection, kind), cx))
        .collect()
}

pub async fn migrate_legacy_credentials(
    credentials: &[LegacyConnectionCredentials],
    cx: &App,
) -> Result<()> {
    for credentials in credentials {
        for (kind, value) in [
            (SecretKind::Password, credentials.password.as_deref()),
            (SecretKind::SshPassword, credentials.ssh_password.as_deref()),
            (
                SecretKind::SshPrivateKeyPassword,
                credentials.ssh_private_key_password.as_deref(),
            ),
        ] {
            let Some(value) = value.filter(|value| !value.is_empty()) else {
                continue;
            };
            let path = credential_path(credentials.connection_id, kind);
            if cx.read_credentials(&path).await?.is_none() {
                cx.write_credentials(&path, kind.key(), value.as_bytes())
                    .await?;
            }
        }
    }
    Ok(())
}

pub async fn finish_writing(tasks: Vec<Task<Result<()>>>) -> Result<()> {
    for task in tasks {
        task.await?;
    }
    Ok(())
}

struct SecretRequest {
    connection_id: i64,
    kind: SecretKind,
    reply: smol::channel::Sender<Result<Option<String>>>,
}

/// The keyring-backed [`ConnectionSecretStore`]. Cheap to clone and `Send`,
/// so `DatabaseService` can hold it while the actual `read_credentials` calls
/// happen on the GPUI foreground task spawned by [`KeyringSecretStore::install`].
#[derive(Clone)]
pub struct KeyringSecretStore {
    sender: smol::channel::Sender<SecretRequest>,
}

impl KeyringSecretStore {
    pub fn install(cx: &mut App) -> Arc<Self> {
        let (sender, receiver) = smol::channel::unbounded::<SecretRequest>();
        cx.spawn(async move |cx| {
            while let Ok(request) = receiver.recv().await {
                let path = credential_path(request.connection_id, request.kind);
                // Awaiting each read before taking the next request is what
                // serializes keyring access.
                let read = cx.update(|cx| cx.read_credentials(&path));
                let result = read.await.and_then(|found| {
                    found
                        .map(|(_, bytes)| {
                            String::from_utf8(bytes)
                                .context("connection credential is not valid UTF-8")
                        })
                        .transpose()
                });
                if request.reply.try_send(result).is_err() {
                    tracing::warn!(
                        "credential read for connection {} finished after its caller stopped waiting",
                        request.connection_id
                    );
                }
            }
        })
        .detach();
        Arc::new(Self { sender })
    }
}

#[async_trait]
impl ConnectionSecretStore for KeyringSecretStore {
    async fn read(&self, connection_id: i64, kind: SecretKind) -> Result<Option<String>> {
        let (reply, receiver) = smol::channel::bounded(1);
        self.sender
            .send(SecretRequest {
                connection_id,
                kind,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Blanco's window is not running"))?;
        receiver
            .recv()
            .await
            .map_err(|_| anyhow::anyhow!("Blanco's window closed before reading the credential"))?
    }
}

/// Fill in the stored secrets of one connection, for the edit dialog.
pub async fn hydrate_connection(
    store: &dyn ConnectionSecretStore,
    mut connection: ConnectionData,
) -> Result<ConnectionData> {
    let Some(connection_id) = connection.id else {
        return Ok(connection);
    };
    for kind in SecretKind::ALL {
        if let Some(value) = store.read(connection_id, kind).await? {
            set_secret(&mut connection, kind, Some(value));
        }
    }
    Ok(connection)
}

pub fn start_deleting_connection(connection_id: i64, cx: &App) -> Vec<Task<Result<()>>> {
    SecretKind::ALL
        .into_iter()
        .map(|kind| cx.delete_credentials(&credential_path(connection_id, kind)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{KeyringSecretStore, credential_path, hydrate_connection};
    use app_database::ConnectionData;
    use database::{ConnectionSecretStore, SecretKind};
    use gpui::TestAppContext;

    #[test]
    fn credential_paths_are_stable_and_connection_scoped() {
        assert_eq!(
            credential_path(42, SecretKind::Password),
            "blanco://connections/42/password"
        );
        assert_ne!(
            credential_path(42, SecretKind::Password),
            credential_path(43, SecretKind::Password)
        );
    }

    /// The test platform stores nothing, so this covers the bridge round trip
    /// and the "no secret stored" path rather than an actual keyring hit.
    #[gpui::test]
    async fn keyring_store_reads_through_the_foreground_bridge(cx: &mut TestAppContext) {
        let store = cx.update(KeyringSecretStore::install);

        for kind in SecretKind::ALL {
            let found = store.read(7, kind).await.expect("read");
            assert_eq!(found, None);
        }

        let mut connection = ConnectionData::new_sqlite("seven".into(), "seven.db".into());
        connection.id = Some(7);
        connection.password = Some("typed".into());
        let connection = hydrate_connection(store.as_ref(), connection)
            .await
            .expect("hydrate");
        assert_eq!(connection.password.as_deref(), Some("typed"));
        assert_eq!(connection.ssh_password, None);
        assert_eq!(connection.ssh_private_key_password, None);
    }
}
