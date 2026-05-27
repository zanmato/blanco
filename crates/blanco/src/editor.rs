mod object_ddl;
mod parameter_form;
mod query_execution;
#[cfg(test)]
mod query_execution_test;
mod rename_form;
mod snippet_editor;
mod sql_operations;
mod table_structure;

use blanco_core::{ColumnInfo, IndexInfo, RoutineKind};
use blanco_ui::{Tab, TabBar};
use gpui::{AnyElement, FontWeight};
use gpui::{
    App, AppContext, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeybindingKeystroke, Keystroke, ParentElement, Render, SharedString, Styled, Task,
    WeakEntity, Window, div, prelude::FluentBuilder, px, rems,
};
use gpui_component::{
    ActiveTheme, Disableable as _, Sizable, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputEvent, InputState, TabSize},
    popover::{Popover, PopoverState},
    resizable::{ResizableState, h_resizable, resizable_panel, v_resizable},
    v_flex,
};
use std::{collections::HashMap, rc::Rc, sync::Arc};
use tracing::{debug, error, info};

use self::object_ddl::ObjectDdlTab;
use self::rename_form::RenameTabForm;
use self::snippet_editor::SnippetEditor;
use self::table_structure::TableStructureTab;
use crate::agent::{ChatPanel, ChatProviderResolver, ChatSessionContext};
use crate::app::RenameTab;
use crate::app::ToggleSidebar;
use crate::app::{ExecuteSubstitutedQuery, FormatQuery};
use crate::app_database::AppDatabase;
use crate::app_database::{EnvironmentType, QueryTabData};
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;
use crate::settings::SettingsView;
use crate::sql::{SqlCompletionProvider, SqlSelectionRangeProvider, SqruffService};
use blanco_ui::{IconName, SqlLog, SqlLogMessage};
use database::{DatabaseService, DatabaseServiceTrait};
use gpui_component::Icon;

pub enum TabType {
    Query(Box<QueryTab>),
    Settings(SettingsTab),
    Snippet(Entity<SnippetEditor>),
    TableStructure(Entity<TableStructureTab>),
    ObjectDdl(Entity<ObjectDdlTab>),
}

pub struct QueryTab {
    pub title: String,
    pub connection_id: i64,               // Connection ID from app database
    pub _db_type: database::DatabaseType, // Database type for this connection
    pub connection_name: Option<String>,  // Connection name from database
    pub database_name: String,            // Database name this tab is connected to
    pub schema_name: Option<String>,      // Optional schema name for context
    pub environment_type: Option<EnvironmentType>, // Environment type from connection
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>,                  // Database ID for persistence
    pub results_panel: Entity<ResultsPanel>, // Each tab has its own results
    pub sql_log: Entity<SqlLog>,             // SQL log for this tab
    // Chat functionality
    pub chat_enabled: bool,
    pub chat_panel: Option<Entity<ChatPanel>>,
    pub sql_log_visible: bool,
    pub sqruff_service: Option<Arc<SqruffService>>,
    pub completion_provider: Option<SqlCompletionProvider>,
    /// In-memory cache of the last values entered for query parameters in
    /// this tab, keyed by parameter label (e.g. "$1" or ":user_id"). Used
    /// to prefill the parameter modal on subsequent runs within the same
    /// session.
    pub last_parameter_values: HashMap<String, String>,
}

pub struct SettingsTab {
    pub title: String,
    pub settings_view: Entity<SettingsView>,
}

pub struct EditorPanel {
    focus_handle: FocusHandle,
    tabs: Vec<TabType>,
    active_tab_ix: usize,
    sidebar_collapsed: bool,
    tabbar_scroll_handle: gpui::ScrollHandle,
    _subscriptions: Vec<gpui::Subscription>,
    run_query_keystroke: KeybindingKeystroke,
    format_query_keystroke: KeybindingKeystroke,
    editor_chat_resize_state: Entity<ResizableState>,
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
    _lint_debounce_task: Task<()>,
}

/// Debounce duration for linting (500ms)
const LINT_DEBOUNCE_MS: u64 = 500;

/// Maximum length of SQL query to log in the SQL log panel
/// Queries longer than this will be truncated to avoid performance issues
const SQL_QUERY_LOG_MAX_LENGTH: usize = 2000;

/// Parameters for creating a new tab with connection
#[derive(Clone)]
pub struct TabCreationParams {
    pub title: String,
    pub content: Option<String>,
    pub db_id: Option<i64>,
    pub connection_id: i64,
    pub db_type: database::DatabaseType,
    pub connection_name: Option<String>,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub environment_type: Option<EnvironmentType>,
}

/// Parameters for creating an object DDL tab (procedures/functions/triggers).
#[derive(Clone)]
pub struct ObjectDdlParams {
    pub kind: RoutineKind,
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub object_name: String,
    pub environment_type: Option<EnvironmentType>,
}

/// Parameters for creating a table structure tab
#[derive(Clone)]
pub struct TableStructureParams {
    pub connection_id: i64,
    pub connection_name: String,
    pub db_type: database::DatabaseType,
    pub database_name: String,
    pub schema_name: Option<String>,
    pub table_name: String,
    pub environment_type: Option<EnvironmentType>,
}

impl EditorPanel {
    #[cfg(test)]
    pub fn active_query_tab(&self) -> Option<&QueryTab> {
        match self.tabs.get(self.active_tab_ix) {
            Some(TabType::Query(tab)) => Some(tab),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        cx.notify();
    }

    pub fn set_all_editors_show_whitespace(
        &mut self,
        show: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for tab in &mut self.tabs {
            if let TabType::Query(query_tab) = tab {
                query_tab
                    .editor
                    .update(cx, |state, cx| state.set_show_whitespaces(show, window, cx));
            }
        }
    }

    pub fn set_all_editors_soft_wrap(
        &mut self,
        wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for tab in &mut self.tabs {
            if let TabType::Query(query_tab) = tab {
                query_tab
                    .editor
                    .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
            }
        }
    }

    fn close_tab(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        if tab_index < self.tabs.len() && self.tabs.len() > 1 {
            // Get the db_id before removing the tab
            let db_id = if let Some(TabType::Query(query_tab)) = self.tabs.get(tab_index) {
                query_tab.db_id
            } else {
                None
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
        if tab_index < self.tabs.len()
            && let Some(TabType::Query(query_tab)) = self.tabs.get_mut(tab_index)
        {
            let old_name = query_tab.title.clone();
            query_tab.title = new_name.to_string();

            // Update database if this tab has a db_id
            if let Some(db_id) = query_tab.db_id {
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
        let snippet_editor = cx.new(|cx| SnippetEditor::new(window, cx));

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
        let snippet_editor = cx.new(|cx| SnippetEditor::new(window, cx));

        // Load snippet data
        let app_database = AppDatabase::global(cx);
        if let Ok(Some(snippet_data)) = gpui_tokio::Tokio::handle(cx)
            .block_on(async { app_database.get_snippet_by_id(snippet_id).await })
        {
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
        sidebar_collapsed: bool,
        saved_tabs: Vec<QueryTabData>,
    ) -> Self {
        info!("Loading {} saved tabs", saved_tabs.len());

        let editor_chat_resize_state = cx.new(|_| ResizableState::default());
        let editor_results_resize_state = cx.new(|_| ResizableState::default());
        let results_log_resize_state = cx.new(|_| ResizableState::default());

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![],
            active_tab_ix: 0,
            sidebar_collapsed,
            tabbar_scroll_handle: gpui::ScrollHandle::default(),
            _subscriptions: Vec::new(),
            run_query_keystroke: Keystroke::parse("secondary-enter")
                .map(KeybindingKeystroke::from_keystroke)
                .expect("valid keystroke literal"),
            format_query_keystroke: Keystroke::parse("shift-alt-f")
                .map(KeybindingKeystroke::from_keystroke)
                .expect("valid keystroke literal"),
            editor_chat_resize_state,
            editor_results_resize_state,
            results_log_resize_state,
            loading: false,
            linting_enabled: false,
            _run_query_task: Task::ready(()),
            abort_query_task: None,
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
                self.create_and_add_tab_with_connection(window, params, cx);
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
            db_service,
        );

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

                let mut editor = InputState::new(window, cx)
                    .code_editor("sql".to_string())
                    .line_number(true)
                    .folding(folding)
                    .tab_size(TabSize {
                        tab_size: tab_size as usize,
                        hard_tabs,
                    })
                    .soft_wrap(word_wrap)
                    .show_whitespaces(show_whitespace);

                let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                    Rc::new(sql_completion_provider);
                editor.lsp.completion_provider = Some(completion_provider);

                // Set up selection range provider for SQL statement highlighting
                {
                    let provider = SqlSelectionRangeProvider::new();
                    let selection_range_provider: Rc<
                        dyn gpui_component::input::SelectionRangeProvider,
                    > = Rc::new(provider);
                    editor.lsp.selection_range_provider = Some(selection_range_provider);
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
            } else if let InputEvent::PressEnter { secondary } = event
                && *secondary
            {
                this.on_run_query(window, cx);
            }
        });
        self._subscriptions.push(subscription);

        // Create SqruffService for this tab
        let formatter_settings = AppSettings::global(cx).settings.formatter.clone();
        let editor_settings = AppSettings::global(cx).settings.editor.clone();
        let sqruff_service = match SqruffService::new(
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
            sql_log: cx.new(|cx| SqlLog::new(10, cx.theme().highlight_theme.clone())),
            sqruff_service,
            completion_provider: Some(sql_completion_provider),
            // Chat functionality
            chat_enabled: false,
            chat_panel: None,
            sql_log_visible: true,
            last_parameter_values: HashMap::new(),
        };

        self.tabs.push(TabType::Query(Box::new(query_tab)));
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
                panel.commit_changes_with_sql_log(window, &query_tab.sql_log, cx);
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
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.chat_enabled = !query_tab.chat_enabled;

            if query_tab.chat_enabled && query_tab.chat_panel.is_none() {
                // Create LLM instance for this connection
                match ChatProviderResolver::get_llm_for_connection(cx) {
                    Ok(llm_instance) => {
                        // Build the session context from QueryTab
                        let session_context = ChatSessionContext::new()
                            .with_input_state(query_tab.editor.downgrade())
                            .with_connection(
                                query_tab.connection_id,
                                query_tab.database_name.clone(),
                            );

                        // Create chat panel with the LLM instance
                        let llm_for_panel = llm_instance.llm.clone();
                        let chat_panel = cx.new(|cx| {
                            ChatPanel::new(
                                llm_for_panel,
                                llm_instance.provider_name.clone(),
                                llm_instance.model_name.clone(),
                                session_context,
                                window,
                                cx,
                            )
                        });
                        query_tab.chat_panel = Some(chat_panel);
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to create chat provider: {}. Not creating chat panel.",
                            e
                        );
                        query_tab.chat_enabled = false;
                    }
                }
            }

            cx.notify();
        }
    }

    pub fn toggle_sql_log_for_active_tab(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.sql_log_visible = !query_tab.sql_log_visible;
            cx.notify();
        }
    }

    /// Create a tab bar click handler closure
    fn tab_bar_click_handler(
        view: WeakEntity<Self>,
    ) -> impl Fn(&usize, &ClickEvent, &mut Window, &mut App) + 'static {
        move |ix: &usize, event: &ClickEvent, window: &mut Window, cx: &mut App| {
            view.update(cx, |this: &mut EditorPanel, cx| {
                if event.click_count() == 1 {
                    this.set_active_tab(*ix, window, cx);
                    return;
                }

                let tab = this.tabs.get(*ix);
                if let Some(TabType::Query(query_tab)) = tab {
                    let tab_title = query_tab.title.clone();

                    // Open rename modal on double click
                    let form = RenameTabForm::new(tab_title, window, cx);
                    let form_for_modal = form;
                    let tab_index = *ix;

                    window.open_dialog(cx, move |modal, _window, _cx| {
                        let form_clone = form_for_modal.clone();
                        let tab_index = tab_index;
                        modal
                            .title("Rename Tab")
                            .w(px(300.))
                            .child(form_for_modal.clone())
                            .footer(
                                DialogFooter::new()
                                    .child(
                                        DialogClose::new()
                                            .child(Button::new("cancel").label("Cancel").outline()),
                                    )
                                    .child(
                                        DialogAction::new()
                                            .child(Button::new("ok").primary().label("Rename")),
                                    ),
                            )
                            .on_ok({
                                let form = form_clone;
                                move |_modal, window, cx| {
                                    // Get the current value from the form
                                    let new_name = form.read(cx).get_value(cx);

                                    // Use the app's global action system instead of local context
                                    // Create a new RenameTab action and dispatch it through the app
                                    window.dispatch_action(
                                        Box::new(RenameTab {
                                            tab_index,
                                            new_name,
                                        }),
                                        cx,
                                    );
                                    true
                                }
                            })
                    });
                };
            })
            .log_err();
        }
    }

    fn with_active_results_panel(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ResultsPanel, &mut Context<ResultsPanel>),
    ) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.results_panel.update(cx, f);
        }
    }

    fn render_tab_bar_item(&self, ix: usize, tab: &TabType, cx: &mut Context<Self>) -> Tab {
        match tab {
            TabType::Query(query_tab) => {
                let show_close_button = self.tabs.len() > 1;
                let tab_index = ix;

                let connection_label = query_tab
                    .connection_name
                    .clone()
                    .unwrap_or_else(|| "No Connection".to_string());
                let group_env_type = query_tab.environment_type;
                let group_connection_label = connection_label.clone();

                Tab::new()
                    .label(&query_tab.title)
                    .group(connection_label)
                    .group_label(move |_, cx| {
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(group_connection_label.clone()),
                            )
                            .when_some(group_env_type, |this, env_type| {
                                this.child(
                                    div()
                                        .text_size(rems(0.55))
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .px(px(6.))
                                        .pt_0p5()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(env_type.get_color(cx))
                                        .text_color(env_type.get_color(cx))
                                        .child(env_type.display_name()),
                                )
                            })
                    })
                    .suffix(
                        h_flex()
                            .gap_1()
                            .pr_1()
                            .child(
                                div()
                                    .pr_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        query_tab
                                            .connection_name
                                            .clone()
                                            .unwrap_or_else(|| "No Connection".to_string()),
                                    ),
                            )
                            .when_some(query_tab.environment_type, |this, env_type| {
                                this.child(
                                    div()
                                        .text_size(rems(0.55))
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .px(px(6.))
                                        .pt_0p5()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(env_type.get_color(cx))
                                        .text_color(env_type.get_color(cx))
                                        .child(env_type.display_name()),
                                )
                            })
                            .when(show_close_button, |this| {
                                this.child(
                                    Button::new(("close-tab", ix))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.close_tab(tab_index, cx);
                                        })),
                                )
                            })
                            .into_any_element(),
                    )
            }
            TabType::Snippet(snippet_editor) => {
                let label = snippet_editor.read(cx).get_title();
                let tab_index = ix;

                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(IconName::File).text_color(cx.theme().green))
                        .child(
                            Button::new(("close-snippet-tab", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_tab(tab_index, cx);
                                })),
                        ),
                )
            }
            TabType::Settings(settings_tab) => {
                let label = settings_tab.title.clone();
                let tab_index = ix;

                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(IconName::Settings))
                        .child(
                            Button::new(("close-settings-tab", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_tab(tab_index, cx);
                                })),
                        ),
                )
            }
            TabType::ObjectDdl(object_ddl_tab) => {
                let inner = object_ddl_tab.read(cx);
                let label = SharedString::from(inner.title.clone());
                let (icon, color) = match inner.kind {
                    RoutineKind::Procedure => (IconName::SquareTerminal, cx.theme().magenta),
                    RoutineKind::Function => (IconName::Braces, cx.theme().cyan),
                    RoutineKind::Trigger => (IconName::DatabaseConnected, cx.theme().yellow),
                };
                let tab_index = ix;
                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(icon).text_color(color))
                        .child(
                            Button::new(("close-object-ddl-tab", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_tab(tab_index, cx);
                                })),
                        ),
                )
            }
            TabType::TableStructure(table_structure_tab) => {
                let label = table_structure_tab.read(cx).title.clone();
                let tab_index = ix;

                Tab::new().label(label).group("Other").suffix(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .pr_1()
                        .child(Icon::new(IconName::Sheet).text_color(cx.theme().blue))
                        .child(
                            Button::new(("close-table-structure-tab", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_tab(tab_index, cx);
                                })),
                        ),
                )
            }
        }
    }

    fn render_row_operations_bar(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .p_2()
            .gap_2()
            .border_t_1()
            .bg(cx.theme().title_bar)
            .border_color(cx.theme().border)
            .flex_wrap()
            .child(
                Button::new("add-row")
                    .outline()
                    .small()
                    .icon(IconName::Plus)
                    .label("Add")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.with_active_results_panel(cx, |panel, cx| {
                            panel.add_new_row(cx);
                        });
                    })),
            )
            .child(
                Button::new("duplicate-row")
                    .outline()
                    .small()
                    .icon(IconName::Copy)
                    .label("Duplicate")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.with_active_results_panel(cx, |panel, cx| {
                            panel.duplicate_row(cx);
                        });
                    })),
            )
            .child(
                Button::new("delete-row")
                    .outline()
                    .small()
                    .icon(IconName::Delete)
                    .label("Delete")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.with_active_results_panel(cx, |panel, cx| {
                            panel.delete_row(cx);
                        });
                    })),
            )
            .child(self.render_apply_edits_button(query_tab, cx))
            .child(
                Button::new("rollback-changes")
                    .outline()
                    .small()
                    .icon(IconName::CircleX)
                    .label("Discard edits")
                    .tooltip("Discard pending cell edits")
                    .disabled(!query_tab.results_panel.read(cx).has_pending_edits(cx))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.with_active_results_panel(cx, |panel, cx| {
                            panel.rollback_changes(window, cx);
                        });
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("toggle-sql-log")
                    .outline()
                    .small()
                    .icon(IconName::SquareTerminal)
                    .tooltip("Toggle SQL Log")
                    .when(query_tab.sql_log_visible, |btn| btn.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_sql_log_for_active_tab(window, cx);
                    })),
            )
            .child(
                Button::new("toggle-chat")
                    .outline()
                    .small()
                    .icon(IconName::Bot)
                    .tooltip("Toggle Chat")
                    .when(query_tab.chat_enabled, |btn| btn.primary())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_chat_for_active_tab(window, cx);
                    })),
            )
    }

    fn render_apply_edits_button(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_pending = query_tab.results_panel.read(cx).has_pending_edits(cx);
        let button = Button::new("commit-changes")
            .outline()
            .small()
            .icon(IconName::Check)
            .label("Apply edits")
            .tooltip("Apply pending cell edits to the database")
            .disabled(!has_pending);

        if !has_pending {
            return button.into_any_element();
        }

        let statements = query_tab.results_panel.read(cx).preview_pending_sql(cx);
        let preview_log = cx.new(|cx| {
            let mut log = SqlLog::new(usize::MAX, cx.theme().highlight_theme.clone());
            if statements.is_empty() {
                log.append_text(&SqlLogMessage::Comment("no statements to apply".into()), cx);
            } else {
                for statement in statements {
                    log.append_text(&SqlLogMessage::SqlStatement(statement), cx);
                }
            }
            log
        });
        let results_panel = query_tab.results_panel.clone();
        let sql_log = query_tab.sql_log.clone();

        Popover::new("commit-changes-popover")
            .trigger(button)
            .content(move |_state, _window, cx| {
                let results_panel = results_panel.clone();
                let sql_log = sql_log.clone();
                let preview_log = preview_log.clone();
                v_flex()
                    .p_2()
                    .gap_2()
                    .w(px(520.))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("Preview SQL"),
                    )
                    .child(div().h(px(280.)).child(preview_log))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("preview-cancel")
                                    .outline()
                                    .small()
                                    .label("Cancel")
                                    .on_click(cx.listener(
                                        |state: &mut PopoverState, _, window, cx| {
                                            state.dismiss(window, cx);
                                        },
                                    )),
                            )
                            .child(
                                Button::new("preview-confirm")
                                    .primary()
                                    .small()
                                    .icon(IconName::Check)
                                    .label("Confirm")
                                    .on_click({
                                        let results_panel = results_panel;
                                        let sql_log = sql_log;
                                        cx.listener(
                                            move |state: &mut PopoverState, _, window, cx| {
                                                results_panel.update(cx, |panel, cx| {
                                                    panel.commit_changes_with_sql_log(
                                                        window, &sql_log, cx,
                                                    );
                                                });
                                                state.dismiss(window, cx);
                                            },
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_query_tab_content(
        &self,
        query_tab: &QueryTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_resizable("editor-split")
            .with_state(&self.editor_chat_resize_state)
            .child(
                resizable_panel().child(
                    v_resizable("editor-results-split")
                        .with_state(&self.editor_results_resize_state)
                        .child(
                            resizable_panel().size(200.).child(
                                v_flex()
                                    .h_full()
                                    .w_full()
                                    .overflow_hidden()
                                    .min_w_0()
                                    .child(
                                        div().flex_1().min_h_0().w_full().relative().child(
                                            Input::new(&query_tab.editor)
                                                .bordered(false)
                                                .h_full()
                                                .w_full()
                                                .rounded_none()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_size(px(14.))
                                                .focus_bordered(false),
                                        ),
                                    ),
                            ),
                        )
                        .child(
                            resizable_panel().size(200.).child(
                                v_flex()
                                    .h_full()
                                    .w_full()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .p_2()
                                            .gap_2()
                                            .border_t_1()
                                            .border_color(cx.theme().border)
                                            .bg(cx.theme().title_bar)
                                            .justify_end()
                                            .child(
                                                Button::new("format-query")
                                                    .outline()
                                                    .small()
                                                    .icon(IconName::WandSparkles)
                                                    .label("Format")
                                                    .tooltip(format!(
                                                        "Format ({})",
                                                        self.format_query_keystroke
                                                    ))
                                                    .on_click(cx.listener(
                                                        |panel, _, window, cx| {
                                                            panel.format_current_query(window, cx)
                                                        },
                                                    )),
                                            )
                                            .map(|this| {
                                                if self.loading {
                                                    this.child(
                                                        Button::new("abort-query")
                                                            .danger()
                                                            .small()
                                                            .icon(IconName::SquareStop)
                                                            .label("Stop")
                                                            .tooltip("Abort the running query")
                                                            .on_click(cx.listener(
                                                                |panel, _, _window, cx| {
                                                                    panel.abort_running_query(cx);
                                                                },
                                                            )),
                                                    )
                                                } else {
                                                    this.child(
                                                        Button::new("explain-query")
                                                            .outline()
                                                            .small()
                                                            .icon(IconName::Map)
                                                            .label("Explain")
                                                            .tooltip(
                                                                "Run EXPLAIN on the statement \
                                                                 at the cursor",
                                                            )
                                                            .on_click(cx.listener(
                                                                |panel, _, window, cx| {
                                                                    panel.on_explain_query(
                                                                        window, cx,
                                                                    )
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("run-query")
                                                            .outline()
                                                            .small()
                                                            .icon(IconName::Play)
                                                            .label("Run Current")
                                                            .tooltip(format!(
                                                                "Run Current ({})",
                                                                self.run_query_keystroke
                                                            ))
                                                            .on_click(cx.listener(
                                                                |panel, _, window, cx| {
                                                                    panel.on_run_query(window, cx)
                                                                },
                                                            )),
                                                    )
                                                }
                                            }),
                                    )
                                    .child(div().flex_1().min_h_0().overflow_hidden().map(|d| {
                                        if query_tab.sql_log_visible {
                                            d.child(
                                                v_resizable("results-log-split")
                                                    .with_state(&self.results_log_resize_state)
                                                    .child(
                                                        resizable_panel()
                                                            .child(query_tab.results_panel.clone()),
                                                    )
                                                    .child(
                                                        resizable_panel()
                                                            .size(120.)
                                                            .child(query_tab.sql_log.clone()),
                                                    ),
                                            )
                                        } else {
                                            d.child(query_tab.results_panel.clone())
                                        }
                                    }))
                                    .child(self.render_row_operations_bar(query_tab, cx)),
                            ),
                        ),
                ),
            )
            .when(
                query_tab.chat_enabled && query_tab.chat_panel.is_some(),
                |this| {
                    this.child(
                        resizable_panel()
                            .size_range(px(500.)..gpui::Pixels::MAX)
                            .child(
                                div()
                                    .border_l_1()
                                    .border_color(cx.theme().border)
                                    .size_full()
                                    .min_h_0()
                                    .when_some(
                                        query_tab.chat_panel.as_ref(),
                                        |this, chat_panel| this.child(chat_panel.clone()),
                                    ),
                            ),
                    )
                },
            )
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_tab = self.tabs.get(self.active_tab_ix);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .on_action(
                cx.listener(|this, action: &ExecuteSubstitutedQuery, window, cx| {
                    this.execute_query(
                        action.query.clone(),
                        action.connection_id,
                        &action.database_name,
                        window,
                        cx,
                    );
                }),
            )
            .on_action(cx.listener(|this, _: &FormatQuery, window, cx| {
                this.format_current_query(window, cx);
            }))
            .child(
                TabBar::new("editor-tabs")
                    .menu(true)
                    .w_full()
                    .pt(px(4.))
                    .selected_index(self.active_tab_ix)
                    .on_click(Self::tab_bar_click_handler(cx.entity().downgrade()))
                    .prefix(
                        Button::new("toggle-sidebar")
                            .ghost()
                            .small()
                            .icon(if self.sidebar_collapsed {
                                Icon::new(IconName::PanelLeftOpen).size_4()
                            } else {
                                Icon::new(IconName::PanelLeftClose).size_4()
                            })
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(ToggleSidebar), cx);
                            })),
                    )
                    .children(
                        self.tabs
                            .iter()
                            .enumerate()
                            .map(|(ix, tab)| self.render_tab_bar_item(ix, tab, cx)),
                    )
                    .track_scroll(&self.tabbar_scroll_handle),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when_some(current_tab, |this, tab| match tab {
                        TabType::Query(query_tab) => {
                            this.child(self.render_query_tab_content(query_tab, cx))
                        }
                        TabType::Settings(settings_tab) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(settings_tab.settings_view.clone()),
                        ),
                        TabType::Snippet(snippet_editor) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(snippet_editor.clone()),
                        ),
                        TabType::TableStructure(table_structure_tab) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(table_structure_tab.clone()),
                        ),
                        TabType::ObjectDdl(object_ddl_tab) => this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(object_ddl_tab.clone()),
                        ),
                    }),
            )
    }
}
