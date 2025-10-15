// Test to verify core.js init() function is called
mod common;

use aiwebengine::repository;
use common::TestContext;

#[tokio::test]
async fn test_core_script_init_called() {
    let context = TestContext::new();

    // Load the core script before starting the server
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server - this should call init() on all scripts
    let port = context
        .start_server()
        .await
        .expect("server failed to start");

    // Give server extra time to initialize scripts
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let client = reqwest::Client::new();

    // First, check the init status via the global function
    let init_status_result = repository::get_script_metadata("https://example.com/core");
    println!("Init status via repository: {:?}", init_status_result);

    // Test that the health endpoint was registered by init()
    let health_response = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .expect("Health check request failed");

    let status = health_response.status();
    println!("Health response status: {}", status);

    let health_body = health_response
        .text()
        .await
        .expect("Failed to read health response");

    println!("Health response body: {}", health_body);

    assert_eq!(status, 200, "Health endpoint should return 200");

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
