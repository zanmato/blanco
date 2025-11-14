use gpui::{
    App, AppContext, Axis, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeybindingKeystroke, Keystroke, MouseButton, ParentElement,
    Render, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt, WindowExt as _,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState, TabSize},
    kbd::Kbd,
    resizable::{ResizableState, h_resizable, resizable_panel, v_resizable},
    tab::{Tab, TabBar},
    v_flex,
};
use log::{debug, error, info};
use std::{rc::Rc, sync::Arc};

use crate::agent::{ChatPanel, SqlContext};
use crate::app::RenameTab;
use crate::app_database::QueryTabData;
use crate::app_events::AppEvent;
use crate::chat_provider_resolver::ChatProviderResolver;
use crate::db_service::DbService;
use crate::rename_form::RenameTabForm;
use crate::results_panel::ResultsPanel;
use crate::settings::{Settings, load_settings};
use crate::sql_completion_provider::SqlCompletionProvider;
use blanco_core::chat_provider::{ChatProvider, ProviderError};
use blanco_ui::{IconName, SqlLog};
use gpui_component::Icon;

#[derive(Clone)]
pub enum EditorPanelEvent {
    // No longer needed - tabs handle their own views
}

pub enum TabType {
    Query(QueryTab),
    Settings(SettingsTab),
}

pub struct QueryTab {
    #[allow(dead_code)]
    pub id: usize,
    pub title: String,
    pub connection_id: i64,              // Connection ID from app database
    pub connection_name: Option<String>, // Connection name from database
    pub database_name: String,           // Database name this tab is connected to
    pub schema_name: Option<String>,     // Optional schema name for context
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>, // Database ID for persistence
    pub results_panel: Entity<crate::results_panel::ResultsPanel>, // Each tab has its own results
    pub sql_log: Entity<SqlLog>, // SQL log for this tab
    // Chat functionality
    pub chat_enabled: bool,
    pub chat_panel: Option<Entity<ChatPanel>>,
}

impl QueryTab {
    /// Get SQL context for the chat session
    pub fn get_sql_context(&self, cx: &mut gpui::App) -> SqlContext {
        let current_query = self.editor.read(cx).text().to_string();
        let connection_id = if self.connection_id != 0 {
            Some(self.connection_id)
        } else {
            None
        };

        // Note: Resolving connection_type from connection_id would require async context
        // For now, we leave database_type as None - the chat tools can resolve it when needed
        let database_type = None;

        let mut context = SqlContext::with_query(current_query);
        context.connection_id = connection_id;
        context.database_type = database_type;

        // TODO: Add recent results when we implement a public method in ResultsPanel
        // For now, we'll leave recent_results as None

        context
    }
}

impl EventEmitter<AppEvent> for QueryTab {}

pub struct SettingsTab {
    #[allow(dead_code)]
    pub id: usize,
    pub title: String,
    pub editor: Entity<InputState>,
    pub original_settings: Settings,
    pub is_valid: bool,
    pub validation_error: Option<String>,
    pub pending_text: Option<String>,
}

pub struct EditorPanel {
    focus_handle: FocusHandle,
    tabs: Vec<TabType>,
    active_tab_ix: usize,
    next_tab_id: usize,
    sidebar_collapsed: bool,
    _subscriptions: Vec<gpui::Subscription>,
    // Temporary storage for saved tabs that will be restored after connections are loaded
    pending_saved_tabs: Option<Vec<crate::app_database::QueryTabData>>,
    run_query_keystroke: KeybindingKeystroke,
    editor_chat_resize_state: Entity<ResizableState>,
    editor_results_resize_state: Entity<ResizableState>,
}

/// Parameters for creating a new tab with connection
#[derive(Clone)]
pub struct TabCreationParams {
    pub title: String,
    pub content: Option<String>,
    pub db_id: Option<i64>,
    pub connection_id: i64,
    #[allow(dead_code)]
    pub connection_type: String,
    pub connection_name: Option<String>,
    pub database_name: String,
    pub schema_name: Option<String>,
}

impl EditorPanel {
    pub fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        cx.notify();
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
                let db_service = DbService::global(cx).clone();
                let app_db = db_service.app_db_handle();

                cx.spawn(async move |_, _cx| {
                    if let Some(app_db) = app_db.read().await.as_ref() {
                        app_db
                            .delete_query_tab(db_id)
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to delete tab: {}", e))
                    } else {
                        Err(anyhow::anyhow!("App database not initialized"))
                    }
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
                let db_service = DbService::global(cx).clone();
                let app_db = db_service.app_db_handle();
                let new_name = new_name.to_string(); // Convert to owned String

                cx.spawn(async move |_, _cx| {
                    if let Some(app_db) = app_db.read().await.as_ref() {
                        // Load existing tab data to preserve all fields
                        if let Ok(Some(existing_tab)) = app_db.load_query_tab_by_id(db_id).await {
                            let mut updated_tab = existing_tab;
                            updated_tab.title = new_name;

                            if let Err(e) = app_db.save_query_tab(&updated_tab).await {
                                log::error!("Failed to update tab name in database: {}", e);
                            }
                        } else {
                            log::error!("Failed to load existing tab data for tab ID: {}", db_id);
                        }
                    } else {
                        log::error!("App database not initialized for tab rename");
                    }
                })
                .detach();
            }

            log::info!(
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

        let settings = crate::settings::load_settings().unwrap_or_default();
        let json_content = serde_json::to_string_pretty(&settings).unwrap();

        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("json".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 2,
                    hard_tabs: false,
                })
                .soft_wrap(true)
                .placeholder("Settings JSON will appear here...")
        });

        // Text will be set in render method where we have window access

        let settings_tab = SettingsTab {
            id: tab_id,
            title: "Settings".to_string(),
            editor,
            original_settings: settings,
            is_valid: true,
            validation_error: None,
            pending_text: Some(json_content),
        };

        self.tabs.push(TabType::Settings(settings_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        cx.notify();
    }

    fn set_active_tab(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            // Tab switching no longer saves automatically - tabs are only saved on query execution
            self.active_tab_ix = ix;
            cx.notify();
        }
    }

    /// Extract the query to execute based on selection or cursor position
    fn extract_current_query(text: &str, cursor_pos: usize, _has_selection: bool) -> String {
        let chars: Vec<char> = text.chars().collect();

        // If there's a selection, we can't easily get it due to API limitations
        // For now, we'll just use cursor position

        // Find start: read backward until we hit an empty line (double newline) or semicolon or start of text
        let mut start = cursor_pos.min(chars.len());
        let mut found_content = false;
        let mut prev_was_newline = false;

        for i in (0..start).rev() {
            if let Some(&ch) = chars.get(i) {
                if ch == ';' {
                    // Found a semicolon, start after it
                    start = (i + 1).min(chars.len());
                    break;
                } else if ch == '\n' {
                    if prev_was_newline && found_content {
                        // Found empty line (double newline) after some content
                        start = (i + 1).min(chars.len());
                        break;
                    }
                    prev_was_newline = true;
                } else if !ch.is_whitespace() {
                    found_content = true;
                    prev_was_newline = false;
                }
            }
            if i == 0 {
                start = 0;
                break;
            }
        }

        // Find end: read forward until we hit a semicolon or empty line or end of text
        let mut end = cursor_pos;
        prev_was_newline = false;

        for i in cursor_pos..chars.len() {
            if let Some(&ch) = chars.get(i) {
                if ch == ';' {
                    end = i + 1;
                    break;
                } else if ch == '\n' {
                    if prev_was_newline {
                        // Found empty line (double newline)
                        end = i;
                        break;
                    }
                    prev_was_newline = true;
                } else if !ch.is_whitespace() {
                    prev_was_newline = false;
                }
            }
            if i == chars.len() - 1 {
                end = chars.len();
                break;
            }
        }

        // Extract the query and trim whitespace
        chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// Run query through the async pipeline
    pub fn run_query(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab_ix) {
            match tab {
                TabType::Query(query_tab) => {
                    // Save the current tab before executing the query
                    // Extract the tab data we need before starting async operations
                    let tab_index = self.active_tab_ix;
                    let content = query_tab.editor.read(cx).text().to_string();

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
                    };

                    // Trigger the save operation in background
                    let db_service = DbService::global(cx).clone();
                    let app_db = db_service.app_db_handle();
                    let _title = query_tab.title.clone();
                    let connection_id = query_tab.connection_id;

                    cx.spawn(async move |_entity_handle, _cx| {
                        if let Some(app_db) = app_db.read().await.as_ref() {
                            // Use the existing connection_id from the query_tab

                            // Create the final tab data with connection_id
                            let mut final_tab_data = tab_data;
                            final_tab_data.connection_id = Some(connection_id);

                            // First save to get or create the database ID
                            let _final_db_id = match app_db.save_query_tab(&final_tab_data).await {
                                Ok(db_id) => {
                                    debug!("Tab saved successfully with db_id: {}", db_id);
                                    db_id
                                }
                                Err(e) => {
                                    error!("Failed to save tab: {}", e);
                                    return;
                                }
                            };
                        }
                    })
                    .detach();

                    // Use the connection_id to get the connection from db_service
                    let connection_id = query_tab.connection_id;

                    // Get text and cursor position from editor
                    let editor = query_tab.editor.read(cx);
                    let full_text = editor.text().to_string();
                    let cursor_pos = editor.cursor();

                    let query = Self::extract_current_query(
                        &full_text, cursor_pos,
                        false, // TODO: Detect actual selection state when API is available
                    );

                    if query.is_empty() {
                        println!("No query to execute");
                        return;
                    }

                    log::info!("Executing query via async pipeline: {}", query);

                    // Log the query to the SQL log
                    query_tab.sql_log.update(cx, |sql_log, cx| {
                        sql_log.append_text(
                            &blanco_ui::SqlLogMessage::SqlStatement(query.clone()),
                            cx,
                        );
                    });

                    // Execute query directly using cx.spawn instead of async pipeline
                    let query_clone = query.clone();
                    let results_panel_clone = query_tab.results_panel.clone();
                    let sql_log_clone = query_tab.sql_log.clone();
                    let db_service = DbService::global(cx).clone();
                    let _app_db_handle = db_service.app_db_handle();
                    let database_name = query_tab.database_name.clone();

                    cx.spawn(async move |editor_panel_entity, cx| {
                        let start_time = std::time::Instant::now();

                        // Execute query using db_service with connection_id
                        log::debug!(
                            "Query execution - using connection_id: '{}', database: '{}'",
                            connection_id,
                            database_name
                        );
                        match db_service
                            .get_or_create_connection_with_database(
                                connection_id,
                                Some(&database_name),
                            )
                            .await
                        {
                            Ok(connection) => {
                                log::debug!(
                                    "Connection retrieved successfully, type: {}",
                                    connection.get_connection_type()
                                );
                                match connection
                                    .execute_query(&query_clone, Some(&database_name))
                                    .await
                                {
                                    Ok(mut result) => {
                                        let duration_ms = start_time.elapsed().as_millis() as i64;

                                        log::info!(
                                            "Query executed successfully: {} rows in {}ms",
                                            result.row_count(),
                                            duration_ms
                                        );

                                        // Add execution metadata
                                        result.query_text = Some(query_clone.clone());
                                        result.execution_time_ms = Some(duration_ms);
                                        result.is_error = false;
                                        result.connection_id = Some(connection_id);

                                        // Extract table metadata from the query
                                        let table_name = connection
                                            .extract_table_name_from_query(&query_clone, false)
                                            .ok()
                                            .flatten();
                                        result.table_name = table_name.clone();

                                        // Extract primary key if we have a table name and results
                                        if let (Some(table_name), false) =
                                            (&table_name, result.rows.is_empty())
                                        {
                                            // Try to get primary key information for the table
                                            if let Ok(Some(pk_column)) = connection
                                                .get_primary_key_for_table(table_name)
                                                .await
                                            {
                                                result.primary_key_column = Some(pk_column);
                                                log::info!(
                                                    "Detected primary key '{}' for table '{}'",
                                                    result.primary_key_column.as_ref().unwrap(),
                                                    table_name
                                                );
                                            }
                                        }

                                        // Store rows_affected before moving result
                                        let rows_affected = result.rows_affected;

                                        // Update results panel
                                        let _ = results_panel_clone.update(cx, |panel, cx| {
                                            panel.set_query_result(result, Some(connection_id), cx);
                                        });

                                        // Log execution result to SQL log
                                        let _ = sql_log_clone.update(cx, |sql_log, cx| {
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

                                        // Emit success event
                                        editor_panel_entity
                                            .update(cx, |_, cx| {
                                                cx.emit(AppEvent::QueryExecutionCompleted {
                                                    connection_id: Some(connection_id),
                                                    success: true,
                                                    execution_time: start_time.elapsed(),
                                                    rows_affected: Some(rows_affected),
                                                    error_message: None,
                                                });
                                            })
                                            .ok();
                                    }
                                    Err(e) => {
                                        log::error!("Query execution failed: {}", e);

                                        // Log execution error to SQL log
                                        let _error_duration =
                                            start_time.elapsed().as_millis() as i64;
                                        let _ = sql_log_clone.update(cx, |sql_log, cx| {
                                            let log_message =
                                                format!("query execution failed: {}", e);
                                            sql_log.append_text(
                                                &blanco_ui::SqlLogMessage::Comment(log_message),
                                                cx,
                                            );
                                        });

                                        // Emit error event
                                        editor_panel_entity
                                            .update(cx, |_, cx| {
                                                cx.emit(AppEvent::QueryExecutionCompleted {
                                                    connection_id: Some(connection_id),
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
                                                    severity:
                                                        crate::app_events::ErrorSeverity::Error,
                                                });
                                            })
                                            .ok();
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to get connection: {}", e);
                                editor_panel_entity
                                    .update(cx, |_, cx| {
                                        cx.emit(AppEvent::ErrorOccurred {
                                            context: format!(
                                                "Connection setup for connection_id: {}",
                                                connection_id,
                                            ),
                                            error: e.to_string(),
                                            severity: crate::app_events::ErrorSeverity::Error,
                                        });
                                    })
                                    .ok();
                            }
                        }
                    })
                    .detach();

                    // Emit query execution started event
                    cx.emit(AppEvent::QueryExecutionStarted {
                        connection_id: Some(connection_id),
                        query: query.clone(),
                    });
                }
                TabType::Settings(_) => {
                    // Not a query tab, do nothing
                }
            }
        }
    }

    fn save_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Settings(settings_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            let json_text = settings_tab.editor.read(cx).text().to_string();

            match serde_json::from_str::<Settings>(&json_text) {
                Ok(settings) => match crate::settings::save_settings(&settings) {
                    Ok(()) => {
                        settings_tab.original_settings = settings.clone();
                        settings_tab.validation_error = None;
                        settings_tab.is_valid = true;
                        self.apply_settings(&settings, cx);
                        println!("Settings saved successfully!");
                    }
                    Err(e) => {
                        settings_tab.validation_error =
                            Some(format!("Failed to save settings: {}", e));
                        settings_tab.is_valid = false;
                    }
                },
                Err(e) => {
                    settings_tab.validation_error = Some(format!("Invalid JSON: {}", e));
                    settings_tab.is_valid = false;
                }
            }
            cx.notify();
        }
    }

    fn reset_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(TabType::Settings(settings_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            let default_settings = Settings::default();
            let json_content = serde_json::to_string_pretty(&default_settings).unwrap();

            settings_tab.pending_text = Some(json_content);
            settings_tab.validation_error = None;
            settings_tab.is_valid = true;
            cx.notify();
        }
    }

    fn apply_settings(&self, settings: &Settings, _cx: &mut Context<Self>) {
        // Apply settings to application
        // For now, we'll just log the settings
        // In a real implementation, this would update the theme, editor settings, etc.
        println!("Applied settings: {:?}", settings);
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
            _subscriptions: Vec::new(),
            pending_saved_tabs: None,
            run_query_keystroke: KeybindingKeystroke::from_keystroke(
                Keystroke::parse("shift-enter").unwrap(),
            ),
            editor_chat_resize_state,
            editor_results_resize_state,
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

                let params = TabCreationParams {
                    title: tab_title,
                    content: Some(tab_content),
                    db_id: tab_db_id,
                    connection_id: _connection_id,
                    connection_type: tab_connection_type
                        .as_deref()
                        .unwrap_or("Unknown")
                        .to_string(),
                    connection_name: tab_data.connection_name.clone(),
                    database_name: tab_data
                        .database_name
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                    schema_name: None, // TODO: Load from database when schema is added
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
            let mut editor = InputState::new(window, cx)
                .code_editor("sql".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(false)
                .placeholder("Enter your SQL query here...");

            // Set up completion provider using connection_id, database_name, and DbService
            let db_service = DbService::global(cx).clone();
            let completion_provider = SqlCompletionProvider::new_with_database(
                params.connection_id,
                params.database_name.clone(),
                db_service,
            );
            let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                Rc::new(completion_provider);
            editor.lsp.completion_provider = Some(completion_provider);

            editor
        });

        // Set content if provided
        if let Some(content) = params.content {
            editor.update(cx, |state, cx| {
                state.replace(&content, window, cx);
            });
        }

        // Create query tab with the connection string
        let query_tab = QueryTab {
            id: tab_id,
            title: params.title.clone(),
            connection_id: params.connection_id,
            connection_name: params.connection_name.clone(),
            database_name: params.database_name.clone(),
            schema_name: params.schema_name.clone(),
            editor: editor.clone(),
            db_id: params.db_id,
            results_panel: cx.new(|cx| ResultsPanel::new(window, cx)),
            sql_log: cx.new(|cx| SqlLog::new(1000, cx.theme().highlight_theme.clone())),
            // Chat functionality
            chat_enabled: false,
            chat_panel: None,
        };

        self.tabs.push(TabType::Query(query_tab));
        cx.notify();
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

    /// Update chat context for the active tab
    pub fn update_chat_context_for_active_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix)
            && query_tab.chat_enabled
            && let Some(ref chat_panel) = query_tab.chat_panel
        {
            let sql_context = query_tab.get_sql_context(cx);
            chat_panel.update(cx, |panel, cx| {
                panel.update_sql_context(sql_context, cx);
            });
        }
    }

    /// Toggle chat for the active tab
    pub fn toggle_chat_for_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.chat_enabled = !query_tab.chat_enabled;

            if query_tab.chat_enabled && query_tab.chat_panel.is_none() {
                // Create chat provider info first
                match create_chat_provider_info(query_tab.connection_id, cx) {
                    Ok(provider_info) => {
                        // Create chat panel with the provider info
                        let chat_panel = cx.new(|cx| {
                            ChatPanel::new(
                                query_tab.id,
                                provider_info.provider,
                                provider_info.provider_name.clone(),
                                provider_info.model_name.clone(),
                                window,
                                cx,
                            )
                        });
                        query_tab.chat_panel = Some(chat_panel);

                        // Emit chat session started event
                        cx.emit(crate::app_events::AppEvent::ChatSessionStarted {
                            tab_id: query_tab.id,
                            provider: provider_info.provider_name,
                            model: provider_info.model_name,
                        });
                    }
                    Err(e) => {
                        log::error!(
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

            // Update chat context when toggled on
            if query_tab.chat_enabled {
                // Use the EditorPanel's method to update chat context
                self.update_chat_context_for_active_tab(cx);
            }

            cx.notify();
        }
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<EditorPanelEvent> for EditorPanel {}
impl EventEmitter<AppEvent> for EditorPanel {}

impl Render for EditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Handle pending text for settings tabs first
        if let Some(TabType::Settings(settings_tab)) = self.tabs.get_mut(self.active_tab_ix)
            && let Some(pending_text) = settings_tab.pending_text.take()
        {
            settings_tab.editor.update(cx, |state, cx| {
                state.replace(&pending_text, window, cx);
            });
        }

        let current_tab = self.tabs.get(self.active_tab_ix);

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(
                // Tab bar
                TabBar::new("editor-tabs")
                    .w_full()
                    .selected_index(self.active_tab_ix)
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        this.set_active_tab(*ix, window, cx);
                    }))
                    .prefix(
                        Button::new("toggle-sidebar")
                            .ghost()
                            .small()
                            .icon(if self.sidebar_collapsed {
                                Icon::new(IconName::PanelLeftOpen).size_4()
                            } else {
                                Icon::new(IconName::PanelLeftClose).size_4()
                            })
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                log::info!("🖱️ Sidebar collapse button clicked!");
                                // Emit the toggle sidebar event
                                log::info!("🖱️ Emitting ToggleSidebar event...");
                                cx.emit(AppEvent::ToggleSidebar);
                                log::info!("🖱️ ToggleSidebar event emitted");
                            }))
                    )
                    .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                        match tab {
                            TabType::Query(query_tab) => {
                                let show_close_button = self.tabs.len() > 1;
                                let tab_index = ix;

                                // Clone the tab title to avoid lifetime issues
                                let tab_title = query_tab.title.clone();
                                Tab::new()
                                    .label(&tab_title)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, event: &gpui::MouseDownEvent, window, cx| {
                                            // Check for double click (click_count == 2)
                                            if event.click_count == 2 {
                                                // Open rename modal on double click
                                                let form = RenameTabForm::new(tab_index, tab_title.clone(), window, cx);
                                                let form_for_modal = form.clone();

                                                window.open_dialog(cx, move |modal, _window, _cx| {
                                                    let form_clone = form_for_modal.clone();
                                                    let tab_index_clone = tab_index;
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

                                                                log::info!("Modal OK button clicked - tab_index={}, new_name='{}'", tab_index_clone, new_name);

                                                                // Use the app's global action system instead of local context
                                                                // Create a new RenameTab action and dispatch it through the app
                                                                window.dispatch_action(Box::new(RenameTab {
                                                                    tab_index: tab_index_clone,
                                                                    new_name,
                                                                }), cx);
                                                                true
                                                            }
                                                        })
                                                });
                                            } else {
                                                // Single click - activate the tab by setting active tab index
                                                cx.emit(AppEvent::TabChanged { tab_id: tab_index });
                                            }
                                        })
                                    )
                                    .suffix(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(
                                                        query_tab.connection_name.clone()
                                                            .unwrap_or_else(|| "No Connection".to_string())
                                                    )
                                            )
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
                            TabType::Settings(settings_tab) => {
                                let mut label = settings_tab.title.clone();
                                if !settings_tab.is_valid {
                                    label.push_str(" ⚠️");
                                }
                                let show_close_button = self.tabs.len() > 1;
                                let tab_index = ix;

                                Tab::new()
                                    .label(label)
                                    .suffix(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .pr_2()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(settings_tab.title.clone())
                                            )
                                            .when(show_close_button, |this| {
                                                this.child(
                                                    Button::new(("close-settings-tab", ix))
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
                        }
                    }))
                    .suffix(
                        h_flex()
                            .gap_1()
                            .px_2()
                            .child(
                                Button::new("settings-tab")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Settings)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_settings_tab(window, cx);
                                    })),
                            ),
                    ),
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
                                                                .border_t_1()
                                                                .border_color(cx.theme().border)
                                                                .on_key_down(cx.listener(|this, evt: &gpui::KeyDownEvent, window, cx| {
                                                                    if evt.keystroke.should_match(&this.run_query_keystroke) {
                                                                        log::debug!("Matches keystroke {:?}", evt.keystroke);
                                                                        this.run_query(window, cx);
                                                                    }
                                                                }))
                                                                .relative() // Make container relative for absolute popup positioning
                                                                .child(
                                                                    Input::new(&query_tab.editor)
                                                                        .bordered(false)
                                                                        .p_0()
                                                                        .h_full()
                                                                        .rounded_none()
                                                                        .font_family("Fira Code")
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
                                                                .bg(cx.theme().muted.opacity(0.5))
                                                                .justify_end()
                                                                // Run button (always visible)
                                                                .child(
                                                                        Button::new("run-query")
                                                                            .outline()
                                                                            .small()
                                                                            .label("Run Current")
                                                                            .children(vec![Kbd::new(self.run_query_keystroke.inner().clone()).into_any_element()])
                                                                            .on_click(cx.listener(|panel, _, window, cx| panel.run_query(window, cx))),
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
                                                                        .child(
                                                                            div()
                                                                                .child(query_tab.sql_log.clone())
                                                                                .scrollable(Axis::Vertical)
                                                                        )
                                                                )
                                                        )
                                                        // Row operation buttons
                                                        .child(
                                                            h_flex()
                                                                .p_2()
                                                                .gap_2()
                                                                .border_t_1()
                                                                .border_color(cx.theme().border)
                                                                .flex_wrap()
                                                                .bg(cx.theme().muted.opacity(0.5))
                                                                .child(
                                                                    Button::new("add-row")
                                                                        .outline()
                                                                        .small()
                                                                        .icon(IconName::Plus)
                                                                        .label("Add Row")
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
                                                                        .label("Duplicate Row")
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
                                                                        .label("Commit Changes")
                                                                        .children(vec![Kbd::new(Keystroke::parse("cmd-shift-c").unwrap()).into_any_element()])
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
                                                                        .children(vec![Kbd::new(Keystroke::parse("cmd-shift-r").unwrap()).into_any_element()])
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
                                                                            // Toggle chat for the current query tab
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
                                            resizable_panel().child(
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
                        // Settings tab: Just editor + buttons
                        this.child(
                            v_flex()
                                .flex_1()
                                .h_full()
                                .overflow_hidden()
                                // Editor
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .border_t_1()
                                        .border_color(cx.theme().border)
                                        .relative()
                                        .child(
                                            Input::new(&settings_tab.editor)
                                                .bordered(false)
                                                .p_0()
                                                .h_full()
                                                .font_family("Fira Code")
                                                .text_size(px(14.))
                                                .focus_bordered(false),
                                        )
                                        // Show validation error
                                        .when_some(settings_tab.validation_error.as_ref(), |this, error| {
                                            this.child(
                                                div()
                                                    .p_2()
                                                    .bg(cx.theme().red.opacity(0.1))
                                                    .border_1()
                                                    .border_color(cx.theme().red)
                                                    .text_color(cx.theme().red)
                                                    .child(format!("❌ JSON Error: {}", error))
                                            )
                                        })
                                )
                                // Button bar
                                .child(
                                    h_flex()
                                        .p_3()
                                        .gap_2()
                                        .border_t_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().muted.opacity(0.5))
                                        .child(
                                            Button::new("reset-settings")
                                                .outline()
                                                .icon(IconName::Asterisk)
                                                .label("Reset to Defaults")
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.reset_settings(cx);
                                                })),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            Button::new("save-settings")
                                                .outline()
                                                .icon(IconName::Save)
                                                .label("Save Settings")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.save_settings(window, cx);
                                                })),
                                        )
                                )
                        )
                    }
                }
            }))
    }
}

/// Create a chat panel with a real provider based on current settings
struct ChatProviderInfo {
    provider: Arc<dyn ChatProvider<Error = ProviderError>>,
    provider_name: String,
    model_name: String,
}

fn create_chat_provider_info(connection_id: i64, cx: &mut App) -> anyhow::Result<ChatProviderInfo> {
    // Load current settings
    let settings =
        load_settings().map_err(|e| anyhow::anyhow!("Failed to load settings: {}", e))?;

    // Validate settings
    let validation_errors = ChatProviderResolver::validate_settings(&settings);
    if !validation_errors.is_empty() {
        return Err(anyhow::anyhow!(
            "Invalid chat settings: {}",
            validation_errors.join(", ")
        ));
    }

    // Create HTTP client using reqwest_client from zed
    let http_client = Arc::new(reqwest_client::ReqwestClient::new());

    // Get db_service
    let db_service = DbService::global(cx).clone();

    // Create chat provider
    let mut resolver = ChatProviderResolver::new(http_client.clone(), db_service);
    resolver.set_connection_id(connection_id);
    let provider_info = resolver.get_provider(&settings)?;

    Ok(ChatProviderInfo {
        provider: provider_info.provider,
        provider_name: provider_info.provider_name,
        model_name: provider_info.model_name,
    })
}
