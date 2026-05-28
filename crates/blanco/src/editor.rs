mod object_ddl;
mod parameter_form;
mod query_execution;
#[cfg(test)]
mod query_execution_test;
mod rename_form;
mod render;
mod snippet_editor;
mod sql_operations;
mod table_structure;
mod tabs;

pub use tabs::{
    ObjectDdlParams, QueryTab, SettingsTab, TabCreationParams, TabType, TableStructureParams,
};

use blanco_core::{ColumnInfo, IndexInfo};
use gpui::{
    AppContext, Context, Entity, FocusHandle, KeybindingKeystroke, Keystroke, Task, WeakEntity,
    Window,
};
use gpui_component::{
    ActiveTheme, WindowExt as _,
    input::{InputEvent, InputState, TabSize},
    resizable::ResizableState,
};
use std::{collections::HashMap, rc::Rc, sync::Arc};
use tracing::{debug, error, info};

use self::object_ddl::ObjectDdlTab;
use self::snippet_editor::SnippetEditor;
use self::table_structure::TableStructureTab;
use crate::agent::{ChatPanel, ChatProviderResolver, ChatSessionContext};
use crate::app_database::AppDatabase;
use crate::app_database::QueryTabData;
use crate::app_settings::AppSettings;
use crate::result_ext::ResultExt;
use crate::results_panel::ResultsPanel;
use crate::settings::SettingsView;
use crate::sql::{SqlCompletionProvider, SqlSelectionRangeProvider, SqruffService};
use blanco_ui::SqlLog;
use database::{DatabaseService, DatabaseServiceTrait};

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
}
