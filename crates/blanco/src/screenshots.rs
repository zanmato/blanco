//! Stages the app for the website screenshots. Built only with the
//! `screenshots` feature and driven by `scripts/screenshots/capture`, which
//! runs Blanco on a virtual display against a throwaway data directory.
//!
//! `BLANCO_SCENE` names the scene to stage, `BLANCO_DEMO_DB` the SQLite file
//! the demo connection opens, `BLANCO_THEME` the theme and `BLANCO_SCENE_READY`
//! the file written once the window shows the scene. With the `scene-export`
//! feature, `BLANCO_SCENE_EXPORT` names a file the shown frame is written to
//! as a vector snapshot first. `BLANCO_FONT` and `BLANCO_MONO_FONT`, when set,
//! replace the theme's fonts.

use std::time::Duration;

use app_database::{
    AppDatabase, ConnectionData, DashboardPanelData, EditorKind, PanelConnection, QueryTabData,
    SnippetData, WindowGeometry,
};
use blanco_core::ddl::{ColumnOperation, ColumnSpec, IndexOperation, IndexSpec};
use blanco_core::{ConnectionContext, DatabaseType};
use gpui::{App, AppContext, AsyncWindowContext, Entity, Focusable as _, Window};
use tokio::runtime::Handle;

use crate::app::{
    BlancoApp, ConnectToConnection, OpenDashboard, OpenSchemaGraph, OpenSettings,
    OpenTableStructure, ShowQueryInNewTab, SidebarTab, ToggleCommandPalette,
};
use crate::app_settings::AppSettings;
use crate::editor::EditorPanel;
use crate::export::modal::{ExportModal, open_export_dialog};
use crate::import::modal::{ImportModal, open_import_dialog};
use crate::results_panel::{CellInput, ChartKind, ResultViewMode, ResultsTableDelegate};

// The visible frame is 1600x1000. On Linux the client-side frame adds a 20px
// shadow inset on every side, which the capture script crops away.
const WINDOW_WIDTH: f32 = 1640.;
const WINDOW_HEIGHT: f32 = 1040.;

const REVENUE_QUERY: &str = r#"-- Orders waiting to ship
SELECT COUNT(*) AS pending FROM orders WHERE status = 'pending';

-- Revenue by genre this year, returns excluded
SELECT
  g.name AS genre,
  COUNT(DISTINCT o.id) AS orders,
  SUM(oi.quantity) AS copies,
  ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue
FROM order_items oi
JOIN orders o ON o.id = oi.order_id
JOIN albums al ON al.id = oi.album_id
JOIN genres g ON g.id = al.genre_id
WHERE
  o.ordered_at >= '2026-01-01'
  AND o.status <> 'returned'
GROUP BY g.name
ORDER BY revenue DESC;

-- Best sellers of all time
SELECT title, artist, copies FROM album_sales ORDER BY copies DESC LIMIT 20;"#;

/// [`Scene::Editor`] puts the caret right after this text, inside the revenue
/// statement, and runs the statement it is in.
const REVENUE_CARET: &str = "JOIN albums al";

const LOW_STOCK_QUERY: &str = r#"SELECT id, title, format, price, stock
FROM albums
WHERE stock < 3
ORDER BY stock, title;
"#;

const RESTOCK_SCRIPT: &str = r#"// Albums that sell out within a month at this quarter's pace.
const sales = db.query(`
  SELECT al.title, ar.name AS artist, al.stock,
         SUM(oi.quantity) / 13.0 AS weekly
  FROM albums al
  JOIN artists ar ON ar.id = al.artist_id
  JOIN order_items oi ON oi.album_id = al.id
  JOIN orders o ON o.id = oi.order_id
  WHERE o.ordered_at >= '2026-07-01'
  GROUP BY al.id
`);

const restock = sales.rows
  .map((row) => ({
    album: row.title,
    artist: row.artist,
    in_stock: row.stock,
    weeks_left: Math.round((row.stock / row.weekly) * 10) / 10,
    order: Math.ceil(row.weekly * 8 - row.stock),
  }))
  .filter((row) => row.in_stock > 0 && row.weeks_left < 4)
  .sort((a, b) => a.weeks_left - b.weeks_left || b.order - a.order);

console.log(`${sales.rows.length} albums sold this quarter`);
console.log(`${restock.length} need restocking`);
db.display(restock);
"#;

/// [`Scene::Motion`] starts the low stock tab with these statements, then
/// types [`MOTION_STATEMENT`] after them a line at a time.
const MOTION_DOCUMENT: &str = r#"-- Orders waiting to ship
SELECT COUNT(*) AS pending FROM orders WHERE status = 'pending';

-- Best sellers of all time
SELECT title, artist, copies FROM album_sales ORDER BY copies DESC LIMIT 20;

-- Albums about to sell out
"#;

const MOTION_STATEMENT: [&str; 4] = [
    "SELECT id, title, format, price, stock, details\n",
    "FROM albums\n",
    "WHERE stock < 3\n",
    "ORDER BY stock, title;",
];

/// The stock cell [`Scene::Motion`] edits, and what it types into it.
const MOTION_EDIT_CELL: (usize, usize) = (0, 4);
const MOTION_EDIT_VALUE: &str = "25";
/// The details cell [`Scene::Motion`] opens in the expanded JSON editor.
const MOTION_JSON_CELL: (usize, usize) = (0, 5);
/// The cell [`Scene::Motion`] right clicks for its context menu, by its text,
/// and the menu item it then points at.
const MOTION_MENU_CELL: &str = "Blue Room";
const MOTION_MENU_ITEM: &str = "Copy as";

/// [`Scene::JsonEdit`] opens this in a tab of its own and expands the
/// preferences of the row at [`JSON_EDIT_CELL`].
const CUSTOMERS_QUERY: &str = "SELECT id, first_name, last_name, city, preferences FROM customers";
const JSON_EDIT_CELL: (usize, usize) = (2, 4);

/// The file [`Scene::Import`] loads into the albums table.
const NEW_RELEASES_CSV: &str = "title,artist_id,genre_id,release_year,format,price,stock
Night Ferry,4,2,2026,LP,27.99,40
Night Ferry,4,2,2026,2LP,34.99,15
Salt and Static,11,5,2026,LP,24.99,30
Glass Orchard,7,1,2026,12\",14.99,60
Low Country Hymns,19,3,2026,LP,26.99,25
";

const TOP_CUSTOMERS_SNIPPET: &str = r#"-- Customers by lifetime spend
SELECT
  c.first_name || ' ' || c.last_name AS customer,
  c.city,
  COUNT(DISTINCT o.id) AS orders,
  ROUND(SUM(oi.quantity * oi.unit_price), 2) AS spent
FROM customers c
JOIN orders o ON o.customer_id = c.id
JOIN order_items oi ON oi.order_id = o.id
WHERE o.status <> 'returned'
GROUP BY c.id
ORDER BY spent DESC
LIMIT 25;
"#;

/// What the window shows when the screenshot is taken.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scene {
    /// The caret in one statement of a multi-statement query, which frames it,
    /// and that statement run on its own, with the sidebar listing the tables
    /// and their sizes.
    Editor,
    /// Result cells edited in place, one of them still open for editing.
    InlineEdit,
    /// A JSON cell open in the expanded editor, formatted.
    JsonEdit,
    /// The terminal pane docked under a query tab, running Claude Code.
    Terminal,
    /// A script that has run, with its log and the grid it displayed.
    Script,
    /// The snippets sidebar with a snippet open in the editor.
    Snippets,
    /// The structure of the albums table with a column and an index on it
    /// staged.
    Structure,
    /// The low stock result filtered on the server to LP records.
    Filter,
    /// The revenue statement's result drawn as a chart.
    Chart,
    /// The revenue statement's query plan.
    Explain,
    /// The ER diagram of the demo database.
    SchemaGraph,
    /// The history panel after a few runs.
    History,
    /// The command palette open over the editor.
    Palette,
    /// The settings tab.
    Settings,
    /// The export dialog for the albums table.
    Export,
    /// The import dialog with a CSV of new releases loaded.
    Import,
    /// A dashboard of the shop's numbers: stats, charts and a table.
    Dashboard,
    /// Not a screenshot: the keyframes of the website's motion demo, each
    /// exported as a snapshot into the directory `BLANCO_SCENE_EXPORT` names.
    /// A statement typed a line at a time, which the editor frames as it
    /// grows; its result; a stock cell in that result edited in place; and a
    /// JSON cell opened in the expanded editor. Then, for the hotspots shown
    /// when the demo stops, a cell's context menu with Copy as open, and the
    /// script tab.
    Motion,
    /// Not a screenshot: a grid of 1 MB text values, scrolled and then
    /// redrawn in place, after which the app quits. Run with
    /// `ZED_MEASUREMENTS=1` to time each frame (scripts/measure-frames).
    LargeValues,
    /// Not a screenshot either: the SQL editor holding a 1 MB query on one
    /// line, redrawn plain, with whitespace shown and with invisible
    /// characters highlighted, after which the app quits.
    LargeQuery,
}

impl Scene {
    pub(crate) fn from_env() -> Option<Self> {
        let scene = std::env::var("BLANCO_SCENE").ok()?;
        match scene.as_str() {
            "editor" => Some(Self::Editor),
            "inline-edit" => Some(Self::InlineEdit),
            "json-edit" => Some(Self::JsonEdit),
            "terminal" => Some(Self::Terminal),
            "script" => Some(Self::Script),
            "snippets" => Some(Self::Snippets),
            "structure" => Some(Self::Structure),
            "filter" => Some(Self::Filter),
            "chart" => Some(Self::Chart),
            "explain" => Some(Self::Explain),
            "schema-graph" => Some(Self::SchemaGraph),
            "history" => Some(Self::History),
            "palette" => Some(Self::Palette),
            "settings" => Some(Self::Settings),
            "export" => Some(Self::Export),
            "import" => Some(Self::Import),
            "dashboard" => Some(Self::Dashboard),
            "motion" => Some(Self::Motion),
            "large-values" => Some(Self::LargeValues),
            "large-query" => Some(Self::LargeQuery),
            _ => {
                tracing::error!("Unknown BLANCO_SCENE {scene:?}");
                None
            }
        }
    }

    /// The editor tab the scene brings to the front, in seeding order.
    fn tab_index(self) -> usize {
        match self {
            Self::Editor
            | Self::Terminal
            | Self::Snippets
            | Self::LargeQuery
            | Self::Structure
            | Self::Chart
            | Self::Explain
            | Self::SchemaGraph
            | Self::History
            | Self::Palette
            | Self::Settings
            | Self::Export
            | Self::Import
            | Self::Dashboard
            | Self::JsonEdit => 0,
            Self::InlineEdit | Self::LargeValues | Self::Filter | Self::Motion => 1,
            Self::Script => 2,
        }
    }
}

/// Fill a fresh app database with the demo connection, tabs, snippets and
/// settings. Does nothing when the database already holds a connection, so a
/// data directory that was not thrown away is never written to twice.
pub(crate) fn seed(database: &AppDatabase, runtime: &Handle) {
    if let Err(error) = runtime.block_on(seed_database(database)) {
        tracing::error!("Failed to seed the screenshot database: {error:#}");
    }
}

async fn seed_database(database: &AppDatabase) -> anyhow::Result<()> {
    if !database.load_connections().await?.is_empty() {
        return Ok(());
    }

    let demo_path = std::env::var("BLANCO_DEMO_DB")?;
    let connection = ConnectionData::new_sqlite("Groove Records".to_string(), demo_path.clone());
    let connection_id = database.save_connection(&connection).await?;

    let theme = std::env::var("BLANCO_THEME").unwrap_or_else(|_| "Catppuccin Mocha".into());
    for (key, value) in [
        ("appearance.theme", theme.as_str()),
        ("mcp.enabled", "false"),
    ] {
        database.save_setting(key, value, false).await?;
    }
    // The theme's fonts are installed where the screenshots are taken. A run
    // elsewhere can name ones it has, so text keeps its look and metrics.
    for (key, variable) in [
        ("appearance.font_family", "BLANCO_FONT"),
        ("appearance.mono_font_family", "BLANCO_MONO_FONT"),
    ] {
        if let Ok(family) = std::env::var(variable) {
            database.save_setting(key, &family, false).await?;
        }
    }
    let window_id = database
        .create_window(
            Some(WindowGeometry {
                x: 0.,
                y: 0.,
                width: WINDOW_WIDTH,
                height: WINDOW_HEIGHT,
                maximized: false,
            }),
            None,
        )
        .await?;

    let tabs = [
        ("Revenue", REVENUE_QUERY, EditorKind::Query),
        ("Low stock", LOW_STOCK_QUERY, EditorKind::Query),
        ("Restock report", RESTOCK_SCRIPT, EditorKind::Script),
    ];
    for (position, (title, content, tab_kind)) in tabs.into_iter().enumerate() {
        database
            .save_query_tab(&QueryTabData {
                id: None,
                title: title.to_string(),
                content: content.to_string(),
                position: position as i32,
                connection_id: Some(connection_id),
                connection_type: Some(connection.db_type.as_str().to_string()),
                connection_name: Some(connection.name.clone()),
                database_name: Some("main".to_string()),
                schema_name: None,
                environment_type: None,
                tab_kind,
                last_run_at: None,
                file_path: None,
                window_id: Some(window_id),
                result_view: None,
            })
            .await?;
    }

    seed_dashboard(database, connection_id, &connection.name).await?;

    let groups: [(&str, &[(&str, &str, EditorKind)]); 2] = [
        (
            "Reports",
            &[
                ("Top customers", TOP_CUSTOMERS_SNIPPET, EditorKind::Query),
                ("Revenue by genre", REVENUE_QUERY, EditorKind::Query),
                ("Restock report", RESTOCK_SCRIPT, EditorKind::Script),
            ],
        ),
        (
            "Maintenance",
            &[
                (
                    "Orphaned order items",
                    "SELECT oi.*\nFROM order_items oi\nLEFT JOIN orders o ON o.id = oi.order_id\nWHERE o.id IS NULL;\n",
                    EditorKind::Query,
                ),
                (
                    "Refresh statistics",
                    "ANALYZE;\nPRAGMA optimize;\n",
                    EditorKind::Query,
                ),
            ],
        ),
    ];
    for (group_position, (group, snippets)) in groups.into_iter().enumerate() {
        let group_id = database
            .save_snippet(&SnippetData {
                id: None,
                name: group.to_string(),
                content: String::new(),
                kind: EditorKind::Query,
                parent_id: None,
                is_group: true,
                position: group_position as i32,
                trigger: None,
            })
            .await?;
        for (position, (name, content, kind)) in snippets.iter().enumerate() {
            database
                .save_snippet(&SnippetData {
                    id: None,
                    name: name.to_string(),
                    content: content.to_string(),
                    kind: *kind,
                    parent_id: Some(group_id),
                    is_group: false,
                    position: position as i32,
                    trigger: None,
                })
                .await?;
        }
    }

    Ok(())
}

/// The shop's dashboard, which [`Scene::Dashboard`] opens: what the panels
/// show, their width and height on the grid, and the query each runs.
const DASHBOARD_PANELS: [(&str, &str, u8, u8, &str); 8] = [
    (
        "Orders waiting to ship",
        r#"{"type":"stat","thresholds":{"warning":200,"critical":400}}"#,
        3,
        2,
        "SELECT COUNT(*) AS pending FROM orders WHERE status = 'pending'",
    ),
    (
        "Revenue this year",
        r#"{"type":"stat"}"#,
        3,
        2,
        "SELECT ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue\nFROM order_items oi\nJOIN orders o ON o.id = oi.order_id\nWHERE o.ordered_at >= '2026-01-01' AND o.status <> 'returned'",
    ),
    (
        "Orders this year",
        r#"{"type":"stat"}"#,
        3,
        2,
        "SELECT COUNT(*) AS orders FROM orders WHERE ordered_at >= '2026-01-01'",
    ),
    (
        "Customers",
        r#"{"type":"stat"}"#,
        3,
        2,
        "SELECT COUNT(*) AS customers FROM customers",
    ),
    (
        "Monthly revenue",
        r#"{"type":"chart","kind":"line","axis":"month","value":"revenue","series":null,"line_show_points":true}"#,
        8,
        4,
        "SELECT substr(o.ordered_at, 1, 7) AS month,\n  ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue\nFROM order_items oi\nJOIN orders o ON o.id = oi.order_id\nWHERE $__timeFilter(o.ordered_at) AND o.status <> 'returned'\nGROUP BY month\nORDER BY month",
    ),
    (
        "Copies by format",
        r#"{"type":"chart","kind":"pie","axis":"format","value":"copies","series":null}"#,
        4,
        4,
        "SELECT al.format, SUM(oi.quantity) AS copies\nFROM order_items oi\nJOIN albums al ON al.id = oi.album_id\nGROUP BY al.format\nORDER BY copies DESC",
    ),
    (
        "Revenue by genre",
        r#"{"type":"chart","kind":"bar","axis":"genre","value":"revenue","series":null}"#,
        6,
        4,
        "SELECT g.name AS genre, ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue\nFROM order_items oi\nJOIN albums al ON al.id = oi.album_id\nJOIN genres g ON g.id = al.genre_id\nGROUP BY g.name\nORDER BY revenue DESC",
    ),
    (
        "Out of stock",
        r#"{"type":"table","thresholds":{"column":"price","warning":30,"critical":40}}"#,
        6,
        4,
        "SELECT al.title, ar.name AS artist, al.price\nFROM albums al\nJOIN artists ar ON ar.id = al.artist_id\nWHERE al.stock = 0 AND al.format = $format\nORDER BY al.title",
    ),
];

async fn seed_dashboard(
    database: &AppDatabase,
    connection_id: i64,
    connection_name: &str,
) -> anyhow::Result<()> {
    let dashboard_id = database
        .create_dashboard("Shop overview", Some(connection_id))
        .await?;
    // A year of monthly revenue, and the out of stock list for one format.
    if let Some(mut dashboard) = database.load_dashboard(dashboard_id).await? {
        dashboard.time_range = "1y".to_string();
        dashboard.variables = serde_json::json!([{
            "name": "format",
            "source": {
                "type": "query",
                "connection_id": connection_id,
                "database_name": "main",
                "query": "SELECT DISTINCT format FROM albums ORDER BY format",
            },
            "current": "LP",
        }])
        .to_string();
        database.update_dashboard(&dashboard).await?;
    }
    for name in ["Sales", "Inventory"] {
        database.create_dashboard(name, Some(connection_id)).await?;
    }
    for (title, view, width, height, query) in DASHBOARD_PANELS {
        database
            .insert_dashboard_panel(&DashboardPanelData {
                id: 0,
                dashboard_id,
                title: title.to_string(),
                connection: Some(PanelConnection {
                    id: connection_id,
                    name: connection_name.to_string(),
                    db_type: DatabaseType::SQLite.as_str().to_string(),
                    environment_type: app_database::EnvironmentType::DEV,
                }),
                database_name: "main".to_string(),
                schema_name: None,
                query: query.to_string(),
                view: view.to_string(),
                position: 0,
                width,
                height,
            })
            .await?;
    }
    Ok(())
}

/// Stage `scene` in the window that holds `app`, then write the ready file.
pub(crate) fn stage(scene: Scene, app: Entity<BlancoApp>, window: &Window, cx: &App) {
    window
        .spawn(cx, async move |cx| {
            if let Err(error) = stage_scene(scene, &app, cx).await {
                tracing::error!("Failed to stage the screenshot scene: {error:#}");
                return;
            }
            // The motion scene exports its keyframes as it goes.
            if scene != Scene::Motion
                && let Some(path) = std::env::var_os("BLANCO_SCENE_EXPORT")
            {
                export_frame(path.into(), cx).await;
            }
            if let Ok(path) = std::env::var("BLANCO_SCENE_READY")
                && let Err(error) = std::fs::write(&path, "ready")
            {
                tracing::error!("Failed to write {path}: {error}");
            }
        })
        .detach();
}

async fn stage_scene(
    scene: Scene,
    app: &Entity<BlancoApp>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    let editor_panel = app.read_with(cx, |app, _| app.editor_panel().clone());

    // The connection is the first and only one the seed created.
    cx.update(|window, cx| {
        window.dispatch_action(Box::new(ConnectToConnection { connection_id: 1 }), cx);
    })?;
    editor_panel.update_in(cx, |panel, window, cx| {
        panel.activate_tab(scene.tab_index(), window, cx);
    })?;
    settle(cx, 1500).await;

    match scene {
        Scene::Editor => {
            hide_sql_log(&editor_panel, cx)?;
            place_caret_in_revenue_statement(&editor_panel, cx)?;
            run_active_tab(&editor_panel, cx).await?;
            // The run moves focus to the results, which hides the caret.
            place_caret_in_revenue_statement(&editor_panel, cx)?;
        }
        Scene::InlineEdit => {
            hide_sql_log(&editor_panel, cx)?;
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.set_pane_heights(gpui::px(200.), gpui::px(120.), window, cx);
            })?;
            run_active_tab(&editor_panel, cx).await?;
            let results_panel = editor_panel
                .read_with(cx, |panel, _| panel.active_results_panel())
                .ok_or_else(|| anyhow::anyhow!("no results panel"))?;
            // Two edits already made, a third one in progress on the stock
            // column.
            results_panel.update_in(cx, |panel, _, cx| {
                panel.commit_cell_edit(1, 3, "24.99".to_string(), cx);
                panel.commit_cell_edit(3, 4, "12".to_string(), cx);
            })?;
            results_panel.update_in(cx, |panel, window, cx| {
                panel.start_cell_edit(5, 4, window, cx);
            })?;
            settle(cx, 200).await;
            let table_state = results_panel.read_with(cx, |panel, _| panel.table_state().clone());
            let input = table_state.read_with(cx, |state, _| {
                state.delegate().edit_state.get_editing_input()
            });
            if let Some(CellInput::Inline(input)) = input {
                input.update_in(cx, |input, window, cx| {
                    input.replace_all("25", window, cx);
                })?;
            }
        }
        Scene::JsonEdit => {
            cx.update(|window, cx| {
                window.dispatch_action(
                    Box::new(ShowQueryInNewTab {
                        connection_id: 1,
                        database_name: "main".to_string(),
                        schema_name: None,
                        title: "customers".to_string(),
                        sql: CUSTOMERS_QUERY.to_string(),
                    }),
                    cx,
                );
            })?;
            settle(cx, 300).await;
            hide_sql_log(&editor_panel, cx)?;
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.set_pane_heights(gpui::px(120.), gpui::px(120.), window, cx);
            })?;
            wait_for_run(&editor_panel, cx).await?;
            let results_panel = editor_panel
                .read_with(cx, |panel, _| panel.active_results_panel())
                .ok_or_else(|| anyhow::anyhow!("no results panel"))?;
            let (row, column) = JSON_EDIT_CELL;
            results_panel.update_in(cx, |panel, window, cx| {
                panel.start_cell_edit(row, column, window, cx);
            })?;
            settle(cx, 200).await;
            let table_state = results_panel.read_with(cx, |panel, _| panel.table_state().clone());
            table_state.update_in(cx, |state, window, cx| {
                ResultsTableDelegate::handle_maximize(state, row, column, false, window, cx);
                // Drag the grip down and to the left, so the whole document
                // shows.
                let size = &mut state.delegate_mut().edit_state.expanded_size;
                size.start_drag();
                size.drag_to(gpui::point(gpui::px(0.), gpui::px(0.)));
                size.drag_to(gpui::point(gpui::px(-180.), gpui::px(180.)));
                cx.notify();
            })?;
        }
        Scene::Terminal => {
            hide_sql_log(&editor_panel, cx)?;
            run_active_tab(&editor_panel, cx).await?;
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.toggle_terminal_for_active_tab(window, cx);
            })?;
            settle(cx, 500).await;
            // Room for Claude Code's welcome screen.
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.set_pane_heights(gpui::px(260.), gpui::px(120.), window, cx);
                panel.set_terminal_height(gpui::px(420.), window, cx);
            })?;
            settle(cx, 1000).await;
            editor_panel.update_in(cx, |panel, _, cx| {
                let Some(view) = panel
                    .active_query_tab()
                    .and_then(|tab| tab.terminal_pane_view())
                else {
                    return;
                };
                view.read(cx)
                    .terminal()
                    .read(cx)
                    .write(b"claude\n".as_slice());
            })?;
            // Claude Code takes a few seconds to start and connect to the MCP
            // server.
            settle(cx, 8000).await;
        }
        Scene::Script => {
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.set_pane_heights(gpui::px(400.), gpui::px(120.), window, cx);
                // Show the script from its first line.
                if let Some(tab) = panel.active_script_tab() {
                    tab.editor.update(cx, |editor, cx| {
                        editor.set_selected_range(0..0, window, cx);
                    });
                }
            })?;
            run_active_tab(&editor_panel, cx).await?;
        }
        Scene::Snippets => {
            app.update_in(cx, |app, _, cx| {
                app.select_sidebar_tab(SidebarTab::Snippets, cx);
            })?;
            // The first snippet after its group.
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.open_snippet_tab(2, window, cx);
            })?;
        }
        Scene::Structure => {
            cx.update(|window, cx| {
                window.dispatch_action(
                    Box::new(OpenTableStructure {
                        context: demo_context(),
                        table_name: "albums".to_string(),
                    }),
                    cx,
                );
            })?;
            settle(cx, 1500).await;
            let structure = editor_panel
                .read_with(cx, |panel, _| panel.active_structure_tab())
                .ok_or_else(|| anyhow::anyhow!("no structure tab"))?;
            structure.update_in(cx, |tab, window, cx| {
                tab.stage_column(
                    ColumnOperation::Add {
                        column: ColumnSpec {
                            name: "label".to_string(),
                            data_type: "TEXT".to_string(),
                            nullable: true,
                            ..ColumnSpec::default()
                        },
                        position: None,
                    },
                    window,
                    cx,
                );
                tab.stage_index(
                    IndexOperation::Create(IndexSpec {
                        name: "albums_label_idx".to_string(),
                        columns: vec!["label".to_string()],
                        ..IndexSpec::default()
                    }),
                    window,
                    cx,
                );
                tab.show_last_column(window, cx);
            })?;
        }
        Scene::Filter => {
            hide_sql_log(&editor_panel, cx)?;
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.set_pane_heights(gpui::px(160.), gpui::px(120.), window, cx);
            })?;
            run_active_tab(&editor_panel, cx).await?;
            let results_panel = editor_panel
                .read_with(cx, |panel, _| panel.active_results_panel())
                .ok_or_else(|| anyhow::anyhow!("no results panel"))?;
            results_panel.update_in(cx, |panel, window, cx| {
                panel.toggle_filter_bar(window, cx);
            })?;
            settle(cx, 300).await;
            let filter_bar = results_panel
                .read_with(cx, |panel, _| panel.active_filter_bar())
                .ok_or_else(|| anyhow::anyhow!("no filter bar"))?;
            filter_bar.update_in(cx, |bar, window, cx| {
                bar.filter_by_value("format", Some("LP"), window, cx);
            })?;
            for _ in 0..100 {
                settle(cx, 50).await;
                if !filter_bar.read_with(cx, |bar, _| bar.is_running()) {
                    break;
                }
            }
        }
        Scene::Chart => {
            hide_sql_log(&editor_panel, cx)?;
            place_caret_in_revenue_statement(&editor_panel, cx)?;
            run_active_tab(&editor_panel, cx).await?;
            let results_panel = editor_panel
                .read_with(cx, |panel, _| panel.active_results_panel())
                .ok_or_else(|| anyhow::anyhow!("no results panel"))?;
            results_panel.update_in(cx, |panel, _, cx| {
                panel.set_view_mode(ResultViewMode::Chart, cx);
            })?;
            let chart = results_panel
                .read_with(cx, |panel, _| panel.active_chart_view())
                .ok_or_else(|| anyhow::anyhow!("no chart view"))?;
            chart.update_in(cx, |chart, window, cx| {
                chart.show(ChartKind::Bar, "genre", "revenue", window, cx);
            })?;
        }
        Scene::Explain => {
            hide_sql_log(&editor_panel, cx)?;
            place_caret_in_revenue_statement(&editor_panel, cx)?;
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.on_explain_query(blanco_core::ExplainMode::Estimated, window, cx);
            })?;
            wait_for_run(&editor_panel, cx).await?;
        }
        Scene::Dashboard => {
            cx.update(|window, cx| {
                window.dispatch_action(Box::new(OpenDashboard { dashboard_id: 1 }), cx);
            })?;
            app.update_in(cx, |app, _, cx| {
                app.select_sidebar_tab(SidebarTab::Dashboards, cx);
            })?;
            settle(cx, 500).await;
            let dashboard = editor_panel
                .read_with(cx, |panel, _| panel.active_dashboard_tab())
                .ok_or_else(|| anyhow::anyhow!("no dashboard tab"))?;
            for _ in 0..200 {
                settle(cx, 50).await;
                if dashboard.read_with(cx, |dashboard, cx| dashboard.is_settled(cx)) {
                    break;
                }
            }
            settle(cx, 500).await;
        }
        Scene::SchemaGraph => {
            cx.update(|window, cx| {
                window.dispatch_action(
                    Box::new(OpenSchemaGraph {
                        context: demo_context(),
                    }),
                    cx,
                );
            })?;
            settle(cx, 1500).await;
            let graph = editor_panel
                .read_with(cx, |panel, _| panel.active_schema_graph_tab())
                .ok_or_else(|| anyhow::anyhow!("no schema graph tab"))?;
            graph.update(cx, |graph, cx| graph.show_all_edges(cx));
        }
        Scene::History => {
            hide_sql_log(&editor_panel, cx)?;
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.activate_tab(1, window, cx);
            })?;
            run_active_tab(&editor_panel, cx).await?;
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.activate_tab(0, window, cx);
            })?;
            settle(cx, 300).await;
            place_caret_in_revenue_statement(&editor_panel, cx)?;
            run_active_tab(&editor_panel, cx).await?;
            app.update_in(cx, |app, _, cx| {
                app.select_sidebar_tab(SidebarTab::History, cx);
            })?;
        }
        Scene::Palette => {
            hide_sql_log(&editor_panel, cx)?;
            place_caret_in_revenue_statement(&editor_panel, cx)?;
            run_active_tab(&editor_panel, cx).await?;
            cx.update(|window, cx| {
                window.dispatch_action(Box::new(ToggleCommandPalette), cx);
            })?;
        }
        Scene::Motion => {
            hide_sql_log(&editor_panel, cx)?;
            let editor = editor_panel
                .read_with(cx, |panel, _| {
                    panel.active_query_tab().map(|tab| tab.editor.clone())
                })
                .ok_or_else(|| anyhow::anyhow!("no query tab"))?;
            editor.update_in(cx, |editor, window, cx| {
                editor.set_value(MOTION_DOCUMENT, window, cx);
                let end = MOTION_DOCUMENT.len();
                editor.set_selected_range(end..end, window, cx);
                editor.focus(window, cx);
            })?;
            settle(cx, 300).await;
            export_keyframe("01-start", cx).await;
            for (ix, line) in MOTION_STATEMENT.iter().enumerate() {
                editor.update_in(cx, |editor, window, cx| editor.insert(*line, window, cx))?;
                settle(cx, 300).await;
                export_keyframe(&format!("{:02}-line-{}", ix + 2, ix + 1), cx).await;
            }

            run_active_tab(&editor_panel, cx).await?;
            settle(cx, 300).await;
            export_keyframe("06-results", cx).await;

            let results_panel = editor_panel
                .read_with(cx, |panel, _| panel.active_results_panel())
                .ok_or_else(|| anyhow::anyhow!("no results panel"))?;
            let (row, col) = MOTION_EDIT_CELL;
            results_panel.update_in(cx, |panel, window, cx| {
                panel.table_state().update(cx, |state, cx| {
                    state.set_selected_cell(row, col, cx);
                });
                panel.table_state().focus_handle(cx).focus(window, cx);
            })?;
            settle(cx, 200).await;
            export_keyframe("07-selected", cx).await;

            results_panel.update_in(cx, |panel, window, cx| {
                panel.start_cell_edit(row, col, window, cx);
            })?;
            settle(cx, 200).await;
            let table_state = results_panel.read_with(cx, |panel, _| panel.table_state().clone());
            let input = table_state.read_with(cx, |state, _| {
                state.delegate().edit_state.get_editing_input()
            });
            let Some(CellInput::Inline(input)) = input else {
                anyhow::bail!("the stock cell did not open for editing");
            };
            export_keyframe("08-editing", cx).await;
            input.update_in(cx, |input, window, cx| {
                input.replace_all(MOTION_EDIT_VALUE, window, cx);
            })?;
            settle(cx, 200).await;
            export_keyframe("09-typed", cx).await;

            results_panel.update_in(cx, |panel, _, cx| {
                panel.commit_cell_edit(row, col, MOTION_EDIT_VALUE.to_string(), cx);
            })?;
            settle(cx, 200).await;
            export_keyframe("10-edited", cx).await;

            let (row, col) = MOTION_JSON_CELL;
            results_panel.update_in(cx, |panel, _, cx| {
                panel.table_state().update(cx, |state, cx| {
                    state.set_selected_cell(row, col, cx);
                });
            })?;
            settle(cx, 200).await;
            export_keyframe("11-json-selected", cx).await;
            results_panel.update_in(cx, |panel, window, cx| {
                panel.start_cell_edit(row, col, window, cx);
            })?;
            settle(cx, 200).await;
            export_keyframe("12-json-inline", cx).await;
            table_state.update_in(cx, |state, window, cx| {
                ResultsTableDelegate::handle_maximize(state, row, col, false, window, cx);
                // Drag the grip out a little, so the whole document shows.
                let size = &mut state.delegate_mut().edit_state.expanded_size;
                size.start_drag();
                size.drag_to(gpui::point(gpui::px(0.), gpui::px(0.)));
                size.drag_to(gpui::point(gpui::px(-60.), gpui::px(40.)));
                cx.notify();
            })?;
            settle(cx, 300).await;
            export_keyframe("13-json-expanded", cx).await;

            // A cell's context menu, opened the way a user does: a right click
            // where the last keyframe shows the cell, then the pointer on an
            // item with a submenu.
            results_panel.update_in(cx, |panel, _, cx| panel.cancel_cell_edit(row, col, cx))?;
            settle(cx, 300).await;
            if let Some(at) = keyframe_text_position("13-json-expanded", MOTION_MENU_CELL) {
                cx.update(|window, cx| {
                    let click = |button_down: bool| {
                        if button_down {
                            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                                button: gpui::MouseButton::Right,
                                position: at,
                                modifiers: gpui::Modifiers::default(),
                                click_count: 1,
                                first_mouse: false,
                            })
                        } else {
                            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                                button: gpui::MouseButton::Right,
                                position: at,
                                modifiers: gpui::Modifiers::default(),
                                click_count: 1,
                            })
                        }
                    };
                    window.dispatch_event(click(true), cx);
                    window.dispatch_event(click(false), cx);
                })?;
                settle(cx, 300).await;
                export_keyframe("14-context-menu", cx).await;
                if let Some(at) = keyframe_text_position("14-context-menu", MOTION_MENU_ITEM) {
                    cx.update(|window, cx| {
                        window.dispatch_event(
                            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                position: at,
                                pressed_button: None,
                                modifiers: gpui::Modifiers::default(),
                            }),
                            cx,
                        );
                    })?;
                    settle(cx, 500).await;
                    export_keyframe("15-copy-as", cx).await;
                }
            }

            // The script tab, for its JavaScript, from its first line.
            editor_panel.update_in(cx, |panel, window, cx| {
                panel.activate_tab(Scene::Script.tab_index(), window, cx);
                if let Some(tab) = panel.active_script_tab() {
                    tab.editor.update(cx, |editor, cx| {
                        editor.set_selected_range(0..0, window, cx);
                    });
                }
            })?;
            settle(cx, 500).await;
            export_keyframe("16-script", cx).await;
        }
        Scene::Settings => {
            cx.update(|window, cx| {
                window.dispatch_action(Box::new(OpenSettings), cx);
            })?;
        }
        Scene::Export => {
            hide_sql_log(&editor_panel, cx)?;
            cx.update(|window, cx| {
                let modal =
                    cx.new(|cx| ExportModal::new(demo_context(), "albums".to_string(), window, cx));
                open_export_dialog(modal, window, cx);
            })?;
        }
        Scene::Import => {
            hide_sql_log(&editor_panel, cx)?;
            // Under the temporary folder rather than the run's own, so the
            // dialog shows a short path.
            let csv_path = std::env::temp_dir().join("new-releases.csv");
            std::fs::write(&csv_path, NEW_RELEASES_CSV)?;
            let csv_path = csv_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("the CSV path is not UTF-8"))?
                .to_string();
            cx.update(|window, cx| {
                let modal =
                    cx.new(|cx| ImportModal::new(demo_context(), "albums".to_string(), window, cx));
                modal.update(cx, |modal, cx| modal.load_file(&csv_path, window, cx));
                open_import_dialog(modal, window, cx);
            })?;
        }
        Scene::LargeValues => {
            measure_large_values(&editor_panel, cx).await?;
            cx.update(|_, cx| cx.quit())?;
        }
        Scene::LargeQuery => {
            measure_large_query(&editor_panel, cx).await?;
            cx.update(|_, cx| cx.quit())?;
        }
    }

    settle(cx, 1500).await;
    Ok(())
}

/// Put a 1 MB `INSERT` on one line in the editor, with a no-break space,
/// a zero width space and a control character every hundred characters, and
/// redraw it plain, with whitespace shown and with invisible characters
/// highlighted.
async fn measure_large_query(
    editor_panel: &Entity<EditorPanel>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    let chunk = format!(
        "{}\u{00A0}{}\u{200B}{}\u{0007}",
        "a".repeat(32),
        "b".repeat(32),
        "c".repeat(32)
    );
    let query = format!(
        "INSERT INTO notes (body) VALUES ('{}');",
        chunk.repeat(1_000_000 / chunk.len())
    );
    let editor = editor_panel
        .read_with(cx, |panel, _| {
            panel.active_query_tab().map(|tab| tab.editor.clone())
        })
        .ok_or_else(|| anyhow::anyhow!("no query tab"))?;
    editor.update_in(cx, |editor, window, cx| {
        editor.set_value(query, window, cx);
        editor.focus(window, cx);
    })?;
    settle(cx, 2000).await;

    for phase in ["plain", "whitespace", "invisible"] {
        editor.update_in(cx, |editor, window, cx| {
            editor.set_show_whitespaces(phase == "whitespace", window, cx);
            editor.set_highlight_invisible_characters(phase == "invisible", window, cx);
        })?;
        settle(cx, 500).await;
        tracing::info!("editor measurement: {phase}");
        for _ in 0..100 {
            cx.update(|window, _| window.refresh())?;
            settle(cx, 16).await;
        }
        tracing::info!("editor measurement: done");
    }
    Ok(())
}

/// Rows of 1 MB text values, each different so no two share a text layout,
/// holding tabs, no-break spaces and control characters for the markers.
const LARGE_VALUES_QUERY: &str = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100)\nSELECT i, i || ' ' || replace(hex(zeroblob(250000)), '00', 'a' || char(160) || char(9) || char(7)) AS big, 'short' AS small FROM n;";

/// Load the large values, then scroll the grid a row per frame and redraw it
/// in place, first as it starts and then with invisible characters shown,
/// logging where each phase starts so the frame durations GPUI prints can be
/// told apart.
async fn measure_large_values(
    editor_panel: &Entity<EditorPanel>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    hide_sql_log(editor_panel, cx)?;
    editor_panel.update_in(cx, |panel, window, cx| {
        if let Some(tab) = panel.active_query_tab() {
            tab.editor.update(cx, |editor, cx| {
                editor.set_value(LARGE_VALUES_QUERY, window, cx);
            });
        }
    })?;
    run_active_tab(editor_panel, cx).await?;
    settle(cx, 1000).await;
    let results_panel = editor_panel
        .read_with(cx, |panel, _| panel.active_results_panel())
        .ok_or_else(|| anyhow::anyhow!("no results panel"))?;
    let table_state = results_panel.read_with(cx, |panel, _| panel.table_state().clone());

    for marked in [false, true] {
        cx.update(|_, cx| {
            AppSettings::global_mut(cx)
                .settings
                .results
                .show_invisible_characters = marked;
        })?;
        table_state.update(cx, |state, cx| state.scroll_to_row(0, cx));
        settle(cx, 500).await;
        let suffix = if marked { "-marked" } else { "" };
        tracing::info!("grid measurement: scroll{suffix}");
        for row in 0..100 {
            table_state.update(cx, |state, cx| state.scroll_to_row(row, cx));
            settle(cx, 16).await;
        }
        tracing::info!("grid measurement: redraw{suffix}");
        for _ in 0..100 {
            cx.update(|window, _| window.refresh())?;
            settle(cx, 16).await;
        }
        tracing::info!("grid measurement: done");
        settle(cx, 500).await;
    }
    Ok(())
}

/// Put the caret inside the revenue statement in the active tab, which frames
/// the statement, and focus the editor so the caret shows.
fn place_caret_in_revenue_statement(
    editor_panel: &Entity<EditorPanel>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    let offset = REVENUE_QUERY
        .find(REVENUE_CARET)
        .map_or(0, |ix| ix + REVENUE_CARET.len());
    editor_panel.update_in(cx, |panel, window, cx| {
        if let Some(tab) = panel.active_query_tab() {
            tab.editor.update(cx, |editor, cx| {
                editor.set_selected_range(offset..offset, window, cx);
                editor.focus(window, cx);
            });
        }
    })
}

/// Hide the log of executed statements under the results, so the grid gets
/// the room.
fn hide_sql_log(
    editor_panel: &Entity<EditorPanel>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    editor_panel.update_in(cx, |panel, window, cx| {
        panel.toggle_sql_view_for_active_tab(window, cx);
    })
}

/// The demo connection the seed created, on SQLite's main database.
fn demo_context() -> ConnectionContext {
    ConnectionContext {
        connection_id: 1,
        connection_name: "Groove Records".to_string(),
        db_type: DatabaseType::SQLite,
        database_name: "main".to_string(),
        schema_name: None,
        environment_type: Some(blanco_core::EnvironmentType::DEV),
    }
}

/// Run the active tab and wait for it to finish.
async fn run_active_tab(
    editor_panel: &Entity<EditorPanel>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    editor_panel.update_in(cx, |panel, window, cx| panel.on_run_query(window, cx))?;
    wait_for_run(editor_panel, cx).await
}

/// Wait for the run the active tab started to finish.
async fn wait_for_run(
    editor_panel: &Entity<EditorPanel>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    for _ in 0..200 {
        settle(cx, 50).await;
        if !editor_panel.read_with(cx, |panel, _| panel.is_loading()) {
            return Ok(());
        }
    }
    anyhow::bail!("the run did not finish")
}

/// Write the frame the window shows next to `path`, with the `scene-export`
/// feature; without it there is nothing to write.
async fn export_frame(path: std::path::PathBuf, cx: &mut AsyncWindowContext) {
    #[cfg(all(feature = "scene-export", not(target_os = "windows")))]
    {
        let written = crate::scene_export::request(path);
        cx.update(|window, _| window.refresh()).ok();
        match written.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!("Failed to export the scene: {error:#}"),
            Err(_) => tracing::error!("The scene export was dropped"),
        }
    }
    #[cfg(not(all(feature = "scene-export", not(target_os = "windows"))))]
    let _ = (path, cx);
}

/// Export the frame the window shows next as `<name>.json` in the directory
/// `BLANCO_SCENE_EXPORT` names, if it names one.
async fn export_keyframe(name: &str, cx: &mut AsyncWindowContext) {
    if let Some(dir) = std::env::var_os("BLANCO_SCENE_EXPORT") {
        let path = std::path::Path::new(&dir).join(format!("{name}.json"));
        export_frame(path, cx).await;
    }
}

/// Where an exported keyframe shows `text`: the middle of the first text run
/// holding it, in window coordinates. None without the keyframe.
fn keyframe_text_position(name: &str, text: &str) -> Option<gpui::Point<gpui::Pixels>> {
    let dir = std::env::var_os("BLANCO_SCENE_EXPORT")?;
    let path = std::path::Path::new(&dir).join(format!("{name}.json"));
    let snapshot: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    snapshot["items"].as_array()?.iter().find_map(|item| {
        let run = item["text"].as_str()?;
        let start = run.find(text)?;
        let first = run[..start].chars().count();
        let last = first + text.chars().count() - 1;
        let xs = item["x"].as_array()?;
        let x = (xs.get(first)?.as_f64()? + xs.get(last)?.as_f64()?) / 2.;
        let y = item["y"].as_f64()? - item["size"].as_f64()? * 0.35;
        Some(gpui::point(gpui::px(x as f32), gpui::px(y as f32)))
    })
}

async fn settle(cx: &mut AsyncWindowContext, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}
