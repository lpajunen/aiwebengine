use crate::error::ErrorResponse;
use axum::http::StatusCode;
use serde_json;
use tracing::error;

/// Safely convert status code with fallback
pub fn safe_status_code(status: u16) -> StatusCode {
    StatusCode::from_u16(status).unwrap_or_else(|_| {
        error!("Invalid status code: {}, using 500 instead", status);
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Safely serialize error response with fallback
pub fn safe_error_json(error_response: &ErrorResponse) -> String {
    serde_json::to_string(error_response).unwrap_or_else(|e| {
        error!("Failed to serialize error response: {}. Using fallback.", e);
        r#"{"error":{"code":"INTERNAL_SERVER_ERROR","message":"Serialization error occurred","status":500}}"#.to_string()
    })
}

/// Safely serialize any JSON value with fallback
pub fn safe_json_serialize<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| {
        error!("JSON serialization failed: {}", e);
        format!("Serialization error: {}", e)
    })
}

/// Create a safe error response with proper status code and JSON
pub fn create_safe_error_response(error_response: ErrorResponse) -> (StatusCode, String) {
    let status = safe_status_code(error_response.status);
    let json = safe_error_json(&error_response);
    (status, json)
}

/// Build a JSON response safely from a serializable value. Returns a Response with
/// `application/json` content-type and the provided status.
pub fn json_response<T: serde::Serialize>(
    status: StatusCode,
    payload: &T,
) -> axum::response::Response {
    match serde_json::to_string(payload) {
        Ok(body) => axum::response::Response::builder()
            .status(status)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body))
            .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::from("{}"))),
        Err(e) => {
            error!("Failed to serialize JSON response: {}", e);
            axum::response::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    r#"{"error":"Serialization failed"}"#,
                ))
                .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::from("{}")))
        }
    }
}

/// Helper macro for handling JavaScript execution results safely
#[macro_export]
macro_rules! handle_js_result {
    ($result:expr, $path:expr, $request_id:expr) => {
        match $result {
            Ok((status, body, content_type)) => {
                let status_code = $crate::safe_helpers::safe_status_code(status);
                let mut response = (status_code, body).into_response();
                if let Some(ct) = content_type {
                    if let Ok(header_value) = axum::http::HeaderValue::from_str(&ct) {
                        response
                            .headers_mut()
                            .insert(axum::http::header::CONTENT_TYPE, header_value);
                    }
                }
                response
            }
            Err(js_error) => {
                tracing::error!("JavaScript execution error for {}: {}", $path, js_error);
                let error_response =
                    $crate::error::errors::script_execution_failed($path, $request_id, &js_error);
                let (status, json) =
                    $crate::safe_helpers::create_safe_error_response(error_response);
                (status, json).into_response()
            }
        }
    };
}

/// Helper macro for safe timeout handling
#[macro_export]
macro_rules! handle_timeout {
    ($timeout_result:expr, $path:expr, $request_id:expr) => {
        match $timeout_result {
            Ok(js_result) => $crate::handle_js_result!(js_result, $path, $request_id),
            Err(_) => {
                tracing::error!("Script execution timeout for path: {}", $path);
                let error_response = $crate::error::errors::script_timeout($path, $request_id);
                let (status, json) =
                    $crate::safe_helpers::create_safe_error_response(error_response);
                (status, json).into_response()
            }
        }
    };
}

/// Circuit breaker for JavaScript execution
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    failure_count: std::sync::Arc<std::sync::Mutex<u32>>,
    last_failure: std::sync::Arc<std::sync::Mutex<std::time::Instant>>,
    threshold: u32,
    timeout: std::time::Duration,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, timeout_secs: u64) -> Self {
        Self {
            failure_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            last_failure: std::sync::Arc::new(std::sync::Mutex::new(
                std::time::Instant::now() - std::time::Duration::from_secs(timeout_secs + 1),
            )),
            threshold,
            timeout: std::time::Duration::from_secs(timeout_secs),
        }
    }

    pub fn can_execute(&self) -> bool {
        if let (Ok(count), Ok(last_fail)) = (self.failure_count.lock(), self.last_failure.lock()) {
            if *count < self.threshold {
                return true;
            }

            // Check if enough time has passed since last failure
            last_fail.elapsed() > self.timeout
        } else {
            // If we can't acquire locks, allow execution (degraded mode)
            error!("Circuit breaker lock contention, allowing execution");
            true
        }
    }

    pub fn record_success(&self) {
        if let Ok(mut count) = self.failure_count.lock() {
            *count = 0;
        }
    }

    pub fn record_failure(&self) {
        if let (Ok(mut count), Ok(mut last_fail)) =
            (self.failure_count.lock(), self.last_failure.lock())
        {
            *count += 1;
            *last_fail = std::time::Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_status_code() {
        assert_eq!(safe_status_code(200), StatusCode::OK);
        assert_eq!(safe_status_code(404), StatusCode::NOT_FOUND);
        // Test with an actually invalid status code (>= 1000 are invalid in HTTP)
        let result = safe_status_code(1000);
        assert_eq!(result, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_circuit_breaker() {
        let cb = CircuitBreaker::new(3, 1);

        // Should allow execution initially
        assert!(cb.can_execute());

        // Record failures
        cb.record_failure();
        cb.record_failure();
        assert!(cb.can_execute()); // Still under threshold

        cb.record_failure();
        assert!(!cb.can_execute()); // Should trip circuit breaker

        // Should allow after success
        cb.record_success();
        assert!(cb.can_execute());
    }
}
