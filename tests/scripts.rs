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
use common::{TestContext, should_skip_integration_tests};
use std::time::Duration;
use tokio::sync::OnceCell;
use tokio::time::timeout;

static INIT: OnceCell<()> = OnceCell::const_new();

async fn setup_env() {
    INIT.get_or_init(|| async {
        // Initialize DB first
        let config = aiwebengine::config::AppConfig::test_config_postgres(0);
        if let Ok(db) = aiwebengine::database::Database::new(&config.repository).await {
            let db_arc = std::sync::Arc::new(db);
            aiwebengine::database::initialize_global_database(db_arc.clone());

            // Initialize repository with PostgreSQL
            repository::initialize_repository(repository::PostgresRepository::new(
                db_arc.pool().clone(),
                "test".to_string(),
            ));
        }
    })
    .await;
}

// ============================================================================
// QuickJS Integration Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_js_registered_route_returns_expected() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none()) // Don't follow redirects
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to create HTTP client");

    // First verify /health works (confirms the engine is up and the DB is reachable)
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

    // Test the root endpoint - should redirect to /engine/docs
    let res = client
        .get(format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .expect("Request to / failed");

    let status = res.status();

    assert_eq!(
        status,
        reqwest::StatusCode::TEMPORARY_REDIRECT,
        "Expected 307 Temporary Redirect status for /, got {} with body: {}",
        status,
        res.text().await.unwrap_or_default()
    );

    // Check the Location header
    let location = res
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .expect("Location header should be present");

    assert_eq!(
        location, "/engine/installed",
        "Expected redirect to /engine/docs, got: {}",
        location
    );

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// JavaScript Logging Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn js_write_log() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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
        msgs.iter().any(|m| m.message == "js-log-test-called"),
        "Expected log entry 'js-log-test-called' not found in logs: {:?}",
        msgs
    );

    // Proper cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Script Management API Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_upsert_script_endpoint() {
    if should_skip_integration_tests() {
        return;
    }
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
    routeRegistry.registerRoute('/test-endpoint', 'test_endpoint_handler', 'GET');
    return { success: true };
}
"#;

    let upsert_request = client
        .post(format!("http://127.0.0.1:{}/engine/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/test-endpoint-script"),
            ("content", test_script_content),
        ])
        .send();

    let response = match timeout(Duration::from_secs(5), upsert_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /engine/upsert_script failed: {:?}", e),
        Err(_) => panic!("POST request to /engine/upsert_script timed out"),
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

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_script_endpoint() {
    if should_skip_integration_tests() {
        return;
    }
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
    routeRegistry.registerRoute('/delete-test-endpoint', 'delete_test_handler', 'GET');
    return { success: true };
}
"#;

    let upsert_request = client
        .post(format!("http://127.0.0.1:{}/engine/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/delete-test-script"),
            ("content", test_script_content),
        ])
        .send();

    let upsert_response = match timeout(Duration::from_secs(5), upsert_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /engine/upsert_script failed: {:?}", e),
        Err(_) => panic!("POST request to /engine/upsert_script timed out"),
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
        .post(format!("http://127.0.0.1:{}/engine/delete_script", port))
        .form(&[("uri", "https://example.com/delete-test-script")])
        .send();

    let delete_response = match timeout(Duration::from_secs(5), delete_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /engine/delete_script failed: {:?}", e),
        Err(_) => panic!("POST request to /engine/delete_script timed out"),
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
        .post(format!("http://127.0.0.1:{}/engine/delete_script", port))
        .form(&[("uri", "https://example.com/nonexistent-script")])
        .send();

    let nonexistent_delete_response =
        match timeout(Duration::from_secs(5), nonexistent_delete_request).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => panic!(
                "POST request to /engine/delete_script for nonexistent script failed: {:?}",
                e
            ),
            Err(_) => {
                panic!("POST request to /engine/delete_script for nonexistent script timed out")
            }
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

#[tokio::test(flavor = "multi_thread")]
async fn test_script_lifecycle_via_http_api() {
    if should_skip_integration_tests() {
        return;
    }
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
    routeRegistry.registerRoute('/lifecycle-test', 'lifecycle_test_handler', 'GET');
    return { success: true };
}
"#;

    // 1. Create script via HTTP API
    let create_request = client
        .post(format!("http://127.0.0.1:{}/engine/upsert_script", port))
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
        .post(format!("http://127.0.0.1:{}/engine/delete_script", port))
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

#[tokio::test(flavor = "multi_thread")]
async fn test_read_script_endpoint() {
    if should_skip_integration_tests() {
        return;
    }
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
    routeRegistry.registerRoute('/read-test-endpoint', 'read_test_handler', 'GET');
    return { success: true };
}
"#;

    let upsert_request = client
        .post(format!("http://127.0.0.1:{}/engine/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/read-test-script"),
            ("content", test_script_content),
        ])
        .send();

    let upsert_response = match timeout(Duration::from_secs(5), upsert_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("POST request to /engine/upsert_script failed: {:?}", e),
        Err(_) => panic!("POST request to /engine/upsert_script timed out"),
    };

    assert_eq!(
        upsert_response.status(),
        200,
        "Expected 200 status for upsert_script"
    );

    // Now test the read_script endpoint
    let read_request = client
        .get(format!(
            "http://127.0.0.1:{}/engine/read_script?uri=https://example.com/read-test-script",
            port
        ))
        .send();

    let read_response = match timeout(Duration::from_secs(5), read_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("GET request to /engine/read_script failed: {:?}", e),
        Err(_) => panic!("GET request to /engine/read_script timed out"),
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
            "http://127.0.0.1:{}/engine/read_script?uri=https://example.com/nonexistent-script",
            port
        ))
        .send();

    let nonexistent_read_response =
        match timeout(Duration::from_secs(5), nonexistent_read_request).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => panic!(
                "GET request to /engine/read_script for nonexistent script failed: {:?}",
                e
            ),
            Err(_) => panic!("GET request to /engine/read_script for nonexistent script timed out"),
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
        .get(format!("http://127.0.0.1:{}/engine/read_script", port))
        .send();

    let missing_uri_response = match timeout(Duration::from_secs(5), missing_uri_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!(
            "GET request to /engine/read_script without uri failed: {:?}",
            e
        ),
        Err(_) => panic!("GET request to /engine/read_script without uri timed out"),
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

#[tokio::test(flavor = "multi_thread")]
async fn test_init_function_called_successfully() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_script_initializer_updates_metadata() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_script_without_init_function() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_init_function_with_error() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_script_initializer_single_script() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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

/// A redeploy must not take the script's routes down. Registrations live only
/// in the in-memory metadata, so upserting new source — and a re-init that then
/// fails or times out against it — used to leave the script with an empty route
/// table, 404ing every one of its routes until some later init() succeeded.
#[tokio::test(flavor = "multi_thread")]
async fn test_redeploy_keeps_routes_when_reinit_fails() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let script_uri = "test://redeploy-keeps-routes";
    let route_key = ("/redeploy-probe".to_string(), "GET".to_string());

    let working_version = r#"
        function init(context) {
            routeRegistry.registerRoute('/redeploy-probe', 'probe_handler', 'GET');
        }
        function probe_handler(request) { return { status: 200, body: "v1" }; }
    "#;
    upsert_script(script_uri, working_version).expect("Should upsert script");

    let initializer = ScriptInitializer::new(5000);
    let result = initializer
        .initialize_script(script_uri, false)
        .await
        .expect("Should initialize");
    assert!(result.success, "First init should succeed: {:?}", result);

    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert!(
        metadata.registrations.contains_key(&route_key),
        "First init should have registered the route"
    );

    // Redeploy a version whose init() fails after registering.
    let broken_version = r#"
        function init(context) {
            routeRegistry.registerRoute('/redeploy-probe', 'probe_handler', 'GET');
            throw new Error("init failed after registering");
        }
        function probe_handler(request) { return { status: 200, body: "v2" }; }
    "#;
    upsert_script(script_uri, broken_version).expect("Should upsert new version");

    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert_eq!(
        metadata.content, broken_version,
        "Upsert should serve the new source"
    );
    assert!(
        metadata.initialized && metadata.registrations.contains_key(&route_key),
        "Routes must stay live between the upsert and the re-init"
    );

    let result = initializer
        .initialize_script(script_uri, false)
        .await
        .expect("Should return InitResult");
    assert!(!result.success, "Re-init should fail");

    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert!(
        metadata.registrations.contains_key(&route_key),
        "A failed re-init must not drop the previously registered routes"
    );
    assert!(
        metadata.initialized,
        "Routing skips scripts that are not marked initialized, so the flag must \
         stay set while a usable route table is installed"
    );
    assert!(
        metadata.init_error.is_some(),
        "The failure should still be recorded"
    );
}

/// A first init() that registers its routes before doing slow setup should come
/// up routable even if that setup then fails — the routes it managed to register
/// are reported with the failure instead of being discarded.
#[tokio::test(flavor = "multi_thread")]
async fn test_failed_first_init_keeps_routes_it_registered() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let script_uri = "test://partial-init-registrations";
    let route_key = ("/partial-probe".to_string(), "GET".to_string());

    let script_content = r#"
        function init(context) {
            routeRegistry.registerRoute('/partial-probe', 'probe_handler', 'GET');
            throw new Error("setup failed after registering");
        }
        function probe_handler(request) { return { status: 200, body: "ok" }; }
    "#;
    upsert_script(script_uri, script_content).expect("Should upsert script");

    let initializer = ScriptInitializer::new(5000);
    let result = initializer
        .initialize_script(script_uri, false)
        .await
        .expect("Should return InitResult");
    assert!(!result.success, "Init should be reported as failed");

    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert!(
        metadata.registrations.contains_key(&route_key),
        "Routes registered before the failure should be installed"
    );
    assert!(
        metadata.initialized,
        "Installed routes must be reachable, which routing gates on this flag"
    );
    assert!(
        metadata.init_error.is_some(),
        "The failure should still be recorded"
    );
}

/// Startup initializes scripts concurrently, so one blocked `init()` does not
/// delay the ones behind it.
///
/// Guards a regression that is silent: swapping the concurrency for a loop —
/// or for an *ordered* combinator like `buffered`, where a pending head blocks
/// the queue from pulling further work — still initializes every script and
/// still passes every other test. Only the clock shows it.
#[tokio::test(flavor = "multi_thread")]
async fn slow_inits_run_alongside_each_other_rather_than_in_sequence() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    // Spin rather than sleep: the budget these burn is the interrupt handler's,
    // which is what a genuinely stuck init() consumes.
    const SPINNERS: usize = 6;
    const SPIN_MS: u64 = 2_000;
    for n in 0..SPINNERS {
        upsert_script(
            &format!("test://init-concurrency/{}", n),
            &format!(
                r#"function init() {{
                       const until = Date.now() + {};
                       while (Date.now() < until) {{}}
                   }}"#,
                SPIN_MS
            ),
        )
        .expect("Should upsert spinner");
    }

    let initializer = ScriptInitializer::new(SPIN_MS + 1_000);
    let started = std::time::Instant::now();
    let results = initializer
        .initialize_all_scripts()
        .await
        .expect("Should initialize all");
    let elapsed = started.elapsed();

    // Spinners left in the shared database would burn their budget again on
    // every later server startup, which is the very cost this test exists to
    // keep out of startup. Removed before the assertions, so a failure below
    // does not leave them behind.
    for n in 0..SPINNERS {
        repository::delete_script(&format!("test://init-concurrency/{}", n));
    }

    for n in 0..SPINNERS {
        let uri = format!("test://init-concurrency/{}", n);
        assert!(
            results.iter().any(|r| r.script_uri == uri),
            "every spinner should be reported, missing {}",
            uri
        );
    }

    // Run in sequence the spinners alone cost SPINNERS * SPIN_MS (12s). The
    // ceiling leaves room for the other scripts in the shared database and for
    // a loaded machine, while staying well under that.
    let sequential_floor = std::time::Duration::from_millis(SPIN_MS * SPINNERS as u64);
    let ceiling = std::time::Duration::from_secs(8);
    assert!(
        elapsed < ceiling,
        "{} spinners of {}ms took {:?}; in sequence they alone would cost {:?}, \
         so initialization is not running them concurrently",
        SPINNERS,
        SPIN_MS,
        elapsed,
        sequential_floor
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_initializer_all_scripts() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn test_init_context_properties() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
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
