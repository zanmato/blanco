//! Stages the app for the website screenshots. Built only with the
//! `screenshots` feature and driven by `scripts/screenshots/capture`, which
//! runs Blanco on a virtual display against a throwaway data directory.
//!
//! `BLANCO_SCENE` names the scene to stage, `BLANCO_DEMO_DB` the SQLite file
//! the demo connection opens, `BLANCO_THEME` the theme and `BLANCO_SCENE_READY`
//! the file written once the window shows the scene.

use std::time::Duration;

use app_database::{
    AppDatabase, ConnectionData, EditorKind, QueryTabData, SnippetData, WindowGeometry,
};
use blanco_core::ddl::{ColumnOperation, ColumnSpec, IndexOperation};
use blanco_core::{ConnectionContext, DatabaseType};
use gpui::{App, AppContext, AsyncWindowContext, Entity, Window};
use tokio::runtime::Handle;

use crate::app::{
    BlancoApp, ConnectToConnection, OpenSchemaGraph, OpenSettings, OpenTableStructure,
    ShowQueryInNewTab, SidebarTab, ToggleCommandPalette,
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
            | Self::JsonEdit => 0,
            Self::InlineEdit | Self::LargeValues | Self::Filter => 1,
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
            })
            .await?;
    }

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

/// Stage `scene` in the window that holds `app`, then write the ready file.
pub(crate) fn stage(scene: Scene, app: Entity<BlancoApp>, window: &Window, cx: &App) {
    window
        .spawn(cx, async move |cx| {
            if let Err(error) = stage_scene(scene, &app, cx).await {
                tracing::error!("Failed to stage the screenshot scene: {error:#}");
                return;
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
                    ColumnOperation::Add(ColumnSpec {
                        name: "label".to_string(),
                        data_type: "TEXT".to_string(),
                        nullable: true,
                        default: None,
                    }),
                    window,
                    cx,
                );
                tab.stage_index(
                    IndexOperation::Create {
                        name: "albums_label_idx".to_string(),
                        columns: vec!["label".to_string()],
                        unique: false,
                    },
                    window,
                    cx,
                );
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
        environment_type: None,
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

async fn settle(cx: &mut AsyncWindowContext, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}
