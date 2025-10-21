use aiwebengine::http_client::{FetchOptions, HttpClient};
use std::collections::HashMap;

#[test]
fn test_fetch_get_request() {
    // Test a simple GET request to httpbin.org
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "https://httpbin.org/get".to_string(),
        FetchOptions::default(),
    );

    assert!(result.is_ok(), "GET request should succeed");
    let response = result.unwrap();
    assert_eq!(response.status, 200);
    assert!(response.ok);
    assert!(!response.body.is_empty());
}

#[test]
fn test_fetch_post_with_json() {
    let client = HttpClient::new().expect("Failed to create client");

    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());

    let body = r#"{"test": "data", "number": 42}"#;

    let result = client.fetch(
        "https://httpbin.org/post".to_string(),
        FetchOptions {
            method: "POST".to_string(),
            headers: Some(headers),
            body: Some(body.to_string()),
            timeout_ms: None,
        },
    );

    assert!(result.is_ok(), "POST request should succeed");
    let response = result.unwrap();
    assert_eq!(response.status, 200);
    assert!(response.ok);

    // httpbin echoes back the JSON we sent
    assert!(response.body.contains("test"));
    assert!(response.body.contains("data"));
}

#[test]
fn test_fetch_custom_headers() {
    let client = HttpClient::new().expect("Failed to create client");

    let mut headers = HashMap::new();
    headers.insert("X-Custom-Header".to_string(), "test-value".to_string());
    headers.insert("User-Agent".to_string(), "aiwebengine/test".to_string());

    let result = client.fetch(
        "https://httpbin.org/headers".to_string(),
        FetchOptions {
            method: "GET".to_string(),
            headers: Some(headers),
            body: None,
            timeout_ms: None,
        },
    );

    assert!(result.is_ok(), "Request with custom headers should succeed");
    let response = result.unwrap();
    assert_eq!(response.status, 200);

    // httpbin echoes back our headers
    assert!(response.body.contains("X-Custom-Header"));
    assert!(response.body.contains("test-value"));
}

#[test]
fn test_fetch_blocks_localhost() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "http://localhost:8080/api".to_string(),
        FetchOptions::default(),
    );

    assert!(result.is_err(), "Localhost should be blocked");
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Localhost"));
}

#[test]
fn test_fetch_blocks_private_ip() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "http://192.168.1.1/api".to_string(),
        FetchOptions::default(),
    );

    assert!(result.is_err(), "Private IP should be blocked");
    let error = result.unwrap_err();
    assert!(error.to_string().contains("not allowed") || error.to_string().contains("Blocked"));
}

#[test]
fn test_fetch_blocks_127_0_0_1() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "http://127.0.0.1:3000/api".to_string(),
        FetchOptions::default(),
    );

    assert!(result.is_err(), "127.0.0.1 should be blocked");
    let error = result.unwrap_err();
    assert!(error.to_string().contains("not allowed") || error.to_string().contains("Blocked"));
}

#[test]
fn test_fetch_invalid_url_scheme() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "ftp://example.com/file".to_string(),
        FetchOptions::default(),
    );

    assert!(result.is_err(), "FTP scheme should be rejected");
    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only http and https are allowed")
    );
}

#[test]
fn test_fetch_file_scheme_blocked() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch("file:///etc/passwd".to_string(), FetchOptions::default());

    assert!(result.is_err(), "File scheme should be rejected");
}

#[test]
fn test_fetch_invalid_url() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch("not-a-valid-url".to_string(), FetchOptions::default());

    assert!(result.is_err(), "Invalid URL should be rejected");
}

#[test]
fn test_fetch_404_not_found() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "https://httpbin.org/status/404".to_string(),
        FetchOptions::default(),
    );

    assert!(
        result.is_ok(),
        "Request should succeed even with 404 status"
    );
    let response = result.unwrap();
    assert_eq!(response.status, 404);
    assert!(!response.ok); // ok should be false for 4xx/5xx
}

#[test]
fn test_fetch_different_methods() {
    let client = HttpClient::new().expect("Failed to create client");

    // Test PUT
    let result = client.fetch(
        "https://httpbin.org/put".to_string(),
        FetchOptions {
            method: "PUT".to_string(),
            headers: None,
            body: Some("test data".to_string()),
            timeout_ms: Some(10000), // 10 second timeout
        },
    );
    assert!(result.is_ok(), "PUT request failed: {:?}", result.err());
    let response = result.unwrap();
    assert_eq!(
        response.status, 200,
        "PUT request returned unexpected status"
    );
    assert!(response.ok, "PUT request ok flag should be true");

    // Test DELETE
    let result = client.fetch(
        "https://httpbin.org/delete".to_string(),
        FetchOptions {
            method: "DELETE".to_string(),
            headers: None,
            body: None,
            timeout_ms: Some(10000), // 10 second timeout
        },
    );
    assert!(result.is_ok(), "DELETE request failed: {:?}", result.err());
    let response = result.unwrap();
    assert_eq!(
        response.status, 200,
        "DELETE request returned unexpected status"
    );
    assert!(response.ok, "DELETE request ok flag should be true");

    // Test PATCH
    let result = client.fetch(
        "https://httpbin.org/patch".to_string(),
        FetchOptions {
            method: "PATCH".to_string(),
            headers: None,
            body: Some("patch data".to_string()),
            timeout_ms: Some(10000), // 10 second timeout
        },
    );
    assert!(result.is_ok(), "PATCH request failed: {:?}", result.err());
    let response = result.unwrap();
    assert_eq!(
        response.status, 200,
        "PATCH request returned unexpected status"
    );
    assert!(response.ok, "PATCH request ok flag should be true");
}

#[test]
fn test_fetch_response_headers() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "https://httpbin.org/response-headers?custom-header=test-value".to_string(),
        FetchOptions::default(),
    );

    assert!(result.is_ok());
    let response = result.unwrap();
    assert!(!response.headers.is_empty());

    // httpbin should return our custom header
    assert!(response.headers.contains_key("custom-header"));
}

// Test secret injection (requires secrets manager to be initialized)
#[test]
fn test_fetch_secret_template_syntax() {
    // Get or initialize secrets manager
    use aiwebengine::secrets::{
        SecretsManager, get_global_secrets_manager, initialize_global_secrets_manager,
    };
    use std::sync::Arc;

    // Try to get existing manager, or create new one
    let manager = get_global_secrets_manager().unwrap_or_else(|| {
        let mgr = Arc::new(SecretsManager::new());
        initialize_global_secrets_manager(mgr.clone());
        mgr
    });

    // Add our test secret to the manager
    manager.set("test_api_key".to_string(), "secret-key-12345".to_string());

    // Now test fetch with secret template
    let client = HttpClient::new().expect("Failed to create client");

    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        "{{secret:test_api_key}}".to_string(),
    );

    let result = client.fetch(
        "https://httpbin.org/headers".to_string(),
        FetchOptions {
            method: "GET".to_string(),
            headers: Some(headers),
            body: None,
            timeout_ms: None,
        },
    );

    assert!(
        result.is_ok(),
        "Request with secret injection should succeed"
    );
    let response = result.unwrap();
    assert_eq!(response.status, 200);

    // The secret value should have been injected
    assert!(response.body.contains("secret-key-12345"));
    assert!(response.body.contains("Authorization"));
}

#[test]
fn test_fetch_missing_secret_error() {
    // Get or initialize secrets manager
    use aiwebengine::secrets::{
        SecretsManager, get_global_secrets_manager, initialize_global_secrets_manager,
    };
    use std::sync::Arc;

    // Try to get existing manager, or create new one
    let manager = get_global_secrets_manager().unwrap_or_else(|| {
        let mgr = Arc::new(SecretsManager::new());
        initialize_global_secrets_manager(mgr.clone());
        mgr
    });

    // Add a different secret (not the one we'll request)
    manager.set("some_other_key".to_string(), "other-value".to_string());

    let client = HttpClient::new().expect("Failed to create client");

    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        "{{secret:nonexistent_key}}".to_string(),
    );

    let result = client.fetch(
        "https://httpbin.org/headers".to_string(),
        FetchOptions {
            method: "GET".to_string(),
            headers: Some(headers),
            body: None,
            timeout_ms: None,
        },
    );

    assert!(result.is_err(), "Missing secret should cause error");
    let error = result.unwrap_err();
    // Accept either error message depending on initialization state
    assert!(
        error.to_string().contains("Secret not found")
            || error
                .to_string()
                .contains("Secrets manager not initialized")
    );
}
