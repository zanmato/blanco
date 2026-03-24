use sqlx::Row;

use crate::app_database::{AppDatabase, ConnectionData, EnvironmentType};
use database::DatabaseType;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_load_postgres_with_ssh_and_ssl_disabled() {
        smol::block_on(async {
            test_save_and_load_postgres_with_ssh_and_ssl_disabled_inner().await;
        });
    }

    async fn test_save_and_load_postgres_with_ssh_and_ssl_disabled_inner() {
        let db = AppDatabase::new_in_memory().await.expect("Failed to create in-memory database");

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

        let saved_id = db.save_connection(&connection).await.expect("Failed to save connection");
        assert!(saved_id > 0);

        let loaded = db.load_connections().await.expect("Failed to load connections");
        assert_eq!(loaded.len(), 1);

        let loaded = &loaded[0];
        assert_eq!(loaded.id, Some(saved_id));
        assert_eq!(loaded.name, "Test PG SSH");
        assert_eq!(loaded.db_type, DatabaseType::PostgreSQL);
        assert_eq!(loaded.host.as_deref(), Some("db.example.com"));
        assert_eq!(loaded.port, Some(5432));
        assert_eq!(loaded.database_name.as_deref(), Some("mydb"));
        assert_eq!(loaded.username.as_deref(), Some("admin"));
        assert_eq!(loaded.password.as_deref(), Some("secret123"));
        assert_eq!(loaded.environment_type, EnvironmentType::Prod);

        // SSH fields
        assert_eq!(loaded.ssh_host.as_deref(), Some("bastion.example.com"));
        assert_eq!(loaded.ssh_port, Some(22));
        assert_eq!(loaded.ssh_user.as_deref(), Some("ssh_user"));
        assert_eq!(loaded.ssh_password.as_deref(), Some("ssh_pass"));
        assert_eq!(loaded.ssh_private_key_path.as_deref(), Some("/home/user/.ssh/id_rsa"));
        assert_eq!(loaded.ssh_private_key_password.as_deref(), Some("key_pass"));

        // SSL fields
        assert_eq!(loaded.ssl_mode.as_deref(), Some("disabled"));
        assert_eq!(loaded.ssl_key_path, None);
        assert_eq!(loaded.ssl_cert_path, None);
        assert_eq!(loaded.ssl_ca_cert_path, None);
    }
}

impl AppDatabase {
    pub async fn save_connection(&self, conn: &ConnectionData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        let db_type_str = conn.db_type.as_str();

        if let Some(id) = conn.id {
            // Update existing connection
            sqlx::query(
                r#"
                UPDATE connections
                SET name = ?, db_type = ?, host = ?, port = ?, database_name = ?,
                    username = ?, password = ?, database_path = ?, last_used_at = ?,
                    is_active = ?, environment_type = ?,
                    ssh_host = ?, ssh_port = ?, ssh_user = ?, ssh_password = ?,
                    ssh_private_key_path = ?, ssh_private_key_password = ?,
                    ssl_mode = ?, ssl_key_path = ?, ssl_cert_path = ?, ssl_ca_cert_path = ?
                WHERE id = ?
                "#,
            )
            .bind(&conn.name)
            .bind(db_type_str)
            .bind(&conn.host)
            .bind(conn.port)
            .bind(&conn.database_name)
            .bind(&conn.username)
            .bind(&conn.password)
            .bind(&conn.database_path)
            .bind(now)
            .bind(conn.is_active.map(|b| if b { 1 } else { 0 }))
            .bind(conn.environment_type.to_i32())
            .bind(&conn.ssh_host)
            .bind(conn.ssh_port)
            .bind(&conn.ssh_user)
            .bind(&conn.ssh_password)
            .bind(&conn.ssh_private_key_path)
            .bind(&conn.ssh_private_key_password)
            .bind(&conn.ssl_mode)
            .bind(&conn.ssl_key_path)
            .bind(&conn.ssl_cert_path)
            .bind(&conn.ssl_ca_cert_path)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            // Insert new connection
            let result = sqlx::query(
                r#"
                INSERT INTO connections (name, db_type, host, port, database_name, username, password, database_path, last_used_at, created_at, is_active, environment_type, ssh_host, ssh_port, ssh_user, ssh_password, ssh_private_key_path, ssh_private_key_password, ssl_mode, ssl_key_path, ssl_cert_path, ssl_ca_cert_path)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&conn.name)
            .bind(db_type_str)
            .bind(&conn.host)
            .bind(conn.port)
            .bind(&conn.database_name)
            .bind(&conn.username)
            .bind(&conn.password)
            .bind(&conn.database_path)
            .bind(now)
            .bind(now)
            .bind(conn.is_active.map(|b| if b { 1 } else { 0 }))
            .bind(conn.environment_type.to_i32())
            .bind(&conn.ssh_host)
            .bind(conn.ssh_port)
            .bind(&conn.ssh_user)
            .bind(&conn.ssh_password)
            .bind(&conn.ssh_private_key_path)
            .bind(&conn.ssh_private_key_password)
            .bind(&conn.ssl_mode)
            .bind(&conn.ssl_key_path)
            .bind(&conn.ssl_cert_path)
            .bind(&conn.ssl_ca_cert_path)
            .execute(&self.pool)
            .await?;

            Ok(result.last_insert_rowid())
        }
    }

    /// Load all connections from the database
    pub async fn load_connections(&self) -> Result<Vec<ConnectionData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, db_type, host, port, database_name, username, password, database_path, last_used_at, is_active, environment_type, ssh_host, ssh_port, ssh_user, ssh_password, ssh_private_key_path, ssh_private_key_password, ssl_mode, ssl_key_path, ssl_cert_path, ssl_ca_cert_path
            FROM connections
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let connections = rows
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
                    password: row.get("password"),
                    database_path: row.get("database_path"),
                    last_used_at: row.get("last_used_at"),
                    is_active: row.get::<Option<i32>, _>("is_active").map(|i| i == 1),
                    environment_type: EnvironmentType::from_i32(
                        row.get::<i64, _>("environment_type") as i32,
                    ),
                    ssh_host: row.get("ssh_host"),
                    ssh_port: row.get("ssh_port"),
                    ssh_user: row.get("ssh_user"),
                    ssh_password: row.get("ssh_password"),
                    ssh_private_key_path: row.get("ssh_private_key_path"),
                    ssh_private_key_password: row.get("ssh_private_key_password"),
                    ssl_mode: row.get("ssl_mode"),
                    ssl_key_path: row.get("ssl_key_path"),
                    ssl_cert_path: row.get("ssl_cert_path"),
                    ssl_ca_cert_path: row.get("ssl_ca_cert_path"),
                }
            })
            .collect();

        Ok(connections)
    }


    /// Delete a connection by ID
    pub async fn delete_connection(&self, connection_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM connections WHERE id = ?")
            .bind(connection_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
