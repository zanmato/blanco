use sqlx::Row;

use crate::{AppDatabase, QueryHistoryData};

impl AppDatabase {
    /// Append a finished execution to the history log. The `id` field of the
    /// incoming record is ignored; the row id is assigned by SQLite and
    /// returned.
    pub async fn record_query_history(&self, entry: &QueryHistoryData) -> Result<i64, sqlx::Error> {
        let pool = self.pool();
        let entry = entry.clone();
        self.run(async move {
            let result = sqlx::query(
                r#"
                INSERT INTO query_history (
                    query_text, executed_at, duration_ms, rows_affected,
                    row_count, success, error_message, connection_id,
                    connection_name, database_name
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&entry.query_text)
            .bind(entry.executed_at)
            .bind(entry.duration_ms)
            .bind(entry.rows_affected)
            .bind(entry.row_count)
            .bind(entry.success as i64)
            .bind(&entry.error_message)
            .bind(entry.connection_id)
            .bind(&entry.connection_name)
            .bind(&entry.database_name)
            .execute(&pool)
            .await?;

            Ok(result.last_insert_rowid())
        })
        .await
    }

    /// Load the most recent history entries, newest first. When `search` is
    /// non-empty, only entries whose query text contains it (case-insensitive)
    /// are returned.
    pub async fn load_query_history(
        &self,
        limit: i64,
        search: Option<String>,
    ) -> Result<Vec<QueryHistoryData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let search = search
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            let rows = if let Some(search) = search {
                let pattern = format!("%{}%", search.replace('%', "\\%").replace('_', "\\_"));
                sqlx::query(
                    r#"
                    SELECT id, query_text, executed_at, duration_ms, rows_affected,
                           row_count, success, error_message, connection_id,
                           connection_name, database_name
                    FROM query_history
                    WHERE query_text LIKE ? ESCAPE '\'
                    ORDER BY executed_at DESC, id DESC
                    LIMIT ?
                    "#,
                )
                .bind(pattern)
                .bind(limit)
                .fetch_all(&pool)
                .await?
            } else {
                sqlx::query(
                    r#"
                    SELECT id, query_text, executed_at, duration_ms, rows_affected,
                           row_count, success, error_message, connection_id,
                           connection_name, database_name
                    FROM query_history
                    ORDER BY executed_at DESC, id DESC
                    LIMIT ?
                    "#,
                )
                .bind(limit)
                .fetch_all(&pool)
                .await?
            };

            let history = rows
                .into_iter()
                .map(|row| QueryHistoryData {
                    id: Some(row.get(0)),
                    query_text: row.get(1),
                    executed_at: row.get(2),
                    duration_ms: row.get(3),
                    rows_affected: row.get(4),
                    row_count: row.get(5),
                    success: row.get::<i64, _>(6) != 0,
                    error_message: row.get(7),
                    connection_id: row.get(8),
                    connection_name: row.get(9),
                    database_name: row.get(10),
                })
                .collect();

            Ok(history)
        })
        .await
    }

    pub async fn delete_query_history_entry(&self, id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM query_history WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    /// Delete history entries beyond the newest `max` rows. A `max` of 0 leaves
    /// the table untouched (treated as "unlimited") to avoid wiping history on a
    /// misconfigured setting.
    pub async fn prune_query_history(&self, max: i64) -> Result<(), sqlx::Error> {
        if max <= 0 {
            return Ok(());
        }
        let pool = self.pool();
        self.run(async move {
            sqlx::query(
                r#"
                DELETE FROM query_history
                WHERE id NOT IN (
                    SELECT id FROM query_history
                    ORDER BY executed_at DESC, id DESC
                    LIMIT ?
                )
                "#,
            )
            .bind(max)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .await
    }

    pub async fn clear_query_history(&self) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM query_history")
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }
}
