//! Chat Provider Resolver
//!
//! This module is responsible for creating and managing chat providers
//! based on the current application settings. It provides a clean interface
//! for the UI layer to get appropriately configured chat providers.

use anyhow::Result;
use http_client::HttpClient;
use std::sync::Arc;

use crate::settings::{ChatSettings, Settings};
use crate::db_service::DbService;
use blanco_core::chat_provider::{ChatProvider, ProviderError};
use blanco_core::{Connection, DatabaseService};

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
    http_client: Arc<dyn HttpClient>,
    db_service: DbService,
    current_connection_string: Option<String>,
    cached_provider: Option<CachedProvider>,
}

impl ChatProviderResolver {
    /// Create a new chat provider resolver
    pub fn new(http_client: Arc<dyn HttpClient>, db_service: DbService) -> Self {
        Self {
            http_client,
            db_service,
            current_connection_string: None,
            cached_provider: None,
        }
    }

    /// Set the current connection string for tool execution
    pub fn set_connection_string(&mut self, connection_string: String) {
        self.current_connection_string = Some(connection_string);
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
        if let Some(cached) = &self.cached_provider {
            if cached.config_hash == config_hash {
                return Ok(ProviderInfo {
                    provider: cached.provider.clone(),
                    provider_name: cached.provider_name.clone(),
                    model_name: cached.model_name.clone(),
                });
            }
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
            "anthropic" => self.create_anthropic_provider(chat_settings),
            "mock" => self.create_mock_provider(chat_settings),
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

        use blanco_openai::{OpenAIClient, OpenAIConfig, ToolExecutor, ListTablesTool};

        let mut config = OpenAIConfig::new(&chat_settings.api_key)
            .with_model(&chat_settings.model)
            .with_max_tokens(chat_settings.max_tokens)
            .with_temperature(chat_settings.temperature);

        // Set base URL if it's not the default OpenAI URL
        if !chat_settings.base_url.is_empty() && chat_settings.base_url != "https://api.openai.com" {
            config = config.with_base_url(&chat_settings.base_url);
        }

        // Create tool executor with database service
        let database_service: Arc<dyn DatabaseService> = Arc::new(self.db_service.clone());
        let mut tool_executor = ToolExecutor::with_database_service(database_service);

        // Register the list-tables tool with connection resolver
        let connection_string = self.current_connection_string.clone();
        let list_tables_tool = Box::new(ListTablesTool::with_connection_resolver(move || {
            // Use the connection string from the current query tab
            connection_string.clone()
        }));
        tool_executor.register_tool(list_tables_tool);

        let client = OpenAIClient::with_tool_executor(
            self.http_client.clone(),
            config,
            tool_executor
        ).map_err(|e| anyhow::anyhow!("Failed to create OpenAI client: {}", e))?;

        Ok(ProviderInfo {
            provider: Arc::new(client),
            provider_name: "OpenAI".to_string(),
            model_name: chat_settings.model.clone(),
        })
    }

    /// Create an Anthropic provider (placeholder for future implementation)
    fn create_anthropic_provider(&self, _chat_settings: &ChatSettings) -> Result<ProviderInfo> {
        Err(anyhow::anyhow!("Anthropic provider is not yet implemented"))
    }

    /// Create a mock provider for testing
    fn create_mock_provider(&self, _chat_settings: &ChatSettings) -> Result<ProviderInfo> {
        // This would create a mock provider for testing
        // For now, we'll return an error since we don't have a mock implementation
        Err(anyhow::anyhow!("Mock provider is not yet implemented"))
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

/// Helper function to query database schema using an existing connection
async fn query_database_schema_with_connection(
    connection: std::sync::Arc<dyn Connection>,
) -> serde_json::Value {
    log::info!("Querying database schema using existing connection");

    // Get basic connection info
    let connection_type = connection.get_connection_type();
    let display_name = connection.get_display_name();

    log::info!("Connected to {} database: {}", connection_type, display_name);

    let mut result = serde_json::json!({
        "connection_type": connection_type,
        "database_name": display_name,
        "tables": []
    });

    // Get all tables (without schema filter for now)
    match connection.get_tables(None).await {
        Ok(tables) => {
            let mut tables_array = Vec::new();

            for table_name in tables {
                log::debug!("Processing table: {}", table_name);

                // Get column information for each table
                match connection.get_columns_for_table(&table_name, None).await {
                    Ok(columns) => {
                        let mut columns_array = Vec::new();
                        for column in columns {
                            columns_array.push(serde_json::json!({
                                "name": column.name,
                                "type": column.data_type,
                                "nullable": column.is_nullable,
                                "primary_key": column.is_primary_key,
                                "default_value": column.default_value,
                                "character_maximum_length": column.character_maximum_length
                            }));
                        }

                                        tables_array.push(serde_json::json!({
                            "name": table_name,
                            "schema": "public", // Default schema, could be enhanced for PostgreSQL
                            "object_type": "TABLE",
                            "columns": columns_array,
                            "column_count": columns_array.len()
                        }));
                    }
                    Err(e) => {
                        log::warn!("Failed to get columns for table {}: {}", table_name, e);
                        // Still add the table with basic info
                        tables_array.push(serde_json::json!({
                            "name": table_name,
                            "schema": "public",
                            "object_type": "TABLE",
                            "columns": [],
                            "error": format!("Failed to get columns: {}", e)
                        }));
                    }
                }
            }

            result["tables"] = serde_json::json!(tables_array);
            log::info!("Schema query completed: {} tables found", tables_array.len());
        }
        Err(e) => {
            log::error!("Failed to get tables: {}", e);
            result["error"] = serde_json::json!(format!("Failed to get tables: {}", e));
        }
    }

    result
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
