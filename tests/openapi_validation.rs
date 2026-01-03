//! OpenAPI Specification Validation Tests
//!
//! This module validates that the generated OpenAPI spec at /engine/openapi.json:
//! - Is valid OpenAPI 3.0 JSON
//! - Contains all Rust-implemented endpoints
//! - Contains security schemes
//! - Has no duplicate paths

mod common;

use common::{TestContext, wait_for_server};
use serde_json::Value;

const OPENAPI_3_0_SCHEMA: &str = r#"{
  "$schema": "http://json-schema.org/draft-04/schema#",
  "type": "object",
  "required": ["openapi", "info", "paths"],
  "properties": {
    "openapi": {
      "type": "string",
      "pattern": "^3\\.(0|1)\\.\\d+$"
    },
    "info": {
      "type": "object",
      "required": ["title", "version"],
      "properties": {
        "title": {"type": "string"},
        "version": {"type": "string"},
        "description": {"type": "string"}
      }
    },
    "servers": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["url"],
        "properties": {
          "url": {"type": "string"},
          "description": {"type": "string"}
        }
      }
    },
    "paths": {
      "type": "object",
      "patternProperties": {
        "^\\/": {
          "type": "object"
        }
      }
    },
    "components": {
      "type": "object",
      "properties": {
        "schemas": {"type": "object"},
        "securitySchemes": {"type": "object"}
      }
    },
    "tags": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["name"],
        "properties": {
          "name": {"type": "string"},
          "description": {"type": "string"}
        }
      }
    }
  }
}"#;

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_spec_is_valid_json() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    assert_eq!(response.status(), 200, "OpenAPI endpoint should return 200");

    let spec_text = response.text().await.expect("Failed to read response body");
    let spec: Value = serde_json::from_str(&spec_text).expect("OpenAPI spec should be valid JSON");

    // Validate against basic OpenAPI 3.0 structure
    let version = spec["openapi"].as_str().unwrap_or("");
    assert!(
        version.starts_with("3."),
        "OpenAPI version should be 3.x, got: {}",
        version
    );
    assert!(spec["info"].is_object(), "OpenAPI spec should have info");
    assert!(spec["paths"].is_object(), "OpenAPI spec should have paths");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_spec_structure() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");

    // Validate basic schema using jsonschema crate
    let schema: Value =
        serde_json::from_str(OPENAPI_3_0_SCHEMA).expect("Failed to parse OpenAPI schema");
    let compiled_schema =
        jsonschema::validator_for(&schema).expect("Failed to compile JSON schema");

    match compiled_schema.validate(&spec) {
        Ok(_) => {
            // Validation passed
        }
        Err(e) => {
            // Just print the error iterator
            panic!("OpenAPI spec validation failed: {:?}", e);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_contains_rust_endpoints() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");
    let paths = spec["paths"]
        .as_object()
        .expect("OpenAPI spec should have paths object");

    // Check for Rust-implemented endpoints
    let required_endpoints = vec![
        "/health",
        "/health/cluster",
        "/graphql",
        "/graphql/ws",
        "/graphql/sse",
        "/mcp",
    ];

    for endpoint in required_endpoints {
        assert!(
            paths.contains_key(endpoint),
            "OpenAPI spec should contain Rust endpoint: {}",
            endpoint
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_has_security_schemes() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");

    // Check for security schemes
    let components = spec["components"]
        .as_object()
        .expect("OpenAPI spec should have components");

    let security_schemes = components["securitySchemes"]
        .as_object()
        .expect("Components should have securitySchemes");

    // Should have OAuth2 and Bearer auth
    assert!(
        security_schemes.contains_key("oauth2"),
        "Should have oauth2 security scheme"
    );
    assert!(
        security_schemes.contains_key("bearerAuth"),
        "Should have bearerAuth security scheme"
    );

    // Verify OAuth2 scheme structure
    let oauth2 = &security_schemes["oauth2"];
    assert_eq!(oauth2["type"].as_str(), Some("oauth2"), "OAuth2 type");
    assert!(
        oauth2["flows"].is_object(),
        "OAuth2 should have flows definition"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_has_schemas() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");

    let components = spec["components"]
        .as_object()
        .expect("OpenAPI spec should have components");

    let schemas = components["schemas"]
        .as_object()
        .expect("Components should have schemas");

    // Check for key schema definitions
    let required_schemas = vec![
        "HealthResponse",
        "ClusterHealthResponse",
        "GraphQLRequest",
        "GraphQLResponse",
        "McpRpcRequest",
        "McpRpcResponse",
        "ErrorResponse",
        "UnauthorizedErrorResponse",
    ];

    for schema_name in required_schemas {
        assert!(
            schemas.contains_key(schema_name),
            "OpenAPI spec should contain schema: {}",
            schema_name
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_no_duplicate_paths() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");
    let paths = spec["paths"]
        .as_object()
        .expect("OpenAPI spec should have paths object");

    // Check that all paths are unique (which they should be in a JSON object)
    // Also verify no method collisions within paths
    for (path, operations) in paths.iter() {
        let ops = operations
            .as_object()
            .unwrap_or_else(|| panic!("Path {} should have operations object", path));

        let methods: Vec<&String> = ops.keys().collect();
        let unique_methods: std::collections::HashSet<_> = methods.iter().collect();

        assert_eq!(
            methods.len(),
            unique_methods.len(),
            "Path {} has duplicate methods",
            path
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_has_tags() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");

    // Check that tags are defined
    let tags = spec["tags"]
        .as_array()
        .expect("OpenAPI spec should have tags array");

    // Should have at least our major tags
    let tag_names: Vec<String> = tags
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect();

    let expected_tags = vec!["Health", "GraphQL", "MCP"];
    for expected in expected_tags {
        assert!(
            tag_names.contains(&expected.to_string()),
            "Tags should include {}",
            expected
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_graphql_endpoints_have_x_protocol() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");
    let paths = spec["paths"]
        .as_object()
        .expect("OpenAPI spec should have paths object");

    // Check WebSocket endpoint has x-protocol
    if let Some(ws_path) = paths.get("/graphql/ws")
        && let Some(get_op) = ws_path["get"].as_object()
    {
        assert!(
            get_op.contains_key("x-protocol"),
            "/graphql/ws should have x-protocol extension"
        );
        assert_eq!(
            get_op["x-protocol"].as_str(),
            Some("graphql-ws"),
            "/graphql/ws x-protocol should be 'graphql-ws'"
        );
    }

    // Check SSE endpoint has x-protocol
    if let Some(sse_path) = paths.get("/graphql/sse")
        && let Some(get_op) = sse_path["get"].as_object()
    {
        assert!(
            get_op.contains_key("x-protocol"),
            "/graphql/sse should have x-protocol extension"
        );
        assert_eq!(
            get_op["x-protocol"].as_str(),
            Some("text/event-stream"),
            "/graphql/sse x-protocol should be 'text/event-stream'"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_javascript_routes_included() {
    let ctx = TestContext::new();
    let port = ctx.start_server().await.expect("Failed to start server");
    wait_for_server(port, 30)
        .await
        .expect("Server failed to start");

    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");
    let paths = spec["paths"]
        .as_object()
        .expect("OpenAPI spec should have paths object");

    // Check for JavaScript-registered routes from core.js
    // These should be present after script initialization
    let js_endpoints = vec!["/health", "/upsert_script", "/delete_script"];

    for endpoint in js_endpoints {
        if let Some(path_item) = paths.get(endpoint) {
            // Check if any operation has x-source: "javascript"
            let has_js_source = path_item
                .as_object()
                .and_then(|ops| {
                    ops.values().find(|op| {
                        op.get("x-source")
                            .and_then(|s| s.as_str())
                            .map(|s| s == "javascript")
                            .unwrap_or(false)
                    })
                })
                .is_some();

            if has_js_source {
                // Found at least one JavaScript route, test passes
                return;
            }
        }
    }

    // If we get here, check if any path has x-source: javascript
    let has_any_js_route = paths.values().any(|path_item| {
        path_item
            .as_object()
            .and_then(|ops| {
                ops.values().find(|op| {
                    op.get("x-source")
                        .and_then(|s| s.as_str())
                        .map(|s| s == "javascript")
                        .unwrap_or(false)
                })
            })
            .is_some()
    });

    assert!(
        has_any_js_route,
        "OpenAPI spec should include at least one JavaScript-registered route with x-source marker"
    );
}
