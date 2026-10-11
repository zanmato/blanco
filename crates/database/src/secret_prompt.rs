//! Asking the user for a secret while a connection opens.
//!
//! `DatabaseService` asks through a [`SecretPrompt`] for the secrets a
//! connection is set to ask for, and for a password the server wants when
//! none is stored. The application installs a prompt that shows a dialog;
//! the default [`NoSecretPrompt`] declines, so tests and headless use never
//! wait on one.

use async_trait::async_trait;

use crate::connection_config::{ConnectionConfig, SecretKind};

/// What a prompt shows: which connection, which secret and whose it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretPromptRequest {
    pub connection_id: i64,
    pub connection_name: String,
    pub kind: SecretKind,
    /// The account the secret belongs to, `user@host` for a password or the
    /// key path for a key passphrase.
    pub account: String,
    /// Why the secret is asked for again, when an earlier one was rejected.
    pub retry_reason: Option<String>,
}

impl SecretPromptRequest {
    pub fn new(config: &ConnectionConfig, kind: SecretKind, retry_reason: Option<String>) -> Self {
        let account = match kind {
            SecretKind::Password => format!("{}@{}", config.username, config.host),
            SecretKind::SshPassword => format!(
                "{}@{}",
                config.ssh_user.as_deref().unwrap_or_default(),
                config.ssh_host.as_deref().unwrap_or_default()
            ),
            SecretKind::SshPrivateKeyPassword => {
                config.ssh_private_key_path.clone().unwrap_or_default()
            }
        };
        Self {
            connection_id: config.id,
            connection_name: config.name.clone(),
            kind,
            account,
            retry_reason,
        }
    }
}

#[async_trait]
pub trait SecretPrompt: Send + Sync {
    /// Ask for the secret. `Ok(None)` means the user cancelled.
    async fn ask(&self, request: SecretPromptRequest) -> anyhow::Result<Option<String>>;

    /// Whether a person can answer. A prompt that cannot is never asked for a
    /// password the server wants, so the server's error stands.
    fn is_interactive(&self) -> bool {
        true
    }
}

/// Prompt that always declines, used until the application installs one.
pub struct NoSecretPrompt;

#[async_trait]
impl SecretPrompt for NoSecretPrompt {
    async fn ask(&self, _request: SecretPromptRequest) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    fn is_interactive(&self) -> bool {
        false
    }
}

/// The user dismissed a secret prompt, so the connection was not opened.
#[derive(Debug)]
pub struct SecretPromptCancelled {
    pub kind: SecretKind,
}

impl std::fmt::Display for SecretPromptCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Connecting was cancelled at the {} prompt",
            self.kind.label()
        )
    }
}

impl std::error::Error for SecretPromptCancelled {}

/// The secret a failed connect was refused for, when asking for it again
/// could help: the server or SSH host rejected the credentials and the
/// secret was asked for, prompted for, or missing.
pub(crate) fn rejected_secret(
    config: &ConnectionConfig,
    prompted: &[SecretKind],
    error: &anyhow::Error,
) -> Option<SecretKind> {
    let message = format!("{error:#}").to_lowercase();
    let promptable = |kind: SecretKind| {
        config.uses_secret(kind)
            && (config.asks_for(kind)
                || prompted.contains(&kind)
                || config.secret(kind).is_none_or(str::is_empty))
    };
    if is_ssh_key_failure(&message) {
        let kind = SecretKind::SshPrivateKeyPassword;
        return promptable(kind).then_some(kind);
    }
    if is_ssh_password_failure(&message) {
        // The tunnel only tries a password when it has no key.
        let kind = SecretKind::SshPassword;
        return (!config.uses_secret(SecretKind::SshPrivateKeyPassword) && promptable(kind))
            .then_some(kind);
    }
    if is_authentication_failure(&message) {
        let kind = SecretKind::Password;
        return promptable(kind).then_some(kind);
    }
    None
}

fn is_ssh_key_failure(message: &str) -> bool {
    message.contains("failed to load private key") || message.contains("ssh key authentication")
}

fn is_ssh_password_failure(message: &str) -> bool {
    message.contains("ssh password authentication")
        || message.contains("no authentication method provided")
}

/// Whether a lowercased error message is a server refusing the credentials,
/// as each backend words it.
pub(crate) fn is_authentication_failure(message: &str) -> bool {
    const MARKERS: &[&str] = &[
        // PostgreSQL
        "password authentication failed",
        "no password was provided",
        "no password supplied",
        // MySQL and MariaDB: 1045, not 1044 (no access to a database)
        "1045 (28000)",
        // SQL Server
        "login failed for user",
        // ClickHouse
        "authentication failed",
        "authentication_failed",
        // Redis
        "noauth",
        "wrongpass",
        "invalid username-password pair",
    ];
    MARKERS.iter().any(|marker| message.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatabaseType;

    fn server(password: Option<&str>) -> ConnectionConfig {
        ConnectionConfig::new(
            1,
            "prod".to_string(),
            DatabaseType::PostgreSQL,
            "db.example.com".to_string(),
            5432,
            "app".to_string(),
            "alice".to_string(),
            password.map(str::to_string),
        )
    }

    #[test]
    fn rejected_password_is_promptable_only_when_missing_asked_or_prompted() {
        let error = anyhow::anyhow!(
            "Failed to connect to PostgreSQL: password authentication failed for user \"alice\""
        );
        assert_eq!(
            rejected_secret(&server(None), &[], &error),
            Some(SecretKind::Password)
        );
        // A stored password the server rejects is reported, not prompted for.
        assert_eq!(rejected_secret(&server(Some("old")), &[], &error), None);
        assert_eq!(
            rejected_secret(&server(Some("typed")), &[SecretKind::Password], &error),
            Some(SecretKind::Password)
        );
        let asking = server(Some("typed")).with_ask_secrets(vec![SecretKind::Password]);
        assert_eq!(
            rejected_secret(&asking, &[], &error),
            Some(SecretKind::Password)
        );
        // Other failures are not about the password.
        let unreachable = anyhow::anyhow!("Connection refused (os error 111)");
        assert_eq!(rejected_secret(&server(None), &[], &unreachable), None);
        let mysql_database =
            anyhow::anyhow!("1044 (42000): Access denied for user 'alice'@'%' to database 'app'");
        assert_eq!(rejected_secret(&server(None), &[], &mysql_database), None);
    }

    #[test]
    fn ssh_failures_name_the_ssh_secret() {
        let tunnel = server(Some("db")).with_ssh_config(
            "bastion".to_string(),
            "deploy".to_string(),
            None,
            None,
            None,
            None,
        );
        let rejected = anyhow::anyhow!("SSH password authentication was rejected");
        assert_eq!(
            rejected_secret(&tunnel, &[], &rejected),
            Some(SecretKind::SshPassword)
        );
        let keyed = server(Some("db")).with_ssh_config(
            "bastion".to_string(),
            "deploy".to_string(),
            None,
            Some("/home/deploy/.ssh/id_ed25519".to_string()),
            None,
            None,
        );
        let locked = anyhow::anyhow!("Failed to load private key: The key is encrypted");
        assert_eq!(
            rejected_secret(&keyed, &[], &locked),
            Some(SecretKind::SshPrivateKeyPassword)
        );
        let request = SecretPromptRequest::new(&keyed, SecretKind::SshPrivateKeyPassword, None);
        assert_eq!(request.account, "/home/deploy/.ssh/id_ed25519");
        let request = SecretPromptRequest::new(&tunnel, SecretKind::SshPassword, None);
        assert_eq!(request.account, "deploy@bastion");
    }
}
