use sqlx::Row;

use crate::{AppDatabase, WindowData, WindowGeometry};

impl AppDatabase {
    /// The windows open when Blanco last quit, the most recently active last
    /// so it opens on top.
    pub async fn load_windows(&self) -> Result<Vec<WindowData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                r#"
                SELECT id, x, y, width, height, maximized, display_uuid, active_tab_id
                FROM windows
                ORDER BY last_active_at ASC, id ASC
                "#,
            )
            .fetch_all(&pool)
            .await?;

            Ok(rows
                .into_iter()
                .map(|row| {
                    let geometry =
                        match (row.get::<Option<f64>, _>(3), row.get::<Option<f64>, _>(4)) {
                            (Some(width), Some(height)) => Some(WindowGeometry {
                                x: row.get::<Option<f64>, _>(1).unwrap_or_default() as f32,
                                y: row.get::<Option<f64>, _>(2).unwrap_or_default() as f32,
                                width: width as f32,
                                height: height as f32,
                                maximized: row.get::<i64, _>(5) != 0,
                            }),
                            _ => None,
                        };
                    WindowData {
                        id: row.get(0),
                        geometry,
                        display_uuid: row.get(6),
                        active_tab_id: row.get(7),
                    }
                })
                .collect())
        })
        .await
    }

    /// Record a newly opened window and return its id.
    pub async fn create_window(
        &self,
        geometry: Option<WindowGeometry>,
        display_uuid: Option<String>,
    ) -> Result<i64, sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        // In milliseconds, as `touch_window_active` writes it: a new window
        // must not sort as older than every window activated before it.
        let last_active_at = chrono::Utc::now().timestamp_millis();
        let pool = self.pool();
        self.run(async move {
            let result = sqlx::query(
                r#"
                INSERT INTO windows (x, y, width, height, maximized, display_uuid, last_active_at, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(geometry.map(|geometry| geometry.x))
            .bind(geometry.map(|geometry| geometry.y))
            .bind(geometry.map(|geometry| geometry.width))
            .bind(geometry.map(|geometry| geometry.height))
            .bind(geometry.is_some_and(|geometry| geometry.maximized))
            .bind(display_uuid)
            .bind(last_active_at)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await?;
            Ok(result.last_insert_rowid())
        })
        .await
    }

    pub async fn save_window_geometry(
        &self,
        id: i64,
        geometry: WindowGeometry,
        display_uuid: Option<String>,
    ) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        let pool = self.pool();
        self.run(async move {
            sqlx::query(
                r#"
                UPDATE windows
                SET x = ?, y = ?, width = ?, height = ?, maximized = ?, display_uuid = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(geometry.x)
            .bind(geometry.y)
            .bind(geometry.width)
            .bind(geometry.height)
            .bind(geometry.maximized)
            .bind(display_uuid)
            .bind(now)
            .bind(id)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .await
    }

    pub async fn set_window_active_tab(&self, id: i64, tab_id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("UPDATE windows SET active_tab_id = ? WHERE id = ?")
                .bind(tab_id)
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    /// Note that the window came to the front, so the next start opens it on
    /// top of the others.
    pub async fn touch_window_active(&self, id: i64) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().timestamp_millis();
        let pool = self.pool();
        self.run(async move {
            sqlx::query("UPDATE windows SET last_active_at = ? WHERE id = ?")
                .bind(now)
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    /// Forget a closed window. Its tabs go with it.
    pub async fn delete_window(&self, id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM windows WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }
}
