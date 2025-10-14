mod common;

use aiwebengine::repository;
use common::{TestContext, wait_for_server};

#[tokio::test]
async fn test_query_parameters() {
    let context = TestContext::new();

    // Dynamically load the query test script
    let _ = repository::upsert_script(
        "https://example.com/query_test",
        include_str!("../scripts/test_scripts/query_test.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test GET request to /api/query without query parameters
    let response_no_query = client
        .get(format!("http://127.0.0.1:{}/api/query", port))
        .send()
        .await
        .expect("GET request without query failed");

    assert_eq!(response_no_query.status(), 200);
    let body_no_query = response_no_query
        .text()
        .await
        .expect("Failed to read response without query");
    assert!(
        body_no_query.contains("Path: /api/query"),
        "Response should contain correct path: {}",
        body_no_query
    );
    assert!(
        body_no_query.contains("Query: none"),
        "Response should indicate no query: {}",
        body_no_query
    );

    // Test GET request to /api/query with query parameters
    let response_with_query = client
        .get(format!(
            "http://127.0.0.1:{}/api/query?id=123&name=test",
            port
        ))
        .send()
        .await
        .expect("GET request with query failed");

    assert_eq!(response_with_query.status(), 200);
    let body_with_query = response_with_query
        .text()
        .await
        .expect("Failed to read response with query");
    assert!(
        body_with_query.contains("Path: /api/query"),
        "Response should contain correct path: {}",
        body_with_query
    );
    assert!(
        body_with_query.contains("Query:")
            && body_with_query.contains("id=123")
            && body_with_query.contains("name=test"),
        "Response should contain parsed query parameters: {}",
        body_with_query
    );

    // Test that handler selection ignores query parameters
    // Both requests should go to the same handler
    assert!(
        body_no_query.contains("/api/query") && body_with_query.contains("/api/query"),
        "Both requests should be handled by the same route"
    );

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
