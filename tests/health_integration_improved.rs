use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;
use tokio::time::timeout;

/// Improved health endpoint test with proper timeout and error handling
#[tokio::test]
async fn test_health_endpoint_improved() {
    // Load the core script which contains the health endpoint
    repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/core.js"),
    );

    // Start server with timeout
    let server_future = start_server_without_shutdown();
    let port_result = timeout(Duration::from_secs(5), server_future).await;

    let port = match port_result {
        Ok(Ok(port)) => {
            println!("Server started on port: {}", port);
            port
        }
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Test health endpoint with timeout
    let health_future = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send();

    let health_response = match timeout(Duration::from_secs(5), health_future).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Health request failed: {:?}", e),
        Err(_) => panic!("Health request timed out"),
    };

    assert_eq!(health_response.status(), 200);

    let health_body = match timeout(Duration::from_secs(5), health_response.text()).await {
        Ok(Ok(body)) => body,
        Ok(Err(e)) => panic!("Failed to read health response: {:?}", e),
        Err(_) => panic!("Reading health response timed out"),
    };

    // Parse the JSON response
    let health_json: serde_json::Value =
        serde_json::from_str(&health_body).expect("Failed to parse health response as JSON");

    // Verify the health response structure
    assert_eq!(health_json["status"], "healthy");
    assert!(health_json["timestamp"].is_string());
    assert!(health_json["checks"].is_object());

    // Verify individual checks
    let checks = &health_json["checks"];
    assert!(checks["javascript"].is_string()); // Could be "ok" or "failed"
    assert_eq!(checks["json"], "ok");

    println!("Health check response: {}", health_body);
}

/// Test that demonstrates running multiple tests without port conflicts
#[tokio::test]
async fn test_multiple_endpoints_same_server() {
    // Load the core script
    repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/core.js"),
    );

    // Start server
    let port_result = timeout(Duration::from_secs(5), start_server_without_shutdown()).await;
    let port = match port_result {
        Ok(Ok(port)) => {
            println!("Server started on port: {}", port);
            port
        }
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Test multiple endpoints
    let endpoints = vec!["/health", "/api/scripts"];

    for endpoint in endpoints {
        let response_future = client
            .get(format!("http://127.0.0.1:{}{}", port, endpoint))
            .send();

        let response = match timeout(Duration::from_secs(5), response_future).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => panic!("Request to {} failed: {:?}", endpoint, e),
            Err(_) => panic!("Request to {} timed out", endpoint),
        };

        assert!(
            response.status().is_success(),
            "Endpoint {} returned status {}",
            endpoint,
            response.status()
        );
        println!("✓ {} returned status {}", endpoint, response.status());
    }
}
