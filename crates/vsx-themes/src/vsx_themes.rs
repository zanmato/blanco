//! VS Code color themes for gpui-component apps: convert an extension's
//! themes to a gpui-component theme file, find extensions on Open VSX, and
//! the settings panel that does both.
//!
//! Nothing here knows the app it runs in. An app gives the panel the folder
//! its custom themes load from at startup, and what "use this theme" means.

#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

pub mod browser;
pub mod color;
pub mod convert;
pub mod gallery;
pub mod package;

use std::path::{Path, PathBuf};

use anyhow::Context as _;

pub use browser::{ThemeBrowser, ThemeBrowserOptions};
pub use convert::{ConvertOptions, ConvertedTheme, ConvertedThemes, SQL, convert_package};
pub use package::Package;

/// Installed themes are named after their extension, so installing again
/// replaces the file and removing finds it.
const FILE_PREFIX: &str = "vscode.";

/// Where the extension `extension_id` (`publisher.name`) is installed in
/// `themes_dir`.
pub fn installed_path(themes_dir: &Path, extension_id: &str) -> PathBuf {
    let safe: String = extension_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    themes_dir.join(format!("{FILE_PREFIX}{safe}.json"))
}

pub fn is_installed(themes_dir: &Path, extension_id: &str) -> bool {
    installed_path(themes_dir, extension_id).is_file()
}

/// Write `themes` to `themes_dir`, replacing an earlier install. Written to a
/// temporary file first, so a failed write leaves the old file whole.
pub fn install(
    themes_dir: &Path,
    extension_id: &str,
    themes: &ConvertedThemes,
) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(themes_dir)
        .with_context(|| t!("vsx_themes.errors.creating", path = themes_dir.display()))?;
    let path = installed_path(themes_dir, extension_id);
    let temporary = path.with_extension("json.partial");
    std::fs::write(&temporary, themes.to_json())
        .with_context(|| t!("vsx_themes.errors.writing", path = temporary.display()))?;
    std::fs::rename(&temporary, &path)
        .with_context(|| t!("vsx_themes.errors.writing", path = path.display()))?;
    Ok(path)
}

pub fn uninstall(themes_dir: &Path, extension_id: &str) -> anyhow::Result<()> {
    let path = installed_path(themes_dir, extension_id);
    std::fs::remove_file(&path)
        .with_context(|| t!("vsx_themes.errors.removing", path = path.display()))
}

/// The theme names an installed file holds.
pub fn installed_theme_names(themes_dir: &Path, extension_id: &str) -> Vec<(String, bool)> {
    let path = installed_path(themes_dir, extension_id);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(document) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    document["themes"]
        .as_array()
        .map(|themes| {
            themes
                .iter()
                .filter_map(|theme| {
                    let name = theme["name"].as_str()?.to_string();
                    Some((name, theme["mode"].as_str() == Some("light")))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_paths_are_safe_file_names() {
        let dir = Path::new("/themes");
        assert_eq!(
            installed_path(dir, "enkia.tokyo-night"),
            Path::new("/themes/vscode.enkia.tokyo-night.json")
        );
        assert_eq!(
            installed_path(dir, "../Evil/Name"),
            Path::new("/themes/vscode..._evil_name.json")
        );
    }

    /// Every string has every translation, with the same placeholders.
    #[test]
    fn every_locale_translates_every_string() {
        use std::collections::{BTreeMap, BTreeSet};

        fn messages(locale: &str) -> BTreeMap<String, String> {
            crate::_rust_i18n_backend()
                .messages_for_locale(locale)
                .unwrap_or_default()
                .into_iter()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect()
        }
        fn placeholders(text: &str) -> BTreeSet<&str> {
            text.split("%{")
                .skip(1)
                .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
                .collect()
        }

        let english = messages("en");
        assert!(!english.is_empty(), "no English strings");
        let locales = crate::_rust_i18n_backend().available_locales();
        assert!(
            locales.iter().any(|locale| locale == "sv"),
            "no Swedish strings"
        );
        let mut failures = Vec::new();
        for locale in locales {
            if locale == "en" {
                continue;
            }
            let translation = messages(&locale);
            for (key, text) in &english {
                match translation.get(key) {
                    None => failures.push(format!("{locale}: missing {key}")),
                    Some(translated) if placeholders(translated) != placeholders(text) => {
                        failures.push(format!("{locale}: {key} has other %{{}} placeholders"))
                    }
                    Some(_) => {}
                }
            }
            for key in translation.keys().filter(|key| !english.contains_key(*key)) {
                failures.push(format!("en: missing {key}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
