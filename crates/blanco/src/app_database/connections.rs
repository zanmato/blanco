use sqlx::Row;

use crate::app_database::{AppDatabase, ConnectionData, EnvironmentType};
use database::DatabaseType;

#[derive(Debug)]
pub struct LegacyConnectionCredentials {
    pub connection_id: i64,
    pub password: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_private_key_password: Option<String>,
}

impl AppDatabase {
    pub async fn save_connection(&self, conn: &ConnectionData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        let pool = self.pool();
        let conn = conn.clone();
        self.run(async move {
            let db_type_str = conn.db_type.as_str();
            if let Some(id) = conn.id {
                // Update existing connection
                sqlx::query(
                    r#"
                    UPDATE connections
                    SET name = ?, db_type = ?, host = ?, port = ?, database_name = ?,
                        username = ?, database_path = ?, last_used_at = ?,
                        is_active = ?, environment_type = ?,
                        ssh_host = ?, ssh_port = ?, ssh_user = ?,
                        ssh_private_key_path = ?,
                        ssl_mode = ?, ssl_key_path = ?, ssl_cert_path = ?, ssl_ca_cert_path = ?,
                        trust_server_certificate = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&conn.name)
                .bind(db_type_str)
                .bind(&conn.host)
                .bind(conn.port)
                .bind(&conn.database_name)
                .bind(&conn.username)
                .bind(&conn.database_path)
                .bind(now)
                .bind(conn.is_active.map(|b| if b { 1 } else { 0 }))
                .bind(conn.environment_type.to_i32())
                .bind(&conn.ssh_host)
                .bind(conn.ssh_port)
                .bind(&conn.ssh_user)
                .bind(&conn.ssh_private_key_path)
                .bind(&conn.ssl_mode)
                .bind(&conn.ssl_key_path)
                .bind(&conn.ssl_cert_path)
                .bind(&conn.ssl_ca_cert_path)
                .bind(if conn.trust_server_certificate { 1 } else { 0 })
                .bind(id)
                .execute(&pool)
                .await?;
                Ok(id)
            } else {
                // Insert new connection
                let result = sqlx::query(
                    r#"
                    INSERT INTO connections (name, db_type, host, port, database_name, username, database_path, last_used_at, created_at, is_active, environment_type, ssh_host, ssh_port, ssh_user, ssh_private_key_path, ssl_mode, ssl_key_path, ssl_cert_path, ssl_ca_cert_path, trust_server_certificate)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&conn.name)
                .bind(db_type_str)
                .bind(&conn.host)
                .bind(conn.port)
                .bind(&conn.database_name)
                .bind(&conn.username)
                .bind(&conn.database_path)
                .bind(now)
                .bind(now)
                .bind(conn.is_active.map(|b| if b { 1 } else { 0 }))
                .bind(conn.environment_type.to_i32())
                .bind(&conn.ssh_host)
                .bind(conn.ssh_port)
                .bind(&conn.ssh_user)
                .bind(&conn.ssh_private_key_path)
                .bind(&conn.ssl_mode)
                .bind(&conn.ssl_key_path)
                .bind(&conn.ssl_cert_path)
                .bind(&conn.ssl_ca_cert_path)
                .bind(if conn.trust_server_certificate { 1 } else { 0 })
                .execute(&pool)
                .await?;

                Ok(result.last_insert_rowid())
            }
        })
        .await
    }

    /// Load all connections from the database
    pub async fn load_connections(&self) -> Result<Vec<ConnectionData>, sqlx::Error> {
        let pool = self.pool();
        let mut connections = self
            .run(async move {
            let rows = sqlx::query(
                r#"
                SELECT id, name, db_type, host, port, database_name, username, database_path, last_used_at, is_active, environment_type, ssh_host, ssh_port, ssh_user, ssh_private_key_path, ssl_mode, ssl_key_path, ssl_cert_path, ssl_ca_cert_path, trust_server_certificate
                FROM connections
                ORDER BY name
                "#,
            )
            .fetch_all(&pool)
            .await?;

            let connections: Vec<ConnectionData> = rows
                .iter()
                .map(|row| {
                    let db_type_str: String = row.get("db_type");
                    let db_type = DatabaseType::from_db_type_str(&db_type_str)
                        .unwrap_or(DatabaseType::PostgreSQL);

                    ConnectionData {
                        id: Some(row.get("id")),
                        name: row.get("name"),
                        db_type,
                        host: row.get("host"),
                        port: row.get("port"),
                        database_name: row.get("database_name"),
                        username: row.get("username"),
                        password: None,
                        database_path: row.get("database_path"),
                        last_used_at: row.get("last_used_at"),
                        is_active: row.get::<Option<i32>, _>("is_active").map(|i| i == 1),
                        environment_type: EnvironmentType::from_i32(
                            row.get::<i64, _>("environment_type") as i32,
                        ),
                        ssh_host: row.get("ssh_host"),
                        ssh_port: row.get("ssh_port"),
                        ssh_user: row.get("ssh_user"),
                        ssh_password: None,
                        ssh_private_key_path: row.get("ssh_private_key_path"),
                        ssh_private_key_password: None,
                        ssl_mode: row.get("ssl_mode"),
                        ssl_key_path: row.get("ssl_key_path"),
                        ssl_cert_path: row.get("ssl_cert_path"),
                        ssl_ca_cert_path: row.get("ssl_ca_cert_path"),
                        trust_server_certificate: row
                            .get::<i64, _>("trust_server_certificate")
                            != 0,
                    }
                })
                .collect();

            Ok(connections)
        })
        .await?;

        let legacy_credentials = self.load_legacy_connection_credentials().await?;
        for credentials in legacy_credentials {
            if let Some(connection) = connections
                .iter_mut()
                .find(|connection| connection.id == Some(credentials.connection_id))
            {
                connection.password = credentials.password;
                connection.ssh_password = credentials.ssh_password;
                connection.ssh_private_key_password = credentials.ssh_private_key_password;
            }
        }
        Ok(connections)
    }

    /// Delete a connection by ID
    pub async fn delete_connection(&self, connection_id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM connections WHERE id = ?")
                .bind(connection_id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    pub async fn load_legacy_connection_credentials(
        &self,
    ) -> Result<Vec<LegacyConnectionCredentials>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let columns = sqlx::query("PRAGMA table_info(connections)")
                .fetch_all(&pool)
                .await?
                .into_iter()
                .map(|row| row.get::<String, _>("name"))
                .collect::<std::collections::HashSet<_>>();

            let legacy_columns = ["password", "ssh_password", "ssh_private_key_password"];
            if !legacy_columns
                .iter()
                .any(|column| columns.contains(*column))
            {
                return Ok(Vec::new());
            }

            let select_column = |column: &str| {
                if columns.contains(column) {
                    column.to_string()
                } else {
                    format!("NULL AS {column}")
                }
            };
            let query = format!(
                "SELECT id, {}, {}, {} FROM connections",
                select_column("password"),
                select_column("ssh_password"),
                select_column("ssh_private_key_password")
            );
            let rows = sqlx::query(&query).fetch_all(&pool).await?;
            Ok(rows
                .into_iter()
                .map(|row| LegacyConnectionCredentials {
                    connection_id: row.get("id"),
                    password: row.get("password"),
                    ssh_password: row.get("ssh_password"),
                    ssh_private_key_password: row.get("ssh_private_key_password"),
                })
                .collect())
        })
        .await
    }

    pub async fn drop_legacy_connection_credential_columns(&self) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let columns = sqlx::query("PRAGMA table_info(connections)")
                .fetch_all(&pool)
                .await?
                .into_iter()
                .map(|row| row.get::<String, _>("name"))
                .collect::<std::collections::HashSet<_>>();
            let mut transaction = pool.begin().await?;
            sqlx::query("PRAGMA secure_delete = ON")
                .execute(&mut *transaction)
                .await?;
            for column in ["password", "ssh_password", "ssh_private_key_password"] {
                if columns.contains(column) {
                    sqlx::query(&format!("ALTER TABLE connections DROP COLUMN {column}"))
                        .execute(&mut *transaction)
                        .await?;
                }
            }
            transaction.commit().await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_save_and_load_postgres_with_ssh_and_ssl_disabled() {
        test_save_and_load_postgres_with_ssh_and_ssl_disabled_inner().await;
    }

    async fn test_save_and_load_postgres_with_ssh_and_ssl_disabled_inner() {
        let db = AppDatabase::new_in_memory(tokio::runtime::Handle::current())
            .await
            .expect("Failed to create in-memory database");

        let mut connection = ConnectionData::new_postgres_with_ssh(
            "Test PG SSH".to_string(),
            "db.example.com".to_string(),
            5432,
            "mydb".to_string(),
            "admin".to_string(),
            "secret123".to_string(),
            "bastion.example.com".to_string(),
            22,
            "ssh_user".to_string(),
            Some("ssh_pass".to_string()),
            Some("/home/user/.ssh/id_rsa".to_string()),
            Some("key_pass".to_string()),
        );
        connection.ssl_mode = Some("disabled".to_string());
        connection.environment_type = EnvironmentType::Prod;

        let saved_id = db
            .save_connection(&connection)
            .await
            .expect("Failed to save connection");
        assert!(saved_id > 0);

        let loaded = db
            .load_connections()
            .await
            .expect("Failed to load connections");
        assert_eq!(loaded.len(), 1);

        let loaded = &loaded[0];
        assert_eq!(loaded.id, Some(saved_id));
        assert_eq!(loaded.name, "Test PG SSH");
        assert_eq!(loaded.db_type, DatabaseType::PostgreSQL);
        assert_eq!(loaded.host.as_deref(), Some("db.example.com"));
        assert_eq!(loaded.port, Some(5432));
        assert_eq!(loaded.database_name.as_deref(), Some("mydb"));
        assert_eq!(loaded.username.as_deref(), Some("admin"));
        assert_eq!(loaded.password, None);
        assert_eq!(loaded.environment_type, EnvironmentType::Prod);

        // SSH fields
        assert_eq!(loaded.ssh_host.as_deref(), Some("bastion.example.com"));
        assert_eq!(loaded.ssh_port, Some(22));
        assert_eq!(loaded.ssh_user.as_deref(), Some("ssh_user"));
        assert_eq!(loaded.ssh_password, None);
        assert_eq!(
            loaded.ssh_private_key_path.as_deref(),
            Some("/home/user/.ssh/id_rsa")
        );
        assert_eq!(loaded.ssh_private_key_password, None);

        // SSL fields
        assert_eq!(loaded.ssl_mode.as_deref(), Some("disabled"));
        assert_eq!(loaded.ssl_key_path, None);
        assert_eq!(loaded.ssl_cert_path, None);
        assert_eq!(loaded.ssl_ca_cert_path, None);
    }

    #[tokio::test]
    async fn mssql_trust_server_certificate_round_trips() {
        let database = AppDatabase::new_in_memory(tokio::runtime::Handle::current())
            .await
            .expect("Failed to create in-memory database");
        let mut connection = ConnectionData::new_mssql(
            "SQL Server".to_string(),
            "localhost".to_string(),
            1433,
            "master".to_string(),
            "sa".to_string(),
            "password".to_string(),
        );
        connection.trust_server_certificate = true;

        database
            .save_connection(&connection)
            .await
            .expect("Failed to save connection");
        let loaded = database
            .load_connections()
            .await
            .expect("Failed to load connections");

        assert_eq!(loaded.len(), 1);
        assert!(
            loaded
                .first()
                .is_some_and(|connection| connection.trust_server_certificate)
        );
    }

    #[tokio::test]
    async fn legacy_credentials_can_be_read_before_columns_are_dropped() {
        let database = AppDatabase::new_in_memory(tokio::runtime::Handle::current())
            .await
            .expect("Failed to create in-memory database");
        let connection = ConnectionData::new_postgres(
            "Postgres".to_string(),
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "user".to_string(),
            "database password".to_string(),
        );
        let connection_id = database
            .save_connection(&connection)
            .await
            .expect("Failed to save connection");
        let pool = database.pool();
        database
            .run(async move {
                sqlx::query("ALTER TABLE connections ADD COLUMN password TEXT")
                    .execute(&pool)
                    .await?;
                sqlx::query("ALTER TABLE connections ADD COLUMN ssh_password TEXT")
                    .execute(&pool)
                    .await?;
                sqlx::query(
                    "ALTER TABLE connections ADD COLUMN ssh_private_key_password TEXT",
                )
                .execute(&pool)
                .await?;
                sqlx::query(
                    "UPDATE connections SET password = ?, ssh_password = ?, ssh_private_key_password = ? WHERE id = ?",
                )
                .bind("database password")
                .bind("ssh password")
                .bind("key password")
                .bind(connection_id)
                .execute(&pool)
                .await?;
                Ok(())
            })
            .await
            .expect("Failed to create legacy columns");

        let legacy = database
            .load_legacy_connection_credentials()
            .await
            .expect("Failed to read legacy credentials");
        let credentials = legacy.first().expect("legacy credential row");
        assert_eq!(credentials.password.as_deref(), Some("database password"));
        assert_eq!(credentials.ssh_password.as_deref(), Some("ssh password"));
        assert_eq!(
            credentials.ssh_private_key_password.as_deref(),
            Some("key password")
        );

        database
            .drop_legacy_connection_credential_columns()
            .await
            .expect("Failed to drop legacy columns");
        let columns = sqlx::query("PRAGMA table_info(connections)")
            .fetch_all(&database.pool())
            .await
            .expect("Failed to inspect connection columns")
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<std::collections::HashSet<_>>();
        assert!(!columns.contains("password"));
        assert!(!columns.contains("ssh_password"));
        assert!(!columns.contains("ssh_private_key_password"));
    }

    #[test]
    fn connection_debug_output_redacts_credentials() {
        let connection = ConnectionData::new_postgres_with_ssh(
            "Postgres".to_string(),
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "user".to_string(),
            "database-secret".to_string(),
            "bastion".to_string(),
            22,
            "ssh-user".to_string(),
            Some("ssh-secret".to_string()),
            None,
            Some("key-secret".to_string()),
        );

        let debug = format!("{connection:?}");
        assert!(!debug.contains("database-secret"));
        assert!(!debug.contains("ssh-secret"));
        assert!(!debug.contains("key-secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
