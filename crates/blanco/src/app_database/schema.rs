use sqlx::sqlite::SqlitePool;
use std::path::PathBuf;

/// Run an `ALTER TABLE ... ADD COLUMN`, tolerating only the "column already
/// exists" case. SQLite reports this as a database error whose message is
/// "duplicate column name: <name>". Every other failure (a locked database, a
/// corrupt file, a typo in the DDL) is propagated.
async fn add_column_if_missing(pool: &SqlitePool, alter_sql: &str) -> Result<(), sqlx::Error> {
    match sqlx::query(alter_sql).execute(pool).await {
        Ok(_) => Ok(()),
        Err(error) if is_duplicate_column_error(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

/// True when `error` is SQLite's "duplicate column name" error, i.e. the column
/// an `ADD COLUMN` tried to add is already present.
fn is_duplicate_column_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db_error) if db_error.message().contains("duplicate column name"))
}

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

    // Incrementally migrate the query_tabs table. Each column is added if it is
    // not already present; a real error (not "column exists") aborts init.
    add_column_if_missing(
        pool,
        "ALTER TABLE query_tabs ADD COLUMN connection_id INTEGER",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE query_tabs ADD COLUMN connection_type TEXT",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE query_tabs ADD COLUMN pg_connection_key TEXT",
    )
    .await?;
    add_column_if_missing(pool, "ALTER TABLE query_tabs ADD COLUMN file_uri TEXT").await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE query_tabs ADD COLUMN database_name TEXT NOT NULL DEFAULT 'default'",
    )
    .await?;
    add_column_if_missing(pool, "ALTER TABLE query_tabs ADD COLUMN schema_name TEXT").await?;
    // Existing profiles predate script tabs, so every stored tab is a query.
    add_column_if_missing(
        pool,
        "ALTER TABLE query_tabs ADD COLUMN tab_kind TEXT NOT NULL DEFAULT 'query'",
    )
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
    .execute(pool)
    .await?;

    add_column_if_missing(
        pool,
        "ALTER TABLE query_history ADD COLUMN connection_id INTEGER",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE query_history ADD COLUMN connection_name TEXT",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE query_history ADD COLUMN database_name TEXT",
    )
    .await?;

    // Index for the history panel's reverse-chronological listing
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_query_history_executed_at
        ON query_history (executed_at DESC)
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

    // Incrementally migrate the connections table (PostgreSQL support, SSH
    // tunneling, SSL, and environment labeling were all added over time).
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN db_type TEXT NOT NULL DEFAULT 'SQLite'",
    )
    .await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN host TEXT").await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN port INTEGER").await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN database_name TEXT",
    )
    .await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN username TEXT").await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN connection_string TEXT",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN is_active INTEGER DEFAULT 1",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN connection_params TEXT",
    )
    .await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN ssh_host TEXT").await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN ssh_port INTEGER").await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN ssh_user TEXT").await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN ssh_private_key_path TEXT",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN local_tunnel_port INTEGER",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN environment_type INTEGER NOT NULL DEFAULT 1",
    )
    .await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN ssl_mode TEXT").await?;
    add_column_if_missing(pool, "ALTER TABLE connections ADD COLUMN ssl_key_path TEXT").await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN ssl_cert_path TEXT",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN ssl_ca_cert_path TEXT",
    )
    .await?;
    add_column_if_missing(
        pool,
        "ALTER TABLE connections ADD COLUMN trust_server_certificate INTEGER NOT NULL DEFAULT 0",
    )
    .await?;

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

    // Snippets predate script tabs, so everything already stored is SQL.
    add_column_if_missing(
        pool,
        "ALTER TABLE snippets ADD COLUMN kind TEXT NOT NULL DEFAULT 'query'",
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn init_schema_is_idempotent_and_tolerates_existing_columns() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");

        // Running twice must succeed: the second pass hits every ADD COLUMN as a
        // duplicate and must swallow only that, not error out.
        init_schema(&pool).await.expect("first init");
        init_schema(&pool).await.expect("second init idempotent");
    }

    #[tokio::test]
    async fn add_column_if_missing_propagates_real_errors() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");

        // Altering a table that does not exist is a genuine error and must not
        // be swallowed like a duplicate-column case.
        let result =
            add_column_if_missing(&pool, "ALTER TABLE does_not_exist ADD COLUMN x TEXT").await;
        assert!(result.is_err(), "missing-table error must propagate");
    }
}
