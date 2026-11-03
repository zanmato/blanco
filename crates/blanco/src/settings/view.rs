use crate::app_settings::AppSettings;
use crate::mcp::{McpService, McpStatus};
use crate::settings::csv_group::csv_group;
use crate::settings::formatter_page::formatter_page;
use crate::settings::keybindings_page::keybindings_page;
use crate::settings::{
    CompletionAcceptKeysSetting, DateDisplayFormat, EditorSettings, Settings, TimeZoneDisplay,
};
use app_database::AppDatabase;
use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px, rems,
};
use gpui_component::ThemeRegistry;
use gpui_component::{
    ActiveTheme, Sizable,
    button::Button,
    group_box::GroupBoxVariant,
    h_flex,
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    setting::{
        NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings as GpuiSettings,
    },
    v_flex,
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

/// One path field per client tool used for backup and restore.
fn client_tool_items(view_handle: &gpui::WeakEntity<SettingsView>) -> Vec<SettingItem> {
    use crate::backup::tools::ToolProgram;
    [
        (ToolProgram::PgDump, "client_tools.pg_dump_path"),
        (ToolProgram::PgRestore, "client_tools.pg_restore_path"),
        (ToolProgram::Psql, "client_tools.psql_path"),
        (ToolProgram::MysqlDump, "client_tools.mysqldump_path"),
        (ToolProgram::Mysql, "client_tools.mysql_path"),
    ]
    .into_iter()
    .map(|(program, key)| {
        let name = program.display_name();
        SettingItem::new(
            name,
            SettingField::input(
                move |cx: &App| {
                    SharedString::from(
                        program
                            .configured_path(&AppSettings::global(cx).settings.client_tools)
                            .to_string(),
                    )
                },
                {
                    let view_handle = view_handle.clone();
                    move |value: SharedString, cx: &mut App| {
                        let path = value.trim().to_string();
                        *program.configured_path_mut(
                            &mut AppSettings::global_mut(cx).settings.client_tools,
                        ) = path.clone();
                        if let Some(view) = view_handle.upgrade() {
                            view.update(cx, |view, cx| {
                                view.save_setting_debounced(key.to_string(), path, false, cx);
                            });
                        }
                    }
                },
            )
            .default_value(SharedString::default()),
        )
        .description(t!("settings.database.client_tool_description", name = name).into_owned())
    })
    .collect()
}

/// The language setting's choices: following the system, which names the
/// language that gives, then every translation by its own name.
fn language_options() -> Vec<(SharedString, SharedString)> {
    let system_language = crate::i18n::language_name(&crate::i18n::system_locale());
    let system = t!(
        "settings.general.language_system",
        language = system_language
    );
    std::iter::once((SharedString::default(), system.into()))
        .chain(crate::i18n::available_locales().into_iter().map(|locale| {
            let name = crate::i18n::language_name(&locale);
            (locale.into(), name.into())
        }))
        .collect()
}

/// Main settings view component
pub struct SettingsView {
    focus_handle: FocusHandle,
    save_tasks: std::collections::HashMap<String, gpui::Task<()>>,
    ui_font_select: Option<Entity<SelectState<SearchableVec<FontOption>>>>,
    mono_font_select: Option<Entity<SelectState<SearchableVec<FontOption>>>>,
    environment_tags_editor: Option<Entity<super::environments_page::EnvironmentTagsEditor>>,
    theme_browser: Option<Entity<vsx_themes::ThemeBrowser>>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            save_tasks: std::collections::HashMap::new(),
            ui_font_select: None,
            mono_font_select: None,
            environment_tags_editor: None,
            theme_browser: None,
            // Re-render when the MCP server starts, stops or fails so the
            // status row stays current.
            _subscriptions: vec![
                cx.observe_global::<McpService>(|_, cx| cx.notify()),
                // These keep translated text from when they were made, so
                // render makes them again in the new language.
                cx.observe_global::<crate::i18n::Locale>(|this, cx| {
                    this.environment_tags_editor = None;
                    this.theme_browser = None;
                    cx.notify();
                }),
            ],
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
        self.save_setting_debounced("appearance.font_family".to_string(), font_name, false, cx);
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
        AppSettings::global_mut(cx)
            .settings
            .appearance
            .mono_font_family = font_name.clone();
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

        let themes_of = |mode: Option<gpui_component::ThemeMode>, cx: &App| {
            ThemeRegistry::global(cx)
                .sorted_themes()
                .into_iter()
                .filter(|theme| mode.is_none_or(|mode| theme.mode == mode))
                .map(|theme| (theme.name.clone(), theme.name.clone()))
                .collect::<Vec<(SharedString, SharedString)>>()
        };
        let sorted_themes = themes_of(None, cx);
        let light_themes = themes_of(Some(gpui_component::ThemeMode::Light), cx);
        let dark_themes = themes_of(Some(gpui_component::ThemeMode::Dark), cx);

        let ui_font_select = self.ui_font_select.clone();
        let mono_font_select = self.mono_font_select.clone();

        vec![
            SettingPage::new(t!("settings.general.page"))
                .resettable(true)
                .groups(vec![
                    SettingGroup::new()
                        .title(t!("settings.general.language_group"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.general.language"),
                                SettingField::dropdown(
                                    language_options(),
                                    |cx: &App| {
                                        SharedString::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .general
                                                .language
                                                .clone()
                                                .unwrap_or_default(),
                                        )
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: SharedString, cx: &mut App| {
                                            let language =
                                                (!val.is_empty()).then(|| val.to_string());
                                            crate::i18n::change(language.as_deref(), cx);
                                            AppSettings::global_mut(cx).settings.general.language =
                                                language;
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        "general.language".to_string(),
                                                        val.to_string(),
                                                        false,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(SharedString::default()),
                            )
                            .description(t!("settings.general.language_description").into_owned()),
                        ]),
                    SettingGroup::new()
                        .title(t!("settings.general.updates"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.general.check_for_updates"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.general.check_for_updates
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .general
                                                .check_for_updates = val;

                                            let key = "general.check_for_updates".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.general.check_for_updates),
                            )
                            .description(
                                t!("settings.general.check_for_updates_description").into_owned(),
                            ),
                        ]),
                    SettingGroup::new()
                        .title(t!("settings.general.sidebar"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.general.follow_active_tab"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.general.follow_active_tab
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .general
                                                .follow_active_tab = val;

                                            let key = "general.follow_active_tab".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.general.follow_active_tab),
                            )
                            .description(
                                t!("settings.general.follow_active_tab_description").into_owned(),
                            ),
                        ]),
                ]),
            // Editor Settings Page
            SettingPage::new(t!("settings.editor.page"))
                .resettable(true)
                .groups(vec![
                    SettingGroup::new()
                        .title(t!("settings.editor.text"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.editor.word_wrap"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.editor.word_wrap
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx).settings.editor.word_wrap =
                                                val;

                                            let key = "editor.word_wrap".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.word_wrap),
                            )
                            .description(t!("settings.editor.word_wrap_description").into_owned()),
                            SettingItem::new(
                                t!("settings.editor.render_whitespace"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.editor.show_whitespace
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .editor
                                                .show_whitespace = val;

                                            let key = "editor.show_whitespace".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.show_whitespace),
                            )
                            .description(
                                t!("settings.editor.render_whitespace_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.editor.highlight_invisible_characters"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx)
                                            .settings
                                            .editor
                                            .highlight_invisible_characters
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .editor
                                                .highlight_invisible_characters = val;

                                            let key =
                                                "editor.highlight_invisible_characters".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(
                                    default_settings.editor.highlight_invisible_characters,
                                ),
                            )
                            .description(
                                t!("settings.editor.highlight_invisible_characters_description")
                                    .into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.editor.colorize_brackets"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.editor.colorize_brackets
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .editor
                                                .colorize_brackets = val;

                                            let key = "editor.colorize_brackets".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.colorize_brackets),
                            )
                            .description(
                                t!("settings.editor.colorize_brackets_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.editor.folding"),
                                SettingField::switch(
                                    move |cx: &App| AppSettings::global(cx).settings.editor.folding,
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx).settings.editor.folding =
                                                val;

                                            let key = "editor.folding".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.folding),
                            )
                            .description(t!("settings.editor.folding_description").into_owned()),
                            SettingItem::new(
                                t!("settings.editor.hard_tabs"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.editor.hard_tabs
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx).settings.editor.hard_tabs =
                                                val;

                                            let key = "editor.hard_tabs".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.hard_tabs),
                            )
                            .description(t!("settings.editor.hard_tabs_description").into_owned()),
                            SettingItem::new(
                                t!("settings.editor.tab_size"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 1.0,
                                        max: 8.0,
                                        step: 1.0,
                                    },
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.editor.tab_size as f64
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            AppSettings::global_mut(cx).settings.editor.tab_size =
                                                val as u32;

                                            let key = "editor.tab_size".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.tab_size as f64),
                            )
                            .description(t!("settings.editor.tab_size_description").into_owned()),
                            SettingItem::new(
                                t!("settings.editor.accept_suggestion_with"),
                                SettingField::dropdown(
                                    vec![
                                        ("enter".into(), "Enter".into()),
                                        ("tab".into(), "Tab".into()),
                                        (
                                            "enter_and_tab".into(),
                                            t!("settings.editor.accept_enter_or_tab").into(),
                                        ),
                                    ],
                                    move |cx: &App| {
                                        SharedString::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .editor
                                                .completion_accept_keys
                                                .key(),
                                        )
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: SharedString, cx: &mut App| {
                                            let keys = CompletionAcceptKeysSetting::from_key(&val);
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .editor
                                                .completion_accept_keys = keys;

                                            let key = "editor.completion_accept_keys".to_string();
                                            let value = keys.key().to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(SharedString::from(
                                    default_settings.editor.completion_accept_keys.key(),
                                )),
                            )
                            .description(
                                t!("settings.editor.accept_suggestion_with_description")
                                    .into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.editor.wrap_tab_navigation"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.editor.tab_navigation_wraps
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .editor
                                                .tab_navigation_wraps = val;

                                            let key = "editor.tab_navigation_wraps".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.tab_navigation_wraps),
                            )
                            .description(
                                t!("settings.editor.wrap_tab_navigation_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.editor.font_size"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: EditorSettings::MIN_FONT_SIZE as f64,
                                        max: EditorSettings::MAX_FONT_SIZE as f64,
                                        step: 1.0,
                                    },
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.editor.font_size as f64
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            crate::settings::set_editor_font_size(val as f32, cx);

                                            let key = "editor.font_size".to_string();
                                            let value = AppSettings::global(cx)
                                                .settings
                                                .editor
                                                .font_size
                                                .to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.editor.font_size as f64),
                            )
                            .description(t!("settings.editor.font_size_description").into_owned()),
                        ]),
                ]),
            formatter_page(view_handle.clone(), &default_settings),
            keybindings_page(view_handle.clone()),
            // Database Settings Page
            SettingPage::new(t!("settings.database.page"))
                .resettable(true)
                .groups(vec![
                    SettingGroup::new()
                        .title(t!("settings.database.connection"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.database.default_connection_timeout"),
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
                                            blanco_core::set_connect_timeout_secs(val as u64);

                                            let key = "database.default_connection_timeout_seconds"
                                                .to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(
                                    default_settings.database.default_connection_timeout_seconds
                                        as f64,
                                ),
                            )
                            .description(
                                t!("settings.database.default_connection_timeout_description")
                                    .into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.database.query_timeout"),
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
                                            .query_timeout_seconds
                                            as f64
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
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(
                                    default_settings.database.query_timeout_seconds as f64,
                                ),
                            )
                            .description(
                                t!("settings.database.query_timeout_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.database.script_timeout"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 0.0,
                                        max: 86400.0,
                                        step: 30.0,
                                    },
                                    move |cx: &App| {
                                        AppSettings::global(cx)
                                            .settings
                                            .database
                                            .script_timeout_seconds
                                            as f64
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .database
                                                .script_timeout_seconds = val as u32;

                                            let key = "database.script_timeout_seconds".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(
                                    default_settings.database.script_timeout_seconds as f64,
                                ),
                            )
                            .description(
                                t!("settings.database.script_timeout_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.database.disconnect_when_idle"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 0.0,
                                        max: 1440.0,
                                        step: 5.0,
                                    },
                                    move |cx: &App| {
                                        AppSettings::global(cx)
                                            .settings
                                            .database
                                            .idle_disconnect_minutes
                                            as f64
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .database
                                                .idle_disconnect_minutes = val as u32;

                                            let key =
                                                "database.idle_disconnect_minutes".to_string();
                                            let value = (val as u32).to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(
                                    default_settings.database.idle_disconnect_minutes as f64,
                                ),
                            )
                            .description(
                                t!("settings.database.disconnect_when_idle_description")
                                    .into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.database.show_connection_notifications"),
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

                                            let key = "database.show_connection_notifications"
                                                .to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(
                                    default_settings.database.show_connection_notifications,
                                ),
                            )
                            .description(
                                t!("settings.database.show_connection_notifications_description")
                                    .into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.database.max_history_items"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 0.0,
                                        max: 100_000.0,
                                        step: 100.0,
                                    },
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.database.max_history_items
                                            as f64
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .database
                                                .max_history_items = val as u32;

                                            let key = "database.max_history_items".to_string();
                                            let value = (val as u32).to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.database.max_history_items as f64),
                            )
                            .description(
                                t!("settings.database.max_history_items_description").into_owned(),
                            ),
                        ]),
                    csv_group(view_handle.clone(), &default_settings),
                    SettingGroup::new()
                        .title(t!("settings.results.title"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.database.show_row_numbers"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.results.show_row_numbers
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .results
                                                .show_row_numbers = val;

                                            let key = "results.show_row_numbers".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.results.show_row_numbers),
                            )
                            .description(
                                t!("settings.database.show_row_numbers_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.database.show_invisible_characters"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx)
                                            .settings
                                            .results
                                            .show_invisible_characters
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .results
                                                .show_invisible_characters = val;

                                            let key =
                                                "results.show_invisible_characters".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.results.show_invisible_characters),
                            )
                            .description(
                                t!("settings.database.show_invisible_characters_description")
                                    .into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.database.defer_values"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 0.0,
                                        max: 1_048_576.0,
                                        step: 16.0,
                                    },
                                    move |cx: &App| {
                                        f64::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .results
                                                .defer_values_over_kb,
                                        )
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .results
                                                .defer_values_over_kb = val as u32;

                                            let key = "results.defer_values_over_kb".to_string();
                                            let value = (val as u32).to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(f64::from(
                                    default_settings.results.defer_values_over_kb,
                                )),
                            )
                            .description(
                                t!("settings.database.defer_values_description").into_owned(),
                            ),
                        ]),
                    SettingGroup::new()
                        .title(t!("settings.database.backup_tools"))
                        .items(client_tool_items(&view_handle)),
                ]),
            SettingPage::new(t!("settings.results.title"))
                .resettable(true)
                .groups(vec![
                    SettingGroup::new()
                        .title(t!("settings.results.dates_and_times"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.results.date_format"),
                                SettingField::dropdown(
                                    DateDisplayFormat::ALL
                                        .into_iter()
                                        .map(|format| (format.key().into(), format.label()))
                                        .collect(),
                                    move |cx: &App| {
                                        SharedString::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .results
                                                .date_format
                                                .key(),
                                        )
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: SharedString, cx: &mut App| {
                                            let Some(format) = DateDisplayFormat::from_key(&val)
                                            else {
                                                return;
                                            };
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .results
                                                .date_format = format;
                                            cx.refresh_windows();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        "results.date_format".into(),
                                                        format.key().into(),
                                                        false,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(SharedString::from(
                                    default_settings.results.date_format.key(),
                                )),
                            )
                            .description(
                                t!("settings.results.date_format_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.results.time_zone"),
                                SettingField::dropdown(
                                    TimeZoneDisplay::ALL
                                        .into_iter()
                                        .map(|zone| (zone.key().into(), zone.label()))
                                        .collect(),
                                    move |cx: &App| {
                                        SharedString::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .results
                                                .time_zone
                                                .key(),
                                        )
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: SharedString, cx: &mut App| {
                                            let Some(zone) = TimeZoneDisplay::from_key(&val) else {
                                                return;
                                            };
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .results
                                                .time_zone = zone;
                                            cx.refresh_windows();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        "results.time_zone".into(),
                                                        zone.key().into(),
                                                        false,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(SharedString::from(
                                    default_settings.results.time_zone.key(),
                                )),
                            )
                            .description(t!("settings.results.time_zone_description").into_owned()),
                        ]),
                ]),
            super::environments_page::environments_page(
                self.environment_tags_editor
                    .clone()
                    .expect("created before the pages"),
            ),
            // Appearance Settings Page
            SettingPage::new(t!("settings.appearance.page"))
                .resettable(true)
                .groups(vec![
                    SettingGroup::new()
                        .title(t!("settings.appearance.theme"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.appearance.theme"),
                                SettingField::dropdown(
                                    sorted_themes,
                                    move |cx: &App| {
                                        SharedString::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .appearance
                                                .theme
                                                .clone(),
                                        )
                                    },
                                    |val: SharedString, cx: &mut App| {
                                        crate::settings::apply_theme_by_name(&val, cx);
                                    },
                                )
                                .default_value(SharedString::from(
                                    default_settings.appearance.theme.clone(),
                                )),
                            )
                            .description(t!("settings.appearance.theme_description").into_owned()),
                            SettingItem::new(
                                t!("settings.appearance.follow_system"),
                                SettingField::switch(
                                    |cx: &App| {
                                        AppSettings::global(cx).settings.appearance.follow_system
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .appearance
                                                .follow_system = val;
                                            let system = cx.window_appearance();
                                            let appearance =
                                                AppSettings::global(cx).settings.appearance.clone();
                                            crate::settings::install_theme(
                                                crate::settings::theme_for_appearance(
                                                    &appearance,
                                                    system,
                                                ),
                                                cx,
                                            );
                                            save_appearance(
                                                &view_handle,
                                                "follow_system",
                                                val.to_string(),
                                                cx,
                                            );
                                        }
                                    },
                                )
                                .default_value(default_settings.appearance.follow_system),
                            )
                            .description(
                                t!("settings.appearance.follow_system_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.appearance.light_theme"),
                                SettingField::dropdown(
                                    light_themes,
                                    |cx: &App| {
                                        SharedString::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .appearance
                                                .light_theme
                                                .clone(),
                                        )
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: SharedString, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .appearance
                                                .light_theme = val.to_string();
                                            crate::settings::follow_system_appearance(
                                                cx.window_appearance(),
                                                cx,
                                            );
                                            save_appearance(
                                                &view_handle,
                                                "light_theme",
                                                val.to_string(),
                                                cx,
                                            );
                                        }
                                    },
                                )
                                .default_value(SharedString::from(
                                    default_settings.appearance.light_theme.clone(),
                                )),
                            ),
                            SettingItem::new(
                                t!("settings.appearance.dark_theme"),
                                SettingField::dropdown(
                                    dark_themes,
                                    |cx: &App| {
                                        SharedString::from(
                                            AppSettings::global(cx)
                                                .settings
                                                .appearance
                                                .dark_theme
                                                .clone(),
                                        )
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: SharedString, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .appearance
                                                .dark_theme = val.to_string();
                                            crate::settings::follow_system_appearance(
                                                cx.window_appearance(),
                                                cx,
                                            );
                                            save_appearance(
                                                &view_handle,
                                                "dark_theme",
                                                val.to_string(),
                                                cx,
                                            );
                                        }
                                    },
                                )
                                .default_value(SharedString::from(
                                    default_settings.appearance.dark_theme.clone(),
                                )),
                            ),
                        ]),
                    SettingGroup::new()
                        .title(t!("settings.appearance.fonts"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.appearance.ui_font"),
                                SettingField::render({
                                    let ui_font_select = ui_font_select;
                                    move |options, _window, cx| {
                                        let theme_font = cx.theme().font_family.to_string();
                                        if let Some(state) = &ui_font_select {
                                            Select::new(state)
                                                .with_size(options.size())
                                                .placeholder(t!(
                                                    "settings.appearance.theme_default_font",
                                                    font = theme_font
                                                ))
                                                .search_placeholder(t!(
                                                    "settings.appearance.search_fonts"
                                                ))
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
                                t!("settings.appearance.ui_font_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.appearance.editor_font"),
                                SettingField::render({
                                    let mono_font_select = mono_font_select;
                                    move |options, _window, cx| {
                                        let theme_font = cx.theme().mono_font_family.to_string();
                                        if let Some(state) = &mono_font_select {
                                            Select::new(state)
                                                .with_size(options.size())
                                                .placeholder(t!(
                                                    "settings.appearance.theme_default_font",
                                                    font = theme_font
                                                ))
                                                .search_placeholder(t!(
                                                    "settings.appearance.search_fonts"
                                                ))
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
                                t!("settings.appearance.editor_font_description").into_owned(),
                            ),
                            SettingItem::new(
                                t!("settings.appearance.terminal_font_size"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 8.0,
                                        max: 32.0,
                                        step: 1.0,
                                    },
                                    move |cx: &App| {
                                        AppSettings::global(cx)
                                            .settings
                                            .appearance
                                            .terminal_font_size
                                            as f64
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            AppSettings::global_mut(cx)
                                                .settings
                                                .appearance
                                                .terminal_font_size = val as f32;

                                            let key = "appearance.terminal_font_size".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(
                                    default_settings.appearance.terminal_font_size as f64,
                                ),
                            )
                            .description(
                                t!("settings.appearance.terminal_font_size_description")
                                    .into_owned(),
                            ),
                        ]),
                    vsx_themes::browser::setting_group(
                        self.theme_browser
                            .clone()
                            .expect("created before the pages"),
                    ),
                ]),
            // MCP Server Settings Page
            SettingPage::new(t!("settings.mcp.page"))
                .resettable(true)
                .groups(vec![
                    SettingGroup::new()
                        .title(t!("settings.mcp.server"))
                        .items(vec![
                            SettingItem::new(
                                t!("settings.mcp.enable"),
                                SettingField::switch(
                                    move |cx: &App| AppSettings::global(cx).settings.mcp.enabled,
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx).settings.mcp.enabled = val;
                                            if val {
                                                McpService::start(cx);
                                            } else {
                                                McpService::stop(cx);
                                            }

                                            let key = "mcp.enabled".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.mcp.enabled),
                            )
                            .description(t!("settings.mcp.enable_description").into_owned()),
                            SettingItem::new(
                                t!("settings.mcp.port"),
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 1024.0,
                                        max: 65535.0,
                                        step: 1.0,
                                    },
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.mcp.port as f64
                                    },
                                    {
                                        let view_handle = view_handle.clone();
                                        move |val: f64, cx: &mut App| {
                                            let port = val.clamp(1024.0, 65535.0) as u16;
                                            AppSettings::global_mut(cx).settings.mcp.port = port;
                                            if McpService::global(cx).is_running() {
                                                McpService::restart(cx);
                                            }

                                            let key = "mcp.port".to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key,
                                                        port.to_string(),
                                                        false,
                                                        cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.mcp.port as f64),
                            )
                            .description(t!("settings.mcp.port_description").into_owned()),
                            SettingItem::new(
                                t!("settings.mcp.allow_writes"),
                                SettingField::switch(
                                    move |cx: &App| {
                                        AppSettings::global(cx).settings.mcp.allow_writes
                                    },
                                    {
                                        let view_handle = view_handle;
                                        move |val: bool, cx: &mut App| {
                                            AppSettings::global_mut(cx).settings.mcp.allow_writes =
                                                val;
                                            McpService::set_allow_writes(cx, val);

                                            let key = "mcp.allow_writes".to_string();
                                            let value = val.to_string();
                                            if let Some(view) = view_handle.upgrade() {
                                                view.update(cx, |view, cx| {
                                                    view.save_setting_debounced(
                                                        key, value, false, cx,
                                                    );
                                                });
                                            }
                                        }
                                    },
                                )
                                .default_value(default_settings.mcp.allow_writes),
                            )
                            .description(t!("settings.mcp.allow_writes_description").into_owned()),
                            SettingItem::render(|_, _, cx| mcp_status_element(cx)),
                        ]),
                ]),
        ]
    }
}

/// Status line plus the copy buttons an agent setup needs: endpoint URL, token,
/// the `claude mcp add` one-liner and the agent workspace folder.
fn mcp_status_element(cx: &mut App) -> gpui::AnyElement {
    let theme = cx.theme();
    let workspace = crate::mcp::discovery::workspace_dir();
    let (status_text, status_color, endpoint) = match McpService::global(cx).status() {
        McpStatus::Running { port, token } => (
            t!(
                "settings.mcp.running_at",
                url = crate::mcp::discovery::server_url(*port)
            )
            .into_owned(),
            theme.green,
            Some((*port, token.clone())),
        ),
        McpStatus::Stopped => (
            t!("settings.mcp.stopped").to_string(),
            theme.muted_foreground,
            None,
        ),
        McpStatus::Error(message) => (
            t!("settings.mcp.error", message = message).into_owned(),
            theme.red,
            None,
        ),
    };

    v_flex()
        .gap_2()
        .w_full()
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t!("settings.mcp.status")),
                )
                .child(div().text_sm().text_color(status_color).child(status_text))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t!("settings.mcp.workspace", path = workspace.display())),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .when_some(endpoint, |this, (port, token)| {
                    let url = crate::mcp::discovery::server_url(port);
                    let command = crate::mcp::discovery::claude_mcp_add_command(port, &token);
                    this.child(
                        Button::new("mcp-copy-url")
                            .small()
                            .outline()
                            .label(t!("settings.mcp.copy_url"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            }),
                    )
                    .child(
                        Button::new("mcp-copy-token")
                            .small()
                            .outline()
                            .label(t!("settings.mcp.copy_token"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(token.clone()));
                            }),
                    )
                    .child(
                        Button::new("mcp-copy-claude-command")
                            .small()
                            .outline()
                            .label(t!("settings.mcp.copy_claude_command"))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                            }),
                    )
                })
                .child(
                    Button::new("mcp-open-workspace")
                        .small()
                        .outline()
                        .label(t!("settings.mcp.open_workspace"))
                        .on_click(move |_, _, cx| {
                            if let Err(error) = std::fs::create_dir_all(&workspace) {
                                tracing::warn!("Failed to create the agent workspace: {error}");
                            }
                            cx.reveal_path(&workspace);
                        }),
                ),
        )
        .into_any_element()
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_font_selects(window, cx);
        if self.environment_tags_editor.is_none() {
            self.environment_tags_editor =
                Some(cx.new(|cx| super::environments_page::EnvironmentTagsEditor::new(window, cx)));
        }
        if self.theme_browser.is_none() {
            let options = vsx_themes::ThemeBrowserOptions {
                themes_dir: crate::custom_themes::themes_dir(),
                convert: vsx_themes::SQL,
                // Custom theme files load once at startup.
                watches_themes_dir: false,
                use_theme: Some(std::rc::Rc::new(
                    |name: &str, _: &mut Window, cx: &mut App| {
                        crate::settings::use_theme(name, cx);
                    },
                )),
            };
            self.theme_browser =
                Some(cx.new(|cx| vsx_themes::ThemeBrowser::new(options, window, cx)));
        }

        // The settings component keeps its search box from its first render,
        // so each language gets its own state.
        GpuiSettings::new(SharedString::from(format!(
            "blanco-settings-{}",
            rust_i18n::locale().as_ref() as &str
        )))
        .with_group_variant(GroupBoxVariant::Outline)
        .pages(self.setting_pages(cx))
    }
}

/// Save `appearance.<field>` through the settings view's debounce.
fn save_appearance(
    view_handle: &gpui::WeakEntity<SettingsView>,
    field: &str,
    value: String,
    cx: &mut App,
) {
    if let Some(view) = view_handle.upgrade() {
        view.update(cx, |view, cx| {
            view.save_setting_debounced(format!("appearance.{field}"), value, false, cx);
        });
    }
}
