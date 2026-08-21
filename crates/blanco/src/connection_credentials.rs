use std::future::Future;

use anyhow::{Context as _, Result};
use gpui::{App, Task};

use app_database::{ConnectionData, LegacyConnectionCredentials};

#[derive(Clone, Copy)]
enum CredentialKind {
    Password,
    SshPassword,
    SshPrivateKeyPassword,
}

impl CredentialKind {
    fn key(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::SshPassword => "ssh-password",
            Self::SshPrivateKeyPassword => "ssh-private-key-password",
        }
    }
}

struct CredentialRead {
    connection_index: usize,
    kind: CredentialKind,
    task: Task<Result<Option<(String, Vec<u8>)>>>,
}

fn credential_path(connection_id: i64, kind: CredentialKind) -> String {
    format!("blanco://connections/{connection_id}/{}", kind.key())
}

fn start_writing_value(
    connection_id: i64,
    kind: CredentialKind,
    value: Option<&str>,
    cx: &App,
) -> Task<Result<()>> {
    let path = credential_path(connection_id, kind);
    match value.filter(|value| !value.is_empty()) {
        Some(value) => cx.write_credentials(&path, kind.key(), value.as_bytes()),
        None => cx.delete_credentials(&path),
    }
}

pub fn start_writing_connection(
    connection_id: i64,
    connection: &ConnectionData,
    cx: &App,
) -> Vec<Task<Result<()>>> {
    [
        (CredentialKind::Password, connection.password.as_deref()),
        (
            CredentialKind::SshPassword,
            connection.ssh_password.as_deref(),
        ),
        (
            CredentialKind::SshPrivateKeyPassword,
            connection.ssh_private_key_password.as_deref(),
        ),
    ]
    .into_iter()
    .map(|(kind, value)| start_writing_value(connection_id, kind, value, cx))
    .collect()
}

pub async fn migrate_legacy_credentials(
    credentials: &[LegacyConnectionCredentials],
    cx: &App,
) -> Result<()> {
    for credentials in credentials {
        for (kind, value) in [
            (CredentialKind::Password, credentials.password.as_deref()),
            (
                CredentialKind::SshPassword,
                credentials.ssh_password.as_deref(),
            ),
            (
                CredentialKind::SshPrivateKeyPassword,
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

fn start_reading(connections: &[ConnectionData], cx: &App) -> Vec<CredentialRead> {
    connections
        .iter()
        .enumerate()
        .filter_map(|(connection_index, connection)| {
            connection.id.map(|connection_id| {
                [
                    CredentialKind::Password,
                    CredentialKind::SshPassword,
                    CredentialKind::SshPrivateKeyPassword,
                ]
                .into_iter()
                .map(move |kind| CredentialRead {
                    connection_index,
                    kind,
                    task: cx.read_credentials(&credential_path(connection_id, kind)),
                })
            })
        })
        .flatten()
        .collect()
}

/// Start keychain reads for every connection and return a future that fills
/// the secrets in once they land. The reads are kicked off synchronously (they
/// need `cx`), but the returned future owns everything it touches so callers
/// can await it from a spawned task instead of blocking the foreground thread.
pub fn hydrate_connections(
    mut connections: Vec<ConnectionData>,
    cx: &App,
) -> impl Future<Output = Result<Vec<ConnectionData>>> + use<> {
    let reads = start_reading(&connections, cx);
    async move {
        for read in reads {
            let Some((_, bytes)) = read.task.await? else {
                continue;
            };
            let value =
                String::from_utf8(bytes).context("connection credential is not valid UTF-8")?;
            let Some(connection) = connections.get_mut(read.connection_index) else {
                continue;
            };
            match read.kind {
                CredentialKind::Password => connection.password = Some(value),
                CredentialKind::SshPassword => connection.ssh_password = Some(value),
                CredentialKind::SshPrivateKeyPassword => {
                    connection.ssh_private_key_password = Some(value);
                }
            }
        }
        Ok(connections)
    }
}

pub fn start_deleting_connection(connection_id: i64, cx: &App) -> Vec<Task<Result<()>>> {
    [
        CredentialKind::Password,
        CredentialKind::SshPassword,
        CredentialKind::SshPrivateKeyPassword,
    ]
    .into_iter()
    .map(|kind| cx.delete_credentials(&credential_path(connection_id, kind)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::{CredentialKind, credential_path};

    #[test]
    fn credential_paths_are_stable_and_connection_scoped() {
        assert_eq!(
            credential_path(42, CredentialKind::Password),
            "blanco://connections/42/password"
        );
        assert_ne!(
            credential_path(42, CredentialKind::Password),
            credential_path(43, CredentialKind::Password)
        );
    }
}
