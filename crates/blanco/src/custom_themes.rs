//! Theme files the user adds to `<config dir>/blanco/themes`, loaded next to
//! the embedded themes and reloaded when they change.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{App, Global, SharedString};
use gpui_component::{ThemeSet, WindowExt as _, notification::NotificationType};
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

/// Theme warnings not shown yet, and the ones already shown so a reload does
/// not repeat them.
#[derive(Default)]
struct ThemeWarnings {
    pending: Vec<String>,
    shown: HashSet<String>,
}

impl Global for ThemeWarnings {}

/// Check every theme file in `dir` and queue a warning per file with unknown
/// keys or invalid JSON, then show the queue in the open windows.
pub fn check_theme_files(dir: &Path, cx: &mut App) {
    let mut warnings = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        match unknown_keys(&content) {
            Ok(keys) if keys.is_empty() => {}
            Ok(keys) => warnings.push(format!(
                "Theme file {name} has keys Blanco does not know, which are ignored: {}",
                keys.join(", ")
            )),
            Err(error) => warnings.push(format!("Theme file {name} could not be read: {error}")),
        }
    }

    let state = cx.default_global::<ThemeWarnings>();
    for warning in warnings {
        if state.shown.insert(warning.clone()) {
            state.pending.push(warning);
        }
    }
    show_theme_warnings(cx);
}

/// Show the queued warnings as notifications in every open window. With no
/// window open yet they stay queued for the first one.
pub fn show_theme_warnings(cx: &mut App) {
    let windows = cx.windows();
    if windows.is_empty() || !cx.has_global::<ThemeWarnings>() {
        return;
    }
    let pending = std::mem::take(&mut cx.global_mut::<ThemeWarnings>().pending);
    for warning in pending {
        for window in &windows {
            let warning = SharedString::from(warning.clone());
            window
                .update(cx, |_, window, cx| {
                    window.push_notification((NotificationType::Warning, warning), cx);
                })
                .ok();
        }
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
}
