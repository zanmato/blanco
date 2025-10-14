use gpui::{
    div, prelude::FluentBuilder, px, Action, App, AppContext, ClickEvent, Context, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, Keystroke,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    highlighter::{Diagnostic, DiagnosticSeverity},
    input::{InputEvent, InputState, Position, TabSize, TextInput},
    sidebar::SidebarToggleButton,
    tab::{Tab, TabBar},
    v_flex, ActiveTheme, IconName, Kbd, Side, Sizable,
};
use log::{debug, error, info, warn};
use lsp_types::{PublishDiagnosticsParams, Uri};
use serde_json;

use crate::app::{ConnectionType, ToggleSidebar};
use crate::app_database::{QueryHistoryData, QueryTabData};
use crate::database::QueryResult;
use crate::db_service::{DbService, PgConnectionKey};
use crate::query_file::QueryFileManager;
use crate::settings::Settings;
use postgres_lsp::PostgresLspManager;
use std::sync::Arc;

#[derive(Clone)]
pub enum EditorPanelEvent {
    // No longer needed - tabs handle their own views
}

pub enum TabType {
    Query(QueryTab),
    Settings(SettingsTab),
}

pub struct QueryTab {
    pub id: usize,
    pub title: String,
    pub connection_name: String,
    pub connection_type: ConnectionType,
    pub pg_connection_key: Option<PgConnectionKey>, // For PostgreSQL connections
    pub connection_id: Option<i64>,                 // Connection ID to tie this tab to a connection
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>, // Database ID for persistence
    pub results_panel: Entity<crate::results_panel::ResultsPanel>, // Each tab has its own results
    pub lsp_manager: Option<PostgresLspManager>, // LSP manager for PostgreSQL connections
    pub document_version: i32, // LSP document version for tracking changes
    pub file_uri: Option<String>, // File URI for LSP integration
    pub pending_diagnostics: Option<Vec<Diagnostic>>, // Store diagnostics from LSP
    pub diagnostics_dirty: bool, // Flag to indicate diagnostics need updating
}

impl QueryTab {
    /// Get the URI for this query tab
    fn uri(&self) -> String {
        // Use file URI if available (for LSP integration), otherwise fall back to synthetic URI
        if let Some(ref file_uri) = self.file_uri {
            file_uri.clone()
        } else {
            format!("inmemory://blanco/query_{}.sql", self.id)
        }
    }
}

pub struct SettingsTab {
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
    pending_save_task: Option<gpui::Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
    query_file_manager: Arc<QueryFileManager>,
}

impl EditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_sidebar_state(window, cx, false)
    }

    pub fn new_with_sidebar_state(
        window: &mut Window,
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

        let first_tab = QueryTab {
            id: 0,
            title: "Query 1".to_string(),
            connection_name: "Test Database".to_string(),
            connection_type: ConnectionType::SQLite,
            pg_connection_key: None,
            connection_id: None,
            editor: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sql".to_string())
                    .line_number(true)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: false,
                    })
                    .soft_wrap(false)
                    .placeholder("Enter your SQL query here...")
            }),
            db_id: None,
            results_panel: cx.new(|cx| crate::results_panel::ResultsPanel::new(window, cx)),
            lsp_manager: None, // No LSP for SQLite
            document_version: 0,
            file_uri: None, // Will be set when saved
            pending_diagnostics: None,
            diagnostics_dirty: false,
        };

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![],
            active_tab_ix: 0,
            next_tab_id: 1,
            sidebar_collapsed,
            pending_save_task: None,
            _subscriptions: Vec::new(),
            query_file_manager,
        };

        // Subscribe to the first tab's editor changes
        let subscription = cx.subscribe(&first_tab.editor, |this, editor_entity, event, cx| {
            if let InputEvent::Change = event {
                this.trigger_auto_save(cx);

                // Send didChange to LSP if this is a PostgreSQL tab and the first tab is active
                if this.tabs.len() > 0 {
                    if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(0) {
                        if let Some(ref lsp_manager) = query_tab.lsp_manager {
                            query_tab.document_version += 1;
                            let version = query_tab.document_version;
                            let uri_str = query_tab.uri();
                            let content = editor_entity.read(cx).text().to_string();
                            let lsp_clone = lsp_manager.clone();
                            let tab_title = query_tab.title.clone();

                            crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                                if let Ok(uri) = uri_str.parse::<Uri>() {
                                    info!("📤 Sending didChange notification for tab '{}' version {}", tab_title, version);
                                    lsp_clone.did_change(uri, version, content).await
                                        .map_err(|e| anyhow::anyhow!("didChange failed: {:?}", e))?;
                                    info!("✅ didChange notification sent successfully for tab '{}' version {}", tab_title, version);
                                    Ok(())
                                } else {
                                    error!("Failed to parse URI for didChange: {}", uri_str);
                                    Err(anyhow::anyhow!("URI parse failed"))
                                }
                            }).detach();
                        }
                    }
                }
            }
        });

        panel._subscriptions.push(subscription);
        panel.tabs.push(TabType::Query(first_tab));
        panel
    }

    pub fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        cx.notify();
    }

    fn add_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(false)
                .placeholder("Enter your SQL query here...")
        });

        // Subscribe to editor changes
        let subscription = cx.subscribe(&editor, |this, editor_entity, event, cx| {
            if let InputEvent::Change = event {
                this.trigger_auto_save(cx);

                // Send didChange to LSP if this is a PostgreSQL tab
                if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                    if let Some(ref lsp_manager) = query_tab.lsp_manager {
                        query_tab.document_version += 1;
                        let version = query_tab.document_version;
                        let uri_str = query_tab.uri();
                        let content = editor_entity.read(cx).text().to_string();
                        let lsp_clone = lsp_manager.clone();
                        let tab_title = query_tab.title.clone();

                        crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                            if let Ok(uri) = uri_str.parse::<Uri>() {
                                info!("📤 Sending didChange notification for tab '{}' version {}", tab_title, version);
                                lsp_clone.did_change(uri, version, content).await
                                    .map_err(|e| anyhow::anyhow!("didChange failed: {:?}", e))?;
                                info!("✅ didChange notification sent successfully for tab '{}' version {}", tab_title, version);
                                Ok(())
                            } else {
                                error!("Failed to parse URI for didChange: {}", uri_str);
                                Err(anyhow::anyhow!("URI parse failed"))
                            }
                        }).detach();
                    }
                }
            }
        });
        self._subscriptions.push(subscription);

        let new_tab = QueryTab {
            id: tab_id,
            title: format!("Query {}", tab_id + 1),
            connection_name: "Test Database".to_string(),
            connection_type: ConnectionType::SQLite,
            pg_connection_key: None,
            connection_id: None,
            editor,
            db_id: None,
            results_panel: cx.new(|cx| crate::results_panel::ResultsPanel::new(window, cx)),
            lsp_manager: None, // No LSP for SQLite
            document_version: 0,
            file_uri: None, // Will be set when saved
            pending_diagnostics: None,
            diagnostics_dirty: false,
        };

        self.tabs.push(TabType::Query(new_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        cx.notify();
    }

    pub fn add_new_tab_with_connection(
        &mut self,
        window: &mut Window,
        connection_name: String,
        connection_type: crate::app::ConnectionType,
        schema_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(false)
                .placeholder("Enter your SQL query here...")
        });

        // Subscribe to editor changes
        let subscription = cx.subscribe(&editor, |this, editor_entity, event, cx| {
            if let InputEvent::Change = event {
                this.trigger_auto_save(cx);

                // Send didChange to LSP if this is a PostgreSQL tab
                if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                    if let Some(ref lsp_manager) = query_tab.lsp_manager {
                        query_tab.document_version += 1;
                        let version = query_tab.document_version;
                        let uri_str = query_tab.uri();
                        let content = editor_entity.read(cx).text().to_string();
                        let lsp_clone = lsp_manager.clone();
                        let tab_title = query_tab.title.clone();

                        crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                            if let Ok(uri) = uri_str.parse::<Uri>() {
                                info!("📤 Sending didChange notification for tab '{}' version {}", tab_title, version);
                                lsp_clone.did_change(uri, version, content).await
                                    .map_err(|e| anyhow::anyhow!("didChange failed: {:?}", e))?;
                                info!("✅ didChange notification sent successfully for tab '{}' version {}", tab_title, version);
                                Ok(())
                            } else {
                                error!("Failed to parse URI for didChange: {}", uri_str);
                                Err(anyhow::anyhow!("URI parse failed"))
                            }
                        }).detach();
                    }
                }
            }
        });
        self._subscriptions.push(subscription);

        let title = match (&connection_type, &schema_name) {
            (crate::app::ConnectionType::PostgreSQL, Some(schema)) => {
                format!("PostgreSQL - {}", schema)
            }
            (crate::app::ConnectionType::PostgreSQL, None) => "PostgreSQL".to_string(),
            (crate::app::ConnectionType::SQLite, _) => connection_name.clone(),
        };

        // Extract PostgreSQL connection key from connection name if it's a PostgreSQL connection
        let pg_connection_key = if connection_type == crate::app::ConnectionType::PostgreSQL {
            // Try to parse the connection name as a display name to extract the key
            // This is a workaround - ideally we should pass the key directly
            Self::extract_pg_key_from_display_name(&connection_name)
        } else {
            None
        };

        let new_tab = QueryTab {
            id: tab_id,
            title,
            connection_name: connection_name.clone(),
            connection_type,
            pg_connection_key,
            connection_id: None,
            editor,
            db_id: None,
            results_panel: cx.new(|cx| crate::results_panel::ResultsPanel::new(window, cx)),
            lsp_manager: None, // Will be initialized later if needed
            document_version: 0,
            file_uri: None, // Will be set when saved
            pending_diagnostics: None,
            diagnostics_dirty: false,
        };

        self.tabs.push(TabType::Query(new_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        cx.notify();
    }

    /// Add a new query tab specifically for PostgreSQL connections
    pub fn add_new_tab_with_postgres_connection(
        &mut self,
        window: &mut Window,
        display_name: String,
        pg_connection_key: PgConnectionKey,
        schema_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(false)
                .placeholder("Enter your SQL query here...")
        });

        // Subscribe to editor changes
        let subscription = cx.subscribe(&editor, |this, editor_entity, event, cx| {
            if let InputEvent::Change = event {
                this.trigger_auto_save(cx);

                // Send didChange to LSP if this is a PostgreSQL tab
                if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                    if let Some(ref lsp_manager) = query_tab.lsp_manager {
                        query_tab.document_version += 1;
                        let version = query_tab.document_version;
                        let uri_str = query_tab.uri();
                        let content = editor_entity.read(cx).text().to_string();
                        let lsp_clone = lsp_manager.clone();
                        let tab_title = query_tab.title.clone();

                        crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                            if let Ok(uri) = uri_str.parse::<Uri>() {
                                info!("📤 Sending didChange notification for tab '{}' version {}", tab_title, version);
                                lsp_clone.did_change(uri, version, content).await
                                    .map_err(|e| anyhow::anyhow!("didChange failed: {:?}", e))?;
                                info!("✅ didChange notification sent successfully for tab '{}' version {}", tab_title, version);
                                Ok(())
                            } else {
                                error!("Failed to parse URI for didChange: {}", uri_str);
                                Err(anyhow::anyhow!("URI parse failed"))
                            }
                        }).detach();
                    }
                }
            }
        });
        self._subscriptions.push(subscription);

        let title = match &schema_name {
            Some(schema) => format!("PostgreSQL - {}", schema),
            None => display_name.clone(),
        };

        // Create the new PostgreSQL tab
        let new_tab = QueryTab {
            id: tab_id,
            title: title.clone(),
            connection_name: display_name.clone(),
            connection_type: ConnectionType::PostgreSQL,
            pg_connection_key: Some(pg_connection_key),
            connection_id: None,
            editor,
            db_id: None,
            results_panel: cx.new(|cx| crate::results_panel::ResultsPanel::new(window, cx)),
            lsp_manager: None, // Will be initialized asynchronously
            document_version: 0,
            file_uri: None, // Will be set when saved
            pending_diagnostics: None,
            diagnostics_dirty: false,
        };

        self.tabs.push(TabType::Query(new_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        cx.notify();

        // Initialize LSP for the new PostgreSQL tab
        info!("🚀 Initializing LSP for new PostgreSQL tab: {}", title);
        let new_tab_index = self.tabs.len() - 1; // Index of the just-added tab
        self.initialize_lsp_for_tab_at_index(new_tab_index, window, cx);
    }

    /// Extract PostgreSQL connection key from display name
    /// This is a workaround - ideally we should pass the key directly from the sidebar
    fn extract_pg_key_from_display_name(display_name: &str) -> Option<PgConnectionKey> {
        // Parse display name format: "username@host:port/database"
        if let Some(at_pos) = display_name.find('@') {
            let username = display_name[..at_pos].to_string();
            let remaining = &display_name[at_pos + 1..];

            if let Some(colon_pos) = remaining.find(':') {
                let host = remaining[..colon_pos].to_string();
                let remaining = &remaining[colon_pos + 1..];

                if let Some(slash_pos) = remaining.find('/') {
                    let port_str = &remaining[..slash_pos];
                    let database = remaining[slash_pos + 1..].to_string();

                    if let Ok(port) = port_str.parse::<u16>() {
                        return Some(PgConnectionKey {
                            host,
                            port,
                            database,
                            username,
                            password: None, // No password in display name format
                        });
                    }
                }
            }
        }
        None
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

                crate::gpui_tokio::Tokio::spawn_result(cx, async move {
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
            // Save the current tab before switching
            self.save_current_tab_immediate(cx);

            self.active_tab_ix = ix;
            cx.notify();
        }
    }

    /// Extract the query to execute based on selection or cursor position
    fn extract_current_query(text: &str, cursor_pos: usize, has_selection: bool) -> String {
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

    fn run_query(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab_ix) {
            match tab {
                TabType::Query(query_tab) => {
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

                    println!(
                        "Executing query: {} on {}",
                        query, query_tab.connection_name
                    );

                    // Get database service
                    let db_service = DbService::global(cx).clone();
                    let user_db = db_service.user_db_handle();
                    let app_db = db_service.app_db_handle();

                    // Track execution timing
                    let start_time = std::time::Instant::now();
                    let executed_at = chrono::Utc::now().timestamp();

                    // Clone connection details for the async task
                    let connection_type = query_tab.connection_type.clone();
                    let pg_connection_key = query_tab.pg_connection_key.clone();

                    // Execute query using Tokio::spawn_result to ensure tokio context
                    let db_task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                        let query_result = match connection_type {
                            ConnectionType::SQLite => {
                                let db = user_db.read().await;
                                if !db.is_connected() {
                                    Err(anyhow::anyhow!("Not connected to SQLite database"))
                                } else {
                                    db.execute_query_async(&query)
                                        .await
                                        .map_err(|e| anyhow::anyhow!("{}", e))
                                }
                            }
                            ConnectionType::PostgreSQL => {
                                if let Some(pg_key) = pg_connection_key {
                                    // Get or create PostgreSQL connection
                                    let connection_string =
                                        if let Some(ref password) = pg_key.password {
                                            format!(
                                                "postgresql://{}:{}@{}:{}/{}",
                                                pg_key.username,
                                                password,
                                                pg_key.host,
                                                pg_key.port,
                                                pg_key.database
                                            )
                                        } else {
                                            format!(
                                                "postgresql://{}@{}:{}/{}",
                                                pg_key.username,
                                                pg_key.host,
                                                pg_key.port,
                                                pg_key.database
                                            )
                                        };

                                    match db_service
                                        .get_or_create_pg_connection(&connection_string)
                                        .await
                                    {
                                        Ok(pg_manager) => pg_manager
                                            .execute_query_async(&query)
                                            .await
                                            .map_err(|e| anyhow::anyhow!("{}", e)),
                                        Err(e) => Err(anyhow::anyhow!(
                                            "Failed to connect to PostgreSQL: {}",
                                            e
                                        )),
                                    }
                                } else {
                                    Err(anyhow::anyhow!("PostgreSQL connection key not found"))
                                }
                            }
                        };

                        let duration_ms = start_time.elapsed().as_millis() as i64;

                        match query_result {
                            Ok(mut result) => {
                                println!(
                                    "Query executed successfully: {} rows",
                                    result.row_count()
                                );

                                // Add execution metadata
                                result.query_text = Some(query.clone());
                                result.execution_time_ms = Some(duration_ms);
                                result.is_error = false;

                                // Save to query history
                                if let Some(app_db) = app_db.read().await.as_ref() {
                                    let history = QueryHistoryData {
                                        id: None,
                                        query_text: query.clone(),
                                        executed_at,
                                        duration_ms: Some(duration_ms),
                                        rows_affected: Some(result.rows_affected as i64),
                                        row_count: Some(result.row_count() as i64),
                                        success: true,
                                        error_message: None,
                                    };

                                    if let Err(e) = app_db.save_query_history(&history).await {
                                        eprintln!("Failed to save query history: {}", e);
                                    }
                                }

                                Ok(result)
                            }
                            Err(e) => {
                                let error_msg = e.to_string();
                                eprintln!("Query execution failed: {}", error_msg);

                                // Save error to query history
                                if let Some(app_db) = app_db.read().await.as_ref() {
                                    let history = QueryHistoryData {
                                        id: None,
                                        query_text: query.clone(),
                                        executed_at,
                                        duration_ms: Some(duration_ms),
                                        rows_affected: None,
                                        row_count: None,
                                        success: false,
                                        error_message: Some(error_msg.clone()),
                                    };

                                    if let Err(e) = app_db.save_query_history(&history).await {
                                        eprintln!("Failed to save query history: {}", e);
                                    }
                                }

                                // Return error as a result
                                Ok(QueryResult {
                                    columns: vec!["Error".to_string()],
                                    column_types: vec!["TEXT".to_string()],
                                    rows: vec![vec![error_msg.clone()]],
                                    rows_affected: 0,
                                    query_text: Some(query.clone()),
                                    execution_time_ms: Some(duration_ms),
                                    is_error: true,
                                })
                            }
                        }
                    });

                    // Get the results panel for this tab
                    let results_panel = query_tab.results_panel.clone();

                    // Update results panel when the task completes
                    cx.spawn(async move |_editor_panel, cx| {
                        if let Ok(result) = db_task.await {
                            let _ = results_panel.update(cx, |panel, cx| {
                                panel.set_query_result(result, cx);
                            });
                        }
                    })
                    .detach();
                }
                TabType::Settings(_) => {
                    // Settings tabs don't support query execution
                }
            }
        }
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn current_editor(&self) -> Option<&Entity<InputState>> {
        self.tabs.get(self.active_tab_ix).and_then(|tab| match tab {
            TabType::Query(query_tab) => Some(&query_tab.editor),
            TabType::Settings(settings_tab) => Some(&settings_tab.editor),
        })
    }

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
                let connection_type = match query_tab.connection_type {
                    ConnectionType::SQLite => Some("SQLite".to_string()),
                    ConnectionType::PostgreSQL => Some("PostgreSQL".to_string()),
                };
                let pg_connection_key = query_tab.pg_connection_key.as_ref().map(|key| {
                    if let Some(ref password) = key.password {
                        format!(
                            "postgres://{}:{}@{}:{}/{}",
                            key.username, password, key.host, key.port, key.database
                        )
                    } else {
                        format!(
                            "postgres://{}@{}:{}/{}",
                            key.username, key.host, key.port, key.database
                        )
                    }
                });
                tabs_data.push((
                    pos,
                    query_tab.db_id,
                    query_tab.title.clone(),
                    content,
                    pos as i32,
                    query_tab.connection_id,
                    connection_type,
                    pg_connection_key,
                ));
            }
        }

        info!("Saving {} query tabs on app quit", tabs_data.len());

        // Save tabs in background using global tokio runtime
        let query_file_manager = self.query_file_manager.clone();
        let save_task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
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

                    // Create/update the query file on disk
                    let file_uri = match query_file_manager
                        .create_query_file(final_db_id, &content)
                        .await
                    {
                        Ok(_) => {
                            let uri = query_file_manager.query_file_uri(final_db_id);
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
            Ok::<Vec<(usize, i64)>, anyhow::Error>(saved_ids)
        });

        // Update db_ids in tabs after save completes
        cx.spawn(async move |editor_panel, mut cx| {
            if let Ok(saved_ids) = save_task.await {
                let _ = editor_panel.update(cx, |panel, cx| {
                    for (tab_index, db_id) in saved_ids {
                        if let Some(TabType::Query(query_tab)) = panel.tabs.get_mut(tab_index) {
                            if query_tab.db_id.is_none() {
                                query_tab.db_id = Some(db_id);
                                // Set the file URI on the QueryTab
                                if let Some(file_uri) =
                                    panel.query_file_manager.query_file_uri(db_id).into()
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
        window: &mut Window,
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
            pending_save_task: None,
            _subscriptions: Vec::new(),
            query_file_manager,
        };

        if saved_tabs.is_empty() {
            debug!("No saved tabs found, creating default tab");
            // No saved tabs, create default tab
            panel.create_and_add_tab(window, "Query 1", "", None, None, &None, &None, cx);
        } else {
            // Restore saved tabs
            for tab_data in saved_tabs {
                debug!(
                    "Restoring tab '{}' with content_len: {} (db_id: {:?}, file_uri: {:?})",
                    tab_data.title,
                    tab_data.content.len(),
                    tab_data.id,
                    tab_data.file_uri
                );

                // Note: File creation will be handled asynchronously in the background

                panel.create_and_add_tab(
                    window,
                    &tab_data.title,
                    &tab_data.content,
                    tab_data.id,
                    tab_data.connection_id,
                    &tab_data.connection_type,
                    &tab_data.pg_connection_key,
                    cx,
                );
            }
        }

        info!("Restored {} tabs total", panel.tabs.len());
        panel
    }

    /// Helper to create and add a query tab with subscription
    fn create_and_add_tab(
        &mut self,
        window: &mut Window,
        title: &str,
        content: &str,
        db_id: Option<i64>,
        connection_id: Option<i64>,
        connection_type: &Option<String>,
        pg_connection_key: &Option<String>,
        cx: &mut Context<Self>,
    ) {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("sql".to_string())
                .line_number(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(false)
                .placeholder("Enter your SQL query here...")
        });

        // Set content if provided
        if !content.is_empty() {
            let content_owned = content.to_string();
            editor.update(cx, |state, cx| {
                state.replace(&content_owned, window, cx);
            });
        }

        // Subscribe to editor changes
        let subscription = cx.subscribe(&editor, |this, editor_entity, event, cx| {
            if let InputEvent::Change = event {
                this.trigger_auto_save(cx);

                // Send didChange to LSP if this is a PostgreSQL tab
                if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                    if let Some(ref lsp_manager) = query_tab.lsp_manager {
                        query_tab.document_version += 1;
                        let version = query_tab.document_version;
                        let uri_str = query_tab.uri();
                        let content = editor_entity.read(cx).text().to_string();
                        let lsp_clone = lsp_manager.clone();
                        let tab_title = query_tab.title.clone();

                        crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                            if let Ok(uri) = uri_str.parse::<Uri>() {
                                info!("📤 Sending didChange notification for tab '{}' version {}", tab_title, version);
                                lsp_clone.did_change(uri, version, content).await
                                    .map_err(|e| anyhow::anyhow!("didChange failed: {:?}", e))?;
                                info!("✅ didChange notification sent successfully for tab '{}' version {}", tab_title, version);
                                Ok(())
                            } else {
                                error!("Failed to parse URI for didChange: {}", uri_str);
                                Err(anyhow::anyhow!("URI parse failed"))
                            }
                        }).detach();
                    }
                }
            }
        });
        self._subscriptions.push(subscription);

        // Determine connection type and create appropriate tab
        let (connection_name, connection_type, pg_connection_key) = match connection_type {
            Some(conn_type) if conn_type == "PostgreSQL" => {
                info!(
                    "🔍 Creating PostgreSQL tab with connection_type: {:?}",
                    connection_type
                );
                let pg_key = pg_connection_key.as_ref().and_then(|key_str| {
                    crate::db_service::PgConnectionKey::from_connection_string(key_str).ok()
                });
                ("PostgreSQL".to_string(), ConnectionType::PostgreSQL, pg_key)
            }
            _ => {
                // Default to SQLite for backward compatibility
                ("Test Database".to_string(), ConnectionType::SQLite, None)
            }
        };

        // Initialize LSP manager for PostgreSQL connections
        // For now, we'll set this to None and initialize it asynchronously
        let lsp_manager = None;

        let query_tab = QueryTab {
            id: tab_id,
            title: title.to_string(),
            connection_name,
            connection_type,
            pg_connection_key,
            connection_id,
            editor,
            db_id,
            results_panel: cx.new(|cx| crate::results_panel::ResultsPanel::new(window, cx)),
            lsp_manager,
            document_version: 0,
            file_uri: db_id.and_then(|id| {
                // Try to get the file URI for existing saved tabs
                self.query_file_manager.query_file_uri(id).into()
            }),
            pending_diagnostics: None,
            diagnostics_dirty: false,
        };

        self.tabs.push(TabType::Query(query_tab));

        // Initialize LSP providers for PostgreSQL tabs
        if let TabType::Query(ref query_tab) = self.tabs.last().unwrap() {
            info!(
                "🔍 Checking LSP initialization for tab with connection_type: {:?}",
                query_tab.connection_type
            );
            if query_tab.connection_type == ConnectionType::PostgreSQL {
                info!(
                    "🚀 Initializing LSP for PostgreSQL tab: {}",
                    query_tab.title
                );
                let new_tab_index = self.tabs.len() - 1; // Index of the just-added tab
                self.initialize_lsp_for_tab_at_index(new_tab_index, window, cx);
            } else {
                info!("⏭️ Skipping LSP initialization for non-PostgreSQL tab");
            }
        }
    }

    /// Initialize LSP providers for a PostgreSQL tab at specific index
    fn initialize_lsp_for_tab_at_index(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        info!(
            "🔧 initialize_lsp_for_tab_at_index called for tab_index: {}",
            tab_index
        );
        if let Some(TabType::Query(query_tab)) = self.tabs.get_mut(tab_index) {
            info!(
                "🔧 Found tab: {}, connection_type: {:?}",
                query_tab.title, query_tab.connection_type
            );
            if query_tab.connection_type == ConnectionType::PostgreSQL {
                info!("🔧 Tab is PostgreSQL, proceeding with LSP initialization");
                let editor = query_tab.editor.clone();
                let tab_id = query_tab.id;
                let pg_connection_key = query_tab.pg_connection_key.clone();
                let connection_name = query_tab.connection_name.clone();

                // Get the file manager reference and tab db_id for the async task
                let query_file_manager = self.query_file_manager.clone();
                let tab_db_id = query_tab.db_id;

                // Initialize LSP providers asynchronously using Tokio runtime
                let lsp_task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                    use postgres_lsp::{PostgresLspConfig, PostgresLspManager};
                    use std::rc::Rc;

                    // Create LSP config from actual connection
                    let config = if let Some(conn_key) = pg_connection_key {
                        info!("🔍 Creating LSP config - conn_key.username: {}, conn_key.host: {}, conn_key.port: {}, conn_key.database: {}, conn_key.password: {}",
                            conn_key.username, conn_key.host, conn_key.port, conn_key.database,
                            if conn_key.password.is_some() { "<present>" } else { "<none>" });

                        let connection_string = if let Some(ref password) = conn_key.password {
                            let conn_str = format!(
                                "postgresql://{}:{}@{}:{}/{}",
                                conn_key.username,
                                password,
                                conn_key.host,
                                conn_key.port,
                                conn_key.database
                            );
                            info!("🔍 LSP connection string with password: {}",
                                conn_str.replace(password, "<hidden>"));
                            conn_str
                        } else {
                            let conn_str = format!(
                                "postgresql://{}@{}:{}/{}",
                                conn_key.username, conn_key.host, conn_key.port, conn_key.database
                            );
                            info!("🔍 LSP connection string without password: {}", conn_str);
                            conn_str
                        };

                        info!(
                            "Creating LSP config for connection: {} ({})",
                            connection_name, connection_string
                        );

                        PostgresLspConfig::new(
                            connection_string,
                            conn_key.host.clone(),
                            conn_key.port,
                            conn_key.database.clone(),
                            conn_key.username.clone(),
                            Some("public".to_string()), // Default schema
                        )
                    } else {
                        warn!(
                            "No connection key available for tab {}, using default config",
                            tab_id
                        );
                        PostgresLspConfig::default()
                    };

                    // Create LSP manager
                    let mut lsp_manager = match PostgresLspManager::new(config).await {
                        Ok(manager) => manager,
                        Err(e) => {
                            error!("Failed to create LSP manager for tab {}: {:?}", tab_id, e);
                            return Err(anyhow::anyhow!("Failed to create LSP manager: {:?}", e));
                        }
                    };

                    // Set up diagnostic handler using the query tab's diagnostic storage
                    let editor_clone_for_diagnostics = editor.clone();
                    let tab_id_for_diagnostics = tab_id;

                    // Create a diagnostic handler that stores diagnostics in the editor
                    let editor_clone_for_diagnostic_storage = editor.clone();
                    lsp_manager.set_diagnostic_handler(Arc::new(move |diagnostic_params: PublishDiagnosticsParams| {
                        info!("🔍 Received {} diagnostics from LSP for tab {}: {:?}",
                              diagnostic_params.diagnostics.len(), tab_id_for_diagnostics, diagnostic_params.uri);

                        // Convert LSP diagnostics to GPUI diagnostics
                        let mut diagnostics = Vec::new();
                        for lsp_diag in &diagnostic_params.diagnostics {
                            let start = Position::new(
                                lsp_diag.range.start.line as u32,
                                lsp_diag.range.start.character as u32
                            );
                            let end = Position::new(
                                lsp_diag.range.end.line as u32,
                                lsp_diag.range.end.character as u32
                            );

                            let severity = match lsp_diag.severity {
                                Some(lsp_types::DiagnosticSeverity::ERROR) => DiagnosticSeverity::Error,
                                Some(lsp_types::DiagnosticSeverity::WARNING) => DiagnosticSeverity::Warning,
                                Some(lsp_types::DiagnosticSeverity::INFORMATION) => DiagnosticSeverity::Info,
                                Some(lsp_types::DiagnosticSeverity::HINT) => DiagnosticSeverity::Hint,
                                Some(_) => DiagnosticSeverity::Warning,
                                None => DiagnosticSeverity::Error,
                            };

                            let diagnostic = Diagnostic::new(start..end, lsp_diag.message.clone())
                                .with_severity(severity)
                                .with_source("postgres-lsp");

                            diagnostics.push(diagnostic);
                        }

                        info!("🔧 Created {} diagnostics for tab {}", diagnostics.len(), tab_id_for_diagnostics);

                        // Store diagnostics in the editor for rendering
                        // Note: For now, we'll just log the diagnostics since we need a proper way to
                        // update the editor from this async context. The hover/completion providers
                        // are already set up and working.
                        info!("🔧 Diagnostics ready for tab {} ({} diagnostics)", tab_id_for_diagnostics, diagnostics.len());
                        // TODO: Implement proper diagnostic storage in editor when we have access to the GPUI context
                    })).await;

                    // Initialize the LSP manager
                    if let Err(e) = lsp_manager.initialize().await {
                        error!(
                            "Failed to initialize LSP manager for tab {}: {:?}",
                            tab_id, e
                        );
                        return Err(anyhow::anyhow!("Failed to initialize LSP manager: {:?}", e));
                    }

                    Ok::<(PostgresLspManager, (), (), ()), anyhow::Error>((lsp_manager, (), (), ()))
                });

                // Update the editor with LSP providers when initialization completes
                cx.spawn(async move |editor_panel, cx| {
                    match lsp_task.await {
                        Ok((lsp_manager, _, _, _)) => {
                            // Store the LSP manager in the query tab and set up LSP providers
                            let _ = editor_panel.update(cx, |panel, cx| {
                                if let Some(TabType::Query(query_tab)) = panel.tabs.get_mut(tab_index) {
                                    query_tab.lsp_manager = Some(lsp_manager.clone());

                                    // Create and set up LSP providers for the editor
                                    let uri_str = query_tab.uri();
                                    if let Ok(uri) = uri_str.parse::<Uri>() {
                                        info!("🔧 Setting up LSP providers for tab '{}' with URI: {}", query_tab.title, uri_str);

                                        // Create hover provider
                                        if let Some(hover_provider) = lsp_manager.create_hover_provider(uri.clone()) {
                                            info!("✅ Created hover provider for tab '{}'", query_tab.title);
                                            query_tab.editor.update(cx, |editor, cx| {
                                                editor.lsp.hover_provider = Some(std::rc::Rc::new(hover_provider));
                                                info!("🖱️ Hover provider set for tab '{}'", query_tab.title);
                                            });
                                        } else {
                                            warn!("⚠️ Failed to create hover provider for tab '{}'", query_tab.title);
                                        }

                                        // Create completion provider
                                        if let Some(completion_provider) = lsp_manager.create_completion_provider(uri.clone()) {
                                            info!("✅ Created completion provider for tab '{}'", query_tab.title);
                                            query_tab.editor.update(cx, |editor, cx| {
                                                editor.lsp.completion_provider = Some(std::rc::Rc::new(completion_provider));
                                                info!("🧩 Completion provider set for tab '{}'", query_tab.title);
                                            });
                                        } else {
                                            warn!("⚠️ Failed to create completion provider for tab '{}'", query_tab.title);
                                        }

                                        // Create code action provider
                                        if let Some(code_action_provider) = lsp_manager.create_code_action_provider(uri) {
                                            info!("✅ Created code action provider for tab '{}'", query_tab.title);
                                            query_tab.editor.update(cx, |editor, cx| {
                                                editor.lsp.code_action_providers.push(std::rc::Rc::new(code_action_provider));
                                                info!("⚡ Code action provider set for tab '{}'", query_tab.title);
                                            });
                                        } else {
                                            warn!("⚠️ Failed to create code action provider for tab '{}'", query_tab.title);
                                        }
                                    } else {
                                        error!("❌ Failed to parse URI for LSP providers: {}", uri_str);
                                    }

                                    // Send workspace configuration before didOpen
                                    let pg_connection_key = query_tab.pg_connection_key.clone();
                                    if let Some(conn_key) = pg_connection_key {
                                        let lsp_clone = lsp_manager.clone();
                                        let tab_title = query_tab.title.clone();
                                        let uri_str = query_tab.uri();
                                        let editor_for_content = query_tab.editor.clone();

                                        let tab_title_for_task = tab_title.clone();
                                        let uri_str_for_task = uri_str.clone();
                                        let lsp_for_task = lsp_manager.clone();
                                        let content_for_task = editor_for_content.read(cx).text().to_string();

                                        crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                                            // Create workspace configuration for this tab's connection
                                            let connection_string = if let Some(ref password) = conn_key.password {
                                                format!(
                                                    "postgresql://{}:{}@{}:{}/{}",
                                                    conn_key.username, password, conn_key.host, conn_key.port, conn_key.database
                                                )
                                            } else {
                                                format!(
                                                    "postgresql://{}@{}:{}/{}",
                                                    conn_key.username, conn_key.host, conn_key.port, conn_key.database
                                                )
                                            };

                                            let workspace_settings = serde_json::json!({
                                                "postgreslsp": {
                                                    "database": {
                                                        "connectionString": connection_string,
                                                        "database": conn_key.database,
                                                        "schema": "public"
                                                    },
                                                    "sql": {
                                                        "dialect": "postgresql",
                                                        "completion": {
                                                            "enableSchemas": true,
                                                            "enableTables": true,
                                                            "enableColumns": true,
                                                            "enableFunctions": true,
                                                            "enableKeywords": true,
                                                            "triggerCharacters": [".", " ", "(", ","]
                                                        },
                                                        "diagnostics": {
                                                            "enableSyntax": true,
                                                            "enableSemantic": true,
                                                            "enableSchema": true,
                                                            "enableLinting": true
                                                        },
                                                        "formatting": {
                                                            "enabled": true,
                                                            "keywordCase": "upper",
                                                            "identifierCase": "preserve",
                                                            "indentSize": 2,
                                                            "lineWidth": 80
                                                        }
                                                    },
                                                    "workspace": {
                                                        "root": ".",
                                                        "cacheEnabled": true
                                                    }
                                                }
                                            });

                                            info!("🔧 Sending workspace/configuration for tab '{}' with connection: {}", tab_title_for_task, connection_string);

                                            // Send workspace configuration change notification
                                            lsp_for_task.set_configuration(workspace_settings).await
                                                .map_err(|e| anyhow::anyhow!("workspace/configuration failed: {:?}", e))?;

                                            info!("✅ Workspace configuration sent successfully for tab '{}'", tab_title_for_task);

                                            // Now send didOpen notification
                                            let content = content_for_task;
                                            if let Ok(uri) = uri_str_for_task.parse::<Uri>() {
                                                info!("📤 Sending didOpen notification for tab '{}' with URI: {}", tab_title_for_task, uri_str_for_task);
                                                lsp_for_task.did_open(
                                                    uri,
                                                    "sql".to_string(),
                                                    1, // Initial version
                                                    content,
                                                ).await
                                                .map_err(|e| anyhow::anyhow!("didOpen failed: {:?}", e))?;
                                                info!("✅ didOpen notification sent successfully for tab '{}'", tab_title_for_task);
                                                Ok(())
                                            } else {
                                                error!("Failed to parse URI for didOpen: {}", uri_str_for_task);
                                                Err(anyhow::anyhow!("URI parse failed"))
                                            }
                                        }).detach();
                                    } else {
                                        error!("No connection key available for workspace configuration");
                                    }
                                }
                            });

                            info!("LSP initialized for tab {}", tab_id);
                        }
                        Err(e) => {
                            error!("LSP initialization failed for tab {}: {:?}", tab_id, e);
                        }
                    }
                }).detach();
            }
        }
    }

    /// Initialize LSP providers for a PostgreSQL tab (legacy method for backward compatibility)
    fn initialize_lsp_for_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.initialize_lsp_for_tab_at_index(self.active_tab_ix, window, cx);
    }

    /// Get LSP status for the current tab
    fn get_lsp_status(&self, cx: &Context<Self>) -> Option<&'static str> {
        if let Some(TabType::Query(query_tab)) = self.tabs.get(self.active_tab_ix) {
            if query_tab.connection_type == ConnectionType::PostgreSQL {
                // For now, return a placeholder status
                // In the future, this will check the actual LSP manager status
                Some("LSP Ready")
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Save the current tab immediately (used when switching tabs)
    fn save_current_tab_immediate(&mut self, cx: &mut Context<Self>) {
        if let Some(TabType::Query(query_tab)) = self.tabs.get(self.active_tab_ix) {
            let content = query_tab.editor.read(cx).text().to_string();
            let connection_type = match query_tab.connection_type {
                ConnectionType::SQLite => Some("SQLite".to_string()),
                ConnectionType::PostgreSQL => Some("PostgreSQL".to_string()),
            };
            let pg_connection_key = query_tab.pg_connection_key.as_ref().map(|key| {
                if let Some(ref password) = key.password {
                    format!(
                        "postgres://{}:{}@{}:{}/{}",
                        key.username, password, key.host, key.port, key.database
                    )
                } else {
                    format!(
                        "postgres://{}@{}:{}/{}",
                        key.username, key.host, key.port, key.database
                    )
                }
            });
            let tab_data = QueryTabData {
                id: query_tab.db_id,
                title: query_tab.title.clone(),
                content: content.clone(),
                position: self.active_tab_ix as i32,
                connection_id: query_tab.connection_id,
                connection_type,
                pg_connection_key,
                file_uri: None, // Will be updated after file creation
            };

            debug!(
                "Saving tab '{}' (db_id: {:?}, position: {}, content_len: {})",
                tab_data.title,
                tab_data.id,
                tab_data.position,
                content.len()
            );

            let db_service = DbService::global(cx).clone();
            let app_db = db_service.app_db_handle();
            let tab_index = self.active_tab_ix;
            let query_file_manager = self.query_file_manager.clone();

            let save_task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                if let Some(app_db) = app_db.read().await.as_ref() {
                    // First save to get or create the database ID
                    let final_db_id = match app_db.save_query_tab(&tab_data).await {
                        Ok(db_id) => {
                            debug!("Tab saved successfully with db_id: {}", db_id);
                            db_id
                        }
                        Err(e) => {
                            error!("Failed to save tab: {}", e);
                            return Err(anyhow::anyhow!("Failed to save tab: {}", e));
                        }
                    };

                    // Create/update the query file on disk
                    let file_uri = match query_file_manager
                        .create_query_file(final_db_id, &content)
                        .await
                    {
                        Ok(_) => {
                            let uri = query_file_manager.query_file_uri(final_db_id);
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
                            title: tab_data.title.clone(),
                            content: tab_data.content.clone(),
                            position: tab_data.position,
                            connection_id: tab_data.connection_id,
                            connection_type: tab_data.connection_type.clone(),
                            pg_connection_key: tab_data.pg_connection_key.clone(),
                            file_uri: Some(file_uri.clone()),
                        };

                        if let Err(e) = app_db.save_query_tab(&updated_tab_data).await {
                            error!("Failed to update file URI: {}", e);
                        } else {
                            debug!("Updated file URI: {}", file_uri);
                        }
                    }

                    Ok(final_db_id)
                } else {
                    error!("App database not initialized");
                    Err(anyhow::anyhow!("App database not initialized"))
                }
            });

            // Update the tab's db_id after save completes
            cx.spawn(async move |editor_panel, mut cx| {
                if let Ok(db_id) = save_task.await {
                    let _ = editor_panel.update(cx, |panel, cx| {
                        if let Some(TabType::Query(query_tab)) = panel.tabs.get_mut(tab_index) {
                            if query_tab.db_id.is_none() {
                                query_tab.db_id = Some(db_id);
                                // Set the file URI on the QueryTab
                                if let Some(file_uri) =
                                    panel.query_file_manager.query_file_uri(db_id).into()
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
                    });
                }
            })
            .detach();
        }
    }

    /// Trigger debounced auto-save for the current tab
    pub fn trigger_auto_save(&mut self, cx: &mut Context<Self>) {
        // Cancel any pending save task
        self.pending_save_task = None;

        // Schedule a new save after 500ms delay
        let task = cx.spawn(async move |editor_panel, mut cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;

            let _ = editor_panel.update(cx, |panel, cx| {
                panel.save_current_tab_immediate(cx);
            });
        });

        self.pending_save_task = Some(task);
    }

    /// Shutdown all LSP processes when the application quits
    pub fn shutdown_lsp_processes(&mut self, cx: &mut Context<Self>) {
        info!("🧹 Shutting down LSP processes...");

        // Collect all LSP managers from PostgreSQL tabs
        let mut lsp_managers: Vec<PostgresLspManager> = Vec::new();
        for tab in &mut self.tabs.iter_mut() {
            if let TabType::Query(query_tab) = tab {
                if let Some(ref lsp_manager) = query_tab.lsp_manager {
                    lsp_managers.push(lsp_manager.clone());
                }
            }
        }

        // Shutdown all LSP processes
        for lsp_manager in lsp_managers {
            crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                info!("🔹 Shutting down LSP manager...");
                lsp_manager.shutdown().await.map_err(|e| {
                    error!("Failed to shutdown LSP manager: {}", e);
                    anyhow::anyhow!("Failed to shutdown LSP manager: {}", e)
                })
            }).detach();
        }

        info!("✅ LSP processes shut down successfully");
    }
}

impl Focusable for EditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<EditorPanelEvent> for EditorPanel {}

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
                        SidebarToggleButton::left()
                            .side(Side::Left)
                            .collapsed(self.sidebar_collapsed)
                            .on_click(cx.listener(|_this, _event, window, cx| {
                                // Dispatch the toggle sidebar action
                                let action = ToggleSidebar;
                                window.dispatch_action(action.boxed_clone(), cx);
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
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(query_tab.connection_name.clone())
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
                                Tab::new(label)
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
                            )
                            .child(
                                Button::new("add-tab")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_new_tab(window, cx);
                                    })),
                            ),
                    ),
            )
            // Render the active tab's complete view
            .when_some(current_tab, |this, tab| {
                match tab {
                    TabType::Query(query_tab) => {
                        // Query tab: Editor + Button bar + Results view
                        this.child(
                            v_flex()
                                .flex_1()
                                .overflow_hidden()
                                // Editor
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .border_t_1()
                                        .border_color(cx.theme().border)
                                        .child(
                                            TextInput::new(&query_tab.editor)
                                                .bordered(false)
                                                .p_0()
                                                .h_full()
                                                .font_family("Fira Code")
                                                .text_size(px(14.))
                                                .focus_bordered(false),
                                        )
                                )
                                // Button bar (between editor and results)
                                .child(
                                    h_flex()
                                        .p_3()
                                        .gap_2()
                                        .border_t_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().muted.opacity(0.5))
                                        .child(
                                            Button::new("format-query")
                                                .outline()
                                                .icon(IconName::Asterisk)
                                                .label("Format")
                                                .children(vec![Kbd::new(Keystroke::parse("shift-f").unwrap()).into_any_element()]),
                                        )
                                        // LSP status indicator for PostgreSQL tabs
                                        .when_some(self.get_lsp_status(cx), |this, status| {
                                            this.child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .bg(cx.theme().primary.opacity(0.1))
                                                    .border_1()
                                                    .border_color(cx.theme().primary)
                                                    .rounded_md()
                                                    .child(
                                                        h_flex()
                                                            .gap_1()
                                                            .items_center()
                                                            .child(
                                                                div()
                                                                    .w_2()
                                                                    .h_2()
                                                                    .bg(cx.theme().primary)
                                                                    .rounded_full()
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(cx.theme().primary)
                                                                    .font_family("Fira Code")
                                                                    .child(status)
                                                            )
                                                    )
                                            )
                                        })
                                        .child(div().flex_1())
                                        // Edit controls (shown when editing)
                                        .when(query_tab.results_panel.read(cx).is_editing(cx), |this| {
                                            this.child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(
                                                        Button::new("cancel-edits")
                                                            .outline()
                                                            .icon(IconName::CircleX)
                                                            .label("Cancel")
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                                                                    query_tab.results_panel.update(cx, |panel, cx| {
                                                                        panel.cancel_all_edits(cx);
                                                                    });
                                                                }
                                                            })),
                                                    )
                                                    .child(
                                                        Button::new("commit-edits")
                                                            .primary()
                                                            .icon(IconName::Check)
                                                            .label("Commit Changes")
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                if let Some(TabType::Query(query_tab)) = this.tabs.get_mut(this.active_tab_ix) {
                                                                    let changes = query_tab.results_panel.update(cx, |panel, cx| {
                                                                        panel.commit_all_edits(cx)
                                                                    });

                                                                    // TODO: Generate and execute UPDATE queries for the changes
                                                                    for (row, col, new_value) in changes {
                                                                        println!("Committing change: row {}, col {}, value '{}'", row, col, new_value);
                                                                    }
                                                                }
                                                            })),
                                                    )
                                            )
                                        })
                                        // Normal run button (shown when not editing)
                                        .when(!query_tab.results_panel.read(cx).is_editing(cx), |this| {
                                            this.child(
                                                Button::new("run-query")
                                                    .primary()
                                                    .icon(IconName::Check)
                                                    .label("Run Current")
                                                    .children(vec![Kbd::new(Keystroke::parse("shift-enter").unwrap()).into_any_element()])
                                                    .on_click(cx.listener(Self::run_query)),
                                            )
                                        })
                                )
                                // Results
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h(px(100.))
                                        .child(query_tab.results_panel.clone())
                                )
                                // Row operation buttons
                                .child(
                                    h_flex()
                                        .p_3()
                                        .gap_2()
                                        .border_t_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().muted.opacity(0.5))
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
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Double-click cells to edit")
                                        )
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
