use sqlx::Row;

use crate::app_database::AppDatabase;

impl AppDatabase {
    pub async fn save_setting(
        &self,
        key: &str,
        value: &str,
        is_secret: bool,
    ) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        let pool = self.pool();
        let key = key.to_string();
        let value = value.to_string();
        self.run(async move {
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO settings (key, value, is_secret, created_at, updated_at)
                VALUES (?, ?, ?, COALESCE((SELECT created_at FROM settings WHERE key = ?), ?), ?)
                "#,
            )
            .bind(&key)
            .bind(&value)
            .bind(is_secret as i64)
            .bind(&key)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await?;

            Ok(())
        })
        .await
    }

    pub async fn load_all_settings(&self) -> Result<Vec<(String, String)>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                r#"
                SELECT key, value FROM settings WHERE is_secret = 0
                "#,
            )
            .fetch_all(&pool)
            .await?;

            Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
        })
        .await
    }
}
