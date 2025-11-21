//! Example usage of the consolidated AppError type
//!
//! This demonstrates how to use the unified error handling throughout the codebase.

use crate::error::{AppError, AppResult};
use crate::repository;

/// Example function showing how to handle different error types
pub async fn example_script_execution(script_uri: &str) -> AppResult<String> {
    // Try to fetch the script
    let script_content = repository::fetch_script(script_uri)
        .ok_or_else(|| AppError::ScriptNotFound {
            uri: script_uri.to_string(),
        })?;

    // Validate the script content
    if script_content.trim().is_empty() {
        return Err(AppError::Validation {
            field: "script_content".to_string(),
            reason: "Script content cannot be empty".to_string(),
        });
    }

    // Execute the script (this would normally call js_engine)
    // For this example, we'll simulate success/failure
    if script_content.contains("error") {
        return Err(AppError::JsExecution {
            message: "Simulated JavaScript execution error".to_string(),
        });
    }

    Ok(format!("Successfully executed script: {}", script_uri))
}

/// Example of converting external errors to AppError
pub async fn example_database_operation() -> AppResult<Vec<String>> {
    // This would normally use sqlx, but we'll simulate an error
    // The sqlx::Error will automatically convert to AppError::Database

    // Simulate a database connection error
    Err(sqlx::Error::Configuration("Connection timeout".into()).into())
}

/// Example of handling configuration errors
pub fn example_config_validation(port: u16) -> AppResult<()> {
    if port == 0 {
        return Err(AppError::ConfigValidation {
            field: "server.port".to_string(),
            reason: "Port cannot be zero".to_string(),
        });
    }

    if port > 65535 {
        return Err(AppError::ConfigValidation {
            field: "server.port".to_string(),
            reason: "Port must be <= 65535".to_string(),
        });
    }

    Ok(())
}

/// Example of security error handling
pub fn example_security_check(input: &str) -> AppResult<()> {
    if input.contains("<script>") {
        return Err(AppError::Security {
            message: "XSS attempt detected".to_string(),
        });
    }

    Ok(())
}

/// Example HTTP handler using the new error type
pub async fn example_http_handler() -> Result<axum::response::Json<serde_json::Value>, crate::error::ErrorResponse> {
    match example_script_execution("https://example.com/test").await {
        Ok(result) => Ok(axum::response::Json(serde_json::json!({
            "success": true,
            "result": result
        }))),
        Err(err) => {
            // AppError automatically converts to ErrorResponse
            Err(err.to_error_response("/api/execute", "POST", "req-123"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_script_execution_success() {
        let result = example_script_execution("https://example.com/valid").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_script_execution_not_found() {
        let result = example_script_execution("https://example.com/nonexistent").await;
        assert!(matches!(result, Err(AppError::ScriptNotFound { .. })));
    }

    #[tokio::test]
    async fn test_script_execution_error() {
        let result = example_script_execution("https://example.com/error-script").await;
        assert!(matches!(result, Err(AppError::JsExecution { .. })));
    }

    #[tokio::test]
    async fn test_config_validation() {
        assert!(example_config_validation(8080).is_ok());
        assert!(matches!(example_config_validation(0), Err(AppError::ConfigValidation { .. })));
        assert!(matches!(example_config_validation(70000), Err(AppError::ConfigValidation { .. })));
    }

    #[tokio::test]
    async fn test_database_error_conversion() {
        let result = example_database_operation().await;
        assert!(matches!(result, Err(AppError::Database { .. })));
    }

    #[test]
    fn test_security_error() {
        assert!(example_security_check("safe input").is_ok());
        assert!(matches!(example_security_check("<script>alert('xss')</script>"), Err(AppError::Security { .. })));
    }

    #[test]
    fn test_error_status_codes() {
        let script_not_found = AppError::ScriptNotFound { uri: "test".to_string() };
        assert_eq!(script_not_found.status_code(), 404);

        let auth_required = AppError::AuthenticationRequired;
        assert_eq!(auth_required.status_code(), 401);

        let internal = AppError::Internal { message: "test".to_string() };
        assert_eq!(internal.status_code(), 500);
    }

    #[test]
    fn test_error_retryable() {
        let timeout = AppError::Timeout;
        assert!(timeout.is_retryable());

        let auth = AppError::AuthenticationRequired;
        assert!(!auth.is_retryable());
    }
}