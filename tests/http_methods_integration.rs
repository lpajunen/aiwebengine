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

    // Start server in background task
    tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = reqwest::Client::new();

    // Test GET request to /api/test
    let get_response = client
        .get("http://127.0.0.1:4000/api/test")
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
        .post("http://127.0.0.1:4000/api/test")
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
        .put("http://127.0.0.1:4000/api/test")
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
        .delete("http://127.0.0.1:4000/api/test")
        .send()
        .await
        .expect("DELETE request failed");
    assert_eq!(delete_response.status(), 204);

    // Test method validation - wrong method should return 405 Method Not Allowed
    // Try PATCH on a path that only has GET/POST/PUT/DELETE registered
    let patch_response = client
        .patch("http://127.0.0.1:4000/api/test")
        .send()
        .await
        .expect("PATCH request failed");
    assert_eq!(patch_response.status(), 405);

    // Test unregistered path returns 404
    let not_found_response = client
        .get("http://127.0.0.1:4000/api/nonexistent")
        .send()
        .await
        .expect("Request to nonexistent path failed");
    assert_eq!(not_found_response.status(), 404);
}
