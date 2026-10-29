//! Dashboards and their panels. A panel is a query on a connection of its
//! own with the way its result is shown, which the app keeps as JSON in
//! `view`. Panels are laid out in `position` order on a grid of
//! [`DASHBOARD_COLUMNS`] columns.

use sqlx::Row;

use crate::AppDatabase;
use crate::types::EnvironmentType;

/// The columns of the grid a panel's width counts in.
pub const DASHBOARD_COLUMNS: u8 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardData {
    pub id: i64,
    pub name: String,
    /// The connection a panel added on the dashboard starts on.
    pub default_connection_id: Option<i64>,
    /// How often the dashboard refreshes itself, `None` for only on request.
    pub refresh_seconds: Option<u32>,
    /// The time range's key, which the app reads.
    pub time_range: String,
    /// The variables, as JSON the app reads.
    pub variables: String,
}

/// What a panel knows of its connection, read with the panel so it can be
/// named and run without the connection list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelConnection {
    pub id: i64,
    pub name: String,
    pub db_type: String,
    pub environment_type: EnvironmentType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardPanelData {
    /// The row id, 0 for a panel not saved yet.
    pub id: i64,
    pub dashboard_id: i64,
    pub title: String,
    /// `None` once the panel's connection was deleted.
    pub connection: Option<PanelConnection>,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub query: String,
    /// How the result is shown, as JSON the app reads.
    pub view: String,
    pub position: i64,
    /// Columns of the grid, 1 to [`DASHBOARD_COLUMNS`].
    pub width: u8,
    /// Rows of the grid.
    pub height: u8,
}

impl AppDatabase {
    pub async fn load_dashboards(&self) -> Result<Vec<DashboardData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                "SELECT id, name, default_connection_id, refresh_seconds, time_range, variables
                 FROM dashboards ORDER BY name COLLATE NOCASE, id",
            )
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(|row| DashboardData {
                    id: row.get(0),
                    name: row.get(1),
                    default_connection_id: row.get(2),
                    refresh_seconds: row
                        .get::<Option<i64>, _>(3)
                        .and_then(|seconds| u32::try_from(seconds).ok()),
                    time_range: row.get(4),
                    variables: row.get(5),
                })
                .collect())
        })
        .await
    }

    pub async fn load_dashboard(&self, id: i64) -> Result<Option<DashboardData>, sqlx::Error> {
        Ok(self
            .load_dashboards()
            .await?
            .into_iter()
            .find(|dashboard| dashboard.id == id))
    }

    /// Create an empty dashboard and return its id.
    pub async fn create_dashboard(
        &self,
        name: &str,
        default_connection_id: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let pool = self.pool();
        let name = name.to_string();
        self.run(async move {
            let now = chrono::Utc::now().timestamp();
            let result = sqlx::query(
                "INSERT INTO dashboards (name, default_connection_id, created_at, updated_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&name)
            .bind(default_connection_id)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await?;
            Ok(result.last_insert_rowid())
        })
        .await
    }

    /// Save a dashboard's name, default connection and refresh interval.
    pub async fn update_dashboard(&self, dashboard: &DashboardData) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        let dashboard = dashboard.clone();
        self.run(async move {
            sqlx::query(
                "UPDATE dashboards
                 SET name = ?, default_connection_id = ?, refresh_seconds = ?, time_range = ?,
                     variables = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&dashboard.name)
            .bind(dashboard.default_connection_id)
            .bind(dashboard.refresh_seconds.map(i64::from))
            .bind(&dashboard.time_range)
            .bind(&dashboard.variables)
            .bind(chrono::Utc::now().timestamp())
            .bind(dashboard.id)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .await
    }

    pub async fn rename_dashboard(&self, id: i64, name: &str) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        let name = name.to_string();
        self.run(async move {
            sqlx::query("UPDATE dashboards SET name = ?, updated_at = ? WHERE id = ?")
                .bind(&name)
                .bind(chrono::Utc::now().timestamp())
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    /// Delete a dashboard with its panels.
    pub async fn delete_dashboard(&self, id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM dashboards WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    /// A dashboard's panels in layout order.
    pub async fn load_dashboard_panels(
        &self,
        dashboard_id: i64,
    ) -> Result<Vec<DashboardPanelData>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                "SELECT p.id, p.title, p.connection_id, c.name, c.db_type, c.environment_type,
                        p.database_name, p.schema_name, p.query, p.view, p.position,
                        p.width, p.height
                 FROM dashboard_panels p
                 LEFT JOIN connections c ON c.id = p.connection_id
                 WHERE p.dashboard_id = ?
                 ORDER BY p.position, p.id",
            )
            .bind(dashboard_id)
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(|row| {
                    let connection = match (
                        row.get::<Option<i64>, _>(2),
                        row.get::<Option<String>, _>(3),
                        row.get::<Option<String>, _>(4),
                    ) {
                        (Some(id), Some(name), Some(db_type)) => Some(PanelConnection {
                            id,
                            name,
                            db_type,
                            environment_type: EnvironmentType::from_i32(
                                row.get::<Option<i32>, _>(5).unwrap_or(1),
                            ),
                        }),
                        _ => None,
                    };
                    DashboardPanelData {
                        id: row.get(0),
                        dashboard_id,
                        title: row.get(1),
                        connection,
                        database_name: row.get(6),
                        schema_name: row.get(7),
                        query: row.get(8),
                        view: row.get(9),
                        position: row.get(10),
                        width: clamp_width(row.get(11)),
                        height: clamp_height(row.get(12)),
                    }
                })
                .collect())
        })
        .await
    }

    /// Add a panel after the dashboard's last one and return its id.
    pub async fn insert_dashboard_panel(
        &self,
        panel: &DashboardPanelData,
    ) -> Result<i64, sqlx::Error> {
        let pool = self.pool();
        let panel = panel.clone();
        self.run(async move {
            let now = chrono::Utc::now().timestamp();
            let result = sqlx::query(
                "INSERT INTO dashboard_panels
                 (dashboard_id, title, connection_id, database_name, schema_name, query, view,
                  position, width, height, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?,
                  (SELECT COALESCE(MAX(position) + 1, 0) FROM dashboard_panels WHERE dashboard_id = ?),
                  ?, ?, ?, ?)",
            )
            .bind(panel.dashboard_id)
            .bind(&panel.title)
            .bind(panel.connection.as_ref().map(|connection| connection.id))
            .bind(&panel.database_name)
            .bind(&panel.schema_name)
            .bind(&panel.query)
            .bind(&panel.view)
            .bind(panel.dashboard_id)
            .bind(i64::from(clamp_width(i64::from(panel.width))))
            .bind(i64::from(clamp_height(i64::from(panel.height))))
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await?;
            sqlx::query("UPDATE dashboards SET updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(panel.dashboard_id)
                .execute(&pool)
                .await?;
            Ok(result.last_insert_rowid())
        })
        .await
    }

    /// Save everything about a panel but its place in the layout order.
    pub async fn update_dashboard_panel(
        &self,
        panel: &DashboardPanelData,
    ) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        let panel = panel.clone();
        self.run(async move {
            sqlx::query(
                "UPDATE dashboard_panels
                 SET title = ?, connection_id = ?, database_name = ?, schema_name = ?, query = ?,
                     view = ?, width = ?, height = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&panel.title)
            .bind(panel.connection.as_ref().map(|connection| connection.id))
            .bind(&panel.database_name)
            .bind(&panel.schema_name)
            .bind(&panel.query)
            .bind(&panel.view)
            .bind(i64::from(clamp_width(i64::from(panel.width))))
            .bind(i64::from(clamp_height(i64::from(panel.height))))
            .bind(chrono::Utc::now().timestamp())
            .bind(panel.id)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .await
    }

    /// Write what a query tab saves to its panel: where it runs, its query
    /// and its view. The title and size stay as the dashboard has them, since
    /// they may have changed while the tab was open.
    pub async fn update_dashboard_panel_source(
        &self,
        panel: &DashboardPanelData,
    ) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        let panel = panel.clone();
        self.run(async move {
            sqlx::query(
                "UPDATE dashboard_panels
                 SET connection_id = ?, database_name = ?, schema_name = ?, query = ?,
                     view = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(panel.connection.as_ref().map(|connection| connection.id))
            .bind(&panel.database_name)
            .bind(&panel.schema_name)
            .bind(&panel.query)
            .bind(&panel.view)
            .bind(chrono::Utc::now().timestamp())
            .bind(panel.id)
            .execute(&pool)
            .await?;
            Ok(())
        })
        .await
    }

    pub async fn delete_dashboard_panel(&self, id: i64) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            sqlx::query("DELETE FROM dashboard_panels WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await?;
            Ok(())
        })
        .await
    }

    /// The dashboards a window has open, in tab order, with whether each was
    /// the active tab.
    pub async fn load_window_dashboards(
        &self,
        window_id: i64,
    ) -> Result<Vec<(i64, bool)>, sqlx::Error> {
        let pool = self.pool();
        self.run(async move {
            let rows = sqlx::query(
                "SELECT dashboard_id, active FROM window_dashboards
                 WHERE window_id = ? ORDER BY position",
            )
            .bind(window_id)
            .fetch_all(&pool)
            .await?;
            Ok(rows
                .into_iter()
                .map(|row| (row.get(0), row.get::<i64, _>(1) != 0))
                .collect())
        })
        .await
    }

    /// Remember the dashboards a window has open, replacing what it had.
    pub async fn save_window_dashboards(
        &self,
        window_id: i64,
        dashboards: &[(i64, bool)],
    ) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        let dashboards = dashboards.to_vec();
        self.run(async move {
            let mut transaction = pool.begin().await?;
            sqlx::query("DELETE FROM window_dashboards WHERE window_id = ?")
                .bind(window_id)
                .execute(&mut *transaction)
                .await?;
            for (position, (dashboard_id, active)) in dashboards.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO window_dashboards (window_id, dashboard_id, position, active)
                     SELECT ?, id, ?, ? FROM dashboards WHERE id = ?",
                )
                .bind(window_id)
                .bind(position as i64)
                .bind(i64::from(*active))
                .bind(dashboard_id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await
        })
        .await
    }

    /// Lay a dashboard's panels out in the order of `panel_ids`.
    pub async fn reorder_dashboard_panels(
        &self,
        dashboard_id: i64,
        panel_ids: &[i64],
    ) -> Result<(), sqlx::Error> {
        let pool = self.pool();
        let panel_ids = panel_ids.to_vec();
        self.run(async move {
            let mut transaction = pool.begin().await?;
            for (position, id) in panel_ids.iter().enumerate() {
                sqlx::query(
                    "UPDATE dashboard_panels SET position = ? WHERE id = ? AND dashboard_id = ?",
                )
                .bind(position as i64)
                .bind(id)
                .bind(dashboard_id)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await
        })
        .await
    }
}

fn clamp_width(width: i64) -> u8 {
    width.clamp(1, i64::from(DASHBOARD_COLUMNS)) as u8
}

fn clamp_height(height: i64) -> u8 {
    height.clamp(1, 12) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConnectionData;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime")
    }

    fn panel(
        dashboard_id: i64,
        connection: Option<PanelConnection>,
        title: &str,
    ) -> DashboardPanelData {
        DashboardPanelData {
            id: 0,
            dashboard_id,
            title: title.to_string(),
            connection,
            database_name: "main".to_string(),
            schema_name: None,
            query: "SELECT 1".to_string(),
            view: "{}".to_string(),
            position: 0,
            width: 6,
            height: 4,
        }
    }

    #[test]
    fn panels_keep_their_order_and_outlive_their_connection() {
        let runtime = runtime();
        let database = runtime
            .block_on(AppDatabase::new_in_memory(runtime.handle().clone()))
            .expect("database");
        runtime.block_on(async {
            let connection_id = database
                .save_connection(&ConnectionData::new_sqlite(
                    "Demo".to_string(),
                    "/tmp/demo.db".to_string(),
                ))
                .await
                .expect("connection");
            let dashboard_id = database
                .create_dashboard("Sales", Some(connection_id))
                .await
                .expect("dashboard");
            let connection = PanelConnection {
                id: connection_id,
                name: "Demo".to_string(),
                db_type: "SQLite".to_string(),
                environment_type: EnvironmentType::DEV,
            };
            let first = database
                .insert_dashboard_panel(&panel(dashboard_id, Some(connection.clone()), "First"))
                .await
                .expect("first");
            let second = database
                .insert_dashboard_panel(&panel(dashboard_id, Some(connection.clone()), "Second"))
                .await
                .expect("second");

            let titles = |panels: Vec<DashboardPanelData>| {
                panels
                    .into_iter()
                    .map(|panel| panel.title)
                    .collect::<Vec<_>>()
            };
            let loaded = database
                .load_dashboard_panels(dashboard_id)
                .await
                .expect("panels");
            assert_eq!(loaded[0].connection.as_ref(), Some(&connection));
            assert_eq!(titles(loaded), ["First", "Second"]);

            database
                .reorder_dashboard_panels(dashboard_id, &[second, first])
                .await
                .expect("reorder");
            let loaded = database
                .load_dashboard_panels(dashboard_id)
                .await
                .expect("panels");
            assert_eq!(titles(loaded), ["Second", "First"]);

            database
                .delete_connection(connection_id)
                .await
                .expect("delete connection");
            let loaded = database
                .load_dashboard_panels(dashboard_id)
                .await
                .expect("panels");
            assert_eq!(loaded.len(), 2);
            assert!(loaded.iter().all(|panel| panel.connection.is_none()));
            let dashboard = database
                .load_dashboard(dashboard_id)
                .await
                .expect("load")
                .expect("dashboard");
            assert_eq!(dashboard.default_connection_id, None);

            let window_id = database.create_window(None, None).await.expect("window");
            database
                .save_window_dashboards(window_id, &[(dashboard_id, true), (9999, false)])
                .await
                .expect("open dashboards");
            assert_eq!(
                database
                    .load_window_dashboards(window_id)
                    .await
                    .expect("open dashboards"),
                vec![(dashboard_id, true)],
                "a dashboard that does not exist is not kept"
            );

            database
                .delete_dashboard(dashboard_id)
                .await
                .expect("delete dashboard");
            assert!(
                database
                    .load_window_dashboards(window_id)
                    .await
                    .expect("open dashboards")
                    .is_empty()
            );
            assert!(
                database
                    .load_dashboard_panels(dashboard_id)
                    .await
                    .expect("panels")
                    .is_empty()
            );
        });
    }
}
