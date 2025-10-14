mod common;

use aiwebengine::repository;
use common::{TestContext, wait_for_server};

#[tokio::test]
async fn test_different_http_methods() {
    let context = TestContext::new();

    // Dynamically load the method test script
    let _ = repository::upsert_script(
        "https://example.com/method_test",
        include_str!("../scripts/test_scripts/method_test.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test GET request to /api/test
    let get_response = client
        .get(format!("http://127.0.0.1:{}/api/test", port))
        .send()
        .await
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
    let post_response = client
        .post(format!("http://127.0.0.1:{}/api/test", port))
        .send()
        .await
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
    let put_response = client
        .put(format!("http://127.0.0.1:{}/api/test", port))
        .send()
        .await
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
    let delete_response = client
        .delete(format!("http://127.0.0.1:{}/api/test", port))
        .send()
        .await
        .expect("DELETE request failed");

    assert_eq!(delete_response.status(), 204);

    // Test method validation - wrong method should return 405 Method Not Allowed
    // Try PATCH on a path that only has GET/POST/PUT/DELETE registered
    let patch_response = client
        .patch(format!("http://127.0.0.1:{}/api/test", port))
        .send()
        .await
        .expect("PATCH request failed");

    assert_eq!(patch_response.status(), 405);

    // Test unregistered path returns 404
    let not_found_response = client
        .get(format!("http://127.0.0.1:{}/api/nonexistent", port))
        .send()
        .await
        .expect("Request to nonexistent path failed");

    assert_eq!(not_found_response.status(), 404);

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
