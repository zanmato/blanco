use crate::app_database::AppDatabase;
use crate::app_settings::AppSettings;
use crate::settings::Settings;
use gpui::{App, Context, FocusHandle, Focusable, IntoElement, Render, SharedString, Task, Window};
use gpui_component::ThemeRegistry;
use gpui_component::{
    Theme,
    group_box::GroupBoxVariant,
    setting::{
        NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings as GpuiSettings,
    },
};
use std::time::Duration;

/// Main settings view component
pub struct SettingsView {
    focus_handle: FocusHandle,
    save_tasks: std::collections::HashMap<String, gpui::Task<()>>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            save_tasks: std::collections::HashMap::new(),
        }
    }

    /// Save a setting with debouncing
    fn save_setting_debounced(
        &mut self,
        key: String,
        value: String,
        is_secret: bool,
        cx: &mut Context<Self>,
    ) {
        // Cancel any existing save task for this key
        self.save_tasks.remove(&key);

        let mut secret_task = Task::ready(Ok(()));
        if is_secret {
            secret_task = cx.write_credentials(
                format!("blanco://{key}").as_str(),
                key.as_str(),
                value.as_bytes(),
            );
        }

        let db = AppDatabase::global(cx).clone();
        let key_clone = key.clone();
        let task = cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            if let Err(e) = db.save_setting(&key_clone, &value, is_secret).await {
                tracing::error!("Failed to save setting {}: {}", key_clone, e);
            }

            if let Err(e) = secret_task.await {
                tracing::error!("Failed to write secret {}: {}", key_clone, e);
            }
        });

        self.save_tasks.insert(key, task);
    }

    /// Get the settings pages for the UI
    fn setting_pages(&mut self, cx: &mut Context<Self>) -> Vec<SettingPage> {
        let default_settings = Settings::default();

        // Capture a weak handle to self for the closures
        let view_handle = cx.entity().downgrade();

        let sorted_themes = ThemeRegistry::global(cx)
            .sorted_themes()
            .into_iter()
            .map(|v| (v.name.clone(), v.name.clone()))
            .collect();

        vec![
            // Editor Settings Page
            SettingPage::new("Editor").resettable(true).groups(vec![
                SettingGroup::new().title("Text").items(vec![
                    SettingItem::new(
                        "Word Wrap",
                        SettingField::switch(
                            move |cx: &App| AppSettings::global(cx).settings.editor.word_wrap,
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).settings.editor.word_wrap = val;

                                    // Save with debouncing
                                    let key = "editor.word_wrap".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(default_settings.editor.word_wrap),
                    )
                    .description("Enable word wrapping in the SQL editor."),
                    SettingItem::new(
                        "Render Whitespace",
                        SettingField::switch(
                            move |cx: &App| AppSettings::global(cx).settings.editor.show_whitespace,
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).settings.editor.show_whitespace =
                                        val;

                                    // Save with debouncing
                                    let key = "editor.show_whitespace".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(default_settings.editor.show_whitespace),
                    )
                    .description("Show whitespace characters in the SQL editor."),
                    SettingItem::new(
                        "Folding",
                        SettingField::switch(
                            move |cx: &App| AppSettings::global(cx).settings.editor.folding,
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).settings.editor.folding = val;

                                    let key = "editor.folding".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(default_settings.editor.folding),
                    )
                    .description("Enable code folding in the SQL editor."),
                    SettingItem::new(
                        "Hard Tabs",
                        SettingField::switch(
                            move |cx: &App| AppSettings::global(cx).settings.editor.hard_tabs,
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).settings.editor.hard_tabs = val;

                                    let key = "editor.hard_tabs".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(default_settings.editor.hard_tabs),
                    )
                    .description("Use tab characters instead of spaces for indentation."),
                    SettingItem::new(
                        "Tab Size",
                        SettingField::number_input(
                            NumberFieldOptions {
                                min: 1.0,
                                max: 8.0,
                                step: 1.0,
                            },
                            move |cx: &App| AppSettings::global(cx).settings.editor.tab_size as f64,
                            {
                                let view_handle = view_handle.clone();
                                move |val: f64, cx: &mut App| {
                                    AppSettings::global_mut(cx).settings.editor.tab_size =
                                        val as u32;

                                    let key = "editor.tab_size".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(default_settings.editor.tab_size as f64),
                    )
                    .description("Number of spaces per tab stop (1-8)."),
                ]),
            ]),
            // Database Settings Page
            SettingPage::new("Database").resettable(true).groups(vec![
                SettingGroup::new().title("Connection").items(vec![
                    SettingItem::new(
                        "Default Connection Timeout",
                        SettingField::number_input(
                            NumberFieldOptions {
                                min: 5.0,
                                max: 300.0,
                                step: 5.0,
                            },
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .database
                                    .default_connection_timeout_seconds
                                    as f64
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: f64, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .database
                                        .default_connection_timeout_seconds = val as u32;

                                    // Save with debouncing
                                    let key =
                                        "database.default_connection_timeout_seconds".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(
                            default_settings.database.default_connection_timeout_seconds as f64,
                        ),
                    )
                    .description(
                        "Default timeout in seconds for database connections (5-300 seconds).",
                    ),
                    SettingItem::new(
                        "Query Timeout",
                        SettingField::number_input(
                            NumberFieldOptions {
                                min: 10.0,
                                max: 600.0,
                                step: 10.0,
                            },
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .database
                                    .query_timeout_seconds as f64
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: f64, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .database
                                        .query_timeout_seconds = val as u32;

                                    // Save with debouncing
                                    let key = "database.query_timeout_seconds".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(default_settings.database.query_timeout_seconds as f64),
                    )
                    .description("Timeout in seconds for SQL query execution (10-600 seconds)."),
                    SettingItem::new(
                        "Show Connection Notifications",
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .database
                                    .show_connection_notifications
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .database
                                        .show_connection_notifications = val;

                                    // Save with debouncing
                                    let key = "database.show_connection_notifications".to_string();
                                    let value = val.to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, value, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(default_settings.database.show_connection_notifications),
                    )
                    .description(
                        "Show notifications when connecting to or disconnecting from databases.",
                    ),
                ]),
            ]),
            // Appearance Settings Page
            SettingPage::new("Appearance").resettable(true).groups(vec![
                SettingGroup::new().title("Theme").items(vec![
                    SettingItem::new(
                        "Theme",
                        SettingField::dropdown(
                            sorted_themes,
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx).settings.appearance.theme.clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let theme_name = val.to_string();
                                    AppSettings::global_mut(cx).settings.appearance.theme =
                                        theme_name.clone();

                                    if let Some(theme_config) =
                                        ThemeRegistry::global(cx).themes().get(&val).cloned()
                                    {
                                        Theme::global_mut(cx).apply_config(&theme_config);
                                    }

                                    // Save with debouncing
                                    let key = "appearance.theme".to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, theme_name, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(
                            default_settings.appearance.theme.clone(),
                        )),
                    )
                    .description("Choose the color theme for the application."),
                ]),
            ]),
            // Chat Settings Page
            SettingPage::new("Chat").resettable(true).groups(vec![
                SettingGroup::new().title("Provider").items(vec![
                    SettingItem::new(
                        "Provider",
                        SettingField::dropdown(
                            vec![("openai".into(), "OpenAI Compatible".into())],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx).settings.chat.provider.clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let provider = val.to_string();
                                    AppSettings::global_mut(cx).settings.chat.provider =
                                        provider.clone();

                                    // Save with debouncing
                                    let key = "chat.provider".to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, provider, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(default_settings.chat.provider.clone())),
                    )
                    .description("Select the AI provider for chat features."),
                    SettingItem::new(
                        "Model",
                        SettingField::input(
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx).settings.chat.model.clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let model = val.to_string();
                                    AppSettings::global_mut(cx).settings.chat.model = model.clone();

                                    // Save with debouncing
                                    let key = "chat.model".to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, model, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(default_settings.chat.model.clone())),
                    )
                    .description("The AI model to use (e.g., gpt-4)."),
                    SettingItem::new(
                        "API Key",
                        SettingField::input(
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx).settings.chat.api_key.clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let api_key = val.to_string();
                                    AppSettings::global_mut(cx).settings.chat.api_key =
                                        api_key.clone();

                                    let key = "chat.api_key".to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, api_key, true, cx);
                                        });
                                    }
                                }
                            },
                        ),
                    )
                    .description(
                        "Your API key for the selected provider. This is stored securely.",
                    ),
                    SettingItem::new(
                        "Base URL",
                        SettingField::input(
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx).settings.chat.base_url.clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let base_url = val.to_string();
                                    AppSettings::global_mut(cx).settings.chat.base_url =
                                        base_url.clone();

                                    // Save with debouncing
                                    let key = "chat.base_url".to_string();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(key, base_url, false, cx);
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(default_settings.chat.base_url.clone())),
                    )
                    .description("Base URL for the API."),
                ]),
            ]),
        ]
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        GpuiSettings::new("blanco-settings")
            .with_group_variant(GroupBoxVariant::Outline)
            .pages(self.setting_pages(cx))
    }
}
