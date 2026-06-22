mod formatter_page;
mod keybindings_page;
mod view;

pub use view::SettingsView;

use crate::app_settings::AppSettings;
use gpui::{App, SharedString};
use gpui_component::Theme;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Apply user font settings on top of the current theme.
/// Called after theme changes and on startup to ensure font preferences persist.
pub fn apply_font_settings(cx: &mut App) {
    let appearance = AppSettings::global(cx).settings.appearance.clone();
    if !appearance.font_family.is_empty() {
        Theme::global_mut(cx).font_family = SharedString::from(appearance.font_family);
    }
    if !appearance.mono_font_family.is_empty() {
        Theme::global_mut(cx).mono_font_family = SharedString::from(appearance.mono_font_family);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub editor: EditorSettings,
    pub formatter: FormatterSettings,
    pub database: DatabaseSettings,
    pub appearance: AppearanceSettings,
    pub chat: ChatSettings,
    /// User overrides for keyboard shortcuts, keyed by action identifier (see
    /// `crate::keybindings`). Only entries that differ from the built-in
    /// defaults are stored here; an empty string means the action is unbound.
    #[serde(default)]
    pub keybindings: HashMap<String, String>,
}

impl Settings {
    /// Load settings from key-value pairs
    pub fn from_key_values(values: &[(String, String)]) -> Self {
        let mut settings = Settings::default();

        for (key, value) in values {
            match key.as_str() {
                "general.check_for_updates" => {
                    settings.general.check_for_updates = value.parse().unwrap_or_default();
                }
                "editor.font_family" => {
                    // Legacy: migrate to appearance.mono_font_family
                    if settings.appearance.mono_font_family.is_empty() {
                        settings.appearance.mono_font_family = value.clone();
                    }
                }
                "editor.word_wrap" => {
                    settings.editor.word_wrap = value.parse().unwrap_or_default();
                }
                "editor.show_whitespace" => {
                    settings.editor.show_whitespace = value.parse().unwrap_or_default();
                }
                "editor.folding" => {
                    settings.editor.folding = value.parse().unwrap_or(true);
                }
                "editor.hard_tabs" => {
                    settings.editor.hard_tabs = value.parse().unwrap_or_default();
                }
                "editor.tab_size" => {
                    settings.editor.tab_size = value.parse().unwrap_or(2);
                }
                "database.default_connection_timeout_seconds" => {
                    settings.database.default_connection_timeout_seconds =
                        value.parse().unwrap_or_default();
                }
                "database.query_timeout_seconds" => {
                    settings.database.query_timeout_seconds = value.parse().unwrap_or_default();
                }
                "database.max_rows" => {
                    settings.database.max_rows = value.parse().unwrap_or_default();
                }
                "database.auto_limit_results" => {
                    settings.database.auto_limit_results = value.parse().unwrap_or_default();
                }
                "database.show_connection_notifications" => {
                    settings.database.show_connection_notifications =
                        value.parse().unwrap_or_default();
                }
                "database.max_history_items" => {
                    settings.database.max_history_items = value.parse().unwrap_or(1000);
                }
                "appearance.theme" => {
                    settings.appearance.theme = value.clone();
                }
                "appearance.font_family" => {
                    settings.appearance.font_family = value.clone();
                }
                "appearance.mono_font_family" => {
                    settings.appearance.mono_font_family = value.clone();
                }
                "chat.provider" => {
                    settings.chat.provider = value.clone();
                }
                "chat.model" => {
                    settings.chat.model = value.clone();
                }
                "chat.base_url" => {
                    settings.chat.base_url = value.clone();
                }
                "chat.max_tokens" => {
                    settings.chat.max_tokens = value.parse().unwrap_or_default();
                }
                "chat.temperature" => {
                    settings.chat.temperature = value.parse().unwrap_or_default();
                }
                "chat.auto_execute_queries" => {
                    settings.chat.auto_execute_queries = value.parse().unwrap_or_default();
                }
                "chat.show_thinking_process" => {
                    settings.chat.show_thinking_process = value.parse().unwrap_or_default();
                }
                "formatter.indented_joins" => {
                    settings.formatter.indented_joins = value.parse().unwrap_or(false);
                }
                "formatter.indented_ctes" => {
                    settings.formatter.indented_ctes = value.parse().unwrap_or(false);
                }
                "formatter.indented_using_on" => {
                    settings.formatter.indented_using_on = value.parse().unwrap_or(true);
                }
                "formatter.indented_on_contents" => {
                    settings.formatter.indented_on_contents = value.parse().unwrap_or(true);
                }
                "formatter.indented_then" => {
                    settings.formatter.indented_then = value.parse().unwrap_or(true);
                }
                "formatter.indented_then_contents" => {
                    settings.formatter.indented_then_contents = value.parse().unwrap_or(true);
                }
                "formatter.allow_implicit_indents" => {
                    settings.formatter.allow_implicit_indents = value.parse().unwrap_or(false);
                }
                "formatter.trailing_comments" => {
                    settings.formatter.trailing_comments = value.clone();
                }
                "formatter.max_line_length" => {
                    settings.formatter.max_line_length = value.parse().unwrap_or(80);
                }
                "formatter.keywords_policy" => {
                    settings.formatter.keywords_policy = value.clone();
                }
                "formatter.identifiers_policy" => {
                    settings.formatter.identifiers_policy = value.clone();
                }
                "formatter.functions_policy" => {
                    settings.formatter.functions_policy = value.clone();
                }
                "formatter.literals_policy" => {
                    settings.formatter.literals_policy = value.clone();
                }
                "formatter.types_policy" => {
                    settings.formatter.types_policy = value.clone();
                }
                "formatter.select_clause_trailing_comma" => {
                    settings.formatter.select_clause_trailing_comma = value.clone();
                }
                "formatter.terminator_multiline_newline" => {
                    settings.formatter.terminator_multiline_newline =
                        value.parse().unwrap_or(false);
                }
                "formatter.require_final_semicolon" => {
                    settings.formatter.require_final_semicolon = value.parse().unwrap_or(false);
                }
                "formatter.exclude_rules" => {
                    settings.formatter.exclude_rules = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                key if key.starts_with("keybinding.") => {
                    let action = key.trim_start_matches("keybinding.").to_string();
                    if !action.is_empty() {
                        settings.keybindings.insert(action, value.clone());
                    }
                }
                _ => {}
            }
        }

        settings
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub check_for_updates: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    pub word_wrap: bool,
    pub show_whitespace: bool,
    pub folding: bool,
    pub hard_tabs: bool,
    pub tab_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    pub default_connection_timeout_seconds: u32,
    pub query_timeout_seconds: u32,
    pub max_rows: u32,
    pub auto_limit_results: bool,
    pub show_connection_notifications: bool,
    /// Maximum number of query-history entries to retain. Older entries beyond
    /// this count are pruned after each execution.
    pub max_history_items: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: String,
    pub font_family: String,
    pub mono_font_family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSettings {
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub auto_execute_queries: bool,
    pub show_thinking_process: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            check_for_updates: true,
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            word_wrap: false,
            show_whitespace: false,
            folding: true,
            hard_tabs: false,
            tab_size: 2,
        }
    }
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            default_connection_timeout_seconds: 30,
            query_timeout_seconds: 60,
            max_rows: 1000,
            auto_limit_results: true,
            show_connection_notifications: true,
            max_history_items: 1000,
        }
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "One Dark - Darkened".to_string(),
            font_family: String::new(),
            mono_font_family: String::new(),
        }
    }
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api_key: "".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            auto_execute_queries: false,
            show_thinking_process: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatterSettings {
    pub indented_joins: bool,
    pub indented_ctes: bool,
    pub indented_using_on: bool,
    pub indented_on_contents: bool,
    pub indented_then: bool,
    pub indented_then_contents: bool,
    pub allow_implicit_indents: bool,
    pub trailing_comments: String,
    pub max_line_length: u32,
    pub exclude_rules: HashSet<String>,
    pub keywords_policy: String,
    pub identifiers_policy: String,
    pub functions_policy: String,
    pub literals_policy: String,
    pub types_policy: String,
    pub select_clause_trailing_comma: String,
    pub terminator_multiline_newline: bool,
    pub require_final_semicolon: bool,
}

impl Default for FormatterSettings {
    fn default() -> Self {
        Self {
            indented_joins: false,
            indented_ctes: false,
            indented_using_on: true,
            indented_on_contents: true,
            indented_then: true,
            indented_then_contents: true,
            allow_implicit_indents: false,
            trailing_comments: "before".to_string(),
            max_line_length: 80,
            exclude_rules: HashSet::from(["LT12".to_string()]),
            keywords_policy: "consistent".to_string(),
            identifiers_policy: "consistent".to_string(),
            functions_policy: "consistent".to_string(),
            literals_policy: "consistent".to_string(),
            types_policy: "consistent".to_string(),
            select_clause_trailing_comma: "forbid".to_string(),
            terminator_multiline_newline: false,
            require_final_semicolon: false,
        }
    }
}
