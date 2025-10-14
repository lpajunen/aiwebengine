mod common;

use aiwebengine::repository;
use std::time::Duration;

#[tokio::test]
async fn js_registered_route_returns_expected() {
    // Use the new TestContext pattern for proper server lifecycle management
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    // Wait for server to be ready - this checks /health which should be registered by core.js
    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to create HTTP client");

    // First verify /health works (this confirms core.js is loaded)
    let health_res = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .expect("Health check failed");

    assert_eq!(
        health_res.status(),
        reqwest::StatusCode::OK,
        "Health endpoint should be OK"
    );

    // Now test the root endpoint which is also registered by core.js
    let res = client
        .get(format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .expect("Request to / failed");

    let status = res.status();
    let body = res.text().await.expect("Failed to read response body");

    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "Expected 200 OK status for /, got {} with body: {}",
        status,
        body
    );

    assert!(
        body.contains("Core handler: OK"),
        "Expected 'Core handler: OK' in response, got: {}",
        body
    );

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn core_js_registers_root_path() {
    // ensure core.js contains a registration for '/'
    let core = repository::fetch_script("https://example.com/core").expect("core script missing");
    assert!(
        core.contains("register('/") || core.contains("register(\"/\""),
        "core.js must register '/' path"
    );
}
