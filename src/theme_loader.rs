use anyhow::{anyhow, Result};
use gpui::{App, AssetSource};
use std::rc::Rc;

pub fn load_and_apply_theme(theme_path: &str, cx: &mut App) -> Result<()> {
    // Load the theme file from assets
    let theme_bytes = cx
        .asset_source()
        .load(theme_path)
        .map_err(|e| anyhow!("Failed to load theme file: {}", e))?
        .ok_or_else(|| anyhow!("Theme file not found: {}", theme_path))?;

    let theme_json = String::from_utf8(theme_bytes.to_vec())
        .map_err(|e| anyhow!("Failed to parse theme file as UTF-8: {}", e))?;

    // Parse the theme using gpui-component's ThemeSet structure
    let theme_set: gpui_component::theme::ThemeSet = serde_json::from_str(&theme_json)
        .map_err(|e| anyhow!("Failed to parse theme JSON: {}", e))?;

    if theme_set.themes.is_empty() {
        return Err(anyhow!("Theme file has no themes"));
    }

    // Get the first theme from the set
    let theme_config = &theme_set.themes[0];

    println!("✓ Loaded theme: {} ({})", theme_config.name,
        if theme_config.mode.is_dark() { "dark" } else { "light" });

    // Apply the theme using gpui-component's built-in apply_config method
    // This handles all color mapping with proper fallbacks
    gpui_component::theme::Theme::global_mut(cx)
        .apply_config(&Rc::new(theme_config.clone()));

    // Apply the theme mode
    gpui_component::theme::Theme::change(theme_config.mode, None, cx);

    println!("✓ Applied {} theme to Blanco", theme_config.name);

    Ok(())
}

pub fn list_available_themes(cx: &App) -> Result<Vec<String>> {
    let theme_files = cx
        .asset_source()
        .list("themes")
        .map_err(|e| anyhow!("Failed to list themes: {}", e))?;

    let themes: Vec<String> = theme_files
        .iter()
        .filter_map(|path| {
            if path.ends_with(".json") {
                Some(path.to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(themes)
}
