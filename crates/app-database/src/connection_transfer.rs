//! Moving connections between machines: a JSON document of connection
//! settings without any secret, and the plan for importing one next to the
//! connections already saved.

use serde::{Deserialize, Serialize};

use crate::{ConnectionData, EnvironmentType};
use database::{DatabaseType, SecretKind};

/// Identifies the document format, so a wrong file is refused clearly.
const FORMAT: &str = "blanco-connections";
const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Document {
    format: String,
    version: u32,
    connections: Vec<ExportedConnection>,
}

/// One connection as written to the file. Passwords and key passphrases are
/// never part of it; they stay in the keyring of the machine they were
/// entered on.
#[derive(Debug, Serialize, Deserialize)]
struct ExportedConnection {
    name: String,
    db_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    database_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    database_path: Option<String>,
    environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_private_key_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_jump_hosts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssl_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssl_key_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssl_cert_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssl_ca_cert_path: Option<String>,
    #[serde(default)]
    trust_server_certificate: bool,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    only_configured_database: bool,
    /// Which secrets are asked for when the connection opens instead of
    /// stored, by `SecretKind::key`. The setting, never a secret itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ask_on_connect: Vec<String>,
}

impl From<&ConnectionData> for ExportedConnection {
    fn from(connection: &ConnectionData) -> Self {
        Self {
            name: connection.name.clone(),
            db_type: connection.db_type.as_str().to_string(),
            host: connection.host.clone(),
            port: connection.port,
            database_name: connection.database_name.clone(),
            username: connection.username.clone(),
            database_path: connection.database_path.clone(),
            environment: connection.environment_type.display_name(),
            ssh_host: connection.ssh_host.clone(),
            ssh_port: connection.ssh_port,
            ssh_user: connection.ssh_user.clone(),
            ssh_private_key_path: connection.ssh_private_key_path.clone(),
            ssh_jump_hosts: connection.ssh_jump_hosts.clone(),
            ssl_mode: connection.ssl_mode.clone(),
            ssl_key_path: connection.ssl_key_path.clone(),
            ssl_cert_path: connection.ssl_cert_path.clone(),
            ssl_ca_cert_path: connection.ssl_ca_cert_path.clone(),
            trust_server_certificate: connection.trust_server_certificate,
            read_only: connection.read_only,
            only_configured_database: connection.only_configured_database,
            ask_on_connect: connection
                .ask_secrets
                .iter()
                .map(|kind| kind.key().to_string())
                .collect(),
        }
    }
}

impl ExportedConnection {
    fn into_connection_data(self) -> anyhow::Result<ConnectionData> {
        let db_type = DatabaseType::from_db_type_str(&self.db_type).ok_or_else(|| {
            anyhow::anyhow!(
                "Connection \"{}\" has an unknown type \"{}\"",
                self.name,
                self.db_type
            )
        })?;
        // A tag is exported by name. One this machine does not have falls
        // back to the default.
        let environment_type = EnvironmentType::named(&self.environment).unwrap_or_default();
        let mut connection = ConnectionData::new_sqlite(self.name, String::new());
        connection.db_type = db_type;
        connection.host = self.host;
        connection.port = self.port;
        connection.database_name = self.database_name;
        connection.username = self.username;
        connection.database_path = self.database_path;
        connection.environment_type = environment_type;
        connection.ssh_host = self.ssh_host;
        connection.ssh_port = self.ssh_port;
        connection.ssh_user = self.ssh_user;
        connection.ssh_private_key_path = self.ssh_private_key_path;
        connection.ssh_jump_hosts = self.ssh_jump_hosts;
        connection.ssl_mode = self.ssl_mode;
        connection.ssl_key_path = self.ssl_key_path;
        connection.ssl_cert_path = self.ssl_cert_path;
        connection.ssl_ca_cert_path = self.ssl_ca_cert_path;
        connection.trust_server_certificate = self.trust_server_certificate;
        connection.read_only = self.read_only;
        connection.only_configured_database = self.only_configured_database;
        // A name this build does not know is skipped rather than refused.
        connection.ask_secrets = self
            .ask_on_connect
            .iter()
            .filter_map(|key| SecretKind::from_key(key))
            .collect();
        Ok(connection)
    }
}

/// The connections as a JSON document, without any secret.
pub fn export_connections(connections: &[ConnectionData]) -> String {
    let document = Document {
        format: FORMAT.to_string(),
        version: VERSION,
        connections: connections.iter().map(ExportedConnection::from).collect(),
    };
    serde_json::to_string_pretty(&document).expect("connections serialize")
}

/// The connections in an exported document. They have no id and no secrets.
pub fn parse_connections(text: &str) -> anyhow::Result<Vec<ConnectionData>> {
    let document: Document = serde_json::from_str(text)
        .map_err(|error| anyhow::anyhow!("This is not a Blanco connections file: {error}"))?;
    if document.format != FORMAT {
        anyhow::bail!("This is not a Blanco connections file");
    }
    if document.version > VERSION {
        anyhow::bail!("This connections file was written by a newer Blanco");
    }
    document
        .connections
        .into_iter()
        .map(ExportedConnection::into_connection_data)
        .collect()
}

/// What importing a document would do.
#[derive(Debug, Default)]
pub struct ImportPlan {
    /// Connections to add, with the name they had in the file when it was
    /// taken already and they are added under a new one.
    pub add: Vec<(ConnectionData, Option<String>)>,
    /// Names of connections that are already saved with the same settings.
    pub skipped: Vec<String>,
}

/// Plan an import next to `existing`: a connection already saved with the
/// same settings is skipped, one whose name is taken by a different
/// connection is renamed "name (imported)", "name (imported 2)", ...
pub fn plan_import(incoming: Vec<ConnectionData>, existing: &[ConnectionData]) -> ImportPlan {
    let mut taken: std::collections::HashSet<String> = existing
        .iter()
        .map(|connection| connection.name.clone())
        .collect();
    let mut plan = ImportPlan::default();
    for connection in incoming {
        if existing
            .iter()
            .any(|saved| same_settings(saved, &connection))
        {
            plan.skipped.push(connection.name);
            continue;
        }
        let mut connection = connection;
        let renamed_from = if taken.contains(&connection.name) {
            let original = connection.name.clone();
            connection.name = (1usize..)
                .map(|number| match number {
                    1 => format!("{original} (imported)"),
                    number => format!("{original} (imported {number})"),
                })
                .find(|candidate| !taken.contains(candidate))
                .unwrap_or_default();
            Some(original)
        } else {
            None
        };
        taken.insert(connection.name.clone());
        plan.add.push((connection, renamed_from));
    }
    plan
}

/// Whether two connections are the same apart from id, secrets and use.
fn same_settings(a: &ConnectionData, b: &ConnectionData) -> bool {
    let comparable = |connection: &ConnectionData| ConnectionData {
        id: None,
        password: None,
        ssh_password: None,
        ssh_private_key_password: None,
        last_used_at: None,
        is_active: None,
        ..connection.clone()
    };
    comparable(a) == comparable(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn postgres(name: &str) -> ConnectionData {
        ConnectionData::new_postgres(
            name.to_string(),
            "db.example.com".to_string(),
            5432,
            "app".to_string(),
            "admin".to_string(),
            "secret".to_string(),
        )
        .with_ssh(
            "bastion".to_string(),
            22,
            "deploy".to_string(),
            Some("ssh-secret".to_string()),
            Some("~/.ssh/id_ed25519".to_string()),
            Some("passphrase".to_string()),
        )
    }

    #[test]
    fn test_export_leaves_secrets_out_and_round_trips() {
        let mut prod = postgres("Prod");
        prod.id = Some(4);
        prod.environment_type = EnvironmentType::PROD;
        prod.read_only = true;

        let document = export_connections(&[prod.clone()]);
        assert!(!document.contains("secret"), "{document}");
        assert!(!document.contains("passphrase"), "{document}");

        let imported = parse_connections(&document).unwrap();
        assert_eq!(imported.len(), 1);
        let imported = &imported[0];
        assert_eq!(imported.id, None);
        assert_eq!(imported.password, None);
        assert_eq!(imported.ssh_password, None);
        assert_eq!(imported.environment_type, EnvironmentType::PROD);
        assert_eq!(
            imported.ssh_private_key_path.as_deref(),
            Some("~/.ssh/id_ed25519")
        );
        assert!(same_settings(imported, &prod));
    }

    #[test]
    fn test_export_carries_which_secrets_are_asked_for() {
        let mut prod = postgres("Prod");
        prod.ask_secrets = vec![SecretKind::Password, SecretKind::SshPrivateKeyPassword];
        prod.ssh_jump_hosts = Some("deploy@bastion:2222, inner".to_string());

        let document = export_connections(&[prod.clone()]);
        assert!(!document.contains("secret"), "{document}");
        let imported = parse_connections(&document).unwrap();
        assert_eq!(imported[0].ask_secrets, prod.ask_secrets);
        assert!(same_settings(&imported[0], &prod));

        // A file written before the setting existed asks for nothing, and a
        // name from a newer build is skipped.
        let older = document.replace("\"ask_on_connect\"", "\"unknown_field\"");
        assert!(parse_connections(&older).unwrap()[0].ask_secrets.is_empty());
        let newer = document.replace("\"password\"", "\"hardware-token\"");
        assert_eq!(
            parse_connections(&newer).unwrap()[0].ask_secrets,
            vec![SecretKind::SshPrivateKeyPassword]
        );
    }

    #[test]
    fn test_plan_skips_duplicates_and_renames_name_clashes() {
        let mut saved_same = postgres("Same");
        saved_same.id = Some(1);
        let mut saved_clash = postgres("Clash");
        saved_clash.id = Some(2);
        saved_clash.port = Some(6543);

        let incoming = parse_connections(&export_connections(&[
            postgres("Same"),
            postgres("Clash"),
            postgres("New"),
        ]))
        .unwrap();
        let plan = plan_import(incoming, &[saved_same, saved_clash]);

        assert_eq!(plan.skipped, ["Same"]);
        let added: Vec<(&str, Option<&str>)> = plan
            .add
            .iter()
            .map(|(connection, renamed_from)| (connection.name.as_str(), renamed_from.as_deref()))
            .collect();
        assert_eq!(added, [("Clash (imported)", Some("Clash")), ("New", None)]);
    }

    #[test]
    fn test_other_files_are_refused() {
        assert!(parse_connections("{}").is_err());
        assert!(parse_connections(r#"{"format":"other","version":1,"connections":[]}"#).is_err());
        assert!(
            parse_connections(r#"{"format":"blanco-connections","version":99,"connections":[]}"#)
                .is_err()
        );
    }
}
