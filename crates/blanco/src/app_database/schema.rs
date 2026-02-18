use sqlx::sqlite::SqlitePool;
use std::path::PathBuf;

/// Initialize the database schema with all required tables
pub async fn init_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
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
    .execute(pool)
    .await?;

    // Migrate existing query_tabs table if needed (add connection_id column)
    sqlx::query(
        r#"
        ALTER TABLE query_tabs ADD COLUMN connection_id INTEGER
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add connection_type column for distinguishing SQLite vs PostgreSQL
    sqlx::query(
        r#"
        ALTER TABLE query_tabs ADD COLUMN connection_type TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add pg_connection_key column for PostgreSQL connection identification
    sqlx::query(
        r#"
        ALTER TABLE query_tabs ADD COLUMN pg_connection_key TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add file_uri column for file-based storage
    sqlx::query(
        r#"
        ALTER TABLE query_tabs ADD COLUMN file_uri TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add database_name column for database-aware query tabs
    sqlx::query(
        r#"
        ALTER TABLE query_tabs ADD COLUMN database_name TEXT NOT NULL DEFAULT 'default'
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add schema_name column for schema context
    sqlx::query(
        r#"
        ALTER TABLE query_tabs ADD COLUMN schema_name TEXT
        "#,
    )
    .execute(pool)
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
    .execute(pool)
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
            created_at INTEGER NOT NULL,
            connection_string TEXT,
            is_active INTEGER DEFAULT 1,
            connection_params TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Migrate existing connections table if needed (add new columns for PostgreSQL support)
    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN db_type TEXT NOT NULL DEFAULT 'SQLite'
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN host TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN port INTEGER
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN database_name TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN username TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN password TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add new columns for unified connection management
    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN connection_string TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN is_active INTEGER DEFAULT 1
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN connection_params TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add SSH tunnel support columns
    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssh_host TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssh_port INTEGER
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssh_user TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssh_password TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssh_private_key_path TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssh_private_key_password TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN local_tunnel_port INTEGER
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add environment_type column for connection environment labeling
    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN environment_type INTEGER NOT NULL DEFAULT 1
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Add SSL support columns
    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssl_mode TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssl_key_path TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssl_cert_path TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    sqlx::query(
        r#"
        ALTER TABLE connections ADD COLUMN ssl_ca_cert_path TEXT
        "#,
    )
    .execute(pool)
    .await
    .ok(); // Ignore error if column already exists

    // Note: database_path NOT NULL constraint has been manually fixed
    // The database schema now allows NULL database_path for PostgreSQL connections

    // Snippets table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS snippets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            content TEXT NOT NULL,
            parent_id INTEGER,
            is_group INTEGER NOT NULL DEFAULT 0,
            position INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (parent_id) REFERENCES snippets(id) ON DELETE CASCADE
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Settings table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            is_secret INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Get the path to the application database file
pub fn app_db_path() -> PathBuf {
    let mut path = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("blanco");
    std::fs::create_dir_all(&path).ok();
    path.push("blanco.db");
    path
}
