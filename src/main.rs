mod app;
mod app_database;
mod assets;
mod connection;
mod connection_modal;
mod connection_sidebar;
mod database;
mod db_service;
mod editor_panel;
mod gpui_tokio;
mod postgres;
mod results_panel;
mod settings;
mod sidebar;
mod test_db;
mod theme_loader;

use assets::Assets;
use db_service::DbService;
use gpui::{px, size, AppContext, Application, WindowBounds, WindowOptions};

fn main() {
    env_logger::init();

    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        gpui_tokio::init(cx);

        // Load and apply the One Dark theme (converted from Zed format)
        if let Err(e) = theme_loader::load_and_apply_theme("themes/one-dark-darkened-converted.json", cx) {
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

        // Initialize app database (for query tabs, history, connections)
        let app_db_handle = db_service.app_db_handle();
        gpui_tokio::Tokio::spawn_result(cx, async move {
            match app_database::AppDatabase::new().await {
                Ok(db) => {
                    let mut app_db = app_db_handle.write().await;
                    *app_db = Some(db);
                    log::info!("App database initialized");
                    Ok(())
                }
                Err(e) => {
                    log::error!("Failed to initialize app database: {}", e);
                    Err(anyhow::anyhow!("App database init failed: {}", e))
                }
            }
        })
        .detach();

        // Initialize test database using the global tokio runtime
        let user_db_handle = db_service.user_db_handle();
        gpui_tokio::Tokio::spawn_result(cx, async move {
            let mut user_db = user_db_handle.write().await;
            match test_db::init_test_database(&mut *user_db).await {
                Ok(_) => {
                    log::info!("Connected to test database");
                    Ok(())
                }
                Err(e) => {
                    log::error!("Failed to initialize test database: {}", e);
                    Err(anyhow::anyhow!("Test database init failed: {}", e))
                }
            }
        })
        .detach();

        // Initialize PostgreSQL connection from pg_dsn.txt using new connection management
        let pg_dsn_path = std::path::Path::new("pg_dsn.txt");
        if pg_dsn_path.exists() {
            if let Ok(dsn) = std::fs::read_to_string(pg_dsn_path) {
                let dsn = dsn.trim().to_string();
                if !dsn.is_empty() {
                    let db_service_clone = db_service.clone();
                    gpui_tokio::Tokio::spawn_result(cx, async move {
                        match db_service_clone.get_or_create_pg_connection(&dsn).await {
                            Ok(_) => {
                                log::info!("Connected to PostgreSQL database using new connection management");
                                Ok(())
                            }
                            Err(e) => {
                                log::error!("Failed to connect to PostgreSQL: {}", e);
                                Err(anyhow::anyhow!("PostgreSQL connection failed: {}", e))
                            }
                        }
                    })
                    .detach();
                }
            }
        }

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
