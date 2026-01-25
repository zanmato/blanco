//! Chat Provider Resolver
//!
//! This module is responsible for creating and managing LLM instances
//! based on the current application settings. It uses the llm crate's LLMBuilder
//! to create LLM instances for different providers (OpenAI, Anthropic, Google, Ollama).

use anyhow::Result;
use std::sync::Arc;

use crate::settings::{ChatSettings, Settings};
use database::DatabaseService;
use llm::{builder::LLMBuilder, builder::LLMBackend, LLMProvider};

/// LLM instance with metadata
#[derive(Clone)]
pub struct LLMInstance {
    pub llm: Arc<Box<dyn LLMProvider>>,
    pub provider_name: String,
    pub model_name: String,
}

/// Chat Provider Resolver
///
/// This resolver handles the creation and caching of LLM instances
/// based on current application settings. It can handle runtime changes
/// to settings and will recreate instances when necessary.
pub struct ChatProviderResolver {
    db_service: DatabaseService,
    current_connection_id: Option<i64>,
    cached_llm: Option<(LLMInstance, u64)>,
    runtime_handle: tokio::runtime::Handle,
}

impl ChatProviderResolver {
    /// Create a new chat provider resolver
    pub fn new(
        db_service: DatabaseService,
        runtime_handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            db_service,
            current_connection_id: None,
            cached_llm: None,
            runtime_handle,
        }
    }

    /// Set the current connection ID for tool execution
    pub fn set_connection_id(&mut self, connection_id: i64) {
        self.current_connection_id = Some(connection_id);
    }

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
        let backend = match chat_settings.provider.to_lowercase().as_str() {
            "openai" => LLMBackend::OpenAI,
            "anthropic" => LLMBackend::Anthropic,
            "google" => LLMBackend::Google,
            "ollama" => LLMBackend::Ollama,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported chat provider: {}",
                    chat_settings.provider
                ))
            }
        };

        if chat_settings.api_key.is_empty() {
            return Err(anyhow::anyhow!("API key is required for {}", chat_settings.provider));
        }

        let mut builder = LLMBuilder::new()
            .backend(backend.clone())
            .api_key(&chat_settings.api_key)
            .model(&chat_settings.model)
            .max_tokens(chat_settings.max_tokens)
            .temperature(chat_settings.temperature);

        // Set base URL if provided (for custom endpoints)
        if !chat_settings.base_url.is_empty() && chat_settings.base_url != "https://api.openai.com" {
            builder = builder.base_url(&chat_settings.base_url);
        }

        let llm = Arc::new(builder.build()?);

        let provider_name = match backend {
            LLMBackend::OpenAI => "OpenAI",
            LLMBackend::Anthropic => "Anthropic",
            LLMBackend::Google => "Google",
            LLMBackend::Ollama => "Ollama",
            _ => "Unknown",
        };

        Ok(LLMInstance {
            llm,
            provider_name: provider_name.to_string(),
            model_name: chat_settings.model.clone(),
        })
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

    /// Clear the LLM cache (useful for testing or forced refresh)
    #[allow(dead_code)]
    pub fn clear_cache(&mut self) {
        self.cached_llm = None;
    }

    /// Check if a provider is properly configured
    #[allow(dead_code)]
    pub fn is_provider_configured(settings: &Settings) -> bool {
        let chat = &settings.chat;
        !chat.api_key.is_empty() && !chat.provider.is_empty() && !chat.model.is_empty()
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
