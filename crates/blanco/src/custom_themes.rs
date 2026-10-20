//! Theme files the user adds to `<config dir>/blanco/themes`, loaded at startup
//! next to the embedded themes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::{App, Global, SharedString, Window};
use gpui_component::{ThemeRegistry, ThemeSet, WindowExt as _, notification::NotificationType};
use serde_json::Value;

/// The folder custom theme files are read from.
pub fn themes_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("blanco")
        .join("themes")
}

/// The keys in a theme file that the theme format does not have, as dotted
/// paths such as `themes.0.colors.backgrund`. Such keys are ignored when the
/// theme loads, which makes a misspelt color silently fall back.
pub fn unknown_keys(content: &str) -> anyhow::Result<Vec<String>> {
    let value: Value = serde_json::from_str(content)?;
    let schema = schemars::schema_for!(ThemeSet);
    let root = schema.as_value();
    let mut unknown = Vec::new();
    walk(root, root, &value, String::new(), &mut unknown);
    unknown.sort();
    Ok(unknown)
}

/// The schemas `schema` stands for once `$ref`, `allOf`, `anyOf` and `oneOf`
/// are followed.
fn alternatives<'a>(root: &'a Value, schema: &'a Value, out: &mut Vec<&'a Value>) {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let target = reference
            .strip_prefix("#/")
            .map(|path| path.split('/').try_fold(root, |node, part| node.get(part)));
        if let Some(Some(target)) = target {
            alternatives(root, target, out);
        }
        return;
    }
    let mut nested = false;
    for key in ["allOf", "anyOf", "oneOf"] {
        if let Some(list) = schema.get(key).and_then(Value::as_array) {
            nested = true;
            for item in list {
                alternatives(root, item, out);
            }
        }
    }
    if !nested || schema.get("properties").is_some() {
        out.push(schema);
    }
}

fn walk(root: &Value, schema: &Value, value: &Value, path: String, out: &mut Vec<String>) {
    let mut schemas = Vec::new();
    alternatives(root, schema, &mut schemas);
    let child = |key: &str| {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    };

    match value {
        Value::Object(map) => {
            let properties: BTreeMap<&str, &Value> = schemas
                .iter()
                .filter_map(|schema| schema.get("properties").and_then(Value::as_object))
                .flat_map(|properties| properties.iter().map(|(k, v)| (k.as_str(), v)))
                .collect();
            let additional = schemas
                .iter()
                .find_map(|schema| schema.get("additionalProperties"))
                .filter(|additional| additional.is_object());
            if properties.is_empty() && additional.is_none() {
                // A free-form object: anything goes.
                return;
            }
            // `$schema` and the like are editor hints, not theme keys.
            for (key, item) in map.iter().filter(|(key, _)| !key.starts_with('$')) {
                match (properties.get(key.as_str()), additional) {
                    (Some(item_schema), _) => walk(root, item_schema, item, child(key), out),
                    (None, Some(additional)) => walk(root, additional, item, child(key), out),
                    (None, None) => out.push(child(key)),
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schemas.iter().find_map(|schema| schema.get("items")) {
                for (index, item) in items.iter().enumerate() {
                    walk(root, item_schema, item, child(&index.to_string()), out);
                }
            }
        }
        _ => {}
    }
}

/// Warnings about theme files, kept until the first window opens.
#[derive(Default)]
struct ThemeWarnings(Vec<String>);

impl Global for ThemeWarnings {}

/// Register the theme files in [`themes_dir`]. Edits apply after a restart.
///
/// Must run before the embedded themes are added: `load_themes_from_str` keeps
/// the first theme of a name, so a custom file with an embedded theme's name
/// wins.
pub fn load_custom_themes(cx: &mut App) {
    let warnings = load_theme_files(&themes_dir(), ThemeRegistry::global_mut(cx));
    cx.set_global(ThemeWarnings(warnings));
}

/// Load every theme file in `dir` into `registry`, returning a warning per
/// file with unknown keys or that does not load.
fn load_theme_files(dir: &Path, registry: &mut ThemeRegistry) -> Vec<String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            return vec![format!(
                "Theme folder {} could not be read: {error}",
                dir.display()
            )];
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    paths.sort();

    let mut warnings = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let loaded = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|content| {
                registry.load_themes_from_str(&content)?;
                unknown_keys(&content)
            });
        match loaded {
            Ok(keys) if keys.is_empty() => {}
            Ok(keys) => {
                tracing::warn!("Theme file {name} has unknown keys: {}", keys.join(", "));
                warnings.push(unknown_keys_warning(&name, &keys));
            }
            Err(error) => {
                let warning = format!("Theme file {name} could not be loaded: {error}");
                tracing::warn!("{warning}");
                warnings.push(warning);
            }
        }
    }
    warnings
}

/// Themes written for other gpui-component apps often carry dozens of keys
/// this version ignores, so the notification names a few and the log has all.
fn unknown_keys_warning(name: &str, keys: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut warning = format!(
        "Theme file {name} has keys Blanco does not know, which are ignored: {}",
        keys.iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    if keys.len() > SHOWN {
        warning.push_str(&format!(
            " and {} more, listed in the log",
            keys.len() - SHOWN
        ));
    }
    warning
}

/// Show the theme file warnings in `window`, once.
pub fn show_theme_warnings(window: &mut Window, cx: &mut App) {
    if !cx.has_global::<ThemeWarnings>() {
        return;
    }
    let warnings = std::mem::take(&mut cx.global_mut::<ThemeWarnings>().0);
    for warning in warnings {
        window.push_notification((NotificationType::Warning, SharedString::from(warning)), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_named_with_their_path() {
        let content = r##"{
            "name": "Mine",
            "themes": [{
                "name": "Mine Dark",
                "mode": "dark",
                "colrs": {},
                "colors": {
                    "background": "#000000",
                    "backgrund": "#111111"
                }
            }]
        }"##;
        assert_eq!(
            unknown_keys(content).unwrap(),
            ["themes.0.colors.backgrund", "themes.0.colrs"]
        );
    }

    #[test]
    fn schema_hints_are_not_unknown() {
        let content = r#"{"$schema": "x", "name": "Mine", "themes": []}"#;
        assert!(unknown_keys(content).unwrap().is_empty());
    }

    #[test]
    fn a_long_list_of_unknown_keys_is_cut_short() {
        let keys: Vec<String> = (0..8).map(|index| format!("key{index}")).collect();
        let warning = unknown_keys_warning("mine.json", &keys);
        assert!(warning.ends_with("key0, key1, key2, key3, key4 and 3 more, listed in the log"));
    }

    #[gpui::test]
    fn theme_files_are_loaded_with_a_warning_per_problem(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        // Aurora from gpui-component, which uses two keys its loader ignores.
        std::fs::write(
            dir.path().join("aurora.json"),
            include_str!("../test-data/themes/aurora.json"),
        )
        .unwrap();
        std::fs::write(dir.path().join("broken.json"), "{").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a theme").unwrap();

        cx.update(|cx| {
            gpui_component::init(cx);
            let warnings = load_theme_files(dir.path(), ThemeRegistry::global_mut(cx));
            assert_eq!(
                warnings,
                [
                    "Theme file aurora.json has keys Blanco does not know, which are ignored: \
                     themes.0.colors.accordion.hover.background, themes.0.highlight.syntax.comment.doc",
                    "Theme file broken.json could not be loaded: \
                     EOF while parsing an object at line 1 column 1",
                ]
            );
            assert!(
                ThemeRegistry::global(cx)
                    .themes()
                    .contains_key(&SharedString::from("Aurora Light"))
            );
        });
    }

    #[gpui::test]
    fn a_missing_themes_folder_is_not_a_problem(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            let warnings =
                load_theme_files(&dir.path().join("themes"), ThemeRegistry::global_mut(cx));
            assert!(warnings.is_empty());
        });
    }

    #[gpui::test]
    fn a_custom_theme_replaces_an_embedded_one_of_the_same_name(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let mine = r##"{"name": "Mine", "themes": [{"name": "Catppuccin Latte", "mode": "light",
            "colors": {"background": "#123456"}}]}"##;
        std::fs::write(dir.path().join("mine.json"), mine).unwrap();

        cx.update(|cx| {
            gpui_component::init(cx);
            let registry = ThemeRegistry::global_mut(cx);
            assert!(load_theme_files(dir.path(), registry).is_empty());
            // As startup does after the custom files.
            registry
                .load_themes_from_str(include_str!("../assets/themes/catppuccin.json"))
                .unwrap();
            let latte =
                &ThemeRegistry::global(cx).themes()[&SharedString::from("Catppuccin Latte")];
            assert_eq!(latte.colors.background.as_deref(), Some("#123456"));
        });
    }
}
