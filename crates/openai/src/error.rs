use http::StatusCode;
use std::fmt;
use crate::types::OpenAIErrorResponse;

/// Error type for OpenAI client operations
#[derive(Clone, Debug)]
pub enum OpenAIError {
    /// API request failed
    ApiError {
        /// HTTP status code
        status: StatusCode,
        /// Error message from API
        message: String,
        /// Error type from API
        error_type: String,
        /// Error code from API
        code: Option<String>,
    },
    /// Network or HTTP client error
    HttpError(String),
    /// Request timeout
    Timeout,
    /// Invalid request parameters
    InvalidRequest(String),
    /// Authentication failed
    AuthenticationError(String),
    /// Rate limit exceeded
    RateLimitError {
        /// Message from API
        message: String,
        /// When the rate limit resets (optional)
        reset_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Server error
    ServerError {
        /// HTTP status code
        status: StatusCode,
        /// Error message
        message: String,
    },
    /// Invalid response format
    InvalidResponse(String),
    /// JSON parsing error
    JsonError(String),
    /// IO error
    IoError(String),
    /// Configuration error
    ConfigError(String),
    /// Tool execution error
    ToolError {
        /// Tool call ID
        tool_call_id: String,
        /// Error message
        message: String,
    },
    /// Streaming error
    StreamError(String),
    /// Unknown error
    Unknown(String),
}

impl fmt::Display for OpenAIError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenAIError::ApiError { status, message, error_type, code } => {
                write!(f, "OpenAI API error [{}]: {} ({})", status, message, error_type)?;
                if let Some(code) = code {
                    write!(f, " - code: {}", code)?;
                }
                Ok(())
            }
            OpenAIError::HttpError(msg) => write!(f, "HTTP error: {}", msg),
            OpenAIError::Timeout => write!(f, "Request timeout"),
            OpenAIError::InvalidRequest(msg) => write!(f, "Invalid request: {}", msg),
            OpenAIError::AuthenticationError(msg) => write!(f, "Authentication error: {}", msg),
            OpenAIError::RateLimitError { message, reset_at } => {
                write!(f, "Rate limit exceeded: {}", message)?;
                if let Some(reset_at) = reset_at {
                    write!(f, " (resets at: {})", reset_at)?;
                }
                Ok(())
            }
            OpenAIError::ServerError { status, message } => {
                write!(f, "Server error [{}]: {}", status, message)
            }
            OpenAIError::InvalidResponse(msg) => write!(f, "Invalid response: {}", msg),
            OpenAIError::JsonError(msg) => write!(f, "JSON error: {}", msg),
            OpenAIError::IoError(msg) => write!(f, "IO error: {}", msg),
            OpenAIError::ConfigError(msg) => write!(f, "Configuration error: {}", msg),
            OpenAIError::ToolError { tool_call_id, message } => {
                write!(f, "Tool error [{}]: {}", tool_call_id, message)
            }
            OpenAIError::StreamError(msg) => write!(f, "Stream error: {}", msg),
            OpenAIError::Unknown(msg) => write!(f, "Unknown error: {}", msg),
        }
    }
}

impl std::error::Error for OpenAIError {}

impl OpenAIError {
    /// Create an API error from HTTP status and response
    pub fn api_error(status: StatusCode, response: &OpenAIErrorResponse) -> Self {
        Self::ApiError {
            status,
            message: response.error.message.clone(),
            error_type: response.error.error_type.clone(),
            code: response.error.code.clone(),
        }
    }

    /// Create a rate limit error
    pub fn rate_limit_error(message: String) -> Self {
        Self::RateLimitError {
            message,
            reset_at: None,
        }
    }

    /// Create a rate limit error with reset time
    pub fn rate_limit_error_with_reset(message: String, reset_at: chrono::DateTime<chrono::Utc>) -> Self {
        Self::RateLimitError {
            message,
            reset_at: Some(reset_at),
        }
    }

    /// Create a server error
    pub fn server_error(status: StatusCode, message: String) -> Self {
        Self::ServerError { status, message }
    }

    /// Create a tool error
    pub fn tool_error(tool_call_id: String, message: String) -> Self {
        Self::ToolError { tool_call_id, message }
    }

    /// Check if this is a retryable error
    pub fn is_retryable(&self) -> bool {
        match self {
            OpenAIError::HttpError(_) => true,
            OpenAIError::Timeout => true,
            OpenAIError::RateLimitError { .. } => true,
            OpenAIError::ServerError { status, .. } => {
                matches!(status.as_u16(), 500..=599)
            }
            _ => false,
        }
    }

    /// Check if this is an authentication error
    pub fn is_authentication_error(&self) -> bool {
        matches!(
            self,
            OpenAIError::ApiError { status, .. } | OpenAIError::ServerError { status, .. }
                if status.as_u16() == 401
        )
    }

    /// Check if this is a rate limit error
    pub fn is_rate_limit_error(&self) -> bool {
        match self {
            OpenAIError::ApiError { status, .. } => status.as_u16() == 429,
            OpenAIError::RateLimitError { .. } => true,
            _ => false,
        }
    }

    /// Check if this is a quota exceeded error
    pub fn is_quota_exceeded_error(&self) -> bool {
        matches!(self, OpenAIError::ApiError { code: Some(code), .. } if code == "insufficient_quota")
    }
}

// Conversion implementations

impl From<http::Error> for OpenAIError {
    fn from(err: http::Error) -> Self {
        Self::HttpError(err.to_string())
    }
}

impl From<serde_json::Error> for OpenAIError {
    fn from(err: serde_json::Error) -> Self {
        Self::JsonError(err.to_string())
    }
}

impl From<async_std::io::Error> for OpenAIError {
    fn from(err: async_std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

impl From<crate::config::ConfigError> for OpenAIError {
    fn from(err: crate::config::ConfigError) -> Self {
        Self::ConfigError(err.to_string())
    }
}


impl From<futures::channel::mpsc::SendError> for OpenAIError {
    fn from(err: futures::channel::mpsc::SendError) -> Self {
        Self::StreamError(format!("Failed to send to stream: {}", err))
    }
}

impl From<OpenAIError> for blanco_core::chat_provider::ProviderError {
    fn from(err: OpenAIError) -> Self {
        anyhow::anyhow!("OpenAI error: {}", err).into()
    }
}


/// Result type for OpenAI operations
pub type OpenAIResult<T> = Result<T, OpenAIError>;

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;

    #[test]
    fn test_error_display() {
        let api_err = OpenAIError::ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "Invalid request".to_string(),
            error_type: "invalid_request".to_string(),
            code: Some("invalid_api_key".to_string()),
        };
        assert_eq!(
            api_err.to_string(),
            "OpenAI API error [400 Bad Request]: Invalid request (invalid_request) - code: invalid_api_key"
        );

        let timeout_err = OpenAIError::Timeout;
        assert_eq!(timeout_err.to_string(), "Request timeout");

        let tool_err = OpenAIError::tool_error("call_123".to_string(), "Function failed".to_string());
        assert_eq!(tool_err.to_string(), "Tool error [call_123]: Function failed");
    }

    #[test]
    fn test_retryable_errors() {
        assert!(OpenAIError::Timeout.is_retryable());
        assert!(OpenAIError::rate_limit_error("Too many requests".to_string()).is_retryable());
        assert!(OpenAIError::server_error(StatusCode::INTERNAL_SERVER_ERROR, "Server error".to_string()).is_retryable());
        assert!(!OpenAIError::InvalidRequest("Bad input".to_string()).is_retryable());
        assert!(!OpenAIError::AuthenticationError("Bad token".to_string()).is_retryable());
    }

    #[test]
    fn test_authentication_error() {
        let auth_err = OpenAIError::ApiError {
            status: StatusCode::UNAUTHORIZED,
            message: "Invalid API key".to_string(),
            error_type: "invalid_request".to_string(),
            code: None,
        };
        assert!(auth_err.is_authentication_error());

        let server_err = OpenAIError::ServerError {
            status: StatusCode::UNAUTHORIZED,
            message: "Unauthorized".to_string(),
        };
        assert!(server_err.is_authentication_error());

        let bad_request = OpenAIError::ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "Bad request".to_string(),
            error_type: "invalid_request".to_string(),
            code: None,
        };
        assert!(!bad_request.is_authentication_error());
    }

    #[test]
    fn test_quota_exceeded_error() {
        let quota_err = OpenAIError::ApiError {
            status: StatusCode::PAYMENT_REQUIRED,
            message: "Insufficient quota".to_string(),
            error_type: "insufficient_quota".to_string(),
            code: Some("insufficient_quota".to_string()),
        };
        assert!(quota_err.is_quota_exceeded_error());

        let other_err = OpenAIError::ApiError {
            status: StatusCode::BAD_REQUEST,
            message: "Bad request".to_string(),
            error_type: "invalid_request".to_string(),
            code: Some("invalid_request".to_string()),
        };
        assert!(!other_err.is_quota_exceeded_error());
    }

    #[test]
    fn test_error_conversions() {
        // Create a JSON error by parsing invalid JSON
        let json_err: serde_json::Error = serde_json::from_str::<serde_json::Value>("{invalid json}").unwrap_err();
        let openai_err: OpenAIError = json_err.into();
        assert!(matches!(openai_err, OpenAIError::JsonError(_)));

        let config_err = crate::config::ConfigError::MissingApiKey;
        let openai_err: OpenAIError = config_err.into();
        assert!(matches!(openai_err, OpenAIError::ConfigError(_)));
    }
}