use app_database::{AppDatabase, ConnectionData, DashboardPanelData, PanelConnection};
use blanco_core::DatabaseService as _;
use database::{ConnectionConfig, DatabaseService};
use gpui::{Entity, TestAppContext, VisualTestContext};

use super::panel::{PanelView, StatSpec};
use super::{DashboardTab, TabCreationParams};
use crate::editor::TabType;
use crate::results_panel::{ChartKind, ChartSpec};
use crate::test_harness::{TestHarness, run_query, set_editor_text, wait_for_query};
use blanco_core::ConnectionContext;

const SEED: &str = "
CREATE TABLE sales (
  id INTEGER PRIMARY KEY,
  region TEXT NOT NULL,
  amount REAL NOT NULL,
  sold_at TEXT NOT NULL
);
INSERT INTO sales (region, amount, sold_at) VALUES
  ('North', 120, datetime('now', '-1 hours')),
  ('North', 80, datetime('now', '-3 days')),
  ('South', 250, datetime('now', '-10 days')),
  ('East', 50, datetime('now', '-400 days'));
";

/// A saved SQLite connection the panels can name, with the demo table.
fn demo_connection(cx: &mut VisualTestContext) -> PanelConnection {
    let (database, service, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            DatabaseService::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let path = tempfile::tempdir()
        .expect("temp dir")
        .keep()
        .join("sales.db")
        .to_string_lossy()
        .into_owned();
    handle.block_on(async {
        let id = database
            .save_connection(&ConnectionData::new_sqlite(
                "Sales".to_string(),
                path.clone(),
            ))
            .await
            .expect("connection");
        service
            .add_connection_config(ConnectionConfig::new_sqlite(id, "Sales".to_string(), path))
            .await;
        service
            .execute_script(id, Some("main"), SEED)
            .await
            .expect("seed");
        service.disconnect(id, None).await.expect("disconnect");
        PanelConnection {
            id,
            name: "Sales".to_string(),
            db_type: "SQLite".to_string(),
            environment_type: app_database::EnvironmentType::DEV,
        }
    })
}

fn panel(
    dashboard_id: i64,
    connection: &PanelConnection,
    title: &str,
    query: &str,
    view: PanelView,
) -> DashboardPanelData {
    DashboardPanelData {
        id: 0,
        dashboard_id,
        title: title.to_string(),
        connection: Some(connection.clone()),
        database_name: "main".to_string(),
        schema_name: None,
        query: query.to_string(),
        view: view.to_json(),
        position: 0,
        width: 6,
        height: 4,
    }
}

async fn wait_for_refresh(tab: &Entity<DashboardTab>, cx: &mut VisualTestContext) {
    for _ in 0..200 {
        cx.run_until_parked();
        let done = tab.read_with(cx, |tab, cx| {
            !tab.loading
                && !tab.refreshing
                && !tab.panels.is_empty()
                && tab.panels.iter().all(|panel| !panel.read(cx).is_loading())
        });
        if done {
            return;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    panic!("The dashboard did not refresh in time");
}

#[gpui::test]
async fn test_dashboard_runs_its_panels_read_only(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let connection = demo_connection(&mut cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let dashboard_id = handle.block_on(async {
        let dashboard_id = database
            .create_dashboard("Sales", Some(connection.id))
            .await
            .expect("dashboard");
        for data in [
            panel(
                dashboard_id,
                &connection,
                "All sales",
                "SELECT region, amount FROM sales ORDER BY id",
                PanelView::default(),
            ),
            panel(
                dashboard_id,
                &connection,
                "Revenue",
                "SELECT SUM(amount) AS revenue FROM sales",
                PanelView::Stat(StatSpec::default()),
            ),
            panel(
                dashboard_id,
                &connection,
                "By region",
                "SELECT region, SUM(amount) AS total FROM sales GROUP BY region",
                PanelView::Chart(ChartSpec {
                    axis: Some("region".to_string()),
                    value: Some("total".to_string()),
                    ..ChartSpec::new(ChartKind::Bar)
                }),
            ),
            panel(
                dashboard_id,
                &connection,
                "Cleanup",
                "DELETE FROM sales",
                PanelView::default(),
            ),
            panel(
                dashboard_id,
                &connection,
                "Broken",
                "SELECT * FROM no_such_table",
                PanelView::default(),
            ),
        ] {
            database.insert_dashboard_panel(&data).await.expect("panel");
        }
        dashboard_id
    });

    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.open_dashboard_tab(dashboard_id, window, cx)
        });
    wait_for_refresh(&tab, &mut cx).await;

    tab.read_with(&cx, |tab, cx| {
        assert_eq!(tab.title, "Sales");
        let panels: Vec<_> = tab.panels.iter().map(|panel| panel.read(cx)).collect();
        assert_eq!(panels[0].row_count(cx), Some(4));
        assert_eq!(panels[0].error(), None);
        assert_eq!(panels[1].row_count(cx), Some(1));
        assert_eq!(panels[2].row_count(cx), Some(3));
        let refused = panels[3].error().expect("the write is refused");
        assert!(refused.contains("read-only"), "{refused}");
        assert!(panels[4].error().is_some());
    });

    // The status bar tells how the refresh went.
    let mut status = String::new();
    for _ in 0..50 {
        cx.run_until_parked();
        status = crate::test_harness::status_line(&harness, &cx)
            .text
            .to_string();
        if status.contains("failed") {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert_eq!(status, "Sales: 2 of 5 panels failed");

    // The refused write never ran, and no connection stays open for it.
    let service = cx.update(|_, cx| DatabaseService::global(cx).clone());
    let status = handle
        .block_on(service.get_connection_status(connection.id, Some("main")))
        .expect("status");
    assert!(!status.is_connected, "the dashboard keeps no connection");
    let remaining = handle
        .block_on(service.execute_query(connection.id, Some("main"), "SELECT COUNT(*) FROM sales"))
        .expect("count");
    assert_eq!(remaining.rows[0][0].as_deref(), Some("4"));
}

#[gpui::test]
async fn test_query_result_is_added_to_a_dashboard(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let connection = demo_connection(&mut cx);
    let dashboard_id = handle
        .block_on(database.create_dashboard("Ops", Some(connection.id)))
        .expect("dashboard");
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title: "Answer".into(),
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context: ConnectionContext {
                        connection_id: connection.id,
                        connection_name: connection.name.clone(),
                        db_type: database::DatabaseType::SQLite,
                        database_name: "main".into(),
                        schema_name: None,
                        environment_type: None,
                    },
                },
                cx,
            );
        });

    set_editor_text(&harness, "-- The answer\nSELECT 42 AS answer", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.add_result_to_dashboard(Some(dashboard_id), window, cx)
        });
    let mut panels = Vec::new();
    for _ in 0..100 {
        cx.run_until_parked();
        panels = handle
            .block_on(database.load_dashboard_panels(dashboard_id))
            .expect("panels");
        if !panels.is_empty() {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    let [panel] = panels.as_slice() else {
        panic!("one panel was added, found {}", panels.len());
    };
    assert_eq!(panel.title, "The answer");
    assert_eq!(panel.query, "SELECT 42 AS answer");
    let PanelView::Table { layout, .. } = PanelView::from_json(&panel.view) else {
        panic!("a table result becomes a table panel");
    };
    assert_eq!(layout.order, ["answer"]);
    assert_eq!(panel.connection.as_ref().map(|c| c.id), Some(connection.id));

    // The tab now saves back into the panel it made.
    let mut linked = None;
    for _ in 0..100 {
        cx.run_until_parked();
        linked = harness.editor_panel.read_with(&cx, |editor, cx| {
            editor
                .active_query_tab()
                .and_then(|tab| tab.results_panel.read(cx).dashboard_link().cloned())
                .and_then(|link| link.panel)
                .map(|panel| panel.id)
        });
        if linked.is_some() {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert_eq!(linked, Some(panel.id));
}

#[gpui::test]
async fn test_panels_filter_on_the_time_range_and_variables(cx: &mut TestAppContext) {
    use super::parameters::{DashboardVariable, VariableSource, variables_to_json};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let connection = demo_connection(&mut cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let dashboard_id = handle.block_on(async {
        let dashboard_id = database
            .create_dashboard("Regions", Some(connection.id))
            .await
            .expect("dashboard");
        let mut dashboard = database
            .load_dashboard(dashboard_id)
            .await
            .expect("load")
            .expect("dashboard");
        dashboard.variables = variables_to_json(&[
            DashboardVariable {
                name: "region".to_string(),
                source: VariableSource::Values {
                    values: vec!["South".to_string(), "North".to_string()],
                },
                current: Some("North".to_string()),
            },
            DashboardVariable {
                name: "regions".to_string(),
                source: VariableSource::Query {
                    connection_id: connection.id,
                    database_name: "main".to_string(),
                    query: "SELECT DISTINCT region FROM sales ORDER BY region".to_string(),
                },
                current: None,
            },
        ]);
        database.update_dashboard(&dashboard).await.expect("save");
        for (title, query) in [
            (
                "Region total",
                "SELECT SUM(amount) AS total FROM sales WHERE region = $region",
            ),
            (
                "Recent sales",
                "SELECT COUNT(*) AS sales FROM sales WHERE $__timeFilter(sold_at)",
            ),
            ("First region", "SELECT ${regions} AS region"),
        ] {
            database
                .insert_dashboard_panel(&panel(
                    dashboard_id,
                    &connection,
                    title,
                    query,
                    PanelView::Stat(StatSpec::default()),
                ))
                .await
                .expect("panel");
        }
        dashboard_id
    });

    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.open_dashboard_tab(dashboard_id, window, cx)
        });
    wait_for_refresh(&tab, &mut cx).await;
    let cells = |tab: &Entity<DashboardTab>, cx: &VisualTestContext| {
        tab.read_with(cx, |tab, cx| {
            tab.panels
                .iter()
                .map(|panel| {
                    let panel = panel.read(cx);
                    panel
                        .cell(0, 0, cx)
                        .or_else(|| panel.error().map(str::to_string))
                })
                .collect::<Vec<_>>()
        })
    };
    // North's two sales, the two of the last 7 days, and the first region
    // the query variable found.
    assert_eq!(
        cells(&tab, &cx),
        vec![
            Some("200".to_string()),
            Some("2".to_string()),
            Some("East".to_string())
        ]
    );

    tab.update_in(&mut cx, |tab, window, cx| {
        tab.time_range = super::TimeRange::Last24Hours;
        tab.variables[0].current = Some("South".to_string());
        tab.refresh(window, cx);
    });
    wait_for_refresh(&tab, &mut cx).await;
    assert_eq!(
        cells(&tab, &cx),
        vec![
            Some("250".to_string()),
            Some("1".to_string()),
            Some("East".to_string())
        ]
    );
}

#[gpui::test]
async fn test_a_query_tab_remembers_its_chart(cx: &mut TestAppContext) {
    use crate::results_panel::ResultViewMode;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let connection = demo_connection(&mut cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let context = ConnectionContext {
        connection_id: connection.id,
        connection_name: connection.name.clone(),
        db_type: database::DatabaseType::SQLite,
        database_name: "main".into(),
        schema_name: None,
        environment_type: None,
    };
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.create_and_add_tab_with_connection(
                window,
                TabCreationParams {
                    title: "By region".into(),
                    content: None,
                    db_id: None,
                    last_run_at: None,
                    context: context.clone(),
                },
                cx,
            );
        });
    set_editor_text(
        &harness,
        "SELECT region, SUM(amount) AS total FROM sales GROUP BY region",
        &mut cx,
    );
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;

    let results = harness.editor_panel.read_with(&cx, |panel, _| {
        panel
            .active_query_tab()
            .map(|tab| tab.results_panel.clone())
            .expect("query tab")
    });
    results.update(&mut cx, |results, cx| {
        results.set_view_mode(ResultViewMode::Chart, cx)
    });
    let mut saved = None;
    for _ in 0..100 {
        cx.run_until_parked();
        saved = handle
            .block_on(database.load_query_tabs(harness.window_id))
            .expect("tabs")
            .into_iter()
            .find(|tab| tab.title == "By region");
        if saved.as_ref().is_some_and(|tab| tab.result_view.is_some()) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    let saved = saved.expect("the tab was saved");
    let Some(PanelView::Chart(spec)) = saved.result_view.as_deref().map(PanelView::from_json)
    else {
        panic!("the chart is remembered, found {:?}", saved.result_view);
    };
    assert_eq!(spec.axis.as_deref(), Some("region"));

    // The restored tab shows its next result as that chart.
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.restore_saved_tabs_with_connections_sync(vec![saved], window, cx);
            let last = panel.tabs.len() - 1;
            panel.activate_tab(last, window, cx);
        });
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    let mode = harness.editor_panel.read_with(&cx, |panel, cx| {
        panel
            .active_query_tab()
            .map(|tab| tab.results_panel.read(cx).active_view_mode())
    });
    assert_eq!(mode, Some(ResultViewMode::Chart));

    // Another query shows as a table, and its chart is not kept for it.
    set_editor_text(&harness, "SELECT 1 AS one", &mut cx);
    run_query(&harness, &mut cx);
    wait_for_query(&harness, &mut cx).await;
    let mut charts = Vec::new();
    for _ in 0..100 {
        cx.run_until_parked();
        charts = handle
            .block_on(database.load_query_tabs(harness.window_id))
            .expect("tabs")
            .into_iter()
            .filter(|tab| tab.title == "By region")
            .filter_map(|tab| tab.result_view)
            .collect();
        if charts.is_empty() {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert!(
        charts.is_empty(),
        "the chart outlived its query: {charts:?}"
    );
}

#[gpui::test]
async fn test_a_panel_resizes_by_its_corner(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let connection = demo_connection(&mut cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let (dashboard_id, panel_id) = handle.block_on(async {
        let dashboard_id = database
            .create_dashboard("Sizes", Some(connection.id))
            .await
            .expect("dashboard");
        let panel_id = database
            .insert_dashboard_panel(&panel(
                dashboard_id,
                &connection,
                "All sales",
                "SELECT region, amount FROM sales",
                PanelView::default(),
            ))
            .await
            .expect("panel");
        (dashboard_id, panel_id)
    });
    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.open_dashboard_tab(dashboard_id, window, cx)
        });
    wait_for_refresh(&tab, &mut cx).await;
    cx.run_until_parked();

    let corner = cx
        .debug_bounds("dashboard-panel-resize")
        .expect("the panel has a resize corner")
        .center();
    // Rightwards past the grid's edge and down two rows.
    let target = point(corner.x + px(4000.), corner.y + px(2. * super::ROW_HEIGHT));
    cx.simulate_mouse_down(corner, MouseButton::Left, Modifiers::none());
    for step in 1..=4 {
        let progress = step as f32 / 4.;
        cx.simulate_mouse_move(
            point(
                corner.x + (target.x - corner.x) * progress,
                corner.y + (target.y - corner.y) * progress,
            ),
            MouseButton::Left,
            Modifiers::none(),
        );
    }
    cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();

    let size = tab.read_with(&cx, |tab, cx| {
        let data = &tab.panels[0].read(cx).data;
        (data.width, data.height)
    });
    assert_eq!(size, (app_database::DASHBOARD_COLUMNS, 6));
    let mut saved = None;
    for _ in 0..100 {
        cx.run_until_parked();
        saved = handle
            .block_on(database.load_dashboard_panels(dashboard_id))
            .expect("panels")
            .into_iter()
            .find(|panel| panel.id == panel_id)
            .map(|panel| (panel.width, panel.height));
        if saved == Some(size) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert_eq!(saved, Some(size), "the size is saved once the drag ends");
}

#[gpui::test]
async fn test_a_panel_is_edited_in_a_query_tab_and_saved_back(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let connection = demo_connection(&mut cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let (dashboard_id, panel_id) = handle.block_on(async {
        let dashboard_id = database
            .create_dashboard("Edits", Some(connection.id))
            .await
            .expect("dashboard");
        let panel_id = database
            .insert_dashboard_panel(&panel(
                dashboard_id,
                &connection,
                "Sales",
                "SELECT region FROM sales",
                PanelView::default(),
            ))
            .await
            .expect("panel");
        (dashboard_id, panel_id)
    });
    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.open_dashboard_tab(dashboard_id, window, cx)
        });
    wait_for_refresh(&tab, &mut cx).await;

    tab.update_in(&mut cx, |tab, window, cx| {
        tab.edit_panel(panel_id, window, cx)
    });
    cx.run_until_parked();
    wait_for_query(&harness, &mut cx).await;
    let linked = harness.editor_panel.read_with(&cx, |editor, cx| {
        let tab = editor.active_query_tab()?;
        let link = tab.results_panel.read(cx).dashboard_link()?.clone();
        Some((tab.title.clone(), link.panel?.id))
    });
    assert_eq!(linked, Some(("Sales".to_string(), panel_id)));

    // Renamed and resized on the dashboard while the query tab is open.
    handle.block_on(async {
        let mut changed = database
            .load_dashboard_panels(dashboard_id)
            .await
            .expect("panels")
            .into_iter()
            .find(|panel| panel.id == panel_id)
            .expect("panel");
        changed.title = "Renamed".to_string();
        changed.width = 3;
        database
            .update_dashboard_panel(&changed)
            .await
            .expect("rename");
    });

    // Saved without running it first: the save runs it.
    set_editor_text(&harness, "SELECT region, amount FROM sales", &mut cx);
    harness
        .editor_panel
        .update_in(&mut cx, |editor, window, cx| {
            editor.update_linked_dashboard_panel(window, cx)
        });
    wait_for_query(&harness, &mut cx).await;
    let mut query = None;
    for _ in 0..100 {
        cx.run_until_parked();
        query = handle
            .block_on(database.load_dashboard_panels(dashboard_id))
            .expect("panels")
            .into_iter()
            .find(|panel| panel.id == panel_id)
            .map(|panel| panel.query);
        if query.as_deref() == Some("SELECT region, amount FROM sales") {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert_eq!(query.as_deref(), Some("SELECT region, amount FROM sales"));
    let kept = handle
        .block_on(database.load_dashboard_panels(dashboard_id))
        .expect("panels")
        .into_iter()
        .find(|panel| panel.id == panel_id)
        .map(|panel| (panel.title, panel.width));
    assert_eq!(
        kept,
        Some(("Renamed".to_string(), 3)),
        "the save keeps what the dashboard changed meanwhile"
    );
}

#[gpui::test]
async fn test_a_deleted_dashboard_closes_its_tab(cx: &mut TestAppContext) {
    use super::DashboardDirectory;

    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let dashboard_id = handle
        .block_on(database.create_dashboard("Gone", None))
        .expect("dashboard");
    let reload = |cx: &mut VisualTestContext| cx.update(|_, cx| DashboardDirectory::reload(cx));
    reload(&mut cx);
    harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.open_dashboard_tab(dashboard_id, window, cx);
        });
    cx.run_until_parked();
    // The only tab, which the user could not close but a deletion must.
    harness.editor_panel.update(&mut cx, |panel, cx| {
        while let Some(ix) = (0..panel.tabs.len())
            .find(|ix| !matches!(panel.tabs.get(*ix), Some(TabType::Dashboard(_))))
        {
            panel.remove_tab(ix, cx);
        }
    });
    reload(&mut cx);
    cx.run_until_parked();
    let open = |cx: &VisualTestContext| {
        harness
            .editor_panel
            .read_with(cx, |panel, cx| panel.active_dashboard_id(cx).is_some())
    };
    assert!(open(&cx));

    handle
        .block_on(database.delete_dashboard(dashboard_id))
        .expect("delete");
    reload(&mut cx);
    for _ in 0..50 {
        cx.run_until_parked();
        if !open(&cx) {
            break;
        }
        cx.executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
    }
    assert!(!open(&cx), "the tab closes with its dashboard");
    let tabs = harness
        .editor_panel
        .read_with(&cx, |panel, _| panel.tabs.len());
    assert_eq!(tabs, 0);
}

#[gpui::test]
async fn test_a_failing_query_is_not_saved_to_its_panel(cx: &mut TestAppContext) {
    let harness = TestHarness::new(cx);
    let mut cx = VisualTestContext::from_window(harness.window_handle.into(), cx);
    let connection = demo_connection(&mut cx);
    let (database, handle) = cx.update(|_, cx| {
        (
            AppDatabase::global(cx).clone(),
            gpui_tokio::Tokio::handle(cx),
        )
    });
    let (dashboard_id, panel_id) = handle.block_on(async {
        let dashboard_id = database
            .create_dashboard("Edits", Some(connection.id))
            .await
            .expect("dashboard");
        let panel_id = database
            .insert_dashboard_panel(&panel(
                dashboard_id,
                &connection,
                "Sales",
                "SELECT region FROM sales",
                PanelView::default(),
            ))
            .await
            .expect("panel");
        (dashboard_id, panel_id)
    });
    let tab = harness
        .editor_panel
        .update_in(&mut cx, |panel, window, cx| {
            panel.open_dashboard_tab(dashboard_id, window, cx)
        });
    wait_for_refresh(&tab, &mut cx).await;
    tab.update_in(&mut cx, |tab, window, cx| {
        tab.edit_panel(panel_id, window, cx)
    });
    cx.run_until_parked();
    wait_for_query(&harness, &mut cx).await;

    set_editor_text(&harness, "SELECT nothing FROM nowhere", &mut cx);
    harness
        .editor_panel
        .update_in(&mut cx, |editor, window, cx| {
            editor.update_linked_dashboard_panel(window, cx)
        });
    wait_for_query(&harness, &mut cx).await;
    for _ in 0..10 {
        cx.run_until_parked();
    }
    let query = handle
        .block_on(database.load_dashboard_panels(dashboard_id))
        .expect("panels")
        .into_iter()
        .find(|panel| panel.id == panel_id)
        .map(|panel| panel.query);
    assert_eq!(query.as_deref(), Some("SELECT region FROM sales"));
}
