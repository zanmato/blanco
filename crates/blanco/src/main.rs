mod agent;
mod app;
mod app_database;
mod app_events;
mod app_settings;
mod assets;
mod chat_provider_resolver;
mod connection;
mod connection_modal;
mod connections_panel;
mod db_service;
mod editor_panel;
mod export_modal;
mod export_service;
mod rename_form;
mod results_panel;
mod settings;
mod settings_view;
mod sql_completion_provider;
mod sql_document_color_provider;
mod sql_statement_parser;
mod ssh_tunnel;
mod time_format;
mod transformers;

use assets::Assets;
use db_service::DbService;
use gpui::{AppContext, Application, SharedString, WindowBounds, WindowOptions, px, size};
use gpui_component::{Theme, ThemeRegistry};
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::{app_database::AppDatabase, app_settings::AppSettings, settings::Settings};

fn main() {
    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer())
            .with(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive("gpui_component=trace".parse().unwrap()),
            )
            .init();

        gpui_component::init(cx);

        // Use default theme for now
        // Settings will be loaded asynchronously
        let theme_name = SharedString::from("One Dark - Darkened");
        if let Err(err) = ThemeRegistry::watch_dir(PathBuf::from("./themes"), cx, move |cx| {
            if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
                Theme::global_mut(cx).apply_config(&theme);
                log::info!("Applying theme {}", theme_name);
            }
        }) {
            log::error!("Failed to watch themes directory: {}", err);
        }

        // Initialize app database (for query tabs, history, connections) synchronously
        let db = async_std::task::block_on(async { AppDatabase::new().await });

        if let Err(e) = db {
            log::error!("Critical: Failed to initialize database: {}", e);
        } else {
            log::info!("Global DB bro!");
            cx.set_global(db.unwrap());
        }

        // Initialize database service with background executor for automatic SSH tunnel establishment
        let app_database = AppDatabase::global(cx).clone();
        let db_service = DbService::new(Some(cx.background_executor().clone()), Some(app_database));

        let app_database = AppDatabase::global(cx).clone();
        let settings =
            async_std::task::block_on(async move { app_database.load_all_settings().await })
                .unwrap_or(Vec::new());

        let app_settings = AppSettings::new(cx, Settings::from_key_values(&settings));
        cx.set_global(app_settings);

        // Store the async event sender globally for components to use
        cx.set_global(db_service);
        cx.activate(true);

        let window_bounds = gpui::Bounds::centered(None, size(px(1400.), px(900.)), cx);

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Blanco - SQL Editor".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::Point {
                    x: px(8.0),
                    y: px(6.0),
                }),
            }),
            window_decorations: Some(gpui::WindowDecorations::Client),
            window_min_size: Some(size(px(800.), px(600.))),
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
        };

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                let blanco_app = cx.new(|cx| app::BlancoApp::new(window, cx));
                cx.new(|cx| gpui_component::Root::new(blanco_app, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
