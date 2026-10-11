mod csv_group;
mod formatter_page;
mod keybindings_page;
mod view;

pub use view::SettingsView;

use crate::app_settings::AppSettings;
use gpui::{App, SharedString, WindowAppearance};
use gpui_component::Theme;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use transformers::{CsvOptions, CsvQuoting};

/// Switch to the registered theme called `name`, persist the choice, and
/// re-apply the user's font settings on top. Used by the settings page and the
/// command palette.
pub fn apply_theme_by_name(name: &str, cx: &mut App) {
    if !install_theme(name, cx) {
        return;
    }
    AppSettings::global_mut(cx).settings.appearance.theme = name.to_string();
    persist_setting("appearance.theme", name.to_string(), cx);
}

/// Make the theme called `name` the current one, without changing settings.
/// False when there is no such theme.
pub fn install_theme(name: &str, cx: &mut App) -> bool {
    let Some(theme_config) = gpui_component::ThemeRegistry::global(cx)
        .themes()
        .get(&SharedString::from(name.to_string()))
        .cloned()
    else {
        tracing::warn!("Unknown theme: {name}");
        return false;
    };
    let mode = theme_config.mode;
    Theme::global_mut(cx).apply_config(&theme_config);
    // Pushes the config into the base layer (scrollbars, resize handles,
    // input frame).
    Theme::change(mode, None, cx);
    apply_font_settings(cx);
    true
}

/// The theme to show: with Follow System on, the light or dark theme for
/// `appearance`, otherwise the chosen theme.
pub fn theme_for_appearance(appearance: &AppearanceSettings, system: WindowAppearance) -> &str {
    if !appearance.follow_system {
        return &appearance.theme;
    }
    match system {
        WindowAppearance::Light | WindowAppearance::VibrantLight => &appearance.light_theme,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => &appearance.dark_theme,
    }
}

/// With Follow System on, switch to the theme for `system`.
pub fn follow_system_appearance(system: WindowAppearance, cx: &mut App) {
    let appearance = &AppSettings::global(cx).settings.appearance;
    if appearance.follow_system {
        let name = theme_for_appearance(appearance, system).to_string();
        install_theme(&name, cx);
    }
}

/// Apply user font settings on top of the current theme.
/// Called after theme changes and on startup to ensure font preferences persist.
pub fn apply_font_settings(cx: &mut App) {
    let appearance = &AppSettings::global(cx).settings.appearance;
    let font_family = SharedString::from(appearance.font_family.clone());
    let mono_font_family = SharedString::from(appearance.mono_font_family.clone());
    if font_family.is_empty() && mono_font_family.is_empty() {
        return;
    }
    // `update` rather than `global_mut`, so the base layer's copy of the theme
    // gets the fonts too.
    Theme::update(cx, |theme| {
        if !font_family.is_empty() {
            theme.font_family = font_family;
        }
        if !mono_font_family.is_empty() {
            theme.mono_font_family = mono_font_family;
        }
    });
}

/// gpui-component re-applies the active theme whenever the registry changes
/// (startup load, hot reload), which puts the theme file's fonts back. Its
/// observer is registered by `gpui_component::init`, so this one runs after it
/// and the user's fonts win.
pub fn keep_fonts_on_theme_reload(cx: &mut App) {
    cx.observe_global::<gpui_component::ThemeRegistry>(apply_font_settings)
        .detach();
}

/// The keys that accept a completion suggestion in the code editors.
pub fn completion_accept_keys(cx: &App) -> gpui_component::input::CompletionAcceptKeys {
    AppSettings::global(cx)
        .settings
        .editor
        .completion_accept_keys
        .accept_keys()
}

/// Step the code editor font size by `delta` pixels, as the zoom shortcuts do.
pub fn adjust_editor_font_size(delta: f32, cx: &mut App) {
    let current = AppSettings::global(cx).settings.editor.font_size;
    set_editor_font_size(current + delta, cx);
}

/// Set the code editor font size, clamped to the supported range, and redraw
/// the open editors. Persisting is left to the caller, which debounces it.
pub fn set_editor_font_size(size: f32, cx: &mut App) {
    AppSettings::global_mut(cx).settings.editor.font_size =
        size.clamp(EditorSettings::MIN_FONT_SIZE, EditorSettings::MAX_FONT_SIZE);
    cx.refresh_windows();
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub editor: EditorSettings,
    pub formatter: FormatterSettings,
    pub database: DatabaseSettings,
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub results: ResultsSettings,
    /// Defaults for Copy as CSV, which the export dialog starts from too.
    #[serde(default)]
    pub csv: CsvSettings,
    /// The built-in MCP server external agents connect to.
    #[serde(default)]
    pub mcp: McpSettings,
    /// Last known window geometry, restored on the next startup. Empty until the
    /// window has been moved or resized at least once.
    #[serde(default)]
    pub window: WindowSettings,
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
                    settings.general.check_for_updates = value.parse().unwrap_or(true);
                }
                "general.follow_active_tab" => {
                    settings.general.follow_active_tab = value.parse().unwrap_or(true);
                }
                "editor.font_family" => {
                    // Legacy: migrate to appearance.mono_font_family
                    if settings.appearance.mono_font_family.is_empty() {
                        settings.appearance.mono_font_family = value.clone();
                    }
                }
                "editor.font_size" => {
                    settings.editor.font_size = value
                        .parse()
                        .unwrap_or(EditorSettings::DEFAULT_FONT_SIZE)
                        .clamp(EditorSettings::MIN_FONT_SIZE, EditorSettings::MAX_FONT_SIZE);
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
                "editor.tab_navigation_wraps" => {
                    settings.editor.tab_navigation_wraps = value.parse().unwrap_or(true);
                }
                "editor.tab_size" => {
                    settings.editor.tab_size = value.parse().unwrap_or(2);
                }
                "editor.completion_accept_keys" => {
                    settings.editor.completion_accept_keys =
                        CompletionAcceptKeysSetting::from_key(value);
                }
                "database.default_connection_timeout_seconds" => {
                    settings.database.default_connection_timeout_seconds =
                        value.parse().unwrap_or_default();
                }
                "database.query_timeout_seconds" => {
                    settings.database.query_timeout_seconds = value.parse().unwrap_or_default();
                }
                "database.script_timeout_seconds" => {
                    settings.database.script_timeout_seconds = value.parse().unwrap_or_default();
                }
                "database.show_connection_notifications" => {
                    settings.database.show_connection_notifications =
                        value.parse().unwrap_or_default();
                }
                "database.idle_disconnect_minutes" => {
                    settings.database.idle_disconnect_minutes = value.parse().unwrap_or_default();
                }
                "database.max_history_items" => {
                    settings.database.max_history_items = value.parse().unwrap_or(1000);
                }
                "csv.delimiter" => {
                    settings.csv.delimiter = CsvDelimiter::from_stored(value);
                }
                "csv.custom_delimiter" => {
                    settings.csv.custom_delimiter = value.clone();
                }
                "csv.header" => {
                    settings.csv.header = value.parse().unwrap_or(true);
                }
                "csv.null_text" => {
                    settings.csv.null_text = value.clone();
                }
                "csv.quote" => {
                    settings.csv.quote = value.clone();
                }
                "csv.quoting" => {
                    settings.csv.quoting = CsvQuoting::from_stored(value);
                }
                "csv.replace_line_breaks" => {
                    settings.csv.replace_line_breaks = value.parse().unwrap_or_default();
                }
                "csv.line_break_replacement" => {
                    settings.csv.line_break_replacement = value.clone();
                }
                "appearance.theme" => {
                    settings.appearance.theme = value.clone();
                }
                "appearance.follow_system" => {
                    settings.appearance.follow_system = value.parse().unwrap_or_default();
                }
                "appearance.light_theme" => {
                    settings.appearance.light_theme = value.clone();
                }
                "appearance.dark_theme" => {
                    settings.appearance.dark_theme = value.clone();
                }
                "appearance.font_family" => {
                    settings.appearance.font_family = value.clone();
                }
                "appearance.mono_font_family" => {
                    settings.appearance.mono_font_family = value.clone();
                }
                "appearance.terminal_font_size" => {
                    settings.appearance.terminal_font_size = value
                        .parse()
                        .unwrap_or(AppearanceSettings::default().terminal_font_size);
                }
                "results.date_format" => {
                    settings.results.date_format =
                        DateDisplayFormat::from_key(value).unwrap_or_default();
                }
                "results.time_zone" => {
                    settings.results.time_zone =
                        TimeZoneDisplay::from_key(value).unwrap_or_default();
                }
                "mcp.enabled" => {
                    settings.mcp.enabled = value.parse().unwrap_or_default();
                }
                "mcp.port" => {
                    settings.mcp.port = value.parse().unwrap_or(McpSettings::DEFAULT_PORT);
                }
                "mcp.allow_writes" => {
                    settings.mcp.allow_writes = value.parse().unwrap_or_default();
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
                "window.x" => {
                    settings.window.x = value.parse().ok();
                }
                "window.y" => {
                    settings.window.y = value.parse().ok();
                }
                "window.width" => {
                    settings.window.width = value.parse().ok();
                }
                "window.height" => {
                    settings.window.height = value.parse().ok();
                }
                "window.maximized" => {
                    settings.window.maximized = value.parse().unwrap_or_default();
                }
                "results.filter_bar_open" => {
                    settings.results.filter_bar_open = value.parse().unwrap_or_default();
                }
                "results.filter_operator" => {
                    settings.results.filter_operator = value.clone();
                }
                "results.show_row_numbers" => {
                    settings.results.show_row_numbers = value.parse().unwrap_or(true);
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

/// The result grid: its server-side filter bar, row numbers and date display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultsSettings {
    /// Whether the filter bar is shown above a table's rows.
    pub filter_bar_open: bool,
    /// The operator a new filter condition starts with, by
    /// `FilterOperator::key`.
    pub filter_operator: String,
    /// Whether the grid shows the row number column. Applies to results
    /// shown after the change.
    pub show_row_numbers: bool,
    /// How the grid shows date and time values. Display only: copy, export
    /// and cell editing keep the text the database returned.
    #[serde(default)]
    pub date_format: DateDisplayFormat,
    /// The zone a timestamp with a time zone is shown in.
    #[serde(default)]
    pub time_zone: TimeZoneDisplay,
}

impl Default for ResultsSettings {
    fn default() -> Self {
        Self {
            filter_bar_open: false,
            filter_operator: blanco_core::FilterOperator::Equals.key().to_string(),
            show_row_numbers: true,
            date_format: DateDisplayFormat::default(),
            time_zone: TimeZoneDisplay::default(),
        }
    }
}

impl ResultsSettings {
    pub fn default_operator(&self) -> blanco_core::FilterOperator {
        blanco_core::FilterOperator::from_key(&self.filter_operator)
            .unwrap_or(blanco_core::FilterOperator::Equals)
    }
}

/// Store one setting in the app database in the background, logging a
/// failure. For choices remembered as a side effect of using the UI rather
/// than made on the settings page.
pub fn persist_setting(key: &'static str, value: String, cx: &mut App) {
    let db = app_database::AppDatabase::global(cx).clone();
    let save = gpui_tokio::Tokio::spawn_result(cx, async move {
        db.save_setting(key, &value, false)
            .await
            .map_err(anyhow::Error::from)
    });
    cx.spawn(async move |_| {
        if let Err(error) = save.await {
            tracing::error!("Failed to persist {key}: {error:#}");
        }
    })
    .detach();
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    /// Check GitHub hourly for a newer release and offer it in the title bar.
    pub check_for_updates: bool,
    /// Select the active tab's table in the sidebar, opening its parents.
    pub follow_active_tab: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            check_for_updates: true,
            follow_active_tab: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    pub word_wrap: bool,
    pub show_whitespace: bool,
    pub folding: bool,
    pub hard_tabs: bool,
    pub tab_size: u32,
    /// Font size in pixels of the code editors (query, script and snippet
    /// tabs).
    pub font_size: f32,
    /// The keys that accept the highlighted completion suggestion.
    #[serde(default)]
    pub completion_accept_keys: CompletionAcceptKeysSetting,
    /// Next and Previous Tab go around from the last tab to the first and
    /// back, rather than stopping at the ends.
    #[serde(default = "default_true")]
    pub tab_navigation_wraps: bool,
}

/// The keys that accept a completion suggestion, as stored in settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionAcceptKeysSetting {
    #[default]
    Enter,
    Tab,
    EnterAndTab,
}

impl CompletionAcceptKeysSetting {
    /// The value written to the settings store.
    pub fn key(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Tab => "tab",
            Self::EnterAndTab => "enter_and_tab",
        }
    }

    /// Reads a stored value, falling back to Enter for anything unknown.
    pub fn from_key(key: &str) -> Self {
        match key {
            "tab" => Self::Tab,
            "enter_and_tab" => Self::EnterAndTab,
            _ => Self::Enter,
        }
    }

    pub fn accept_keys(self) -> gpui_component::input::CompletionAcceptKeys {
        use gpui_component::input::CompletionAcceptKeys;
        match self {
            Self::Enter => CompletionAcceptKeys::Enter,
            Self::Tab => CompletionAcceptKeys::Tab,
            Self::EnterAndTab => CompletionAcceptKeys::EnterAndTab,
        }
    }
}

fn default_true() -> bool {
    true
}

impl EditorSettings {
    pub const DEFAULT_FONT_SIZE: f32 = 14.0;
    pub const MIN_FONT_SIZE: f32 = 8.0;
    pub const MAX_FONT_SIZE: f32 = 32.0;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    pub default_connection_timeout_seconds: u32,
    pub query_timeout_seconds: u32,
    /// Wall-clock budget for a script run in seconds, 0 for unlimited. Scripts
    /// are meant for long sequential work, so the default is unlimited.
    pub script_timeout_seconds: u32,
    pub show_connection_notifications: bool,
    /// Maximum number of query-history entries to retain. Older entries beyond
    /// this count are pruned after each execution.
    pub max_history_items: u32,
    /// Minutes without activity after which a connection is closed, 0 to keep
    /// connections open. A connection can override it.
    #[serde(default)]
    pub idle_disconnect_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: String,
    /// Switch between `light_theme` and `dark_theme` with the system
    /// appearance instead of showing `theme`.
    pub follow_system: bool,
    pub light_theme: String,
    pub dark_theme: String,
    pub font_family: String,
    pub mono_font_family: String,
    /// Terminal pane font size in pixels.
    pub terminal_font_size: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateDisplayFormat {
    #[default]
    Iso8601,
    Sql,
    DayMonthYear,
    MonthDayYear,
}

impl DateDisplayFormat {
    pub const ALL: [Self; 4] = [
        Self::Iso8601,
        Self::Sql,
        Self::DayMonthYear,
        Self::MonthDayYear,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Iso8601 => "iso8601",
            Self::Sql => "sql",
            Self::DayMonthYear => "day_month_year",
            Self::MonthDayYear => "month_day_year",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|format| format.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Iso8601 => "ISO 8601 (2024-01-31T14:05:09)",
            Self::Sql => "SQL (2024-01-31 14:05:09)",
            Self::DayMonthYear => "Day first (31/01/2024 14:05:09)",
            Self::MonthDayYear => "Month first (01/31/2024 2:05:09 PM)",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeZoneDisplay {
    #[default]
    Local,
    Utc,
}

impl TimeZoneDisplay {
    pub const ALL: [Self; 2] = [Self::Local, Self::Utc];

    pub fn key(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Utc => "utc",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|zone| zone.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "Local time",
            Self::Utc => "UTC",
        }
    }
}

/// Persisted window geometry, in logical pixels. `width`/`height` are `None`
/// until the window has been sized at least once, in which case the startup
/// code falls back to a centered default. Position is best-effort: some
/// platforms (e.g. Wayland) ignore client-requested window positions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WindowSettings {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub maximized: bool,
}

/// The built-in MCP server. Off by default: it grants whoever holds the token
/// access to every configured database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSettings {
    pub enabled: bool,
    /// Loopback port to listen on.
    pub port: u16,
    /// Run data-modifying statements without the confirmation dialog. PROD
    /// connections and DROP/TRUNCATE still confirm.
    pub allow_writes: bool,
}

impl McpSettings {
    pub const DEFAULT_PORT: u16 = 7821;
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: Self::DEFAULT_PORT,
            allow_writes: false,
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
            font_size: Self::DEFAULT_FONT_SIZE,
            completion_accept_keys: CompletionAcceptKeysSetting::default(),
            tab_navigation_wraps: true,
        }
    }
}

impl DatabaseSettings {
    /// The query timeout as a duration, `None` when it is disabled (0).
    pub fn query_timeout(&self) -> Option<std::time::Duration> {
        (self.query_timeout_seconds > 0)
            .then(|| std::time::Duration::from_secs(u64::from(self.query_timeout_seconds)))
    }

    /// The global idle timeout, `None` when it is off (0).
    pub fn idle_disconnect_timeout(&self) -> Option<std::time::Duration> {
        (self.idle_disconnect_minutes > 0)
            .then(|| std::time::Duration::from_secs(u64::from(self.idle_disconnect_minutes) * 60))
    }
}

/// The character between CSV values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CsvDelimiter {
    Comma,
    #[default]
    Semicolon,
    Tab,
    Pipe,
    /// The character in [`CsvSettings::custom_delimiter`].
    Custom,
}

impl CsvDelimiter {
    pub const ALL: [CsvDelimiter; 5] = [
        CsvDelimiter::Comma,
        CsvDelimiter::Semicolon,
        CsvDelimiter::Tab,
        CsvDelimiter::Pipe,
        CsvDelimiter::Custom,
    ];

    /// The stored setting value.
    pub fn as_str(self) -> &'static str {
        match self {
            CsvDelimiter::Comma => "comma",
            CsvDelimiter::Semicolon => "semicolon",
            CsvDelimiter::Tab => "tab",
            CsvDelimiter::Pipe => "pipe",
            CsvDelimiter::Custom => "custom",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CsvDelimiter::Comma => "Comma",
            CsvDelimiter::Semicolon => "Semicolon",
            CsvDelimiter::Tab => "Tab",
            CsvDelimiter::Pipe => "Pipe",
            CsvDelimiter::Custom => "Custom",
        }
    }

    /// Parse a stored value, falling back to the default for anything unknown.
    pub fn from_stored(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|delimiter| delimiter.as_str() == value)
            .unwrap_or_default()
    }

    /// The character, taking a custom one from the first character of
    /// `custom`. An empty custom delimiter falls back to the default.
    pub fn char(self, custom: &str) -> char {
        match self {
            CsvDelimiter::Comma => ',',
            CsvDelimiter::Semicolon => ';',
            CsvDelimiter::Tab => '\t',
            CsvDelimiter::Pipe => '|',
            CsvDelimiter::Custom => custom
                .chars()
                .next()
                .unwrap_or_else(|| CsvDelimiter::default().char("")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsvSettings {
    pub delimiter: CsvDelimiter,
    pub custom_delimiter: String,
    pub header: bool,
    /// Written for NULL values.
    pub null_text: String,
    /// The quote character, its first character counts.
    pub quote: String,
    pub quoting: CsvQuoting,
    pub replace_line_breaks: bool,
    /// Written in place of each line break inside a value when
    /// `replace_line_breaks` is on.
    pub line_break_replacement: String,
}

impl Default for CsvSettings {
    fn default() -> Self {
        let defaults = CsvOptions::default();
        Self {
            delimiter: CsvDelimiter::default(),
            custom_delimiter: String::new(),
            header: defaults.header,
            null_text: defaults.null_text,
            quote: defaults.quote.to_string(),
            quoting: defaults.quoting,
            replace_line_breaks: false,
            line_break_replacement: " ".to_string(),
        }
    }
}

impl CsvSettings {
    pub fn to_options(&self) -> CsvOptions {
        CsvOptions {
            delimiter: self.delimiter.char(&self.custom_delimiter),
            header: self.header,
            null_text: self.null_text.clone(),
            quote: self.quote.chars().next().unwrap_or('"'),
            quoting: self.quoting,
            line_break_replacement: self
                .replace_line_breaks
                .then(|| self.line_break_replacement.clone()),
        }
    }
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            default_connection_timeout_seconds: 30,
            query_timeout_seconds: 60,
            script_timeout_seconds: 0,
            show_connection_notifications: true,
            max_history_items: 1000,
            idle_disconnect_minutes: 0,
        }
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: "One Dark - Darkened".to_string(),
            follow_system: false,
            light_theme: "Catppuccin Latte".to_string(),
            dark_theme: "One Dark - Darkened".to_string(),
            font_family: String::new(),
            mono_font_family: String::new(),
            terminal_font_size: 12.0,
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

#[cfg(test)]
mod appearance_tests {
    use super::*;

    #[test]
    fn follow_system_picks_the_theme_for_the_appearance() {
        let mut appearance = AppearanceSettings::default();
        appearance.theme = "Chosen".into();
        assert_eq!(
            theme_for_appearance(&appearance, WindowAppearance::Light),
            "Chosen"
        );

        appearance.follow_system = true;
        assert_eq!(
            theme_for_appearance(&appearance, WindowAppearance::VibrantLight),
            "Catppuccin Latte"
        );
        assert_eq!(
            theme_for_appearance(&appearance, WindowAppearance::Dark),
            "One Dark - Darkened"
        );
    }

    #[gpui::test]
    fn user_fonts_survive_a_theme_registry_change(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut settings = Settings::default();
            settings.appearance.mono_font_family = "JetBrains Mono".into();
            cx.set_global(AppSettings::new(settings));
            keep_fonts_on_theme_reload(cx);

            gpui_component::ThemeRegistry::global_mut(cx)
                .load_themes_from_str(include_str!("../assets/themes/catppuccin.json"))
                .unwrap();
            assert!(install_theme("Catppuccin Latte", cx));
        });
        cx.run_until_parked();

        // What the themes directory watcher does on a reload.
        cx.update(|cx| {
            gpui_component::ThemeRegistry::global_mut(cx);
        });
        cx.run_until_parked();

        cx.update(|cx| {
            assert_eq!(Theme::global(cx).mono_font_family, "JetBrains Mono");
        });
    }

    #[test]
    fn test_completion_accept_keys_round_trip() {
        for setting in [
            CompletionAcceptKeysSetting::Enter,
            CompletionAcceptKeysSetting::Tab,
            CompletionAcceptKeysSetting::EnterAndTab,
        ] {
            let stored = vec![(
                "editor.completion_accept_keys".to_string(),
                setting.key().to_string(),
            )];
            assert_eq!(
                Settings::from_key_values(&stored)
                    .editor
                    .completion_accept_keys,
                setting
            );
        }
        assert_eq!(
            CompletionAcceptKeysSetting::from_key("bogus"),
            CompletionAcceptKeysSetting::Enter
        );
    }
}
