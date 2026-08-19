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
mod table_structure;
mod tabs;

pub use tabs::{
    ObjectDdlParams, QueryTab, ScriptTab, SettingsTab, TabCreationParams, TabType,
    TableStructureParams,
};

use blanco_core::{ColumnInfo, IndexInfo};
use gpui::{
    AppContext, Context, Entity, FocusHandle, KeybindingKeystroke, Keystroke, Task, WeakEntity,
    Window,
};
use gpui_component::{
    ActiveTheme, WindowExt as _,
    input::{EditorState, InputEvent, TabSize},
    resizable::ResizableState,
};
use std::{collections::HashMap, rc::Rc, sync::Arc};
use tracing::{debug, error, info};

use self::object_ddl::ObjectDdlTab;
use self::snippet_editor::SnippetEditor;
use self::table_structure::TableStructureTab;
use crate::agent::{ChatPanel, ChatProviderResolver, ChatSessionContext, TabLanguage};
use crate::app_database::AppDatabase;
use crate::app_database::QueryTabData;
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;
use crate::script_completion::ScriptCompletionProvider;
use crate::settings::SettingsView;
use crate::sql::{SqlCompletionProvider, SqlSelectionRangeProvider, SqruffService};
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

impl EditorPanel {
    /// The results panel of the currently active query tab, if any.
    pub fn active_results_panel(&self) -> Option<Entity<ResultsPanel>> {
        match self.tabs.get(self.active_tab_ix) {
            Some(TabType::Query(tab)) => Some(tab.results_panel.clone()),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn active_query_tab(&self) -> Option<&QueryTab> {
        match self.tabs.get(self.active_tab_ix) {
            Some(TabType::Query(tab)) => Some(tab),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn active_script_tab(&self) -> Option<&ScriptTab> {
        match self.tabs.get(self.active_tab_ix) {
            Some(TabType::Script(tab)) => Some(tab),
            _ => None,
        }
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
            .filter_map(|tab| match tab {
                TabType::Query(query_tab) => Some(query_tab.editor.clone()),
                TabType::Script(script_tab) => Some(script_tab.editor.clone()),
                _ => None,
            })
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
            let db_id = match self.tabs.get(tab_index) {
                Some(TabType::Query(query_tab)) => query_tab.db_id,
                Some(TabType::Script(script_tab)) => script_tab.db_id,
                _ => None,
            };

            // Remove tab from UI
            self.tabs.remove(tab_index);

            // Adjust active tab index
            if self.active_tab_ix >= self.tabs.len() {
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
        let renamed = match self.tabs.get_mut(tab_index) {
            Some(TabType::Query(query_tab)) => {
                let old_name = std::mem::replace(&mut query_tab.title, new_name.to_string());
                Some((old_name, query_tab.db_id))
            }
            Some(TabType::Script(script_tab)) => {
                let old_name = std::mem::replace(&mut script_tab.title, new_name.to_string());
                Some((old_name, script_tab.db_id))
            }
            _ => None,
        };

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
            cx.new(|cx| SnippetEditor::new(crate::app_database::EditorKind::Query, window, cx));

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
        let app_database = AppDatabase::global(cx);
        let snippet_data = gpui_tokio::Tokio::handle(cx)
            .block_on(async { app_database.get_snippet_by_id(snippet_id).await })
            .map_err(anyhow::Error::from)
            .log_err()
            .flatten();

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

        self.tabs.push(TabType::Snippet(snippet_editor));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    pub fn create_table_structure_tab(
        &mut self,
        params: TableStructureParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = cx.new(|cx| {
            TableStructureTab::new(
                params.connection_id,
                params.db_type,
                Some(params.connection_name),
                params.database_name,
                params.schema_name,
                params.table_name,
                params.environment_type,
                Vec::new(),
                Vec::new(),
                window,
                cx,
            )
        });

        self.tabs.push(TabType::TableStructure(tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    pub fn create_object_ddl_tab(
        &mut self,
        params: ObjectDdlParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WeakEntity<ObjectDdlTab> {
        let tab = cx.new(|cx| {
            ObjectDdlTab::new(
                params.kind,
                params.connection_id,
                params.db_type,
                Some(params.connection_name),
                params.database_name,
                params.schema_name,
                params.object_name,
                params.environment_type,
                window,
                cx,
            )
        });
        let weak = tab.downgrade();
        self.tabs.push(TabType::ObjectDdl(tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
        weak
    }

    pub fn update_last_table_structure_tab(
        &mut self,
        columns: Vec<ColumnInfo>,
        indexes: Vec<IndexInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(TabType::TableStructure(tab)) = self.tabs.last_mut() {
            tab.update(cx, |tab, cx| {
                tab.set_columns(columns, window, cx);
                tab.set_indexes(indexes, window, cx);
            });
        }
    }

    pub fn create_schema_graph_tab(
        &mut self,
        params: schema_graph::SchemaGraphParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = cx.new(|cx| {
            schema_graph::SchemaGraphTab::new(
                params.connection_id,
                params.db_type,
                Some(params.connection_name),
                params.database_name,
                params.schema_name,
                params.environment_type,
                window,
                cx,
            )
        });

        self.tabs.push(TabType::SchemaGraph(tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
    }

    pub fn update_last_schema_graph_tab(
        &mut self,
        tables: Vec<blanco_core::connection_trait::TableSchemaInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(TabType::SchemaGraph(tab)) = self.tabs.last_mut() {
            tab.update(cx, |tab, cx| {
                tab.update_with_schema(tables, window, cx);
            });
        }
    }

    pub fn set_schema_graph_error(
        &mut self,
        error: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(TabType::SchemaGraph(tab)) = self.tabs.last_mut() {
            tab.update(cx, |tab, cx| {
                tab.set_error(error, window, cx);
            });
        }
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

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![],
            active_tab_ix: 0,
            tabbar_scroll_handle: gpui::ScrollHandle::default(),
            _subscriptions: Vec::new(),
            run_query_keystroke: Keystroke::parse("secondary-enter")
                .map(KeybindingKeystroke::from_keystroke)
                .expect("valid keystroke literal"),
            format_query_keystroke: Keystroke::parse("shift-alt-f")
                .map(KeybindingKeystroke::from_keystroke)
                .expect("valid keystroke literal"),
            editor_chat_resize_state,
            script_editor_chat_resize_state,
            editor_results_resize_state,
            results_log_resize_state,
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
                    connection_id: _connection_id,
                    db_type: tab_db_type,
                    connection_name: tab_data.connection_name.clone(),
                    database_name: tab_data
                        .database_name
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                    schema_name: None, // Schema not yet persisted in query tabs
                    environment_type: tab_data.environment_type,
                };
                if tab_data.tab_kind == crate::app_database::EditorKind::Script {
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
            params.connection_id,
            params.database_name.clone(),
            params.schema_name.clone(),
            blanco_core::DriverType::from(params.db_type),
            db_service,
        );

        // SQL completions and statement-based selection ranges only make sense
        // for SQL backends. Line-oriented backends (Redis) get neither.
        let supports_sql = params.db_type.supports_sql();

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
                    .language(params.db_type.editor_language().to_string())
                    .line_number(true)
                    .folding(folding)
                    .tab_size(TabSize {
                        tab_size: tab_size as usize,
                        hard_tabs,
                    })
                    .soft_wrap(word_wrap)
                    .show_whitespaces(show_whitespace);

                if supports_sql {
                    let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                        Rc::new(sql_completion_provider);
                    editor.lsp_mut().completion_provider = Some(completion_provider);

                    // Set up selection range provider for SQL statement highlighting
                    let provider = SqlSelectionRangeProvider::new();
                    let selection_range_provider: Rc<
                        dyn gpui_component::input::SelectionRangeProvider,
                    > = Rc::new(provider);
                    editor.lsp_mut().selection_range_provider = Some(selection_range_provider);
                } else if params.db_type == database::DatabaseType::Redis {
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
                params.db_type.to_sqruff_dialect(),
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
                params.connection_id,
                &params.database_name,
                params.db_type,
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
            title: params.title.clone(),
            connection_id: params.connection_id,
            _db_type: params.db_type,
            connection_name: params.connection_name.clone(),
            database_name: params.database_name.clone(),
            schema_name: params.schema_name.clone(),
            environment_type: params.environment_type,
            editor: editor.clone(),
            db_id: params.db_id,
            results_panel,
            sql_view: cx.new(|cx| {
                SqlView::new(
                    10,
                    cx.theme().highlight_theme.clone(),
                    params.db_type.editor_language(),
                )
                .show_copy_button(false)
            }),
            sqruff_service,
            completion_provider: supports_sql.then_some(sql_completion_provider),
            // Chat functionality
            chat_enabled: false,
            chat_panel: None,
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
        let db_type = params.db_type;
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
                params.connection_id,
                &params.database_name,
                params.db_type,
                window,
                cx,
            )
        });
        self._subscriptions
            .push(cx.observe(&results_panel, |_, _, cx| cx.notify()));

        let script_tab = ScriptTab {
            title: params.title,
            connection_id: params.connection_id,
            db_type,
            connection_name: params.connection_name,
            database_name: params.database_name,
            schema_name: params.schema_name,
            environment_type: params.environment_type,
            editor,
            db_id: params.db_id,
            results_panel,
            log_view: cx.new(|cx| {
                SqlView::new(
                    SCRIPT_LOG_MAX_LINES,
                    cx.theme().highlight_theme.clone(),
                    SCRIPT_EDITOR_LANGUAGE,
                )
                .show_copy_button(false)
            }),
            log_visible: true,
            chat_enabled: false,
            chat_panel: None,
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
        let (enabled, panel) = match self.tabs.get_mut(self.active_tab_ix) {
            Some(TabType::Query(query_tab)) => {
                query_tab.chat_enabled = !query_tab.chat_enabled;
                if !query_tab.chat_enabled || query_tab.chat_panel.is_some() {
                    cx.notify();
                    return;
                }
                let context = ChatSessionContext::new()
                    .with_input_state(query_tab.editor.downgrade())
                    .with_connection(
                        query_tab.connection_id,
                        query_tab.database_name.clone(),
                        query_tab._db_type,
                    )
                    .with_tab_language(TabLanguage::Query);
                (
                    &mut query_tab.chat_enabled,
                    (&mut query_tab.chat_panel, context),
                )
            }
            Some(TabType::Script(script_tab)) => {
                script_tab.chat_enabled = !script_tab.chat_enabled;
                if !script_tab.chat_enabled || script_tab.chat_panel.is_some() {
                    cx.notify();
                    return;
                }
                let context = ChatSessionContext::new()
                    .with_input_state(script_tab.editor.downgrade())
                    .with_connection(
                        script_tab.connection_id,
                        script_tab.database_name.clone(),
                        script_tab.db_type,
                    )
                    .with_tab_language(TabLanguage::Script);
                (
                    &mut script_tab.chat_enabled,
                    (&mut script_tab.chat_panel, context),
                )
            }
            _ => return,
        };

        let (chat_panel, session_context) = panel;
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
        let editor = match self.tabs.get(self.active_tab_ix) {
            Some(TabType::Query(query_tab)) => query_tab.editor.clone(),
            Some(TabType::Script(script_tab)) => script_tab.editor.clone(),
            _ => return false,
        };
        editor.update(cx, |state, cx| {
            state.focus(window, cx);
            state.insert(text, window, cx);
        });
        cx.notify();
        true
    }
}
