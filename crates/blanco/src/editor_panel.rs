use gpui::{
    div, prelude::FluentBuilder, px, App, AppContext, Axis, ClickEvent, Context, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, Keystroke,
    ParentElement, Pixels, Point, Render, Styled, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    highlighter::Diagnostic,
    input::{InputState, TabSize, TextInput},
    tab::{Tab, TabBar},
    v_flex, ActiveTheme, ContextModal as _, IconName, Kbd, Sizable, StyledExt,
};
use log::{debug, error, info};
use std::rc::Rc;

use crate::agent::{ChatPanel, SqlContext};
use crate::app_database::QueryTabData;
use crate::app_events::AppEvent;
use crate::chat_provider_resolver::ChatProviderResolver;
use crate::db_service::DbService;
use crate::query_file::QueryFileManager;
use crate::results_panel::ResultsPanel;
use crate::settings::{Settings, load_settings};
use crate::sql_completion_provider::SqlCompletionProvider;
use blanco_ui::SqlLog;
use gpui_component::Icon;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum SplitType {
    EditorTable,
    TableLog,
}

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
    pub connection_string: String, // Unified connection string
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>, // Database ID for persistence
    pub results_panel: Entity<crate::results_panel::ResultsPanel>, // Each tab has its own results
    pub sql_log: Entity<SqlLog>, // SQL log for this tab
    #[allow(dead_code)]
    pub cached_diagnostics: Arc<Mutex<Vec<Diagnostic>>>, // Store diagnostics for this tab
    pub file_uri: Option<String>, // File URI for integration
    pub current_completions: Option<crate::sql_completion::CompletionResult>,
    pub selected_completion_index: usize,
    // Chat functionality
    pub chat_enabled: bool,
    pub chat_panel: Option<Entity<ChatPanel>>,
}

impl QueryTab {
    /// Get connection instance from unified connection manager
    #[allow(dead_code)]
    pub async fn get_connection(
        &self,
        cx: &gpui::App,
    ) -> Option<std::sync::Arc<dyn blanco_core::Connection>> {
        let db_service = crate::db_service::DbService::global(cx);
        let unified_manager = db_service.unified_manager().await;
        let unified_manager_guard = unified_manager.read().await;
        unified_manager_guard
            .get_connection(&self.connection_string)
            .await
    }

    /// Get connection metadata without accessing fields directly
    #[allow(dead_code)]
    pub async fn get_ui_metadata(
        &self,
        cx: &gpui::App,
    ) -> Option<blanco_core::ConnectionUIMetadata> {
        self.get_connection(cx)
            .await
            .map(|conn| conn.get_ui_metadata())
    }

    /// Get display name from connection
    #[allow(dead_code)]
    pub async fn get_display_name(&self, cx: &gpui::App) -> String {
        self.get_ui_metadata(cx)
            .await
            .map(|metadata| metadata.display_name)
            .unwrap_or_else(|| "Unknown Connection".to_string())
    }

    /// Get SQL context for the chat session
    pub fn get_sql_context(&self, cx: &mut gpui::App) -> SqlContext {
        let current_query = self.editor.read(cx).text().to_string();
        let connection_string = if !self.connection_string.is_empty() {
            Some(self.connection_string.clone())
        } else {
            None
        };

        let database_type = if self.connection_string.starts_with("sqlite:") {
            Some("SQLite".to_string())
        } else if self.connection_string.starts_with("postgresql:")
            || self.connection_string.starts_with("postgres:")
        {
            Some("PostgreSQL".to_string())
        } else {
            None
        };

        let mut context = SqlContext::with_query(current_query);
        context.connection_string = connection_string;
        context.database_type = database_type;

        // TODO: Add recent results when we implement a public method in ResultsPanel
        // For now, we'll leave recent_results as None

        context
    }

    /// Update chat context with current query state
    #[allow(dead_code)]
    pub fn update_chat_context(&self, cx: &mut Context<Self>) {
        if let Some(ref chat_panel) = self.chat_panel {
            let sql_context = self.get_sql_context(cx);
            chat_panel.update(cx, |panel, cx| {
                panel.update_sql_context(sql_context, cx);
            });
        }
    }

    /// Update chat context when query is executed
    #[allow(dead_code)]
    pub fn update_chat_context_on_query_execution(&self, cx: &mut Context<Self>) {
        if self.chat_enabled {
            self.update_chat_context(cx);
        }
    }

    /// Get icon name from connection
    #[allow(dead_code)]
    pub async fn get_icon_name(&self, cx: &gpui::App) -> blanco_ui::IconName {
        self.get_ui_metadata(cx)
            .await
            .map(|metadata| match metadata.icon_name {
                blanco_core::IconName::Sqlite => blanco_ui::IconName::Sqlite,
                blanco_core::IconName::Postgres => blanco_ui::IconName::Postgresql,
                blanco_core::IconName::Database => blanco_ui::IconName::SquareTerminal,
                blanco_core::IconName::Table => blanco_ui::IconName::SquareTerminal,
                blanco_core::IconName::Column => blanco_ui::IconName::SquareTerminal,
                _ => blanco_ui::IconName::SquareTerminal,
            })
            .unwrap_or(blanco_ui::IconName::SquareTerminal)
    }

    /// Get file-safe name from connection
    #[allow(dead_code)]
    pub async fn get_file_safe_name(&self, cx: &gpui::App) -> String {
        self.get_ui_metadata(cx)
            .await
            .map(|metadata| metadata.display_name.replace(['/', '\\'], "-"))
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Check if connection supports schemas
    #[allow(dead_code)]
    pub async fn supports_schemas(&self, cx: &gpui::App) -> bool {
        if let Some(conn) = self.get_connection(cx).await {
            conn.supports_schemas()
        } else {
            false
        }
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
    query_file_manager: Arc<QueryFileManager>,
    // Temporary storage for saved tabs that will be restored after connections are loaded
    pending_saved_tabs: Option<Vec<crate::app_database::QueryTabData>>,
    // Split pane state
    #[allow(dead_code)]
    editor_table_split: f32, // Position between editor and table (0.0-1.0)
    #[allow(dead_code)]
    table_log_split: f32, // Position between table and log (0.0-1.0)
    #[allow(dead_code)]
    dragging_split: Option<SplitType>, // Which handle is being dragged
    #[allow(dead_code)]
    drag_start_position: Option<Point<f32>>, // Start position of drag
}

/// Parameters for creating a new tab with connection
#[derive(Clone)]
struct TabCreationParams {
    title: String,
    content: String,
    db_id: Option<i64>,
    connection_string: String,
    #[allow(dead_code)]
    connection_type: String,
}

impl EditorPanel {
    #[allow(dead_code)]
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_sidebar_state(window, cx, false)
    }

    /// Create a new editor panel with optional sidebar state
    #[allow(dead_code)]
    pub fn new_with_sidebar_state(
        _window: &mut Window,
        cx: &mut Context<Self>,
        sidebar_collapsed: bool,
    ) -> Self {
        // Initialize QueryFileManager
        let query_file_manager = Arc::new(
            QueryFileManager::new()
                .map_err(|e| {
                    error!("Failed to initialize QueryFileManager: {}", e);
                    e
                })
                .expect("Failed to create QueryFileManager"),
        );

        Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![], // Start with no tabs - tabs are created on demand
            active_tab_ix: 0,
            next_tab_id: 1,
            sidebar_collapsed,
            _subscriptions: Vec::new(),
            query_file_manager,
            pending_saved_tabs: None,
            // Initialize split pane state with reasonable defaults
            // 40% editor, 40% table, 20% log
            editor_table_split: 0.4,
            table_log_split: 0.4,
            dragging_split: None,
            drag_start_position: None,
        }
    }

    pub fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        cx.notify();
    }

    /// Add a new query tab using the unified connection interface
    pub fn add_new_tab_with_unified_connection(
        &mut self,
        window: &mut Window,
        display_name: String,
        connection_string: String,
        schema_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        log::debug!(
            "Creating new tab with connection string: '{}', display_name: '{}'",
            connection_string,
            display_name
        );
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
                .soft_wrap(true)
                .placeholder("-- Enter your SQL query here...");

            // Set up completion provider using connection string and DbService
            let db_service = DbService::global(cx).clone();
            let completion_provider =
                SqlCompletionProvider::new(connection_string.clone(), db_service);
            let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                Rc::new(completion_provider);
            editor.lsp.completion_provider = Some(completion_provider);

            editor
        });

        // Create unified query tab with connection string
        log::info!("🚀 Creating new query tab with ID {}", tab_id);
        let query_tab = QueryTab {
            id: tab_id,
            title: if let Some(ref schema) = schema_name {
                format!("{} - {}", display_name, schema)
            } else {
                display_name.clone()
            },
            connection_string: connection_string.clone(),
            editor,
            db_id: None,
            results_panel: cx.new(|cx| {
                ResultsPanel::with_connection_string(Some(connection_string.clone()), window, cx)
            }),
            sql_log: cx.new(|cx| SqlLog::new(1000, cx.theme().highlight_theme.clone())), // Maximum 1000 lines in the log
            cached_diagnostics: Arc::new(Mutex::new(Vec::new())),
            file_uri: None, // Will be set when file is created
            current_completions: None,
            selected_completion_index: 0,
            // Chat functionality
            chat_enabled: false,
            chat_panel: None,
        };

        self.tabs.push(TabType::Query(query_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        cx.notify();

        // Note: Completion provider will be set up when connection is available
        // The connection-based completion provider requires an actual database connection
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

    /// Run query using the unified connection interface
    fn run_query_unified(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.run_query_unified_internal(_window, cx)
    }

    /// Internal method for unified query execution (without ClickEvent requirement)
    fn run_query_unified_internal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab_ix) {
            match tab {
                TabType::Query(query_tab) => {
                    // Save the current tab before executing the query
                    // Extract the tab data we need before starting async operations
                    let tab_index = self.active_tab_ix;
                    let content = query_tab.editor.read(cx).text().to_string();
                    let connection_type = if query_tab.connection_string.starts_with("sqlite:") {
                        Some("SQLite".to_string())
                    } else if query_tab.connection_string.starts_with("postgresql:")
                        || query_tab.connection_string.starts_with("postgres:")
                    {
                        Some("PostgreSQL".to_string())
                    } else {
                        None
                    };
                    let pg_connection_key =
                        if connection_type.as_ref().is_some_and(|t| t == "PostgreSQL") {
                            Some(query_tab.connection_string.clone())
                        } else {
                            None
                        };
                    let tab_data = QueryTabData {
                        id: query_tab.db_id,
                        title: query_tab.title.clone(),
                        content: content.clone(),
                        position: tab_index as i32,
                        connection_id: None, // Will be set in async task
                        connection_type,
                        pg_connection_key,
                        file_uri: None, // Will be updated after file creation
                    };

                    // Trigger the save operation in background
                    let db_service = DbService::global(cx).clone();
                    let app_db = db_service.app_db_handle();
                    let connection_string = query_tab.connection_string.clone();
                    let _title = query_tab.title.clone();
                    let query_file_manager = self.query_file_manager.clone();
                    let connection_name = query_tab.title.clone();

                    cx.spawn(async move |_entity_handle, _cx| {
                        if let Some(app_db) = app_db.read().await.as_ref() {
                            // Find or create the connection and get its ID
                            let connection_id = if !connection_string.is_empty()
                                && connection_string != "sqlite::memory:"
                            {
                                match app_db.find_or_create_connection(&connection_string).await {
                                    Ok(id) => Some(id),
                                    Err(e) => {
                                        error!("Failed to find or create connection: {}", e);
                                        return;
                                    }
                                }
                            } else {
                                None
                            };

                            // Create the final tab data with connection_id
                            let mut final_tab_data = tab_data;
                            final_tab_data.connection_id = connection_id;

                            // First save to get or create the database ID
                            let final_db_id = match app_db.save_query_tab(&final_tab_data).await {
                                Ok(db_id) => {
                                    debug!("Tab saved successfully with db_id: {}", db_id);
                                    db_id
                                }
                                Err(e) => {
                                    error!("Failed to save tab: {}", e);
                                    return;
                                }
                            };

                            // Try to migrate the query file from legacy location if it exists
                            if let Err(e) = query_file_manager
                                .migrate_query_file(final_db_id, &connection_name)
                                .await
                            {
                                debug!("Migration not needed or failed for tab: {}", e);
                            }

                            // Create/update the query file on disk
                            let file_uri = match query_file_manager
                                .create_query_file(final_db_id, &connection_name, &content)
                                .await
                            {
                                Ok(_) => {
                                    let uri = query_file_manager
                                        .query_file_uri(final_db_id, &connection_name);
                                    debug!("Created query file for tab with URI: {}", uri);
                                    Some(uri)
                                }
                                Err(e) => {
                                    error!("Failed to create query file: {}", e);
                                    None
                                }
                            };

                            // Update the database record with the file URI
                            if let Some(ref file_uri) = file_uri {
                                let updated_tab_data = QueryTabData {
                                    id: Some(final_db_id),
                                    title: final_tab_data.title.clone(),
                                    content: final_tab_data.content.clone(),
                                    position: final_tab_data.position,
                                    connection_id: final_tab_data.connection_id,
                                    connection_type: final_tab_data.connection_type.clone(),
                                    pg_connection_key: final_tab_data.pg_connection_key.clone(),
                                    file_uri: Some(file_uri.clone()),
                                };

                                if let Err(e) = app_db.save_query_tab(&updated_tab_data).await {
                                    error!("Failed to update file URI: {}", e);
                                } else {
                                    debug!("Updated file URI: {}", file_uri);
                                }
                            }
                        }
                    })
                    .detach();

                    // Use the unified connection string
                    let connection_string = &query_tab.connection_string;

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
                    let connection_string_clone = connection_string.clone();
                    let query_clone = query.clone();
                    let results_panel_clone = query_tab.results_panel.clone();
                    let sql_log_clone = query_tab.sql_log.clone();
                    let db_service = DbService::global(cx).clone();
                    let _app_db_handle = db_service.app_db_handle();

                    cx.spawn(async move |editor_panel_entity, cx| {
                        let start_time = std::time::Instant::now();

                        // Execute query using unified connection manager
                        log::debug!(
                            "Query execution - using connection string: '{}'",
                            connection_string_clone
                        );
                        let unified_manager = db_service.unified_manager().await;
                        let manager_guard = unified_manager.read().await;
                        match manager_guard
                            .get_or_create_connection(&connection_string_clone)
                            .await
                        {
                            Ok(connection) => {
                                log::debug!(
                                    "Connection retrieved successfully, type: {}",
                                    connection.get_connection_type()
                                );
                                match connection.execute_query(&query_clone).await {
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
                                        result.connection_string =
                                            Some(connection_string_clone.clone());

                                        // Extract table metadata from the query
                                        let table_name = connection
                                            .extract_table_name_from_query(&query_clone)
                                            .ok()
                                            .flatten();
                                        result.table_name = table_name.clone();

                                        // Extract primary key if we have a table name and results
                                        if let (Some(ref table_name), false) =
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
                                            panel.set_query_result(
                                                result,
                                                Some(connection_string_clone.clone()),
                                                cx,
                                            );
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
                                                    connection_id: connection_string_clone.clone(),
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
                                                    connection_id: connection_string_clone.clone(),
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
                                                        "Query execution on {}",
                                                        connection_string_clone
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
                                                "Connection setup for {}",
                                                connection_string_clone
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
                        connection_id: connection_string.clone(),
                        query: query.clone(),
                    });
                }
                TabType::Settings(_) => {
                    // Not a query tab, do nothing
                }
            }
        }
    }

    #[allow(dead_code)]
    fn run_query(&mut self, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        // For backward compatibility, call the unified method
        self.run_query_unified(event, window, cx)
    }

    /// Run query without requiring a ClickEvent (for action handlers)
    fn run_query_no_event(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Call unified method without ClickEvent
        self.run_query_unified_internal(window, cx)
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

    #[allow(dead_code)]
    fn current_editor(&self) -> Option<&Entity<InputState>> {
        self.tabs.get(self.active_tab_ix).map(|tab| match tab {
            TabType::Query(query_tab) => &query_tab.editor,
            TabType::Settings(settings_tab) => &settings_tab.editor,
        })
    }

    #[allow(dead_code)]
    fn is_settings_tab(&self, tab: &TabType) -> bool {
        matches!(tab, TabType::Settings(_))
    }

    /// Save all query tabs to the app database
    pub fn save_tabs(&mut self, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();

        // Collect tab data with indices
        let mut tabs_data = Vec::new();
        for (pos, tab) in self.tabs.iter().enumerate() {
            if let TabType::Query(query_tab) = tab {
                let content = query_tab.editor.read(cx).text().to_string();
                let connection_type = if query_tab.connection_string.starts_with("sqlite:") {
                    Some("SQLite".to_string())
                } else if query_tab.connection_string.starts_with("postgresql:")
                    || query_tab.connection_string.starts_with("postgres:")
                {
                    Some("PostgreSQL".to_string())
                } else {
                    None
                };
                let pg_connection_key =
                    if connection_type.as_ref().is_some_and(|t| t == "PostgreSQL") {
                        Some(query_tab.connection_string.clone())
                    } else {
                        None
                    };
                tabs_data.push((
                    pos,
                    query_tab.db_id,
                    query_tab.title.clone(),
                    content,
                    pos as i32,
                    None, // connection_id removed in unified structure
                    connection_type,
                    pg_connection_key,
                    query_tab.title.clone(),
                ));
            }
        }

        info!("Saving {} query tabs on app quit", tabs_data.len());

        // Save tabs in background
        let query_file_manager = self.query_file_manager.clone();
        cx.spawn(async move |editor_panel_handle, cx| {
            let mut saved_ids = Vec::new();
            if let Some(app_db) = app_db.read().await.as_ref() {
                for (
                    tab_index,
                    db_id,
                    title,
                    content,
                    position,
                    connection_id,
                    connection_type,
                    pg_connection_key,
                    connection_name,
                ) in tabs_data
                {
                    // First save to get or create the database ID
                    let temp_tab_data = QueryTabData {
                        id: db_id,
                        title: title.clone(),
                        content: content.clone(),
                        position,
                        connection_id,
                        connection_type: connection_type.clone(),
                        pg_connection_key: pg_connection_key.clone(),
                        file_uri: None, // Will be updated after file creation
                    };

                    let final_db_id = match app_db.save_query_tab(&temp_tab_data).await {
                        Ok(saved_id) => {
                            debug!("Tab '{}' saved with db_id: {}", title, saved_id);
                            saved_id
                        }
                        Err(e) => {
                            error!("Failed to save tab '{}': {}", title, e);
                            continue;
                        }
                    };

                    // Try to migrate the query file from legacy location if it exists
                    // This ensures backward compatibility when switching to connection-specific directories
                    if let Err(e) = query_file_manager
                        .migrate_query_file(final_db_id, &connection_name)
                        .await
                    {
                        debug!("Migration not needed or failed for tab '{}': {}", title, e);
                    }

                    // Create/update the query file on disk
                    let file_uri = match query_file_manager
                        .create_query_file(final_db_id, &connection_name, &content)
                        .await
                    {
                        Ok(_) => {
                            let uri =
                                query_file_manager.query_file_uri(final_db_id, &connection_name);
                            debug!("Created query file for tab '{}' with URI: {}", title, uri);
                            Some(uri)
                        }
                        Err(e) => {
                            error!("Failed to create query file for tab '{}': {}", title, e);
                            None
                        }
                    };

                    // Update the database record with the file URI
                    if let Some(ref file_uri) = file_uri {
                        let updated_tab_data = QueryTabData {
                            id: Some(final_db_id),
                            title: title.clone(),
                            content: content.clone(),
                            position,
                            connection_id,
                            connection_type: connection_type.clone(),
                            pg_connection_key: pg_connection_key.clone(),
                            file_uri: Some(file_uri.clone()),
                        };

                        if let Err(e) = app_db.save_query_tab(&updated_tab_data).await {
                            error!("Failed to update file URI for tab '{}': {}", title, e);
                        } else {
                            debug!("Updated file URI for tab '{}': {}", title, file_uri);
                        }
                    }

                    saved_ids.push((tab_index, final_db_id));
                }
            } else {
                error!("App database not initialized");
            }

            if let Some(editor_panel) = editor_panel_handle.upgrade() {
                let _ = editor_panel.update(cx, |panel, _cx| {
                    for (tab_index, db_id) in saved_ids {
                        if let Some(TabType::Query(query_tab)) = panel.tabs.get_mut(tab_index) {
                            if query_tab.db_id.is_none() {
                                query_tab.db_id = Some(db_id);
                                // Set the file URI on the QueryTab
                                if let Some(file_uri) = panel
                                    .query_file_manager
                                    .query_file_uri(db_id, &query_tab.title)
                                    .into()
                                {
                                    let file_uri_debug = file_uri.clone();
                                    query_tab.file_uri = Some(file_uri);
                                    debug!(
                                        "Updated tab with db_id: {} and file_uri: {}",
                                        db_id, file_uri_debug
                                    );
                                } else {
                                    debug!("Updated tab with db_id: {}", db_id);
                                }
                            }
                        }
                    }
                });
            }
        })
        .detach();
    }

    /// Load saved query tabs from the app database
    /// This should be called from BlancoApp initialization
    pub fn new_with_saved_tabs(
        _window: &mut Window,
        cx: &mut Context<Self>,
        sidebar_collapsed: bool,
        saved_tabs: Vec<QueryTabData>,
    ) -> Self {
        info!("Loading {} saved tabs", saved_tabs.len());

        // Initialize QueryFileManager
        let query_file_manager = Arc::new(
            QueryFileManager::new()
                .map_err(|e| {
                    error!("Failed to initialize QueryFileManager: {}", e);
                    e
                })
                .expect("Failed to create QueryFileManager"),
        );

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![],
            active_tab_ix: 0,
            next_tab_id: 0,
            sidebar_collapsed,
            _subscriptions: Vec::new(),
            query_file_manager,
            pending_saved_tabs: None,
            // Initialize split pane state with reasonable defaults
            // 40% editor, 40% table, 20% log
            editor_table_split: 0.4,
            table_log_split: 0.4,
            dragging_split: None,
            drag_start_position: None,
        };

        if saved_tabs.is_empty() {
            debug!("No saved tabs found, creating default tab");
        } else {
            debug!(
                "Found {} saved tabs, storing for restoration after connections load",
                saved_tabs.len()
            );
            // Store saved tabs for later restoration after connections are loaded
            panel.pending_saved_tabs = Some(saved_tabs);
        }

        info!("Restored {} tabs total", panel.tabs.len());
        panel
    }

    /// Restore saved tabs after connections have been loaded (synchronous version)
    /// This function matches saved tabs with actual connections and only restores valid ones
    pub fn restore_saved_tabs_with_connections_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), anyhow::Error> {
        let saved_tabs = match self.pending_saved_tabs.take() {
            Some(tabs) => tabs,
            None => {
                debug!("No pending saved tabs to restore");
                return Ok(());
            }
        };

        if saved_tabs.is_empty() {
            debug!("No saved tabs to restore");
            return Ok(());
        }

        info!(
            "Attempting to restore {} saved tabs with connection matching",
            saved_tabs.len()
        );

        // Get all available connections from the database to match with saved tabs
        let db_service = DbService::global(cx).clone();
        let _app_db = db_service.app_db_handle();

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
                let tab_pg_connection_key = tab_data.pg_connection_key.clone();

                // We need to do this synchronously since we need window access
                // Use the pg_connection_key for PostgreSQL or construct the SQLite path
                let connection_string = if let Some(tab_connection_type) = &tab_connection_type {
                    if tab_connection_type == "PostgreSQL" {
                        if let Some(pg_key) = &tab_pg_connection_key {
                            pg_key.clone()
                        } else {
                            debug!(
                                "PostgreSQL tab missing pg_connection_key, skipping tab '{}'",
                                tab_title
                            );
                            continue;
                        }
                    } else if tab_connection_type == "SQLite" {
                        // For SQLite, use the standard path
                        "sqlite:/home/user/.config/blanco/test.db".to_string()
                    } else {
                        debug!(
                            "Unsupported connection type '{}', skipping tab '{}'",
                            tab_connection_type, tab_title
                        );
                        continue;
                    }
                } else {
                    debug!("Tab has no connection_type, skipping tab '{}'", tab_title);
                    continue;
                };

                debug!(
                    "Restoring tab '{}' with connection string: {}",
                    tab_title, connection_string
                );

                let params = TabCreationParams {
                    title: tab_title,
                    content: tab_content,
                    db_id: tab_db_id,
                    connection_string,
                    connection_type: tab_connection_type
                        .as_deref()
                        .unwrap_or("Unknown")
                        .to_string(),
                };
                self.create_and_add_tab_with_connection_string(window, params, cx);
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
        Ok(())
    }

    /// Helper to create a tab with a specific connection string
    fn create_and_add_tab_with_connection_string(
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

            // Set up completion provider using connection string and DbService
            let db_service = DbService::global(cx).clone();
            let completion_provider =
                SqlCompletionProvider::new(params.connection_string.clone(), db_service);
            let completion_provider: Rc<dyn gpui_component::input::CompletionProvider> =
                Rc::new(completion_provider);
            editor.lsp.completion_provider = Some(completion_provider);

            editor
        });

        // Set content if provided
        if !params.content.is_empty() {
            let content_owned = params.content.clone();
            editor.update(cx, |state, cx| {
                state.replace(&content_owned, window, cx);
            });
        }

        // Create query tab with the connection string
        let query_tab = QueryTab {
            id: tab_id,
            title: params.title.clone(),
            connection_string: params.connection_string,
            editor,
            db_id: params.db_id,
            results_panel: cx.new(|cx| ResultsPanel::new(window, cx)),
            sql_log: cx.new(|cx| SqlLog::new(1000, cx.theme().highlight_theme.clone())),
            cached_diagnostics: Arc::new(Mutex::new(Vec::new())),
            file_uri: None,
            current_completions: None,
            selected_completion_index: 0,
            // Chat functionality
            chat_enabled: false,
            chat_panel: None,
        };

        self.tabs.push(TabType::Query(query_tab));
        cx.notify();
    }

    /// Execute the current query in the active tab using unified connection system
    pub fn execute_current_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Use the unified query execution method without requiring ClickEvent
        self.run_query_no_event(window, cx);
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

    // Split pane resize handling methods
    #[allow(dead_code)]
    fn start_split_drag(
        &mut self,
        split_type: SplitType,
        position: Point<f32>,
        cx: &mut Context<Self>,
    ) {
        self.dragging_split = Some(split_type);
        self.drag_start_position = Some(position);
        cx.notify();
    }

    #[allow(dead_code)]
    fn handle_split_drag(
        &mut self,
        current_position: Point<f32>,
        _window_bounds: gpui::Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let (Some(split_type), Some(start_pos)) = (self.dragging_split, self.drag_start_position)
        {
            let delta_y = current_position.y - start_pos.y;
            let delta_ratio = delta_y / 1000.0; // Simple ratio for now

            match split_type {
                SplitType::EditorTable => {
                    let new_split = (self.editor_table_split + delta_ratio).clamp(0.2, 0.7); // Min 20%, Max 70%
                    self.table_log_split -= new_split - self.editor_table_split; // Adjust log split
                    self.editor_table_split = new_split;
                }
                SplitType::TableLog => {
                    let new_split = (self.table_log_split + delta_ratio).clamp(0.1, 0.5); // Min 10%, Max 50%
                    self.table_log_split = new_split;
                }
            }

            cx.notify();
        }
    }

    #[allow(dead_code)]
    fn end_split_drag(&mut self, cx: &mut Context<Self>) {
        self.dragging_split = None;
        self.drag_start_position = None;
        cx.notify();
    }

    // Helper method to create a resize handle (visual only for now)
    #[allow(dead_code)]
    fn resize_handle(&self, _split_type: SplitType, cx: &mut Context<Self>) -> impl IntoElement {
        div().h_1().w_full().bg(cx.theme().border)
    }

    /// Update chat context for the active tab
    pub fn update_chat_context_for_active_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            if query_tab.chat_enabled {
                if let Some(ref chat_panel) = query_tab.chat_panel {
                    let sql_context = query_tab.get_sql_context(cx);
                    chat_panel.update(cx, |panel, cx| {
                        panel.update_sql_context(sql_context, cx);
                    });
                }
            }
        }
    }

    /// Toggle chat for the active tab
    pub fn toggle_chat_for_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            query_tab.chat_enabled = !query_tab.chat_enabled;

            if query_tab.chat_enabled && query_tab.chat_panel.is_none() {
                // Create chat panel if it doesn't exist
                let chat_panel = cx.new(|cx| {
                    // Try to create a chat panel with real provider
                    match create_chat_panel_with_provider(query_tab.id, query_tab.connection_string.clone(), window, cx) {
                        Ok(panel) => panel,
                        Err(e) => {
                            log::error!("Failed to create chat provider: {}. Using mock provider.", e);
                            ChatPanel::new(query_tab.id, None, window, cx)
                        }
                    }
                });
                query_tab.chat_panel = Some(chat_panel);

                // Emit chat session started event
                cx.emit(crate::app_events::AppEvent::ChatSessionStarted {
                    tab_id: query_tab.id,
                    provider: "Mock".to_string(),
                    model: "mock-gpt-4".to_string(),
                });
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

    /// Update chat context for all tabs that have chat enabled
    #[allow(dead_code)]
    pub fn update_chat_context_for_all_tabs(&mut self, cx: &mut Context<Self>) {
        for tab in &mut self.tabs {
            if let TabType::Query(query_tab) = tab {
                if query_tab.chat_enabled {
                    if let Some(ref chat_panel) = query_tab.chat_panel {
                        let sql_context = query_tab.get_sql_context(cx);
                        chat_panel.update(cx, |panel, cx| {
                            panel.update_sql_context(sql_context, cx);
                        });
                    }
                }
            }
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
        if let Some(TabType::Settings(settings_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            if let Some(pending_text) = settings_tab.pending_text.take() {
                settings_tab.editor.update(cx, |state, cx| {
                    state.replace(&pending_text, window, cx);
                });
            }
        }

        let current_tab = self.tabs.get(self.active_tab_ix);

        v_flex()
            .size_full()
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

                                Tab::new(&query_tab.title)
                                    .suffix(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .pr_2()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(query_tab.title.clone())
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

                                Tab::new(label)
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
            .when_some(current_tab, |this, tab| {
                match tab {
                    TabType::Query(query_tab) => {
                        // Query tab: Layout with optional chat panel
                        this.child(
                            h_flex()
                                .flex_1()
                                .h_full()
                                // Left side: Always show the main content (Editor + Button bar + Results)
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .h_full()
                                        .min_w_0()
                                        // Editor
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .border_t_1()
                                                .border_color(cx.theme().border)
                                                .relative() // Make container relative for absolute popup positioning
                                                .child(
                                                    TextInput::new(&query_tab.editor)
                                                        .bordered(false)
                                                        .p_0()
                                                        .h_full()
                                                        .font_family("Fira Code")
                                                        .text_size(px(14.))
                                                        .focus_bordered(false)
                                                )
                                                // SQL Completion Popup
                                                .when_some(query_tab.current_completions.as_ref(), |this, completions| {
                                                    this.when(!completions.items.is_empty(), |this| {
                                                        this.child(
                                                            div()
                                                                .absolute()
                                                                .top(px(100.0)) // Position below the editor
                                                                .left(px(50.0))  // Offset from left edge
                                                                .border_1()
                                                                .border_color(cx.theme().border)
                                                                .bg(cx.theme().background)
                                                                .rounded(px(4.0))
                                                                .shadow_lg()
                                                                .min_w(px(200.0))
                                                                .max_w(px(400.0))
                                                                .max_h(px(200.0))
                                                                .child(
                                                                    v_flex()
                                                                        .children(
                                                                            completions.items.iter().enumerate().map(|(index, item)| {
                                                                                let is_selected = index == query_tab.selected_completion_index;
                                                                                div()
                                                                                    .id(("completion-item", index))
                                                                                    .w_full()
                                                                                    .px_3()
                                                                                    .py_2()
                                                                                    .when(is_selected, |div| {
                                                                                        div.bg(cx.theme().primary.opacity(0.2))
                                                                                    })
                                                                                    .hover(|div| {
                                                                                        div.bg(cx.theme().muted.opacity(0.5))
                                                                                    })
                                                                                    .cursor_pointer()
                                                                                    .child(
                                                                                h_flex()
                                                                                    .items_center()
                                                                                    .gap_2()
                                                                                    .child(
                                                                                        // Kind indicator
                                                                                        div()
                                                                                            .w(px(8.0))
                                                                                            .h(px(8.0))
                                                                                            .rounded(px(2.0))
                                                                                            .bg(match item.kind {
                                                                                                crate::sql_completion::CompletionItemKind::Table => cx.theme().blue,
                                                                                                crate::sql_completion::CompletionItemKind::Column => cx.theme().green,
                                                                                                crate::sql_completion::CompletionItemKind::Keyword => cx.theme().primary,
                                                                                                crate::sql_completion::CompletionItemKind::Schema => cx.theme().blue,
                                                                                                crate::sql_completion::CompletionItemKind::Function => cx.theme().primary,
                                                                                                crate::sql_completion::CompletionItemKind::Alias => cx.theme().muted,
                                                                                            })
                                                                                    )
                                                                                    .child(
                                                                                        div()
                                                                                            .text_sm()
                                                                                            .text_color(cx.theme().foreground)
                                                                                            .child(item.label.clone())
                                                                                    )
                                                                                    .when_some(item.detail.as_ref(), |this, detail| {
                                                                                        this.child(
                                                                                            div()
                                                                                                .text_xs()
                                                                                                .text_color(cx.theme().muted_foreground)
                                                                                                .child(detail.clone())
                                                                                        )
                                                                                    })
                                                                                )
                                                                            })
                                                                        )
                                                                )
                                                        )
                                                    })
                                                })
                                        )
                                        // Button bar (between editor and results)
                                        .child(
                                            h_flex()
                                                .px_3()
                                                .py_1()
                                                .gap_2()
                                                .border_t_1()
                                                .border_color(cx.theme().border)
                                                .bg(cx.theme().muted.opacity(0.5))
                                                .justify_end()
                                                // Run button (always visible)
                                                .child(
                                                        Button::new("run-query")
                                                            .outline()
                                                            .icon(IconName::Check)
                                                            .label("Run Current")
                                                            .children(vec![Kbd::new(Keystroke::parse("shift-enter").unwrap()).into_any_element()])
                                                            .on_click(cx.listener(Self::run_query_unified)),
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
                                                .p_3()
                                                .gap_2()
                                                .border_t_1()
                                                .border_color(cx.theme().border)
                                                .bg(cx.theme().muted.opacity(0.5))
                                                // Chat toggle button
                                                .child(
                                                    Button::new("toggle-chat")
                                                        .outline()
                                                        .icon(if query_tab.chat_enabled {
                                                            IconName::Bot
                                                        } else {
                                                            IconName::Plus
                                                        })
                                                        .label(if query_tab.chat_enabled {
                                                            "Chat ON"
                                                        } else {
                                                            "Chat OFF"
                                                        })
                                                        .when(query_tab.chat_enabled, |btn| {
                                                            btn.primary()
                                                        })
                                                        .on_click(cx.listener(|this, _, _window, cx| {
                                                            // Toggle chat for the current query tab
                                                            this.toggle_chat_for_active_tab(_window, cx);
                                                        }))
                                                )
                                                .child(
                                                    Button::new("add-row")
                                                        .outline()
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
                                                        .icon(IconName::Copy)
                                                        .label("Duplicate Row")
                                                        // TODO: Disable when no row is selected
                                                        .on_click(cx.listener(|this, _, _window, cx| {
                                                            // Duplicate the first row (for now - later we'll implement row selection)
                                                            if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                                                                query_tab.results_panel.update(cx, |results_panel, cx| {
                                                                    results_panel.duplicate_row(0, cx); // Duplicate first row for now
                                                                });
                                                            }
                                                        })),
                                                )
                                                // Commit and rollback buttons (always available)
                                                .child(
                                                    Button::new("commit-changes")
                                                        .outline()
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
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child("Double-click cells to edit")
                                                )
                                )
                                )
                                // Right side: Chat panel (only when enabled)
                                .when(
                                    query_tab.chat_enabled && query_tab.chat_panel.is_some(),
                                    |this| {
                                        this.child(
                                            div()
                                                .h_full()
                                                .w_80()
                                                .min_w_0()
                                                .border_l_1()
                                                .border_color(cx.theme().border)
                                                .child(query_tab.chat_panel.as_ref().unwrap().clone())
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
                                .overflow_hidden()
                                // Editor
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .border_t_1()
                                        .border_color(cx.theme().border)
                                        .child(
                                            TextInput::new(&settings_tab.editor)
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
                                                .primary()
                                                .icon(IconName::Check)
                                                .label("Save Settings")
                                                .children(vec![Kbd::new(Keystroke::parse("shift-enter").unwrap()).into_any_element()])
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.save_settings(window, cx);
                                                })),
                                        )
                                )
                        )
                    }
                }
            })
    }
}

/// Create a chat panel with a real provider based on current settings
fn create_chat_panel_with_provider(
    tab_id: usize,
    connection_string: String,
    window: &mut Window,
    cx: &mut gpui::Context<ChatPanel>,
) -> anyhow::Result<ChatPanel> {
    // Load current settings
    let settings = load_settings().map_err(|e| anyhow::anyhow!("Failed to load settings: {}", e))?;

    // Validate settings
    let validation_errors = ChatProviderResolver::validate_settings(&settings);
    if !validation_errors.is_empty() {
        return Err(anyhow::anyhow!("Invalid chat settings: {}", validation_errors.join(", ")));
    }

    // Create HTTP client using reqwest_client from zed
    let http_client = Arc::new(reqwest_client::ReqwestClient::new());

    // Get db_service
    let db_service = DbService::global(cx).clone();

    // Create resolver and get provider
    let mut resolver = ChatProviderResolver::new(http_client, db_service);
    resolver.set_connection_string(connection_string.clone());
    let provider_info = resolver.get_provider(&settings)?;

    // Create chat panel with the provider
    let chat_panel = ChatPanel::new_with_provider(
        tab_id,
        provider_info.provider,
        provider_info.provider_name,
        provider_info.model_name,
        window,
        cx
    );

    Ok(chat_panel)
}
