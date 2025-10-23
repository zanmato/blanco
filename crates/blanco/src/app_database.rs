use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::{ConnectOptions, Row};
use std::path::PathBuf;
use std::str::FromStr;

/// Application database for persisting query tabs, history, and connections
pub struct AppDatabase {
    pool: SqlitePool,
}

impl AppDatabase {
    pub async fn new() -> Result<Self, sqlx::Error> {
        let db_path = Self::app_db_path();

        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
            .create_if_missing(true)
            .disable_statement_logging();

        let pool = SqlitePool::connect_with(options).await?;

        let mut db = Self { pool };
        db.init_schema().await?;
        Ok(db)
    }

    fn app_db_path() -> PathBuf {
        let mut path = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("blanco");
        std::fs::create_dir_all(&path).ok();
        path.push("blanco.db");
        path
    }

    async fn init_schema(&mut self) -> Result<(), sqlx::Error> {
        // Query tabs table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS query_tabs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                position INTEGER NOT NULL,
                connection_id INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE SET NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Migrate existing query_tabs table if needed (add connection_id column)
        sqlx::query(
            r#"
            ALTER TABLE query_tabs ADD COLUMN connection_id INTEGER
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        // Add connection_type column for distinguishing SQLite vs PostgreSQL
        sqlx::query(
            r#"
            ALTER TABLE query_tabs ADD COLUMN connection_type TEXT
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        // Add pg_connection_key column for PostgreSQL connection identification
        sqlx::query(
            r#"
            ALTER TABLE query_tabs ADD COLUMN pg_connection_key TEXT
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        // Add file_uri column for file-based storage
        sqlx::query(
            r#"
            ALTER TABLE query_tabs ADD COLUMN file_uri TEXT
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        // Query history table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS query_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                query_text TEXT NOT NULL,
                executed_at INTEGER NOT NULL,
                duration_ms INTEGER,
                rows_affected INTEGER,
                row_count INTEGER,
                success INTEGER NOT NULL,
                error_message TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Connections table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS connections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                db_type TEXT NOT NULL,
                host TEXT,
                port INTEGER,
                database_name TEXT,
                username TEXT,
                password TEXT,
                database_path TEXT,
                last_used_at INTEGER,
                created_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Migrate existing connections table if needed (add new columns for PostgreSQL support)
        sqlx::query(
            r#"
            ALTER TABLE connections ADD COLUMN db_type TEXT NOT NULL DEFAULT 'SQLite'
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        sqlx::query(
            r#"
            ALTER TABLE connections ADD COLUMN host TEXT
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        sqlx::query(
            r#"
            ALTER TABLE connections ADD COLUMN port INTEGER
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        sqlx::query(
            r#"
            ALTER TABLE connections ADD COLUMN database_name TEXT
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        sqlx::query(
            r#"
            ALTER TABLE connections ADD COLUMN username TEXT
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        sqlx::query(
            r#"
            ALTER TABLE connections ADD COLUMN password TEXT
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        sqlx::query(
            r#"
            ALTER TABLE connections ADD COLUMN created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // Ignore error if column already exists

        Ok(())
    }

    // Query Tabs
    pub async fn save_query_tab(&self, tab: &QueryTabData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();

        if let Some(id) = tab.id {
            // Update existing tab
            sqlx::query(
                r#"
                UPDATE query_tabs
                SET title = ?, content = ?, position = ?, connection_id = ?, connection_type = ?, pg_connection_key = ?, file_uri = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(&tab.title)
            .bind(&tab.content)
            .bind(tab.position)
            .bind(tab.connection_id)
            .bind(&tab.connection_type)
            .bind(&tab.pg_connection_key)
            .bind(&tab.file_uri)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            // Insert new tab
            let result = sqlx::query(
                r#"
                INSERT INTO query_tabs (title, content, position, connection_id, connection_type, pg_connection_key, file_uri, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&tab.title)
            .bind(&tab.content)
            .bind(tab.position)
            .bind(tab.connection_id)
            .bind(&tab.connection_type)
            .bind(&tab.pg_connection_key)
            .bind(&tab.file_uri)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await?;

            Ok(result.last_insert_rowid())
        }
    }

    pub async fn load_query_tabs(&self) -> Result<Vec<QueryTabData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, title, content, position, connection_id, connection_type, pg_connection_key, file_uri
            FROM query_tabs
            ORDER BY position ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let tabs = rows
            .into_iter()
            .map(|row| QueryTabData {
                id: Some(row.get::<i64, _>(0)),
                title: row.get(1),
                content: row.get(2),
                position: row.get(3),
                connection_id: row.get(4),
                connection_type: row.get(5),
                pg_connection_key: row.get(6),
                file_uri: row.get(7),
            })
            .collect();

        Ok(tabs)
    }

    pub async fn delete_query_tab(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM query_tabs WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Find tabs that need migration (no file_uri set)
    pub async fn find_tabs_needing_migration(&self) -> Result<Vec<QueryTabData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, title, content, position, connection_id, connection_type, pg_connection_key, file_uri
            FROM query_tabs
            WHERE file_uri IS NULL
            ORDER BY position ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let tabs = rows
            .into_iter()
            .map(|row| QueryTabData {
                id: Some(row.get::<i64, _>(0)),
                title: row.get(1),
                content: row.get(2),
                position: row.get(3),
                connection_id: row.get(4),
                connection_type: row.get(5),
                pg_connection_key: row.get(6),
                file_uri: row.get(7),
            })
            .collect();

        Ok(tabs)
    }

    /// Update file_uri for a specific tab
    pub async fn update_tab_file_uri(&self, id: i64, file_uri: &str) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().timestamp();

        sqlx::query(
            r#"
            UPDATE query_tabs
            SET file_uri = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(file_uri)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // Query History
    pub async fn save_query_history(&self, history: &QueryHistoryData) -> Result<i64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO query_history
            (query_text, executed_at, duration_ms, rows_affected, row_count, success, error_message)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&history.query_text)
        .bind(history.executed_at)
        .bind(history.duration_ms)
        .bind(history.rows_affected)
        .bind(history.row_count)
        .bind(if history.success { 1 } else { 0 })
        .bind(&history.error_message)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    // Connections
    pub async fn save_connection(&self, conn: &ConnectionData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();

        if let Some(id) = conn.id {
            // Update existing connection
            sqlx::query(
                r#"
                UPDATE connections
                SET name = ?, db_type = ?, host = ?, port = ?, database_name = ?,
                    username = ?, password = ?, database_path = ?, last_used_at = ?
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
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            // Insert new connection
            let result = sqlx::query(
                r#"
                INSERT INTO connections (name, db_type, host, port, database_name, username, password, database_path, last_used_at, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            .execute(&self.pool)
            .await?;

            Ok(result.last_insert_rowid())
        }
    }

    /// Load all connections from the database
    pub async fn load_connections(&self) -> Result<Vec<ConnectionData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, db_type, host, port, database_name, username, password, database_path, last_used_at
            FROM connections
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let connections = rows
            .iter()
            .map(|row| ConnectionData {
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
            })
            .collect();

        Ok(connections)
    }
}

#[derive(Debug, Clone)]
pub struct QueryTabData {
    pub id: Option<i64>,
    pub title: String,
    pub content: String,
    pub position: i32,
    pub connection_id: Option<i64>,
    pub connection_type: Option<String>,
    pub pg_connection_key: Option<String>,
    pub file_uri: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueryHistoryData {
    pub id: Option<i64>,
    pub query_text: String,
    pub executed_at: i64,
    pub duration_ms: Option<i64>,
    pub rows_affected: Option<i64>,
    pub row_count: Option<i64>,
    pub success: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConnectionData {
    pub id: Option<i64>,
    pub name: String,
    pub db_type: String,
    pub host: Option<String>,
    pub port: Option<i32>,
    pub database_name: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database_path: Option<String>,
    pub last_used_at: Option<i64>,
}

impl ConnectionData {
    pub fn new_sqlite(name: String, database_path: String) -> Self {
        Self {
            id: None,
            name,
            db_type: "SQLite".to_string(),
            host: None,
            port: None,
            database_name: None,
            username: None,
            password: None,
            database_path: Some(database_path),
            last_used_at: None,
        }
    }

    pub fn new_postgres(
        name: String,
        host: String,
        port: i32,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        Self {
            id: None,
            name,
            db_type: "PostgreSQL".to_string(),
            host: Some(host),
            port: Some(port),
            database_name: Some(database),
            username: Some(username),
            password: Some(password),
            database_path: None,
            last_used_at: None,
        }
    }
}
