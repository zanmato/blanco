use sqlx::Row;

use crate::app_database::{AppDatabase, ConnectionData, EnvironmentType};
use database::DatabaseType;

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

    /// Get connection details by ID
    #[allow(dead_code)]
    pub async fn get_connection_by_id(
        &self,
        connection_id: i64,
    ) -> Result<Option<ConnectionData>, sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT id, name, db_type, host, port, database_name, username, password, database_path,
                   last_used_at, is_active, environment_type,
                   ssh_host, ssh_port, ssh_user, ssh_password, ssh_private_key_path, ssh_private_key_password,
                   ssl_mode, ssl_key_path, ssl_cert_path, ssl_ca_cert_path
            FROM connections
            WHERE id = ?
            "#,
        )
        .bind(connection_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            let db_type_str: String = row.get("db_type");
            let db_type =
                DatabaseType::from_db_type_str(&db_type_str).unwrap_or(DatabaseType::PostgreSQL);

            let connection_data = ConnectionData {
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
                    row.get::<i64, _>("environment_type") as i32
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
            };
            Ok(Some(connection_data))
        } else {
            Ok(None)
        }
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
