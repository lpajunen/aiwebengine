// Example: Optimized version of health_integration.rs
mod common;

use aiwebengine::repository;
use common::{TestContext, wait_for_server};

#[tokio::test]
async fn test_health_endpoint() {
    let context = TestContext::new();

    // Load the core script which contains the health endpoint
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server with proper shutdown support
    let port = context
        .start_server()
        .await
        .expect("server failed to start");

    // Wait for server to be ready (replaces long sleep)
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test health endpoint
    let health_response = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .expect("Health check request failed");

    assert_eq!(health_response.status(), 200);

    let health_body = health_response
        .text()
        .await
        .expect("Failed to read health response");

    // Parse the JSON response
    let health_json: serde_json::Value =
        serde_json::from_str(&health_body).expect("Failed to parse health response as JSON");

    // Verify the health response structure
    assert_eq!(health_json["status"], "healthy");
    assert!(health_json["timestamp"].is_string());
    assert!(health_json["checks"].is_object());

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_health_endpoint_content_type() {
    let context = TestContext::new();

    // Load the core script
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test that the health endpoint returns correct content type
    let response = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .expect("Health request failed");

    let content_type = response
        .headers()
        .get("content-type")
        .expect("Content-Type header missing")
        .to_str()
        .expect("Content-Type header not valid string");

    assert_eq!(content_type, "application/json");

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_script_logs_endpoint() {
    let context = TestContext::new();

    // Load the core script which contains the script_logs endpoint
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test script_logs endpoint with a valid URI parameter
    let logs_response = client
        .get(format!(
            "http://127.0.0.1:{}/script_logs?uri=https://example.com/core",
            port
        ))
        .send()
        .await
        .expect("Script logs request failed");

    assert_eq!(logs_response.status(), 200);

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
