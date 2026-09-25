//! Core Repository Tests
//!
//! This module contains tests for core repository operations including:
//! - Script lifecycle (create, read, update, delete)
//! - Asset management
//! - Log message storage and pruning

mod common;

use aiwebengine::log_retention::LogRetention;
use aiwebengine::repository;
use common::{TestContext, wait_for_server};

// ============================================================================
// Script Repository Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_dynamic_script_lifecycle() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    let context = TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

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
async fn test_upsert_overwrites_existing_script() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    let context = TestContext::new();
    let port = context
        .start_server()
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
async fn test_insert_and_list_log_messages() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    let context = TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let test_uri = "test_insert_and_list_log_messages";

    // Clear any existing logs for this URI
    let _ = repository::clear_log_messages(test_uri);

    // Record starting length so test is robust to previous state
    let start = repository::fetch_log_messages(test_uri).len();

    repository::insert_log_message(test_uri, "log-one", "INFO");
    repository::insert_log_message(test_uri, "log-two", "INFO");

    let msgs = repository::fetch_log_messages(test_uri);
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
async fn test_prune_keeps_newest_per_script() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    let context = TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let test_uri = "test_prune_keeps_newest_per_script";

    // Clear any existing logs for this URI
    let _ = repository::clear_log_messages(test_uri);

    // Insert 25 distinct messages
    for i in 0..25 {
        repository::insert_log_message(test_uri, &format!("prune-test-{}", i), "INFO");
    }

    // Age bound disabled, so only the count decides what survives.
    let retention = LogRetention {
        keep_per_script: 20,
        keep_hours: 0,
    };
    repository::prune_log_messages_async(retention)
        .await
        .expect("prune should succeed");

    let msgs = repository::fetch_log_messages(test_uri);
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

/// The age bound is what keeps a script that logged once a year ago from
/// sitting on its full quota forever, so it has to delete lines the count
/// would have kept.
#[tokio::test(flavor = "multi_thread")]
async fn test_prune_drops_lines_past_the_retention_window() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    let context = TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let test_uri = "test_prune_drops_lines_past_the_retention_window";
    let _ = repository::clear_log_messages(test_uri);

    repository::insert_log_message(test_uri, "stale-line", "INFO");
    repository::insert_log_message(test_uri, "fresh-line", "INFO");

    // Backdate the first line past the window. Only the age bound can remove
    // it: two lines are well inside any per-script count.
    let db = repository::get_db_pool().expect("database pool");
    sqlx::query("UPDATE logs SET created_at = now() - interval '48 hours' WHERE script_uri = $1 AND message = $2")
        .bind(test_uri)
        .bind("stale-line")
        .execute(db.pool())
        .await
        .expect("backdating the stale line should succeed");

    let retention = LogRetention {
        keep_per_script: 1000,
        keep_hours: 24,
    };
    repository::prune_log_messages_async(retention)
        .await
        .expect("prune should succeed");

    let messages: Vec<String> = repository::fetch_log_messages(test_uri)
        .into_iter()
        .map(|m| m.message)
        .collect();
    assert!(
        !messages.iter().any(|m| m.contains("stale-line")),
        "a line older than the window should be pruned even well inside the count: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("fresh-line")),
        "a line inside the window should survive: {messages:?}"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Asset Repository Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_management() {
    if std::env::var("DATABASE_URL").is_err() {
        return;
    }
    let context = TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    // Engine assets are served from compiled-in static fallbacks under the
    // legacy core URI (no database rows required)
    let asset = repository::fetch_asset("https://example.com/core", "logo.svg");
    assert!(asset.is_some());
    let asset = asset.unwrap();
    assert_eq!(asset.uri, "logo.svg");
    assert_eq!(asset.mimetype, "image/svg+xml");
    assert!(!asset.content.is_empty());

    // Test upsert and fetch dynamic asset; assets require an owning script
    let owner_uri = "https://example.com/asset-mgmt-test";
    let _ = repository::upsert_script(owner_uri, "// asset owner test script");

    let test_content = b"test content".to_vec();
    let now = std::time::SystemTime::now();
    let test_asset = repository::Asset {
        uri: "test.txt".to_string(),
        name: Some("Test File".to_string()),
        mimetype: "text/plain".to_string(),
        content: test_content.clone(),
        created_at: now,
        updated_at: now,
        script_uri: owner_uri.to_string(),
    };
    let _ = repository::upsert_asset(test_asset);

    // Test listing assets
    let assets = repository::fetch_assets(owner_uri);
    assert!(assets.contains_key("test.txt"));

    let fetched = repository::fetch_asset(owner_uri, "test.txt");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.uri, "test.txt");
    assert_eq!(fetched.mimetype, "text/plain");
    assert_eq!(fetched.content, test_content);

    // Test delete
    let deleted = repository::delete_asset(owner_uri, "test.txt");
    assert!(deleted);

    // Verify it's gone
    let fetched_after_delete = repository::fetch_asset(owner_uri, "test.txt");
    assert!(fetched_after_delete.is_none());

    let _ = repository::delete_script(owner_uri);

    context.cleanup().await.expect("Failed to cleanup");
}
