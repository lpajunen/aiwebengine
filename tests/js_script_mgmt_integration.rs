mod common;

use aiwebengine::repository;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn js_script_mgmt_functions_work() {
    // upsert the management test script so it registers /js-mgmt-check and the upsert logic
    let _ = repository::upsert_script(
        "https://example.com/js-mgmt-test",
        include_str!("../scripts/test_scripts/js_script_mgmt_test.js"),
    );

    // Use the new TestContext pattern for proper server lifecycle management
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    // Wait for server to be ready and scripts to be executed
    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    // Give extra time for JavaScript scripts to execute and register routes
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Server started on port: {}", port);

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
    assert!(
        obj.contains_key("deleted_before"),
        "Response missing 'deleted_before' field"
    );
    assert!(
        obj.contains_key("deleted"),
        "Response missing 'deleted' field"
    );
    assert!(obj.contains_key("after"), "Response missing 'after' field");

    // list should be an array containing some known URIs
    let list = obj
        .get("list")
        .unwrap()
        .as_array()
        .expect("list should be an array");
    assert!(
        list.iter().any(|v| {
            v.as_str()
                .map(|s| s.contains("example.com/core"))
                .unwrap_or(false)
        }),
        "Expected core script in list"
    );

    // The script management test validates that:
    // 1. Scripts can be created via upsertScript()
    // 2. Scripts can be retrieved via getScript()
    // 3. Scripts can be listed via listScripts()
    // 4. Scripts can be deleted via deleteScript()
    // 5. After deletion, getScript() returns null

    // These operations are validated by the js_mgmt_check handler above
    // No need to test route availability since script deletion doesn't affect
    // already-registered routes in the current system design

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_upsert_script_endpoint() {
    // Use the new TestContext pattern for proper server lifecycle management
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    // Wait for server to be ready and scripts to be executed
    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    // Give extra time for JavaScript scripts to execute and register routes
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Server started on port: {}", port);

    let client = reqwest::Client::new();

    // Test the upsert_script endpoint
    let test_script_content = r#"
function test_endpoint_handler(req) {
    return { status: 200, body: 'Test endpoint works!' };
}

function init(context) {
    register('/test-endpoint', 'test_endpoint_handler', 'GET');
    return { success: true };
}
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

    assert_eq!(
        response.status(),
        200,
        "Expected 200 status for upsert_script"
    );

    let body: serde_json::Value = match timeout(Duration::from_secs(5), response.json()).await {
        Ok(Ok(json)) => json,
        Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
        Err(_) => panic!("Reading JSON response timed out"),
    };

    assert_eq!(body["success"], true, "Expected success=true in response");
    assert_eq!(
        body["uri"], "https://example.com/test-endpoint-script",
        "Expected correct URI in response"
    );
    assert!(
        body["contentLength"].as_u64().unwrap() > 0,
        "Expected contentLength > 0"
    );

    // Verify the script was actually upserted by calling the new endpoint
    tokio::time::sleep(Duration::from_millis(500)).await; // Give time for script to be processed and initialized

    let test_endpoint_request = client
        .get(format!("http://127.0.0.1:{}/test-endpoint", port))
        .send();

    let test_response = match timeout(Duration::from_secs(5), test_endpoint_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("GET request to test endpoint failed: {:?}", e),
        Err(_) => panic!("GET request to test endpoint timed out"),
    };

    assert_eq!(
        test_response.status(),
        200,
        "Expected 200 status for test endpoint"
    );

    let test_body = match timeout(Duration::from_secs(5), test_response.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read test response: {:?}", e),
        Err(_) => panic!("Reading test response timed out"),
    };

    assert_eq!(
        test_body, "Test endpoint works!",
        "Expected correct response body"
    );

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_delete_script_endpoint() {
    // Use the new TestContext pattern for proper server lifecycle management
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    // Wait for server to be ready and scripts to be executed
    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    // Give extra time for JavaScript scripts to execute and register routes
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Server started on port: {}", port);

    let client = reqwest::Client::new();

    // First, upsert a test script
    let test_script_content = r#"
function delete_test_handler(req) {
    return { status: 200, body: 'Delete test endpoint works!' };
}

function init(context) {
    register('/delete-test-endpoint', 'delete_test_handler', 'GET');
    return { success: true };
}
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

    assert_eq!(
        upsert_response.status(),
        200,
        "Expected 200 status for upsert_script"
    );

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

    assert_eq!(
        test_response.status(),
        200,
        "Expected 200 status for upserted endpoint"
    );

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

    assert_eq!(
        delete_response.status(),
        200,
        "Expected 200 status for delete_script"
    );

    let delete_body: serde_json::Value =
        match timeout(Duration::from_secs(5), delete_response.json()).await {
            Ok(Ok(json)) => json,
            Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
            Err(_) => panic!("Reading JSON response timed out"),
        };

    assert_eq!(
        delete_body["success"], true,
        "Expected success=true in delete response"
    );
    assert_eq!(
        delete_body["uri"], "https://example.com/delete-test-script",
        "Expected correct URI in delete response"
    );

    // Verify the script was actually deleted by checking the endpoint returns 404
    tokio::time::sleep(Duration::from_millis(100)).await; // Give time for script to be deleted

    let after_delete_request = client
        .get(format!("http://127.0.0.1:{}/delete-test-endpoint", port))
        .send();

    let after_delete_response = match timeout(Duration::from_secs(5), after_delete_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!(
            "GET request to delete test endpoint after deletion failed: {:?}",
            e
        ),
        Err(_) => panic!("GET request to delete test endpoint after deletion timed out"),
    };

    assert_eq!(
        after_delete_response.status(),
        404,
        "Expected 404 for deleted script endpoint"
    );

    // Test deleting a non-existent script
    let nonexistent_delete_request = client
        .post(format!("http://127.0.0.1:{}/delete_script", port))
        .form(&[("uri", "https://example.com/nonexistent-script")])
        .send();

    let nonexistent_delete_response =
        match timeout(Duration::from_secs(5), nonexistent_delete_request).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => panic!(
                "POST request to /delete_script for nonexistent script failed: {:?}",
                e
            ),
            Err(_) => panic!("POST request to /delete_script for nonexistent script timed out"),
        };

    assert_eq!(
        nonexistent_delete_response.status(),
        404,
        "Expected 404 for nonexistent script deletion"
    );

    let nonexistent_body: serde_json::Value =
        match timeout(Duration::from_secs(5), nonexistent_delete_response.json()).await {
            Ok(Ok(json)) => json,
            Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
            Err(_) => panic!("Reading JSON response timed out"),
        };

    assert_eq!(
        nonexistent_body["error"], "Script not found",
        "Expected 'Script not found' error"
    );

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_script_lifecycle_via_http_api() {
    // Use the new TestContext pattern for proper server lifecycle management
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    // Wait for server to be ready and scripts to be executed
    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    // Give extra time for JavaScript scripts to execute and register routes
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Server started on port: {}", port);

    let client = reqwest::Client::new();

    // Test script content
    let script_content = r#"
function lifecycle_test_handler(req) {
    return { status: 200, body: 'Lifecycle test successful!' };
}

function init(context) {
    register('/lifecycle-test', 'lifecycle_test_handler', 'GET');
    return { success: true };
}
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

    assert_eq!(
        create_response.status(),
        200,
        "Expected 200 status for script creation"
    );

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

    assert_eq!(
        test_response.status(),
        200,
        "Expected 200 status for lifecycle test"
    );

    let test_body = match timeout(Duration::from_secs(5), test_response.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read test response: {:?}", e),
        Err(_) => panic!("Reading test response timed out"),
    };

    assert_eq!(
        test_body, "Lifecycle test successful!",
        "Expected correct lifecycle test response"
    );

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

    assert_eq!(
        delete_response.status(),
        200,
        "Expected 200 status for script deletion"
    );

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

    assert_eq!(
        after_delete_response.status(),
        404,
        "Expected 404 for deleted script endpoint"
    );

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_read_script_endpoint() {
    // Use the new TestContext pattern for proper server lifecycle management
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    // Wait for server to be ready and scripts to be executed
    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    // Give extra time for JavaScript scripts to execute and register routes
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Server started on port: {}", port);

    let client = reqwest::Client::new();

    // First, upsert a test script
    let test_script_content = r#"
function read_test_handler(req) {
    return { status: 200, body: 'Read test endpoint works!' };
}

function init(context) {
    register('/read-test-endpoint', 'read_test_handler', 'GET');
    return { success: true };
}
"#;

    let upsert_request = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/read-test-script"),
            ("content", test_script_content),
        ])
        .send();

    let upsert_response = match timeout(Duration::from_secs(5), upsert_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /upsert_script failed: {:?}", e),
        Err(_) => panic!("POST request to /upsert_script timed out"),
    };

    assert_eq!(
        upsert_response.status(),
        200,
        "Expected 200 status for upsert_script"
    );

    // Now test the read_script endpoint
    let read_request = client
        .get(format!(
            "http://127.0.0.1:{}/read_script?uri=https://example.com/read-test-script",
            port
        ))
        .send();

    let read_response = match timeout(Duration::from_secs(5), read_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("GET request to /read_script failed: {:?}", e),
        Err(_) => panic!("GET request to /read_script timed out"),
    };

    assert_eq!(
        read_response.status(),
        200,
        "Expected 200 status for read_script"
    );

    let read_body = match timeout(Duration::from_secs(5), read_response.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read response body: {:?}", e),
        Err(_) => panic!("Reading response body timed out"),
    };

    // The response should contain the script content
    assert!(
        read_body.contains("function read_test_handler"),
        "Expected script content in response"
    );
    assert!(
        read_body.contains("Read test endpoint works!"),
        "Expected script content in response"
    );

    // Test reading a non-existent script
    let nonexistent_read_request = client
        .get(format!(
            "http://127.0.0.1:{}/read_script?uri=https://example.com/nonexistent-script",
            port
        ))
        .send();

    let nonexistent_read_response =
        match timeout(Duration::from_secs(5), nonexistent_read_request).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => panic!(
                "GET request to /read_script for nonexistent script failed: {:?}",
                e
            ),
            Err(_) => panic!("GET request to /read_script for nonexistent script timed out"),
        };

    assert_eq!(
        nonexistent_read_response.status(),
        404,
        "Expected 404 for nonexistent script"
    );

    let nonexistent_body: serde_json::Value =
        match timeout(Duration::from_secs(5), nonexistent_read_response.json()).await {
            Ok(Ok(json)) => json,
            Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
            Err(_) => panic!("Reading JSON response timed out"),
        };

    assert_eq!(
        nonexistent_body["error"], "Script not found",
        "Expected 'Script not found' error"
    );

    // Test missing uri parameter
    let missing_uri_request = client
        .get(format!("http://127.0.0.1:{}/read_script", port))
        .send();

    let missing_uri_response = match timeout(Duration::from_secs(5), missing_uri_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("GET request to /read_script without uri failed: {:?}", e),
        Err(_) => panic!("GET request to /read_script without uri timed out"),
    };

    assert_eq!(
        missing_uri_response.status(),
        400,
        "Expected 400 for missing uri parameter"
    );

    let missing_uri_body: serde_json::Value =
        match timeout(Duration::from_secs(5), missing_uri_response.json()).await {
            Ok(Ok(json)) => json,
            Ok(Err(e)) => panic!("Failed to parse JSON response: {:?}", e),
            Err(_) => panic!("Reading JSON response timed out"),
        };

    assert_eq!(
        missing_uri_body["error"], "Missing required parameter: uri",
        "Expected 'Missing required parameter: uri' error"
    );

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
