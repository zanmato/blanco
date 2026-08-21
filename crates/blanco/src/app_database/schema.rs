use sqlx::sqlite::SqlitePool;
use std::path::PathBuf;

/// Schema version stored in SQLite's `PRAGMA user_version`. Bump it and append
/// a step to [`MIGRATIONS`] for every schema change.
pub const SCHEMA_VERSION: i64 = 1;

/// The complete schema for a fresh database, at [`SCHEMA_VERSION`]. Keep it in
/// sync with the migrations: a database created from this must be identical
/// to one migrated step by step.
const BASELINE: &[&str] = &[
    r#"
    CREATE TABLE connections (
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
        is_active INTEGER DEFAULT 1,
        ssh_host TEXT,
        ssh_port INTEGER,
        ssh_user TEXT,
        ssh_private_key_path TEXT,
        environment_type INTEGER NOT NULL DEFAULT 1,
        ssl_mode TEXT,
        ssl_key_path TEXT,
        ssl_cert_path TEXT,
        ssl_ca_cert_path TEXT,
        trust_server_certificate INTEGER NOT NULL DEFAULT 0,
        read_only INTEGER NOT NULL DEFAULT 0
    )
    "#,
    r#"
    CREATE TABLE query_tabs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        title TEXT NOT NULL,
        content TEXT NOT NULL,
        position INTEGER NOT NULL,
        connection_id INTEGER,
        connection_type TEXT,
        database_name TEXT NOT NULL DEFAULT 'default',
        schema_name TEXT,
        tab_kind TEXT NOT NULL DEFAULT 'query',
        last_run_at INTEGER,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        FOREIGN KEY (connection_id) REFERENCES connections(id) ON DELETE SET NULL
    )
    "#,
    r#"
    CREATE TABLE query_history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        query_text TEXT NOT NULL,
        executed_at INTEGER NOT NULL,
        duration_ms INTEGER,
        rows_affected INTEGER,
        row_count INTEGER,
        success INTEGER NOT NULL,
        error_message TEXT,
        connection_id INTEGER,
        connection_name TEXT,
        database_name TEXT
    )
    "#,
    "CREATE INDEX idx_query_history_executed_at ON query_history (executed_at DESC)",
    r#"
    CREATE TABLE snippets (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        content TEXT NOT NULL,
        kind TEXT NOT NULL DEFAULT 'query',
        parent_id INTEGER,
        is_group INTEGER NOT NULL DEFAULT 0,
        position INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        FOREIGN KEY (parent_id) REFERENCES snippets(id) ON DELETE CASCADE
    )
    "#,
    r#"
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        is_secret INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    )
    "#,
];

/// Migration steps. `MIGRATIONS[n]` upgrades a database from version `n` to
/// `n + 1`. Step 0 brings the unversioned, pre-`user_version` layout (which
/// accumulated columns through `ADD COLUMN IF MISSING` calls) up to the
/// baseline; its column list must stay in sync with [`BASELINE`].
const MIGRATIONS: &[fn(&SqlitePool) -> futures::future::BoxFuture<'_, Result<(), sqlx::Error>>] =
    &[|pool| Box::pin(migrate_unversioned_to_v1(pool))];

/// Columns that may be missing from an unversioned database, per table, in the
/// form accepted by `ALTER TABLE ... ADD COLUMN`.
const UNVERSIONED_COLUMNS: &[(&str, &[&str])] = &[
    (
        "connections",
        &[
            "ssh_host TEXT",
            "ssh_port INTEGER",
            "ssh_user TEXT",
            "ssh_private_key_path TEXT",
            "environment_type INTEGER NOT NULL DEFAULT 1",
            "ssl_mode TEXT",
            "ssl_key_path TEXT",
            "ssl_cert_path TEXT",
            "ssl_ca_cert_path TEXT",
            "trust_server_certificate INTEGER NOT NULL DEFAULT 0",
            "read_only INTEGER NOT NULL DEFAULT 0",
        ],
    ),
    (
        "query_tabs",
        &[
            "connection_id INTEGER",
            "connection_type TEXT",
            "database_name TEXT NOT NULL DEFAULT 'default'",
            "schema_name TEXT",
            "tab_kind TEXT NOT NULL DEFAULT 'query'",
            "last_run_at INTEGER",
        ],
    ),
    (
        "query_history",
        &[
            "connection_id INTEGER",
            "connection_name TEXT",
            "database_name TEXT",
        ],
    ),
    ("snippets", &["kind TEXT NOT NULL DEFAULT 'query'"]),
];

async fn migrate_unversioned_to_v1(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    for (table, columns) in UNVERSIONED_COLUMNS {
        let existing = existing_columns(pool, table).await?;
        for column in *columns {
            let name = column.split_whitespace().next().unwrap_or_default();
            if !existing.iter().any(|existing| existing == name) {
                sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN {column}"))
                    .execute(pool)
                    .await?;
            }
        }
    }
    Ok(())
}

async fn existing_columns(pool: &SqlitePool, table: &str) -> Result<Vec<String>, sqlx::Error> {
    use sqlx::Row as _;
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect())
}

async fn user_version(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    use sqlx::Row as _;
    let row = sqlx::query("PRAGMA user_version").fetch_one(pool).await?;
    row.try_get::<i64, _>(0)
}

async fn set_user_version(pool: &SqlitePool, version: i64) -> Result<(), sqlx::Error> {
    sqlx::query(&format!("PRAGMA user_version = {version}"))
        .execute(pool)
        .await?;
    Ok(())
}

async fn has_tables(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    use sqlx::Row as _;
    let row = sqlx::query(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'connections'",
    )
    .fetch_one(pool)
    .await?;
    Ok(row.try_get::<i64, _>(0)? > 0)
}

/// Create or upgrade the application schema. A fresh database gets the
/// baseline directly; an existing one is walked through [`MIGRATIONS`] from
/// its recorded version. Refuses to open a database written by a newer build.
pub async fn init_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let version = user_version(pool).await?;
    if version > SCHEMA_VERSION {
        return Err(sqlx::Error::Configuration(
            format!(
                "app database is at schema version {version}, newer than this build supports ({SCHEMA_VERSION})"
            )
            .into(),
        ));
    }

    if version == 0 && !has_tables(pool).await? {
        for statement in BASELINE {
            sqlx::query(statement).execute(pool).await?;
        }
        set_user_version(pool, SCHEMA_VERSION).await?;
        return Ok(());
    }

    for (index, migration) in MIGRATIONS.iter().enumerate() {
        let from = index as i64;
        if from < version {
            continue;
        }
        migration(pool).await?;
        set_user_version(pool, from + 1).await?;
        tracing::info!("Migrated app database schema to version {}", from + 1);
    }
    Ok(())
}

/// Get the path to the application database file
pub fn app_db_path() -> PathBuf {
    let mut path = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("blanco");
    if let Err(error) = std::fs::create_dir_all(&path) {
        tracing::error!(
            "Failed to create app data directory {}: {error}",
            path.display()
        );
    }
    path.push("blanco.db");
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row as _;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool")
    }

    async fn columns(pool: &SqlitePool, table: &str) -> Vec<String> {
        let mut columns = existing_columns(pool, table).await.expect("table info");
        columns.sort();
        columns
    }

    #[tokio::test]
    async fn fresh_database_gets_baseline_and_version() {
        let pool = pool().await;
        init_schema(&pool).await.expect("init");
        assert_eq!(user_version(&pool).await.expect("version"), SCHEMA_VERSION);
        init_schema(&pool).await.expect("second init is a no-op");
    }

    #[tokio::test]
    async fn unversioned_database_is_migrated_to_the_baseline_shape() {
        let pool = pool().await;
        // The oldest layout: tables exist with only their original columns.
        for statement in [
            "CREATE TABLE connections (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE, db_type TEXT NOT NULL, host TEXT, port INTEGER, database_name TEXT, username TEXT, database_path TEXT, last_used_at INTEGER, created_at INTEGER NOT NULL, is_active INTEGER DEFAULT 1)",
            "CREATE TABLE query_tabs (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, content TEXT NOT NULL, position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
            "CREATE TABLE query_history (id INTEGER PRIMARY KEY AUTOINCREMENT, query_text TEXT NOT NULL, executed_at INTEGER NOT NULL, duration_ms INTEGER, rows_affected INTEGER, row_count INTEGER, success INTEGER NOT NULL, error_message TEXT)",
            "CREATE TABLE snippets (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, content TEXT NOT NULL, parent_id INTEGER, is_group INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, is_secret INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
            "INSERT INTO connections (name, db_type, created_at) VALUES ('old', 'SQLite', 0)",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("legacy ddl");
        }

        init_schema(&pool).await.expect("migrate");
        assert_eq!(user_version(&pool).await.expect("version"), SCHEMA_VERSION);

        let fresh = self::pool().await;
        init_schema(&fresh).await.expect("fresh init");
        for table in [
            "connections",
            "query_tabs",
            "query_history",
            "snippets",
            "settings",
        ] {
            assert_eq!(
                columns(&pool, table).await,
                columns(&fresh, table).await,
                "{table} columns differ between migrated and fresh databases"
            );
        }

        let row = sqlx::query("SELECT read_only FROM connections WHERE name = 'old'")
            .fetch_one(&pool)
            .await
            .expect("migrated row");
        assert_eq!(row.get::<i64, _>(0), 0);
    }

    #[tokio::test]
    async fn newer_database_is_refused() {
        let pool = pool().await;
        set_user_version(&pool, SCHEMA_VERSION + 1)
            .await
            .expect("stamp");
        assert!(init_schema(&pool).await.is_err());
    }
}
