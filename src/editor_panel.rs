use gpui::{
    div, px, prelude::FluentBuilder, App, AppContext, ClickEvent, Context,
    Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Keystroke, ParentElement, Render,
    Styled, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputState, TabSize, TextInput},
    sidebar::SidebarToggleButton,
    tab::{Tab, TabBar},
    v_flex, ActiveTheme, IconName, Kbd, Side, Sizable,
};
use serde_json;

use crate::app::{ToggleSidebar};
use crate::app_database::{QueryHistoryData, QueryTabData};
use crate::db_service::DbService;
use crate::database::QueryResult;
use crate::settings::Settings;

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
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>, // Database ID for persistence
    pub results_panel: Entity<crate::results_panel::ResultsPanel>, // Each tab has its own results
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
}

impl EditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first_tab = QueryTab {
            id: 0,
            title: "Query 1".to_string(),
            connection_name: "Test Database".to_string(),
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
        };

        Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![TabType::Query(first_tab)],
            active_tab_ix: 0,
            next_tab_id: 1,
            sidebar_collapsed: false,
        }
    }

    pub fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.sidebar_collapsed = collapsed;
        cx.notify();
    }

    fn add_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let new_tab = QueryTab {
            id: tab_id,
            title: format!("Query {}", tab_id + 1),
            connection_name: "Test Database".to_string(),
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
        };

        self.tabs.push(TabType::Query(new_tab));
        self.active_tab_ix = self.tabs.len() - 1;
        cx.notify();
    }

    pub fn add_settings_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Check if settings tab already exists
        if self.tabs.iter().any(|tab| matches!(tab, TabType::Settings(_))) {
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
        chars[start..end].iter().collect::<String>().trim().to_string()
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
                        &full_text,
                        cursor_pos,
                        false  // TODO: Detect actual selection state when API is available
                    );

                    if query.is_empty() {
                        println!("No query to execute");
                        return;
                    }

                    println!("Executing query: {}", query);

                    // Get database service
                    let db_service = DbService::global(cx).clone();
                    let user_db = db_service.user_db_handle();
                    let app_db = db_service.app_db_handle();

                    // Track execution timing
                    let start_time = std::time::Instant::now();
                    let executed_at = chrono::Utc::now().timestamp();

                    // Execute query using Tokio::spawn_result to ensure tokio context
                    let db_task = crate::gpui_tokio::Tokio::spawn_result(cx, async move {
                        let db = user_db.read().await;

                        let query_result = if !db.is_connected() {
                            Err(anyhow::anyhow!("Not connected to a database"))
                        } else {
                            db.execute_query_async(&query).await.map_err(|e| anyhow::anyhow!("{}", e))
                        };

                        let duration_ms = start_time.elapsed().as_millis() as i64;

                        match query_result {
                            Ok(mut result) => {
                                println!("Query executed successfully: {} rows", result.row_count());

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
                Ok(settings) => {
                    match crate::settings::save_settings(&settings) {
                        Ok(()) => {
                            settings_tab.original_settings = settings.clone();
                            settings_tab.validation_error = None;
                            settings_tab.is_valid = true;
                            self.apply_settings(&settings, cx);
                            println!("Settings saved successfully!");
                        }
                        Err(e) => {
                            settings_tab.validation_error = Some(format!("Failed to save settings: {}", e));
                            settings_tab.is_valid = false;
                        }
                    }
                }
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
        self.tabs.get(self.active_tab_ix).and_then(|tab| {
            match tab {
                TabType::Query(query_tab) => Some(&query_tab.editor),
                TabType::Settings(settings_tab) => Some(&settings_tab.editor),
            }
        })
    }

    fn is_settings_tab(&self, tab: &TabType) -> bool {
        matches!(tab, TabType::Settings(_))
    }

    /// Save all query tabs to the app database
    pub fn save_tabs(&mut self, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();

        // Collect tab data
        let mut tabs_data = Vec::new();
        for (pos, tab) in self.tabs.iter().enumerate() {
            if let TabType::Query(query_tab) = tab {
                let content = query_tab.editor.read(cx).text().to_string();
                tabs_data.push((
                    query_tab.db_id,
                    query_tab.title.clone(),
                    content,
                    pos as i32,
                ));
            }
        }

        println!("Saving {} tabs", tabs_data.len());

        // Save tabs in background using global tokio runtime
        crate::gpui_tokio::Tokio::spawn_result(cx, async move {
            if let Some(app_db) = app_db.read().await.as_ref() {
                for (db_id, title, content, position) in tabs_data {
                    let tab_data = QueryTabData {
                        id: db_id,
                        title,
                        content,
                        position,
                    };
                    if let Err(e) = app_db.save_query_tab(&tab_data).await {
                        eprintln!("Failed to save tab: {}", e);
                    }
                }
            }
            Ok::<(), anyhow::Error>(())
        }).detach();
    }

    /// Load saved query tabs from the app database
    pub fn load_saved_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();

        // Load tabs using global tokio runtime
        let task: gpui::Task<()> = cx.spawn(async move |editor_panel, mut cx| {
            let saved_tabs = if let Some(app_db) = app_db.read().await.as_ref() {
                app_db.load_query_tabs().await.ok().unwrap_or_default()
            } else {
                Vec::new()
            };

            // Update UI with loaded tabs
            if !saved_tabs.is_empty() {
                let _ = editor_panel.update(cx, |panel, _cx| {
                    // Clear existing tabs
                    panel.tabs.clear();

                    // We need window for InputState::new, but we don't have it in async context
                    // For now, we'll skip loading tabs - this needs a different approach
                    // TODO: Find a way to create InputState without window reference
                    println!("Loaded {} tabs from database (UI update pending)", saved_tabs.len());
                });
            }
        });
        task.detach();
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
                            .on_click(cx.listener(|_this, _, _window, cx| {
                                cx.dispatch_action(&ToggleSidebar);
                            }))
                    )
                    .children(self.tabs.iter().map(|tab| {
                        match tab {
                            TabType::Query(query_tab) => {
                                Tab::new(&query_tab.title)
                                    .suffix(
                                        div()
                                            .ml_2()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(query_tab.connection_name.clone())
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
                                        .child(div().flex_1())
                                        .child(
                                            Button::new("run-query")
                                                .primary()
                                                .icon(IconName::Check)
                                                .label("Run Current")
                                                .children(vec![Kbd::new(Keystroke::parse("shift-enter").unwrap()).into_any_element()])
                                                .on_click(cx.listener(Self::run_query)),
                                        )
                                )
                                // Results
                                .child(
                                    div()
                                        .h(px(300.))
                                        .min_h(px(100.))
                                        .child(query_tab.results_panel.clone())
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
