use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn test_graphql_endpoints() {
    // Load the GraphQL test script
    let _ = repository::upsert_script(
        "https://example.com/graphql_test",
        include_str!("../scripts/test_scripts/graphql_test.js"),
    );

    // Start server in background task
    let port = start_server_without_shutdown()
        .await
        .expect("server failed to start");
    tokio::spawn(async move {
        // Server is already started, just keep it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = reqwest::Client::new();

    // Test GraphiQL GET endpoint
    let graphiql_response = client
        .get(format!("http://127.0.0.1:{}/graphql", port))
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

    // Should contain data (even if placeholder)
    assert!(query_json["data"].is_object());
    assert!(query_json["data"]["hello"].is_string());

    // Test script management GraphQL operations
    // Test listing scripts (currently returns placeholder since JS resolver calling not implemented)
    let list_scripts_query = r#"{ scripts }"#;
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

    // Should return actual script data from JavaScript resolver
    assert!(list_json["data"]["scripts"].is_string());
    let scripts_response = list_json["data"]["scripts"].as_str().unwrap();
    // Should contain script URIs and character counts
    assert!(scripts_response.contains("https://example.com/core"));
    assert!(scripts_response.contains("chars"));

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
}
