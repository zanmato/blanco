use sqlx::Row;

use crate::app_database::{AppDatabase, SnippetData};

impl AppDatabase {
    // Snippets
    pub async fn save_snippet(&self, snippet: &SnippetData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();

        if let Some(id) = snippet.id {
            tracing::info!("UPDATING SNIPPET ID {}", id);
            sqlx::query(
                r#"
                UPDATE snippets
                SET name = ?, content = ?, parent_id = ?, is_group = ?, position = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(&snippet.name)
            .bind(&snippet.content)
            .bind(snippet.parent_id)
            .bind(snippet.is_group as i64)
            .bind(snippet.position)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            let result = sqlx::query(
                r#"
                INSERT INTO snippets (name, content, parent_id, is_group, position, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&snippet.name)
            .bind(&snippet.content)
            .bind(snippet.parent_id)
            .bind(snippet.is_group as i64)
            .bind(snippet.position)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await?;

            Ok(result.last_insert_rowid())
        }
    }

    pub async fn load_snippets(&self) -> Result<Vec<SnippetData>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, content, parent_id, is_group, position, created_at, updated_at
            FROM snippets
            ORDER BY position, name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let snippets = rows
            .into_iter()
            .map(|row| SnippetData {
                id: Some(row.get(0)),
                name: row.get(1),
                content: row.get(2),
                parent_id: row.get(3),
                is_group: row.get::<i64, _>(4) != 0,
                position: row.get(5),
                created_at: row.get(6),
                updated_at: row.get(7),
            })
            .collect();

        Ok(snippets)
    }

    pub async fn get_snippet_by_id(&self, id: i64) -> Result<Option<SnippetData>, sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT id, name, content, parent_id, is_group, position, created_at, updated_at
            FROM snippets
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(SnippetData {
                id: Some(row.get(0)),
                name: row.get(1),
                content: row.get(2),
                parent_id: row.get(3),
                is_group: row.get::<i64, _>(4) != 0,
                position: row.get(5),
                created_at: row.get(6),
                updated_at: row.get(7),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_snippet(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM snippets WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn move_snippet(
        &self,
        id: i64,
        new_parent_id: Option<i64>,
        new_position: i32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE snippets
            SET parent_id = ?, position = ?
            WHERE id = ?
            "#,
        )
        .bind(new_parent_id)
        .bind(new_position)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
