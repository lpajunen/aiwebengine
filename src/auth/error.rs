// Authentication Error Types
// Comprehensive error handling for OAuth2, JWT, and session management

use thiserror::Error;

use crate::security::{EncryptionError, SessionError};

#[derive(Debug, Error)]
pub enum AuthError {
    // JWT-related errors
    #[error("Invalid JWT token: {0}")]
    InvalidToken(String),

    #[error("JWT error: {0}")]
    JwtError(String),

    #[error("JWT token expired")]
    TokenExpired,

    #[error("JWT signature verification failed")]
    SignatureVerificationFailed,

    #[error("Missing required JWT claim: {0}")]
    MissingClaim(String),

    // OAuth2-related errors
    #[error("OAuth2 error: {0}")]
    OAuth2Error(String),

    #[error("OAuth2 provider error: {0}")]
    ProviderError(String),

    #[error("Invalid OAuth2 state parameter")]
    InvalidState,

    #[error("OAuth2 code exchange failed: {0}")]
    CodeExchangeFailed(String),

    #[error("Failed to retrieve user info: {0}")]
    UserInfoFailed(String),

    #[error("Unsupported OAuth2 provider: {0}")]
    UnsupportedProvider(String),

    #[error("OAuth2 redirect URI mismatch")]
    RedirectUriMismatch,

    // Session-related errors
    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    #[error("Session error: {0}")]
    SessionError(String),

    #[error("Session not found or expired")]
    NoSession,

    #[error("Invalid session cookie")]
    InvalidSessionCookie,

    // Configuration errors
    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Missing required configuration: {0}")]
    MissingConfig(String),

    #[error("Invalid configuration value for {key}: {reason}")]
    InvalidConfig { key: String, reason: String },

    // Encryption errors
    #[error("Encryption error: {0}")]
    Encryption(#[from] EncryptionError),

    // Security errors
    #[error("CSRF validation failed")]
    CsrfValidationFailed,

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Authentication required")]
    AuthenticationRequired,

    #[error("Insufficient permissions")]
    InsufficientPermissions,

    // Network/HTTP errors
    #[error("HTTP request failed: {0}")]
    HttpError(String),

    #[error("JSON parsing error: {0}")]
    JsonError(String),

    // General errors
    #[error("Internal authentication error: {0}")]
    Internal(String),

    #[error("Provider communication timeout")]
    Timeout,
}

// Conversion from reqwest errors
impl From<reqwest::Error> for AuthError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            AuthError::Timeout
        } else {
            AuthError::HttpError(err.to_string())
        }
    }
}

// Conversion from serde_json errors
impl From<serde_json::Error> for AuthError {
    fn from(err: serde_json::Error) -> Self {
        AuthError::JsonError(err.to_string())
    }
}

// HTTP status code mapping for error responses
impl AuthError {
    pub fn status_code(&self) -> u16 {
        match self {
            AuthError::InvalidToken(_)
            | AuthError::JwtError(_)
            | AuthError::TokenExpired
            | AuthError::SignatureVerificationFailed
            | AuthError::NoSession
            | AuthError::InvalidSessionCookie
            | AuthError::AuthenticationRequired => 401,

            AuthError::InsufficientPermissions => 403,

            AuthError::RateLimitExceeded => 429,

            AuthError::ConfigError(_)
            | AuthError::MissingConfig(_)
            | AuthError::InvalidConfig { .. }
            | AuthError::Internal(_) => 500,

            AuthError::Timeout => 504,

            _ => 400,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, AuthError::Timeout | AuthError::HttpError(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_status_codes() {
        assert_eq!(AuthError::AuthenticationRequired.status_code(), 401);
        assert_eq!(AuthError::InsufficientPermissions.status_code(), 403);
        assert_eq!(AuthError::RateLimitExceeded.status_code(), 429);
        assert_eq!(
            AuthError::ConfigError("test".to_string()).status_code(),
            500
        );
        assert_eq!(AuthError::Timeout.status_code(), 504);
    }

    #[test]
    fn test_retryable_errors() {
        assert!(AuthError::Timeout.is_retryable());
        assert!(AuthError::HttpError("connection failed".to_string()).is_retryable());
        assert!(!AuthError::AuthenticationRequired.is_retryable());
        assert!(!AuthError::InvalidToken("bad token".to_string()).is_retryable());
    }

    #[test]
    fn test_error_display() {
        let err = AuthError::InvalidToken("malformed".to_string());
        assert_eq!(err.to_string(), "Invalid JWT token: malformed");

        let err = AuthError::InvalidConfig {
            key: "jwt_secret".to_string(),
            reason: "too short".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Invalid configuration value for jwt_secret: too short"
        );
    }
}
