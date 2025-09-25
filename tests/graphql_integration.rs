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

    // Load the core script to get script management GraphQL operations
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
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
    // Test listing scripts - now requires subfield selection since scripts returns ScriptInfo objects
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

    // Should return actual script data from JavaScript resolver as an array of objects
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

    // Test script query - should return ScriptDetail object instead of string
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

    // Should return a ScriptDetail object with uri field - this proves it's an object, not a string
    assert!(script_json["data"]["script"].is_object());
    let script_obj = &script_json["data"]["script"];

    // Verify the object has the uri field
    assert!(script_obj["uri"].is_string());

    // Verify the uri field matches what we requested
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
}

#[tokio::test]
async fn test_graphql_script_mutations() {
    // Load the core script to get script management GraphQL operations
    let _ = repository::upsert_script(
        "https://example.com/core",
        include_str!("../scripts/feature_scripts/core.js"),
    );

    // Start server in background task
    let port = start_server_without_shutdown()
        .await
        .expect("server failed to start");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = reqwest::Client::new();

    // Test upsertScript mutation - should return structured UpsertScriptResponse object
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

    // Check for errors
    if let Some(errors) = upsert_json.get("errors") {
        panic!("GraphQL upsert mutation failed with errors: {:?}", errors);
    }

    // Verify the response structure
    assert!(upsert_json["data"]["upsertScript"].is_object());
    let upsert_result = &upsert_json["data"]["upsertScript"];

    // Check all required fields are present and have correct types
    assert!(upsert_result["message"].is_string());
    assert!(upsert_result["uri"].is_string());
    assert!(upsert_result["chars"].is_number());
    assert!(upsert_result["success"].is_boolean());

    // Verify field values
    assert_eq!(upsert_result["uri"].as_str().unwrap(), "http://test/script");
    assert_eq!(upsert_result["chars"].as_u64().unwrap(), 20); // "console.log('test');" is 20 characters
    assert_eq!(upsert_result["success"].as_bool().unwrap(), true);
    assert!(
        upsert_result["message"]
            .as_str()
            .unwrap()
            .contains("Script upserted successfully")
    );

    // Test deleteScript mutation - should return structured DeleteScriptResponse object
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

    // Check for errors
    if let Some(errors) = delete_json.get("errors") {
        panic!("GraphQL delete mutation failed with errors: {:?}", errors);
    }

    // Verify the response structure
    assert!(delete_json["data"]["deleteScript"].is_object());
    let delete_result = &delete_json["data"]["deleteScript"];

    // Check all required fields are present and have correct types
    assert!(delete_result["message"].is_string());
    assert!(delete_result["uri"].is_string());
    assert!(delete_result["success"].is_boolean());

    // Verify field values
    assert_eq!(delete_result["uri"].as_str().unwrap(), "http://test/script");
    assert_eq!(delete_result["success"].as_bool().unwrap(), true);
    assert!(
        delete_result["message"]
            .as_str()
            .unwrap()
            .contains("Script deleted successfully")
    );

    // Test deleteScript with non-existent script - should return success: false
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

    // Check for errors
    if let Some(errors) = delete_nonexistent_json.get("errors") {
        panic!(
            "GraphQL delete nonexistent mutation failed with errors: {:?}",
            errors
        );
    }

    // Verify the response structure for non-existent script
    assert!(delete_nonexistent_json["data"]["deleteScript"].is_object());
    let delete_nonexistent_result = &delete_nonexistent_json["data"]["deleteScript"];

    // Check all required fields are present and have correct types
    assert!(delete_nonexistent_result["message"].is_string());
    assert!(delete_nonexistent_result["uri"].is_string());
    assert!(delete_nonexistent_result["success"].is_boolean());

    // Verify field values for non-existent script
    assert_eq!(
        delete_nonexistent_result["uri"].as_str().unwrap(),
        "http://test/nonexistent"
    );
    assert_eq!(
        delete_nonexistent_result["success"].as_bool().unwrap(),
        false
    );
    assert!(
        delete_nonexistent_result["message"]
            .as_str()
            .unwrap()
            .contains("Script not found")
    );
}
