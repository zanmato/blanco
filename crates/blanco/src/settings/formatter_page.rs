use crate::app_settings::AppSettings;
use crate::settings::Settings;
use gpui::{App, IntoElement, ParentElement, SharedString, Styled, Window, div};
use gpui_component::ActiveTheme;
use gpui_component::checkbox::Checkbox;
use gpui_component::setting::{
    NumberFieldOptions, RenderOptions, SettingField, SettingFieldElement, SettingGroup,
    SettingItem, SettingPage,
};
use gpui_component::v_flex;

/// The lint rules by code. Their descriptions are looked up as
/// `settings.formatter.rules.<code>` when rendered, after the locale is set.
struct RuleGroup {
    title_key: &'static str,
    rules: &'static [&'static str],
}

const ALIASING_RULES: &[&str] = &[
    "AL01", "AL02", "AL03", "AL04", "AL05", "AL06", "AL07", "AL08", "AL09",
];

const AMBIGUOUS_RULES: &[&str] = &[
    "AM01", "AM02", "AM03", "AM04", "AM05", "AM06", "AM07", "AM08", "AM09",
];

const CAPITALISATION_RULES: &[&str] = &["CP01", "CP02", "CP03", "CP04", "CP05"];

const CONVENTION_RULES: &[&str] = &[
    "CV01", "CV02", "CV03", "CV04", "CV05", "CV06", "CV07", "CV08", "CV09", "CV10", "CV11", "CV12",
];

const LAYOUT_RULES: &[&str] = &[
    "LT01", "LT02", "LT03", "LT04", "LT05", "LT06", "LT07", "LT08", "LT09", "LT10", "LT11", "LT12",
    "LT13", "LT14", "LT15",
];

const REFERENCES_RULES: &[&str] = &["RF01", "RF02", "RF03", "RF04", "RF05", "RF06"];

const STRUCTURE_RULES: &[&str] = &[
    "ST01", "ST02", "ST03", "ST04", "ST05", "ST06", "ST07", "ST08", "ST09", "ST10", "ST11", "ST12",
];

const RULE_GROUPS: &[RuleGroup] = &[
    RuleGroup {
        title_key: "settings.formatter.rule_groups.aliasing",
        rules: ALIASING_RULES,
    },
    RuleGroup {
        title_key: "settings.formatter.rule_groups.ambiguous",
        rules: AMBIGUOUS_RULES,
    },
    RuleGroup {
        title_key: "settings.formatter.rule_groups.capitalisation",
        rules: CAPITALISATION_RULES,
    },
    RuleGroup {
        title_key: "settings.formatter.rule_groups.convention",
        rules: CONVENTION_RULES,
    },
    RuleGroup {
        title_key: "settings.formatter.rule_groups.layout",
        rules: LAYOUT_RULES,
    },
    RuleGroup {
        title_key: "settings.formatter.rule_groups.references",
        rules: REFERENCES_RULES,
    },
    RuleGroup {
        title_key: "settings.formatter.rule_groups.structure",
        rules: STRUCTURE_RULES,
    },
];

struct ExcludeRulesField {
    view_handle: gpui::WeakEntity<super::view::SettingsView>,
}

impl SettingFieldElement for ExcludeRulesField {
    type Element = gpui::AnyElement;

    fn render_field(
        &self,
        _options: &RenderOptions,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self::Element {
        let excluded = AppSettings::global(cx)
            .settings
            .formatter
            .exclude_rules
            .clone();
        let view_handle = self.view_handle.clone();

        let mut groups: Vec<gpui::AnyElement> = Vec::new();

        for group in RULE_GROUPS {
            let mut group_children: Vec<gpui::AnyElement> = Vec::new();

            group_children.push(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .pb_1()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(SharedString::from(t!(group.title_key)))
                    .into_any_element(),
            );

            for &rule in group.rules {
                let is_excluded = excluded.contains(rule);
                let code = rule.to_string();
                let view_handle = view_handle.clone();

                let checkbox = Checkbox::new(rule)
                    .checked(is_excluded)
                    .on_click(move |checked, _window, cx: &mut App| {
                        let rules =
                            &mut AppSettings::global_mut(cx).settings.formatter.exclude_rules;
                        if *checked {
                            rules.insert(code.clone());
                        } else {
                            rules.remove(&code);
                        }
                        let value = rules.iter().cloned().collect::<Vec<_>>().join(",");
                        if let Some(view) = view_handle.upgrade() {
                            view.update(cx, |view, cx| {
                                view.save_setting_debounced(
                                    "formatter.exclude_rules".into(),
                                    value,
                                    false,
                                    cx,
                                );
                            });
                        }
                    })
                    .into_any_element();

                let row = div()
                    .whitespace_normal()
                    .child(div().flex().flex_row().gap_1().child(checkbox).child(
                        div().text_sm().child(format!(
                            "{rule} {}",
                            t!(format!("settings.formatter.rules.{rule}"))
                        )),
                    ))
                    .into_any_element();

                group_children.push(row);
            }

            groups.push(
                v_flex()
                    .gap_0p5()
                    .pt_3()
                    .children(group_children)
                    .into_any_element(),
            );
        }

        v_flex().children(groups).into_any_element()
    }
}

pub fn formatter_page(
    view_handle: gpui::WeakEntity<super::view::SettingsView>,
    default_settings: &Settings,
) -> SettingPage {
    let defaults = &default_settings.formatter;

    SettingPage::new(t!("settings.formatter.title"))
        .resettable(true)
        .groups(vec![
            SettingGroup::new()
                .title(t!("settings.formatter.indentation"))
                .items(vec![
                    SettingItem::new(
                        t!("settings.formatter.indented_joins"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx).settings.formatter.indented_joins
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .indented_joins = val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.indented_joins".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.indented_joins),
                    )
                    .description(t!("settings.formatter.indented_joins_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.indented_ctes"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx).settings.formatter.indented_ctes
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).settings.formatter.indented_ctes =
                                        val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.indented_ctes".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.indented_ctes),
                    )
                    .description(t!("settings.formatter.indented_ctes_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.indented_using_on"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx).settings.formatter.indented_using_on
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .indented_using_on = val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.indented_using_on".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.indented_using_on),
                    )
                    .description(
                        t!("settings.formatter.indented_using_on_description").into_owned(),
                    ),
                    SettingItem::new(
                        t!("settings.formatter.indented_on_contents"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .formatter
                                    .indented_on_contents
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .indented_on_contents = val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.indented_on_contents".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.indented_on_contents),
                    )
                    .description(
                        t!("settings.formatter.indented_on_contents_description").into_owned(),
                    ),
                    SettingItem::new(
                        t!("settings.formatter.indented_then"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx).settings.formatter.indented_then
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).settings.formatter.indented_then =
                                        val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.indented_then".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.indented_then),
                    )
                    .description(t!("settings.formatter.indented_then_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.indented_then_contents"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .formatter
                                    .indented_then_contents
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .indented_then_contents = val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.indented_then_contents".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.indented_then_contents),
                    )
                    .description(
                        t!("settings.formatter.indented_then_contents_description").into_owned(),
                    ),
                    SettingItem::new(
                        t!("settings.formatter.allow_implicit_indents"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .formatter
                                    .allow_implicit_indents
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .allow_implicit_indents = val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.allow_implicit_indents".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.allow_implicit_indents),
                    )
                    .description(
                        t!("settings.formatter.allow_implicit_indents_description").into_owned(),
                    ),
                    SettingItem::new(
                        t!("settings.formatter.trailing_comments"),
                        SettingField::dropdown(
                            vec![
                                (
                                    "before".into(),
                                    t!("settings.formatter.trailing_comments_before").into(),
                                ),
                                (
                                    "after".into(),
                                    t!("settings.formatter.trailing_comments_after").into(),
                                ),
                            ],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx)
                                        .settings
                                        .formatter
                                        .trailing_comments
                                        .clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let value = val.to_string();
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .trailing_comments = value.clone();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.trailing_comments".into(),
                                                value,
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(defaults.trailing_comments.clone())),
                    )
                    .description(
                        t!("settings.formatter.trailing_comments_description").into_owned(),
                    ),
                ]),
            SettingGroup::new()
                .title(t!("settings.formatter.line_length"))
                .items(vec![
                    SettingItem::new(
                        t!("settings.formatter.max_line_length"),
                        SettingField::number_input(
                            NumberFieldOptions {
                                min: 0.0,
                                max: 200.0,
                                step: 10.0,
                            },
                            move |cx: &App| {
                                AppSettings::global(cx).settings.formatter.max_line_length as f64
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: f64, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .max_line_length = val as u32;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.max_line_length".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.max_line_length as f64),
                    )
                    .description(t!("settings.formatter.max_line_length_description").into_owned()),
                ]),
            SettingGroup::new()
                .title(t!("settings.formatter.excluded_rules"))
                .items(vec![
                    SettingItem::new(
                        t!("settings.formatter.exclude_rules"),
                        SettingField::element(ExcludeRulesField {
                            view_handle: view_handle.clone(),
                        }),
                    )
                    .description(t!("settings.formatter.exclude_rules_description").into_owned()),
                ]),
            SettingGroup::new()
                .title(t!("settings.formatter.capitalisation"))
                .items(vec![
                    SettingItem::new(
                        t!("settings.formatter.keywords"),
                        SettingField::dropdown(
                            vec![
                                (
                                    "consistent".into(),
                                    t!("settings.formatter.case_consistent").into(),
                                ),
                                ("upper".into(), t!("settings.formatter.case_upper").into()),
                                ("lower".into(), t!("settings.formatter.case_lower").into()),
                                (
                                    "capitalise".into(),
                                    t!("settings.formatter.case_capitalise").into(),
                                ),
                            ],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx)
                                        .settings
                                        .formatter
                                        .keywords_policy
                                        .clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let value = val.to_string();
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .keywords_policy = value.clone();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.keywords_policy".into(),
                                                value,
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(defaults.keywords_policy.clone())),
                    )
                    .description(t!("settings.formatter.keywords_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.identifiers"),
                        SettingField::dropdown(
                            vec![
                                (
                                    "consistent".into(),
                                    t!("settings.formatter.case_consistent").into(),
                                ),
                                ("upper".into(), t!("settings.formatter.case_upper").into()),
                                ("lower".into(), t!("settings.formatter.case_lower").into()),
                                (
                                    "capitalise".into(),
                                    t!("settings.formatter.case_capitalise").into(),
                                ),
                                ("pascal".into(), t!("settings.formatter.case_pascal").into()),
                            ],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx)
                                        .settings
                                        .formatter
                                        .identifiers_policy
                                        .clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let value = val.to_string();
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .identifiers_policy = value.clone();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.identifiers_policy".into(),
                                                value,
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(defaults.identifiers_policy.clone())),
                    )
                    .description(t!("settings.formatter.identifiers_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.functions"),
                        SettingField::dropdown(
                            vec![
                                (
                                    "consistent".into(),
                                    t!("settings.formatter.case_consistent").into(),
                                ),
                                ("upper".into(), t!("settings.formatter.case_upper").into()),
                                ("lower".into(), t!("settings.formatter.case_lower").into()),
                                (
                                    "capitalise".into(),
                                    t!("settings.formatter.case_capitalise").into(),
                                ),
                                ("pascal".into(), t!("settings.formatter.case_pascal").into()),
                            ],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx)
                                        .settings
                                        .formatter
                                        .functions_policy
                                        .clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let value = val.to_string();
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .functions_policy = value.clone();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.functions_policy".into(),
                                                value,
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(defaults.functions_policy.clone())),
                    )
                    .description(t!("settings.formatter.functions_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.literals"),
                        SettingField::dropdown(
                            vec![
                                (
                                    "consistent".into(),
                                    t!("settings.formatter.case_consistent").into(),
                                ),
                                ("upper".into(), t!("settings.formatter.case_upper").into()),
                                ("lower".into(), t!("settings.formatter.case_lower").into()),
                                (
                                    "capitalise".into(),
                                    t!("settings.formatter.case_capitalise").into(),
                                ),
                            ],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx)
                                        .settings
                                        .formatter
                                        .literals_policy
                                        .clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let value = val.to_string();
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .literals_policy = value.clone();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.literals_policy".into(),
                                                value,
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(defaults.literals_policy.clone())),
                    )
                    .description(t!("settings.formatter.literals_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.data_types"),
                        SettingField::dropdown(
                            vec![
                                (
                                    "consistent".into(),
                                    t!("settings.formatter.case_consistent").into(),
                                ),
                                ("upper".into(), t!("settings.formatter.case_upper").into()),
                                ("lower".into(), t!("settings.formatter.case_lower").into()),
                                (
                                    "capitalise".into(),
                                    t!("settings.formatter.case_capitalise").into(),
                                ),
                                ("pascal".into(), t!("settings.formatter.case_pascal").into()),
                            ],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx)
                                        .settings
                                        .formatter
                                        .types_policy
                                        .clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let value = val.to_string();
                                    AppSettings::global_mut(cx).settings.formatter.types_policy =
                                        value.clone();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.types_policy".into(),
                                                value,
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(defaults.types_policy.clone())),
                    )
                    .description(t!("settings.formatter.data_types_description").into_owned()),
                ]),
            SettingGroup::new()
                .title(t!("settings.formatter.convention"))
                .items(vec![
                    SettingItem::new(
                        t!("settings.formatter.trailing_commas"),
                        SettingField::dropdown(
                            vec![
                                (
                                    "forbid".into(),
                                    t!("settings.formatter.trailing_commas_forbid").into(),
                                ),
                                (
                                    "require".into(),
                                    t!("settings.formatter.trailing_commas_require").into(),
                                ),
                            ],
                            move |cx: &App| {
                                SharedString::from(
                                    AppSettings::global(cx)
                                        .settings
                                        .formatter
                                        .select_clause_trailing_comma
                                        .clone(),
                                )
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: SharedString, cx: &mut App| {
                                    let value = val.to_string();
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .select_clause_trailing_comma = value.clone();
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.select_clause_trailing_comma".into(),
                                                value,
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(SharedString::from(
                            defaults.select_clause_trailing_comma.clone(),
                        )),
                    )
                    .description(t!("settings.formatter.trailing_commas_description").into_owned()),
                    SettingItem::new(
                        t!("settings.formatter.multiline_semicolon_newline"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .formatter
                                    .terminator_multiline_newline
                            },
                            {
                                let view_handle = view_handle.clone();
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .terminator_multiline_newline = val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.terminator_multiline_newline".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.terminator_multiline_newline),
                    )
                    .description(
                        t!("settings.formatter.multiline_semicolon_newline_description")
                            .into_owned(),
                    ),
                    SettingItem::new(
                        t!("settings.formatter.require_final_semicolon"),
                        SettingField::switch(
                            move |cx: &App| {
                                AppSettings::global(cx)
                                    .settings
                                    .formatter
                                    .require_final_semicolon
                            },
                            {
                                let view_handle = view_handle;
                                move |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx)
                                        .settings
                                        .formatter
                                        .require_final_semicolon = val;
                                    if let Some(view) = view_handle.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.save_setting_debounced(
                                                "formatter.require_final_semicolon".into(),
                                                val.to_string(),
                                                false,
                                                cx,
                                            );
                                        });
                                    }
                                }
                            },
                        )
                        .default_value(defaults.require_final_semicolon),
                    )
                    .description(
                        t!("settings.formatter.require_final_semicolon_description").into_owned(),
                    ),
                ]),
        ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule descriptions are looked up by a key built from the code, which the
    /// locale test cannot see, so a new rule needs its key here.
    #[test]
    fn every_rule_has_a_description() {
        let backend = crate::_rust_i18n_backend();
        for group in RULE_GROUPS {
            assert!(backend.translate("en", group.title_key).is_some());
            for rule in group.rules {
                let key = format!("settings.formatter.rules.{rule}");
                assert!(backend.translate("en", &key).is_some(), "missing {key}");
            }
        }
    }
}
