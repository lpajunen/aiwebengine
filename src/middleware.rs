use axum::{extract::Request, http::HeaderMap, middleware::Next, response::Response};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::debug;

/// Global counter for generating request IDs
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Header name for request ID
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Middleware that generates and injects request IDs
pub async fn request_id_middleware(request: Request, next: Next) -> Response {
    let request_id = generate_request_id();

    // Add request ID to request extensions for use in handlers
    let mut request = request;
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    // Add request ID to response headers
    let mut response = next.run(request).await;

    if let Ok(header_value) = axum::http::HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(
            axum::http::header::HeaderName::from_static(REQUEST_ID_HEADER),
            header_value,
        );
    }

    debug!("Request {} processed", request_id);
    response
}

/// Generate a unique request ID
pub fn generate_request_id() -> String {
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    format!("req_{}_{}", timestamp, counter)
}

/// Extract request ID from request headers or generate a new one
pub fn extract_or_generate_request_id(headers: &HeaderMap) -> String {
    if let Some(request_id) = headers.get(REQUEST_ID_HEADER)
        && let Ok(id) = request_id.to_str()
    {
        return id.to_string();
    }

    generate_request_id()
}

/// Type for storing request ID in request extensions
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Helper trait to get request ID from various sources
pub trait HasRequestId {
    fn request_id(&self) -> &str;
}

impl HasRequestId for RequestId {
    fn request_id(&self) -> &str {
        &self.0
    }
}

impl HasRequestId for &RequestId {
    fn request_id(&self) -> &str {
        &self.0
    }
}

impl HasRequestId for String {
    fn request_id(&self) -> &str {
        self
    }
}

impl HasRequestId for &str {
    fn request_id(&self) -> &str {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn test_generate_request_id() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();

        // Should generate different IDs
        assert_ne!(id1, id2);

        // Should follow the format "req_{timestamp}_{counter}"
        assert!(id1.starts_with("req_"));
        assert!(id2.starts_with("req_"));

        // Should have at least 3 parts when split by underscore
        let parts1: Vec<&str> = id1.split('_').collect();
        let parts2: Vec<&str> = id2.split('_').collect();
        assert_eq!(parts1.len(), 3);
        assert_eq!(parts2.len(), 3);
        assert_eq!(parts1[0], "req");
        assert_eq!(parts2[0], "req");

        // Counter should increase (may not be sequential due to parallel tests)
        let counter1: u64 = parts1[2].parse().expect("Test data should be valid");
        let counter2: u64 = parts2[2].parse().expect("Test data should be valid");
        assert!(counter2 > counter1);
    }

    #[test]
    fn test_extract_or_generate_request_id_with_existing_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_static("existing-req-123"),
        );

        let id = extract_or_generate_request_id(&headers);
        assert_eq!(id, "existing-req-123");
    }

    #[test]
    fn test_extract_or_generate_request_id_without_header() {
        let headers = HeaderMap::new();
        let id = extract_or_generate_request_id(&headers);

        // Should generate a new ID
        assert!(id.starts_with("req_"));
    }

    #[test]
    fn test_extract_or_generate_request_id_with_invalid_header() {
        let mut headers = HeaderMap::new();
        // Insert invalid UTF-8 header value
        headers.insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_bytes(&[0xff, 0xfe]).expect("Test setup should work"),
        );

        let id = extract_or_generate_request_id(&headers);
        // Should generate new ID when header is invalid UTF-8
        assert!(id.starts_with("req_"));
    }

    #[test]
    fn test_request_id_struct() {
        let request_id = RequestId("test-123".to_string());
        assert_eq!(request_id.0, "test-123");

        let cloned = request_id.clone();
        assert_eq!(cloned.0, request_id.0);

        // Test Debug trait
        let debug_str = format!("{:?}", request_id);
        assert!(debug_str.contains("test-123"));
    }

    #[test]
    fn test_has_request_id_trait_implementations() {
        // Test RequestId implementation
        let request_id = RequestId("test-456".to_string());
        assert_eq!(request_id.request_id(), "test-456");

        // Test &RequestId implementation
        let request_id_ref = &request_id;
        assert_eq!(request_id_ref.request_id(), "test-456");

        // Test String implementation
        let string_id = "test-789".to_string();
        assert_eq!(string_id.request_id(), "test-789");

        // Test &str implementation
        let str_id = "test-abc";
        assert_eq!(str_id.request_id(), "test-abc");
    }

    #[test]
    fn test_request_counter_increments() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        let id3 = generate_request_id();

        // Extract counters from IDs
        let counter1: u64 = id1
            .split('_')
            .nth(2)
            .expect("Generated ID should have proper format")
            .parse()
            .expect("Counter should be valid u64");
        let counter2: u64 = id2
            .split('_')
            .nth(2)
            .expect("Generated ID should have proper format")
            .parse()
            .expect("Counter should be valid u64");
        let counter3: u64 = id3
            .split('_')
            .nth(2)
            .expect("Generated ID should have proper format")
            .parse()
            .expect("Counter should be valid u64");

        // Counters should increase (order preserved within single thread)
        assert!(counter2 > counter1);
        assert!(counter3 > counter2);
    }

    #[test]
    fn test_request_id_header_constant() {
        assert_eq!(REQUEST_ID_HEADER, "x-request-id");
    }

    #[test]
    fn test_request_id_header_value_creation() {
        // Test that generated request IDs can be converted to valid header values
        let id = generate_request_id();
        let header_value_result = axum::http::HeaderValue::from_str(&id);

        // Should successfully create header value from generated ID
        assert!(header_value_result.is_ok());

        let header_value =
            header_value_result.expect("Generated request ID should be valid header value");
        let converted_back = header_value
            .to_str()
            .expect("Header value should convert back to string");
        assert_eq!(converted_back, id);
    }

    #[test]
    fn test_request_id_concurrent_generation() {
        // Test that concurrent request ID generation produces unique IDs
        use std::sync::{Arc, Mutex};
        use std::thread;

        let ids = Arc::new(Mutex::new(Vec::new()));
        let mut handles = vec![];

        // Spawn multiple threads generating request IDs
        for _ in 0..10 {
            let ids_clone = ids.clone();
            let handle = thread::spawn(move || {
                let id = generate_request_id();
                ids_clone
                    .lock()
                    .expect("Test mutex should not be poisoned")
                    .push(id);
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread should complete successfully");
        }

        let final_ids = ids.lock().expect("Test mutex should not be poisoned");
        assert_eq!(final_ids.len(), 10);

        // Check that all IDs are unique
        let mut unique_ids = final_ids.clone();
        unique_ids.sort();
        unique_ids.dedup();
        assert_eq!(unique_ids.len(), final_ids.len());
    }

    #[test]
    fn test_request_id_format_consistency() {
        // Generate multiple IDs and verify they all follow the same format
        let ids: Vec<String> = (0..10).map(|_| generate_request_id()).collect();

        for id in &ids {
            assert!(id.starts_with("req_"));
            let parts: Vec<&str> = id.split('_').collect();
            assert_eq!(parts.len(), 3);
            assert_eq!(parts[0], "req");

            // Timestamp should be parseable as u128
            let timestamp: u128 = parts[1].parse().expect("Invalid timestamp format");
            assert!(timestamp > 0);

            // Counter should be parseable as u64
            let _counter: u64 = parts[2].parse().expect("Invalid counter format");
        }

        // All IDs should be unique
        let mut unique_ids = ids.clone();
        unique_ids.sort();
        unique_ids.dedup();
        assert_eq!(unique_ids.len(), ids.len());
    }

    #[test]
    fn test_multiple_extract_calls_same_header() {
        let mut headers = HeaderMap::new();
        headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("stable-id-123"));

        let id1 = extract_or_generate_request_id(&headers);
        let id2 = extract_or_generate_request_id(&headers);

        // Should return same ID when header is present
        assert_eq!(id1, "stable-id-123");
        assert_eq!(id2, "stable-id-123");
        assert_eq!(id1, id2);
    }
}
