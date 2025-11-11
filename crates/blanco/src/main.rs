mod agent;
mod app;
mod app_database;
mod app_events;
mod assets;
mod chat_provider_resolver;
mod connection;
mod connection_modal;
mod connection_sidebar;
mod db_service;
mod editor_panel;
mod rename_form;
mod results_panel;
mod settings;
mod sidebar;
mod sql_completion;
mod sql_completion_provider;
mod theme_loader;
mod time_format;
mod transformers;

pub use sidebar::ConnectionSidebar;

use assets::Assets;
use db_service::DbService;
use gpui::{px, size, AppContext, Application, WindowBounds, WindowOptions};
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

        // Load and apply the One Dark theme (converted from Zed format)
        if let Err(e) =
            theme_loader::load_and_apply_theme("themes/one-dark-darkened-converted.json", cx)
        {
            eprintln!("Failed to load theme: {}", e);
        }

        // Load Fira Code fonts
        let font_paths = cx.asset_source().list("fonts/fira-code").unwrap();
        let mut embedded_fonts = Vec::new();
        for font_path in font_paths {
            if font_path.ends_with(".ttf") {
                let font_bytes = cx
                    .asset_source()
                    .load(&font_path)
                    .ok()
                    .flatten()
                    .map(|bytes| bytes.to_vec());
                if let Some(bytes) = font_bytes {
                    embedded_fonts.push(bytes.into());
                }
            }
        }
        cx.text_system().add_fonts(embedded_fonts).unwrap();

        // Initialize database service
        let db_service = DbService::new();

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

        cx.open_window(window_options, |window, cx| {
            let blanco_app = cx.new(|cx| app::BlancoApp::new(window, cx));
            cx.new(|cx| gpui_component::Root::new(blanco_app.into(), window, cx))
        })
        .expect("Failed to open window");
    });
}
