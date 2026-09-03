mod object_ddl;
mod parameter_form;
mod query_execution;
#[cfg(test)]
mod query_execution_test;
#[cfg(test)]
mod redis_highlighting_test;
mod rename_form;
mod render;
pub(crate) mod schema_graph;
mod script_execution;
#[cfg(test)]
mod script_execution_test;
mod snippet_editor;
mod sql_operations;
mod tab_access;
mod table_structure;
mod tabs;
mod terminal_pane;

pub use tabs::{
    ConnectionBackedTab, ObjectDdlParams, QueryTab, ScriptTab, SettingsTab, TabCreationParams,
    TabType, TableStructureParams,
};

use blanco_core::ConnectionContext;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable as _, KeybindingKeystroke, Keystroke,
    ParentElement as _, Task, WeakEntity, Window,
};
use gpui_component::{
    ActiveTheme, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{EditorState, InputEvent, TabSize},
    resizable::ResizableState,
};
use std::{collections::HashMap, rc::Rc, sync::Arc};
use tracing::{debug, error, info};

use self::object_ddl::ObjectDdlTab;
use self::snippet_editor::SnippetEditor;
use self::table_structure::TableStructureTab;
use crate::agent::{ChatPanel, ChatProviderResolver, ChatSessionContext};
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;
use crate::script_completion::ScriptCompletionProvider;
use crate::settings::SettingsView;
use crate::sql::{
    SqlCompletionProvider, SqlSelectionRangeProvider, SqlSignatureHelpProvider, SqruffService,
};
use app_database::QueryTabData;
use app_database::{AppDatabase, EnvironmentType};
use blanco_ui::SqlView;
use database::{DatabaseService, DatabaseServiceTrait};

pub struct EditorPanel {
    focus_handle: FocusHandle,
    tabs: Vec<TabType>,
    active_tab_ix: usize,
    tabbar_scroll_handle: gpui::ScrollHandle,
    _subscriptions: Vec<gpui::Subscription>,
    run_query_keystroke: KeybindingKeystroke,
    format_query_keystroke: KeybindingKeystroke,
    editor_chat_resize_state: Entity<ResizableState>,
    /// Kept separate from [`Self::editor_chat_resize_state`] so a script tab's
    /// chat width does not follow a query tab's, and vice versa.
    script_editor_chat_resize_state: Entity<ResizableState>,
    editor_results_resize_state: Entity<ResizableState>,
    results_log_resize_state: Entity<ResizableState>,
    /// Split between a tab's content column and its terminal pane.
    editor_terminal_resize_state: Entity<ResizableState>,
    loading: bool,
    linting_enabled: bool,
    _run_query_task: Task<()>,
    /// Background task that actually drives the database call. Held separately
    /// from `_run_query_task` so an Abort click can drop just the background
    /// future (cancelling the query) while the foreground task keeps running
    /// to report the cancellation back to the UI.
    abort_query_task: Option<Task<()>>,
    /// Cancel flag for the script currently running in a script tab. Scripts
    /// can't be stopped by dropping their task (they own an OS thread), so Stop
    /// raises this flag and the script thread unwinds itself.
    script_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    _lint_debounce_task: Task<()>,
}

/// Events emitted by the editor panel for other parts of the app to react to.
#[derive(Clone, Debug)]
pub enum EditorPanelEvent {
    /// A query finished executing and was written to the history log.
    QueryRecorded,
    /// A Run on a query tab is over, one way or another: `Ok` carries the final
    /// result set as JSON (capped rows), `Err` the failure, the user's cancel,
    /// or why it never started. Emitted for every run so the MCP `run_tab`
    /// bridge can answer its caller; other listeners may ignore it.
    QueryRunEnded {
        tab_index: usize,
        outcome: Result<serde_json::Value, String>,
    },
}

impl gpui::EventEmitter<EditorPanelEvent> for EditorPanel {}

/// Debounce duration for linting (500ms)
const LINT_DEBOUNCE_MS: u64 = 500;

/// Maximum length of SQL query to log in the SQL log panel
/// Queries longer than this will be truncated to avoid performance issues
const SQL_QUERY_LOG_MAX_LENGTH: usize = 2000;

/// Highlighter language used by script tabs, for both the editor and its log.
pub(crate) const SCRIPT_EDITOR_LANGUAGE: &str = "javascript";

/// Console output can be chatty, so a script's log keeps more scrollback than
/// the query log's handful of statements.
const SCRIPT_LOG_MAX_LINES: usize = 500;

/// Starting content of a new script tab: a short reference for the injected
/// `db` API, since it exists nowhere else in the UI.
const DEFAULT_SCRIPT: &str = r#"// JavaScript runs against the tab's connection through `db`.
// Every call is synchronous and blocks until the database answers.
//
//   db.query(sql, params?)   -> { columns, rows: [{column: value|null}], rowsAffected, executionTimeMs }
//   db.execute(sql, params?) -> number of affected rows
//   db.transaction([sql, …]) -> { rowsAffected, operationsExecuted }
//   db.display(value)        -> render a result (or any array/object) in the grid
//   console.log / warn / error write to the log below.

const tables = db.query("SELECT 1 AS example");
console.log("rows:", tables.rows.length);
db.display(tables);
"#;

/// The effective keystroke for an action, for tooltips. Falls back to the
/// built-in default when the user's override does not parse, and to an empty
/// keystroke when the action is unbound.
fn keystroke_hint(action: &str, cx: &App) -> KeybindingKeystroke {
    let settings = &AppSettings::global(cx).settings;
    let keystrokes = crate::keybindings::effective_keystroke(settings, action);
    let chord = keystrokes
        .split_whitespace()
        .next()
        .unwrap_or(crate::keybindings::default_keystroke(action));
    let keystroke = Keystroke::parse(chord)
        .or_else(|_| Keystroke::parse(crate::keybindings::default_keystroke(action)))
        .unwrap_or_default();
    KeybindingKeystroke::from_keystroke(keystroke)
}

impl EditorPanel {
    /// The results panel of the currently active query tab, if any.
    pub fn active_results_panel(&self) -> Option<Entity<ResultsPanel>> {
        self.tabs
            .get(self.active_tab_ix)
            .and_then(TabType::query)
            .map(|tab| tab.results_panel.clone())
    }

    #[cfg(test)]
    pub fn active_query_tab(&self) -> Option<&QueryTab> {
        self.tabs.get(self.active_tab_ix).and_then(TabType::query)
    }

    #[cfg(test)]
    pub fn active_script_tab(&self) -> Option<&ScriptTab> {
        self.tabs.get(self.active_tab_ix).and_then(TabType::script)
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The code editors of every connection-backed tab (query and script), the
    /// ones that follow the global editor settings.
    fn connection_backed_editors(&self) -> Vec<Entity<EditorState>> {
        self.tabs
            .iter()
            .filter_map(TabType::connection_tab)
            .map(|tab| tab.editor.clone())
            .collect()
    }

    pub fn set_all_editors_show_whitespace(
        &mut self,
        show: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for editor in self.connection_backed_editors() {
            editor.update(cx, |state, cx| state.set_show_whitespaces(show, window, cx));
        }
    }

    pub fn set_all_editors_soft_wrap(
        &mut self,
        wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for editor in self.connection_backed_editors() {
            editor.update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
        }
    }

    fn close_tab(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        if tab_index < self.tabs.len() && self.tabs.len() > 1 {
            // Get the db_id before removing the tab
            let db_id = self.tabs.get(tab_index).and_then(TabType::db_id);

            // Remove tab from UI
            self.tabs.remove(tab_index);

            // Adjust active tab index so the same tab stays active when one
            // before it is closed.
            if tab_index < self.active_tab_ix {
                self.active_tab_ix -= 1;
            } else if self.active_tab_ix >= self.tabs.len() {
                self.active_tab_ix = self.tabs.len().saturating_sub(1);
            }

            // Delete from database if it has a db_id
            if let Some(db_id) = db_id {
                let app_database = AppDatabase::global(cx).clone();
                cx.spawn(async move |_, _cx| {
                    app_database
                        .delete_query_tab(db_id)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to delete tab: {}", e))
                })
                .detach();
            }

            cx.notify();
        }
    }

    pub fn rename_tab(&mut self, tab_index: usize, new_name: &str, cx: &mut Context<Self>) {
        let renamed = self
            .tabs
            .get_mut(tab_index)
            .and_then(TabType::connection_tab_mut)
            .map(|tab| {
                let old_name = std::mem::replace(&mut tab.title, new_name.to_string());
                (old_name, tab.db_id)
            });

        if let Some((old_name, db_id)) = renamed {
            // Update database if this tab has a db_id
            if let Some(db_id) = db_id {
                let app_database = AppDatabase::global(cx).clone();
                let new_name = new_name.to_string(); // Convert to owned String

                cx.spawn(async move |_, _cx| {
                    // Load existing tab data to preserve all fields
                    if let Ok(Some(existing_tab)) = app_database.load_query_tab_by_id(db_id).await {
                        let mut updated_tab = existing_tab;
                        updated_tab.title = new_name;

                        if let Err(e) = app_database.save_query_tab(&updated_tab).await {
                            tracing::error!("Failed to update tab name in database: {}", e);
                        }
                    } else {
                        tracing::error!("Failed to load existing tab data for tab ID: {}", db_id);
                    }
                })
                .detach();
            }

            tracing::info!(
                "Tab {} renamed from '{}' to '{}'",
                tab_index,
                old_name,
                new_name
            );
            cx.notify();
        }
    }

    pub fn add_settings_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Check if settings tab already exists
        if self
            .tabs
            .iter()
            .any(|tab| matches!(tab, TabType::Settings(_)))
        {
            // Find and focus existing settings tab
            for (i, tab) in self.tabs.iter().enumerate() {
                if matches!(tab, TabType::Settings(_)) {
                    self.active_tab_ix = i;
                    cx.notify();
                    return;
                }
            }
            return;
        }

        // Settings are now stored in the global AppDatabase
        let settings_view = cx.new(SettingsView::new);

        let settings_tab = SettingsTab {
            title: "Settings".to_string(),
            settings_view,
        };

        self.tabs.push(TabType::Settings(settings_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    pub fn create_snippet_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // New snippets start as queries; the footer toggle switches them.
        let snippet_editor =
            cx.new(|cx| SnippetEditor::new(app_database::EditorKind::Query, window, cx));

        self.tabs.push(TabType::Snippet(snippet_editor));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    pub fn open_snippet_tab(
        &mut self,
        snippet_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Load the snippet first so the editor can be built with the right
        // language rather than built as SQL and immediately rebuilt.
        let app_database = AppDatabase::global(cx).clone();
        let load = gpui_tokio::Tokio::spawn_result(cx, async move {
            app_database
                .get_snippet_by_id(snippet_id)
                .await
                .map_err(anyhow::Error::from)
        });
        cx.spawn_in(window, async move |this, cx| {
            let snippet_data = load.await.log_err().flatten();
            this.update_in(cx, |this, window, cx| {
                let kind = snippet_data
                    .as_ref()
                    .map(|snippet| snippet.kind)
                    .unwrap_or_default();
                let snippet_editor = cx.new(|cx| SnippetEditor::new(kind, window, cx));

                if let Some(snippet_data) = snippet_data {
                    snippet_editor.update(cx, |editor, cx| {
                        editor.load_snippet(snippet_data, window, cx);
                    });
                }

                this.tabs.push(TabType::Snippet(snippet_editor));
                this.active_tab_ix = this.tabs.len() - 1;
                this.scroll_tabbar_to_the_end(window, cx);
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    pub fn create_table_structure_tab(
        &mut self,
        params: TableStructureParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WeakEntity<TableStructureTab> {
        let tab = cx.new(|cx| {
            TableStructureTab::new(
                params.context,
                params.table_name,
                Vec::new(),
                Vec::new(),
                window,
                cx,
            )
        });

        let weak = tab.downgrade();
        self.tabs.push(TabType::TableStructure(tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
        weak
    }

    pub fn create_object_ddl_tab(
        &mut self,
        params: ObjectDdlParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WeakEntity<ObjectDdlTab> {
        let tab = cx.new(|cx| {
            ObjectDdlTab::new(params.kind, params.context, params.object_name, window, cx)
        });
        let weak = tab.downgrade();
        self.tabs.push(TabType::ObjectDdl(tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
        weak
    }

    pub fn create_schema_graph_tab(
        &mut self,
        params: schema_graph::SchemaGraphParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WeakEntity<schema_graph::SchemaGraphTab> {
        let tab = cx.new(|cx| schema_graph::SchemaGraphTab::new(params.context, window, cx));
        let weak = tab.downgrade();
        self.tabs.push(TabType::SchemaGraph(tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
        weak
    }

    /// Show or hide the terminal pane docked under the active tab. The shell
    /// is spawned on first use and kept while hidden; when it exits the pane
    /// closes and the next toggle starts a fresh shell. The MCP server is
    /// started first so the agents launched from the pane find it.
    pub fn toggle_terminal_for_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self
            .tabs
            .get_mut(self.active_tab_ix)
            .and_then(TabType::connection_tab_mut)
        else {
            return;
        };
        tab.terminal_enabled = !tab.terminal_enabled;
        if !tab.terminal_enabled {
            cx.notify();
            return;
        }
        if tab.terminal.is_none() {
            if !crate::mcp::McpService::global(cx).is_running() {
                crate::mcp::McpService::start(cx);
            }
            let tab_id = tab.editor.entity_id().as_u64();
            let terminal = match terminal_pane::spawn_tab_terminal(tab_id, &tab.context, window, cx)
            {
                Ok(terminal) => terminal,
                Err(error) => {
                    error!("Failed to start a terminal: {error:#}");
                    tab.terminal_enabled = false;
                    window.push_notification(
                        (
                            gpui_component::notification::NotificationType::Error,
                            format!("Could not start a terminal: {error:#}"),
                        ),
                        cx,
                    );
                    return;
                }
            };
            let view = cx.new(|cx| {
                blanco_terminal::view::TerminalView::new(
                    terminal,
                    terminal_pane::style_from_theme,
                    window,
                    cx,
                )
            });
            let subscription = cx.subscribe(&view, |panel, exited_view, event, cx| {
                if let blanco_terminal::view::TerminalViewEvent::Exited = event {
                    for tab in panel
                        .tabs
                        .iter_mut()
                        .filter_map(TabType::connection_tab_mut)
                    {
                        if tab
                            .terminal
                            .as_ref()
                            .is_some_and(|pane| pane.view == exited_view)
                        {
                            tab.terminal = None;
                            tab.terminal_enabled = false;
                        }
                    }
                    cx.notify();
                }
            });
            tab.terminal = Some(terminal_pane::TerminalPane {
                view,
                _subscription: subscription,
            });
        }
        if let Some(pane) = &tab.terminal {
            let focus_handle = pane.view.read(cx).focus_handle(cx);
            window.focus(&focus_handle, cx);
        }
        cx.notify();
    }

    /// Titles of the open tabs in display order, for the command palette.
    pub fn tab_titles(&self, cx: &App) -> Vec<String> {
        self.tabs.iter().map(|tab| tab.title(cx)).collect()
    }

    pub fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_active_tab(ix, window, cx);
    }

    pub fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        self.close_tab(self.active_tab_ix, cx);
    }

    fn set_active_tab(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.active_tab_ix = ix;
            self.tabbar_scroll_handle.scroll_to_item(ix);
            cx.notify();
        }
    }

    /// Load saved query tabs from the app database
    /// This should be called from BlancoApp initialization
    pub fn new_with_saved_tabs(
        window: &mut Window,
        cx: &mut Context<Self>,
        saved_tabs: Vec<QueryTabData>,
    ) -> Self {
        info!("Loading {} saved tabs", saved_tabs.len());

        let editor_chat_resize_state = cx.new(|_| ResizableState::default());
        let script_editor_chat_resize_state = cx.new(|_| ResizableState::default());
        let editor_results_resize_state = cx.new(|_| ResizableState::default());
        let results_log_resize_state = cx.new(|_| ResizableState::default());
        let editor_terminal_resize_state = cx.new(|_| ResizableState::default());

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![],
            active_tab_ix: 0,
            tabbar_scroll_handle: gpui::ScrollHandle::default(),
            _subscriptions: Vec::new(),
            run_query_keystroke: keystroke_hint("RunQuery", cx),
            format_query_keystroke: keystroke_hint("FormatQuery", cx),
            editor_chat_resize_state,
            script_editor_chat_resize_state,
            editor_results_resize_state,
            results_log_resize_state,
            editor_terminal_resize_state,
            loading: false,
            linting_enabled: false,
            _run_query_task: Task::ready(()),
            abort_query_task: None,
            script_cancel: None,
            _lint_debounce_task: Task::ready(()),
        };

        panel.restore_saved_tabs_with_connections_sync(saved_tabs, window, cx);

        // Enable linting after a short delay so that initial selection range events
        // from restored tabs don't trigger linting for every tab on startup.
        cx.spawn(async move |entity_handle, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(LINT_DEBOUNCE_MS + 200))
                .await;
            entity_handle
                .update(cx, |this, _cx| {
                    this.linting_enabled = true;
                })
                .log_err();
        })
        .detach();

        // React to AppSettings changes (e.g. from SettingsView)
        let settings_subscription =
            cx.observe_global_in::<AppSettings>(window, |this, window, cx| {
                let word_wrap = AppSettings::global(cx).settings.editor.word_wrap;
                let show_whitespace = AppSettings::global(cx).settings.editor.show_whitespace;
                this.set_all_editors_soft_wrap(word_wrap, window, cx);
                this.set_all_editors_show_whitespace(show_whitespace, window, cx);
            });
        panel._subscriptions.push(settings_subscription);

        panel
    }

    /// Restore saved tabs after connections have been loaded (synchronous version)
    /// This function matches saved tabs with actual connections and only restores valid ones
    pub fn restore_saved_tabs_with_connections_sync(
        &mut self,
        saved_tabs: Vec<QueryTabData>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if saved_tabs.is_empty() {
            debug!("No saved tabs to restore");
            return;
        }

        info!(
            "Attempting to restore {} saved tabs with connection matching",
            saved_tabs.len()
        );

        let mut restored_count = 0;
        let total_tabs = saved_tabs.len();

        for tab_data in saved_tabs {
            debug!(
                "Processing tab '{}' (connection_id: {:?}, connection_type: {:?})",
                tab_data.title, tab_data.connection_id, tab_data.connection_type
            );

            // Try to find matching connection
            if let Some(_connection_id) = tab_data.connection_id {
                let tab_title = tab_data.title.clone();
                let tab_content = tab_data.content.clone();
                let tab_db_id = tab_data.id;
                let tab_connection_type = tab_data.connection_type.clone();

                // Convert connection_type string to DatabaseType enum
                let tab_db_type = tab_connection_type
                    .as_ref()
                    .and_then(|t| database::DatabaseType::from_db_type_str(t))
                    .unwrap_or(database::DatabaseType::PostgreSQL);

                let params = TabCreationParams {
                    title: tab_title,
                    content: Some(tab_content),
                    db_id: tab_db_id,
                    last_run_at: tab_data.last_run_at,
                    context: ConnectionContext {
                        connection_id: _connection_id,
                        db_type: tab_db_type,
                        connection_name: tab_data.connection_name.clone().unwrap_or_default(),
                        database_name: tab_data
                            .database_name
                            .clone()
                            .unwrap_or_else(|| "default".to_string()),
                        schema_name: tab_data.schema_name.clone(),
                        environment_type: tab_data.environment_type,
                    },
                };
                if tab_data.tab_kind == app_database::EditorKind::Script {
                    self.create_and_add_script_tab(window, params, cx);
                } else {
                    self.create_and_add_tab_with_connection(window, params, cx);
                }
                restored_count += 1;
            } else {
                debug!(
                    "Tab '{}' has no connection_id, skipping tab",
                    tab_data.title
                );
            }
        }

        info!(
            "Successfully restored {} out of {} saved tabs",
            restored_count, total_tabs
        );
    }

    /// Helper to create a tab with a specific connection
    pub fn create_and_add_tab_with_connection(
        &mut self,
        window: &mut Window,
        params: TabCreationParams,
        cx: &mut Context<Self>,
    ) {
        // Create completion provider outside the entity closure so it can be stored on QueryTab
        let db_service: Arc<dyn DatabaseServiceTrait> =
            Arc::new(DatabaseService::global(cx).clone());
        let sql_completion_provider = SqlCompletionProvider::new(
            params.context.connection_id,
            params.context.database_name.clone(),
            params.context.schema_name.clone(),
            params.context.db_type,
            db_service,
        );

        // SQL completions and statement-based selection ranges only make sense
        // for SQL backends. Line-oriented backends (Redis) get neither.
        let supports_sql = params.context.db_type.supports_sql();

        let editor = cx.new({
            let sql_completion_provider = sql_completion_provider.clone();
            |cx| {
                // Read settings
                let editor_settings = &AppSettings::global(cx).settings.editor;
                let word_wrap = editor_settings.word_wrap;
                let show_whitespace = editor_settings.show_whitespace;
                let folding = editor_settings.folding;
                let hard_tabs = editor_settings.hard_tabs;
                let tab_size = editor_settings.tab_size;

                let mut editor = EditorState::new(window, cx)
                    .language(params.context.db_type.editor_language().to_string())
                    .line_number(true)
                    .folding(folding)
                    .tab_size(TabSize {
                        tab_size: tab_size as usize,
                        hard_tabs,
                    })
                    .soft_wrap(word_wrap)
                    .show_whitespaces(show_whitespace);

                if supports_sql {
                    let signature_help_provider: Rc<
                        dyn gpui_component::input::SignatureHelpProvider,
                    > = Rc::new(SqlSignatureHelpProvider::new(
                        sql_completion_provider.clone(),
                    ));
                    editor.lsp_mut().signature_help_provider = Some(signature_help_provider);

                    let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                        Rc::new(sql_completion_provider);
                    editor.lsp_mut().completion_provider = Some(completion_provider);

                    // Set up selection range provider for SQL statement highlighting
                    let provider = SqlSelectionRangeProvider::new();
                    let selection_range_provider: Rc<
                        dyn gpui_component::input::SelectionRangeProvider,
                    > = Rc::new(provider);
                    editor.lsp_mut().selection_range_provider = Some(selection_range_provider);
                } else if params.context.db_type == database::DatabaseType::Redis {
                    // Redis gets command/subcommand completion instead of the
                    // SQL schema-aware completion.
                    let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                        Rc::new(crate::redis_completion::RedisCompletionProvider::new());
                    editor.lsp_mut().completion_provider = Some(completion_provider);
                }

                editor
            }
        });

        // Set content if provided
        if let Some(content) = params.content {
            editor.update(cx, |state, cx| {
                state.replace(&content, window, cx);
            });
        }

        // Subscribe to editor text changes for auto-linting
        let subscription = cx.subscribe_in(&editor, window, |this, _editor, event, window, cx| {
            if let InputEvent::SelectionRangeChange { range } = event {
                if this.linting_enabled {
                    this.lint_current_query_debounced(*range, cx);
                }
                // Re-render so the Ln/Col indicator in the action bar tracks the cursor.
                cx.notify();
            } else if let InputEvent::PressEnter { secondary, .. } = event
                && *secondary
            {
                this.on_run_query(window, cx);
            }
        });
        self._subscriptions.push(subscription);

        // Create SqruffService for this tab. Sqruff drives SQL formatting and
        // linting only, so non-SQL backends (Redis) skip it entirely.
        let formatter_settings = AppSettings::global(cx).settings.formatter.clone();
        let editor_settings = AppSettings::global(cx).settings.editor.clone();
        let sqruff_service = if supports_sql {
            match SqruffService::new(
                params.context.db_type.to_sqruff_dialect(),
                &formatter_settings,
                &editor_settings,
            ) {
                Ok(service) => Some(Arc::new(service)),
                Err(e) => {
                    error!(
                        "Failed to create SqruffService for tab '{}': {}",
                        params.title, e
                    );
                    None
                }
            }
        } else {
            None
        };

        // Create query tab with the connection string
        let results_panel = cx.new(|cx| {
            ResultsPanel::new(
                params.context.connection_id,
                &params.context.database_name,
                params.context.db_type,
                window,
                cx,
            )
        });
        // Re-render the editor when the results panel notifies, so toolbar
        // buttons that depend on panel state (e.g. Apply/Discard edits) stay
        // in sync.
        self._subscriptions
            .push(cx.observe(&results_panel, |_, _, cx| cx.notify()));
        let query_tab = QueryTab {
            base: ConnectionBackedTab {
                title: params.title.clone(),
                context: params.context.clone(),
                editor: editor.clone(),
                db_id: params.db_id,
                last_run_at: params.last_run_at,
                results_panel,
                chat_enabled: false,
                terminal_enabled: false,
                terminal: None,
                chat_panel: None,
            },
            sql_view: cx.new(|cx| {
                SqlView::new(
                    10,
                    cx.theme().highlight_theme.clone(),
                    params.context.db_type.editor_language(),
                )
                .show_copy_button(false)
            }),
            commit_preview: cx
                .new(|cx| SqlView::new(usize::MAX, cx.theme().highlight_theme.clone(), "sql")),
            sqruff_service,
            completion_provider: supports_sql.then_some(sql_completion_provider),
            sql_view_visible: true,
            last_parameter_values: HashMap::new(),
        };

        self.tabs.push(TabType::Query(Box::new(query_tab)));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    /// Create a JavaScript script tab bound to the same connection identity as
    /// a query tab. Scripts drive the database through the injected `db` object
    /// rather than through the SQL pipeline, so this deliberately skips the
    /// completion, selection-range, formatter and linter wiring.
    pub fn create_and_add_script_tab(
        &mut self,
        window: &mut Window,
        params: TabCreationParams,
        cx: &mut Context<Self>,
    ) {
        let editor_settings = AppSettings::global(cx).settings.editor.clone();
        let db_type = params.context.db_type;
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language(SCRIPT_EDITOR_LANGUAGE.to_string())
                .line_number(true)
                .folding(editor_settings.folding)
                .tab_size(TabSize {
                    tab_size: editor_settings.tab_size as usize,
                    hard_tabs: editor_settings.hard_tabs,
                })
                .soft_wrap(editor_settings.word_wrap)
                .show_whitespaces(editor_settings.show_whitespace);

            // The injected `db` object is the only API a script has that the
            // editor can't infer, so that is what gets completed.
            let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                Rc::new(ScriptCompletionProvider::new(db_type));
            editor.lsp_mut().completion_provider = Some(completion_provider);
            editor
        });

        let content = params.content.unwrap_or_else(|| DEFAULT_SCRIPT.to_string());
        editor.update(cx, |state, cx| {
            state.replace(&content, window, cx);
        });

        let subscription = cx.subscribe_in(&editor, window, |this, _editor, event, window, cx| {
            if let InputEvent::SelectionRangeChange { .. } = event {
                if this.linting_enabled {
                    this.lint_current_script_debounced(cx);
                }
                cx.notify();
            } else if let InputEvent::PressEnter { secondary, .. } = event
                && *secondary
            {
                this.on_run_query(window, cx);
            }
        });
        self._subscriptions.push(subscription);

        let results_panel = cx.new(|cx| {
            ResultsPanel::new(
                params.context.connection_id,
                &params.context.database_name,
                params.context.db_type,
                window,
                cx,
            )
        });
        self._subscriptions
            .push(cx.observe(&results_panel, |_, _, cx| cx.notify()));

        let script_tab = ScriptTab {
            base: ConnectionBackedTab {
                title: params.title,
                context: params.context,
                editor,
                db_id: params.db_id,
                last_run_at: params.last_run_at,
                results_panel,
                chat_enabled: false,
                terminal_enabled: false,
                terminal: None,
                chat_panel: None,
            },
            log_view: cx.new(|cx| {
                SqlView::new(
                    SCRIPT_LOG_MAX_LINES,
                    cx.theme().highlight_theme.clone(),
                    SCRIPT_EDITOR_LANGUAGE,
                )
                .show_copy_button(false)
            }),
            log_visible: true,
        };

        self.tabs.push(TabType::Script(Box::new(script_tab)));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    fn scroll_tabbar_to_the_end(&self, window: &mut Window, _: &mut Context<Self>) {
        let scroll_handle = self.tabbar_scroll_handle.clone();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, _| {
                let max_offset = scroll_handle.max_offset();
                scroll_handle.set_offset(gpui::point(-max_offset.x, gpui::px(0.0)));
            })
        });
    }

    /// Cancel the query currently running for the active tab. Drops the
    /// background task driving the database call, which (for sqlx-backed
    /// drivers) cancels the in-flight statement on the next yield point. The
    /// foreground task observes the dropped sender and reports the cancel.
    pub fn abort_running_query(&mut self, cx: &mut Context<Self>) {
        if self.abort_query_task.take().is_some() {
            cx.notify();
        }
    }

    /// Commit current changes in the active tab's results panel
    pub fn commit_current_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(TabType::Query(query_tab)) = self.tabs.get(self.active_tab_ix) else {
            return;
        };
        if query_tab.context.environment_type == Some(EnvironmentType::Prod) {
            let connection_name = query_tab.context.connection_name.clone();
            self.confirm_prod_write(
                "Commit edits to PROD?",
                format!(
                    "\"{connection_name}\" is tagged as a production connection. The pending grid edits will be written to it."
                ),
                "Commit to PROD",
                |panel, window, cx| panel.commit_current_changes_unchecked(window, cx),
                window,
                cx,
            );
            return;
        }
        self.commit_current_changes_unchecked(window, cx);
    }

    /// Open a confirmation dialog for a write against a PROD connection and
    /// run `on_confirm` on this panel when the user accepts.
    pub(crate) fn confirm_prod_write(
        &mut self,
        title: &'static str,
        message: String,
        confirm_label: &'static str,
        on_confirm: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_panel = cx.entity().downgrade();
        let on_confirm = Rc::new(on_confirm);
        window.open_dialog(cx, move |dialog, _, _| {
            let weak_panel = weak_panel.clone();
            let on_confirm = on_confirm.clone();
            dialog.title(title).child(message.clone()).footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new()
                            .child(Button::new("prod-write-cancel").label("Cancel").outline()),
                    )
                    .child(
                        DialogAction::new().child(
                            Button::new("prod-write-confirm")
                                .danger()
                                .label(confirm_label)
                                .on_click(move |_, window, cx| {
                                    let on_confirm = on_confirm.clone();
                                    weak_panel
                                        .update(cx, |panel, cx| on_confirm(panel, window, cx))
                                        .log_err();
                                }),
                        ),
                    ),
            )
        });
    }

    fn commit_current_changes_unchecked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            let changes = query_tab
                .results_panel
                .update(cx, |panel, cx| panel.commit_all_edits(cx));

            if changes.is_empty() {
                window.push_notification("No changes to commit", cx);
                return;
            }

            // Execute the commit in the results panel with SQL logging
            query_tab.results_panel.update(cx, |panel, cx| {
                panel.commit_changes_with_sql_view(window, &query_tab.sql_view, cx);
            });

            window.push_notification(format!("Committing {} changes", changes.len()), cx);
        }
    }

    /// Rollback current changes in the active tab's results panel
    pub fn rollback_current_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.results_panel.update(cx, |panel, cx| {
                panel.cancel_all_edits(cx);
            });

            window.push_notification("Changes rolled back", cx);
        }
    }

    /// Toggle chat for the active tab
    pub fn toggle_chat_for_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The two tab kinds differ only in which fields hold the connection
        // identity and what language the tab's buffer is in; the panel itself
        // is built the same way, so both branches go through
        // `create_chat_panel`.
        let Some(active) = self.tabs.get_mut(self.active_tab_ix) else {
            return;
        };
        let Some(language) = active.tab_language() else {
            return;
        };
        let Some(tab) = active.connection_tab_mut() else {
            return;
        };
        tab.chat_enabled = !tab.chat_enabled;
        if !tab.chat_enabled || tab.chat_panel.is_some() {
            cx.notify();
            return;
        }
        let session_context = tab.chat_session_context(language);
        let enabled = &mut tab.chat_enabled;
        let chat_panel = &mut tab.chat_panel;
        match Self::create_chat_panel(session_context, window, cx) {
            Ok(panel) => *chat_panel = Some(panel),
            Err(error) => {
                tracing::error!(
                    "Failed to create chat provider: {error}. Not creating chat panel."
                );
                *enabled = false;
            }
        }

        cx.notify();
    }

    fn create_chat_panel(
        session_context: ChatSessionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Entity<ChatPanel>> {
        let llm_instance = ChatProviderResolver::get_llm_for_connection(cx)?;
        Ok(cx.new(|cx| {
            ChatPanel::new(
                llm_instance.llm.clone(),
                llm_instance.provider_name.clone(),
                llm_instance.model_name.clone(),
                session_context,
                window,
                cx,
            )
        }))
    }

    pub fn toggle_sql_view_for_active_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.sql_view_visible = !query_tab.sql_view_visible;
            cx.notify();
        }
    }

    /// Insert text at the cursor of the active query tab's editor and focus it.
    /// Returns `true` when an active query tab received the text.
    pub fn insert_into_active_query(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self
            .tabs
            .get(self.active_tab_ix)
            .and_then(TabType::connection_tab)
            .map(|tab| tab.editor.clone())
        else {
            return false;
        };
        editor.update(cx, |state, cx| {
            state.focus(window, cx);
            state.insert(text, window, cx);
        });
        cx.notify();
        true
    }
}
