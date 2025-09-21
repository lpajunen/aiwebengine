use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn test_different_http_methods() {
    // Dynamically load the method test script
    repository::upsert_script(
        "https://example.com/method_test",
        include_str!("../scripts/method_test.js"),
    );

    // Start server with timeout
    let port = tokio::time::timeout(Duration::from_secs(5), start_server_without_shutdown())
        .await
        .expect("Server startup timed out")
        .expect("Server failed to start");

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to create HTTP client");

    // Test GET request to /api/test
    let get_response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://127.0.0.1:{}/api/test", port))
            .send(),
    )
    .await
    .expect("GET request timed out")
    .expect("GET request failed");

    assert_eq!(get_response.status(), 200);
    let get_body = get_response
        .text()
        .await
        .expect("Failed to read GET response");
    assert!(
        get_body.contains("GET request to /api/test"),
        "GET response incorrect: {}",
        get_body
    );

    // Test POST request to /api/test
    let post_response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .post(format!("http://127.0.0.1:{}/api/test", port))
            .send(),
    )
    .await
    .expect("POST request timed out")
    .expect("POST request failed");

    assert_eq!(post_response.status(), 201);
    let post_body = post_response
        .text()
        .await
        .expect("Failed to read POST response");
    assert!(
        post_body.contains("POST request to /api/test"),
        "POST response incorrect: {}",
        post_body
    );
    assert!(
        post_body.contains("with method POST"),
        "POST method not in response: {}",
        post_body
    );

    // Test PUT request to /api/test
    let put_response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .put(format!("http://127.0.0.1:{}/api/test", port))
            .send(),
    )
    .await
    .expect("PUT request timed out")
    .expect("PUT request failed");

    assert_eq!(put_response.status(), 200);
    let put_body = put_response
        .text()
        .await
        .expect("Failed to read PUT response");
    assert!(
        put_body.contains("PUT request to /api/test"),
        "PUT response incorrect: {}",
        put_body
    );

    // Test DELETE request to /api/test
    let delete_response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .delete(format!("http://127.0.0.1:{}/api/test", port))
            .send(),
    )
    .await
    .expect("DELETE request timed out")
    .expect("DELETE request failed");

    assert_eq!(delete_response.status(), 204);

    // Test method validation - wrong method should return 405 Method Not Allowed
    // Try PATCH on a path that only has GET/POST/PUT/DELETE registered
    let patch_response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .patch(format!("http://127.0.0.1:{}/api/test", port))
            .send(),
    )
    .await
    .expect("PATCH request timed out")
    .expect("PATCH request failed");

    assert_eq!(patch_response.status(), 405);

    // Test unregistered path returns 404
    let not_found_response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://127.0.0.1:{}/api/nonexistent", port))
            .send(),
    )
    .await
    .expect("Request to nonexistent path timed out")
    .expect("Request to nonexistent path failed");

    assert_eq!(not_found_response.status(), 404);
}
