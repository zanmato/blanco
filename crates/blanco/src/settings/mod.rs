use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::fs;
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub editor: EditorSettings,
    pub database: DatabaseSettings,
    pub appearance: AppearanceSettings,
    pub lsp: LspSettings,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSettings {
    pub enabled: bool,
    pub auto_download: bool,
    pub completion: LspCompletionSettings,
    pub diagnostics: LspDiagnosticsSettings,
    pub formatting: LspFormattingSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspCompletionSettings {
    pub auto_trigger: bool,
    pub trigger_characters: Vec<String>,
    pub max_suggestions: u32,
    pub show_documentation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnosticsSettings {
    pub enabled: bool,
    pub real_time_validation: bool,
    pub underline_errors: bool,
    pub show_warnings: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspFormattingSettings {
    pub enabled: bool,
    pub format_on_save: bool,
    pub format_on_type: bool,
    pub sql_dialect: String,
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

impl Default for LspSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_download: true,
            completion: LspCompletionSettings::default(),
            diagnostics: LspDiagnosticsSettings::default(),
            formatting: LspFormattingSettings::default(),
        }
    }
}

impl Default for LspCompletionSettings {
    fn default() -> Self {
        Self {
            auto_trigger: true,
            trigger_characters: vec![".".to_string(), " ".to_string(), "(".to_string()],
            max_suggestions: 20,
            show_documentation: true,
        }
    }
}

impl Default for LspDiagnosticsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            real_time_validation: true,
            underline_errors: true,
            show_warnings: true,
        }
    }
}

impl Default for LspFormattingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            format_on_save: false,
            format_on_type: false,
            sql_dialect: "postgresql".to_string(),
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
