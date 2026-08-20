//! HTTP API Integration Tests
//!
//! This module contains all HTTP/REST API endpoint tests including:
//! - Health endpoint tests
//! - HTTP method handling (GET, POST, PUT, DELETE)
//! - Query parameter parsing
//! - Form data handling
//! - GraphQL endpoint tests

mod common;

use aiwebengine::repository;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

// ============================================================================
// Health Endpoint Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_health_endpoint() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // Start server with proper shutdown support
    let port = context
        .start_server()
        .await
        .expect("server failed to start");

    // Wait for server to be ready
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test health endpoint
    let health_response = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .expect("Health check request failed");

    assert_eq!(health_response.status(), 200);

    let health_body = health_response
        .text()
        .await
        .expect("Failed to read health response");

    // Parse the JSON response
    let health_json: serde_json::Value =
        serde_json::from_str(&health_body).expect("Failed to parse health response as JSON");

    // Verify the health response structure
    assert_eq!(health_json["status"], "healthy");
    assert!(health_json["timestamp"].is_string());
    // The health endpoint performs a real database check and reports its status.
    assert_eq!(health_json["database"], "ok");

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_health_endpoint_content_type() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // Start server
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test that the health endpoint returns correct content type
    let response = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .expect("Health request failed");

    let content_type = response
        .headers()
        .get("content-type")
        .expect("Content-Type header missing")
        .to_str()
        .expect("Content-Type header not valid string");

    assert_eq!(content_type, "application/json");

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_logs_endpoint() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test script_logs endpoint with a valid URI parameter
    let logs_response = client
        .get(format!(
            "http://127.0.0.1:{}/engine/script_logs?uri=https://example.com/core",
            port
        ))
        .send()
        .await
        .expect("Script logs request failed");

    assert_eq!(logs_response.status(), 200);

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_logs_all_scripts_and_filters() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let first = "test_script_logs_filters_one";
    let second = "test_script_logs_filters_two";
    let _ = repository::clear_log_messages(first);
    let _ = repository::clear_log_messages(second);
    repository::insert_log_message(first, "first-info", "INFO");
    repository::insert_log_message(first, "first-error", "ERROR");
    repository::insert_log_message(second, "second-info", "INFO");

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    // Omitting uri spans every script, and each entry names the script that
    // logged it.
    let body: serde_json::Value = client
        .get(format!("{}/engine/script_logs", base))
        .send()
        .await
        .expect("all-scripts logs request failed")
        .json()
        .await
        .expect("all-scripts logs response not JSON");
    let logs = body["logs"].as_array().expect("logs is not an array");
    assert!(body["uri"].is_null());
    let uris: Vec<&str> = logs
        .iter()
        .filter_map(|entry| entry["scriptUri"].as_str())
        .collect();
    assert!(uris.contains(&first), "expected logs from {}", first);
    assert!(uris.contains(&second), "expected logs from {}", second);

    // level filters, case-insensitively
    let body: serde_json::Value = client
        .get(format!(
            "{}/engine/script_logs?uri={}&level=error",
            base, first
        ))
        .send()
        .await
        .expect("level-filtered logs request failed")
        .json()
        .await
        .expect("level-filtered logs response not JSON");
    let logs = body["logs"].as_array().expect("logs is not an array");
    assert_eq!(logs.len(), 1, "expected only the ERROR entry");
    assert_eq!(logs[0]["message"], "first-error");

    // limit keeps the newest entries
    let body: serde_json::Value = client
        .get(format!("{}/engine/script_logs?uri={}&limit=1", base, first))
        .send()
        .await
        .expect("limited logs request failed")
        .json()
        .await
        .expect("limited logs response not JSON");
    let logs = body["logs"].as_array().expect("logs is not an array");
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["message"], "first-error");

    // since excludes everything logged before it
    let future_millis = chrono::Utc::now().timestamp_millis() + 60_000;
    let body: serde_json::Value = client
        .get(format!(
            "{}/engine/script_logs?since={}",
            base, future_millis
        ))
        .send()
        .await
        .expect("since-filtered logs request failed")
        .json()
        .await
        .expect("since-filtered logs response not JSON");
    assert_eq!(body["count"], 0);

    // invalid filters are refused rather than silently ignored
    let response = client
        .get(format!("{}/engine/script_logs?since=not-a-time", base))
        .send()
        .await
        .expect("bad since request failed");
    assert_eq!(response.status(), 400);

    let response = client
        .get(format!("{}/engine/script_logs?limit=0", base))
        .send()
        .await
        .expect("bad limit request failed");
    assert_eq!(response.status(), 400);

    let _ = repository::clear_log_messages(first);
    let _ = repository::clear_log_messages(second);
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_logs_delete_clears_and_prunes() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let cleared = "test_script_logs_delete_cleared";
    let kept = "test_script_logs_delete_kept";
    let _ = repository::clear_log_messages(cleared);
    let _ = repository::clear_log_messages(kept);
    repository::insert_log_message(cleared, "to-be-cleared", "INFO");
    repository::insert_log_message(kept, "to-be-kept", "INFO");

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    // A uri clears that script's logs and leaves every other script alone.
    let response = client
        .delete(format!("{}/engine/script_logs?uri={}", base, cleared))
        .send()
        .await
        .expect("clear logs request failed");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("clear response not JSON");
    assert_eq!(body["cleared"], true);
    assert!(repository::fetch_log_messages(cleared).is_empty());
    assert!(!repository::fetch_log_messages(kept).is_empty());

    // Without a uri it prunes every script back to its newest entries.
    for i in 0..25 {
        repository::insert_log_message(cleared, &format!("prune-{}", i), "INFO");
    }
    let response = client
        .delete(format!("{}/engine/script_logs", base))
        .send()
        .await
        .expect("prune logs request failed");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("prune response not JSON");
    assert_eq!(body["pruned"], true);
    assert!(repository::fetch_log_messages(cleared).len() <= 20);

    let _ = repository::clear_log_messages(cleared);
    let _ = repository::clear_log_messages(kept);
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_routes_endpoint() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    let body: serde_json::Value = client
        .get(format!("{}/engine/routes", base))
        .send()
        .await
        .expect("routes request failed")
        .json()
        .await
        .expect("routes response not JSON");

    let routes = body["routes"].as_array().expect("routes is not an array");
    assert_eq!(body["count"], routes.len());
    assert!(body["host"].is_null());
    // Every entry carries the full introspection shape, so a client needs no
    // transform.
    for route in routes {
        assert!(route["path"].is_string(), "route without a path: {}", route);
        assert!(
            route["method"].is_string(),
            "route without a method: {}",
            route
        );
        assert!(
            route["script_uri"].is_string(),
            "route without a script_uri: {}",
            route
        );
        assert!(route["tags"].is_array(), "route without tags: {}", route);
    }

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Engine Management Endpoint Tests (/engine/*)
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_engine_management_endpoints() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);

    // /engine/script_logs is the canonical alias of /script_logs
    let response = client
        .get(format!(
            "{}/engine/script_logs?uri=https://example.com/core",
            base
        ))
        .send()
        .await
        .expect("engine script_logs request failed");
    assert_eq!(response.status(), 200);

    // /engine/scripts lists script metadata
    let response = client
        .get(format!("{}/engine/scripts", base))
        .send()
        .await
        .expect("engine scripts request failed");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("scripts response not JSON");
    assert!(body["scripts"].is_array());
    assert!(body["count"].is_number());

    // /engine/script_init_status without uri returns all statuses
    let response = client
        .get(format!("{}/engine/script_init_status", base))
        .send()
        .await
        .expect("engine script_init_status request failed");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("status response not JSON");
    assert!(body["statuses"].is_array());

    // /engine/script_owners lists owners for anyone
    let script_uri = "https://example.com/owners-endpoint-test";
    let _ = repository::upsert_script(script_uri, "function init() {}");
    let response = client
        .get(format!("{}/engine/script_owners?uri={}", base, script_uri))
        .send()
        .await
        .expect("engine script_owners request failed");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("owners response not JSON");
    assert!(body["owners"].is_array());

    // Missing required parameters are rejected
    for path in ["/engine/secrets", "/engine/script_owners"] {
        let response = client
            .get(format!("{}{}", base, path))
            .send()
            .await
            .expect("missing-param request failed");
        assert_eq!(response.status(), 400, "expected 400 for {}", path);
    }

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_favicon_default_served_when_unregistered() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // No script registers /favicon.ico, so the engine default is served.
    let response = client
        .get(format!("http://127.0.0.1:{}/favicon.ico", port))
        .send()
        .await
        .expect("favicon request failed");
    assert_eq!(response.status(), 200);

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// HTTP Methods Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_different_http_methods() {
    if should_skip_integration_tests() {
        return;
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn test_head_request_falls_back_to_get_with_empty_body() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let _ = repository::upsert_script(
        "https://example.com/method_test",
        include_str!("../scripts/test_scripts/method_test.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // No HEAD handler is registered for /api/test, only GET - HEAD should
    // transparently run the GET handler and come back with an empty body.
    let head_response = client
        .head(format!("http://127.0.0.1:{}/api/test", port))
        .send()
        .await
        .expect("HEAD request failed");

    assert_eq!(head_response.status(), 200);
    let head_body = head_response
        .text()
        .await
        .expect("Failed to read HEAD response");
    assert!(
        head_body.is_empty(),
        "HEAD response should have an empty body, got: {:?}",
        head_body
    );

    // An unregistered path must still 404 on HEAD, not silently match something else.
    let head_not_found_response = client
        .head(format!("http://127.0.0.1:{}/api/nonexistent", port))
        .send()
        .await
        .expect("HEAD request to nonexistent path failed");
    assert_eq!(head_not_found_response.status(), 404);

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_head_request_on_asset_route_strips_body() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let script_uri = "https://example.com/head_asset_test";
    let _ = repository::upsert_script(script_uri, "function init() {}");
    repository::upsert_asset(repository::Asset {
        uri: "head-test.css".to_string(),
        mimetype: "text/css".to_string(),
        content: b"body { color: red; }".to_vec(),
        name: Some("head-test.css".to_string()),
        script_uri: script_uri.to_string(),
        created_at: std::time::SystemTime::now(),
        updated_at: std::time::SystemTime::now(),
    })
    .expect("Failed to create asset");

    let script = r#"
        function init(context) {
          routeRegistry.registerAssetRoute("/head-test.css", "head-test.css");
          return { success: true };
        }
    "#;
    let _ = repository::upsert_script(script_uri, script);

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    let get_response = client
        .get(format!("http://127.0.0.1:{}/head-test.css", port))
        .send()
        .await
        .expect("GET request failed");
    assert_eq!(get_response.status(), 200);
    let get_body = get_response.text().await.expect("Failed to read GET body");
    assert_eq!(get_body, "body { color: red; }");

    let head_response = client
        .head(format!("http://127.0.0.1:{}/head-test.css", port))
        .send()
        .await
        .expect("HEAD request failed");
    assert_eq!(head_response.status(), 200);
    assert_eq!(
        head_response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/css; charset=utf-8")
    );
    let head_body = head_response
        .text()
        .await
        .expect("Failed to read HEAD body");
    assert!(
        head_body.is_empty(),
        "HEAD response for asset route should have an empty body, got: {:?}",
        head_body
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_explicit_head_handler_overrides_get_fallback() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let script = r#"
        function get_handler(context) {
          return { status: 200, body: "GET body" };
        }
        function head_handler(context) {
          return { status: 200, body: "", headers: { "x-head-handler": "custom" } };
        }
        function init(context) {
          routeRegistry.registerRoute("/api/explicit-head", "get_handler", "GET");
          routeRegistry.registerRoute("/api/explicit-head", "head_handler", "HEAD");
          return { success: true };
        }
    "#;
    let _ = repository::upsert_script("https://example.com/explicit_head_test", script);

    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    let head_response = client
        .head(format!("http://127.0.0.1:{}/api/explicit-head", port))
        .send()
        .await
        .expect("HEAD request failed");

    assert_eq!(head_response.status(), 200);
    assert_eq!(
        head_response
            .headers()
            .get("x-head-handler")
            .and_then(|v| v.to_str().ok()),
        Some("custom"),
        "Explicitly registered HEAD handler should run instead of falling back to GET"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Query Parameters Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_query_parameters() {
    if should_skip_integration_tests() {
        return;
    }
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

    // Test GET request without query parameters
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

    // Test GET request with query parameters
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
    assert!(
        body_no_query.contains("/api/query") && body_with_query.contains("/api/query"),
        "Both requests should be handled by the same route"
    );

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Form Data Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_form_data() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // Dynamically load the form test script
    let _ = repository::upsert_script(
        "https://example.com/form_test",
        include_str!("../scripts/test_scripts/form_test.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test simple GET request to root
    let root_response = client
        .get(format!("http://127.0.0.1:{}/", port))
        .send()
        .await;

    match root_response {
        Ok(resp) => {
            println!("Root request succeeded with status: {}", resp.status());
            let body = resp.text().await.unwrap_or_default();
            println!("Root response body: {}", body);
        }
        Err(e) => {
            println!("Root request failed: {}", e);
        }
    }

    // Test POST request without form data
    let response_no_form = client
        .post(format!("http://127.0.0.1:{}/api/form", port))
        .send()
        .await
        .expect("POST request without form data failed");

    println!(
        "POST REQUEST MADE TO /api/form, STATUS: {}",
        response_no_form.status()
    );
    let body_no_form = response_no_form
        .text()
        .await
        .expect("Failed to read response without form data");
    println!("RESPONSE BODY: {}", body_no_form);
    assert!(
        body_no_form.contains("Path: /api/form"),
        "Response should contain correct path: {}",
        body_no_form
    );
    assert!(
        body_no_form.contains("Method: POST"),
        "Response should contain correct method: {}",
        body_no_form
    );
    assert!(
        body_no_form.contains("Form: none"),
        "Response should indicate no form data: {}",
        body_no_form
    );

    // Test POST request with form data
    let response_with_form = client
        .post(format!("http://127.0.0.1:{}/api/form", port))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("id=456&name=form_test&email=test@example.com")
        .send()
        .await
        .expect("POST request with form data failed");

    assert_eq!(response_with_form.status(), 200);
    let body_with_form = response_with_form
        .text()
        .await
        .expect("Failed to read response with form data");
    assert!(
        body_with_form.contains("Path: /api/form"),
        "Response should contain correct path: {}",
        body_with_form
    );
    assert!(
        body_with_form.contains("Method: POST"),
        "Response should contain correct method: {}",
        body_with_form
    );
    assert!(
        body_with_form.contains("Form:")
            && body_with_form.contains("id=456")
            && body_with_form.contains("name=form_test")
            && body_with_form.contains("email=test@example.com"),
        "Response should contain parsed form data: {}",
        body_with_form
    );

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// GraphQL Endpoint Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_graphql_endpoints() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // Load the GraphQL test script (a script that registers its own
    // queries, mutations, and subscriptions via graphQLRegistry)
    let _ = repository::upsert_script(
        "https://example.com/graphql_test",
        include_str!("../scripts/test_scripts/graphql_test.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test GraphQL POST endpoint with introspection query
    let introspection_query = r#"{__schema{queryType{name fields{name type{name kind}}}}}"#;

    let graphql_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(format!(r#"{{"query": "{}"}}"#, introspection_query))
        .send()
        .await
        .expect("GraphQL introspection request failed");

    assert_eq!(graphql_response.status(), 200);

    let graphql_body = graphql_response
        .text()
        .await
        .expect("Failed to read GraphQL response");

    let graphql_json: serde_json::Value =
        serde_json::from_str(&graphql_body).expect("Failed to parse GraphQL response as JSON");

    // Check if there are errors
    if let Some(errors) = graphql_json.get("errors") {
        panic!(
            "GraphQL introspection query failed with errors: {:?}",
            errors
        );
    }

    // Verify the schema contains our registered operations
    let schema = &graphql_json["data"]["__schema"];

    // Check Query type has our registered query
    let query_fields = &schema["queryType"]["fields"];
    assert!(
        query_fields
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["name"] == "hello")
    );

    // Test executing a registered query
    let query_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(r#"{"query": "{ hello }"}"#)
        .send()
        .await
        .expect("GraphQL query request failed");

    assert_eq!(query_response.status(), 200);

    let query_body = query_response
        .text()
        .await
        .expect("Failed to read query response");

    let query_json: serde_json::Value =
        serde_json::from_str(&query_body).expect("Failed to parse query response as JSON");

    // Should contain data
    assert!(query_json["data"].is_object());
    assert!(query_json["data"]["hello"].is_string());
    assert_eq!(
        query_json["data"]["hello"].as_str().unwrap(),
        "Hello from JavaScript!"
    );

    // Engine management is REST/MCP only — the built-in scripts must not
    // register any GraphQL operations. Check the registry by script URI
    // (the shared test database may contain unrelated leftover scripts,
    // so schema introspection is not a reliable negative check here).
    {
        let registry = aiwebengine::graphql::GRAPHQL_REGISTRY.read().unwrap();
        let builtin_uris = [
            "https://example.com/core",
            "https://example.com/cli",
            "https://example.com/auth",
        ];
        let builtin_ops: Vec<&String> = registry
            .queries
            .iter()
            .chain(registry.mutations.iter())
            .chain(registry.subscriptions.iter())
            .filter(|(_, op)| builtin_uris.contains(&op.script_uri.as_str()))
            .map(|(name, _)| name)
            .collect();
        assert!(
            builtin_ops.is_empty(),
            "Built-in engine scripts must not register GraphQL operations, found: {:?}",
            builtin_ops
        );
    }

    // Test GraphQL SSE endpoint (basic connectivity test)
    let sse_response = client
        .get(format!(
            "http://127.0.0.1:{}/graphql/sse?query=subscription {{ userUpdates }}",
            port
        ))
        .send()
        .await
        .expect("GraphQL SSE request failed");

    assert_eq!(sse_response.status(), 200);

    // Check that SSE headers are present
    let content_type = sse_response.headers().get("content-type").unwrap();
    assert_eq!(content_type, "text/event-stream");

    let cache_control = sse_response.headers().get("cache-control").unwrap();
    assert_eq!(cache_control, "no-cache");

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_graphql_script_defined_mutations() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // Load the GraphQL test script, which registers its own mutations
    // via graphQLRegistry (script-provided GraphQL is supported; engine
    // management via GraphQL is not)
    let _ = repository::upsert_script(
        "https://example.com/graphql_test",
        include_str!("../scripts/test_scripts/graphql_test.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Execute the script-defined createUser mutation
    let create_user_body = serde_json::json!({
        "query": "mutation { createUser(name: \"Alice\") }"
    });

    let create_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(create_user_body.to_string())
        .send()
        .await
        .expect("GraphQL createUser mutation request failed");

    assert_eq!(create_response.status(), 200);

    let create_body = create_response
        .text()
        .await
        .expect("Failed to read createUser mutation response");

    let create_json: serde_json::Value =
        serde_json::from_str(&create_body).expect("Failed to parse createUser mutation response");

    if let Some(errors) = create_json.get("errors") {
        panic!(
            "GraphQL createUser mutation failed with errors: {:?}",
            errors
        );
    }

    assert_eq!(
        create_json["data"]["createUser"].as_str().unwrap(),
        "Created user: Alice"
    );

    // Engine script management mutations must NOT be exposed via GraphQL —
    // script management is REST/MCP only. Check the registry by script URI
    // (the shared test database may contain unrelated leftover scripts, so
    // executing the mutation is not a reliable negative check here).
    {
        let registry = aiwebengine::graphql::GRAPHQL_REGISTRY.read().unwrap();
        let core_mutations: Vec<&String> = registry
            .mutations
            .iter()
            .filter(|(_, op)| op.script_uri == "https://example.com/core")
            .map(|(name, _)| name)
            .collect();
        assert!(
            core_mutations.is_empty(),
            "core.js must not register GraphQL mutations, found: {:?}",
            core_mutations
        );
    }

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_graphql_registration_clearing() {
    if should_skip_integration_tests() {
        return;
    }
    use aiwebengine::graphql::{
        GRAPHQL_REGISTRY, GraphQLOperation, clear_script_graphql_registrations,
    };

    // Test that clearing GraphQL registrations works
    let script_uri = "http://test/clear_test";

    // First, simulate adding some registrations to the registry
    {
        let mut registry = GRAPHQL_REGISTRY.write().unwrap();
        registry.queries.insert(
            script_uri.to_string(),
            GraphQLOperation {
                sdl: "type Query { testQuery: String }".to_string(),
                resolver_function: "testResolver".to_string(),
                script_uri: script_uri.to_string(),
                visibility: aiwebengine::graphql::OperationVisibility::External,
            },
        );
        registry.mutations.insert(
            script_uri.to_string(),
            GraphQLOperation {
                sdl: "type Mutation { testMutation: String }".to_string(),
                resolver_function: "testMutationResolver".to_string(),
                script_uri: script_uri.to_string(),
                visibility: aiwebengine::graphql::OperationVisibility::External,
            },
        );
        registry.subscriptions.insert(
            script_uri.to_string(),
            GraphQLOperation {
                sdl: "type Subscription { testSubscription: String }".to_string(),
                resolver_function: "testSubscriptionResolver".to_string(),
                script_uri: script_uri.to_string(),
                visibility: aiwebengine::graphql::OperationVisibility::External,
            },
        );
    }

    // Verify they were added
    {
        let registry = GRAPHQL_REGISTRY.read().unwrap();
        assert!(registry.queries.contains_key(script_uri));
        assert!(registry.mutations.contains_key(script_uri));
        assert!(registry.subscriptions.contains_key(script_uri));
        assert_eq!(
            registry.queries[script_uri].resolver_function,
            "testResolver"
        );
        assert_eq!(
            registry.mutations[script_uri].resolver_function,
            "testMutationResolver"
        );
        assert_eq!(
            registry.subscriptions[script_uri].resolver_function,
            "testSubscriptionResolver"
        );
    }

    // Clear the registrations
    clear_script_graphql_registrations(script_uri);

    // Verify they were cleared
    {
        let registry = GRAPHQL_REGISTRY.read().unwrap();
        assert!(!registry.queries.contains_key(script_uri));
        assert!(!registry.mutations.contains_key(script_uri));
        assert!(!registry.subscriptions.contains_key(script_uri));
    }
}

// ============================================================================
// Request Body Limit Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_oversized_request_body_is_rejected() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Default security.max_request_body_bytes is 1 MB; send a larger body
    let oversized = "x".repeat(1024 * 1024 + 100 * 1024);

    // Dynamic script route with a non-form content type must reject with 413
    let response = client
        .post(format!("http://127.0.0.1:{}/engine/upsert_script", port))
        .header("content-type", "application/json")
        .body(oversized.clone())
        .send()
        .await
        .expect("request failed");
    assert_eq!(response.status(), 413);

    // GraphQL endpoint reports the limit as a JSON error
    let response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("content-type", "application/json")
        .body(oversized)
        .send()
        .await
        .expect("request failed");
    let body = response.text().await.expect("failed to read body");
    assert!(
        body.contains("too large"),
        "GraphQL should reject oversized bodies, got: {}",
        body
    );

    context.cleanup().await.expect("Failed to cleanup");
}
