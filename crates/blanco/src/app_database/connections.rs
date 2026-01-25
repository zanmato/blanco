use sqlx::Row;

use crate::app_database::{AppDatabase, ConnectionData, EnvironmentType};

impl AppDatabase {
    pub async fn save_connection(&self, conn: &ConnectionData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();

        if let Some(id) = conn.id {
            // Update existing connection
            sqlx::query(
                r#"
                UPDATE connections
                SET name = ?, db_type = ?, host = ?, port = ?, database_name = ?,
                    username = ?, password = ?, database_path = ?, last_used_at = ?,
                    connection_string = ?, is_active = ?, connection_params = ?, environment_type = ?,
                    ssh_host = ?, ssh_port = ?, ssh_user = ?, ssh_password = ?,
                    ssh_private_key_path = ?, ssh_private_key_password = ?, local_tunnel_port = ?
                WHERE id = ?
                "#,
            )
            .bind(&conn.name)
            .bind(&conn.db_type)
            .bind(&conn.host)
            .bind(conn.port)
            .bind(&conn.database_name)
            .bind(&conn.username)
            .bind(&conn.password)
            .bind(&conn.database_path)
            .bind(now)
            .bind(&conn.connection_string)
            .bind(conn.is_active.map(|b| if b { 1 } else { 0 }))
            .bind(conn.connection_params.as_ref().map(|v| v.to_string()))
            .bind(conn.environment_type.to_i32())
            .bind(&conn.ssh_host)
            .bind(conn.ssh_port)
            .bind(&conn.ssh_user)
            .bind(&conn.ssh_password)
            .bind(&conn.ssh_private_key_path)
            .bind(&conn.ssh_private_key_password)
            .bind(conn.local_tunnel_port)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            // Insert new connection
            let result = sqlx::query(
                r#"
                INSERT INTO connections (name, db_type, host, port, database_name, username, password, database_path, last_used_at, created_at, connection_string, is_active, connection_params, environment_type, ssh_host, ssh_port, ssh_user, ssh_password, ssh_private_key_path, ssh_private_key_password, local_tunnel_port)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&conn.name)
            .bind(&conn.db_type)
            .bind(&conn.host)
            .bind(conn.port)
            .bind(&conn.database_name)
            .bind(&conn.username)
            .bind(&conn.password)
            .bind(&conn.database_path)
            .bind(now)
            .bind(now)
            .bind(&conn.connection_string)
            .bind(conn.is_active.map(|b| if b { 1 } else { 0 }))
            .bind(conn.connection_params.as_ref().map(|v| v.to_string()))
            .bind(conn.environment_type.to_i32())
            .bind(&conn.ssh_host)
            .bind(conn.ssh_port)
            .bind(&conn.ssh_user)
            .bind(&conn.ssh_password)
            .bind(&conn.ssh_private_key_path)
            .bind(&conn.ssh_private_key_password)
            .bind(conn.local_tunnel_port)
            .execute(&self.pool)
            .await?;

            Ok(result.last_insert_rowid())
        }
    }

    /// Load all connections from the database
    pub async fn load_connections(&self) -> Result<Vec<ConnectionData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, db_type, host, port, database_name, username, password, database_path, last_used_at, connection_string, is_active, connection_params, environment_type, ssh_host, ssh_port, ssh_user, ssh_password, ssh_private_key_path, ssh_private_key_password, local_tunnel_port
            FROM connections
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let connections = rows
            .iter()
            .map(|row| {
                // Parse connection_params if present
                let connection_params: Option<serde_json::Value> = row
                    .get::<Option<String>, _>("connection_params")
                    .and_then(|s| serde_json::from_str(&s).ok());

                ConnectionData {
                    id: Some(row.get("id")),
                    name: row.get("name"),
                    db_type: row.get("db_type"),
                    host: row.get("host"),
                    port: row.get("port"),
                    database_name: row.get("database_name"),
                    username: row.get("username"),
                    password: row.get("password"),
                    database_path: row.get("database_path"),
                    last_used_at: row.get("last_used_at"),
                    connection_string: row.get("connection_string"),
                    is_active: row.get::<Option<i32>, _>("is_active").map(|i| i == 1),
                    connection_params,
                    environment_type: EnvironmentType::from_i32(
                        row.get::<i64, _>("environment_type") as i32,
                    ),
                    ssh_host: row.get("ssh_host"),
                    ssh_port: row.get("ssh_port"),
                    ssh_user: row.get("ssh_user"),
                    ssh_password: row.get("ssh_password"),
                    ssh_private_key_path: row.get("ssh_private_key_path"),
                    ssh_private_key_password: row.get("ssh_private_key_password"),
                    local_tunnel_port: row.get("local_tunnel_port"),
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
                   last_used_at, connection_string, is_active, connection_params, environment_type,
                   ssh_host, ssh_port, ssh_user, ssh_password, ssh_private_key_path, ssh_private_key_password, local_tunnel_port
            FROM connections
            WHERE id = ?
            "#,
        )
        .bind(connection_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            let connection_data = ConnectionData {
                id: Some(row.get(0)),
                name: row.get(1),
                db_type: row.get(2),
                host: row.get(3),
                port: row.get(4),
                database_name: row.get(5),
                username: row.get(6),
                password: row.get(7),
                database_path: row.get(8),
                last_used_at: row.get(9),
                connection_string: row.get(10),
                is_active: Some(row.get::<i64, _>(11) != 0),
                connection_params: row.get(12),
                environment_type: EnvironmentType::from_i32(row.get::<i64, _>(13) as i32),
                ssh_host: row.get(14),
                ssh_port: row.get(15),
                ssh_user: row.get(16),
                ssh_password: row.get(17),
                ssh_private_key_path: row.get(18),
                ssh_private_key_password: row.get(19),
                local_tunnel_port: row.get(20),
            };
            Ok(Some(connection_data))
        } else {
            Ok(None)
        }
    }
}
