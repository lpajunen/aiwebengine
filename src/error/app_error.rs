use thiserror::Error;

/// Unified application error type that consolidates all error handling
/// across the aiwebengine codebase. This replaces the mix of anyhow,
/// custom error types, and ad-hoc error handling.
#[derive(Debug, Error)]
pub enum AppError {
    // Configuration errors
    #[error("Configuration error: {message}")]
    Config {
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Configuration validation failed: {field} - {reason}")]
    ConfigValidation { field: String, reason: String },

    // Authentication and authorization errors
    #[error("Authentication required")]
    AuthenticationRequired,

    #[error("Authentication failed: {message}")]
    AuthenticationFailed { message: String },

    #[error("Authorization failed: {message}")]
    AuthorizationFailed { message: String },

    #[error("Session error: {message}")]
    Session { message: String },

    // Database and repository errors
    #[error("Database error: {message}")]
    Database {
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("Script not found: {uri}")]
    ScriptNotFound { uri: String },

    #[error("Asset not found: {name}")]
    AssetNotFound { name: String },

    // JavaScript execution errors
    #[error("JavaScript execution failed: {message}")]
    JsExecution { message: String },

    #[error("JavaScript execution timeout after {timeout_ms}ms")]
    JsTimeout { timeout_ms: u64 },

    #[error("JavaScript compilation error: {message}")]
    JsCompilation { message: String },

    // Security and validation errors
    #[error("Security violation: {message}")]
    Security { message: String },

    #[error("Input validation failed: {field} - {reason}")]
    Validation { field: String, reason: String },

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    // Network and HTTP errors
    #[error("HTTP request failed: {message}")]
    Http { message: String },

    #[error("Request timeout")]
    Timeout,

    // GraphQL errors
    #[error("GraphQL error: {message}")]
    Graphql { message: String },

    // Stream errors
    #[error("Stream error: {message}")]
    Stream { message: String },

    // File system errors
    #[error("File system error: {message}")]
    FileSystem { message: String },

    // Generic internal errors
    #[error("Internal error: {message}")]
    Internal { message: String },

    // External service errors
    #[error("External service error: {service} - {message}")]
    ExternalService { service: String, message: String },
}

impl AppError {
    /// Create a configuration error
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
            source: None,
        }
    }

    /// Create a configuration error with source
    pub fn config_with_source(
        message: impl Into<String>,
        _source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Config {
            message: message.into(),
            source: None, // Temporarily disable source boxing for compatibility
        }
    }

    /// Create a validation error
    pub fn validation(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            reason: reason.into(),
        }
    }

    /// Create an internal error
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    /// Convert to HTTP status code
    pub fn status_code(&self) -> u16 {
        match self {
            // Client errors (4xx)
            AppError::AuthenticationRequired => 401,
            AppError::AuthenticationFailed { .. } => 401,
            AppError::AuthorizationFailed { .. } => 403,
            AppError::Validation { .. } => 400,
            AppError::RateLimitExceeded => 429,
            AppError::ScriptNotFound { .. } => 404,
            AppError::AssetNotFound { .. } => 404,

            // Server errors (5xx)
            AppError::Config { .. } => 500,
            AppError::ConfigValidation { .. } => 500,
            AppError::Database { .. } => 500,
            AppError::JsExecution { .. } => 500,
            AppError::JsTimeout { .. } => 504,
            AppError::JsCompilation { .. } => 500,
            AppError::Security { .. } => 403,
            AppError::Http { .. } => 502,
            AppError::Timeout => 504,
            AppError::Graphql { .. } => 500,
            AppError::Stream { .. } => 500,
            AppError::FileSystem { .. } => 500,
            AppError::Internal { .. } => 500,
            AppError::ExternalService { .. } => 502,
            AppError::Session { .. } => 401,
        }
    }

    /// Check if error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            AppError::Timeout
                | AppError::Http { .. }
                | AppError::ExternalService { .. }
                | AppError::Database { .. }
        )
    }

    /// Convert to structured error response
    pub fn to_error_response(
        &self,
        path: &str,
        method: &str,
        request_id: &str,
    ) -> crate::error::ErrorResponse {
        use crate::error::{ErrorCode, ErrorResponseBuilder};

        let (code, message) = match self {
            AppError::AuthenticationRequired => {
                (ErrorCode::Unauthorized, "Authentication required")
            }
            AppError::AuthenticationFailed { message } => {
                (ErrorCode::Unauthorized, message.as_str())
            }
            AppError::AuthorizationFailed { message } => (ErrorCode::Forbidden, message.as_str()),
            AppError::Validation { field, reason } => {
                return ErrorResponseBuilder::new(
                    ErrorCode::ValidationError,
                    format!("{}: {}", field, reason),
                )
                .path(path)
                .method(method)
                .request_id(request_id)
                .build();
            }
            AppError::RateLimitExceeded => (ErrorCode::TooManyRequests, "Rate limit exceeded"),
            AppError::ScriptNotFound { uri } => {
                return ErrorResponseBuilder::new(
                    ErrorCode::ScriptNotFound,
                    format!("Script not found: {}", uri),
                )
                .path(path)
                .method(method)
                .request_id(request_id)
                .build();
            }
            AppError::AssetNotFound { name } => {
                return ErrorResponseBuilder::new(
                    ErrorCode::NotFound,
                    format!("Asset not found: {}", name),
                )
                .path(path)
                .method(method)
                .request_id(request_id)
                .build();
            }
            AppError::JsTimeout { timeout_ms } => {
                return ErrorResponseBuilder::new(
                    ErrorCode::ScriptTimeout,
                    format!("Script timeout after {}ms", timeout_ms),
                )
                .path(path)
                .method(method)
                .request_id(request_id)
                .build();
            }
            AppError::JsExecution { message } => {
                (ErrorCode::ScriptExecutionFailed, message.as_str())
            }
            AppError::JsCompilation { message } => {
                (ErrorCode::ScriptExecutionFailed, message.as_str())
            }
            AppError::Config { message, .. } => (ErrorCode::ConfigurationError, message.as_str()),
            AppError::ConfigValidation { field, reason } => {
                return ErrorResponseBuilder::new(
                    ErrorCode::ConfigurationError,
                    format!("{}: {}", field, reason),
                )
                .path(path)
                .method(method)
                .request_id(request_id)
                .build();
            }
            AppError::Database { message, .. } => (ErrorCode::DatabaseError, message.as_str()),
            AppError::Security { message } => (ErrorCode::Forbidden, message.as_str()),
            AppError::Http { message } => (ErrorCode::BadGateway, message.as_str()),
            AppError::Timeout => (ErrorCode::GatewayTimeout, "Request timeout"),
            AppError::Graphql { message } => (ErrorCode::InternalServerError, message.as_str()),
            AppError::Stream { message } => (ErrorCode::InternalServerError, message.as_str()),
            AppError::FileSystem { message } => (ErrorCode::InternalServerError, message.as_str()),
            AppError::Internal { message } => (ErrorCode::InternalServerError, message.as_str()),
            AppError::ExternalService { service, message } => {
                return ErrorResponseBuilder::new(
                    ErrorCode::BadGateway,
                    format!("{}: {}", service, message),
                )
                .path(path)
                .method(method)
                .request_id(request_id)
                .build();
            }
            AppError::Session { message } => (ErrorCode::Unauthorized, message.as_str()),
        };

        ErrorResponseBuilder::new(code, message)
            .path(path)
            .method(method)
            .request_id(request_id)
            .build()
    }
}

// Conversion implementations from existing error types

impl From<crate::auth::AuthError> for AppError {
    fn from(err: crate::auth::AuthError) -> Self {
        match err {
            crate::auth::AuthError::AuthenticationRequired => AppError::AuthenticationRequired,
            crate::auth::AuthError::InsufficientPermissions => AppError::AuthorizationFailed {
                message: "Insufficient permissions".to_string(),
            },
            crate::auth::AuthError::RateLimitExceeded => AppError::RateLimitExceeded,
            crate::auth::AuthError::Timeout => AppError::Timeout,
            crate::auth::AuthError::NoSession => AppError::Session {
                message: "Session not found or expired".to_string(),
            },
            crate::auth::AuthError::InvalidSessionCookie => AppError::Session {
                message: "Invalid session cookie".to_string(),
            },
            other => AppError::AuthenticationFailed {
                message: other.to_string(),
            },
        }
    }
}

impl From<crate::repository::RepositoryError> for AppError {
    fn from(err: crate::repository::RepositoryError) -> Self {
        match err {
            crate::repository::RepositoryError::ScriptNotFound(uri) => {
                AppError::ScriptNotFound { uri }
            }
            crate::repository::RepositoryError::AssetNotFound(name) => {
                AppError::AssetNotFound { name }
            }
            crate::repository::RepositoryError::LockError(msg) => {
                AppError::Internal { message: msg }
            }
            crate::repository::RepositoryError::InvalidData(msg) => AppError::Validation {
                field: "data".to_string(),
                reason: msg,
            },
        }
    }
}

impl From<crate::user_repository::UserRepositoryError> for AppError {
    fn from(err: crate::user_repository::UserRepositoryError) -> Self {
        match err {
            crate::user_repository::UserRepositoryError::UserNotFound(id) => AppError::Validation {
                field: "user_id".to_string(),
                reason: format!("User not found: {}", id),
            },
            crate::user_repository::UserRepositoryError::LockError(msg) => {
                AppError::Internal { message: msg }
            }
            crate::user_repository::UserRepositoryError::InvalidData(msg) => AppError::Validation {
                field: "user_data".to_string(),
                reason: msg,
            },
        }
    }
}

impl From<crate::security::SecurityError> for AppError {
    fn from(err: crate::security::SecurityError) -> Self {
        AppError::Security {
            message: err.to_string(),
        }
    }
}

impl From<crate::security::EncryptionError> for AppError {
    fn from(err: crate::security::EncryptionError) -> Self {
        AppError::Security {
            message: format!("Encryption error: {}", err),
        }
    }
}

impl From<crate::security::SessionError> for AppError {
    fn from(err: crate::security::SessionError) -> Self {
        AppError::Session {
            message: err.to_string(),
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        AppError::Database {
            message: err.to_string(),
            source: Some(Box::new(err)),
        }
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            AppError::Timeout
        } else {
            AppError::Http {
                message: err.to_string(),
            }
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Validation {
            field: "json".to_string(),
            reason: err.to_string(),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::FileSystem {
            message: err.to_string(),
        }
    }
}

// For backward compatibility with anyhow
impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal {
            message: err.to_string(),
        }
    }
}

// Result type alias for convenience
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_status_codes() {
        assert_eq!(AppError::AuthenticationRequired.status_code(), 401);
        assert_eq!(
            AppError::Validation {
                field: "test".to_string(),
                reason: "invalid".to_string(),
            }
            .status_code(),
            400
        );
        assert_eq!(AppError::RateLimitExceeded.status_code(), 429);
        assert_eq!(AppError::JsTimeout { timeout_ms: 5000 }.status_code(), 504);
        assert_eq!(
            AppError::Internal {
                message: "test".to_string(),
            }
            .status_code(),
            500
        );
    }

    #[test]
    fn test_error_retryable() {
        assert!(AppError::Timeout.is_retryable());
        assert!(
            AppError::Http {
                message: "test".to_string()
            }
            .is_retryable()
        );
        assert!(!AppError::AuthenticationRequired.is_retryable());
        assert!(
            !AppError::Validation {
                field: "test".to_string(),
                reason: "invalid".to_string(),
            }
            .is_retryable()
        );
    }

    #[test]
    fn test_error_to_response() {
        let error = AppError::ScriptNotFound {
            uri: "test://script".to_string(),
        };
        let response = error.to_error_response("/test", "GET", "req-123");

        assert_eq!(response.status, 404);
        assert!(response.error.message.contains("Script not found"));
        assert_eq!(response.error.path, "/test");
        assert_eq!(response.error.method, "GET");
        assert_eq!(response.error.request_id, "req-123");
    }

    #[test]
    fn test_conversions() {
        // Test auth error conversion
        let auth_err = crate::auth::AuthError::AuthenticationRequired;
        let app_err: AppError = auth_err.into();
        assert!(matches!(app_err, AppError::AuthenticationRequired));

        // Test repository error conversion
        let repo_err = crate::repository::RepositoryError::ScriptNotFound("test".to_string());
        let app_err: AppError = repo_err.into();
        assert!(matches!(app_err, AppError::ScriptNotFound { .. }));

        // Test serde error conversion
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let app_err: AppError = json_err.into();
        assert!(matches!(app_err, AppError::Validation { .. }));
    }
}
