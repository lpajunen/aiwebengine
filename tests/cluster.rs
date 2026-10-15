mod common;

use aiwebengine::notifications;
use chrono::Utc;
use common::AdminServer;

/// Test notification message structure includes timestamp and server_id
#[test]
fn test_notification_message_structure() {
    let msg = notifications::NotificationMessage {
        uri: "test.js".to_string(),
        action: "upserted".to_string(),
        timestamp: Utc::now().timestamp(),
        server_id: "test-server-123".to_string(),
    };

    let json = serde_json::to_value(&msg).expect("Failed to serialize");

    assert!(json.get("uri").is_some());
    assert!(json.get("action").is_some());
    assert!(json.get("timestamp").is_some());
    assert!(json.get("server_id").is_some());

    // Verify server_id is included for debugging
    assert_eq!(
        json.get("server_id").and_then(|v| v.as_str()),
        Some("test-server-123")
    );

    println!("✓ Notification message includes timestamp and server_id for debugging");
}

/// Test stream broadcast message structure includes timestamp and server_id
#[test]
fn test_stream_broadcast_message_structure() {
    let msg = notifications::StreamBroadcastMessage {
        stream_path: "/test/stream".to_string(),
        message: "test message".to_string(),
        metadata_filter: None,
        match_mode: aiwebengine::stream_registry::FilterMatchMode::Subset,
        timestamp: Utc::now().timestamp(),
        server_id: "test-server-456".to_string(),
    };

    let json = serde_json::to_value(&msg).expect("Failed to serialize");

    assert!(json.get("stream_path").is_some());
    assert!(json.get("message").is_some());
    assert!(json.get("timestamp").is_some());
    assert!(json.get("server_id").is_some());

    // Verify server_id is included for debugging
    assert_eq!(
        json.get("server_id").and_then(|v| v.as_str()),
        Some("test-server-456")
    );

    println!("✓ Stream broadcast message includes timestamp and server_id for debugging");
}

/// Test server ID generation and global storage
#[test]
fn test_server_id_generation() {
    let server_id_1 = notifications::generate_server_id();
    let server_id_2 = notifications::generate_server_id();

    // Each generation should produce unique IDs
    assert_ne!(server_id_1, server_id_2);

    // IDs should be valid UUID format (36 characters with hyphens)
    assert_eq!(server_id_1.len(), 36);
    assert_eq!(server_id_2.len(), 36);

    println!("✓ Server ID generation produces unique identifiers");
}

/// Test that stream registry provides global access
#[test]
fn test_stream_registry_global_access() {
    let registry = aiwebengine::stream_registry::get_global_registry();

    // Register a stream
    let result = registry.register_stream("/test/stream", "test_script.js", None);

    assert!(result.is_ok());

    // Verify stream is registered
    let is_registered = registry.is_stream_registered("/test/stream");
    assert!(is_registered);

    // Cleanup
    let _ = registry.unregister_stream("/test/stream");

    println!("✓ Stream registry provides global singleton access");
}

/// Test notification payload deserialization
#[test]
fn test_notification_payload_deserialization() {
    let payload = serde_json::json!({
        "uri": "test.js",
        "action": "upserted",
        "timestamp": 1234567890,
        "server_id": "instance-123"
    });

    let msg: notifications::NotificationMessage =
        serde_json::from_value(payload).expect("Failed to deserialize notification message");

    assert_eq!(msg.uri, "test.js");
    assert_eq!(msg.action, "upserted");
    assert_eq!(msg.timestamp, 1234567890);
    assert_eq!(msg.server_id, "instance-123");

    println!("✓ Notification payloads can be correctly deserialized");
}

/// Test stream broadcast payload deserialization
#[test]
fn test_stream_broadcast_payload_deserialization() {
    let payload = serde_json::json!({
        "stream_path": "/updates",
        "message": "Hello World",
        "timestamp": 1234567890,
        "server_id": "instance-456"
    });

    let msg: notifications::StreamBroadcastMessage =
        serde_json::from_value(payload).expect("Failed to deserialize stream broadcast message");

    assert_eq!(msg.stream_path, "/updates");
    assert_eq!(msg.message, "Hello World");
    assert_eq!(msg.timestamp, 1234567890);
    assert_eq!(msg.server_id, "instance-456");

    println!("✓ Stream broadcast payloads can be correctly deserialized");
}

/// The cluster diagnostics endpoint lives under `/engine` because it reports
/// internal topology (pool metrics, listener state, per-script job counts).
/// No unauthenticated `/health/cluster` path serves it, and `/engine/health/cluster`
/// rejects callers without administrator rights.
#[tokio::test(flavor = "multi_thread")]
async fn cluster_health_requires_admin_and_no_longer_serves_the_old_path() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    // Deliberately not the administrator: this is about what a caller
    // without a session is refused.
    let client = engine.anonymous().clone();
    let base = format!("http://127.0.0.1:{}", port);

    let response = client
        .get(format!("{}/engine/health/cluster", base))
        .send()
        .await
        .expect("cluster health request failed");
    assert_eq!(
        response.status(),
        403,
        "cluster diagnostics must not be readable without an admin session"
    );

    let response = client
        .get(format!("{}/health/cluster", base))
        .send()
        .await
        .expect("legacy cluster health request failed");
    assert_ne!(
        response.status(),
        200,
        "the unauthenticated /health/cluster path must not serve diagnostics"
    );

    // The plain liveness probe stays public and unauthenticated.
    let response = client
        .get(format!("{}/health", base))
        .send()
        .await
        .expect("health request failed");
    assert_eq!(response.status(), 200);

    engine.shutdown().await;
}
