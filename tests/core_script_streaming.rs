use aiwebengine::{js_engine, repository, script_init, stream_registry::GLOBAL_STREAM_REGISTRY};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::info;

#[tokio::test]
async fn test_core_js_script_streaming() {
    // Test the real core.js script with streaming functionality

    // First, read the actual core.js file to use for testing
    let core_js_path = "/Users/lassepajunen/work/aiwebengine/scripts/feature_scripts/core.js";
    let core_script_content =
        std::fs::read_to_string(core_js_path).expect("Failed to read core.js file");

    info!("Testing core.js script streaming functionality");

    // Store and execute the core script
    let _ = repository::upsert_script("core.js", &core_script_content);
    let result = js_engine::execute_script("core.js", &core_script_content);
    assert!(
        result.success,
        "Core script execution failed: {:?}",
        result.error
    );

    // Initialize the script to register streams and routes
    let init_context = script_init::InitContext::new("core.js".to_string(), false);
    let registrations =
        js_engine::call_init_if_exists("core.js", &core_script_content, init_context)
            .expect("Failed to call init on core.js");
    assert!(
        registrations.is_some(),
        "Core.js should have an init() function"
    );

    // Verify the /script_updates stream was registered
    assert!(
        GLOBAL_STREAM_REGISTRY.is_stream_registered("/script_updates"),
        "Script updates stream should be registered by core.js"
    );

    info!("Script updates stream successfully registered");

    // Create a connection to the stream
    let connection = aiwebengine::stream_manager::StreamConnectionManager::new()
        .create_connection("/script_updates", None)
        .await
        .expect("Failed to create stream connection");

    let mut receiver = connection.receiver;
    let connection_id = connection.connection_id;

    info!(
        "Created stream connection {} for /script_updates",
        connection_id
    );

    // Give the system a moment to establish the connection
    sleep(Duration::from_millis(200)).await;

    // Now test the upsert_script endpoint using core.js
    info!("Testing script upsert through core.js /upsert_script endpoint...");

    let mut form_data = std::collections::HashMap::new();
    form_data.insert("uri".to_string(), "test_streaming.js".to_string());
    form_data.insert(
        "content".to_string(),
        "console.log('test streaming script');".to_string(),
    );

    let upsert_result = js_engine::execute_script_for_request(
        "core.js",
        "upsert_script_handler",
        "/upsert_script",
        "POST",
        None,
        Some(&form_data),
        None,
    );

    info!("Upsert result: {:?}", upsert_result);
    assert!(
        upsert_result.is_ok(),
        "Script upsert failed: {:?}",
        upsert_result
    );

    // Wait for the streaming message
    match timeout(Duration::from_secs(3), receiver.recv()).await {
        Ok(Ok(message)) => {
            info!("Received script update message: {}", message);
            let parsed: serde_json::Value =
                serde_json::from_str(&message).expect("Failed to parse message as JSON");

            assert_eq!(parsed["type"], "script_update");
            assert_eq!(parsed["uri"], "test_streaming.js");
            assert!(
                parsed["action"].as_str().unwrap() == "inserted"
                    || parsed["action"].as_str().unwrap() == "updated"
            );
            assert!(parsed["contentLength"].as_u64().unwrap() > 0);
            assert!(parsed["timestamp"].as_str().is_some());

            info!("Script update message validated successfully!");
        }
        Ok(Err(e)) => panic!("Receiver error: {}", e),
        Err(_) => {
            // Let's check if the message was sent but we missed it
            info!("Timeout waiting for message. Let's check connection stats...");

            let stats = GLOBAL_STREAM_REGISTRY
                .get_stream_stats()
                .expect("Failed to get stream stats");
            info!("Stream stats: {:?}", stats);

            panic!("Timeout waiting for script update message");
        }
    }

    // Test deletion as well
    info!("Testing script deletion through core.js /delete_script endpoint...");

    let mut delete_form_data = std::collections::HashMap::new();
    delete_form_data.insert("uri".to_string(), "test_streaming.js".to_string());

    let delete_result = js_engine::execute_script_for_request(
        "core.js",
        "delete_script_handler",
        "/delete_script",
        "POST",
        None,
        Some(&delete_form_data),
        None,
    );

    assert!(
        delete_result.is_ok(),
        "Script delete failed: {:?}",
        delete_result
    );

    // Wait for the deletion message
    match timeout(Duration::from_secs(2), receiver.recv()).await {
        Ok(Ok(message)) => {
            info!("Received script deletion message: {}", message);
            let parsed: serde_json::Value =
                serde_json::from_str(&message).expect("Failed to parse deletion message as JSON");

            assert_eq!(parsed["type"], "script_update");
            assert_eq!(parsed["uri"], "test_streaming.js");
            assert_eq!(parsed["action"], "removed");
            assert!(parsed["timestamp"].as_str().is_some());
        }
        Ok(Err(e)) => panic!("Receiver error for deletion: {}", e),
        Err(_) => panic!("Timeout waiting for deletion message"),
    }

    // Clean up
    GLOBAL_STREAM_REGISTRY
        .remove_connection("/script_updates", &connection_id)
        .expect("Failed to remove test connection");

    info!("Core.js script streaming test completed successfully!");
}

#[tokio::test]
async fn test_script_stream_health_and_stats() {
    // Test that we can get health and stats for the script update stream

    let core_js_path = "/Users/lassepajunen/work/aiwebengine/scripts/feature_scripts/core.js";
    let core_script_content =
        std::fs::read_to_string(core_js_path).expect("Failed to read core.js file");

    // Execute core script to register the stream
    let _ = repository::upsert_script("core_health.js", &core_script_content);
    let result = js_engine::execute_script("core_health.js", &core_script_content);
    assert!(result.success, "Core script execution failed");

    // Initialize the script to register streams
    let init_context = script_init::InitContext::new("core_health.js".to_string(), false);
    let registrations =
        js_engine::call_init_if_exists("core_health.js", &core_script_content, init_context)
            .expect("Failed to call init on core_health.js");
    assert!(
        registrations.is_some(),
        "Core.js should have an init() function"
    );

    // Test stream health and statistics
    let health = GLOBAL_STREAM_REGISTRY
        .get_health_status()
        .expect("Failed to get health status");

    info!("Stream health: {}", health);

    // Should have at least one stream registered
    assert!(health["total_streams"].as_u64().unwrap() >= 1);

    let stats = GLOBAL_STREAM_REGISTRY
        .get_stream_stats()
        .expect("Failed to get stream stats");

    info!("Stream stats: {:?}", stats);

    // Should have script_updates stream
    assert!(stats.contains_key("/script_updates"));

    info!("Script stream health and stats test completed successfully!");
}
