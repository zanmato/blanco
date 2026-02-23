use gpui::{
    App, AppContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeybindingKeystroke, Keystroke, ParentElement, Render,
    SharedString, Styled, Task, WeakEntity, Window, div, prelude::FluentBuilder, px, rems,
};
use gpui_component::{
    ActiveTheme, Sizable, WindowExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    highlighter::Diagnostic,
    input::{Input, InputEvent, InputState, TabSize},
    notification::NotificationType,
    resizable::{ResizableState, h_resizable, resizable_panel, v_resizable},
    tab::{Tab, TabBar},
    v_flex,
};
use ropey::Rope;
use std::{rc::Rc, sync::Arc, time::Duration};
use tracing::{debug, error, info};

use crate::agent::{ChatPanel, ChatSessionContext};
use crate::app::RenameTab;
use crate::app::{ExecuteSubstitutedQuery, FormatQuery};
use crate::app_database::AppDatabase;
use crate::app_database::{EnvironmentType, QueryTabData};
use crate::app_events::AppEvent;
use crate::app_settings::AppSettings;
use crate::parameter_form::ParameterForm;
use crate::rename_form::RenameTabForm;
use crate::results_panel::ResultsPanel;
use crate::snippet_editor::SnippetEditor;
use crate::sql_completion::SqlCompletionProvider;
use crate::sql_selection_range_provider::SqlSelectionRangeProvider;
use crate::sql_statement_parser::extract_statement_info;
use crate::sqruff_service::SqruffService;
use crate::table_structure::TableStructureTab;
use blanco_ui::{IconName, SqlLog};
use database::{DatabaseService, DatabaseServiceTrait};
use gpui_component::{Icon, RopeExt};

pub enum TabType {
    Query(QueryTab),
    Settings(SettingsTab),
    Snippet(Entity<SnippetEditor>),
    TableStructure(Entity<TableStructureTab>),
}

pub struct QueryTab {
    #[allow(dead_code)]
    pub id: usize,
    pub title: String,
    pub connection_id: i64,              // Connection ID from app database
    pub db_type: database::DatabaseType, // Database type for this connection
    pub connection_name: Option<String>, // Connection name from database
    pub database_name: String,           // Database name this tab is connected to
    pub schema_name: Option<String>,     // Optional schema name for context
    pub environment_type: Option<EnvironmentType>, // Environment type from connection
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>, // Database ID for persistence
    pub results_panel: Entity<crate::results_panel::ResultsPanel>, // Each tab has its own results
    pub sql_log: Entity<SqlLog>, // SQL log for this tab
    // Chat functionality
    pub chat_enabled: bool,
    pub chat_panel: Option<Entity<ChatPanel>>,
    pub sqruff_service: Option<Arc<SqruffService>>,
}

impl EventEmitter<AppEvent> for QueryTab {}

pub struct SettingsTab {
    #[allow(dead_code)]
    pub id: usize,
    pub title: String,
    pub settings_view: Entity<crate::settings::SettingsView>,
}

pub struct EditorPanel {
    focus_handle: FocusHandle,
    tabs: Vec<TabType>,
    active_tab_ix: usize,
    next_tab_id: usize,
    sidebar_collapsed: bool,
    tabbar_scroll_handle: gpui::ScrollHandle,
    _subscriptions: Vec<gpui::Subscription>,
    run_query_keystroke: KeybindingKeystroke,
    format_query_keystroke: KeybindingKeystroke,
    editor_chat_resize_state: Entity<ResizableState>,
    editor_results_resize_state: Entity<ResizableState>,
    loading: bool,
    _run_query_task: Task<()>,
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

        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        // Settings are now stored in the global AppDatabase
        let settings_view = cx.new(crate::settings::SettingsView::new);

        // Subscribe to settings view events to forward them to the app and update editors
        cx.subscribe_in(
            &settings_view,
            window,
            |editor_panel, _settings_view, event, window, cx| {
                match event {
                    AppEvent::EditorSettingChanged { setting, value } => {
                        // Forward the event to the BlancoApp
                        cx.emit(AppEvent::EditorSettingChanged {
                            setting: setting.clone(),
                            value: value.clone(),
                        });

                        // Also update all editors directly for immediate feedback
                        let value_bool = value.parse::<bool>().unwrap_or(false);
                        match setting.as_str() {
                            "word_wrap" => {
                                editor_panel.set_all_editors_soft_wrap(value_bool, window, cx);
                            }
                            "show_whitespace" => {
                                editor_panel
                                    .set_all_editors_show_whitespace(value_bool, window, cx);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            },
        )
        .detach();

        let settings_tab = SettingsTab {
            id: tab_id,
            title: "Settings".to_string(),
            settings_view,
        };

        self.tabs.push(TabType::Settings(settings_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    pub fn create_snippet_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.next_tab_id += 1;

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
        self.next_tab_id += 1;

        let snippet_editor = cx.new(|cx| SnippetEditor::new(window, cx));

        // Load snippet data
        let app_database = AppDatabase::global(cx);
        if let Ok(Some(snippet_data)) =
            smol::block_on(async { app_database.get_snippet_by_id(snippet_id).await })
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
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let tab = cx.new(|cx| {
            TableStructureTab::new(
                tab_id,
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

    pub fn update_last_table_structure_tab(
        &mut self,
        columns: Vec<blanco_core::connection_trait::ColumnInfo>,
        indexes: Vec<blanco_core::connection_trait::IndexInfo>,
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

    /// Handler for Run Query button/keyboard
    /// Checks for parameters and shows modal if needed, otherwise executes directly
    pub fn on_run_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Extract the values we need before any mutable borrows
        let tab_index = self.active_tab_ix;

        if let Some(tab) = self.tabs.get(tab_index)
            && let TabType::Query(query_tab) = tab
        {
            // Get text, cursor position, and selection from editor
            let editor = query_tab.editor.read(cx);
            let full_text = editor.text().to_string();
            let cursor_pos = editor.cursor();
            let selected_text = editor.selected_text().to_string();

            // Clone the values we need
            let connection_id = query_tab.connection_id;
            let database_name = query_tab.database_name.clone();

            // Use extract_statement_info to get parameters
            // For selected text, parse from the selection; otherwise use cursor position
            let statement_info = if !selected_text.trim().is_empty() {
                let selected_text = Rope::from_str(&selected_text);
                extract_statement_info(&selected_text, 0)
            } else {
                let full_text = Rope::from_str(&full_text);
                extract_statement_info(&full_text, cursor_pos)
            };

            // Determine the query to execute: use selected text if available, otherwise extract from statement_info
            let query = if !selected_text.trim().is_empty() {
                selected_text
            } else {
                statement_info
                    .as_ref()
                    .map(|info| info.text.clone())
                    .unwrap_or_default()
            };

            if query.is_empty() {
                window.push_notification((NotificationType::Error, "No query to execute"), cx);
                return;
            }

            // Check if query has parameters
            let has_params = statement_info
                .as_ref()
                .map(|info| !info.parameters.is_empty())
                .unwrap_or(false);

            tracing::info!("statement_info {:?}", statement_info);

            if has_params {
                // Show parameter modal instead of executing directly
                let params = statement_info.unwrap().parameters;
                self.show_parameter_modal(query, params, connection_id, database_name, window, cx);
            } else {
                // Execute directly
                self.execute_query(query, connection_id, &database_name, window, cx);
            }
        }
    }

    /// Execute a query with the given parameters
    fn execute_query(
        &mut self,
        query: String,
        _connection_id: i64,
        database_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Save the current tab before executing the query
        // Extract the tab data we need before starting async operations
        let tab_index = self.active_tab_ix;
        let tab = self.tabs.get(self.active_tab_ix);

        if let Some(TabType::Query(query_tab)) = tab {
            let content = query_tab.editor.read(cx).text().to_string();

            let _results_panel = query_tab.results_panel.clone();
            let _sql_log = query_tab.sql_log.clone();

            let connection_type = None;
            let tab_data = QueryTabData {
                id: query_tab.db_id,
                title: query_tab.title.clone(),
                content: content.clone(),
                position: tab_index as i32,
                connection_id: Some(query_tab.connection_id),
                connection_type,
                connection_name: query_tab.connection_name.clone(),
                database_name: Some(query_tab.database_name.clone()),
                schema_name: query_tab.schema_name.clone(),
                environment_type: query_tab.environment_type,
            };

            // Trigger the save operation in background
            let app_database = AppDatabase::global(cx).clone();
            let _title = query_tab.title.clone();
            let connection_id = query_tab.connection_id;

            cx.spawn(async move |entity_handle, cx| {
                // Create the final tab data with connection_id
                let mut final_tab_data = tab_data;
                final_tab_data.connection_id = Some(connection_id);

                // First save to get or create the database ID
                let tab_db_id = match app_database.save_query_tab(&final_tab_data).await {
                    Ok(db_id) => {
                        debug!("Tab saved successfully with db_id: {}", db_id);
                        db_id
                    }
                    Err(e) => {
                        error!("Failed to save tab: {}", e);
                        return;
                    }
                };

                let _ = entity_handle.update(cx, |editor_panel: &mut EditorPanel, _| {
                    if let Some(tab) = editor_panel.tabs.get_mut(editor_panel.active_tab_ix)
                        && let TabType::Query(query_tab) = tab
                    {
                        query_tab.db_id = Some(tab_db_id);
                    }
                });
            })
            .detach();

            // Set loading state to true
            self.loading = true;
            cx.notify();

            // Log the query to the SQL log, truncating if too long
            let query_for_log = if query.len() > SQL_QUERY_LOG_MAX_LENGTH {
                format!("{}... (truncated)", &query[..SQL_QUERY_LOG_MAX_LENGTH])
            } else {
                query.clone()
            };
            query_tab.sql_log.update(cx, |sql_log, cx| {
                sql_log.append_text(&blanco_ui::SqlLogMessage::SqlStatement(query_for_log), cx);
            });

            // Clone values for background task
            let query_clone = query.clone();
            let db_service = DatabaseService::global(cx).clone();
            let database_name = database_name.to_string();
            let database_name_for_background = database_name.clone();

            // Spawn background task for query execution (keeps UI responsive)
            let query_task = cx.background_spawn(async move {
                let start_time = std::time::Instant::now();
                let execution_result = db_service
                    .execute_query(
                        connection_id,
                        Some(&database_name_for_background),
                        &query_clone,
                    )
                    .await;
                (execution_result, start_time)
            });

            // Spawn foreground task to handle the result and update UI
            let results_panel_clone = query_tab.results_panel.clone();
            let sql_log_clone = query_tab.sql_log.clone();
            let db_service = DatabaseService::global(cx).clone();
            let query_for_metadata = query.clone();

            self._run_query_task = cx.spawn_in(window, async move |editor_panel_entity, window| {
                let (execution_result, start_time) = query_task.await;

                match execution_result {
                    Ok(mut result) => {
                        let duration_ms = start_time.elapsed().as_millis() as i64;

                        // Add execution metadata
                        result.query_text = Some(query_for_metadata.clone());
                        result.execution_time_ms = Some(duration_ms);
                        result.is_error = false;
                        result.connection_id = Some(connection_id);

                        // Extract table metadata and get columns - still need connection for this
                        if let Ok(connection) = db_service
                            .get_or_create_connection_by_id(connection_id, Some(&database_name))
                            .await
                        {
                            // Extract table metadata from the query
                            let table_name = connection
                                .extract_table_name_from_query(&query_for_metadata, false)
                                .ok()
                                .flatten();
                            result.table_name = table_name.clone();

                            // Load full table metadata (including primary keys and foreign keys)
                            if let (Some(table_name), false) = (&table_name, result.rows.is_empty())
                            {
                                if let Ok(columns) =
                                    connection.get_columns_for_table(table_name, None).await
                                {
                                    result.table_columns = Some(columns);
                                }
                            }
                        }

                        // Store rows_affected before moving result
                        let rows_affected =
                            std::cmp::max(result.rows_affected, result.row_count() as u64);

                        let _ = window.update(move |window, cx| {
                            // Update results panel
                            results_panel_clone.update(cx, |panel, cx| {
                                panel.set_query_result(result, Some(connection_id), window, cx);
                            });

                            // Log execution result to SQL log
                            sql_log_clone.update(cx, |sql_log, cx| {
                                let log_message = format!(
                                    "{}, {} rows in {}",
                                    crate::time_format::format_current_timestamp(),
                                    rows_affected,
                                    crate::time_format::format_duration(duration_ms)
                                );
                                sql_log.append_text(
                                    &blanco_ui::SqlLogMessage::Comment(log_message),
                                    cx,
                                );
                            });

                            // Set loading to false and emit success event
                            editor_panel_entity
                                .update(cx, |editor_panel, cx| {
                                    editor_panel.loading = false;
                                    cx.emit(AppEvent::QueryExecutionCompleted {
                                        connection_id: Some(connection_id),
                                        database_name: Some(database_name.clone()),
                                        success: true,
                                        execution_time: start_time.elapsed(),
                                        rows_affected: Some(rows_affected),
                                        error_message: None,
                                    });
                                    cx.notify();
                                })
                                .ok();
                        });
                    }
                    Err(e) => {
                        tracing::error!("Query execution failed: {}", e);

                        let _ = window.update(|window, cx| {
                            // Log execution error to SQL log
                            let _error_duration = start_time.elapsed().as_millis() as i64;
                            sql_log_clone.update(cx, |sql_log, cx| {
                                let log_message = format!("query execution failed: {}", e);
                                sql_log.append_text(
                                    &blanco_ui::SqlLogMessage::Comment(log_message),
                                    cx,
                                );
                            });

                            // Set loading to false and emit error event
                            editor_panel_entity
                                .update(cx, |editor_panel, cx| {
                                    editor_panel.loading = false;
                                    cx.notify();
                                    cx.emit(AppEvent::QueryExecutionCompleted {
                                        connection_id: Some(connection_id),
                                        database_name: Some(database_name.clone()),
                                        success: false,
                                        execution_time: start_time.elapsed(),
                                        rows_affected: None,
                                        error_message: Some(e.to_string()),
                                    });
                                })
                                .ok();

                            editor_panel_entity
                                .update(cx, |_, cx| {
                                    cx.emit(AppEvent::ErrorOccurred {
                                        context: format!(
                                            "Query execution on connection_id: {}",
                                            connection_id,
                                        ),
                                        error: e.to_string(),
                                        severity: crate::app_events::ErrorSeverity::Error,
                                    });
                                })
                                .ok();

                            window.push_notification(
                                (NotificationType::Error, SharedString::from(e.to_string())),
                                cx,
                            );
                        });
                    }
                }
            });
        }
    }

    /// Show parameter input modal and execute query with substituted values
    fn show_parameter_modal(
        &mut self,
        query: String,
        params: Vec<crate::sql_statement_parser::QueryParameter>,
        connection_id: i64,
        database_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let param_form = cx.new(|cx| ParameterForm::new(query.clone(), params, window, cx));

        let weak_editor_panel = cx.entity().downgrade();
        window.open_dialog(cx, move |modal, _, _| {
            modal
                .title("Query Parameters")
                .w(px(500.))
                .child(param_form.clone())
                .confirm()
                .on_ok({
                    let param_form = param_form.clone();
                    let database_name = database_name.clone();
                    let weak_editor_panel = weak_editor_panel.clone();
                    move |_event, window, cx| {
                        let substituted_query = param_form.read(cx).get_substituted_query(cx);
                        weak_editor_panel
                            .update(cx, |editor_panel, cx| {
                                editor_panel.execute_query(
                                    substituted_query,
                                    connection_id,
                                    &database_name,
                                    window,
                                    cx,
                                );
                            })
                            .ok();
                        true // Close dialog
                    }
                })
        });
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

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![],
            active_tab_ix: 0,
            next_tab_id: 0,
            sidebar_collapsed,
            tabbar_scroll_handle: gpui::ScrollHandle::default(),
            _subscriptions: Vec::new(),
            run_query_keystroke: KeybindingKeystroke::from_keystroke(
                Keystroke::parse("secondary-enter").unwrap(),
            ),
            format_query_keystroke: KeybindingKeystroke::from_keystroke(
                Keystroke::parse("shift-alt-f").unwrap(),
            ),
            editor_chat_resize_state,
            editor_results_resize_state,
            loading: false,
            _run_query_task: Task::ready(()),
            _lint_debounce_task: Task::ready(()),
        };

        panel.restore_saved_tabs_with_connections_sync(saved_tabs, window, cx);

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

                debug!(
                    "Restoring tab '{}' with connection id: {:?}",
                    tab_title, tab_data.connection_id,
                );

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
                    schema_name: None, // TODO: Load from database when schema is added
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
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let editor = cx.new(|cx| {
            // Read settings
            let word_wrap = AppSettings::global(cx).settings.editor.word_wrap;
            let show_whitespace = AppSettings::global(cx).settings.editor.show_whitespace;

            let mut editor = InputState::new(window, cx)
                .code_editor("sql".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 2,
                    hard_tabs: false,
                })
                .soft_wrap(word_wrap)
                .show_whitespaces(show_whitespace);

            // Set up completion provider using connection_id, database_name, and DbService
            let db_service: Arc<dyn DatabaseServiceTrait> =
                Arc::new(DatabaseService::global(cx).clone());
            let completion_provider = SqlCompletionProvider::new(
                params.connection_id,
                params.database_name.clone(),
                db_service,
            );
            let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                Rc::new(completion_provider);
            editor.lsp.completion_provider = Some(completion_provider);

            // Set up selection range provider for SQL statement highlighting
            let selection_range_provider = SqlSelectionRangeProvider::new()
                .map_err(|e| error!("Failed to create selection range provider: {}", e))
                .ok();
            if let Some(provider) = selection_range_provider {
                let selection_range_provider: Rc<
                    dyn gpui_component::input::SelectionRangeProvider,
                > = Rc::new(provider);
                editor.lsp.selection_range_provider = Some(selection_range_provider);
            }

            editor
        });

        // Set content if provided
        if let Some(content) = params.content {
            editor.update(cx, |state, cx| {
                state.replace(&content, window, cx);
            });
        }

        // Subscribe to editor text changes for auto-linting
        let subscription = cx.subscribe_in(&editor, window, |this, _editor, event, window, cx| {
            // Only lint if the event is a text change
            if let InputEvent::SelectionRangeChange { range } = event {
                tracing::debug!("Selection range changed, linting current query");
                this.lint_current_query_debounced(*range, cx);
            } else if let InputEvent::PressEnter { secondary } = event
                && *secondary
            {
                this.on_run_query(window, cx);
            }
        });
        self._subscriptions.push(subscription);

        // Create SqruffService for this tab
        let sqruff_service = match SqruffService::new(&params.db_type.to_sqruff_dialect()) {
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
        let query_tab = QueryTab {
            id: tab_id,
            title: params.title.clone(),
            connection_id: params.connection_id,
            db_type: params.db_type.clone(),
            connection_name: params.connection_name.clone(),
            database_name: params.database_name.clone(),
            schema_name: params.schema_name.clone(),
            environment_type: params.environment_type,
            editor: editor.clone(),
            db_id: params.db_id,
            results_panel: cx.new(|cx| {
                ResultsPanel::new(params.connection_id, &params.database_name, window, cx)
            }),
            sql_log: cx.new(|cx| SqlLog::new(1000, cx.theme().highlight_theme.clone())),
            sqruff_service,
            // Chat functionality
            chat_enabled: false,
            chat_panel: None,
        };

        self.tabs.push(TabType::Query(query_tab));
        self.active_tab_ix = tab_id;
        self.scroll_tabbar_to_the_end(window, cx);

        cx.notify();
    }

    fn scroll_tabbar_to_the_end(&self, window: &mut Window, _: &mut Context<Self>) {
        let scroll_handle = self.tabbar_scroll_handle.clone();
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, _| {
                let max_offset = scroll_handle.max_offset();
                scroll_handle.set_offset(gpui::point(-max_offset.width, gpui::px(0.0)));
            })
        });
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
                match crate::agent::ChatProviderResolver::get_llm_for_connection(cx) {
                    Ok(llm_instance) => {
                        // Build the session context from QueryTab
                        let session_context = ChatSessionContext::new()
                            .with_input_state(query_tab.editor.downgrade())
                            .with_connection(
                                query_tab.connection_id,
                                query_tab.database_name.clone(),
                            );

                        // Create chat panel with the LLM instance
                        // Clone the Arc to share the LLM instance between sessions
                        let llm_for_panel = llm_instance.llm.clone();
                        let chat_panel = cx.new(|cx| {
                            ChatPanel::new(
                                query_tab.id,
                                llm_for_panel,
                                llm_instance.provider_name.clone(),
                                llm_instance.model_name.clone(),
                                session_context,
                                window,
                                cx,
                            )
                        });
                        query_tab.chat_panel = Some(chat_panel);

                        // Emit chat session started event
                        cx.emit(crate::app_events::AppEvent::ChatSessionStarted {
                            tab_id: query_tab.id,
                            provider: llm_instance.provider_name,
                            model: llm_instance.model_name,
                        });
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to create chat provider: {}. Not creating chat panel.",
                            e
                        );
                        query_tab.chat_enabled = false; // Disable chat if creation failed
                    }
                }
            } else if !query_tab.chat_enabled {
                // Emit chat session ended event
                cx.emit(crate::app_events::AppEvent::ChatSessionEnded {
                    tab_id: query_tab.id,
                });
            }

            // Emit chat toggled event
            cx.emit(crate::app_events::AppEvent::ChatToggled {
                tab_id: query_tab.id,
                enabled: query_tab.chat_enabled,
            });

            cx.notify();
        }
    }

    /// Trigger a debounced lint of the current query.
    ///
    /// This cancels any pending lint task and schedules a new one after the debounce delay.
    fn lint_current_query_debounced(&mut self, range: lsp_types::Range, cx: &mut Context<Self>) {
        // Cancel any pending lint task by dropping it
        self._lint_debounce_task = Task::ready(());

        self._lint_debounce_task = cx.spawn(async move |entity_handle, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(LINT_DEBOUNCE_MS))
                .await;

            let _ = entity_handle.update(cx, |this, cx| {
                this.lint_current_query(range, cx);
            });
        });
    }

    /// Lint the current query/statement and update editor diagnostics.
    fn lint_current_query(&mut self, range: lsp_types::Range, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;

        let Some(TabType::Query(query_tab)) = self.tabs.get(tab_index) else {
            return;
        };

        let editor = query_tab.editor.clone();

        // Get the text for the given range
        let (statement_text, byte_range) = {
            let editor_ref = editor.read(cx);
            let text = editor_ref.text();

            // Convert LSP range to byte offsets
            let start_byte = text.position_to_offset(&range.start);
            let end_byte = text.position_to_offset(&range.end);

            // Extract text from the range
            let extracted = text.slice(start_byte..end_byte.min(text.len())).to_string();

            (extracted, start_byte..end_byte)
        };

        let sqruff_service = if let Some(sqruff_service) = &query_tab.sqruff_service {
            sqruff_service.clone()
        } else {
            tracing::warn!("Sqruff service not available for linting");
            return;
        };

        // Background task: do the heavy linting work
        let lint_task = cx.background_spawn(async move {
            sqruff_service.lint(&statement_text, Some(byte_range.start))
        });

        // Foreground task: wait for background task to complete and update UI
        cx.spawn(async move |entity_handle, async_cx| {
            let diagnostics = match lint_task.await {
                Ok(diags) => diags,
                Err(e) => {
                    tracing::error!("Linting failed: {}", e);
                    return Ok::<(), anyhow::Error>(());
                }
            };

            // Update editor diagnostics
            // The diagnostics from sqruff are relative to the statement text,
            // so we need to adjust them to the full file position
            let _ = entity_handle.update(async_cx, |editor_panel, cx| {
                if let Some(TabType::Query(query_tab)) = editor_panel.tabs.get_mut(tab_index) {
                    query_tab.editor.update(cx, |state, cx| {
                        // Calculate the statement start position
                        let start_char = state.text().byte_to_char_idx(byte_range.start);
                        let start_pos = state.text().offset_to_position(start_char);

                        // Adjust diagnostics by adding the statement start position
                        let adjusted_diagnostics: Vec<Diagnostic> = diagnostics
                            .into_iter()
                            .map(|diag| {
                                let start = lsp_types::Position::new(
                                    diag.range.start.line + start_pos.line,
                                    if diag.range.start.line == 0 {
                                        diag.range.start.character + start_pos.character
                                    } else {
                                        diag.range.start.character
                                    },
                                );
                                let end = lsp_types::Position::new(
                                    diag.range.end.line + start_pos.line,
                                    if diag.range.end.line == 0 {
                                        diag.range.end.character + start_pos.character
                                    } else {
                                        diag.range.end.character
                                    },
                                );
                                Diagnostic::new(start..end, diag.message)
                                    .with_severity(diag.severity)
                            })
                            .collect();

                        state.diagnostics_mut().map(|set| {
                            set.clear();
                            set.extend(adjusted_diagnostics);
                        });
                        cx.notify();
                    });
                }
            });

            Ok(())
        })
        .detach();
    }

    /// Format the current query/statement.
    fn format_current_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_index = self.active_tab_ix;

        let Some(TabType::Query(query_tab)) = self.tabs.get(tab_index) else {
            window.push_notification((NotificationType::Error, "No query tab active"), cx);
            return;
        };

        let editor = query_tab.editor.clone();

        // Get current text and cursor position
        let (text, cursor_pos) = {
            let editor_ref = editor.read(cx);
            (editor_ref.text().clone(), editor_ref.cursor())
        };

        // Extract current statement using existing function
        let Some(statement_info) = extract_statement_info(&text, cursor_pos) else {
            window.push_notification((NotificationType::Info, "No query found at cursor"), cx);
            return;
        };

        let statement_text = statement_info.text.clone();
        let byte_range = statement_info.byte_range.clone();

        // Format in background
        let sqruff_service = if let Some(sqruff_service) = &query_tab.sqruff_service {
            sqruff_service.clone()
        } else {
            tracing::warn!("Sqruff service not available for linting");
            return;
        };

        cx.spawn_in(window, async move |entity_handle, window| {
            let formatted_result = sqruff_service.format(&statement_text);
            let formatted = match formatted_result {
                Ok(f) => f,
                Err(e) => {
                    let _ = window.update(|_window, cx| {
                        let error_message = SharedString::from(format!("Format failed: {}", e));
                        _window.push_notification((NotificationType::Error, error_message), cx);
                    });
                    return Ok::<(), anyhow::Error>(());
                }
            };

            let _ = entity_handle.update_in(window, |editor_panel, window, cx| {
                if let Some(TabType::Query(query_tab)) =
                    editor_panel.tabs.get_mut(editor_panel.active_tab_ix)
                {
                    query_tab.editor.update(cx, |state, cx| {
                        // Replace the statement range with formatted text
                        // Create an LSP TextEdit for the replacement
                        let start = state.text().byte_to_char_idx(byte_range.start);
                        let end = state.text().byte_to_char_idx(byte_range.end);
                        let start_pos = state.text().offset_to_position(start);
                        let end_pos = state.text().offset_to_position(end);
                        let text_edit = lsp_types::TextEdit {
                            range: lsp_types::Range {
                                start: start_pos,
                                end: end_pos,
                            },
                            new_text: formatted,
                            ..Default::default()
                        };
                        state.apply_lsp_edits(&vec![text_edit], window, cx);
                    });

                    window.push_notification((NotificationType::Success, "Query formatted"), cx);
                }
            });

            Ok(())
        })
        .detach();
    }

    /// Create a tab bar click handler closure
    fn tab_bar_click_handler(
        view: WeakEntity<Self>,
    ) -> impl Fn(&usize, &ClickEvent, &mut Window, &mut App) + 'static {
        move |ix: &usize, event: &ClickEvent, window: &mut Window, cx: &mut App| {
            let _ = view.update(cx, |this: &mut EditorPanel, cx| {
                if event.click_count() == 1 {
                    this.set_active_tab(*ix, window, cx);
                    return;
                }

                let tab = this.tabs.get(*ix);
                if let Some(TabType::Query(query_tab)) = tab {
                    let tab_title = query_tab.title.clone();

                    // Open rename modal on double click
                    let form = RenameTabForm::new(tab_title, window, cx);
                    let form_for_modal = form.clone();
                    let tab_index = *ix;

                    window.open_dialog(cx, move |modal, _window, _cx| {
                        let form_clone = form_for_modal.clone();
                        let tab_index = tab_index;
                        modal
                            .title("Rename Tab")
                            .w(px(300.))
                            .child(form_for_modal.clone())
                            .footer({
                                let _form = form_clone.clone();
                                move |ok, cancel, window, cx| {
                                    vec![cancel(window, cx), ok(window, cx)]
                                }
                            })
                            .on_ok({
                                let form = form_clone.clone();
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
            });
        }
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<AppEvent> for EditorPanel {}

impl Render for EditorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_tab = self.tabs.get(self.active_tab_ix);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .on_action(cx.listener(|this, action: &ExecuteSubstitutedQuery, window, cx| {
                this.execute_query(
                    action.query.clone(),
                    action.connection_id,
                    &action.database_name,
                    window,
                    cx,
                );
            }))
            .on_action(cx.listener(|this, _: &FormatQuery, window, cx| {
                this.format_current_query(window, cx);
            }))
            .child(
                // Tab bar
                TabBar::new("editor-tabs")
                    .menu(true)
                    .w_full()
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
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(AppEvent::ToggleSidebar);
                            }))
                    )
                    .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                        match tab {
                            TabType::Query(query_tab) => {
                                let show_close_button = self.tabs.len() > 1;
                                let tab_index = ix;

                                Tab::new()
                                    .label(& query_tab.title)
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
                                                        query_tab.connection_name.clone()
                                                            .unwrap_or_else(|| "No Connection".to_string())
                                                    )
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
                                                        .child(env_type.display_name())
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
                                                        }))
                                                )
                                            })
                                            .into_any_element()
                                    )
                            }
                            TabType::Snippet(snippet_editor) => {
                                let label = snippet_editor.read(cx).get_title();
                                let tab_index = ix;

                                Tab::new()
                                    .label(label)
                                    .suffix(
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
                                                    }))
                                            )
                                    )
                            }
                            TabType::Settings(settings_tab) => {
                                let label = settings_tab.title.clone();
                                let tab_index = ix;

                                Tab::new()
                                    .label(label)
                                    .suffix(
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
                                                    }))
                                            )
                                    )
                            }
                            TabType::TableStructure(table_structure_tab) => {
                                let label = table_structure_tab.read(cx).title.clone();
                                let tab_index = ix;

                                Tab::new()
                                    .label(label)
                                    .suffix(
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
                                                    }))
                                            )
                                    )
                            }
                        }
                    }))
                    .track_scroll(&self.tabbar_scroll_handle)
            )
            // Render the active tab's complete view
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when_some(current_tab, |this, tab| {
                match tab {
                    TabType::Query(query_tab) => {
                        // Query tab: Layout with optional chat panel
                        this.child(
                            h_resizable("editor-split")
                                .with_state(&self.editor_chat_resize_state)
                                // Left side: Always show the main content (Editor + Button bar + Results)
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
                                                        // Editor
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_h_0()
                                                                .relative() // Make container relative for absolute popup positioning
                                                                .child(
                                                                    Input::new(&query_tab.editor)
                                                                        .bordered(false)
                                                                        .h_full()
                                                                        .rounded_none()
                                                                        .font_family(cx.theme().mono_font_family.clone())
                                                                        .text_size(px(14.))
                                                                        .focus_bordered(false)
                                                                )
                                                        )
                                                )
                                            ).child(
                                                resizable_panel().size(200.).child(
                                                    v_flex()
                                                        .h_full()
                                                        .w_full()
                                                        .overflow_hidden()
                                                        .min_w_0()
                                                        // Button bar (between editor and results)
                                                        .child(
                                                            h_flex()
                                                                .p_2()
                                                                .gap_2()
                                                                .border_t_1()
                                                                .border_color(cx.theme().border)
                                                                .bg(cx.theme().title_bar)
                                                                .justify_end()
                                                                // Format button (left side)
                                                                .child(
                                                                    Button::new("format-query")
                                                                        .outline()
                                                                        .small()
                                                                        .icon(IconName::WandSparkles)
                                                                        .label("Format")
                                                                        .tooltip(format!("Format ({})", self.format_query_keystroke))
                                                                        .on_click(cx.listener(|panel, _, window, cx| panel.format_current_query(window, cx)))
                                                                )
                                                                // Run button (right side)
                                                                .child(
                                                                    Button::new("run-query")
                                                                        .outline()
                                                                        .small()
                                                                        .icon(IconName::Play)
                                                                        .label("Run Current")
                                                                        .loading(self.loading)
                                                                        .loading_icon(IconName::LoaderCircle)
                                                                        .tooltip(format!("Run Current ({})", self.run_query_keystroke))
                                                                        .on_click(cx.listener(|panel, _, window, cx| panel.on_run_query(window, cx))),
                                                                )
                                                        )
                                                        // Results section: Results on top, SQL Log on bottom
                                                        .child(
                                                            v_flex()
                                                                .flex_grow()
                                                                .min_h(px(200.))
                                                                // Results panel (top)
                                                                .child(
                                                                    div()
                                                                        .flex_1()
                                                                        .child(query_tab.results_panel.clone())
                                                                )
                                                                // SQL Log panel (bottom)
                                                                .child(
                                                                    div()
                                                                        .flex_1()
                                                                        .max_h(px(160.))
                                                                        .overflow_hidden()
                                                                        .bg(cx.theme().highlight_theme.style.editor_background.unwrap_or(cx.theme().background))
                                                                        .child(query_tab.sql_log.clone())
                                                                )
                                                        )
                                                        // Row operation buttons
                                                        .child(
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
                                                                            // Add a new row with empty values
                                                                            if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                                                                                query_tab.results_panel.update(cx, |results_panel, cx| {
                                                                                    results_panel.add_new_row(cx);
                                                                                });
                                                                            }
                                                                        })),
                                                                )
                                                                .child(
                                                                    Button::new("duplicate-row")
                                                                        .outline()
                                                                        .small()
                                                                        .icon(IconName::Copy)
                                                                        .label("Duplicate")
                                                                        .on_click(cx.listener(|this, _, _window, cx| {
                                                                            if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                                                                                query_tab.results_panel.update(cx, |results_panel, cx| {
                                                                                    results_panel.duplicate_row(cx);
                                                                                });
                                                                            }
                                                                        }))
                                                                )
                                                                // Commit and rollback buttons (always available)
                                                                .child(
                                                                    Button::new("commit-changes")
                                                                        .outline()
                                                                        .small()
                                                                        .icon(IconName::Check)
                                                                        .label("Commit")
                                                                        .on_click(cx.listener(|this, _, window, cx| {
                                                                            if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                                                                                // Execute the actual commit in the results panel with SQL logging
                                                                                query_tab.results_panel.update(cx, |panel, cx| {
                                                                                    panel.commit_changes_with_sql_log(window, &query_tab.sql_log, cx);
                                                                                });
                                                                            }
                                                                        })),
                                                                )
                                                                .child(
                                                                    Button::new("rollback-changes")
                                                                        .outline()
                                                                        .small()
                                                                        .icon(IconName::CircleX)
                                                                        .label("Rollback")
                                                                        .on_click(cx.listener(|this, _, _window, cx| {
                                                                            if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                                                                                query_tab.results_panel.update(cx, |panel, cx| {
                                                                                    panel.rollback_changes(_window, cx);
                                                                                });
                                                                            }
                                                                        })),
                                                                )
                                                                .child(div().flex_1())
                                                                // Chat toggle button
                                                                .child(
                                                                    Button::new("toggle-chat")
                                                                        .outline()
                                                                        .small()
                                                                        .icon(IconName::Bot)
                                                                        .when(query_tab.chat_enabled, |btn| {
                                                                            btn.primary()
                                                                        })
                                                                        .on_click(cx.listener(|this, _, _window, cx| {
                                                                            this.toggle_chat_for_active_tab(_window, cx);
                                                                        }))
                                                                ),
                                                            ),
                                                        ),
                                        ),
                                    ),
                                )
                                // Right side: Chat panel (only when enabled)
                                .when(
                                    query_tab.chat_enabled && query_tab.chat_panel.is_some(),
                                    |this| {
                                        this.child(
                                            resizable_panel().size_range(px(500.)..gpui::Pixels::MAX).child(
                                                div()
                                                    .border_l_1()
                                                    .border_color(cx.theme().border)
                                                    .size_full()
                                                    .min_h_0()
                                                    .child(
                                                        query_tab.chat_panel.as_ref().unwrap().clone(),
                                                    ),
                                            ),
                                        )
                                    }
                                )
                        )
                    }
                    TabType::Settings(settings_tab) => {
                        // Settings tab: Show the new settings view
                        this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(settings_tab.settings_view.clone())
                        )
                    }
                    TabType::Snippet(snippet_editor) => {
                        // Snippet tab: Show the snippet editor
                        this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(snippet_editor.clone())
                        )
                    }
                    TabType::TableStructure(table_structure_tab) => {
                        // Table structure tab: Show column and index information
                        this.child(
                            div()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                .child(table_structure_tab.clone())
                        )
                    }
                }
            }))
    }
}
