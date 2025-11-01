mod app;
mod app_database;
mod app_events;
mod assets;
mod async_pipeline;
mod connection;
mod connection_modal;
mod connection_sidebar;
mod db_service;
mod editor_panel;
mod query_file;
mod results_panel;
mod settings;
mod sidebar;
mod sql_completion;
mod sql_completion_provider;
mod test_db;
mod theme_loader;
mod time_format;
mod transformers;
mod unified_connection_manager;

pub use sidebar::ConnectionSidebar;

// Integration test modules for unified connection interface
// pub mod integration_test; // Disabled for now

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

                    // Run migration for existing tabs without file_uri
                    if let Err(e) = migrate_existing_tabs_to_files(&app_db_handle).await {
                        log::error!("Failed to migrate existing tabs to files: {}", e);
                    }

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

        // Initialize test database
        cx.spawn(async move |cx| {
            match test_db::init_test_database().await {
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
                log::info!("🔍 Loaded DSN from pg_dsn.txt: {}", dsn);
                if !dsn.is_empty() {
                    let db_service_clone = db_service.clone();
                    cx.spawn(async move |cx| {
                        let unified_manager = db_service_clone.unified_manager().await;
                        let result = unified_manager.read().await.get_or_create_connection(&dsn).await;
                        match result {
                            Ok(_) => {
                                log::info!("Connected to PostgreSQL database using unified connection management");
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

        // Initialize async event processor
        let (mut async_processor, async_event_tx) = async_pipeline::AsyncEventProcessor::new(db_service.clone());

        // Start the async processor
        cx.spawn(async move |cx| {
            if let Err(e) = async_processor.start().await {
                log::error!("Failed to start async event processor: {}", e);
            }
        }).detach();

        // Store the async event sender globally for components to use
        cx.set_global(db_service);
        cx.set_global(async_event_tx);
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

/// Migrate existing tabs in the database to file-based storage
/// This creates .sql files on disk for tabs that don't have file_uri set
async fn migrate_existing_tabs_to_files(
    app_db_handle: &std::sync::Arc<async_std::sync::RwLock<Option<app_database::AppDatabase>>>,
) -> Result<(), anyhow::Error> {
    use query_file::QueryFileManager;

    log::info!("🔄 Starting migration of existing tabs to file-based storage");

    // Find tabs that need migration (no file_uri)
    let tabs_needing_migration = {
        let db_guard = app_db_handle.read().await;
        let app_db = db_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("App database not initialized"))?;
        app_db.find_tabs_needing_migration().await?
    };

    if tabs_needing_migration.is_empty() {
        log::info!("✅ No tabs need migration - all tabs already have file_uri");
        return Ok(());
    }

    log::info!(
        "📋 Found {} tabs that need migration",
        tabs_needing_migration.len()
    );

    // Initialize query file manager
    let query_file_manager = QueryFileManager::new()?;

    // Migrate each tab
    let total_tabs = tabs_needing_migration.len();
    let mut migrated_count = 0;
    for tab in tabs_needing_migration {
        let tab_id = tab.id.ok_or_else(|| anyhow::anyhow!("Tab missing ID"))?;

        log::debug!("🔄 Migrating tab '{}' (ID: {}) to file", tab.title, tab_id);

        // Determine connection name for migration
        let connection_name = match tab.connection_type.as_deref() {
            Some("PostgreSQL") => "PostgreSQL",
            _ => "Test Database",
        };

        // Create the query file on disk
        match query_file_manager
            .create_query_file(tab_id, connection_name, &tab.content)
            .await
        {
            Ok(_) => {
                // Get the file URI
                let file_uri = query_file_manager.query_file_uri(tab_id, connection_name);

                // Update the database record with the file URI
                {
                    let db_guard = app_db_handle.read().await;
                    if let Some(app_db) = db_guard.as_ref() {
                        match app_db.update_tab_file_uri(tab_id, &file_uri).await {
                            Ok(_) => {
                                log::info!(
                                    "✅ Migrated tab '{}' (ID: {}) to file: {}",
                                    tab.title,
                                    tab_id,
                                    file_uri
                                );
                                migrated_count += 1;
                            }
                            Err(e) => {
                                log::error!(
                                    "❌ Failed to update file URI for tab '{}': {}",
                                    tab.title,
                                    e
                                );
                            }
                        }
                    } else {
                        log::error!("❌ App database not available for updating tab file URI");
                    }
                }
            }
            Err(e) => {
                log::error!(
                    "❌ Failed to create query file for tab '{}': {}",
                    tab.title,
                    e
                );
            }
        }
    }

    log::info!(
        "🎉 Migration completed: {}/{} tabs migrated to file-based storage",
        migrated_count,
        total_tabs
    );

    Ok(())
}
