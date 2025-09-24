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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_error_code_serialization() {
        let code = ErrorCode::BadRequest;
        let serialized = serde_json::to_string(&code).unwrap();
        assert_eq!(serialized, "\"BAD_REQUEST\"");
    }

    #[test]
    fn test_error_code_deserialization() {
        let json = "\"SCRIPT_EXECUTION_FAILED\"";
        let code: ErrorCode = serde_json::from_str(json).unwrap();
        assert!(matches!(code, ErrorCode::ScriptExecutionFailed));
    }

    #[test]
    fn test_error_response_builder_basic() {
        let error = ErrorResponseBuilder::new(ErrorCode::NotFound, "Test error")
            .build();

        assert_eq!(error.status, 404);
        assert_eq!(error.error.message, "Test error");
        assert!(matches!(error.error.code, ErrorCode::NotFound));
        assert_eq!(error.error.request_id, "unknown");
        assert_eq!(error.error.path, "/");
        assert_eq!(error.error.method, "GET");
    }

    #[test]
    fn test_error_response_builder_with_details() {
        let error = ErrorResponseBuilder::new(ErrorCode::ValidationError, "Invalid input")
            .details("Field 'name' is required")
            .build();

        assert_eq!(error.status, 400);
        assert_eq!(error.error.details, Some("Field 'name' is required".to_string()));
    }

    #[test]
    fn test_error_response_builder_with_context() {
        let error = ErrorResponseBuilder::new(ErrorCode::InternalServerError, "Server error")
            .context("user_id", json!(123))
            .context("action", json!("create_post"))
            .build();

        assert_eq!(error.error.context.len(), 2);
        assert_eq!(error.error.context["user_id"], json!(123));
        assert_eq!(error.error.context["action"], json!("create_post"));
    }

    #[test]
    fn test_error_response_builder_full() {
        let error = ErrorResponseBuilder::new(ErrorCode::Unauthorized, "Access denied")
            .details("Invalid token")
            .request_id("req-123")
            .path("/api/users")
            .method("POST")
            .context("token_expired", json!(true))
            .build();

        assert_eq!(error.status, 401);
        assert_eq!(error.error.message, "Access denied");
        assert_eq!(error.error.details, Some("Invalid token".to_string()));
        assert_eq!(error.error.request_id, "req-123");
        assert_eq!(error.error.path, "/api/users");
        assert_eq!(error.error.method, "POST");
        assert_eq!(error.error.context["token_expired"], json!(true));
    }

    #[test]
    fn test_code_to_status_mapping() {
        let builder = ErrorResponseBuilder::new(ErrorCode::BadRequest, "test");
        
        // Test client errors
        assert_eq!(builder.code_to_status(&ErrorCode::BadRequest), 400);
        assert_eq!(builder.code_to_status(&ErrorCode::Unauthorized), 401);
        assert_eq!(builder.code_to_status(&ErrorCode::Forbidden), 403);
        assert_eq!(builder.code_to_status(&ErrorCode::NotFound), 404);
        assert_eq!(builder.code_to_status(&ErrorCode::MethodNotAllowed), 405);
        assert_eq!(builder.code_to_status(&ErrorCode::Conflict), 409);
        assert_eq!(builder.code_to_status(&ErrorCode::UnprocessableEntity), 422);
        assert_eq!(builder.code_to_status(&ErrorCode::TooManyRequests), 429);

        // Test server errors
        assert_eq!(builder.code_to_status(&ErrorCode::InternalServerError), 500);
        assert_eq!(builder.code_to_status(&ErrorCode::NotImplemented), 501);
        assert_eq!(builder.code_to_status(&ErrorCode::BadGateway), 502);
        assert_eq!(builder.code_to_status(&ErrorCode::ServiceUnavailable), 503);
        assert_eq!(builder.code_to_status(&ErrorCode::GatewayTimeout), 504);

        // Test application specific errors
        assert_eq!(builder.code_to_status(&ErrorCode::ScriptExecutionFailed), 500);
        assert_eq!(builder.code_to_status(&ErrorCode::ScriptTimeout), 504);
        assert_eq!(builder.code_to_status(&ErrorCode::ScriptNotFound), 404);
        assert_eq!(builder.code_to_status(&ErrorCode::ValidationError), 400);
        assert_eq!(builder.code_to_status(&ErrorCode::ConfigurationError), 500);
        assert_eq!(builder.code_to_status(&ErrorCode::DatabaseError), 500);
        assert_eq!(builder.code_to_status(&ErrorCode::FileSystemError), 500);
    }

    #[test]
    fn test_error_response_serialization() {
        let error = ErrorResponseBuilder::new(ErrorCode::NotFound, "Resource not found")
            .details("User with ID 123 not found")
            .request_id("req-456")
            .path("/api/users/123")
            .method("GET")
            .context("user_id", json!(123))
            .build();

        let serialized = serde_json::to_string(&error).unwrap();
        let deserialized: ErrorResponse = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.status, error.status);
        assert_eq!(deserialized.error.message, error.error.message);
        assert_eq!(deserialized.error.details, error.error.details);
        assert_eq!(deserialized.error.request_id, error.error.request_id);
        assert_eq!(deserialized.error.context, error.error.context);
    }

    #[test]
    fn test_helper_functions_not_found() {
        let error = errors::not_found("/api/users/123", "req-789");
        
        assert_eq!(error.status, 404);
        assert_eq!(error.error.message, "Resource not found");
        assert_eq!(error.error.path, "/api/users/123");
        assert_eq!(error.error.request_id, "req-789");
        assert!(matches!(error.error.code, ErrorCode::NotFound));
    }

    #[test]
    fn test_helper_functions_method_not_allowed() {
        let error = errors::method_not_allowed("/api/users", "DELETE", "req-101");
        
        assert_eq!(error.status, 405);
        assert_eq!(error.error.message, "Method not allowed");
        assert_eq!(error.error.path, "/api/users");
        assert_eq!(error.error.method, "DELETE");
        assert_eq!(error.error.request_id, "req-101");
        assert!(matches!(error.error.code, ErrorCode::MethodNotAllowed));
    }

    #[test]
    fn test_helper_functions_script_execution_failed() {
        let error = errors::script_execution_failed("/api/script", "Syntax error on line 5", "req-202");
        
        assert_eq!(error.status, 500);
        assert_eq!(error.error.message, "Script execution failed");
        assert_eq!(error.error.details, Some("Syntax error on line 5".to_string()));
        assert_eq!(error.error.path, "/api/script");
        assert_eq!(error.error.request_id, "req-202");
        assert!(matches!(error.error.code, ErrorCode::ScriptExecutionFailed));
    }

    #[test]
    fn test_helper_functions_script_timeout() {
        let error = errors::script_timeout("/api/long-script", "req-303");
        
        assert_eq!(error.status, 504);
        assert_eq!(error.error.message, "Script execution timeout");
        assert_eq!(error.error.path, "/api/long-script");
        assert_eq!(error.error.request_id, "req-303");
        assert!(matches!(error.error.code, ErrorCode::ScriptTimeout));
    }

    #[test]
    fn test_helper_functions_internal_server_error() {
        let error = errors::internal_server_error("/api/database", "Connection refused", "req-404");
        
        assert_eq!(error.status, 500);
        assert_eq!(error.error.message, "Internal server error");
        assert_eq!(error.error.details, Some("Connection refused".to_string()));
        assert_eq!(error.error.path, "/api/database");
        assert_eq!(error.error.request_id, "req-404");
        assert!(matches!(error.error.code, ErrorCode::InternalServerError));
    }

    #[test]
    fn test_error_details_clone() {
        let details = ErrorDetails {
            code: ErrorCode::BadRequest,
            message: "Test message".to_string(),
            details: Some("Test details".to_string()),
            request_id: "req-123".to_string(),
            timestamp: "2023-01-01T00:00:00Z".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            context: HashMap::new(),
        };

        let cloned = details.clone();
        assert_eq!(details.message, cloned.message);
        assert_eq!(details.request_id, cloned.request_id);
    }

    #[test]
    fn test_empty_context_serialization() {
        let error = ErrorResponseBuilder::new(ErrorCode::BadRequest, "Test")
            .build();

        let serialized = serde_json::to_string(&error).unwrap();
        // Empty context should be omitted from serialization
        assert!(!serialized.contains("\"context\""));
    }

    #[test]
    fn test_none_details_serialization() {
        let error = ErrorResponseBuilder::new(ErrorCode::BadRequest, "Test")
            .build();

        let serialized = serde_json::to_string(&error).unwrap();
        // None details should be omitted from serialization
        assert!(!serialized.contains("\"details\""));
    }
}
