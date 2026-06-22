//! Chat Provider Resolver
//!
//! This module is responsible for creating and managing LLM instances
//! based on the current application settings. It uses the llm crate's LLMBuilder
//! to create LLM instances for different providers (OpenAI, Anthropic, Google, Ollama).

use anyhow::Result;
use std::sync::Arc;

use crate::agent::openai_compatible::CompatibleProvider;
use crate::agent::streaming::{NonStreamingAdapter, StreamingChatProvider};
use crate::settings::{ChatSettings, Settings};
use llm::{builder::LLMBackend, builder::LLMBuilder};

/// LLM instance with metadata
#[derive(Clone)]
pub struct LLMInstance {
    pub llm: Arc<dyn StreamingChatProvider>,
    pub provider_name: String,
    pub model_name: String,
}

/// Chat Provider Resolver
///
/// This resolver handles the creation and caching of LLM instances
/// based on current application settings. It can handle runtime changes
/// to settings and will recreate instances when necessary.
#[derive(Default)]
pub struct ChatProviderResolver {
    cached_llm: Option<(LLMInstance, u64)>,
}

impl ChatProviderResolver {
    /// Get an LLM instance based on current settings
    ///
    /// This method will:
    /// 1. Check if we have a cached LLM for the current configuration
    /// 2. Create a new LLM instance if settings have changed or no cache exists
    /// 3. Return the LLM instance for use in chat sessions
    pub fn get_llm(&mut self, settings: &Settings) -> Result<LLMInstance> {
        let config_hash = self.calculate_config_hash(&settings.chat);

        // Check if we can reuse the cached LLM
        if let Some((cached, hash)) = &self.cached_llm
            && *hash == config_hash
        {
            return Ok(cached.clone());
        }

        // Create new LLM instance based on settings
        let llm_instance = self.create_llm_from_settings(&settings.chat)?;

        // Cache the LLM instance
        self.cached_llm = Some((llm_instance.clone(), config_hash));

        Ok(llm_instance)
    }

    /// Create an LLM instance based on chat settings
    fn create_llm_from_settings(&self, chat_settings: &ChatSettings) -> Result<LLMInstance> {
        if chat_settings.api_key.is_empty() {
            return Err(anyhow::anyhow!(
                "API key is required for {}",
                chat_settings.provider
            ));
        }

        match chat_settings.provider.to_lowercase().as_str() {
            // OpenAI and every OpenAI-compatible endpoint (z.AI, local proxies, ...) go
            // through the `/chat/completions` wrapper. The `llm` crate's native OpenAI
            // backend now targets the `/responses` endpoint, which these servers do not
            // implement.
            "openai" => {
                // Streams `/chat/completions` directly so reasoning models' `reasoning_content`
                // and incremental output reach the UI. Retries on the initial request are
                // handled inside the provider.
                let provider = CompatibleProvider::new(
                    &chat_settings.api_key,
                    Self::resolve_base_url(&chat_settings.base_url),
                    Some(chat_settings.model.clone()),
                    Some(chat_settings.max_tokens),
                    Some(chat_settings.temperature),
                    None,
                );

                Ok(LLMInstance {
                    llm: Arc::new(provider),
                    provider_name: "OpenAI Compatible".to_string(),
                    model_name: chat_settings.model.clone(),
                })
            }
            // Native backends that use their own (non-OpenAI) protocols.
            other => {
                let (backend, provider_name) = match other {
                    "anthropic" => (LLMBackend::Anthropic, "Anthropic"),
                    "google" => (LLMBackend::Google, "Google"),
                    "ollama" => (LLMBackend::Ollama, "Ollama"),
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Unsupported chat provider: {}",
                            chat_settings.provider
                        ));
                    }
                };

                let mut builder = LLMBuilder::new()
                    .backend(backend)
                    .api_key(&chat_settings.api_key)
                    .model(&chat_settings.model)
                    .max_tokens(chat_settings.max_tokens)
                    .temperature(chat_settings.temperature)
                    // Retry transient failures (network blips, 429/5xx) with exponential
                    // backoff so a single hiccup does not fail the whole chat turn. Kept in
                    // sync with the compatible provider's own retry policy.
                    .resilient(true)
                    .resilient_attempts(3)
                    .resilient_backoff(200, 2000)
                    .resilient_jitter(true);

                if let Some(base_url) = Self::resolve_base_url(&chat_settings.base_url) {
                    builder = builder.base_url(base_url);
                }

                // Native backends only expose a blocking `chat_with_tools`; adapt it to the
                // streaming interface so the session loop is uniform across providers.
                let provider: Arc<dyn StreamingChatProvider> =
                    Arc::new(NonStreamingAdapter::new(Arc::new(builder.build()?)));

                Ok(LLMInstance {
                    llm: provider,
                    provider_name: provider_name.to_string(),
                    model_name: chat_settings.model.clone(),
                })
            }
        }
    }

    /// Normalize the configured base URL into an explicit override, or `None` to let the
    /// provider fall back to its default. The legacy default lacked the `/v1` suffix that
    /// the chat-completions endpoint requires, so it is upgraded here for existing configs.
    fn resolve_base_url(base_url: &str) -> Option<String> {
        match base_url.trim() {
            "" => None,
            "https://api.openai.com" => Some("https://api.openai.com/v1".to_string()),
            other => Some(other.to_string()),
        }
    }

    /// Calculate a configuration hash for caching purposes
    fn calculate_config_hash(&self, chat_settings: &ChatSettings) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        chat_settings.provider.hash(&mut hasher);
        chat_settings.model.hash(&mut hasher);
        chat_settings.api_key.hash(&mut hasher);
        chat_settings.base_url.hash(&mut hasher);
        chat_settings.max_tokens.hash(&mut hasher);
        chat_settings.temperature.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    /// Get validation errors for current settings
    pub fn validate_settings(settings: &Settings) -> Vec<String> {
        let mut errors = Vec::new();
        let chat = &settings.chat;

        if chat.provider.is_empty() {
            errors.push("Chat provider is required".to_string());
        }

        if chat.model.is_empty() {
            errors.push("Chat model is required".to_string());
        }

        if chat.api_key.is_empty() {
            errors.push("API key is required".to_string());
        }

        if chat.max_tokens == 0 {
            errors.push("Max tokens must be greater than 0".to_string());
        }

        if !(0.0..=2.0).contains(&chat.temperature) {
            errors.push("Temperature must be between 0.0 and 2.0".to_string());
        }

        errors
    }

    /// Get an LLM instance based on current application settings
    ///
    /// This static method validates settings, creates a resolver,
    /// and returns the LLM instance in one step.
    pub fn get_llm_for_connection(cx: &mut gpui::App) -> Result<LLMInstance> {
        use crate::app_settings::AppSettings;

        // Get settings
        let app_settings = AppSettings::global(cx);

        // Validate settings
        let validation_errors = Self::validate_settings(&app_settings.settings);
        if !validation_errors.is_empty() {
            return Err(anyhow::anyhow!(
                "Invalid chat settings: {}",
                validation_errors.join(", ")
            ));
        }

        // Create resolver and get LLM instance
        Self::default().get_llm(&app_settings.settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ChatSettings;

    #[test]
    fn test_config_hash_different_for_different_settings() {
        let settings1 = ChatSettings {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api_key: "key1".to_string(),
            base_url: "https://api.openai.com".to_string(),
            max_tokens: 1000,
            temperature: 0.7,
            auto_execute_queries: false,
            show_thinking_process: false,
        };

        let settings2 = ChatSettings {
            provider: "openai".to_string(),
            model: "gpt-3.5-turbo".to_string(), // Different model
            api_key: "key1".to_string(),
            base_url: "https://api.openai.com".to_string(),
            max_tokens: 1000,
            temperature: 0.7,
            auto_execute_queries: false,
            show_thinking_process: false,
        };

        // Test hash calculation directly
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        settings1.provider.hash(&mut hasher);
        settings1.model.hash(&mut hasher);
        settings1.api_key.hash(&mut hasher);
        settings1.base_url.hash(&mut hasher);
        settings1.max_tokens.hash(&mut hasher);
        settings1.temperature.to_bits().hash(&mut hasher);
        let hash1 = hasher.finish();

        let mut hasher = DefaultHasher::new();
        settings2.provider.hash(&mut hasher);
        settings2.model.hash(&mut hasher);
        settings2.api_key.hash(&mut hasher);
        settings2.base_url.hash(&mut hasher);
        settings2.max_tokens.hash(&mut hasher);
        settings2.temperature.to_bits().hash(&mut hasher);
        let hash2 = hasher.finish();

        assert_ne!(
            hash1, hash2,
            "Hashes should be different for different settings"
        );
    }

    #[test]
    fn test_validate_settings() {
        let mut settings = Settings::default();
        settings.chat.api_key = "".to_string(); // Empty API key

        let errors = ChatProviderResolver::validate_settings(&settings);
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.contains("API key")));

        settings.chat.api_key = "test-key".to_string();
        let errors = ChatProviderResolver::validate_settings(&settings);
        assert!(errors.is_empty(), "Valid settings should not have errors");
    }
}
