use gpui::{
    div, px, prelude::FluentBuilder, App, AppContext, ClickEvent, Context,
    Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Keystroke, ParentElement, Render,
    Styled, Window,
};
use std::sync::Arc;
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    input::{InputState, TabSize, TextInput},
    tab::{Tab, TabBar},
    v_flex, ActiveTheme, IconName, Kbd, Sizable,
};
use serde_json;

use crate::app_database::{QueryHistoryData, QueryTabData};
use crate::db_service::DbService;
use crate::database::QueryResult;
use crate::settings::Settings;

#[derive(Clone)]
pub enum EditorPanelEvent {
    QueryExecuted(QueryResult),
}

pub enum TabType {
    Query(QueryTab),
    Settings(SettingsTab),
}

pub struct QueryTab {
    pub id: usize,
    pub title: String,
    pub editor: Entity<InputState>,
    pub db_id: Option<i64>, // Database ID for persistence
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
}

impl EditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first_tab = QueryTab {
            id: 0,
            title: "Query 1".to_string(),
            editor: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sequel".to_string())
                    .line_number(true)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: false,
                    })
                    .soft_wrap(false)
                    .placeholder("Enter your SQL query here...")
            }),
            db_id: None,
        };

        Self {
            focus_handle: cx.focus_handle(),
            tabs: vec![TabType::Query(first_tab)],
            active_tab_ix: 0,
            next_tab_id: 1,
        }
    }

    fn add_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        let new_tab = QueryTab {
            id: tab_id,
            title: format!("Query {}", tab_id + 1),
            editor: cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor("sequel".to_string())
                    .line_number(true)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: false,
                    })
                    .soft_wrap(false)
                    .placeholder("Enter your SQL query here...")
            }),
            db_id: None,
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

    fn run_query(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab_ix) {
            match tab {
                TabType::Query(query_tab) => {
                    let query = query_tab.editor.read(cx).text().to_string();
                    println!("Executing query: {}", query);

                    // Get database service
                    let db_service = DbService::global(cx).clone();
                    let user_db = db_service.user_db_handle();
                    let app_db = db_service.app_db_handle();
                    let runtime = Arc::clone(&db_service.runtime);

                    // Run query in background thread using smol::unblock
                    smol::spawn(async move {
                        // Use tokio runtime to execute sqlx operations
                        runtime.block_on(async {
                            let db = user_db.read().await;

                            // Check if connected
                            if !db.is_connected() {
                                eprintln!("Not connected to a database");
                                return;
                            }

                            // Track execution time
                            let start_time = std::time::Instant::now();
                            let executed_at = chrono::Utc::now().timestamp();

                            // Execute the query
                            match db.execute_query(&query).await {
                                Ok(result) => {
                                    let duration_ms = start_time.elapsed().as_millis() as i64;

                                    println!("Query executed successfully: {} rows", result.row_count());

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

                                    // TODO: Update UI with results
                                    // For now, we'll just print the results
                                }
                                Err(e) => {
                                    let duration_ms = start_time.elapsed().as_millis() as i64;
                                    let error_msg = e.to_string();

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

                                    eprintln!("Query execution failed: {}", error_msg);
                                }
                            }
                        });
                    }).detach();
                }
                TabType::Settings(_) => {
                    self.save_settings(_window, cx);
                }
            }
        }
        cx.notify();
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

    fn get_tab_title(&self, tab: &TabType) -> String {
        match tab {
            TabType::Query(query_tab) => query_tab.title.clone(),
            TabType::Settings(settings_tab) => {
                let mut title = settings_tab.title.clone();
                if !settings_tab.is_valid {
                    title.push_str(" ⚠️");
                }
                title
            }
        }
    }

    fn is_settings_tab(&self, tab: &TabType) -> bool {
        matches!(tab, TabType::Settings(_))
    }

    /// Save all query tabs to the app database
    pub fn save_tabs(&mut self, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();
        let runtime = Arc::clone(&db_service.runtime);

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

        // Save in background using smol with tokio runtime
        smol::spawn(async move {
            runtime.block_on(async {
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
            });
        }).detach();
    }

    /// Load saved query tabs from the app database
    pub fn load_saved_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let db_service = DbService::global(cx).clone();
        let app_db = db_service.app_db_handle();
        let runtime = db_service.runtime();

        // Load tabs synchronously using block_on
        let saved_tabs = runtime.block_on(async {
            if let Some(app_db) = app_db.read().await.as_ref() {
                app_db.load_query_tabs().await.ok()
            } else {
                None
            }
        });

        // Update UI with loaded tabs
        if let Some(tabs) = saved_tabs {
            if !tabs.is_empty() {
                // Clear existing tabs
                self.tabs.clear();

                // Create tabs from saved data
                for tab_data in tabs {
                    let tab_id = self.next_tab_id;
                    self.next_tab_id += 1;

                    let editor = cx.new(|cx| {
                        let mut state = InputState::new(window, cx)
                            .code_editor("sequel".to_string())
                            .line_number(true)
                            .tab_size(TabSize {
                                tab_size: 4,
                                hard_tabs: false,
                            })
                            .soft_wrap(false)
                            .placeholder("Enter your SQL query here...");

                        // Set the saved content
                        state.replace(&tab_data.content, window, cx);
                        state
                    });

                    let query_tab = QueryTab {
                        id: tab_id,
                        title: tab_data.title,
                        editor,
                        db_id: tab_data.id,
                    };

                    self.tabs.push(TabType::Query(query_tab));
                }

                self.active_tab_ix = 0;
                cx.notify();
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

impl Render for EditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Handle pending text for settings tabs first
        let is_settings_tab = if let Some(TabType::Settings(settings_tab)) = self.tabs.get_mut(self.active_tab_ix) {
            if let Some(pending_text) = settings_tab.pending_text.take() {
                settings_tab.editor.update(cx, |state, cx| {
                    state.replace(&pending_text, window, cx);
                });
            }
            true
        } else {
            false
        };

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
                    .children(self.tabs.iter().map(|tab| Tab::new(&self.get_tab_title(tab))))
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
            .child(
                // Editor
                v_flex()
                    .flex_1()
                    .min_h(px(200.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .when_some(self.current_editor(), |this, editor| {
                        this.child(
                            TextInput::new(editor)
                                .bordered(false)
                                .p_0()
                                .h_full()
                                .font_family("Monaco")
                                .text_size(px(14.))
                                .focus_bordered(false),
                        )
                    })
                    // Show validation error for settings tab
                    .when_some(current_tab.and_then(|tab| {
                        if let TabType::Settings(settings_tab) = tab {
                            settings_tab.validation_error.as_ref()
                        } else {
                            None
                        }
                    }), |this, error| {
                        this.child(
                            div()
                                .p_2()
                                .bg(cx.theme().red.opacity(0.1))
                                .border_1()
                                .border_color(cx.theme().red)
                                .text_color(cx.theme().red)
                                .child(format!("❌ JSON Error: {}", error))
                        )
                    }),
            )
            .child(
                // Button bar - different for query vs settings tabs
                h_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.5))
                    .when(is_settings_tab, |this| {
                        this.child(
                            Button::new("reset-settings")
                                .outline()
                                .icon(IconName::Asterisk)
                                .label("Reset to Defaults")
                                .on_click(cx.listener(|this, _, window, cx| {
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
                    })
                    .when(!is_settings_tab, |this| {
                        this.child(
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
                                .label("Run Query")
                                .children(vec![Kbd::new(Keystroke::parse("shift-enter").unwrap()).into_any_element()])
                                .on_click(cx.listener(Self::run_query)),
                        )
                    }),
            )
    }
}
