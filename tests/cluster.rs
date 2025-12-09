use aiwebengine::{notifications, scheduler};
use chrono::Utc;

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

/// Test scheduler job key generation is deterministic
#[test]
fn test_scheduler_job_key_deterministic() {
    let scheduler = scheduler::Scheduler::new();

    // Register multiple jobs with different keys
    let run_time = Utc::now() + chrono::Duration::minutes(5);

    let job1 = scheduler
        .register_one_off("script1.js", "handler1", Some("job1".to_string()), run_time)
        .expect("Failed to register job1");

    let job2 = scheduler
        .register_one_off("script1.js", "handler2", Some("job2".to_string()), run_time)
        .expect("Failed to register job2");

    let job3 = scheduler
        .register_one_off("script2.js", "handler1", Some("job1".to_string()), run_time)
        .expect("Failed to register job3");

    // Each job should have a unique ID but deterministic key
    assert_ne!(job1.id, job2.id);
    assert_ne!(job1.id, job3.id);
    assert_ne!(job2.id, job3.id);

    assert_eq!(job1.key, "job1");
    assert_eq!(job2.key, "job2");
    assert_eq!(job3.key, "job1"); // Same key but different script

    println!("✓ Scheduler jobs have deterministic keys for advisory locking");
}

/// Test scheduler can get job counts per script
#[test]
fn test_scheduler_job_counts() {
    let scheduler = scheduler::Scheduler::new();
    let run_time = Utc::now() + chrono::Duration::minutes(5);

    // Register jobs for multiple scripts
    scheduler
        .register_one_off("script1.js", "handler1", Some("job1".to_string()), run_time)
        .expect("Failed to register job on script1");
    scheduler
        .register_one_off("script1.js", "handler2", Some("job2".to_string()), run_time)
        .expect("Failed to register second job on script1");
    scheduler
        .register_one_off("script2.js", "handler1", Some("job1".to_string()), run_time)
        .expect("Failed to register job on script2");

    let counts = scheduler.get_job_counts();

    assert_eq!(counts.get("script1.js"), Some(&2));
    assert_eq!(counts.get("script2.js"), Some(&1));

    let total: usize = counts.values().sum();
    assert_eq!(total, 3);

    println!("✓ Scheduler provides job counts for health monitoring");
}

/// Test scheduler clearing jobs for a script
#[test]
fn test_scheduler_clear_script_jobs() {
    let scheduler = scheduler::Scheduler::new();
    let run_time = Utc::now() + chrono::Duration::minutes(5);

    // Register multiple jobs
    scheduler
        .register_one_off("script1.js", "handler1", Some("job1".to_string()), run_time)
        .expect("Failed to register job1");
    scheduler
        .register_one_off("script1.js", "handler2", Some("job2".to_string()), run_time)
        .expect("Failed to register job2");
    scheduler
        .register_one_off("script2.js", "handler1", Some("job1".to_string()), run_time)
        .expect("Failed to register job on script2");

    // Clear jobs for script1
    let removed = scheduler.clear_script("script1.js");
    assert_eq!(removed, 2);

    let counts = scheduler.get_job_counts();
    assert_eq!(counts.get("script1.js"), None);
    assert_eq!(counts.get("script2.js"), Some(&1));

    println!("✓ Scheduler can clear all jobs for a specific script");
}

/// Test recurring job interval validation
#[test]
fn test_recurring_job_interval_validation() {
    let scheduler = scheduler::Scheduler::new();

    // Intervals less than 1 minute should fail
    let result = scheduler.register_recurring(
        "script.js",
        "handler",
        Some("job".to_string()),
        chrono::Duration::seconds(30), // 30 seconds - too short
        None,
    );

    assert!(result.is_err());

    // 1 minute or more should succeed
    let result = scheduler.register_recurring(
        "script.js",
        "handler",
        Some("job".to_string()),
        chrono::Duration::minutes(1),
        None,
    );

    assert!(result.is_ok());

    println!("✓ Scheduler validates recurring job intervals (minimum 1 minute)");
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
