use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn js_script_mgmt_functions_work() {
    // upsert the management test script so it registers /js-mgmt-check and the upsert logic
    repository::upsert_script(
        "https://example.com/js-mgmt-test",
        include_str!("../scripts/js_script_mgmt_test.js"),
    );

    // Start server with timeout
    let server_future = start_server_without_shutdown();
    let port = match timeout(Duration::from_secs(5), server_future).await {
        Ok(Ok(port)) => port,
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    println!("Server started on port: {}", port);

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();

    // Test the management endpoint with timeout
    let mgmt_request = client
        .get(format!("http://127.0.0.1:{}/js-mgmt-check", port))
        .send();

    let res = match timeout(Duration::from_secs(5), mgmt_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Management check request failed: {:?}", e),
        Err(_) => panic!("Management check request timed out"),
    };

    let body = match timeout(Duration::from_secs(5), res.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read management check response: {:?}", e),
        Err(_) => panic!("Reading management check response timed out"),
    };

    let v: serde_json::Value = serde_json::from_str(&body).expect("Expected JSON response");
    let obj = v.as_object().expect("Expected object");

    // got may be null or a string
    assert!(obj.contains_key("got"), "Response missing 'got' field");
    assert!(obj.contains_key("list"), "Response missing 'list' field");
    assert!(obj.contains_key("deleted_before"), "Response missing 'deleted_before' field");
    assert!(obj.contains_key("deleted"), "Response missing 'deleted' field");
    assert!(obj.contains_key("after"), "Response missing 'after' field");

    // list should be an array containing some known URIs
    let list = obj.get("list").unwrap().as_array().expect("list should be an array");
    assert!(list.iter().any(|v| {
        v.as_str()
            .map(|s| s.contains("example.com/core"))
            .unwrap_or(false)
    }), "Expected core script in list");

    // verify the upserted script was deleted via deleteScript and is no longer callable
    let delete_check_request = client
        .get(format!("http://127.0.0.1:{}/from-js", port))
        .send();

    let res2 = match timeout(Duration::from_secs(5), delete_check_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Delete check request failed: {:?}", e),
        Err(_) => panic!("Delete check request timed out"),
    };

    let status = res2.status();
    // The endpoint should now return 404 since the script was deleted
    assert_eq!(status, 404, "Expected 404 for deleted script endpoint, got {}", status);
}

#[tokio::test]
async fn test_upsert_script_endpoint() {
    // Start server with timeout
    let server_future = start_server_without_shutdown();
    let port = match timeout(Duration::from_secs(5), server_future).await {
        Ok(Ok(port)) => port,
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    println!("Server started on port: {}", port);

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();

    // Test the upsert_script endpoint
    let test_script_content = r#"
function test_endpoint_handler(req) {
    return { status: 200, body: 'Test endpoint works!' };
}
register('/test-endpoint', 'test_endpoint_handler', 'GET');
"#;

    let upsert_request = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/test-endpoint-script"),
            ("content", test_script_content),
        ])
        .send();

    let response = match timeout(Duration::from_secs(5), upsert_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /upsert_script failed: {:?}", e),
        Err(_) => panic!("POST request to /upsert_script timed out"),
    };

    assert_eq!(response.status(), 200, "Expected 200 status for upsert_script");

    let body: serde_json::Value = match timeout(Duration::from_secs(5), response.json()).await {
        Ok(Ok(json)) => json,
        Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
        Err(_) => panic!("Reading JSON response timed out"),
    };

    assert_eq!(body["success"], true, "Expected success=true in response");
    assert_eq!(body["uri"], "https://example.com/test-endpoint-script", "Expected correct URI in response");
    assert!(body["contentLength"].as_u64().unwrap() > 0, "Expected contentLength > 0");

    // Verify the script was actually upserted by calling the new endpoint
    tokio::time::sleep(Duration::from_millis(100)).await; // Give time for script to be processed

    let test_endpoint_request = client
        .get(format!("http://127.0.0.1:{}/test-endpoint", port))
        .send();

    let test_response = match timeout(Duration::from_secs(5), test_endpoint_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("GET request to test endpoint failed: {:?}", e),
        Err(_) => panic!("GET request to test endpoint timed out"),
    };

    assert_eq!(test_response.status(), 200, "Expected 200 status for test endpoint");

    let test_body = match timeout(Duration::from_secs(5), test_response.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read test response: {:?}", e),
        Err(_) => panic!("Reading test response timed out"),
    };

    assert_eq!(test_body, "Test endpoint works!", "Expected correct response body");
}

#[tokio::test]
async fn test_delete_script_endpoint() {
    // Start server with timeout
    let server_future = start_server_without_shutdown();
    let port = match timeout(Duration::from_secs(5), server_future).await {
        Ok(Ok(port)) => port,
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    println!("Server started on port: {}", port);

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();

    // First, upsert a test script
    let test_script_content = r#"
function delete_test_handler(req) {
    return { status: 200, body: 'Delete test endpoint works!' };
}
register('/delete-test-endpoint', 'delete_test_handler', 'GET');
"#;

    let upsert_request = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/delete-test-script"),
            ("content", test_script_content),
        ])
        .send();

    let upsert_response = match timeout(Duration::from_secs(5), upsert_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /upsert_script failed: {:?}", e),
        Err(_) => panic!("POST request to /upsert_script timed out"),
    };

    assert_eq!(upsert_response.status(), 200, "Expected 200 status for upsert_script");

    // Verify the script was upserted
    tokio::time::sleep(Duration::from_millis(100)).await;

    let verify_request = client
        .get(format!("http://127.0.0.1:{}/delete-test-endpoint", port))
        .send();

    let test_response = match timeout(Duration::from_secs(5), verify_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("GET request to delete test endpoint failed: {:?}", e),
        Err(_) => panic!("GET request to delete test endpoint timed out"),
    };

    assert_eq!(test_response.status(), 200, "Expected 200 status for upserted endpoint");

    // Now test the delete_script endpoint
    let delete_request = client
        .post(format!("http://127.0.0.1:{}/delete_script", port))
        .form(&[("uri", "https://example.com/delete-test-script")])
        .send();

    let delete_response = match timeout(Duration::from_secs(5), delete_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /delete_script failed: {:?}", e),
        Err(_) => panic!("POST request to /delete_script timed out"),
    };

    assert_eq!(delete_response.status(), 200, "Expected 200 status for delete_script");

    let delete_body: serde_json::Value = match timeout(Duration::from_secs(5), delete_response.json()).await {
        Ok(Ok(json)) => json,
        Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
        Err(_) => panic!("Reading JSON response timed out"),
    };

    assert_eq!(delete_body["success"], true, "Expected success=true in delete response");
    assert_eq!(delete_body["uri"], "https://example.com/delete-test-script", "Expected correct URI in delete response");

    // Verify the script was actually deleted by checking the endpoint returns 404
    tokio::time::sleep(Duration::from_millis(100)).await; // Give time for script to be deleted

    let after_delete_request = client
        .get(format!("http://127.0.0.1:{}/delete-test-endpoint", port))
        .send();

    let after_delete_response = match timeout(Duration::from_secs(5), after_delete_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("GET request to delete test endpoint after deletion failed: {:?}", e),
        Err(_) => panic!("GET request to delete test endpoint after deletion timed out"),
    };

    assert_eq!(after_delete_response.status(), 404, "Expected 404 for deleted script endpoint");

    // Test deleting a non-existent script
    let nonexistent_delete_request = client
        .post(format!("http://127.0.0.1:{}/delete_script", port))
        .form(&[("uri", "https://example.com/nonexistent-script")])
        .send();

    let nonexistent_delete_response = match timeout(Duration::from_secs(5), nonexistent_delete_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /delete_script for nonexistent script failed: {:?}", e),
        Err(_) => panic!("POST request to /delete_script for nonexistent script timed out"),
    };

    assert_eq!(nonexistent_delete_response.status(), 404, "Expected 404 for nonexistent script deletion");

    let nonexistent_body: serde_json::Value = match timeout(Duration::from_secs(5), nonexistent_delete_response.json()).await {
        Ok(Ok(json)) => json,
        Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
        Err(_) => panic!("Reading JSON response timed out"),
    };

    assert_eq!(nonexistent_body["error"], "Script not found", "Expected 'Script not found' error");
}

#[tokio::test]
async fn test_script_lifecycle_via_http_api() {
    // Start server with timeout
    let server_future = start_server_without_shutdown();
    let port = match timeout(Duration::from_secs(5), server_future).await {
        Ok(Ok(port)) => port,
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    println!("Server started on port: {}", port);

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();

    // Test script content
    let script_content = r#"
function lifecycle_test_handler(req) {
    return { status: 200, body: 'Lifecycle test successful!' };
}
register('/lifecycle-test', 'lifecycle_test_handler', 'GET');
"#;

    // 1. Create script via HTTP API
    let create_request = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/lifecycle-test-script"),
            ("content", script_content),
        ])
        .send();

    let create_response = match timeout(Duration::from_secs(5), create_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Failed to create script via HTTP API: {:?}", e),
        Err(_) => panic!("Create script request timed out"),
    };

    assert_eq!(create_response.status(), 200, "Expected 200 status for script creation");

    // 2. Verify script works
    tokio::time::sleep(Duration::from_millis(100)).await;

    let test_request = client
        .get(format!("http://127.0.0.1:{}/lifecycle-test", port))
        .send();

    let test_response = match timeout(Duration::from_secs(5), test_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Failed to test script endpoint: {:?}", e),
        Err(_) => panic!("Test script request timed out"),
    };

    assert_eq!(test_response.status(), 200, "Expected 200 status for lifecycle test");

    let test_body = match timeout(Duration::from_secs(5), test_response.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read test response: {:?}", e),
        Err(_) => panic!("Reading test response timed out"),
    };

    assert_eq!(test_body, "Lifecycle test successful!", "Expected correct lifecycle test response");

    // 3. Delete script via HTTP API
    let delete_request = client
        .post(format!("http://127.0.0.1:{}/delete_script", port))
        .form(&[("uri", "https://example.com/lifecycle-test-script")])
        .send();

    let delete_response = match timeout(Duration::from_secs(5), delete_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Failed to delete script via HTTP API: {:?}", e),
        Err(_) => panic!("Delete script request timed out"),
    };

    assert_eq!(delete_response.status(), 200, "Expected 200 status for script deletion");

    // 4. Verify script is gone
    tokio::time::sleep(Duration::from_millis(100)).await;

    let after_delete_request = client
        .get(format!("http://127.0.0.1:{}/lifecycle-test", port))
        .send();

    let after_delete_response = match timeout(Duration::from_secs(5), after_delete_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Failed to check deleted script endpoint: {:?}", e),
        Err(_) => panic!("Check deleted script request timed out"),
    };

    assert_eq!(after_delete_response.status(), 404, "Expected 404 for deleted script endpoint");
}
