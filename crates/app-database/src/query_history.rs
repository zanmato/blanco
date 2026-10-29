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
                    connection_name, database_name, execution_us, fetch_us
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            .bind(entry.execution_us)
            .bind(entry.fetch_us)
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
        self.load_history_rows(limit, search, None).await
    }

    /// Like [`Self::load_query_history`], with `query_text` holding at most
    /// the first `preview_chars` characters of each query. A script that
    /// inserted thousands of rows is hundreds of kilobytes, which a list of
    /// entries never shows; [`Self::query_history_text`] reads the whole.
    pub async fn load_query_history_previews(
        &self,
        limit: i64,
        search: Option<String>,
        preview_chars: i64,
    ) -> Result<Vec<QueryHistoryData>, sqlx::Error> {
        self.load_history_rows(limit, search, Some(preview_chars))
            .await
    }

    /// The whole query text of a history entry, `None` once it is deleted.
    pub async fn query_history_text(&self, id: i64) -> Result<Option<String>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query_scalar("SELECT query_text FROM query_history WHERE id = ?")
                .bind(id)
                .fetch_optional(&pool)
                .await
        })
        .await
    }

    async fn load_history_rows(
        &self,
        limit: i64,
        search: Option<String>,
        text_chars: Option<i64>,
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
                    SELECT id, substr(query_text, 1, coalesce(?, length(query_text))), executed_at, duration_ms, rows_affected,
                           row_count, success, error_message, connection_id,
                           connection_name, database_name, execution_us, fetch_us
                    FROM query_history
                    WHERE query_text LIKE ? ESCAPE '\'
                    ORDER BY executed_at DESC, id DESC
                    LIMIT ?
                    "#,
                )
                .bind(text_chars)
                .bind(pattern)
                .bind(limit)
                .fetch_all(&pool)
                .await?
            } else {
                sqlx::query(
                    r#"
                    SELECT id, substr(query_text, 1, coalesce(?, length(query_text))), executed_at, duration_ms, rows_affected,
                           row_count, success, error_message, connection_id,
                           connection_name, database_name, execution_us, fetch_us
                    FROM query_history
                    ORDER BY executed_at DESC, id DESC
                    LIMIT ?
                    "#,
                )
                .bind(text_chars)
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
                    execution_us: row.get(11),
                    fetch_us: row.get(12),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(query_text: &str) -> QueryHistoryData {
        QueryHistoryData {
            id: None,
            query_text: query_text.to_string(),
            executed_at: 1,
            duration_ms: None,
            execution_us: None,
            fetch_us: None,
            rows_affected: None,
            row_count: None,
            success: true,
            error_message: None,
            connection_id: None,
            connection_name: None,
            database_name: None,
        }
    }

    #[tokio::test]
    async fn previews_hold_the_start_and_the_text_is_read_whole() {
        let db = AppDatabase::new_in_memory(tokio::runtime::Handle::current())
            .await
            .expect("app database");
        let rows: Vec<String> = (0..2_633).map(|ix| format!("({ix}, 'name')")).collect();
        let query = format!("INSERT INTO t VALUES {};", rows.join(", "));
        let id = db
            .record_query_history(&entry(&query))
            .await
            .expect("record");

        let previews = db
            .load_query_history_previews(10, Some("(2632,".to_string()), 150)
            .await
            .expect("previews");
        assert_eq!(previews.len(), 1, "the search reads past the preview");
        assert_eq!(previews[0].query_text, query[..150]);

        let whole = db.load_query_history(10, None).await.expect("history");
        assert_eq!(whole[0].query_text, query);
        assert_eq!(db.query_history_text(id).await.expect("text"), Some(query));
    }
}
