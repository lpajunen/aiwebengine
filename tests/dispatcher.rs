//! Message Dispatcher Integration Tests
//!
//! Tests for the message dispatcher system that enables inter-script communication

mod common;

use aiwebengine::repository;
use common::TestContext;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_basic_functionality() {
    let context = TestContext::new();

    // Load the dispatcher test script before starting server
    let _ = repository::upsert_script(
        "https://example.com/dispatcher_test",
        include_str!("../scripts/test_scripts/dispatcher_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");

    // Test register listener
    let res = client
        .get(format!(
            "http://127.0.0.1:{}/test-dispatcher/register-listener",
            port
        ))
        .send()
        .await
        .expect("Failed to register listener");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Failed to parse JSON");
    assert!(body["success"].as_bool().unwrap_or(false));

    let results = body["results"].as_array().expect("results should be array");
    assert!(
        results
            .iter()
            .any(|r| r["test"].as_str() == Some("registerListener - valid"))
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_send_message() {
    let context = TestContext::new();

    // Load the dispatcher test script before starting server
    let _ = repository::upsert_script(
        "https://example.com/dispatcher_test",
        include_str!("../scripts/test_scripts/dispatcher_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");

    // Test send message
    let res = client
        .get(format!(
            "http://127.0.0.1:{}/test-dispatcher/send-message",
            port
        ))
        .send()
        .await
        .expect("Failed to send message");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Failed to parse JSON");
    assert!(body["success"].as_bool().unwrap_or(false));

    let results = body["results"].as_array().expect("results should be array");

    // Should have test for sending with data
    let send_with_data = results
        .iter()
        .find(|r| r["test"].as_str() == Some("sendMessage - with data"));
    assert!(send_with_data.is_some());

    // Should have test for no listeners
    let no_listeners = results
        .iter()
        .find(|r| r["test"].as_str() == Some("sendMessage - no listeners"));
    assert!(no_listeners.is_some());

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_multiple_handlers() {
    let context = TestContext::new();

    // Load the dispatcher test script before starting server
    let _ = repository::upsert_script(
        "https://example.com/dispatcher_test",
        include_str!("../scripts/test_scripts/dispatcher_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");

    // Test multiple handlers for same message type
    let res = client
        .get(format!(
            "http://127.0.0.1:{}/test-dispatcher/multiple-handlers",
            port
        ))
        .send()
        .await
        .expect("Failed to test multiple handlers");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Failed to parse JSON");
    assert!(body["success"].as_bool().unwrap_or(false));

    let results = body["results"].as_array().expect("results should be array");
    let multiple_test = results
        .iter()
        .find(|r| r["test"].as_str() == Some("multiple handlers"));
    assert!(multiple_test.is_some());

    // Verify both handlers received the message
    if let Some(test) = multiple_test {
        // Note: Due to async execution, we check that at least messages were sent
        // The actual handler execution might be deferred
        assert!(test["success"].as_bool().unwrap_or(false));
    }

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_data_serialization() {
    let context = TestContext::new();

    // Load the dispatcher test script before starting server
    let _ = repository::upsert_script(
        "https://example.com/dispatcher_test",
        include_str!("../scripts/test_scripts/dispatcher_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");

    // Test data serialization
    let res = client
        .get(format!(
            "http://127.0.0.1:{}/test-dispatcher/data-serialization",
            port
        ))
        .send()
        .await
        .expect("Failed to test data serialization");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Failed to parse JSON");
    assert!(body["success"].as_bool().unwrap_or(false));

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_error_handling() {
    let context = TestContext::new();

    // Load the dispatcher test script before starting server
    let _ = repository::upsert_script(
        "https://example.com/dispatcher_test",
        include_str!("../scripts/test_scripts/dispatcher_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");

    // Test error handling
    let res = client
        .get(format!(
            "http://127.0.0.1:{}/test-dispatcher/error-handling",
            port
        ))
        .send()
        .await
        .expect("Failed to test error handling");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Failed to parse JSON");
    assert!(body["success"].as_bool().unwrap_or(false));

    // Dispatcher should handle errors gracefully
    let results = body["results"].as_array().expect("results should be array");
    let error_test = results
        .iter()
        .find(|r| r["test"].as_str() == Some("error handling"));
    assert!(error_test.is_some());

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_run_all_tests() {
    let context = TestContext::new();

    // Load the dispatcher test script before starting server
    let _ = repository::upsert_script(
        "https://example.com/dispatcher_test",
        include_str!("../scripts/test_scripts/dispatcher_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("Failed to create HTTP client");

    // Run all dispatcher tests
    let res = client
        .get(format!("http://127.0.0.1:{}/test-dispatcher/run-all", port))
        .send()
        .await
        .expect("Failed to run all tests");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Failed to parse JSON");

    assert!(
        body["success"].as_bool().unwrap_or(false),
        "All tests should succeed"
    );

    let total_tests = body["totalTests"].as_u64().unwrap_or(0);
    let passed = body["passed"].as_u64().unwrap_or(0);
    let failed = body["failed"].as_u64().unwrap_or(0);

    println!(
        "Dispatcher tests: {} total, {} passed, {} failed",
        total_tests, passed, failed
    );

    assert!(total_tests > 0, "Should have run some tests");
    assert_eq!(passed, total_tests, "All tests should pass");
    assert_eq!(failed, 0, "No tests should fail");

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dispatcher_validation() {
    let context = TestContext::new();

    // Load the dispatcher test script before starting server
    let _ = repository::upsert_script(
        "https://example.com/dispatcher_test",
        include_str!("../scripts/test_scripts/dispatcher_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client");

    // Test validation by running register listener tests
    let res = client
        .get(format!(
            "http://127.0.0.1:{}/test-dispatcher/register-listener",
            port
        ))
        .send()
        .await
        .expect("Failed to test validation");

    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("Failed to parse JSON");
    assert!(body["success"].as_bool().unwrap_or(false));

    let results = body["results"].as_array().expect("results should be array");

    // Should reject empty message type
    let empty_type_test = results
        .iter()
        .find(|r| r["test"].as_str() == Some("registerListener - empty message type"));
    assert!(empty_type_test.is_some());
    if let Some(test) = empty_type_test {
        assert!(
            test["success"].as_bool().unwrap_or(false),
            "Should validate empty message type"
        );
    }

    // Should reject empty handler name
    let empty_handler_test = results
        .iter()
        .find(|r| r["test"].as_str() == Some("registerListener - empty handler"));
    assert!(empty_handler_test.is_some());
    if let Some(test) = empty_handler_test {
        assert!(
            test["success"].as_bool().unwrap_or(false),
            "Should validate empty handler name"
        );
    }

    context.cleanup().await.expect("Failed to cleanup");
}
