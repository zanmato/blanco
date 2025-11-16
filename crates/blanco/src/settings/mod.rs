use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub editor: EditorSettings,
    pub database: DatabaseSettings,
    pub appearance: AppearanceSettings,
    pub chat: ChatSettings,
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

pub fn get_settings_path() -> PathBuf {
    let config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    config_dir.join("blanco").join("settings.json")
}

pub fn load_settings() -> Result<Settings, Box<dyn std::error::Error>> {
    let settings_path = get_settings_path();

    if !settings_path.exists() {
        create_default_settings()?;
        return Ok(Settings::default());
    }

    let content = fs::read_to_string(&settings_path)?;
    let settings: Settings = serde_json::from_str(&content)?;
    Ok(settings)
}

pub fn save_settings(settings: &Settings) -> Result<(), Box<dyn std::error::Error>> {
    let settings_path = get_settings_path();

    // Create directory if it doesn't exist
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json_content = serde_json::to_string_pretty(settings)?;
    let mut file = fs::File::create(&settings_path)?;
    file.write_all(json_content.as_bytes())?;

    // Set file permissions to 600 (read/write for owner only)
    let mut perms = fs::metadata(&settings_path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&settings_path, perms)?;

    Ok(())
}

pub fn create_default_settings() -> Result<(), Box<dyn std::error::Error>> {
    let default_settings = Settings::default();
    save_settings(&default_settings)
}
