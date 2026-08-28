use crate::settings::{EditorSettings, FormatterSettings};
use gpui_component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_component::input::Position;
use sqruff_lib::core::config::{FluffConfig, Value};
use sqruff_lib::core::linter::core::Linter;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

struct LinterState {
    linter: Linter,
    fingerprint: u64,
}

pub struct SqruffService {
    dialect: String,
    state: Mutex<LinterState>,
}

fn compute_fingerprint(formatter: &FormatterSettings, editor: &EditorSettings) -> u64 {
    let mut hasher = DefaultHasher::new();
    formatter.indented_joins.hash(&mut hasher);
    formatter.indented_ctes.hash(&mut hasher);
    formatter.indented_using_on.hash(&mut hasher);
    formatter.indented_on_contents.hash(&mut hasher);
    formatter.indented_then.hash(&mut hasher);
    formatter.indented_then_contents.hash(&mut hasher);
    formatter.allow_implicit_indents.hash(&mut hasher);
    formatter.trailing_comments.hash(&mut hasher);
    formatter.max_line_length.hash(&mut hasher);
    let mut rules: Vec<&String> = formatter.exclude_rules.iter().collect();
    rules.sort();
    rules.len().hash(&mut hasher);
    for rule in rules {
        rule.hash(&mut hasher);
    }
    formatter.keywords_policy.hash(&mut hasher);
    formatter.identifiers_policy.hash(&mut hasher);
    formatter.functions_policy.hash(&mut hasher);
    formatter.literals_policy.hash(&mut hasher);
    formatter.types_policy.hash(&mut hasher);
    formatter.select_clause_trailing_comma.hash(&mut hasher);
    formatter.terminator_multiline_newline.hash(&mut hasher);
    formatter.require_final_semicolon.hash(&mut hasher);
    editor.hard_tabs.hash(&mut hasher);
    editor.tab_size.hash(&mut hasher);
    hasher.finish()
}

fn build_config(
    dialect: &str,
    formatter: &FormatterSettings,
    editor: &EditorSettings,
) -> FluffConfig {
    let mut configs = ahash::AHashMap::new();

    let exclude_rules_str = formatter
        .exclude_rules
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(",");

    let mut core_config = ahash::AHashMap::new();
    core_config.insert("dialect".to_string(), Value::String(dialect.into()));
    core_config.insert(
        "exclude_rules".to_string(),
        Value::String(exclude_rules_str.into()),
    );
    core_config.insert(
        "max_line_length".to_string(),
        Value::Int(formatter.max_line_length as i32),
    );
    configs.insert("core".to_string(), Value::Map(core_config));

    let indent_unit = if editor.hard_tabs { "tab" } else { "space" };
    let mut indentation_config = ahash::AHashMap::new();
    indentation_config.insert("indent_unit".to_string(), Value::String(indent_unit.into()));
    indentation_config.insert(
        "tab_space_size".to_string(),
        Value::Int(editor.tab_size as i32),
    );
    indentation_config.insert(
        "indented_joins".to_string(),
        Value::Bool(formatter.indented_joins),
    );
    indentation_config.insert(
        "indented_ctes".to_string(),
        Value::Bool(formatter.indented_ctes),
    );
    indentation_config.insert(
        "indented_using_on".to_string(),
        Value::Bool(formatter.indented_using_on),
    );
    indentation_config.insert(
        "indented_on_contents".to_string(),
        Value::Bool(formatter.indented_on_contents),
    );
    indentation_config.insert(
        "indented_then".to_string(),
        Value::Bool(formatter.indented_then),
    );
    indentation_config.insert(
        "indented_then_contents".to_string(),
        Value::Bool(formatter.indented_then_contents),
    );
    indentation_config.insert(
        "allow_implicit_indents".to_string(),
        Value::Bool(formatter.allow_implicit_indents),
    );
    indentation_config.insert(
        "trailing_comments".to_string(),
        Value::String(formatter.trailing_comments.as_str().into()),
    );
    configs.insert("indentation".to_string(), Value::Map(indentation_config));

    // sqruff nests per-rule options under a `rules` map (the `[sqruff:rules:x]`
    // sections of a config file); a flat "rules:x" key is silently ignored.
    let mut rules = ahash::AHashMap::new();
    let mut rule = |name: &str, entries: Vec<(&str, Value)>| {
        let section: ahash::AHashMap<String, Value> = entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        rules.insert(name.to_string(), Value::Map(section));
    };
    rule(
        "capitalisation.keywords",
        vec![(
            "capitalisation_policy",
            Value::String(formatter.keywords_policy.as_str().into()),
        )],
    );
    rule(
        "capitalisation.identifiers",
        vec![(
            "extended_capitalisation_policy",
            Value::String(formatter.identifiers_policy.as_str().into()),
        )],
    );
    rule(
        "capitalisation.functions",
        vec![(
            "extended_capitalisation_policy",
            Value::String(formatter.functions_policy.as_str().into()),
        )],
    );
    rule(
        "capitalisation.literals",
        vec![(
            "capitalisation_policy",
            Value::String(formatter.literals_policy.as_str().into()),
        )],
    );
    rule(
        "capitalisation.types",
        vec![(
            "extended_capitalisation_policy",
            Value::String(formatter.types_policy.as_str().into()),
        )],
    );
    rule(
        "convention.select_trailing_comma",
        vec![(
            "select_clause_trailing_comma",
            Value::String(formatter.select_clause_trailing_comma.as_str().into()),
        )],
    );
    rule(
        "convention.terminator",
        vec![
            (
                "multiline_newline",
                Value::Bool(formatter.terminator_multiline_newline),
            ),
            (
                "require_final_semicolon",
                Value::Bool(formatter.require_final_semicolon),
            ),
        ],
    );
    configs.insert("rules".to_string(), Value::Map(rules));

    FluffConfig::new(configs, None, None)
}

impl SqruffService {
    pub fn new(
        dialect: &str,
        formatter: &FormatterSettings,
        editor: &EditorSettings,
    ) -> Result<Self, String> {
        let config = build_config(dialect, formatter, editor);
        let linter = Linter::new(config, None, None, false);

        Ok(Self {
            dialect: dialect.to_string(),
            state: Mutex::new(LinterState {
                linter,
                fingerprint: compute_fingerprint(formatter, editor),
            }),
        })
    }

    fn ensure_linter(
        &self,
        formatter: &FormatterSettings,
        editor: &EditorSettings,
    ) -> Result<(), String> {
        let fingerprint = compute_fingerprint(formatter, editor);
        let mut state = self
            .state
            .lock()
            .map_err(|e| format!("Linter lock poisoned: {}", e))?;

        if state.fingerprint != fingerprint {
            let config = build_config(&self.dialect, formatter, editor);
            state.linter = Linter::new(config, None, None, false);
            state.fingerprint = fingerprint;
        }

        Ok(())
    }

    pub fn lint(
        &self,
        sql: &str,
        formatter: &FormatterSettings,
        editor: &EditorSettings,
        _statement_offset: Option<usize>,
    ) -> Result<Vec<Diagnostic>, String> {
        if sql.len() > 300_000 {
            return Err("SQL text too large to lint".into());
        }

        if sql.trim().is_empty() {
            return Ok(Vec::new());
        }

        self.ensure_linter(formatter, editor)?;

        let mut state = self
            .state
            .lock()
            .map_err(|e| format!("Linter lock poisoned: {}", e))?;
        let linted_file = state.linter.lint_string_wrapped(sql, false);
        drop(state);

        let violations = linted_file.violations();

        let diagnostics: Vec<Diagnostic> = violations
            .iter()
            .map(|violation| {
                let severity = Self::severity_for_rule(violation.rule_code());

                let line_no = violation.line_no.saturating_sub(1) as u32;
                let col_no = violation.line_pos.saturating_sub(1) as u32;

                let length = if violation.source_slice.end > violation.source_slice.start {
                    (violation.source_slice.end - violation.source_slice.start) as u32
                } else {
                    10
                };

                let start = Position::new(line_no, col_no);
                let end = Position::new(line_no, col_no + length);

                let message = format!("{}: {}", violation.rule_code(), violation.description);

                Diagnostic::new(start..end, message).with_severity(severity)
            })
            .collect();

        Ok(diagnostics)
    }

    pub fn format(
        &self,
        sql: &str,
        formatter: &FormatterSettings,
        editor: &EditorSettings,
    ) -> Result<String, String> {
        if sql.trim().is_empty() {
            return Ok(sql.to_string());
        }

        self.ensure_linter(formatter, editor)?;

        let mut state = self
            .state
            .lock()
            .map_err(|e| format!("Linter lock poisoned: {}", e))?;
        let linted_file = state.linter.lint_string_wrapped(sql, true);
        drop(state);

        let formatted = linted_file.fix_string();

        if formatted.is_empty() {
            tracing::warn!("Sqruff formatter produced empty output, using original");
            Ok(sql.to_string())
        } else {
            Ok(formatted)
        }
    }

    fn severity_for_rule(rule_code: &str) -> DiagnosticSeverity {
        match &rule_code[..2.min(rule_code.len())] {
            "LT" => DiagnosticSeverity::Hint,
            "CV" => DiagnosticSeverity::Warning,
            "ST" => DiagnosticSeverity::Error,
            "RF" => DiagnosticSeverity::Warning,
            "AM" => DiagnosticSeverity::Warning,
            "JJ" => DiagnosticSeverity::Warning,
            _ => DiagnosticSeverity::Warning,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upper_keywords() -> FormatterSettings {
        FormatterSettings {
            keywords_policy: "upper".to_string(),
            ..FormatterSettings::default()
        }
    }

    #[test]
    fn fingerprint_is_stable_for_equal_settings_and_changes_with_any_policy() {
        let editor = EditorSettings::default();
        let base = FormatterSettings::default();
        assert_eq!(
            compute_fingerprint(&base, &editor),
            compute_fingerprint(&base.clone(), &editor)
        );

        let variants = [
            FormatterSettings {
                keywords_policy: "upper".to_string(),
                ..base.clone()
            },
            FormatterSettings {
                indented_joins: !base.indented_joins,
                ..base.clone()
            },
            FormatterSettings {
                max_line_length: base.max_line_length + 1,
                ..base.clone()
            },
            FormatterSettings {
                exclude_rules: std::collections::HashSet::new(),
                ..base.clone()
            },
            FormatterSettings {
                require_final_semicolon: !base.require_final_semicolon,
                ..base.clone()
            },
        ];
        for variant in &variants {
            assert_ne!(
                compute_fingerprint(variant, &editor),
                compute_fingerprint(&base, &editor),
                "{variant:?}"
            );
        }

        let wider_tabs = EditorSettings {
            tab_size: editor.tab_size + 2,
            ..editor
        };
        assert_ne!(
            compute_fingerprint(&base, &wider_tabs),
            compute_fingerprint(&base, &editor)
        );
    }

    #[test]
    fn formats_per_dialect_without_diagnostics() {
        let editor = EditorSettings::default();
        let formatter = upper_keywords();
        for db_type in [
            blanco_core::DatabaseType::PostgreSQL,
            blanco_core::DatabaseType::MySQL,
            blanco_core::DatabaseType::SQLite,
            blanco_core::DatabaseType::MsSql,
            blanco_core::DatabaseType::ClickHouse,
        ] {
            let dialect = db_type.to_sqruff_dialect();
            let service = SqruffService::new(dialect, &formatter, &editor)
                .unwrap_or_else(|error| panic!("{dialect}: {error}"));
            let formatted = service
                .format("select a,b from t where a=1", &formatter, &editor)
                .unwrap_or_else(|error| panic!("{dialect}: {error}"));
            assert!(formatted.contains("SELECT"), "{dialect}: {formatted}");
            assert!(formatted.contains("FROM"), "{dialect}: {formatted}");
            assert!(formatted.contains("WHERE"), "{dialect}: {formatted}");

            let diagnostics = service
                .lint(&formatted, &formatter, &editor, None)
                .unwrap_or_else(|error| panic!("{dialect}: {error}"));
            assert!(
                diagnostics.is_empty(),
                "{dialect}: formatted SQL should lint clean, got {diagnostics:?}"
            );
        }
    }

    #[test]
    fn rebuilds_linter_when_settings_change() {
        let editor = EditorSettings::default();
        let service = SqruffService::new("postgres", &FormatterSettings::default(), &editor)
            .expect("linter should build");
        let lower = FormatterSettings {
            keywords_policy: "lower".to_string(),
            ..FormatterSettings::default()
        };
        let formatted = service
            .format("SELECT a FROM t", &lower, &editor)
            .expect("format should succeed");
        assert!(formatted.contains("select"), "{formatted}");
        let formatted = service
            .format("select a from t", &upper_keywords(), &editor)
            .expect("format should succeed");
        assert!(formatted.contains("SELECT"), "{formatted}");
    }
}
