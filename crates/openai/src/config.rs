use serde::{Deserialize, Serialize};
use std::env;

/// OpenAI configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenAIConfig {
    /// API key for authentication
    pub api_key: String,
    /// Base URL for the API (defaults to OpenAI's API)
    pub base_url: String,
    /// Default model to use
    pub model: String,
    /// Default maximum tokens for responses
    pub max_tokens: u32,
    /// Default temperature for sampling
    pub temperature: f32,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
    /// Organization ID (if applicable)
    pub organization: Option<String>,
    /// Additional headers to send with requests
    pub additional_headers: std::collections::HashMap<String, String>,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4".to_string(),
            max_tokens: 2048,
            temperature: 0.7,
            timeout_seconds: 60,
            organization: None,
            additional_headers: std::collections::HashMap::new(),
        }
    }
}

impl OpenAIConfig {
    /// Create a new configuration with just an API key
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Self::default()
        }
    }

    /// Create a configuration from environment variables
    ///
    /// Environment variables:
    /// - OPENAI_API_KEY: API key (required)
    /// - OPENAI_BASE_URL: Base URL (optional)
    /// - OPENAI_MODEL: Default model (optional)
    /// - OPENAI_MAX_TOKENS: Default max tokens (optional)
    /// - OPENAI_TEMPERATURE: Default temperature (optional)
    /// - OPENAI_TIMEOUT: Request timeout in seconds (optional)
    /// - OPENAI_ORGANIZATION: Organization ID (optional)
    pub fn from_env() -> Result<Self, ConfigError> {
        let api_key = env::var("OPENAI_API_KEY").map_err(|_| ConfigError::MissingApiKey)?;

        let mut config = Self::new(api_key);

        if let Ok(base_url) = env::var("OPENAI_BASE_URL") {
            config.base_url = base_url;
        }

        if let Ok(model) = env::var("OPENAI_MODEL") {
            config.model = model;
        }

        if let Ok(max_tokens) = env::var("OPENAI_MAX_TOKENS") {
            config.max_tokens = max_tokens
                .parse()
                .map_err(|_| ConfigError::InvalidMaxTokens)?;
        }

        if let Ok(temperature) = env::var("OPENAI_TEMPERATURE") {
            config.temperature = temperature
                .parse()
                .map_err(|_| ConfigError::InvalidTemperature)?;
        }

        if let Ok(timeout) = env::var("OPENAI_TIMEOUT") {
            config.timeout_seconds = timeout.parse().map_err(|_| ConfigError::InvalidTimeout)?;
        }

        if let Ok(organization) = env::var("OPENAI_ORGANIZATION") {
            config.organization = Some(organization);
        }

        Ok(config)
    }

    /// Set the API key
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = api_key.into();
        self
    }

    /// Set the base URL
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the default model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the default max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set the default temperature
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set the request timeout in seconds
    pub fn with_timeout(mut self, timeout_seconds: u64) -> Self {
        self.timeout_seconds = timeout_seconds;
        self
    }

    /// Set the organization ID
    pub fn with_organization(mut self, organization: impl Into<String>) -> Self {
        self.organization = Some(organization.into());
        self
    }

    /// Add an additional header
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.additional_headers.insert(key.into(), value.into());
        self
    }

    /// Add multiple additional headers
    pub fn with_headers(mut self, headers: std::collections::HashMap<String, String>) -> Self {
        self.additional_headers.extend(headers);
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.api_key.is_empty() {
            return Err(ConfigError::MissingApiKey);
        }

        if self.base_url.is_empty() {
            return Err(ConfigError::MissingBaseUrl);
        }

        if self.model.is_empty() {
            return Err(ConfigError::MissingModel);
        }

        if self.temperature < 0.0 || self.temperature > 2.0 {
            return Err(ConfigError::InvalidTemperature);
        }

        if self.max_tokens == 0 {
            return Err(ConfigError::InvalidMaxTokens);
        }

        if self.timeout_seconds == 0 {
            return Err(ConfigError::InvalidTimeout);
        }

        // Validate that the base URL is valid
        if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
            return Err(ConfigError::InvalidBaseUrl);
        }

        Ok(())
    }

    /// Get the chat completions endpoint URL
    pub fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    /// Check if the configuration is valid
    pub fn is_valid(&self) -> bool {
        self.validate().is_ok()
    }
}

/// Configuration errors
#[derive(Clone, Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum ConfigError {
    #[error("API key is required")]
    MissingApiKey,
    #[error("Base URL is required")]
    MissingBaseUrl,
    #[error("Model is required")]
    MissingModel,
    #[error("Invalid temperature: must be between 0.0 and 2.0")]
    InvalidTemperature,
    #[error("Invalid max tokens: must be greater than 0")]
    InvalidMaxTokens,
    #[error("Invalid timeout: must be greater than 0")]
    InvalidTimeout,
    #[error("Invalid base URL: must start with http:// or https://")]
    InvalidBaseUrl,
    #[error("Environment variable error: {0}")]
    EnvVarError(#[from] env::VarError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = OpenAIConfig::default();
        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.model, "gpt-4");
        assert_eq!(config.max_tokens, 2048);
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.timeout_seconds, 60);
    }

    #[test]
    fn test_config_builder() {
        let config = OpenAIConfig::new("test-key")
            .with_model("gpt-3.5-turbo")
            .with_max_tokens(1024)
            .with_temperature(0.5)
            .with_timeout(30)
            .with_organization("test-org");

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.model, "gpt-3.5-turbo");
        assert_eq!(config.max_tokens, 1024);
        assert_eq!(config.temperature, 0.5);
        assert_eq!(config.timeout_seconds, 30);
        assert_eq!(config.organization, Some("test-org".to_string()));
    }

    #[test]
    fn test_config_validation() {
        let mut config = OpenAIConfig::new("test-key");
        assert!(config.validate().is_ok());

        config.api_key = String::new();
        assert!(matches!(config.validate(), Err(ConfigError::MissingApiKey)));

        config.api_key = "test-key".to_string();
        config.temperature = 3.0;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidTemperature)
        ));

        config.temperature = 0.7;
        config.max_tokens = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidMaxTokens)
        ));
    }

    #[test]
    fn test_chat_completions_url() {
        let config = OpenAIConfig::new("test-key");
        assert_eq!(
            config.chat_completions_url(),
            "https://api.openai.com/v1/chat/completions"
        );

        let config = config.with_base_url("https://api.example.com/v1");
        assert_eq!(
            config.chat_completions_url(),
            "https://api.example.com/v1/chat/completions"
        );
    }
}
