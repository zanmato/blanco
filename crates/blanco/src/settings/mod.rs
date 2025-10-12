use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::fs;
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub general: GeneralSettings,
    pub editor: EditorSettings,
    pub database: DatabaseSettings,
    pub appearance: AppearanceSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub auto_save: bool,
    pub auto_save_interval_seconds: u32,
    pub check_for_updates: bool,
    pub telemetry: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    pub font_size: u32,
    pub font_family: String,
    pub tab_size: u32,
    pub hard_tabs: bool,
    pub word_wrap: bool,
    pub line_numbers: bool,
    pub minimap: bool,
    pub auto_complete: bool,
    pub bracket_matching: bool,
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
    pub sidebar_width: u32,
    pub results_panel_height: u32,
    pub show_status_bar: bool,
    pub compact_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: GeneralSettings::default(),
            editor: EditorSettings::default(),
            database: DatabaseSettings::default(),
            appearance: AppearanceSettings::default(),
        }
    }
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            auto_save: true,
            auto_save_interval_seconds: 30,
            check_for_updates: true,
            telemetry: false,
        }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: 14,
            font_family: "Monaco".to_string(),
            tab_size: 4,
            hard_tabs: false,
            word_wrap: false,
            line_numbers: true,
            minimap: true,
            auto_complete: true,
            bracket_matching: true,
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
            theme: "dark".to_string(),
            sidebar_width: 250,
            results_panel_height: 300,
            show_status_bar: true,
            compact_mode: false,
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
    
    Ok(())
}

pub fn create_default_settings() -> Result<(), Box<dyn std::error::Error>> {
    let default_settings = Settings::default();
    save_settings(&default_settings)
}
