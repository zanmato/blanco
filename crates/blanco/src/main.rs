mod agent;
mod app;
mod app_database;
mod app_events;
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
mod sql_completion_provider;
mod sql_statement_parser;
mod ssh_tunnel;
mod time_format;
mod transformers;

use assets::Assets;
use db_service::DbService;
use gpui::{AppContext, Application, SharedString, WindowBounds, WindowOptions, px, size};
use gpui_component::{Theme, ThemeRegistry};
use gpui_tokio;
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt as _};

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
        gpui_tokio::init(cx);

        // Get the tokio runtime handle for automatic SSH tunnel establishment
        let runtime_handle = gpui_tokio::Tokio::handle(cx);

        // Load and watch themes from ./themes directory
        let settings = settings::load_settings().unwrap();
        let theme_name = SharedString::from(settings.appearance.theme);
        if let Err(err) = ThemeRegistry::watch_dir(PathBuf::from("./themes"), cx, move |cx| {
            log::info!("themes {:?}", ThemeRegistry::global(cx).themes());
            if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
                Theme::global_mut(cx).apply_config(&theme);
                log::info!("Applying theme {}", theme_name);
            }
        }) {
            log::error!("Failed to watch themes directory: {}", err);
        }

        // Initialize database service with tokio runtime handle for automatic SSH tunnel establishment
        let db_service = DbService::new(Some(runtime_handle));

        // Initialize app database (for query tabs, history, connections) synchronously
        let app_db_handle = db_service.app_db_handle();
        let db = async_std::task::block_on(async {
            match app_database::AppDatabase::new().await {
                Ok(db) => {
                    let mut app_db = app_db_handle.write().await;
                    *app_db = Some(db);
                    log::info!("App database initialized");

                    // Drop the write lock before migration
                    drop(app_db);

                    Ok(())
                }
                Err(e) => {
                    log::error!("Failed to initialize app database: {}", e);
                    Err(anyhow::anyhow!("App database init failed: {}", e))
                }
            }
        });

        if let Err(e) = db {
            log::error!("Critical: Failed to initialize database: {}", e);
        }

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
