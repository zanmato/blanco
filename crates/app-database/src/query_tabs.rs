use sqlx::Row;

use crate::{AppDatabase, EditorKind, EnvironmentType, QueryTabData};

impl AppDatabase {
    pub async fn save_query_tab(&self, tab: &QueryTabData) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        let pool = self.pool();
        let tab = tab.clone();
        self.run(async move {
            if let Some(id) = tab.id {
                // Update existing tab
                sqlx::query(
                    r#"
                    UPDATE query_tabs
                    SET title = ?, content = ?, position = ?, connection_id = ?, connection_type = ?, database_name = ?, schema_name = ?, tab_kind = ?, updated_at = ?
                    WHERE id = ?
                    "#,
                )
                .bind(&tab.title)
                .bind(&tab.content)
                .bind(tab.position)
                .bind(tab.connection_id)
                .bind(&tab.connection_type)
                .bind(&tab.database_name)
                .bind(&tab.schema_name)
                .bind(tab.tab_kind.as_str())
                .bind(now)
                .bind(id)
                .execute(&pool)
                .await?;
                Ok(id)
            } else {
                // Insert new tab
                let result = sqlx::query(
                    r#"
                    INSERT INTO query_tabs (title, content, position, connection_id, connection_type, database_name, schema_name, tab_kind, created_at, updated_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&tab.title)
                .bind(&tab.content)
                .bind(tab.position)
                .bind(tab.connection_id)
                .bind(&tab.connection_type)
                .bind(&tab.database_name)
                .bind(&tab.schema_name)
                .bind(tab.tab_kind.as_str())
                .bind(now)
                .bind(now)
                .execute(&pool)
                .await?;

                Ok(result.last_insert_rowid())
            }
        })
        .await
    }

    pub async fn load_query_tabs(&self) -> Result<Vec<QueryTabData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                r#"
                SELECT qt.id, qt.title, qt.content, qt.position, qt.connection_id, c.db_type, c.name, qt.database_name, qt.schema_name, c.environment_type, qt.tab_kind, qt.last_run_at
                FROM query_tabs qt
                INNER JOIN connections c ON c.id = qt.connection_id
                ORDER BY qt.position ASC
                "#,
            )
            .fetch_all(&pool)
            .await?;

            let tabs = rows
                .into_iter()
                .map(|row| {
                    let id = row.get::<i64, _>(0);
                    let title: String = row.get(1);

                    let title = if title.trim().is_empty() {
                        format!("Query {}", id)
                    } else {
                        title
                    };

                    QueryTabData {
                        id: Some(id),
                        title,
                        content: row.get(2),
                        position: row.get(3),
                        connection_id: row.get(4),
                        connection_type: row.get(5),
                        connection_name: Some(row.get(6)),
                        database_name: Some(row.get(7)),
                        schema_name: row.get(8),
                        environment_type: Some(EnvironmentType::from_i32(
                            row.get::<i64, _>(9) as i32,
                        )),
                        tab_kind: EditorKind::from_stored(&row.get::<String, _>(10)),
                        last_run_at: row.get(11),
                    }
                })
                .collect();

            Ok(tabs)
        })
        .await
    }

    pub async fn load_query_tab_by_id(&self, id: i64) -> Result<Option<QueryTabData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let row = sqlx::query(
                r#"
                SELECT qt.id, qt.title, qt.content, qt.position, qt.connection_id, c.db_type, c.name, qt.database_name, qt.schema_name, qt.tab_kind, qt.last_run_at
                FROM query_tabs qt
                INNER JOIN connections c ON c.id = qt.connection_id
                WHERE qt.id = ?
                "#,
            )
            .bind(id)
            .fetch_optional(&pool)
            .await?;

            if let Some(row) = row {
                let tab_id = row.get::<i64, _>(0);
                let title: String = row.get(1);

                let title = if title.trim().is_empty() {
                    format!("Query {}", tab_id)
                } else {
                    title
                };

                Ok(Some(QueryTabData {
                    id: Some(tab_id),
                    title,
                    content: row.get(2),
                    position: row.get(3),
                    connection_id: row.get(4),
                    connection_type: row.get(5),
                    connection_name: Some(row.get(6)),
                    database_name: Some(row.get(7)),
                    schema_name: row.get(8),
                    environment_type: None,
                    tab_kind: EditorKind::from_stored(&row.get::<String, _>(9)),
                    last_run_at: row.get(10),
                }))
            } else {
                Ok(None)
            }
        })
        .await
    }

    pub async fn touch_query_tab_last_run(
        &self,
        id: i64,
        timestamp: i64,
    ) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("UPDATE query_tabs SET last_run_at = ? WHERE id = ?")
                .bind(timestamp)
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    pub async fn delete_query_tab(&self, id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM query_tabs WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }
}
