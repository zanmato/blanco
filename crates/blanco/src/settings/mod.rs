mod view;

pub use view::SettingsView;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub editor: EditorSettings,
    pub database: DatabaseSettings,
    pub appearance: AppearanceSettings,
    pub chat: ChatSettings,
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
                    settings.editor.font_family = value.clone();
                }
                "editor.word_wrap" => {
                    settings.editor.word_wrap = value.parse().unwrap_or_default();
                }
                "editor.show_whitespace" => {
                    settings.editor.show_whitespace = value.parse().unwrap_or_default();
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
                "appearance.theme" => {
                    settings.appearance.theme = value.clone();
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
    pub font_family: String,
    pub word_wrap: bool,
    pub show_whitespace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    pub default_connection_timeout_seconds: u32,
    pub query_timeout_seconds: u32,
    pub max_rows: u32,
    pub auto_limit_results: bool,
    pub show_connection_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: String,
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
            font_family: "Fira Code".to_string(),
            word_wrap: false,
            show_whitespace: false,
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
        }
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "One Dark - Darkened".to_string(),
        }
    }
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api_key: "".to_string(),
            base_url: "https://api.openai.com".to_string(),
            max_tokens: 2048,
            temperature: 0.7,
            auto_execute_queries: false,
            show_thinking_process: false,
        }
    }
}
