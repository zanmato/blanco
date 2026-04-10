use crate::app_database::AppDatabase;
use crate::app_settings::AppSettings;
use crate::settings::formatter_page::formatter_page;
use crate::settings::Settings;
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Styled, Subscription, Task, Window, px, rems,
};
use gpui_component::ThemeRegistry;
use gpui_component::{
    ActiveTheme, Sizable, Theme,
    group_box::GroupBoxVariant,
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    setting::{
        NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings as GpuiSettings,
    },
};
use std::time::Duration;

#[derive(Debug, Clone)]
struct FontOption {
    value: String,
    label: String,
}

impl SelectItem for FontOption {
    type Value = String;

    fn title(&self) -> SharedString {
        SharedString::from(self.label.clone())
    }

    fn value(&self) -> &String {
        &self.value
    }

    fn matches(&self, query: &str) -> bool {
        self.label.to_lowercase().contains(&query.to_lowercase())
    }
}

/// Main settings view component
pub struct SettingsView {
    focus_handle: FocusHandle,
    save_tasks: std::collections::HashMap<String, gpui::Task<()>>,
    ui_font_select: Option<Entity<SelectState<SearchableVec<FontOption>>>>,
    mono_font_select: Option<Entity<SelectState<SearchableVec<FontOption>>>>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            save_tasks: std::collections::HashMap::new(),
            ui_font_select: None,
            mono_font_select: None,
            _subscriptions: Vec::new(),
        }
    }

    fn ensure_font_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ui_font_select.is_some() {
            return;
        }

        let font_names = cx.text_system().all_font_names();

        let build_options = |current_value: &str| -> (SearchableVec<FontOption>, Option<usize>) {
            let options: Vec<FontOption> = font_names
                .iter()
                .map(|name| FontOption {
                    value: name.clone(),
                    label: name.clone(),
                })
                .collect();

            let selected_index = if current_value.is_empty() {
                None
            } else {
                options.iter().position(|o| o.value == current_value)
            };

            (SearchableVec::new(options), selected_index)
        };

        let settings = &AppSettings::global(cx).settings.appearance;
        let (ui_items, ui_index) = build_options(&settings.font_family);
        let (mono_items, mono_index) = build_options(&settings.mono_font_family);

        let ui_font_select = cx.new(|cx| {
            SelectState::new(
                ui_items,
                ui_index.map(|i| gpui_component::IndexPath::default().row(i)),
                window,
                cx,
            )
            .searchable(true)
        });

        let mono_font_select = cx.new(|cx| {
            SelectState::new(
                mono_items,
                mono_index.map(|i| gpui_component::IndexPath::default().row(i)),
                window,
                cx,
            )
            .searchable(true)
        });

        let sub1 = cx.subscribe_in(&ui_font_select, window, Self::on_ui_font_selected);
        let sub2 = cx.subscribe_in(&mono_font_select, window, Self::on_mono_font_selected);

        self.ui_font_select = Some(ui_font_select);
        self.mono_font_select = Some(mono_font_select);
        self._subscriptions.push(sub1);
        self._subscriptions.push(sub2);
    }

    fn on_ui_font_selected(
        &mut self,
        _: &Entity<SelectState<SearchableVec<FontOption>>>,
        event: &SelectEvent<SearchableVec<FontOption>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(value) = event;
        let font_name = value.clone().unwrap_or_default();
        AppSettings::global_mut(cx).settings.appearance.font_family = font_name.clone();
        crate::settings::apply_font_settings(cx);
        self.save_setting_debounced(
            "appearance.font_family".to_string(),
            font_name,
            false,
            cx,
        );
    }

    fn on_mono_font_selected(
        &mut self,
        _: &Entity<SelectState<SearchableVec<FontOption>>>,
        event: &SelectEvent<SearchableVec<FontOption>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(value) = event;
        let font_name = value.clone().unwrap_or_default();
        AppSettings::global_mut(cx).settings.appearance.mono_font_family = font_name.clone();
        crate::settings::apply_font_settings(cx);
        self.save_setting_debounced(
            "appearance.mono_font_family".to_string(),
            font_name,
            false,
            cx,
        );
    }

    /// Save a setting with debouncing
    pub(super) fn save_setting_debounced(
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

        let ui_font_select = self.ui_font_select.clone();
        let mono_font_select = self.mono_font_select.clone();

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
            formatter_page(view_handle.clone(), &default_settings),
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
                                        crate::settings::apply_font_settings(cx);
                                    }

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
                SettingGroup::new().title("Fonts").items(vec![
                    SettingItem::new(
                        "UI Font",
                        SettingField::render({
                            let ui_font_select = ui_font_select.clone();
                            move |options, _window, cx| {
                                let theme_font = cx.theme().font_family.to_string();
                                if let Some(state) = &ui_font_select {
                                    Select::new(state)
                                        .with_size(options.size)
                                        .placeholder(format!("{theme_font} (theme default)"))
                                        .search_placeholder("Search fonts...")
                                        .cleanable(true)
                                        .menu_max_h(rems(16.))
                                        .w(px(240.))
                                        .into_any_element()
                                } else {
                                    gpui::div().into_any_element()
                                }
                            }
                        }),
                    )
                    .description("Font used for the application UI."),
                    SettingItem::new(
                        "Editor Font",
                        SettingField::render({
                            let mono_font_select = mono_font_select.clone();
                            move |options, _window, cx| {
                                let theme_font = cx.theme().mono_font_family.to_string();
                                if let Some(state) = &mono_font_select {
                                    Select::new(state)
                                        .with_size(options.size)
                                        .placeholder(format!("{theme_font} (theme default)"))
                                        .search_placeholder("Search fonts...")
                                        .cleanable(true)
                                        .menu_max_h(rems(16.))
                                        .w(px(240.))
                                        .into_any_element()
                                } else {
                                    gpui::div().into_any_element()
                                }
                            }
                        }),
                    )
                    .description(
                        "Monospace font used in the SQL editor and results table.",
                    ),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_font_selects(window, cx);

        GpuiSettings::new("blanco-settings")
            .with_group_variant(GroupBoxVariant::Outline)
            .pages(self.setting_pages(cx))
    }
}
