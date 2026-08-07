use sqlx::Row;

use crate::app_database::{AppDatabase, EditorKind, SnippetData};

impl AppDatabase {
    pub async fn save_snippet(&self, snippet: &SnippetData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        let pool = self.pool();
        let snippet = snippet.clone();
        self.run(async move {
            if let Some(id) = snippet.id {
                sqlx::query(
                    r#"
                    UPDATE snippets
                    SET name = ?, content = ?, kind = ?, parent_id = ?, is_group = ?, position = ?, updated_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&snippet.name)
                .bind(&snippet.content)
                .bind(snippet.kind.as_str())
                .bind(snippet.parent_id)
                .bind(snippet.is_group as i64)
                .bind(snippet.position)
                .bind(now)
                .bind(id)
                .execute(&pool)
                .await?;
                Ok(id)
            } else {
                let result = sqlx::query(
                    r#"
                    INSERT INTO snippets (name, content, kind, parent_id, is_group, position, created_at, updated_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&snippet.name)
                .bind(&snippet.content)
                .bind(snippet.kind.as_str())
                .bind(snippet.parent_id)
                .bind(snippet.is_group as i64)
                .bind(snippet.position)
                .bind(now)
                .bind(now)
                .execute(&pool)
                .await?;

                Ok(result.last_insert_rowid())
            }
        })
        .await
    }

    pub async fn load_snippets(&self) -> Result<Vec<SnippetData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                r#"
                SELECT id, name, content, parent_id, is_group, position, kind
                FROM snippets
                ORDER BY position, name
                "#,
            )
            .fetch_all(&pool)
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
                    kind: EditorKind::from_stored(&row.get::<String, _>(6)),
                })
                .collect();

            Ok(snippets)
        })
        .await
    }

    pub async fn get_snippet_by_id(&self, id: i64) -> Result<Option<SnippetData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let row = sqlx::query(
                r#"
                SELECT id, name, content, parent_id, is_group, position, kind
                FROM snippets
                WHERE id = ?
                "#,
            )
            .bind(id)
            .fetch_optional(&pool)
            .await?;

            if let Some(row) = row {
                Ok(Some(SnippetData {
                    id: Some(row.get(0)),
                    name: row.get(1),
                    content: row.get(2),
                    parent_id: row.get(3),
                    is_group: row.get::<i64, _>(4) != 0,
                    position: row.get(5),
                    kind: EditorKind::from_stored(&row.get::<String, _>(6)),
                }))
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn delete_snippet(&self, id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM snippets WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    /// Reparent and renumber a set of snippets in one transaction.
    ///
    /// A reorder renumbers every sibling in the affected group, so the updates
    /// have to land together: a partial write would leave duplicate or gapped
    /// positions and the tree would come back in a different order.
    pub async fn reorder_snippets(
        &self,
        updates: Vec<(i64, Option<i64>, i32)>,
    ) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let mut transaction = pool.begin().await?;
            for (id, parent_id, position) in updates {
                sqlx::query(
                    r#"
                    UPDATE snippets
                    SET parent_id = ?, position = ?
                    WHERE id = ?
                    "#,
                )
                .bind(parent_id)
                .bind(position)
                .bind(id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn in_memory_db() -> AppDatabase {
        AppDatabase::new_in_memory(tokio::runtime::Handle::current())
            .await
            .expect("failed to create in-memory database")
    }

    fn snippet(name: &str, content: &str, kind: EditorKind) -> SnippetData {
        SnippetData {
            id: None,
            name: name.to_string(),
            content: content.to_string(),
            kind,
            parent_id: None,
            is_group: false,
            position: 0,
        }
    }

    #[tokio::test]
    async fn script_snippets_round_trip_their_kind() {
        let db = in_memory_db().await;

        let query_id = db
            .save_snippet(&snippet("counts", "SELECT 1", EditorKind::Query))
            .await
            .expect("failed to save query snippet");
        let script_id = db
            .save_snippet(&snippet(
                "drop templates",
                "db.execute('DROP DATABASE x');",
                EditorKind::Script,
            ))
            .await
            .expect("failed to save script snippet");

        let loaded = db
            .get_snippet_by_id(script_id)
            .await
            .expect("load failed")
            .expect("script snippet should exist");
        assert_eq!(loaded.kind, EditorKind::Script);

        let all = db.load_snippets().await.expect("load failed");
        let kind_of = |id: i64| {
            all.iter()
                .find(|snippet| snippet.id == Some(id))
                .map(|snippet| snippet.kind)
        };
        assert_eq!(kind_of(query_id), Some(EditorKind::Query));
        assert_eq!(kind_of(script_id), Some(EditorKind::Script));
    }

    #[tokio::test]
    async fn reordering_persists_the_new_parent_and_order() {
        let db = in_memory_db().await;

        let mut ids = Vec::new();
        for name in ["first", "second", "third"] {
            ids.push(
                db.save_snippet(&snippet(name, "SELECT 1", EditorKind::Query))
                    .await
                    .expect("failed to save snippet"),
            );
        }
        let group_id = {
            let mut group = snippet("group", "", EditorKind::Query);
            group.is_group = true;
            db.save_snippet(&group).await.expect("failed to save group")
        };

        // "third" moves into the group, and the remaining roots are renumbered.
        db.reorder_snippets(vec![(ids[2], Some(group_id), 0)])
            .await
            .expect("failed to reorder");
        db.reorder_snippets(vec![(ids[1], None, 0), (ids[0], None, 1)])
            .await
            .expect("failed to reorder");

        let loaded = db.load_snippets().await.expect("load failed");
        let by_id = |id: i64| {
            loaded
                .iter()
                .find(|snippet| snippet.id == Some(id))
                .expect("snippet should exist")
        };

        assert_eq!(by_id(ids[2]).parent_id, Some(group_id));
        assert_eq!(by_id(ids[1]).position, 0);
        assert_eq!(by_id(ids[0]).position, 1);

        let root_order: Vec<&str> = loaded
            .iter()
            .filter(|snippet| snippet.parent_id.is_none() && !snippet.is_group)
            .map(|snippet| snippet.name.as_str())
            .collect();
        assert_eq!(root_order, vec!["second", "first"]);
    }

    #[tokio::test]
    async fn changing_a_snippets_kind_is_persisted() {
        let db = in_memory_db().await;

        let id = db
            .save_snippet(&snippet("later a script", "SELECT 1", EditorKind::Query))
            .await
            .expect("failed to save snippet");

        let mut updated = snippet(
            "later a script",
            "db.query('SELECT 1');",
            EditorKind::Script,
        );
        updated.id = Some(id);
        db.save_snippet(&updated).await.expect("failed to update");

        let loaded = db
            .get_snippet_by_id(id)
            .await
            .expect("load failed")
            .expect("snippet should exist");
        assert_eq!(loaded.kind, EditorKind::Script);
        assert_eq!(loaded.content, "db.query('SELECT 1');");
    }
}
