use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn test_health_endpoint() {
    // Load the core script which contains the health endpoint
    repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/core.js"),
    );

    // Start server in background task
    tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = reqwest::Client::new();

    // Test health endpoint
    let health_response = client
        .get("http://127.0.0.1:4000/health")
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

    // Verify individual checks
    let checks = &health_json["checks"];
    assert_eq!(checks["javascript"], "ok");
    assert!(checks["logging"].is_string()); // Could be "ok" or "failed"
    assert_eq!(checks["json"], "ok");

    println!("Health check response: {}", health_body);
}

#[tokio::test]
async fn test_health_endpoint_content_type() {
    // Load the core script
    repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/core.js"),
    );

    // Start server
    tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
    });

    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = reqwest::Client::new();

    // Test that the health endpoint returns correct content type
    let response = client
        .get("http://127.0.0.1:4000/health")
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
}
