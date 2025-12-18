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
mod connections_panel_delegate;
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
mod time_format;
mod transformers;

use assets::Assets;
use database::{ConnectionConfig, DatabaseService, DatabaseType};
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
            .with(tracing_subscriber::EnvFilter::from_default_env())
            .init();

        gpui_component::init(cx);
        gpui_tokio::init(cx);

        // Get the tokio runtime handle for automatic SSH tunnel establishment
        let runtime_handle = gpui_tokio::Tokio::handle(cx);

        // Use default theme for now
        // Settings will be loaded asynchronously
        let theme_name = SharedString::from("One Dark - Darkened");
        if let Err(err) = ThemeRegistry::watch_dir(PathBuf::from("./themes"), cx, move |cx| {
            if let Some(theme) = ThemeRegistry::global(cx).themes().get(&theme_name).cloned() {
                Theme::global_mut(cx).apply_config(&theme);
                tracing::info!("Applying theme {}", theme_name);
            }
        }) {
            tracing::error!("Failed to watch themes directory: {}", err);
        }

        // Initialize app database (for query tabs, history, connections) synchronously
        let db = async_std::task::block_on(async { AppDatabase::new().await });

        if let Err(e) = db {
            tracing::error!("Critical: Failed to initialize database: {}", e);
        } else {
            cx.set_global(db.unwrap());
        }

        // Initialize database service with tokio runtime handle for automatic SSH tunnel establishment
        let db_service = DatabaseService::new(runtime_handle.clone());
        cx.set_global(db_service);

        // Load connections from app database and add them to the database service
        let app_database = AppDatabase::global(cx).clone();
        let connections =
            async_std::task::block_on(async move { app_database.load_connections().await })
                .unwrap_or(Vec::new());

        for connection in connections {
            if let Some(connection_id) = connection.id {
                // Convert ConnectionData to ConnectionConfig
                let db_type = match connection.db_type.as_str() {
                    "SQLite" => DatabaseType::SQLite,
                    "PostgreSQL" => DatabaseType::PostgreSQL,
                    "MySQL" => DatabaseType::MySQL,
                    _ => DatabaseType::PostgreSQL, // Default to PostgreSQL
                };

                let connection_config = match db_type {
                    DatabaseType::SQLite => ConnectionConfig::new_sqlite(
                        connection_id,
                        connection.name.clone(),
                        connection
                            .database_path
                            .unwrap_or_else(|| format!("{}.db", connection.name)),
                    ),
                    _ => {
                        let default_port = match db_type {
                            DatabaseType::PostgreSQL => 5432,
                            DatabaseType::MySQL => 3306,
                            DatabaseType::SQLite => 0,
                        };

                        let mut config = ConnectionConfig::new(
                            connection_id,
                            connection.name.clone(),
                            db_type,
                            connection.host.unwrap_or_else(|| "localhost".to_string()),
                            connection.port.unwrap_or(default_port) as u16,
                            connection.database_name.unwrap_or_else(|| "".to_string()),
                            connection.username.unwrap_or_else(|| "".to_string()),
                            connection.password,
                        );

                        // Add SSH configuration if present
                        if let Some(ssh_host) = connection.ssh_host {
                            if let Some(ssh_user) = connection.ssh_user {
                                config = config.with_ssh_config(
                                    ssh_host,
                                    ssh_user,
                                    connection.ssh_password,
                                    connection.ssh_private_key_path,
                                    connection.ssh_private_key_password,
                                    connection.ssh_port,
                                );
                            }
                        }

                        config
                    }
                };

                let db_service = DatabaseService::global(cx).clone();
                cx.spawn(async move |_| {
                    db_service.add_connection_config(connection_config).await;
                })
                .detach();
            }
        }

        let app_database = AppDatabase::global(cx).clone();
        let settings =
            async_std::task::block_on(async move { app_database.load_all_settings().await })
                .unwrap_or(Vec::new());

        let app_settings = AppSettings::new(cx, Settings::from_key_values(&settings));
        cx.set_global(app_settings);

        // Store the database service globally for components to use
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
