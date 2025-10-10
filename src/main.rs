mod app;
mod app_database;
mod connection;
mod database;
mod db_service;
mod editor_panel;
mod results_panel;
mod settings;
mod sidebar;
mod test_db;

use db_service::DbService;
use gpui::{px, size, AppContext, Application, WindowBounds, WindowOptions};

fn main() {
    env_logger::init();

    let app = Application::new();

    // Initialize database service (creates tokio runtime)
    let db_service = DbService::new();

    // Initialize databases in background thread using the DbService runtime
    let db_init = db_service.clone();
    std::thread::spawn(move || {
        // Initialize app database (already uses runtime internally)
        if let Err(e) = db_init.init_app_db() {
            eprintln!("Failed to initialize app database: {}", e);
        }

        // Initialize and connect to test database using the runtime
        db_init.runtime().block_on(async {
            let mut user_db = db_init.user_db.write().await;
            if let Err(e) = test_db::init_test_database(&mut user_db).await {
                eprintln!("Failed to initialize test database: {}", e);
            } else {
                println!("✓ Connected to test database");
            }
        });
    });

    app.run(move |cx| {
        gpui_component::init(cx);
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
