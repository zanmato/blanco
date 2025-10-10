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
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

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
                database_path TEXT NOT NULL,
                last_used_at INTEGER,
                created_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

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
                SET title = ?, content = ?, position = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(&tab.title)
            .bind(&tab.content)
            .bind(tab.position)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            // Insert new tab
            let result = sqlx::query(
                r#"
                INSERT INTO query_tabs (title, content, position, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(&tab.title)
            .bind(&tab.content)
            .bind(tab.position)
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
            SELECT id, title, content, position, created_at, updated_at
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

    pub async fn get_query_history(&self, limit: i64) -> Result<Vec<QueryHistoryData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, query_text, executed_at, duration_ms, rows_affected, row_count, success, error_message
            FROM query_history
            ORDER BY executed_at DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let history = rows
            .into_iter()
            .map(|row| QueryHistoryData {
                id: Some(row.get::<i64, _>(0)),
                query_text: row.get(1),
                executed_at: row.get(2),
                duration_ms: row.get(3),
                rows_affected: row.get(4),
                row_count: row.get(5),
                success: row.get::<i64, _>(6) == 1,
                error_message: row.get(7),
            })
            .collect();

        Ok(history)
    }

    // Connections
    pub async fn save_connection(&self, conn: &ConnectionData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();

        if let Some(id) = conn.id {
            // Update existing connection
            sqlx::query(
                r#"
                UPDATE connections
                SET name = ?, database_path = ?, last_used_at = ?
                WHERE id = ?
                "#,
            )
            .bind(&conn.name)
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
                INSERT INTO connections (name, database_path, last_used_at, created_at)
                VALUES (?, ?, ?, ?)
                "#,
            )
            .bind(&conn.name)
            .bind(&conn.database_path)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await?;

            Ok(result.last_insert_rowid())
        }
    }

    pub async fn get_connections(&self) -> Result<Vec<ConnectionData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, database_path, last_used_at
            FROM connections
            ORDER BY last_used_at DESC NULLS LAST
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let connections = rows
            .into_iter()
            .map(|row| ConnectionData {
                id: Some(row.get::<i64, _>(0)),
                name: row.get(1),
                database_path: row.get(2),
                last_used_at: row.get(3),
            })
            .collect();

        Ok(connections)
    }

    pub async fn delete_connection(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM connections WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct QueryTabData {
    pub id: Option<i64>,
    pub title: String,
    pub content: String,
    pub position: i32,
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
    pub database_path: String,
    pub last_used_at: Option<i64>,
}
