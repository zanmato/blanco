#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tikv_jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

mod agent;
mod app;
mod app_database;
mod app_settings;
mod assets;
mod command_palette;
#[cfg(test)]
mod connection_integration_test;
mod connection_modal;
mod connections;
mod editor;
mod export;
mod history_panel;
mod import;
mod keybindings;
mod result_ext;
mod results_panel;
mod settings;
mod snippets_panel;
mod sql;
mod status_bar;
#[cfg(test)]
mod test_harness;
mod time_format;
mod transformers;

use assets::Assets;
use database::DatabaseService;
use gpui::{AppContext, SharedString, WindowBounds, WindowOptions, px, size};
use gpui_component::{Theme, ThemeRegistry};
use gpui_platform::application;
use std::path::PathBuf;

use crate::{app_database::AppDatabase, app_settings::AppSettings, settings::Settings};

fn main() {
    // Honour `RUST_LOG` when set, otherwise default to a useful baseline so
    // plain runs still surface our own info-level lines without drowning in
    // dependency chatter.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("blanco=info,warn"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
    let app = application()
        .with_quit_mode(gpui::QuitMode::LastWindowClosed)
        .with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init(cx);
        command_palette::CommandPalette::init(cx);
        sql::register_languages();

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

        // Initialize database service with tokio runtime handle for automatic SSH tunnel establishment
        let db_service = DatabaseService::new(runtime_handle.clone());
        cx.set_global(db_service);

        // Load connections from app database and add them to the database service
        let app_database = AppDatabase::global(cx).clone();
        let connections = runtime_handle
            .block_on(async move { app_database.load_connections().await })
            .unwrap_or_default();

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
            if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
                Theme::global_mut(cx).apply_config(&theme);
            }
            settings::apply_font_settings(cx);
        }) {
            tracing::error!("Failed to watch themes directory: {}", err);
        }

        settings::apply_font_settings(cx);

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
