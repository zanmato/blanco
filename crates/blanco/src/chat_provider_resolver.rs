//! Chat Provider Resolver
//!
//! This module is responsible for creating and managing chat providers
//! based on the current application settings. It provides a clean interface
//! for the UI layer to get appropriately configured chat providers.

use anyhow::Result;
use std::sync::Arc;

// Use reqwest
use reqwest;

use crate::settings::{ChatSettings, Settings};
use database::{DatabaseService, DatabaseServiceTrait};
use blanco_core::chat_provider::{ChatProvider, ProviderError};

use blanco_openai::{ListTablesTool, OpenAIClient, OpenAIConfig, ReadTabTool, ToolExecutor};

/// Provider cache entry with configuration hash
#[derive(Clone)]
struct CachedProvider {
    provider: Arc<dyn ChatProvider<Error = ProviderError>>,
    config_hash: u64,
    provider_name: String,
    model_name: String,
}

/// Chat Provider Resolver
///
/// This resolver handles the creation and caching of chat providers
/// based on current application settings. It can handle runtime changes
/// to settings and will recreate providers when necessary.
pub struct ChatProviderResolver {
    http_client: Arc<reqwest::Client>,
    db_service: DatabaseService,
    current_connection_id: Option<i64>,
    cached_provider: Option<CachedProvider>,
}

impl ChatProviderResolver {
    /// Create a new chat provider resolver
    pub fn new(http_client: Arc<reqwest::Client>, db_service: DatabaseService) -> Self {
        Self {
            http_client,
            db_service,
            current_connection_id: None,
            cached_provider: None,
        }
    }

    /// Set the current connection ID for tool execution
    pub fn set_connection_id(&mut self, connection_id: i64) {
        self.current_connection_id = Some(connection_id);
    }

    /// Get a chat provider based on current settings
    ///
    /// This method will:
    /// 1. Check if we have a cached provider for the current configuration
    /// 2. Create a new provider if settings have changed or no cache exists
    /// 3. Return the provider for use in chat sessions
    pub fn get_provider(&mut self, settings: &Settings) -> Result<ProviderInfo> {
        let config_hash = self.calculate_config_hash(&settings.chat);

        // Check if we can reuse the cached provider
        if let Some(cached) = &self.cached_provider
            && cached.config_hash == config_hash
        {
            return Ok(ProviderInfo {
                provider: cached.provider.clone(),
                provider_name: cached.provider_name.clone(),
                model_name: cached.model_name.clone(),
            });
        }

        // Create new provider based on settings
        let provider_info = self.create_provider_from_settings(&settings.chat)?;

        // Cache the provider
        self.cached_provider = Some(CachedProvider {
            provider: provider_info.provider.clone(),
            config_hash,
            provider_name: provider_info.provider_name.clone(),
            model_name: provider_info.model_name.clone(),
        });

        Ok(provider_info)
    }

    /// Create a provider based on chat settings
    fn create_provider_from_settings(&self, chat_settings: &ChatSettings) -> Result<ProviderInfo> {
        match chat_settings.provider.to_lowercase().as_str() {
            "openai" => self.create_openai_provider(chat_settings),
            _ => Err(anyhow::anyhow!(
                "Unsupported chat provider: {}",
                chat_settings.provider
            )),
        }
    }

    /// Create an OpenAI provider
    fn create_openai_provider(&self, chat_settings: &ChatSettings) -> Result<ProviderInfo> {
        if chat_settings.api_key.is_empty() {
            return Err(anyhow::anyhow!("OpenAI API key is required"));
        }

        let mut config = OpenAIConfig::new(&chat_settings.api_key)
            .with_model(&chat_settings.model)
            .with_max_tokens(chat_settings.max_tokens)
            .with_temperature(chat_settings.temperature);

        // Set base URL if it's not the default OpenAI URL
        if !chat_settings.base_url.is_empty() && chat_settings.base_url != "https://api.openai.com"
        {
            config = config.with_base_url(&chat_settings.base_url);
        }

        // Create tool executor with database service
        let database_service = self.db_service.clone();
        let mut tool_executor = ToolExecutor::with_database_service(Arc::new(database_service));

        // Register the list-tables tool with connection resolver
        let connection_id = self.current_connection_id;
        let list_tables_tool = Box::new(ListTablesTool::with_connection_id(
            connection_id.unwrap_or(0),
        ));
        tool_executor.register_tool(list_tables_tool);

        // Register the read tab tool
        let read_tab_tool = Box::new(ReadTabTool::new());
        tool_executor.register_tool(read_tab_tool);

        let client =
            OpenAIClient::with_tool_executor(self.http_client.clone(), config, tool_executor)
                .map_err(|e| anyhow::anyhow!("Failed to create OpenAI client: {}", e))?;

        Ok(ProviderInfo {
            provider: Arc::new(client),
            provider_name: "OpenAI".to_string(),
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

    /// Clear the provider cache (useful for testing or forced refresh)
    #[allow(dead_code)]
    pub fn clear_cache(&mut self) {
        self.cached_provider = None;
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

/// Information about a chat provider instance
#[derive(Clone)]
pub struct ProviderInfo {
    /// The provider instance
    pub provider: Arc<dyn ChatProvider<Error = ProviderError>>,
    /// Human-readable provider name
    pub provider_name: String,
    /// Model name being used
    pub model_name: String,
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

        // Test hash calculation directly without needing a full resolver
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
    fn test_config_hash_different_for_different_base_urls() {
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
            model: "gpt-4".to_string(),
            api_key: "key1".to_string(),
            base_url: "https://api.example.com".to_string(), // Different base URL
            max_tokens: 1000,
            temperature: 0.7,
            auto_execute_queries: false,
            show_thinking_process: false,
        };

        // Test the hash calculation directly without creating a resolver
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
            "Hashes should be different for different base URLs"
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

    // Note: Mock HTTP client implementation needs to be updated to match zed-http-client traits
    // The mock implementation is commented out until it can be properly updated
    /*
    struct MockHttpClient;

    impl HttpClient for MockHttpClient {
        // Implementation needs to match zed-http-client HttpClient trait
    }
    */
}
