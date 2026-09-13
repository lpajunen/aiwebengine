//! OpenAPI Specification Validation Tests
//!
//! This module validates that the generated OpenAPI spec at /engine/openapi.json:
//! - Is valid OpenAPI 3.0 JSON
//! - Contains all Rust-implemented endpoints
//! - Contains security schemes
//! - Has no duplicate paths

mod common;

use common::AdminServer;
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
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
        "/engine/health/cluster",
        // The file-editing surface. Each of these is half of a pair — read and
        // edit, check and write, locate and read — and a client that cannot
        // find one of them in the document cannot run the loop they form.
        "/engine/read_script",
        "/engine/edit_script",
        "/engine/search",
        "/engine/assets",
        "/engine/assets/batch",
        "/engine/run_tests",
        "/engine/script_logs",
        "/engine/script_logs/stream",
        "/graphql",
        "/graphql/ws",
        "/graphql/sse",
        "/mcp",
        // Every way in, federated or internal. These are handlers a client
        // has to be able to find, and each of them carried a `utoipa::path`
        // annotation while being absent from the generated document.
        "/auth/login",
        "/auth/account",
        "/auth/guest",
        "/auth/local/register",
        "/auth/local/login",
        "/auth/local/password",
        "/auth/local/claim",
        "/auth/local/recovery_codes",
        "/auth/local/recover",
        "/auth/sessions",
        "/auth/sessions/revoke",
        "/auth/oauth2/authorize",
        "/auth/oauth2/consent",
        "/auth/oauth2/token",
        "/auth/oauth2/register",
        "/.well-known/oauth-authorization-server",
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/{resource}",
        "/.well-known/microsoft-identity-association.json",
    ];

    for endpoint in required_endpoints {
        assert!(
            paths.contains_key(endpoint),
            "OpenAPI spec should contain Rust endpoint: {}",
            endpoint
        );
    }
}

/// The parameters a caller needs in order to use the file-editing surface at
/// all: a document naming the endpoint but not its filters describes a
/// capability nobody can reach.
#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_documents_the_file_editing_parameters() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let spec: Value = engine
        .client()
        .get(format!("http://localhost:{}/engine/openapi.json", port))
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec")
        .json()
        .await
        .expect("Failed to parse JSON");

    let names = |path: &str, method: &str| -> Vec<String> {
        spec["paths"][path][method]["parameters"]
            .as_array()
            .map(|parameters| {
                parameters
                    .iter()
                    .filter_map(|parameter| parameter["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    for parameter in ["uri", "lines", "grep"] {
        assert!(
            names("/engine/read_script", "get").contains(&parameter.to_string()),
            "a scoped read of a script's root source needs '{}' documented, got {:?}",
            parameter,
            names("/engine/read_script", "get")
        );
    }

    for parameter in ["query", "scope", "script"] {
        assert!(
            names("/engine/search", "get").contains(&parameter.to_string()),
            "searching needs '{}' documented, got {:?}",
            parameter,
            names("/engine/search", "get")
        );
    }

    // The precondition that turns an asset write into a create.
    assert!(
        names("/engine/assets", "post").contains(&"If-None-Match".to_string()),
        "creating rather than overwriting needs its header documented, got {:?}",
        names("/engine/assets", "post")
    );

    // A body a caller has to guess at is a body they will get wrong: the batch
    // takes three fields that are not `files`, and the edit endpoints take
    // their preconditions in the body rather than the query.
    let body_description = |path: &str, method: &str| -> String {
        spec["paths"][path][method]["requestBody"]["description"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    for field in ["content", "remove", "reinit"] {
        assert!(
            body_description("/engine/assets/batch", "post").contains(field),
            "the batch body needs '{}' described, got {:?}",
            field,
            body_description("/engine/assets/batch", "post")
        );
    }
    for field in ["edits", "base_sha256", "reinit"] {
        assert!(
            body_description("/engine/edit_script", "post").contains(field),
            "the script edit body needs '{}' described, got {:?}",
            field,
            body_description("/engine/edit_script", "post")
        );
        assert!(
            body_description("/engine/assets", "patch").contains(field),
            "the asset edit body needs '{}' described, got {:?}",
            field,
            body_description("/engine/assets", "patch")
        );
    }

    // And the refusals a caller has to handle: a conflict is the one that means
    // "re-read and try again" rather than "stop".
    for (path, method) in [
        ("/engine/edit_script", "post"),
        ("/engine/assets", "patch"),
        ("/engine/assets", "post"),
    ] {
        assert!(
            spec["paths"][path][method]["responses"]["409"].is_object(),
            "{} {} should document its 409",
            method,
            path
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_documents_both_logout_methods() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");

    // A link signs out by GET and a form by POST; the route serves both, so a
    // document naming only one of them describes a smaller API than exists.
    for method in ["get", "post"] {
        assert!(
            spec["paths"]["/auth/logout"][method].is_object(),
            "OpenAPI spec should document {} /auth/logout",
            method.to_uppercase()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_has_security_schemes() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
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
    let engine = AdminServer::start().await.expect("server failed to start");

    // The built-in scripts no longer register HTTP routes (engine endpoints
    // are native Rust), so load a script that registers routes via
    // routeRegistry to exercise the JavaScript path merging.
    engine
        .deploy_script(
            "https://example.com/method_test",
            include_str!("../scripts/test_scripts/method_test.js"),
        )
        .await;

    let port = engine.port();

    let client = engine.client();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    // JS script init() runs asynchronously after the server becomes ready.
    // Under parallel test load this can take noticeably longer than 500 ms.
    // Poll the OpenAPI spec until at least one JavaScript-registered route
    // appears, or until we hit the deadline (~10 s).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let response = client
            .get(&url)
            .send()
            .await
            .expect("Failed to fetch OpenAPI spec");

        let spec: Value = response.json().await.expect("Failed to parse JSON");
        let paths = spec["paths"]
            .as_object()
            .expect("OpenAPI spec should have paths object");

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

        if has_any_js_route {
            return; // ✓ JavaScript routes are present
        }

        if tokio::time::Instant::now() >= deadline {
            panic!(
                "OpenAPI spec should include at least one JavaScript-registered route with \
                 x-source marker (timed out waiting for JS init to complete)"
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Asset and stream routes registered without explicit tags must fall back to
/// the "Assets" and "Streams" Swagger groups respectively. method_test.js
/// registers `/method-test.css` (asset) and the engine registers
/// `/engine/script_updates` (stream) with no tags.
#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_asset_and_stream_default_groups() {
    let engine = AdminServer::start().await.expect("server failed to start");

    // core.js no longer registers asset routes; load a script that does.
    // registerAssetRoute requires the asset to exist and be owned by the
    // registering script, so store the asset first.
    // In this order, and the write of each is checked: an asset is keyed by the
    // script that owns it, so the row has to exist before the asset can be
    // stored, and the asset has to be stored before the `init()` that registers
    // a route onto it can succeed. Both writes used to be made with their
    // results discarded, and the asset's was failing for want of the script
    // row — which the test could not notice, because it was reading an asset
    // route that an earlier run had left registered.
    let uri = "https://example.com/method_test";
    aiwebengine::repository::upsert_script(
        uri,
        include_str!("../scripts/test_scripts/method_test.js"),
    )
    .expect("script should be stored");

    let now = std::time::SystemTime::now();
    aiwebengine::repository::upsert_asset(aiwebengine::repository::Asset {
        uri: "method-test.css".to_string(),
        name: Some("method-test.css".to_string()),
        mimetype: "text/css".to_string(),
        content: b"body { color: black; }".to_vec(),
        created_at: now,
        updated_at: now,
        script_uri: uri.to_string(),
    })
    .expect("asset should be stored");

    engine.initialize_script(uri).await;

    let port = engine.port();

    let client = engine.client();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    // Returns the "tags" array for GET on `path` if the operation is present.
    fn get_tags(spec: &Value, path: &str) -> Option<Vec<String>> {
        spec["paths"][path]["get"]["tags"].as_array().map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
    }

    // JS init runs asynchronously; poll until both routes appear or we time out.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let spec: Value = client
            .get(&url)
            .send()
            .await
            .expect("Failed to fetch OpenAPI spec")
            .json()
            .await
            .expect("Failed to parse JSON");

        let asset_tags = get_tags(&spec, "/method-test.css");
        let stream_tags = get_tags(&spec, "/engine/script_updates");

        if let (Some(asset_tags), Some(stream_tags)) = (asset_tags, stream_tags) {
            assert_eq!(
                asset_tags,
                vec!["Assets".to_string()],
                "untagged asset route should default to the Assets group"
            );
            assert_eq!(
                stream_tags,
                vec!["Streams".to_string()],
                "untagged stream route should default to the Streams group"
            );
            return;
        }

        if tokio::time::Instant::now() >= deadline {
            panic!(
                "OpenAPI spec should include /method-test.css (asset) and \
                 /engine/script_updates (stream) routes (timed out waiting for JS init to complete)"
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// The log tail answers with an event stream, not a document. A client reading
/// the spec has to be able to tell that apart before it opens the connection,
/// the same way it can for the GraphQL subscription endpoints.
#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_log_tail_is_marked_as_an_event_stream() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
    let url = format!("http://localhost:{}/engine/openapi.json", port);

    let response = client
        .get(&url)
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec");

    let spec: Value = response.json().await.expect("Failed to parse JSON");
    let operation = &spec["paths"]["/engine/script_logs/stream"]["get"];
    assert!(
        operation.is_object(),
        "the log tail should be documented in the OpenAPI spec"
    );
    assert_eq!(
        operation["x-protocol"].as_str(),
        Some("text/event-stream"),
        "the log tail should be marked as an event stream"
    );
    assert_eq!(operation["x-transport"].as_str(), Some("sse"));
}

/// A limit a caller cannot find is one they meet by surprise. The document has
/// to carry the ones this engine enforces, and carry the values it is actually
/// running rather than the defaults it shipped with.
#[tokio::test(flavor = "multi_thread")]
async fn test_openapi_publishes_the_limits_a_developer_can_meet() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let spec: Value = engine
        .client()
        .get(format!("http://localhost:{}/engine/openapi.json", port))
        .send()
        .await
        .expect("Failed to fetch OpenAPI spec")
        .json()
        .await
        .expect("Failed to parse JSON");

    let limits = &spec["x-aiwebengine-limits"];
    assert!(
        limits.is_object(),
        "the spec should publish x-aiwebengine-limits, got {}",
        limits
    );

    // The groups a script author asks about, each of which has bitten someone.
    for group in [
        "execution",
        "size",
        "database",
        "fetch",
        "graphql",
        "scheduler",
        "search",
        "retention",
    ] {
        assert!(
            limits[group].is_object(),
            "x-aiwebengine-limits should describe '{}', got {}",
            group,
            limits
        );
    }

    // Published numbers must be the ones the engine enforces, not a second
    // copy that can drift from them.
    assert_eq!(
        limits["database"]["maxTablesPerScript"].as_u64(),
        Some(aiwebengine::db_schema_utils::MAX_TABLES_PER_SCRIPT as u64),
    );
    assert_eq!(
        limits["database"]["maxQueryLimit"].as_i64(),
        Some(aiwebengine::repository::MAX_QUERY_LIMIT),
    );
    assert_eq!(
        limits["fetch"]["maxResponseBytes"].as_u64(),
        Some(aiwebengine::http_client::MAX_RESPONSE_SIZE as u64),
    );
    assert_eq!(
        limits["size"]["maxStorageValueBytes"].as_u64(),
        Some(aiwebengine::repository::MAX_STORAGE_VALUE_BYTES as u64),
    );
    assert_eq!(
        limits["scheduler"]["minRecurringIntervalMs"].as_i64(),
        Some(aiwebengine::scheduler::MIN_RECURRING_INTERVAL_MS),
    );

    // The execution budget is configuration, so it has to arrive from the
    // running engine rather than from a default written into the document.
    assert_eq!(
        limits["execution"]["timeoutMs"].as_u64(),
        Some(aiwebengine::js_engine::current_execution_limits().timeout_ms),
    );

    // The constraints that are not numbers are the ones that surprise people,
    // so they are published too.
    let notes = limits["notes"].as_array().expect("notes should be a list");
    assert!(
        !notes.is_empty(),
        "the execution model belongs in the document beside the numbers"
    );

    // And the prose half points at the machine-readable half.
    let description = spec["info"]["description"].as_str().unwrap_or_default();
    assert!(
        description.contains("x-aiwebengine-limits"),
        "the document's own description should say where the limits are, got {:?}",
        description
    );
}

/// The type definitions are the other document a script author reads, and they
/// cannot link to the spec for the constraints that shape how code is written
/// — the ones that are not numbers at all.
#[tokio::test(flavor = "multi_thread")]
async fn test_type_definitions_describe_the_execution_model() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let types = engine
        .client()
        .get(format!(
            "http://localhost:{}/engine/types/v{}/aiwebengine.d.ts",
            port,
            env!("CARGO_PKG_VERSION")
        ))
        .send()
        .await
        .expect("Failed to fetch type definitions")
        .text()
        .await
        .expect("Failed to read type definitions");

    for topic in [
        "x-aiwebengine-limits",
        "There are no timers",
        "There is no concurrency",
        "fresh runtime",
        "50 tables per script",
        "at most 5 redirects",
        "Retention",
    ] {
        assert!(
            types.contains(topic),
            "the type definitions should describe '{}'",
            topic
        );
    }
}
