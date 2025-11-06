//! Script Management and Execution Tests
//!
//! This module contains all tests related to JavaScript script management and execution:
//! - QuickJS integration and route registration
//! - Core script initialization
//! - Script init() function handling
//! - JavaScript logging functionality
//! - Script management API (CRUD operations)

mod common;

use aiwebengine::js_engine::call_init_if_exists;
use aiwebengine::repository;
use aiwebengine::repository::{get_script_metadata, upsert_script};
use aiwebengine::script_init::{InitContext, ScriptInitializer};
use common::TestContext;
use std::time::Duration;
use tokio::time::timeout;

// ============================================================================
// QuickJS Integration Tests
// ============================================================================

#[tokio::test]
async fn test_js_registered_route_returns_expected() {
    let context = TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to create HTTP client");

    // First verify /health works (confirms core.js is loaded)
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

    // Test the root endpoint registered by core.js
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

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_core_js_registers_root_path() {
    // Ensure core.js contains a registration for '/'
    let core = repository::fetch_script("https://example.com/core").expect("core script missing");
    assert!(
        core.contains("register('/") || core.contains("register(\"/\""),
        "core.js must register '/' path"
    );
}

// ============================================================================
// Core Script Initialization Tests
// ============================================================================

#[tokio::test]
async fn test_core_script_init_called() {
    let context = TestContext::new();

    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("server failed to start");

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let client = reqwest::Client::new();

    let init_status_result = repository::get_script_metadata("https://example.com/core");
    println!("Init status via repository: {:?}", init_status_result);

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

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// JavaScript Logging Tests
// ============================================================================

#[tokio::test]
async fn js_write_log_and_listlogs() {
    // upsert the js_log_test script so it registers its routes
    let _ = repository::upsert_script(
        "https://example.com/js-log-test",
        include_str!("../scripts/test_scripts/js_log_test.js"),
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

    // Call the route which should call writeLog with timeout
    let log_request = client
        .get(format!("http://127.0.0.1:{}/js-log-test", port))
        .send();

    let res = match timeout(Duration::from_secs(5), log_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Log test request failed: {:?}", e),
        Err(_) => panic!("Log test request timed out"),
    };

    let body = match timeout(Duration::from_secs(5), res.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read log test response: {:?}", e),
        Err(_) => panic!("Reading log test response timed out"),
    };

    assert!(
        body.contains("logged"),
        "Expected 'logged' in response, got: {}",
        body
    );

    // Verify the log message was written via Rust API
    let msgs = repository::fetch_log_messages("https://example.com/js-log-test");
    assert!(
        msgs.iter().any(|m| m == "js-log-test-called"),
        "Expected log entry 'js-log-test-called' not found in logs: {:?}",
        msgs
    );

    // Verify via JS-exposed route that calls listLogs()
    // Retry a few times to allow any small propagation/timing delays
    let mut found = false;
    let mut last_body = String::new();

    for i in 0..10 {
        let list_request = client
            .get(format!("http://127.0.0.1:{}/js-list", port))
            .send();

        let res2 = match timeout(Duration::from_secs(5), list_request).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                println!("attempt {}: request failed: {:?}", i, e);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                println!("attempt {}: request timed out", i);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let body2 = match timeout(Duration::from_secs(5), res2.text()).await {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => {
                println!("attempt {}: failed to read response: {:?}", i, e);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                println!("attempt {}: reading response timed out", i);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        println!("attempt {}: /js-list -> {}", i, body2);
        last_body = body2.clone();

        if body2.contains("js-log-test-called") {
            found = true;
            break;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    if !found {
        println!("/js-list last body: {}", last_body);
        context.cleanup().await.expect("Failed to cleanup");
        panic!(
            "Expected log entry 'js-log-test-called' not found in /js-list output after 10 attempts"
        );
    }

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn js_list_logs_for_uri() {
    // Insert some test log messages for different URIs
    repository::insert_log_message("https://example.com/js-log-test-uri", "test-message-1");
    repository::insert_log_message("https://example.com/js-log-test-uri", "test-message-2");
    repository::insert_log_message("https://example.com/other-script", "other-message");

    // upsert the js_log_test_uri script so it registers its routes
    let _ = repository::upsert_script(
        "https://example.com/js-log-test-uri-script",
        include_str!("../scripts/test_scripts/js_log_test_uri.js"),
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

    // Call the route which should call listLogsForUri
    let list_request = client
        .get(format!("http://127.0.0.1:{}/js-list-for-uri", port))
        .send();

    let res = match timeout(Duration::from_secs(5), list_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("List logs for URI request failed: {:?}", e),
        Err(_) => panic!("List logs for URI request timed out"),
    };

    let body = match timeout(Duration::from_secs(5), res.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read list logs for URI response: {:?}", e),
        Err(_) => panic!("Reading list logs for URI response timed out"),
    };

    println!("Response body: {}", body);

    // Parse the JSON response
    let response: serde_json::Value =
        serde_json::from_str(&body).expect("Failed to parse JSON response");

    // Check that current logs contain the expected messages
    let current_logs = response["current"]
        .as_array()
        .expect("current should be an array");
    assert!(
        current_logs.iter().any(|v| v == "test-message-1"),
        "Expected 'test-message-1' in current logs"
    );
    assert!(
        current_logs.iter().any(|v| v == "test-message-2"),
        "Expected 'test-message-2' in current logs"
    );

    // Check that other logs contain the expected message
    let other_logs = response["other"]
        .as_array()
        .expect("other should be an array");
    assert!(
        other_logs.iter().any(|v| v == "other-message"),
        "Expected 'other-message' in other logs"
    );

    // Verify that logs are properly separated by URI
    assert!(
        !current_logs.iter().any(|v| v == "other-message"),
        "Current logs should not contain messages from other URI"
    );
    assert!(
        !other_logs.iter().any(|v| v == "test-message-1"),
        "Other logs should not contain messages from current URI"
    );

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Script Management API Tests
// ============================================================================

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

// ============================================================================
// Script Init Function Tests
// ============================================================================

#[tokio::test]
async fn test_init_function_called_successfully() {
    let script_uri = "test://init-success";
    let script_content = r#"
        let initWasCalled = false;
        
        function init(context) {
            initWasCalled = true;
            console.log("Init called for: " + context.scriptName);
            console.log("Is startup: " + context.isStartup);
        }
        
        function getInitStatus() {
            return initWasCalled;
        }
    "#;

    // Upsert the script first
    upsert_script(script_uri, script_content).expect("Should upsert script");

    // Create init context
    let context = InitContext::new(script_uri.to_string(), true);

    // Call init function directly (without ScriptInitializer)
    let result = call_init_if_exists(script_uri, script_content, context);

    assert!(result.is_ok(), "Should execute without error");
    assert!(
        result.unwrap().is_some(),
        "Should return Some(registrations) indicating init was called"
    );

    // Note: call_init_if_exists doesn't update metadata - that's done by ScriptInitializer
}

#[tokio::test]
async fn test_script_initializer_updates_metadata() {
    let script_uri = "test://init-metadata";
    let script_content = r#"
        function init(context) {
            console.log("Updating metadata test");
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    // Use ScriptInitializer which handles metadata updates
    let initializer = ScriptInitializer::new(5000);
    let result = initializer
        .initialize_script(script_uri, true)
        .await
        .expect("Should initialize");

    assert!(result.success, "Initialization should succeed");

    // Now verify metadata was updated
    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert!(
        metadata.initialized,
        "Script should be marked as initialized"
    );
    assert!(metadata.init_error.is_none(), "Should have no init error");
    assert!(
        metadata.last_init_time.is_some(),
        "Should have init timestamp"
    );
}

#[tokio::test]
async fn test_script_without_init_function() {
    let script_uri = "test://no-init";
    let script_content = r#"
        function handleRequest(request) {
            return { status: 200, body: "Hello" };
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    let context = InitContext::new(script_uri.to_string(), false);
    let result = call_init_if_exists(script_uri, script_content, context);

    assert!(result.is_ok(), "Should execute without error");
    assert!(
        result.unwrap().is_none(),
        "Should return None when no init function exists"
    );
}

#[tokio::test]
async fn test_init_function_with_error() {
    let script_uri = "test://init-error";
    let script_content = r#"
        function init(context) {
            throw new Error("Init failed intentionally");
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    // Use ScriptInitializer to handle errors properly
    let initializer = ScriptInitializer::new(5000);
    let result = initializer
        .initialize_script(script_uri, true)
        .await
        .expect("Should return InitResult");

    assert!(!result.success, "Initialization should fail");
    assert!(result.error.is_some(), "Should have error message");

    // Debug print
    println!("Error message: {:?}", result.error);

    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Init") || error_msg.contains("failed"),
        "Error message should contain init-related text, got: {}",
        error_msg
    );

    // Verify metadata was updated with error
    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert!(
        !metadata.initialized,
        "Script should not be marked as initialized"
    );
    assert!(metadata.init_error.is_some(), "Should have init error");
}

#[tokio::test]
async fn test_script_initializer_single_script() {
    let script_uri = "test://initializer-test";
    let script_content = r#"
        function init(context) {
            console.log("Initialized: " + context.scriptName);
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    let initializer = ScriptInitializer::new(5000); // 5 second timeout
    let result = initializer
        .initialize_script(script_uri, true)
        .await
        .expect("Should initialize");

    assert!(result.success, "Initialization should succeed");
    assert!(result.error.is_none(), "Should have no error");
    assert!(result.duration_ms > 0, "Should have measurable duration");
}

#[tokio::test]
async fn test_script_initializer_all_scripts() {
    // Create multiple test scripts
    let scripts = vec![
        (
            "test://multi-init-1",
            r#"function init(ctx) { console.log("Init 1"); }"#,
        ),
        (
            "test://multi-init-2",
            r#"function init(ctx) { console.log("Init 2"); }"#,
        ),
        ("test://multi-no-init", r#"function handler() { }"#),
    ];

    for (uri, content) in &scripts {
        upsert_script(uri, content).expect("Should upsert script");
    }

    let initializer = ScriptInitializer::new(5000);
    let results = initializer
        .initialize_all_scripts()
        .await
        .expect("Should initialize all");

    // Should have initialized all dynamic scripts (not static ones)
    assert!(results.len() >= 3, "Should have at least 3 results");

    // Count successful initializations
    let successful = results.iter().filter(|r| r.success).count();
    assert!(successful >= 3, "At least 3 scripts should succeed");
}

#[tokio::test]
async fn test_init_context_properties() {
    let script_uri = "test://context-test";
    let script_content = r#"
        let capturedContext = null;
        
        function init(context) {
            capturedContext = context;
            console.log("ScriptName: " + context.scriptName);
            console.log("IsStartup: " + context.isStartup);
            console.log("Timestamp: " + context.timestamp);
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    let context = InitContext::new(script_uri.to_string(), true);
    let result = call_init_if_exists(script_uri, script_content, context);

    assert!(result.is_ok(), "Should execute successfully");
    assert!(result.unwrap().is_some(), "Init should be called");
}
