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
use common::{TestContext, wait_for_server};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// ============================================================================
// Health Endpoint Tests
// ============================================================================

#[tokio::test]
async fn test_health_endpoint() {
    let context = TestContext::new();

    // Load the core script which contains the health endpoint
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

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
    assert!(health_json["checks"].is_object());

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_health_endpoint_content_type() {
    let context = TestContext::new();

    // Load the core script
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

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

#[tokio::test]
async fn test_script_logs_endpoint() {
    let context = TestContext::new();

    // Load the core script which contains the script_logs endpoint
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test script_logs endpoint with a valid URI parameter
    let logs_response = client
        .get(format!(
            "http://127.0.0.1:{}/script_logs?uri=https://example.com/core",
            port
        ))
        .send()
        .await
        .expect("Script logs request failed");

    assert_eq!(logs_response.status(), 200);

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// HTTP Methods Tests
// ============================================================================

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

// ============================================================================
// Query Parameters Tests
// ============================================================================

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

#[tokio::test]
async fn test_form_data() {
    // Initialize tracing for test logging
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().compact())
        .try_init();

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

#[tokio::test]
async fn test_graphql_endpoints() {
    let context = TestContext::new();

    // Load the GraphQL test script
    let _ = repository::upsert_script(
        "https://example.com/graphql_test",
        include_str!("../scripts/test_scripts/graphql_test.js"),
    );

    // Load the core script to get script management GraphQL operations
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test GraphiQL GET endpoint
    let graphiql_response = client
        .get(format!("http://127.0.0.1:{}/engine/graphql", port))
        .send()
        .await
        .expect("GraphiQL request failed");

    assert_eq!(graphiql_response.status(), 200);

    let graphiql_body = graphiql_response
        .text()
        .await
        .expect("Failed to read GraphiQL response");

    // Check that GraphiQL HTML contains expected elements
    assert!(graphiql_body.contains("GraphiQL"));
    assert!(graphiql_body.contains("/graphql"));

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

    // Test script management GraphQL operations
    let list_scripts_query = r#"{ scripts { uri chars } }"#;
    let list_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(format!(r#"{{"query": "{}"}}"#, list_scripts_query))
        .send()
        .await
        .expect("GraphQL list scripts request failed");

    assert_eq!(list_response.status(), 200);

    let list_body = list_response
        .text()
        .await
        .expect("Failed to read list scripts response");

    let list_json: serde_json::Value =
        serde_json::from_str(&list_body).expect("Failed to parse list scripts response");

    if let Some(errors) = list_json.get("errors") {
        panic!("GraphQL scripts query failed with errors: {:?}", errors);
    }

    // Should return actual script data as an array of objects
    assert!(list_json["data"]["scripts"].is_array());
    let scripts_array = list_json["data"]["scripts"].as_array().unwrap();
    assert!(!scripts_array.is_empty());

    // Should contain script objects with uri and chars properties
    let has_test_script = scripts_array.iter().any(|script| {
        (script["uri"]
            .as_str()
            .unwrap_or("")
            .contains("https://example.com/graphql_test")
            || script["uri"]
                .as_str()
                .unwrap_or("")
                .contains("https://example.com/core"))
            && script["chars"].is_number()
    });
    assert!(
        has_test_script,
        "Should contain test script with uri and chars properties"
    );

    // Test script query - should return ScriptDetail object
    let script_query = r#"{ script(uri: \"https://example.com/graphql_test\") { uri } }"#;
    let script_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(format!(r#"{{"query": "{}"}}"#, script_query))
        .send()
        .await
        .expect("GraphQL script query request failed");

    assert_eq!(script_response.status(), 200);

    let script_body = script_response
        .text()
        .await
        .expect("Failed to read script query response");

    let script_json: serde_json::Value =
        serde_json::from_str(&script_body).expect("Failed to parse script query response");

    if let Some(errors) = script_json.get("errors") {
        panic!("GraphQL script query failed with errors: {:?}", errors);
    }

    // Should return a ScriptDetail object with uri field
    assert!(script_json["data"]["script"].is_object());
    let script_obj = &script_json["data"]["script"];

    assert!(script_obj["uri"].is_string());
    assert_eq!(
        script_obj["uri"].as_str().unwrap(),
        "https://example.com/graphql_test"
    );

    // Test GraphQL SSE endpoint (basic connectivity test)
    let sse_response = client
        .post(format!("http://127.0.0.1:{}/graphql/sse", port))
        .header("Content-Type", "application/json")
        .body(r#"{"query": "subscription { userUpdates }"}"#)
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

#[tokio::test]
async fn test_graphql_script_mutations() {
    let context = TestContext::new();

    // Clean up any existing test scripts
    let _ = repository::delete_script("http://test/script");
    let _ = repository::delete_script("http://test/nonexistent");

    // Load the core script to get script management GraphQL operations
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test upsertScript mutation (now JavaScript-defined)
    let upsert_mutation_body = serde_json::json!({
        "query": "mutation { upsertScript(uri: \"http://test/script\", content: \"console.log('test');\") { message uri chars success } }"
    });

    let upsert_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(upsert_mutation_body.to_string())
        .send()
        .await
        .expect("GraphQL upsert mutation request failed");

    assert_eq!(upsert_response.status(), 200);

    let upsert_body = upsert_response
        .text()
        .await
        .expect("Failed to read upsert mutation response");

    let upsert_json: serde_json::Value =
        serde_json::from_str(&upsert_body).expect("Failed to parse upsert mutation response");

    if let Some(errors) = upsert_json.get("errors") {
        panic!("GraphQL upsert mutation failed with errors: {:?}", errors);
    }

    // Verify the response structure (JavaScript-defined mutation)
    assert!(upsert_json["data"]["upsertScript"].is_object());
    let upsert_result = &upsert_json["data"]["upsertScript"];

    assert!(upsert_result["message"].is_string());
    assert!(upsert_result["uri"].is_string());
    assert!(upsert_result["chars"].is_number());
    assert!(upsert_result["success"].is_boolean());

    assert_eq!(upsert_result["uri"].as_str().unwrap(), "http://test/script");
    assert_eq!(upsert_result["chars"].as_u64().unwrap(), 20);
    assert!(upsert_result["success"].as_bool().unwrap());
    assert!(
        upsert_result["message"]
            .as_str()
            .unwrap()
            .contains("Script upserted successfully")
    );

    // Test deleteScript mutation (now JavaScript-defined)
    let delete_mutation_body = serde_json::json!({
        "query": "mutation { deleteScript(uri: \"http://test/script\") { message uri success } }"
    });

    let delete_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(delete_mutation_body.to_string())
        .send()
        .await
        .expect("GraphQL delete mutation request failed");

    assert_eq!(delete_response.status(), 200);

    let delete_body = delete_response
        .text()
        .await
        .expect("Failed to read delete mutation response");

    let delete_json: serde_json::Value =
        serde_json::from_str(&delete_body).expect("Failed to parse delete mutation response");

    if let Some(errors) = delete_json.get("errors") {
        panic!("GraphQL delete mutation failed with errors: {:?}", errors);
    }

    // Verify the response structure (JavaScript-defined mutation)
    assert!(delete_json["data"]["deleteScript"].is_object());
    let delete_result = &delete_json["data"]["deleteScript"];

    assert!(delete_result["message"].is_string());
    assert!(delete_result["uri"].is_string());
    assert!(delete_result["success"].is_boolean());

    assert_eq!(delete_result["uri"].as_str().unwrap(), "http://test/script");
    assert!(delete_result["success"].as_bool().unwrap());
    assert!(
        delete_result["message"]
            .as_str()
            .unwrap()
            .contains("deleted successfully")
    );

    // Test deleteScript with non-existent script (JavaScript-defined mutation)
    let delete_nonexistent_mutation_body = serde_json::json!({
        "query": "mutation { deleteScript(uri: \"http://test/nonexistent\") { message uri success } }"
    });

    let delete_nonexistent_response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .header("Content-Type", "application/json")
        .body(delete_nonexistent_mutation_body.to_string())
        .send()
        .await
        .expect("GraphQL delete nonexistent mutation request failed");

    assert_eq!(delete_nonexistent_response.status(), 200);

    let delete_nonexistent_body = delete_nonexistent_response
        .text()
        .await
        .expect("Failed to read delete nonexistent mutation response");

    let delete_nonexistent_json: serde_json::Value = serde_json::from_str(&delete_nonexistent_body)
        .expect("Failed to parse delete nonexistent mutation response");

    if let Some(errors) = delete_nonexistent_json.get("errors") {
        panic!(
            "GraphQL delete nonexistent mutation failed with errors: {:?}",
            errors
        );
    }

    // Verify the response structure for non-existent script (JavaScript-defined mutation)
    assert!(delete_nonexistent_json["data"]["deleteScript"].is_object());
    let delete_nonexistent_result = &delete_nonexistent_json["data"]["deleteScript"];

    assert!(delete_nonexistent_result["message"].is_string());
    assert!(delete_nonexistent_result["uri"].is_string());
    assert!(delete_nonexistent_result["success"].is_boolean());

    assert_eq!(
        delete_nonexistent_result["uri"].as_str().unwrap(),
        "http://test/nonexistent"
    );
    assert!(!delete_nonexistent_result["success"].as_bool().unwrap());
    assert!(
        delete_nonexistent_result["message"]
            .as_str()
            .unwrap()
            .contains("not found")
    );

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_graphql_registration_clearing() {
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
            },
        );
        registry.mutations.insert(
            script_uri.to_string(),
            GraphQLOperation {
                sdl: "type Mutation { testMutation: String }".to_string(),
                resolver_function: "testMutationResolver".to_string(),
                script_uri: script_uri.to_string(),
            },
        );
        registry.subscriptions.insert(
            script_uri.to_string(),
            GraphQLOperation {
                sdl: "type Subscription { testSubscription: String }".to_string(),
                resolver_function: "testSubscriptionResolver".to_string(),
                script_uri: script_uri.to_string(),
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
