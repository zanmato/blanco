#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tikv_jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

mod agent;
mod app;
mod app_settings;
mod assets;
mod command_palette;
mod connection_credentials;
#[cfg(test)]
mod connection_integration_test;
mod connection_modal;
mod connections;
mod copy_handler;
mod editor;
mod export;
mod history_panel;
mod import;
mod keybindings;
mod logging;
mod mcp;
mod redis_completion;
mod redis_syntax;
mod result_ext;
mod results_panel;
mod script_completion;
mod settings;
mod snippets_panel;
mod sql;
mod status_bar;
#[cfg(test)]
mod test_harness;
mod time_format;

use assets::Assets;
use database::DatabaseService;
use gpui::{AppContext, AssetSource as _, SharedString, WindowBounds, WindowOptions, px, size};
use gpui_component::{Theme, ThemeRegistry};
use gpui_platform::application;
use std::path::PathBuf;

use crate::{app_settings::AppSettings, settings::Settings};
use app_database::AppDatabase;

fn main() {
    // Honour `RUST_LOG` when set, otherwise default to a useful baseline so
    // plain runs still surface our own info-level lines without drowning in
    // dependency chatter.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("blanco=info,warn"));
    let log_file = logging::open_log_file();
    let subscriber = tracing_subscriber::fmt().with_env_filter(env_filter);
    match log_file {
        Some(file) => subscriber
            .with_writer(logging::StderrAndFile::new(file))
            .with_ansi(false)
            .init(),
        None => subscriber.init(),
    }
    logging::install_panic_hook();
    let app = application()
        .with_quit_mode(gpui::QuitMode::LastWindowClosed)
        .with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init(cx);
        command_palette::CommandPalette::init(cx);
        sql::register_languages();
        redis_syntax::register_language();

        // Get the tokio runtime handle for automatic SSH tunnel establishment
        let runtime_handle = gpui_tokio::Tokio::handle(cx);

        // Initialize app database (for query tabs, history, connections) synchronously.
        // sqlx is built with the `runtime-tokio` feature, so all of its work
        // must be driven on the shared tokio runtime; smol's executor would
        // panic with "this functionality requires a Tokio context".
        let db = runtime_handle.block_on({
            let runtime_handle = runtime_handle.clone();
            async move { AppDatabase::new(runtime_handle).await }
        });

        match db {
            Ok(database) => cx.set_global(database),
            Err(e) => {
                tracing::error!("Critical: Failed to initialize database: {}", e);
                return;
            }
        }

        let app_database = AppDatabase::global(cx).clone();
        let legacy_credentials = runtime_handle.block_on({
            let app_database = app_database.clone();
            async move { app_database.load_legacy_connection_credentials().await }
        });
        if let Ok(legacy_credentials) = legacy_credentials {
            match smol::block_on(connection_credentials::migrate_legacy_credentials(
                &legacy_credentials,
                cx,
            )) {
                Ok(()) => {
                    if let Err(error) = runtime_handle.block_on(async move {
                        app_database
                            .drop_legacy_connection_credential_columns()
                            .await
                    }) {
                        tracing::error!(
                            "Failed to remove legacy plaintext credential columns: {error}"
                        );
                    }
                }
                Err(error) => {
                    tracing::error!(
                        "Failed to migrate connection credentials to secure storage: {error}"
                    );
                }
            }
        } else if let Err(error) = legacy_credentials {
            tracing::error!("Failed to read legacy connection credentials: {error}");
        }

        // Initialize database service with tokio runtime handle for automatic SSH tunnel establishment
        let db_service = DatabaseService::new(runtime_handle.clone());
        cx.set_global(db_service);
        mcp::McpService::init(cx);

        // Load connections from app database and add them to the database service
        let app_database = AppDatabase::global(cx).clone();
        let connections = runtime_handle
            .block_on(async move { app_database.load_connections().await })
            .unwrap_or_default();
        let connections =
            match smol::block_on(connection_credentials::hydrate_connections(connections, cx)) {
                Ok(connections) => connections,
                Err(error) => {
                    tracing::error!("Failed to load connection credentials: {error}");
                    Vec::new()
                }
            };

        for connection in connections {
            if let Some(config) = connection.to_connection_config() {
                let db_service = DatabaseService::global(cx).clone();
                cx.spawn(async move |_| {
                    db_service.add_connection_config(config).await;
                })
                .detach();
            }
        }

        let app_database = AppDatabase::global(cx).clone();
        let settings = runtime_handle
            .block_on(async move { app_database.load_all_settings().await })
            .unwrap_or_default();

        let app_settings = AppSettings::new(cx, Settings::from_key_values(&settings));
        blanco_core::set_connect_timeout_secs(
            app_settings
                .settings
                .database
                .default_connection_timeout_seconds as u64,
        );
        cx.set_global(app_settings);

        // Apply theme
        let theme_name = SharedString::from(&AppSettings::global(cx).settings.appearance.theme);
        if let Err(err) = ThemeRegistry::watch_dir(PathBuf::from("./themes"), cx, move |cx| {
            // `ThemeRegistry::reload` clears the registry and repopulates it
            // from the watched directory alone, and it runs just before this
            // callback, so the embedded themes have to be (re)loaded here
            // rather than once at startup. `load_themes_from_str` skips names
            // that are already present, so a `./themes/*.json` on disk still
            // wins and hot-reload keeps working during development.
            load_embedded_themes(cx);
            if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
                let mode = theme.mode;
                Theme::global_mut(cx).apply_config(&theme);
                // `apply_config` only touches the gpui-component theme. `change`
                // re-applies the config we just stored and pushes it into the
                // base layer (scrollbars, resize handles, input frame), which
                // otherwise keeps the theme installed by `gpui_component::init`.
                Theme::change(mode, None, cx);
            }
            settings::apply_font_settings(cx);
        }) {
            tracing::error!("Failed to watch themes directory: {}", err);
        }

        settings::apply_font_settings(cx);

        if AppSettings::global(cx).settings.mcp.enabled {
            mcp::McpService::start(cx);
        }

        cx.set_text_rendering_mode(gpui::TextRenderingMode::Subpixel);

        // Restore the window geometry saved on the last run
        let window_bounds = {
            let window = &AppSettings::global(cx).settings.window;
            match (window.width, window.height) {
                (Some(width), Some(height)) => {
                    let bounds = gpui::Bounds {
                        origin: gpui::point(
                            px(window.x.unwrap_or_default()),
                            px(window.y.unwrap_or_default()),
                        ),
                        size: size(px(width), px(height)),
                    };
                    if window.maximized {
                        WindowBounds::Maximized(bounds)
                    } else {
                        WindowBounds::Windowed(bounds)
                    }
                }
                _ => WindowBounds::Windowed(gpui::Bounds::centered(
                    None,
                    size(px(1400.), px(900.)),
                    cx,
                )),
            }
        };

        let window_options = WindowOptions {
            window_bounds: Some(window_bounds),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Blanco".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::Point {
                    x: px(8.0),
                    y: px(6.0),
                }),
            }),
            window_decorations: Some(gpui::WindowDecorations::Client),
            window_min_size: Some(size(px(1024.), px(768.))),
            focus: true,
            show: true,
            kind: gpui::WindowKind::Normal,
            is_movable: true,
            app_owns_titlebar_drag: true,
            // Matches gpui's own default: throttle to 30fps while inactive.
            inactive_frame_interval: Some(std::time::Duration::from_micros(33_333)),
            is_minimizable: true,
            is_resizable: true,
            tabbing_identifier: None,
            display_id: None,
            window_background: gpui::WindowBackgroundAppearance::Opaque,
            app_id: Some("com.blanco.sql-editor".into()),
            icon: None,
        };

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                let blanco_app = cx.new(|cx| app::BlancoApp::new(window, cx));
                cx.new(|cx| gpui_component::Root::new(blanco_app, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();

        cx.activate(true);
    });
}

/// Register the themes compiled into the binary, so an installed build has the
/// full set without needing a `themes/` directory next to the working
/// directory.
fn load_embedded_themes(cx: &mut gpui::App) {
    let paths = match Assets.list("themes/") {
        Ok(paths) => paths,
        Err(error) => {
            tracing::error!("Failed to list embedded themes: {error}");
            return;
        }
    };

    for path in paths {
        let content = match Assets.load(&path) {
            Ok(Some(content)) => content,
            Ok(None) => continue,
            Err(error) => {
                tracing::error!("Failed to read embedded theme {path}: {error}");
                continue;
            }
        };
        let content = match std::str::from_utf8(&content) {
            Ok(content) => content,
            Err(error) => {
                tracing::error!("Embedded theme {path} is not valid UTF-8: {error}");
                continue;
            }
        };
        if let Err(error) = ThemeRegistry::global_mut(cx).load_themes_from_str(content) {
            tracing::error!("Failed to load embedded theme {path}: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The themes are only reachable in an installed build if they are embedded
    /// in the binary; before they lived at the repo root and resolved through
    /// the working directory, so a packaged build silently fell back to
    /// gpui-component's built-in theme.
    #[test]
    fn embedded_themes_are_registered_and_parse() {
        let paths = Assets.list("themes/").expect("themes should be embedded");
        assert!(
            paths.iter().any(|p| p.as_ref() == "themes/catppuccin.json"),
            "catppuccin.json missing from the embedded assets, got {paths:?}"
        );

        let mut names = Vec::new();
        for path in &paths {
            let bytes = Assets
                .load(path)
                .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
                .unwrap_or_else(|| panic!("{path} has no content"));
            let parsed: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{path} is not valid JSON: {error}"));
            for theme in parsed["themes"]
                .as_array()
                .unwrap_or_else(|| panic!("{path} has no themes array"))
            {
                names.push(theme["name"].as_str().unwrap_or_default().to_string());
                // Cards are drawn against these, and a misspelled key is
                // silently ignored by the theme loader.
                for key in ["background", "sidebar.background", "border"] {
                    assert!(
                        theme["colors"][key].is_string(),
                        "{path}: {} is missing {key}",
                        theme["name"]
                    );
                }
            }

            // Parsing as JSON only proves the file is well formed. The loader
            // ignores keys it does not recognise, so a token spelled the way
            // Zed spells it (`link.foreground` for `link`) parses cleanly and
            // then silently falls back to gpui-component's default theme.
            // Deserializing into the loader's own type is what catches that.
            let theme_set: gpui_component::ThemeSet = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{path} does not match the theme schema: {error}"));
            for theme in &theme_set.themes {
                let colors = &theme.colors;
                assert!(
                    colors.background.is_some()
                        && colors.title_bar.is_some()
                        && colors.status_bar.is_some()
                        && colors.tab_bar.is_some()
                        && colors.sidebar.is_some()
                        && colors.table.is_some()
                        && colors.table_head.is_some()
                        && colors.link.is_some(),
                    "{path}: {} leaves load-bearing colors unset",
                    theme.name
                );
            }
        }

        for expected in [
            "Catppuccin Latte",
            "Catppuccin Frappe",
            "Catppuccin Macchiato",
            "Catppuccin Mocha",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "{expected} missing from the embedded themes, got {names:?}"
            );
        }
    }
}
