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

struct Rule {
    code: &'static str,
    description: &'static str,
}

struct RuleGroup {
    title: &'static str,
    rules: &'static [Rule],
}

const ALIASING_RULES: &[Rule] = &[
    Rule {
        code: "AL01",
        description: "Implicit/explicit aliasing of table.",
    },
    Rule {
        code: "AL02",
        description: "Implicit/explicit aliasing of columns.",
    },
    Rule {
        code: "AL03",
        description: "Column expression without alias. Use explicit AS clause.",
    },
    Rule {
        code: "AL04",
        description: "Table aliases should be unique within each clause.",
    },
    Rule {
        code: "AL05",
        description: "Tables should not be aliased if that alias is not used.",
    },
    Rule {
        code: "AL06",
        description: "Identify aliases in from clause and join conditions.",
    },
    Rule {
        code: "AL07",
        description: "Avoid table aliases in from clauses and join conditions.",
    },
    Rule {
        code: "AL08",
        description: "Column aliases should be unique within each clause.",
    },
    Rule {
        code: "AL09",
        description: "Find self-aliased columns and fix them.",
    },
];

const AMBIGUOUS_RULES: &[Rule] = &[
    Rule {
        code: "AM01",
        description: "Ambiguous use of DISTINCT in a SELECT with GROUP BY.",
    },
    Rule {
        code: "AM02",
        description: "UNION not immediately followed by DISTINCT or ALL.",
    },
    Rule {
        code: "AM03",
        description: "Ambiguous ordering directions for columns in ORDER BY.",
    },
    Rule {
        code: "AM04",
        description: "Outermost query should produce known number of columns.",
    },
    Rule {
        code: "AM05",
        description: "Join clauses should be fully qualified.",
    },
    Rule {
        code: "AM06",
        description: "Inconsistent column references in GROUP BY/ORDER BY.",
    },
    Rule {
        code: "AM07",
        description: "All queries in set expression should return same number of columns.",
    },
    Rule {
        code: "AM08",
        description: "Implicit cross join detected.",
    },
    Rule {
        code: "AM09",
        description: "LIMIT/OFFSET without ORDER BY.",
    },
];

const CAPITALISATION_RULES: &[Rule] = &[
    Rule {
        code: "CP01",
        description: "Inconsistent capitalisation of keywords.",
    },
    Rule {
        code: "CP02",
        description: "Inconsistent capitalisation of unquoted identifiers.",
    },
    Rule {
        code: "CP03",
        description: "Inconsistent capitalisation of function names.",
    },
    Rule {
        code: "CP04",
        description: "Inconsistent capitalisation of boolean/null literal.",
    },
    Rule {
        code: "CP05",
        description: "Inconsistent capitalisation of datatypes.",
    },
];

const CONVENTION_RULES: &[Rule] = &[
    Rule {
        code: "CV01",
        description: "Consistent usage of != or <> for \"not equal to\".",
    },
    Rule {
        code: "CV02",
        description: "Use COALESCE instead of IFNULL or NVL.",
    },
    Rule {
        code: "CV03",
        description: "Trailing commas within select clause.",
    },
    Rule {
        code: "CV04",
        description: "Use consistent syntax to express count number of rows.",
    },
    Rule {
        code: "CV05",
        description: "Relational operators should not be used to check for NULL.",
    },
    Rule {
        code: "CV06",
        description: "Statements must end with a semi-colon.",
    },
    Rule {
        code: "CV07",
        description: "Top-level statements should not be wrapped in brackets.",
    },
    Rule {
        code: "CV08",
        description: "Use LEFT JOIN instead of RIGHT JOIN.",
    },
    Rule {
        code: "CV09",
        description: "Block a list of configurable words from being used.",
    },
    Rule {
        code: "CV10",
        description: "Consistent usage of preferred quotes for quoted literals.",
    },
    Rule {
        code: "CV11",
        description: "Enforce consistent type casting style.",
    },
    Rule {
        code: "CV12",
        description: "Join conditions should use the JOIN ... ON syntax.",
    },
];

const LAYOUT_RULES: &[Rule] = &[
    Rule {
        code: "LT01",
        description: "Inappropriate spacing.",
    },
    Rule {
        code: "LT02",
        description: "Incorrect indentation.",
    },
    Rule {
        code: "LT03",
        description: "Operators should follow a standard for newlines.",
    },
    Rule {
        code: "LT04",
        description: "Leading/Trailing comma enforcement.",
    },
    Rule {
        code: "LT05",
        description: "Line is too long.",
    },
    Rule {
        code: "LT06",
        description: "Function name not immediately followed by parenthesis.",
    },
    Rule {
        code: "LT07",
        description: "WITH clause closing bracket should be on a new line.",
    },
    Rule {
        code: "LT08",
        description: "Blank line expected after CTE closing bracket.",
    },
    Rule {
        code: "LT09",
        description: "Select targets should be on a new line unless there is only one.",
    },
    Rule {
        code: "LT10",
        description: "SELECT modifiers must be on the same line as SELECT.",
    },
    Rule {
        code: "LT11",
        description: "Set operators should be surrounded by newlines.",
    },
    Rule {
        code: "LT12",
        description: "Files must end with a single trailing newline.",
    },
    Rule {
        code: "LT13",
        description: "Files must not begin with newlines or whitespace.",
    },
    Rule {
        code: "LT14",
        description: "Keyword clause newline enforcement.",
    },
    Rule {
        code: "LT15",
        description: "Too many consecutive blank lines.",
    },
];

const REFERENCES_RULES: &[Rule] = &[
    Rule {
        code: "RF01",
        description: "References cannot reference objects not present in FROM.",
    },
    Rule {
        code: "RF02",
        description: "References should be qualified if select has more than one table.",
    },
    Rule {
        code: "RF03",
        description: "References should be consistent in single-table statements.",
    },
    Rule {
        code: "RF04",
        description: "Keywords should not be used as identifiers.",
    },
    Rule {
        code: "RF05",
        description: "Do not use special characters in identifiers.",
    },
    Rule {
        code: "RF06",
        description: "Unnecessary quoted identifier.",
    },
];

const STRUCTURE_RULES: &[Rule] = &[
    Rule {
        code: "ST01",
        description: "Do not specify ELSE NULL in a CASE WHEN (redundant).",
    },
    Rule {
        code: "ST02",
        description: "Unnecessary CASE statement.",
    },
    Rule {
        code: "ST03",
        description: "Query defines a CTE but does not use it.",
    },
    Rule {
        code: "ST04",
        description: "Nested CASE in ELSE could be flattened.",
    },
    Rule {
        code: "ST05",
        description: "Join/From clauses should not contain subqueries. Use CTEs.",
    },
    Rule {
        code: "ST06",
        description: "Select wildcards then simple targets before calculations.",
    },
    Rule {
        code: "ST07",
        description: "Prefer specifying join keys instead of USING.",
    },
    Rule {
        code: "ST08",
        description: "Looking for DISTINCT before a bracket.",
    },
    Rule {
        code: "ST09",
        description: "Joins should list earlier/later table first.",
    },
    Rule {
        code: "ST10",
        description: "Redundant constant expression.",
    },
    Rule {
        code: "ST11",
        description: "Joined table not referenced in query.",
    },
    Rule {
        code: "ST12",
        description: "Remove consecutive semicolons.",
    },
];

const RULE_GROUPS: &[RuleGroup] = &[
    RuleGroup {
        title: "Aliasing",
        rules: ALIASING_RULES,
    },
    RuleGroup {
        title: "Ambiguous",
        rules: AMBIGUOUS_RULES,
    },
    RuleGroup {
        title: "Capitalisation",
        rules: CAPITALISATION_RULES,
    },
    RuleGroup {
        title: "Convention",
        rules: CONVENTION_RULES,
    },
    RuleGroup {
        title: "Layout",
        rules: LAYOUT_RULES,
    },
    RuleGroup {
        title: "References",
        rules: REFERENCES_RULES,
    },
    RuleGroup {
        title: "Structure",
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
                    .child(group.title.to_string())
                    .into_any_element(),
            );

            for rule in group.rules {
                let is_excluded = excluded.contains(rule.code);
                let code = rule.code.to_string();
                let view_handle = view_handle.clone();

                let checkbox = Checkbox::new(rule.code)
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
                    .child(
                        div().flex().flex_row().gap_1().child(checkbox).child(
                            div()
                                .text_sm()
                                .child(format!("{} {}", rule.code, rule.description)),
                        ),
                    )
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

    SettingPage::new("Formatter").resettable(true).groups(vec![
        SettingGroup::new().title("Indentation").items(vec![
            SettingItem::new(
                "Indented Joins",
                SettingField::switch(
                    move |cx: &App| AppSettings::global(cx).settings.formatter.indented_joins,
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
            .description("Indent JOIN clauses relative to the FROM clause."),
            SettingItem::new(
                "Indented CTEs",
                SettingField::switch(
                    move |cx: &App| AppSettings::global(cx).settings.formatter.indented_ctes,
                    {
                        let view_handle = view_handle.clone();
                        move |val: bool, cx: &mut App| {
                            AppSettings::global_mut(cx).settings.formatter.indented_ctes = val;
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
            .description("Indent CTE contents within the WITH clause."),
            SettingItem::new(
                "Indented USING/ON",
                SettingField::switch(
                    move |cx: &App| AppSettings::global(cx).settings.formatter.indented_using_on,
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
            .description("Indent USING and ON keywords within JOIN clauses."),
            SettingItem::new(
                "Indented ON Contents",
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
            .description("Indent the contents after ON in JOIN conditions."),
            SettingItem::new(
                "Indented THEN",
                SettingField::switch(
                    move |cx: &App| AppSettings::global(cx).settings.formatter.indented_then,
                    {
                        let view_handle = view_handle.clone();
                        move |val: bool, cx: &mut App| {
                            AppSettings::global_mut(cx).settings.formatter.indented_then = val;
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
            .description("Indent THEN keyword in CASE expressions."),
            SettingItem::new(
                "Indented THEN Contents",
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
            .description("Indent the contents after THEN in CASE expressions."),
            SettingItem::new(
                "Allow Implicit Indents",
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
            .description("Allow implicit indentation in certain contexts."),
            SettingItem::new(
                "Trailing Comments",
                SettingField::dropdown(
                    vec![
                        ("before".into(), "Before the line".into()),
                        ("after".into(), "After the line".into()),
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
            .description("Where to place trailing comments when reflowing long lines."),
        ]),
        SettingGroup::new().title("Line Length").items(vec![
            SettingItem::new(
                "Max Line Length",
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
            .description("Maximum line length before wrapping (0 disables)."),
        ]),
        SettingGroup::new().title("Excluded Rules").items(vec![
            SettingItem::new(
                "Exclude Rules",
                SettingField::element(ExcludeRulesField {
                    view_handle: view_handle.clone(),
                }),
            )
            .description("Rules to exclude from linting and formatting."),
        ]),
        SettingGroup::new().title("Capitalisation").items(vec![
            SettingItem::new(
                "Keywords",
                SettingField::dropdown(
                    vec![
                        ("consistent".into(), "Consistent".into()),
                        ("upper".into(), "Upper".into()),
                        ("lower".into(), "Lower".into()),
                        ("capitalise".into(), "Capitalise".into()),
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
            .description("Capitalisation policy for SQL keywords (SELECT, FROM, etc.)."),
            SettingItem::new(
                "Identifiers",
                SettingField::dropdown(
                    vec![
                        ("consistent".into(), "Consistent".into()),
                        ("upper".into(), "Upper".into()),
                        ("lower".into(), "Lower".into()),
                        ("capitalise".into(), "Capitalise".into()),
                        ("pascal".into(), "Pascal".into()),
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
            .description("Capitalisation policy for unquoted identifiers."),
            SettingItem::new(
                "Functions",
                SettingField::dropdown(
                    vec![
                        ("consistent".into(), "Consistent".into()),
                        ("upper".into(), "Upper".into()),
                        ("lower".into(), "Lower".into()),
                        ("capitalise".into(), "Capitalise".into()),
                        ("pascal".into(), "Pascal".into()),
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
            .description("Capitalisation policy for function names."),
            SettingItem::new(
                "Literals",
                SettingField::dropdown(
                    vec![
                        ("consistent".into(), "Consistent".into()),
                        ("upper".into(), "Upper".into()),
                        ("lower".into(), "Lower".into()),
                        ("capitalise".into(), "Capitalise".into()),
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
            .description("Capitalisation policy for null and boolean literals."),
            SettingItem::new(
                "Data Types",
                SettingField::dropdown(
                    vec![
                        ("consistent".into(), "Consistent".into()),
                        ("upper".into(), "Upper".into()),
                        ("lower".into(), "Lower".into()),
                        ("capitalise".into(), "Capitalise".into()),
                        ("pascal".into(), "Pascal".into()),
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
            .description("Capitalisation policy for data type names."),
        ]),
        SettingGroup::new().title("Convention").items(vec![
            SettingItem::new(
                "Trailing Commas",
                SettingField::dropdown(
                    vec![
                        ("forbid".into(), "Forbid".into()),
                        ("require".into(), "Require".into()),
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
            .description("Whether to require or forbid trailing commas in SELECT clauses."),
            SettingItem::new(
                "Multiline Semicolon on New Line",
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
            .description("Place semicolons on a new line for multiline statements."),
            SettingItem::new(
                "Require Final Semicolon",
                SettingField::switch(
                    move |cx: &App| {
                        AppSettings::global(cx)
                            .settings
                            .formatter
                            .require_final_semicolon
                    },
                    {
                        let view_handle = view_handle.clone();
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
            .description("Require a semicolon at the end of the final statement."),
        ]),
    ])
}
