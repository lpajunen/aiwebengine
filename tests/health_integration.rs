use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn test_health_endpoint() {
    // Load the core script which contains the health endpoint
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server in background task
    let port = start_server_without_shutdown()
        .await
        .expect("server failed to start");
    tokio::spawn(async move {
        // Server is already started, just keep it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

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
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server
    let port = start_server_without_shutdown()
        .await
        .expect("server failed to start");
    tokio::spawn(async move {
        // Server is already started, just keep it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    tokio::time::sleep(Duration::from_millis(1000)).await;

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
}

#[tokio::test]
async fn test_script_logs_endpoint() {
    // Load the core script which contains the script_logs endpoint
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Insert some test log messages for a specific URI
    repository::insert_log_message("https://example.com/test-script", "test-log-1");
    repository::insert_log_message("https://example.com/test-script", "test-log-2");
    repository::insert_log_message("https://example.com/other-script", "other-log");

    // Start server in background task
    let port = start_server_without_shutdown()
        .await
        .expect("server failed to start");
    tokio::spawn(async move {
        // Server is already started, just keep it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = reqwest::Client::new();

    // Test script_logs endpoint with valid URI
    let logs_response = client
        .get(format!(
            "http://127.0.0.1:{}/script_logs?uri=https://example.com/test-script",
            port
        ))
        .send()
        .await
        .expect("Script logs request failed");

    assert_eq!(logs_response.status(), 200);

    let logs_body = logs_response
        .text()
        .await
        .expect("Failed to read script logs response");

    // Parse the JSON response
    let logs_json: serde_json::Value =
        serde_json::from_str(&logs_body).expect("Failed to parse script logs response as JSON");

    // Verify the response structure
    assert_eq!(logs_json["uri"], "https://example.com/test-script");
    assert!(logs_json["logs"].is_array());
    assert!(logs_json["count"].is_number());
    assert!(logs_json["timestamp"].is_string());

    // Verify the logs contain the expected messages
    let logs_array = logs_json["logs"].as_array().unwrap();
    assert!(logs_array.iter().any(|v| v == "test-log-1"));
    assert!(logs_array.iter().any(|v| v == "test-log-2"));
    assert!(!logs_array.iter().any(|v| v == "other-log")); // Should not contain logs from other URI

    // Test script_logs endpoint without URI parameter (should return 400)
    let bad_response = client
        .get(format!("http://127.0.0.1:{}/script_logs", port))
        .send()
        .await
        .expect("Script logs bad request failed");

    assert_eq!(bad_response.status(), 400);

    let bad_body = bad_response
        .text()
        .await
        .expect("Failed to read bad request response");

    let bad_json: serde_json::Value =
        serde_json::from_str(&bad_body).expect("Failed to parse bad request response as JSON");

    assert_eq!(bad_json["error"], "Missing required parameter: uri");
}
