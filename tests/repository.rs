//! Core Repository Tests
//!
//! This module contains tests for core repository operations including:
//! - Script lifecycle (create, read, update, delete)
//! - Asset management
//! - Log message storage and pruning
//! - GraphQL subscription schema configuration

mod common;

use aiwebengine::repository;
use common::{TestContext, wait_for_server};
use serde_json::{Value, json};
use tracing::info;

// ============================================================================
// Script Repository Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_dynamic_script_lifecycle_memory() {
    test_dynamic_script_lifecycle("memory").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_dynamic_script_lifecycle_postgres() {
    // Skip if no database connection
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    test_dynamic_script_lifecycle("postgresql").await;
}

async fn test_dynamic_script_lifecycle(storage_type: &str) {
    let context = TestContext::new();
    let port = context
        .start_server_with_storage(storage_type)
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    // Verify initial static scripts exist
    let scripts = repository::fetch_scripts();
    assert!(scripts.contains_key("https://example.com/core"));

    // Upsert a dynamic script
    let _ = repository::upsert_script(
        "https://example.com/dyn",
        "routeRegistry.registerRoute('/dyn', (req) => ({ status: 200, body: 'dyn' }));",
    );
    let scripts = repository::fetch_scripts();
    assert!(scripts.contains_key("https://example.com/dyn"));

    // Fetch single script
    let one = repository::fetch_script("https://example.com/dyn");
    assert!(one.is_some());
    assert!(one.unwrap().contains("/dyn"));

    // Delete script
    let removed = repository::delete_script("https://example.com/dyn");
    assert!(removed);

    let scripts = repository::fetch_scripts();
    assert!(!scripts.contains_key("https://example.com/dyn"));

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_upsert_overwrites_existing_script_memory() {
    test_upsert_overwrites_existing_script("memory").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_upsert_overwrites_existing_script_postgres() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    test_upsert_overwrites_existing_script("postgresql").await;
}

async fn test_upsert_overwrites_existing_script(storage_type: &str) {
    let context = TestContext::new();
    let port = context
        .start_server_with_storage(storage_type)
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let uri = "https://example.com/dyn2";
    let content_v1 =
        "routeRegistry.registerRoute('/dyn2', (req) => ({ status: 200, body: 'v1' }));";
    let content_v2 =
        "routeRegistry.registerRoute('/dyn2', (req) => ({ status: 200, body: 'v2' }));";
    // Upsert v1 and verify
    let _ = repository::upsert_script(uri, content_v1);
    let got = repository::fetch_script(uri);
    assert!(got.is_some());
    assert!(got.unwrap().contains("v1"));

    // Upsert v2 and verify update
    let _ = repository::upsert_script(uri, content_v2);
    let got2 = repository::fetch_script(uri);
    assert!(got2.is_some());
    assert!(got2.unwrap().contains("v2"));

    // Cleanup
    let _ = repository::delete_script(uri);
    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Log Message Repository Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_insert_and_list_log_messages_memory() {
    test_insert_and_list_log_messages("memory").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_insert_and_list_log_messages_postgres() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    test_insert_and_list_log_messages("postgresql").await;
}

async fn test_insert_and_list_log_messages(storage_type: &str) {
    let context = TestContext::new();
    let port = context
        .start_server_with_storage(storage_type)
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    // Use a unique URI for this test to avoid interference from other tests
    let test_uri = format!("test_insert_and_list_log_messages_{}", storage_type);

    // Clear any existing logs for this URI
    let _ = repository::clear_log_messages(&test_uri);

    // Record starting length so test is robust to previous state
    let start = repository::fetch_log_messages(&test_uri).len();

    repository::insert_log_message(&test_uri, "log-one", "INFO");
    repository::insert_log_message(&test_uri, "log-two", "INFO");

    let msgs = repository::fetch_log_messages(&test_uri);
    assert!(
        msgs.len() >= start + 2,
        "expected at least two new messages"
    );
    // Last two messages should be the ones we inserted
    let last = &msgs[msgs.len() - 2..];
    assert_eq!(last[0].message, "log-one");
    assert_eq!(last[1].message, "log-two");

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_prune_keeps_latest_20_logs_memory() {
    test_prune_keeps_latest_20_logs("memory").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_prune_keeps_latest_20_logs_postgres() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    test_prune_keeps_latest_20_logs("postgresql").await;
}

async fn test_prune_keeps_latest_20_logs(storage_type: &str) {
    let context = TestContext::new();
    let port = context
        .start_server_with_storage(storage_type)
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    // Use a unique URI for this test to avoid interference from other tests
    let test_uri = format!("test_prune_keeps_latest_20_logs_{}", storage_type);

    // Clear any existing logs for this URI
    let _ = repository::clear_log_messages(&test_uri);

    // Insert 25 distinct messages
    for i in 0..25 {
        repository::insert_log_message(&test_uri, &format!("prune-test-{}", i), "INFO");
    }

    let _ = repository::prune_log_messages();
    let msgs = repository::fetch_log_messages(&test_uri);
    assert!(msgs.len() <= 20, "prune should keep at most 20 messages");

    // Ensure the latest message is the last one we inserted
    if let Some(last) = msgs.last() {
        assert!(
            last.message.contains("prune-test-24"),
            "expected latest message to be prune-test-24"
        );
    } else {
        panic!("no messages after prune");
    }

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Asset Repository Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_management_memory() {
    test_asset_management("memory").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_management_postgres() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    test_asset_management("postgresql").await;
}

async fn test_asset_management(storage_type: &str) {
    let context = TestContext::new();
    let port = context
        .start_server_with_storage(storage_type)
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    // Test static asset
    let asset = repository::fetch_asset("logo.svg");
    assert!(asset.is_some());
    let asset = asset.unwrap();
    assert_eq!(asset.uri, "logo.svg");
    assert_eq!(asset.mimetype, "image/svg+xml");
    assert!(!asset.content.is_empty());

    // Test listing assets
    let assets = repository::fetch_assets();
    assert!(assets.contains_key("logo.svg"));

    // Test upsert and fetch dynamic asset
    let test_content = b"test content".to_vec();
    let now = std::time::SystemTime::now();
    let test_asset = repository::Asset {
        uri: "test.txt".to_string(),
        name: Some("Test File".to_string()),
        mimetype: "text/plain".to_string(),
        content: test_content.clone(),
        created_at: now,
        updated_at: now,
        script_uri: "https://example.com/core".to_string(),
    };
    let _ = repository::upsert_asset(test_asset);

    let fetched = repository::fetch_asset("test.txt");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.uri, "test.txt");
    assert_eq!(fetched.mimetype, "text/plain");
    assert_eq!(fetched.content, test_content);

    // Test delete
    let deleted = repository::delete_asset("test.txt");
    assert!(deleted);

    // Verify it's gone
    let fetched_after_delete = repository::fetch_asset("test.txt");
    assert!(fetched_after_delete.is_none());

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// GraphQL Subscription Schema Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_subscription_schema_configured() {
    let context = TestContext::new();

    // Start the server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    info!("Server started on port: {}", port);

    // Test GraphQL introspection query for subscription type
    let client = reqwest::Client::new();
    let introspection_query = json!({
        "query": "query { __schema { subscriptionType { name fields { name } } } }"
    });

    let response = client
        .post(format!("http://127.0.0.1:{}/graphql", port))
        .json(&introspection_query)
        .send()
        .await
        .expect("Failed to send introspection query");

    let response_body: Value = response.json().await.expect("Failed to parse response");

    // Check if subscription type exists
    if let Some(schema) = response_body.get("data").and_then(|d| d.get("__schema")) {
        if let Some(subscription_type) = schema.get("subscriptionType") {
            if subscription_type.is_null() {
                panic!("❌ GraphQL subscription type is not configured");
            } else {
                info!("✅ GraphQL subscription type is configured!");
                info!("Subscription type: {:?}", subscription_type);

                // Check for expected subscription fields
                if let Some(fields) = subscription_type.get("fields")
                    && let Some(fields_array) = fields.as_array()
                {
                    let field_names: Vec<String> = fields_array
                        .iter()
                        .filter_map(|f| f.get("name").and_then(|n| n.as_str().map(String::from)))
                        .collect();

                    info!("Available subscription fields: {:?}", field_names);

                    // We expect at least one subscription field (scriptUpdates from core.js)
                    assert!(
                        !field_names.is_empty(),
                        "Expected at least one subscription field"
                    );
                    assert!(
                        field_names.contains(&"scriptUpdates".to_string()),
                        "Expected 'scriptUpdates' subscription field to be present"
                    );
                }
            }
        } else {
            panic!("❌ No subscriptionType field in schema");
        }
    } else {
        panic!("❌ Invalid GraphQL schema response: {:?}", response_body);
    }

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
