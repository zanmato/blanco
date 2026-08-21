use crate::settings::Settings;
use gpui::{App, Global, Task};

pub struct AppSettings {
    pub settings: Settings,
}

impl Global for AppSettings {}

impl AppSettings {
    pub fn new(cx: &mut App, settings: Settings) -> Self {
        // List of secret settings that should be loaded from credentials
        let secret_keys = vec!["chat.api_key"];
        #[allow(clippy::type_complexity)]
        let mut tasks: Vec<Task<Result<Option<(String, Vec<u8>)>, anyhow::Error>>> = Vec::new();

        // Create a task for each secret setting
        for key in &secret_keys {
            let task = cx.read_credentials(format!("blanco://{key}").as_str());
            tasks.push(task);
        }

        // Spawn a task to load all secret settings
        let mut loaded_settings = settings.clone();
        cx.spawn(async move |cx| {
            // Await all credential loading tasks
            for task in tasks {
                let (key, value) = match task.await {
                    Ok(Some(credential)) => credential,
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::error!("Failed to read secret setting from keychain: {error}");
                        continue;
                    }
                };
                let value_str = match String::from_utf8(value) {
                    Ok(value_str) => value_str,
                    Err(error) => {
                        tracing::error!("Secret setting '{key}' is not valid UTF-8: {error}");
                        continue;
                    }
                };
                match key.as_str() {
                    "chat.api_key" => {
                        loaded_settings.chat.api_key = value_str;
                    }
                    _ => {
                        tracing::warn!("Unknown secret setting key: {}", key);
                    }
                }
            }

            cx.update(|cx| {
                Self::global_mut(cx).settings = loaded_settings;
            });
        })
        .detach();

        Self { settings }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }
}
