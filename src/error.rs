use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Error classification for different types of errors
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    // Client errors (4xx)
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    Conflict,
    UnprocessableEntity,
    TooManyRequests,

    // Server errors (5xx)
    InternalServerError,
    NotImplemented,
    BadGateway,
    ServiceUnavailable,
    GatewayTimeout,

    // Application specific errors
    ScriptExecutionFailed,
    ScriptTimeout,
    ScriptNotFound,
    ValidationError,
    ConfigurationError,
    DatabaseError,
    FileSystemError,
}

/// Structured error response format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetails,
    pub status: u16,
}

/// Details of an error occurrence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetails {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub request_id: String,
    pub timestamp: String,
    pub path: String,
    pub method: String,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub context: HashMap<String, serde_json::Value>,
}

/// Builder for creating error responses
pub struct ErrorResponseBuilder {
    code: ErrorCode,
    message: String,
    details: Option<String>,
    request_id: String,
    timestamp: String,
    path: String,
    method: String,
    context: HashMap<String, serde_json::Value>,
}

impl ErrorResponseBuilder {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
            request_id: "unknown".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            path: "/".to_string(),
            method: "GET".to_string(),
            context: HashMap::new(),
        }
    }

    pub fn details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    pub fn request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = request_id.into();
        self
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    pub fn context(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> ErrorResponse {
        let status = self.code_to_status(&self.code);

        ErrorResponse {
            error: ErrorDetails {
                code: self.code,
                message: self.message,
                details: self.details,
                request_id: self.request_id,
                timestamp: self.timestamp,
                path: self.path,
                method: self.method,
                context: self.context,
            },
            status,
        }
    }

    fn code_to_status(&self, code: &ErrorCode) -> u16 {
        match code {
            ErrorCode::BadRequest => 400,
            ErrorCode::Unauthorized => 401,
            ErrorCode::Forbidden => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::MethodNotAllowed => 405,
            ErrorCode::Conflict => 409,
            ErrorCode::UnprocessableEntity => 422,
            ErrorCode::TooManyRequests => 429,
            ErrorCode::InternalServerError => 500,
            ErrorCode::NotImplemented => 501,
            ErrorCode::BadGateway => 502,
            ErrorCode::ServiceUnavailable => 503,
            ErrorCode::GatewayTimeout => 504,
            ErrorCode::ScriptExecutionFailed => 500,
            ErrorCode::ScriptTimeout => 504,
            ErrorCode::ScriptNotFound => 404,
            ErrorCode::ValidationError => 400,
            ErrorCode::ConfigurationError => 500,
            ErrorCode::DatabaseError => 500,
            ErrorCode::FileSystemError => 500,
        }
    }
}

/// Helper functions for common error types
pub mod errors {
    use super::*;

    pub fn not_found(path: &str, request_id: &str) -> ErrorResponse {
        ErrorResponseBuilder::new(ErrorCode::NotFound, "Resource not found")
            .path(path)
            .request_id(request_id)
            .build()
    }

    pub fn method_not_allowed(path: &str, method: &str, request_id: &str) -> ErrorResponse {
        ErrorResponseBuilder::new(ErrorCode::MethodNotAllowed, "Method not allowed")
            .path(path)
            .method(method)
            .request_id(request_id)
            .build()
    }

    pub fn script_execution_failed(path: &str, error: &str, request_id: &str) -> ErrorResponse {
        ErrorResponseBuilder::new(ErrorCode::ScriptExecutionFailed, "Script execution failed")
            .details(error)
            .path(path)
            .request_id(request_id)
            .build()
    }

    pub fn script_timeout(path: &str, request_id: &str) -> ErrorResponse {
        ErrorResponseBuilder::new(ErrorCode::ScriptTimeout, "Script execution timeout")
            .path(path)
            .request_id(request_id)
            .build()
    }

    pub fn internal_server_error(path: &str, error: &str, request_id: &str) -> ErrorResponse {
        ErrorResponseBuilder::new(ErrorCode::InternalServerError, "Internal server error")
            .details(error)
            .path(path)
            .request_id(request_id)
            .build()
    }
}