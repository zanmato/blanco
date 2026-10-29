pub(crate) mod dashboard;
mod file_runner;
mod object_ddl;
pub(crate) mod open_tabs;
mod parameter_form;
pub(crate) mod process_list;
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
mod sql_files;
#[cfg(test)]
mod sql_files_test;
mod sql_operations;
pub(crate) mod tab_access;
mod tab_actions;
#[cfg(test)]
mod tab_persistence_test;
mod table_structure;
mod tabs;
mod terminal_pane;

pub(crate) use sql_operations::KeywordCase;
pub(crate) use tab_actions::{CloseScope, MovedTab};
pub use tabs::{
    ConnectionBackedTab, ObjectDdlParams, OutlinedStatement, QueryTab, ScriptTab, SettingsTab,
    TabCreationParams, TabType, TableStructureParams,
};

use blanco_core::ConnectionContext;
use gpui::{
    App, AppContext, Context, Entity, EntityId, FocusHandle, Focusable as _, KeybindingKeystroke,
    Keystroke, ParentElement as _, Task, WeakEntity, Window,
};
use gpui_component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogClose, DialogFooter},
    input::{EditorState, InputEvent, TabSize},
    resizable::ResizableState,
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};
use tracing::{debug, error, info};

use self::object_ddl::ObjectDdlTab;
use self::snippet_editor::SnippetEditor;
use self::table_structure::TableStructureTab;
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;
use crate::script_completion::ScriptCompletionProvider;
use crate::settings::SettingsView;
use crate::sql::{SqlCompletionProvider, SqlSignatureHelpProvider, SqruffService};
use app_database::AppDatabase;
use app_database::QueryTabData;
use blanco_ui::SqlView;
use database::{DatabaseService, DatabaseServiceTrait};

/// What the PROD write confirmation dialog shows, and which tab remembers
/// the "don't ask again" choice.
pub(crate) struct ProdWritePrompt {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    pub tab_index: usize,
}

pub struct EditorPanel {
    focus_handle: FocusHandle,
    tabs: Vec<TabType>,
    /// The first `pinned_count` tabs are pinned: they stay at the start and
    /// Close Others and Close to the Right leave them.
    pinned_count: usize,
    active_tab_ix: usize,
    tabbar_scroll_handle: gpui::ScrollHandle,
    _subscriptions: Vec<gpui::Subscription>,
    run_query_keystroke: KeybindingKeystroke,
    format_query_keystroke: KeybindingKeystroke,
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
    /// A SQL file running statement by statement on a tab's connection.
    file_run: Option<file_runner::FileRun>,
    _lint_debounce_task: Task<()>,
    /// Pending saves of edited tab text, keyed by the tab's editor. Each is
    /// replaced on the next keystroke, so a tab is saved once typing pauses.
    autosave_tasks: HashMap<EntityId, Task<()>>,
    /// Tabs whose first save is still being inserted. Their next save waits,
    /// so a tab never gets two rows.
    autosave_inserting: HashSet<EntityId>,
    /// Recently closed tabs, most recent last, for Reopen Closed Tab.
    closed_tabs: Vec<ClosedTab>,
    /// The active tab as last written to the window's row, to restore it.
    persisted_active_tab: Option<i64>,
    /// The dashboards open as tabs as last written for the window, with
    /// whether each was active.
    persisted_dashboards: Vec<(i64, bool)>,
    /// A save to a dashboard waiting for the run of the tab with these
    /// results to end.
    pending_panel_save: Option<(Entity<ResultsPanel>, dashboard::PendingPanelSave)>,
    /// The `windows` row this panel's tabs are stored under.
    window_id: i64,
}

/// A closed tab, kept so Reopen Closed Tab can bring it back.
struct ClosedTab {
    is_script: bool,
    params: TabCreationParams,
    /// The `.sql` file the tab was tied to.
    file_path: Option<String>,
}

/// How many closed tabs Reopen Closed Tab can bring back.
const MAX_CLOSED_TABS: usize = 20;

/// How long typing must pause before a tab's text is saved.
const AUTOSAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(750);

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

    /// The object the active tab shows, for the sidebar to select.
    pub fn active_reveal_target(&self, cx: &App) -> Option<crate::connections::RevealTarget> {
        self.tabs.get(self.active_tab_ix)?.reveal_target(cx)
    }

    #[cfg(any(test, feature = "screenshots"))]
    pub fn active_query_tab(&self) -> Option<&QueryTab> {
        self.tabs.get(self.active_tab_ix).and_then(TabType::query)
    }

    #[cfg(any(test, feature = "screenshots"))]
    pub fn active_script_tab(&self) -> Option<&ScriptTab> {
        self.tabs.get(self.active_tab_ix).and_then(TabType::script)
    }

    #[cfg(any(test, feature = "screenshots"))]
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

    /// Connections with uncommitted edits in a result grid of any tab.
    pub fn connections_with_pending_edits(&self, cx: &App) -> std::collections::HashSet<i64> {
        self.tabs
            .iter()
            .filter_map(TabType::connection_tab)
            .filter(|tab| tab.results_panel.read(cx).has_pending_changes(cx))
            .map(|tab| tab.context.connection_id)
            .collect()
    }

    /// Move the query and script tabs of connection `from` to `to`, keeping
    /// their titles, text, database and schema. Used when a temporary
    /// connection is saved and gets its permanent id; temporary tabs are never
    /// persisted, so there is nothing in the app database to update.
    pub fn move_tabs_to_connection(
        &mut self,
        from: i64,
        to: &ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut moved = Vec::new();
        for index in (0..self.tabs.len()).rev() {
            let is_pinned = self.is_pinned(index);
            let tab = &mut self.tabs[index];
            let is_script = matches!(tab, TabType::Script(_));
            let Some(connection_tab) = tab.connection_tab_mut() else {
                continue;
            };
            if connection_tab.context.connection_id != from {
                continue;
            }
            moved.push((
                is_script,
                is_pinned,
                // The recreated tab stays tied to its file, unsaved changes
                // included.
                connection_tab.file.take(),
                TabCreationParams {
                    title: connection_tab.title.clone(),
                    content: Some(connection_tab.editor.read(cx).value().to_string()),
                    db_id: None,
                    last_run_at: None,
                    context: ConnectionContext {
                        database_name: connection_tab.context.database_name.clone(),
                        schema_name: connection_tab.context.schema_name.clone(),
                        ..to.clone()
                    },
                },
            ));
            self.tabs.remove(index);
            if index < self.pinned_count {
                self.pinned_count -= 1;
            }
        }
        self.active_tab_ix = self.active_tab_ix.min(self.tabs.len().saturating_sub(1));
        for (is_script, was_pinned, file, params) in moved.into_iter().rev() {
            if is_script {
                self.create_and_add_script_tab(window, params, cx);
            } else {
                self.create_and_add_tab_with_connection(window, params, cx);
            }
            if let Some(tab) = self.tabs.last_mut().and_then(TabType::connection_tab_mut) {
                tab.file = file;
            }
            // The tab comes back last and unpinned.
            if was_pinned && let Some(last) = self.tabs.len().checked_sub(1) {
                self.toggle_pin(last, window, cx);
            }
        }
        cx.notify();
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

    pub fn set_all_editors_highlight_invisible_characters(
        &mut self,
        highlight: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for editor in self.connection_backed_editors() {
            editor.update(cx, |state, cx| {
                state.set_highlight_invisible_characters(highlight, window, cx)
            });
        }
    }

    pub fn set_all_editors_completion_accept_keys(
        &mut self,
        accept_keys: gpui_component::input::CompletionAcceptKeys,
        cx: &mut Context<Self>,
    ) {
        for editor in self.connection_backed_editors() {
            editor.update(cx, |state, _| {
                state.lsp_mut().completion_menu.accept_keys = accept_keys;
            });
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
        // The last tab stays, so a window is never left without an editor
        // by the user's own close.
        if self.tabs.len() > 1 {
            self.remove_tab(tab_index, cx);
        }
    }

    /// Take a tab out, even the last one, as for a dashboard that was
    /// deleted.
    fn remove_tab(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        if tab_index < self.tabs.len() {
            // Get the db_id before removing the tab
            let db_id = self.tabs.get(tab_index).and_then(TabType::db_id);
            if let Some(closed) = self.tabs.get(tab_index).and_then(|tab| {
                let connection_tab = tab.connection_tab()?;
                self.autosave_tasks
                    .remove(&connection_tab.editor.entity_id());
                Some(ClosedTab {
                    is_script: matches!(tab, TabType::Script(_)),
                    params: TabCreationParams {
                        title: connection_tab.title.clone(),
                        content: Some(connection_tab.editor.read(cx).text().to_string()),
                        db_id: None,
                        last_run_at: connection_tab.last_run_at,
                        context: connection_tab.context.clone(),
                    },
                    file_path: connection_tab.file.as_ref().map(|file| file.path_string()),
                })
            }) {
                if self.closed_tabs.len() == MAX_CLOSED_TABS {
                    self.closed_tabs.remove(0);
                }
                self.closed_tabs.push(closed);
            }

            // Remove tab from UI
            self.tabs.remove(tab_index);
            if tab_index < self.pinned_count {
                self.pinned_count -= 1;
            }

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

    /// Bring back the most recently closed tab, with its text and connection.
    pub fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(closed) = self.closed_tabs.pop() else {
            return;
        };
        // A new tab is saved shortly after it opens, so the reopened one is
        // persisted again without waiting for an edit.
        if closed.is_script {
            self.create_and_add_script_tab(window, closed.params, cx);
        } else {
            self.create_and_add_tab_with_connection(window, closed.params, cx);
        }
        // Tied to its file again, and unsaved where its text differs from it.
        if let Some(path) = closed.file_path
            && let Some(ix) = self.tabs.len().checked_sub(1)
        {
            self.restore_tab_file(ix, &path, cx);
        }
        self.focus_active_tab(window, cx);
    }

    /// Put the keyboard in the active tab. Changing tabs takes the focused
    /// editor out of the window, and with nothing focused the shortcuts stop
    /// reaching the app until something is clicked.
    pub(super) fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self
            .tabs
            .get(self.active_tab_ix)
            .and_then(TabType::connection_tab)
        {
            Some(tab) => tab.editor.update(cx, |editor, cx| editor.focus(window, cx)),
            None => self.focus_handle.focus(window, cx),
        }
    }

    /// Save a tab's text once typing pauses, so nothing typed is lost on a
    /// crash or restart.
    fn watch_for_autosave(
        &mut self,
        editor: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscription = cx.subscribe_in(editor, window, |this, editor, event, _, cx| {
            if let InputEvent::Change = event {
                this.schedule_autosave(editor.entity_id(), cx);
            }
        });
        self._subscriptions.push(subscription);
    }

    fn schedule_autosave(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update(cx, |this, cx| {
                this.autosave_tasks.remove(&editor);
                this.save_tab_for_editor(editor, cx);
            })
            .log_err();
        });
        self.autosave_tasks.insert(editor, task);
    }

    /// Write the tab that owns `editor` to the app database: its text,
    /// position and connection. A new tab gets its row here.
    fn save_tab_for_editor(&mut self, editor: EntityId, cx: &mut Context<Self>) {
        if self.autosave_inserting.contains(&editor) {
            // The first save is still being inserted; try again after it.
            self.schedule_autosave(editor, cx);
            return;
        }
        let Some((position, tab)) = self.tabs.iter().enumerate().find_map(|(ix, tab)| {
            let connection_tab = tab.connection_tab()?;
            (connection_tab.editor.entity_id() == editor).then_some((ix, tab))
        }) else {
            return;
        };
        let Some(connection_tab) = tab.connection_tab() else {
            return;
        };
        // A connection opened for this session only has no row for the tab to
        // refer to.
        if connection_tab.context.is_temporary() {
            return;
        }
        let tab_data = QueryTabData {
            id: connection_tab.db_id,
            title: connection_tab.title.clone(),
            content: connection_tab.editor.read(cx).text().to_string(),
            position: position as i32,
            connection_id: Some(connection_tab.context.connection_id),
            connection_type: Some(connection_tab.context.db_type.as_str().to_string()),
            connection_name: Some(connection_tab.context.connection_name.clone()),
            database_name: Some(connection_tab.context.database_name.clone()),
            schema_name: connection_tab.context.schema_name.clone(),
            environment_type: connection_tab.context.environment_type,
            tab_kind: if matches!(tab, TabType::Script(_)) {
                app_database::EditorKind::Script
            } else {
                app_database::EditorKind::Query
            },
            last_run_at: connection_tab.last_run_at,
            file_path: connection_tab.file.as_ref().map(|file| file.path_string()),
            window_id: Some(self.window_id),
            // Written on its own when the view changes.
            result_view: None,
        };
        let inserting = tab_data.id.is_none();
        if inserting {
            self.autosave_inserting.insert(editor);
        }
        let last_run_at = tab_data.last_run_at;
        let app_database = AppDatabase::global(cx).clone();
        cx.spawn(async move |this, cx| {
            let saved = app_database.save_query_tab(&tab_data).await;
            if let (Ok(db_id), Some(last_run_at)) = (&saved, last_run_at)
                && let Err(error) = app_database
                    .touch_query_tab_last_run(*db_id, last_run_at)
                    .await
            {
                error!("Failed to save the tab's last run: {error}");
            }
            this.update(cx, |this, cx| {
                this.autosave_inserting.remove(&editor);
                let db_id = match saved {
                    Ok(db_id) => db_id,
                    Err(error) => {
                        error!("Failed to save tab: {error}");
                        return;
                    }
                };
                let owner = this
                    .tabs
                    .iter_mut()
                    .filter_map(TabType::connection_tab_mut)
                    .find(|tab| tab.editor.entity_id() == editor);
                match owner {
                    Some(tab) => tab.db_id = Some(db_id),
                    // Closed while its first save was in flight: drop the row
                    // so the tab does not come back on the next start.
                    None if inserting => {
                        cx.spawn(async move |_, _| {
                            if let Err(error) = app_database.delete_query_tab(db_id).await {
                                error!("Failed to remove a closed tab: {error}");
                            }
                        })
                        .detach();
                    }
                    None => {}
                }
            })
            .log_err();
        })
        .detach();
    }

    /// Remember the active tab so the next start opens on it.
    /// Save how a query tab shows its result, so the tab shows its next
    /// result the same way after a restart.
    fn remember_result_view(
        &mut self,
        results_panel: &Entity<ResultsPanel>,
        cx: &mut Context<Self>,
    ) {
        let Some(db_id) = self.tabs.iter().find_map(|tab| match tab {
            TabType::Query(tab) if &tab.results_panel == results_panel => tab.db_id,
            _ => None,
        }) else {
            return;
        };
        let view = results_panel
            .read(cx)
            .remembered_view(cx)
            .map(|view| view.to_json());
        let app_database = AppDatabase::global(cx).clone();
        cx.background_spawn(async move {
            if let Err(error) = app_database.set_query_tab_result_view(db_id, view).await {
                error!("Failed to remember how the tab shows its result: {error}");
            }
        })
        .detach();
    }

    fn persist_active_tab(&mut self, cx: &mut Context<Self>) {
        let active = self.tabs.get(self.active_tab_ix).and_then(TabType::db_id);
        if active.is_none() || active == self.persisted_active_tab {
            return;
        }
        self.persisted_active_tab = active;
        let Some(active) = active else {
            return;
        };
        let window_id = self.window_id;
        let app_database = AppDatabase::global(cx).clone();
        cx.spawn(async move |_, _| {
            if let Err(error) = app_database.set_window_active_tab(window_id, active).await {
                error!("Failed to remember the active tab: {error}");
            }
        })
        .detach();
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

    /// The editor of the active tab, when it is a snippet.
    pub(crate) fn active_snippet_editor(&self) -> Option<Entity<SnippetEditor>> {
        match self.tabs.get(self.active_tab_ix)? {
            TabType::Snippet(editor) => Some(editor.clone()),
            _ => None,
        }
    }

    /// Give open editors of the snippet `id` its new name.
    pub(crate) fn rename_snippet_tabs(&mut self, id: i64, name: &str, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let TabType::Snippet(editor) = tab
                && editor.read(cx).snippet_id == Some(id)
            {
                editor.update(cx, |editor, cx| editor.set_name(name, cx));
            }
        }
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

    /// Open a structure tab for a table not created yet, in the database
    /// and schema of `context`.
    pub fn create_new_table_tab(
        &mut self,
        context: blanco_core::ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableStructureTab> {
        let tab = cx.new(|cx| TableStructureTab::new_table(context, window, cx));
        // The tab names itself once the table is created.
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
        self.tabs.push(TabType::TableStructure(tab.clone()));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
        tab
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

    /// Open a process list for the server behind `context`, reusing the one
    /// already open for that connection.
    pub fn create_process_list_tab(
        &mut self,
        context: blanco_core::ConnectionContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<process_list::ProcessListTab> {
        let existing = self.tabs.iter().position(|tab| {
            matches!(tab, TabType::ProcessList(tab)
                if tab.read(cx).context.connection_id == context.connection_id)
        });
        if let Some(ix) = existing
            && let Some(TabType::ProcessList(tab)) = self.tabs.get(ix)
        {
            let tab = tab.clone();
            self.activate_tab(ix, window, cx);
            return tab;
        }
        let tab = cx.new(|cx| process_list::ProcessListTab::new(context, window, cx));
        self.tabs.push(TabType::ProcessList(tab.clone()));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);
        cx.notify();
        tab
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

    /// Move to the next (`step` 1) or previous (`step` -1) tab. At either end
    /// it wraps around, or stays put when `wrap` is off.
    pub fn activate_adjacent_tab(
        &mut self,
        step: isize,
        wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = adjacent_tab(self.active_tab_ix, self.tabs.len(), step, wrap) {
            self.set_active_tab(ix, window, cx);
        }
    }

    /// Show or hide the results area under the active tab's editor.
    pub fn toggle_results(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self
            .tabs
            .get_mut(self.active_tab_ix)
            .and_then(TabType::connection_tab_mut)
        {
            tab.results_hidden = !tab.results_hidden;
            cx.notify();
        }
    }

    /// The connection the active tab works against.
    pub fn active_context(&self, cx: &App) -> Option<ConnectionContext> {
        self.tabs.get(self.active_tab_ix)?.context(cx)
    }

    /// Whether the active tab is a query or script editor on an SQL backend.
    pub(crate) fn active_tab_edits_sql(&self) -> bool {
        self.tabs
            .get(self.active_tab_ix)
            .and_then(TabType::connection_tab)
            .is_some_and(|tab| tab.context.db_type.supports_sql())
    }

    pub fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close_tab(self.active_tab_ix, window, cx);
    }

    fn set_active_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.active_tab_ix = ix;
            self.tabbar_scroll_handle.scroll_to_item(ix);
            self.focus_active_tab(window, cx);
            cx.notify();
        }
    }

    /// A panel for the window stored as `window_id`, showing its saved tabs
    /// with `active_tab` in front.
    pub fn new_with_saved_tabs(
        window: &mut Window,
        cx: &mut Context<Self>,
        window_id: i64,
        active_tab: Option<i64>,
        saved_tabs: Vec<QueryTabData>,
    ) -> Self {
        info!("Loading {} saved tabs", saved_tabs.len());

        let editor_results_resize_state = cx.new(|_| ResizableState::default());
        let results_log_resize_state = cx.new(|_| ResizableState::default());
        let editor_terminal_resize_state = cx.new(|_| ResizableState::default());

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![],
            pinned_count: 0,
            active_tab_ix: 0,
            tabbar_scroll_handle: gpui::ScrollHandle::default(),
            _subscriptions: Vec::new(),
            run_query_keystroke: keystroke_hint("RunQuery", cx),
            format_query_keystroke: keystroke_hint("FormatQuery", cx),
            editor_results_resize_state,
            results_log_resize_state,
            editor_terminal_resize_state,
            loading: false,
            linting_enabled: false,
            _run_query_task: Task::ready(()),
            abort_query_task: None,
            script_cancel: None,
            file_run: None,
            _lint_debounce_task: Task::ready(()),
            autosave_tasks: HashMap::new(),
            autosave_inserting: HashSet::new(),
            closed_tabs: Vec::new(),
            persisted_active_tab: active_tab,
            persisted_dashboards: Vec::new(),
            pending_panel_save: None,
            window_id,
        };

        panel.restore_saved_tabs_with_connections_sync(saved_tabs, window, cx);
        // Open on the tab that was active when the app last ran.
        if let Some(active) = panel.persisted_active_tab
            && let Some(ix) = panel
                .tabs
                .iter()
                .position(|tab| tab.db_id() == Some(active))
        {
            panel.active_tab_ix = ix;
        }
        // Every change to the panel may have switched the active tab.
        // A save to a dashboard can wait for a run, which ends in this event
        // whichever way it goes.
        let run_ended_subscription = cx.subscribe_in(
            &cx.entity(),
            window,
            |this, _, event: &EditorPanelEvent, window, cx| {
                if let EditorPanelEvent::QueryRunEnded { tab_index, outcome } = event {
                    this.finish_pending_panel_save(*tab_index, outcome, window, cx);
                }
            },
        );
        panel._subscriptions.push(run_ended_subscription);
        let active_tab_subscription = cx.observe_self(|this, cx| {
            this.persist_active_tab(cx);
            this.persist_open_dashboards(cx);
        });
        panel._subscriptions.push(active_tab_subscription);

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
                let highlight_invisible = AppSettings::global(cx)
                    .settings
                    .editor
                    .highlight_invisible_characters;
                this.set_all_editors_soft_wrap(word_wrap, window, cx);
                this.set_all_editors_show_whitespace(show_whitespace, window, cx);
                this.set_all_editors_highlight_invisible_characters(
                    highlight_invisible,
                    window,
                    cx,
                );
                let accept_keys = crate::settings::completion_accept_keys(cx);
                this.set_all_editors_completion_accept_keys(accept_keys, cx);
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
                    if let Some(view) = &tab_data.result_view
                        && let Some(TabType::Query(tab)) = self.tabs.last()
                    {
                        let view = dashboard::PanelView::from_json(view);
                        tab.results_panel
                            .update(cx, |results, _| results.set_pending_view(view));
                    }
                }
                if let Some(path) = &tab_data.file_path {
                    self.restore_tab_file(self.tabs.len() - 1, path, cx);
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
            cx.background_executor().clone(),
        );

        // SQL completions and the statement outline only make sense for SQL
        // backends. Line-oriented backends (Redis) get neither.
        let supports_sql = params.context.db_type.supports_sql();

        let editor = cx.new({
            let sql_completion_provider = sql_completion_provider.clone();
            |cx| {
                // Read settings
                let editor_settings = &AppSettings::global(cx).settings.editor;
                let word_wrap = editor_settings.word_wrap;
                let show_whitespace = editor_settings.show_whitespace;
                let highlight_invisible = editor_settings.highlight_invisible_characters;
                let folding = editor_settings.folding;
                let hard_tabs = editor_settings.hard_tabs;
                let tab_size = editor_settings.tab_size;

                let mut editor = EditorState::new(window, cx)
                    .language(crate::sql::highlight_language(params.context.db_type).to_string())
                    .line_number(true)
                    .folding(folding)
                    .tab_size(TabSize {
                        tab_size: tab_size as usize,
                        hard_tabs,
                    })
                    .soft_wrap(word_wrap)
                    .show_whitespaces(show_whitespace)
                    .highlight_invisible_characters(highlight_invisible);
                editor.lsp_mut().completion_menu.accept_keys =
                    crate::settings::completion_accept_keys(cx);

                if supports_sql {
                    let signature_help_provider: Rc<
                        dyn gpui_component::input::SignatureHelpProvider,
                    > = Rc::new(SqlSignatureHelpProvider::new(
                        sql_completion_provider.clone(),
                    ));
                    editor.lsp_mut().signature_help_provider = Some(signature_help_provider);
                    editor.lsp_mut().code_action_providers.push(Rc::new(
                        crate::sql::expand_star::ExpandStarProvider::new(
                            sql_completion_provider.clone(),
                        ),
                    ));

                    let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                        Rc::new(sql_completion_provider);
                    editor.lsp_mut().completion_provider = Some(completion_provider);
                    editor.lsp_mut().completion_menu.max_width =
                        crate::sql::completion::COMPLETION_MENU_MAX_WIDTH;
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

        // The frame around the statement the cursor sits in. Only SQL backends
        // have statements to outline, so only they get a collection.
        let statement_outline = supports_sql.then(|| {
            editor.update(cx, |state, cx| {
                state.create_range_decorations_collection(vec![], cx)
            })
        });

        let subscription = cx.subscribe_in(&editor, window, |this, _editor, event, window, cx| {
            if let InputEvent::PressEnter { secondary, .. } = event
                && *secondary
            {
                this.on_run_query(window, cx);
            }
        });
        self._subscriptions.push(subscription);
        self.watch_for_autosave(&editor, window, cx);

        // The editor notifies on edits and on cursor moves, which is exactly
        // when the statement under the cursor can change. Re-rendering here
        // also keeps the Ln/Col indicator in the action bar on the cursor.
        let subscription = cx.observe(&editor, |this, editor, cx| {
            this.refresh_statement_outline(&editor, cx);
            this.refresh_file_dirty(&editor, cx);
            cx.notify();
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
        self._subscriptions.push(cx.subscribe(
            &results_panel,
            |this, results_panel, event: &crate::results_panel::ResultsPanelEvent, cx| {
                let crate::results_panel::ResultsPanelEvent::ViewChanged = event;
                this.remember_result_view(&results_panel, cx);
            },
        ));
        let query_tab = QueryTab {
            base: ConnectionBackedTab {
                title: params.title.clone(),
                context: params.context.clone(),
                editor: editor.clone(),
                db_id: params.db_id,
                last_run_at: params.last_run_at,
                results_panel,
                terminal_enabled: false,
                skip_prod_write_confirmation: false,
                results_hidden: false,
                terminal: None,
                file: None,
            },
            sql_view: cx.new(|cx| {
                SqlView::new(
                    10,
                    crate::sql::highlight_language(params.context.db_type),
                    cx,
                )
                .show_copy_button(false)
            }),
            commit_preview: cx.new(|cx| {
                SqlView::new(
                    usize::MAX,
                    crate::sql::highlight_language(params.context.db_type),
                    cx,
                )
            }),
            sqruff_service,
            completion_provider: supports_sql.then_some(sql_completion_provider),
            sql_view_visible: true,
            last_parameter_values: HashMap::new(),
            statement_outline,
            outlined_statement: OutlinedStatement::default(),
        };

        self.tabs.push(TabType::Query(Box::new(query_tab)));
        self.active_tab_ix = self.tabs.len() - 1;
        if params.db_id.is_none() {
            // Persist a new tab even before it is edited.
            self.schedule_autosave(editor.entity_id(), cx);
        }
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    /// Create a JavaScript script tab bound to the same connection identity as
    /// a query tab. Scripts drive the database through the injected `db` object
    /// rather than through the SQL pipeline, so this deliberately skips the
    /// SQL completion, statement outline, formatter and sqruff wiring.
    pub fn create_and_add_script_tab(
        &mut self,
        window: &mut Window,
        params: TabCreationParams,
        cx: &mut Context<Self>,
    ) {
        let editor_settings = AppSettings::global(cx).settings.editor.clone();
        let db_type = params.context.db_type;
        let params_db_id = params.db_id;
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
                .show_whitespaces(editor_settings.show_whitespace)
                .highlight_invisible_characters(editor_settings.highlight_invisible_characters);
            editor.lsp_mut().completion_menu.accept_keys =
                editor_settings.completion_accept_keys.accept_keys();

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

        // A script's syntax check covers the whole buffer, so only edits can
        // invalidate it.
        let subscription = cx.subscribe_in(&editor, window, |this, _editor, event, window, cx| {
            if let InputEvent::Change = event {
                if this.linting_enabled {
                    this.lint_current_script_debounced(cx);
                }
            } else if let InputEvent::PressEnter { secondary, .. } = event
                && *secondary
            {
                this.on_run_query(window, cx);
            }
        });
        self._subscriptions.push(subscription);

        // Re-render so the Ln/Col indicator in the action bar tracks the cursor.
        let subscription = cx.observe(&editor, |this, editor, cx| {
            this.refresh_file_dirty(&editor, cx);
            cx.notify();
        });
        self._subscriptions.push(subscription);
        self.watch_for_autosave(&editor, window, cx);
        let editor_id = editor.entity_id();

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
                terminal_enabled: false,
                skip_prod_write_confirmation: false,
                results_hidden: false,
                terminal: None,
                file: None,
            },
            log_view: cx.new(|cx| {
                SqlView::new(SCRIPT_LOG_MAX_LINES, SCRIPT_EDITOR_LANGUAGE, cx)
                    .show_copy_button(false)
            }),
            log_visible: true,
        };

        self.tabs.push(TabType::Script(Box::new(script_tab)));
        self.active_tab_ix = self.tabs.len() - 1;
        if params_db_id.is_none() {
            // Persist a new tab even before it is edited.
            self.schedule_autosave(editor_id, cx);
        }
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    /// Bring the tab at `ix` into view once the strip has laid out its new
    /// order, scrolling no further than needed. `from` is the offset to scroll
    /// from when something else has moved the strip in the meantime.
    fn scroll_tabbar_to_tab(
        &self,
        ix: usize,
        from: Option<gpui::Point<gpui::Pixels>>,
        window: &mut Window,
    ) {
        let scroll_handle = self.tabbar_scroll_handle.clone();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, _| {
                if let Some(offset) = from {
                    scroll_handle.set_offset(offset);
                }
                scroll_handle.scroll_to_item(ix);
                window.refresh();
            })
        });
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
        if let Some(environment) = query_tab.context.environment_type
            && environment.confirms_writes()
        {
            let connection_name = query_tab.context.connection_name.clone();
            self.confirm_prod_write(
                ProdWritePrompt {
                    title: format!("Commit edits to {environment}?"),
                    message: format!(
                        "\"{connection_name}\" is tagged {environment}. The pending grid edits will be written to it."
                    ),
                    confirm_label: format!("Commit to {environment}"),
                    tab_index: self.active_tab_ix,
                },
                |panel, window, cx| panel.commit_current_changes_unchecked(window, cx),
                None,
                window,
                cx,
            );
            return;
        }
        self.commit_current_changes_unchecked(window, cx);
    }

    /// Confirm a write against a PROD connection and run `on_confirm` on this
    /// panel when the user accepts. The dialog offers to stop asking for the
    /// rest of the tab's life. `on_cancel` runs when the dialog goes away
    /// without a confirmation, however it was dismissed.
    pub(crate) fn confirm_prod_write(
        &mut self,
        prompt: ProdWritePrompt,
        on_confirm: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        on_cancel: Option<Box<dyn Fn(&mut Self, &mut Context<Self>)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ProdWritePrompt {
            title,
            message,
            confirm_label,
            tab_index,
        } = prompt;
        let already_confirmed = self
            .tabs
            .get(tab_index)
            .and_then(|tab| tab.connection_tab())
            .is_some_and(|tab| tab.skip_prod_write_confirmation);
        if already_confirmed {
            on_confirm(self, window, cx);
            return;
        }

        let weak_panel = cx.entity().downgrade();
        let on_confirm = Rc::new(on_confirm);
        let on_cancel: Option<Rc<dyn Fn(&mut Self, &mut Context<Self>)>> = on_cancel.map(Rc::from);
        // The close hook fires for every way the dialog goes away, so it
        // reports a cancel only when nothing ran.
        let confirmed = Rc::new(Cell::new(false));
        window.open_dialog(cx, move |dialog, _, _| {
            let run = {
                let weak_panel = weak_panel.clone();
                let on_confirm = on_confirm.clone();
                let confirmed = confirmed.clone();
                move |remember: bool, window: &mut Window, cx: &mut App| {
                    confirmed.set(true);
                    let on_confirm = on_confirm.clone();
                    weak_panel
                        .update(cx, |panel, cx| {
                            if remember
                                && let Some(tab) = panel
                                    .tabs
                                    .get_mut(tab_index)
                                    .and_then(|tab| tab.connection_tab_mut())
                            {
                                tab.skip_prod_write_confirmation = true;
                            }
                            on_confirm(panel, window, cx);
                        })
                        .log_err();
                    window.close_dialog(cx);
                }
            };
            let run_once = run.clone();
            let run_and_remember = run;
            let confirmed_on_close = confirmed.clone();
            let weak_on_close = weak_panel.clone();
            let on_cancel = on_cancel.clone();
            dialog
                .title(title.clone())
                .child(message.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("prod-write-cancel").label("Cancel").outline()),
                        )
                        // Two `DialogAction` siblings share one element id and
                        // therefore one click state, so only one of them would
                        // ever close the dialog. The run closure closes it instead.
                        .child(
                            Button::new("prod-write-confirm-remember")
                                .outline()
                                .label("Don't ask again for this tab")
                                .on_click(move |_, window, cx| run_and_remember(true, window, cx)),
                        )
                        .child(
                            Button::new("prod-write-confirm")
                                .danger()
                                .label(confirm_label.clone())
                                .on_click(move |_, window, cx| run_once(false, window, cx)),
                        ),
                )
                .on_close(move |_, _, cx| {
                    if confirmed_on_close.get() {
                        return;
                    }
                    let Some(on_cancel) = on_cancel.clone() else {
                        return;
                    };
                    weak_on_close
                        .update(cx, |panel, cx| on_cancel(panel, cx))
                        .log_err();
                })
        });
    }

    fn commit_current_changes_unchecked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            let changes = query_tab
                .results_panel
                .update(cx, |panel, cx| panel.commit_all_edits(cx));

            // Added, duplicated and deleted rows are pending changes without
            // any edited cell, so cell edits alone cannot decide emptiness.
            let has_pending = query_tab.results_panel.read(cx).has_pending_edits(cx);
            if changes.is_empty() && !has_pending {
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

    /// Size the editor and the log under the results in every tab, leaving the
    /// rest to the results.
    #[cfg(feature = "screenshots")]
    pub(crate) fn set_pane_heights(
        &mut self,
        editor: gpui::Pixels,
        log: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_results_resize_state.update(cx, |state, cx| {
            state.resize_panel(0, editor, window, cx);
        });
        self.results_log_resize_state.update(cx, |state, cx| {
            state.resize_panel(1, log, window, cx);
        });
    }

    /// Size the terminal pane docked under the active tab.
    #[cfg(feature = "screenshots")]
    pub(crate) fn set_terminal_height(
        &mut self,
        height: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_terminal_resize_state.update(cx, |state, cx| {
            state.resize_panel(1, height, window, cx);
        });
    }

    /// The active tab when it is a dashboard.
    #[cfg(feature = "screenshots")]
    pub(crate) fn active_dashboard_tab(&self) -> Option<Entity<dashboard::DashboardTab>> {
        match self.tabs.get(self.active_tab_ix) {
            Some(TabType::Dashboard(tab)) => Some(tab.clone()),
            _ => None,
        }
    }

    /// The active tab when it is a schema graph.
    #[cfg(feature = "screenshots")]
    pub(crate) fn active_schema_graph_tab(
        &self,
    ) -> Option<Entity<self::schema_graph::SchemaGraphTab>> {
        match self.tabs.get(self.active_tab_ix) {
            Some(TabType::SchemaGraph(tab)) => Some(tab.clone()),
            _ => None,
        }
    }

    /// The active tab when it is a table's structure.
    #[cfg(any(test, feature = "screenshots"))]
    pub(crate) fn active_structure_tab(&self) -> Option<Entity<TableStructureTab>> {
        match self.tabs.get(self.active_tab_ix) {
            Some(TabType::TableStructure(tab)) => Some(tab.clone()),
            _ => None,
        }
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

/// The tab `step` places away from `active` among `count` tabs, wrapping at
/// the ends or stopping there.
fn adjacent_tab(active: usize, count: usize, step: isize, wrap: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let target = active as isize + step;
    if wrap {
        Some(target.rem_euclid(count as isize) as usize)
    } else {
        (0..count as isize)
            .contains(&target)
            .then_some(target as usize)
    }
}

#[cfg(test)]
mod adjacent_tab_tests {
    use super::adjacent_tab;

    #[test]
    fn test_adjacent_tab_wraps_or_stops() {
        assert_eq!(adjacent_tab(0, 3, 1, true), Some(1));
        assert_eq!(adjacent_tab(2, 3, 1, true), Some(0));
        assert_eq!(adjacent_tab(0, 3, -1, true), Some(2));
        assert_eq!(adjacent_tab(2, 3, 1, false), None);
        assert_eq!(adjacent_tab(0, 3, -1, false), None);
        assert_eq!(adjacent_tab(0, 0, 1, true), None);
    }
}
