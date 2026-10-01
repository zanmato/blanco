//! Stages the app for the website screenshots. Built only with the
//! `screenshots` feature and driven by `scripts/screenshots/capture`, which
//! runs Blanco on a virtual display against a throwaway data directory.
//!
//! `BLANCO_SCENE` names the scene to stage, `BLANCO_DEMO_DB` the SQLite file
//! the demo connection opens, `BLANCO_THEME` the theme and `BLANCO_SCENE_READY`
//! the file written once the window shows the scene.

use std::time::Duration;

use app_database::{AppDatabase, ConnectionData, EditorKind, QueryTabData, SnippetData};
use gpui::{App, AsyncWindowContext, Entity, Window};
use tokio::runtime::Handle;

use crate::app::{BlancoApp, ConnectToConnection, SidebarTab};
use crate::editor::EditorPanel;
use crate::results_panel::CellInput;

const WINDOW_WIDTH: &str = "1600";
const WINDOW_HEIGHT: &str = "1000";

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
    /// The terminal pane docked under a query tab, running Claude Code.
    Terminal,
    /// A script that has run, with its log and the grid it displayed.
    Script,
    /// The snippets sidebar with a snippet open in the editor.
    Snippets,
}

impl Scene {
    pub(crate) fn from_env() -> Option<Self> {
        let scene = std::env::var("BLANCO_SCENE").ok()?;
        match scene.as_str() {
            "editor" => Some(Self::Editor),
            "inline-edit" => Some(Self::InlineEdit),
            "terminal" => Some(Self::Terminal),
            "script" => Some(Self::Script),
            "snippets" => Some(Self::Snippets),
            _ => {
                tracing::error!("Unknown BLANCO_SCENE {scene:?}");
                None
            }
        }
    }

    /// The editor tab the scene brings to the front, in seeding order.
    fn tab_index(self) -> usize {
        match self {
            Self::Editor | Self::Terminal | Self::Snippets => 0,
            Self::InlineEdit => 1,
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
        ("window.x", "0"),
        ("window.y", "0"),
        ("window.width", WINDOW_WIDTH),
        ("window.height", WINDOW_HEIGHT),
        ("window.maximized", "false"),
        ("mcp.enabled", "false"),
    ] {
        database.save_setting(key, value, false).await?;
    }

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
    }

    settle(cx, 1500).await;
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

/// Run the active tab and wait for it to finish.
async fn run_active_tab(
    editor_panel: &Entity<EditorPanel>,
    cx: &mut AsyncWindowContext,
) -> anyhow::Result<()> {
    editor_panel.update_in(cx, |panel, window, cx| panel.on_run_query(window, cx))?;
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
