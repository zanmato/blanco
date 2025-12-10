use serde::{Deserialize, Serialize};
use gpui::App;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub editor: EditorSettings,
    pub database: DatabaseSettings,
    pub appearance: AppearanceSettings,
    pub chat: ChatSettings,
}

impl Settings {
    /// Convert settings to key-value pairs for database storage
    pub fn to_key_values(&self) -> Vec<(String, String)> {
        let mut values = Vec::new();

        // General settings
        values.push(("general.check_for_updates".to_string(), self.general.check_for_updates.to_string()));

        // Editor settings
        values.push(("editor.font_family".to_string(), self.editor.font_family.clone()));
        values.push(("editor.word_wrap".to_string(), self.editor.word_wrap.to_string()));

        // Database settings
        values.push(("database.default_connection_timeout_seconds".to_string(), self.database.default_connection_timeout_seconds.to_string()));
        values.push(("database.query_timeout_seconds".to_string(), self.database.query_timeout_seconds.to_string()));
        values.push(("database.max_rows".to_string(), self.database.max_rows.to_string()));
        values.push(("database.auto_limit_results".to_string(), self.database.auto_limit_results.to_string()));
        values.push(("database.show_connection_notifications".to_string(), self.database.show_connection_notifications.to_string()));

        // Appearance settings
        values.push(("appearance.theme".to_string(), self.appearance.theme.clone()));

        // Chat settings (excluding API key which is secret)
        values.push(("chat.provider".to_string(), self.chat.provider.clone()));
        values.push(("chat.model".to_string(), self.chat.model.clone()));
        values.push(("chat.base_url".to_string(), self.chat.base_url.clone()));
        values.push(("chat.max_tokens".to_string(), self.chat.max_tokens.to_string()));
        values.push(("chat.temperature".to_string(), self.chat.temperature.to_string()));
        values.push(("chat.auto_execute_queries".to_string(), self.chat.auto_execute_queries.to_string()));
        values.push(("chat.show_thinking_process".to_string(), self.chat.show_thinking_process.to_string()));

        values
    }

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
                "database.default_connection_timeout_seconds" => {
                    settings.database.default_connection_timeout_seconds = value.parse().unwrap_or_default();
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
                    settings.database.show_connection_notifications = value.parse().unwrap_or_default();
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

/// Load settings from database
pub async fn load_settings(db: &crate::app_database::AppDatabase, _cx: &mut App) -> Result<Settings, Box<dyn std::error::Error>> {
    // Load non-secret settings from database
    let db_values = db.load_all_settings().await?;
    let mut settings = Settings::from_key_values(&db_values);

    // Secret API key will be loaded from credentials in AppSettings::new
    // This happens after the settings are initialized

    Ok(settings)
}

/// Save settings to database and credentials
pub async fn save_settings(settings: &Settings, db: &crate::app_database::AppDatabase, cx: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    // Save non-secret settings to database
    let key_values = settings.to_key_values();
    for (key, value) in key_values {
        db.save_setting(&key, &value, false).await?;
    }

    // Save secret API key to credentials
    // Note: This is handled in real-time in settings_view.rs with debouncing
    // The API key is stored using cx.write_credentials when changed

    Ok(())
}

/// Get just the API key (placeholder implementation)
pub fn get_api_key(_cx: &App) -> Option<String> {
    // TODO: Implement credential retrieval when GPUI Credential is available
    None
}

/// Save just the API key (placeholder implementation)
pub fn save_api_key(api_key: &str, _cx: &mut App) {
    // TODO: Implement credential storage when GPUI Credential is available
    // For now, this is a no-op
    if api_key.is_empty() {
        // Clear the API key
    } else {
        // Save the API key
    }
}
