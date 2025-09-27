use aiwebengine::{repository_safe, stream_registry::GLOBAL_STREAM_REGISTRY};
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[tokio::test]
async fn test_script_update_streaming_integration() {
    // Initialize logging for the test
    tracing_subscriber::fmt::init();

    info!("Starting script update streaming integration test");

    // Test the script update streaming functionality by:
    // 1. Registering the /script_updates stream
    // 2. Creating a script that triggers updates
    // 3. Verifying that update messages are broadcast correctly

    // First, let's create the core.js script that includes the streaming functionality
    let core_script_content = r#"
        // Register script updates stream endpoint for test 1
        registerWebStream('/script_updates_test1');

        // Helper function to broadcast script update messages
        function broadcastScriptUpdate(uri, action, details = {}) {
            try {
                const message = {
                    type: 'script_update',
                    uri: uri,
                    action: action, // 'inserted', 'updated', 'removed'
                    timestamp: new Date().toISOString(),
                    ...details
                };
                
                sendStreamMessageToPath('/script_updates_test1', JSON.stringify(message));
                writeLog(`Broadcasted script update: ${action} ${uri}`);
            } catch (error) {
                writeLog(`Failed to broadcast script update: ${error.message}`);
            }
        }

        // Test upsert function
        function test_upsert_handler(req) {
            try {
                const uri = req.query?.uri || 'test_script.js';
                const content = req.query?.content || 'console.log("Hello World");';
                
                // Check if script already exists to determine action
                const existingScript = getScript(uri);
                const action = existingScript ? 'updated' : 'inserted';
                
                // Call the upsertScript function
                upsertScript(uri, content);
                
                // Broadcast the script update
                broadcastScriptUpdate(uri, action, {
                    contentLength: content.length,
                    previousExists: !!existingScript
                });
                
                return {
                    status: 200,
                    body: JSON.stringify({
                        success: true,
                        action: action,
                        uri: uri,
                        contentLength: content.length
                    }),
                    contentType: 'application/json'
                };
            } catch (error) {
                return {
                    status: 500,
                    body: JSON.stringify({ error: error.message }),
                    contentType: 'application/json'
                };
            }
        }

        register('/test_upsert_script', 'test_upsert_handler', 'GET');
        
        writeLog('Script update streaming test script loaded');
    "#;

    // Store the core script in the repository and execute it to register the stream and endpoints
    aiwebengine::repository_safe::upsert_script("test_streaming_core.js", core_script_content);
    let result =
        aiwebengine::js_engine::execute_script("test_streaming_core.js", core_script_content);
    assert!(
        result.success,
        "Core script execution failed: {:?}",
        result.error
    );

    // Verify the stream was registered
    assert!(
        GLOBAL_STREAM_REGISTRY.is_stream_registered("/script_updates_test1"),
        "Script updates stream should be registered"
    );

    // Create a connection to the stream
    let connection = aiwebengine::stream_manager::StreamConnectionManager::new()
        .create_connection("/script_updates_test1", None)
        .await
        .expect("Failed to create stream connection");

    let mut receiver = connection.receiver;
    let connection_id = connection.connection_id;

    info!(
        "Created stream connection {} for /script_updates",
        connection_id
    );

    // Give the system a moment to establish the connection
    sleep(Duration::from_millis(100)).await;

    // Now test script operations that should trigger streaming messages

    // Test 1: Insert a new script
    info!("Testing script insertion...");
    let mut query_params = std::collections::HashMap::new();
    query_params.insert("uri".to_string(), "new_test.js".to_string());
    query_params.insert(
        "content".to_string(),
        "console.log('new script');".to_string(),
    );

    let insert_result = aiwebengine::js_engine::execute_script_for_request(
        "test_streaming_core.js",
        "test_upsert_handler",
        "/test_upsert_script",
        "GET",
        Some(&query_params),
        None,
        None,
    );

    assert!(
        insert_result.is_ok(),
        "Script insert test failed: {:?}",
        insert_result
    );

    // Wait for and verify the insert message
    match tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await {
        Ok(Ok(message)) => {
            info!("Received insert message: {}", message);
            let parsed: serde_json::Value =
                serde_json::from_str(&message).expect("Failed to parse insert message as JSON");

            assert_eq!(parsed["type"], "script_update");
            assert_eq!(parsed["action"], "inserted");
            assert_eq!(parsed["uri"], "new_test.js");
            assert!(parsed["contentLength"].as_u64().unwrap() > 0);
        }
        Ok(Err(e)) => panic!("Receiver error for insert: {}", e),
        Err(_) => panic!("Timeout waiting for insert message"),
    }

    // Test 2: Update the existing script
    info!("Testing script update...");
    let mut update_query_params = std::collections::HashMap::new();
    update_query_params.insert("uri".to_string(), "new_test.js".to_string());
    update_query_params.insert(
        "content".to_string(),
        "console.log('updated script content');".to_string(),
    );

    let update_result = aiwebengine::js_engine::execute_script_for_request(
        "test_streaming_core.js",
        "test_upsert_handler",
        "/test_upsert_script",
        "GET",
        Some(&update_query_params),
        None,
        None,
    );

    assert!(
        update_result.is_ok(),
        "Script update test failed: {:?}",
        update_result
    );

    // Wait for and verify the update message
    match tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await {
        Ok(Ok(message)) => {
            info!("Received update message: {}", message);
            let parsed: serde_json::Value =
                serde_json::from_str(&message).expect("Failed to parse update message as JSON");

            assert_eq!(parsed["type"], "script_update");
            assert_eq!(parsed["action"], "updated");
            assert_eq!(parsed["uri"], "new_test.js");
            assert_eq!(parsed["previousExists"], true);
        }
        Ok(Err(e)) => panic!("Receiver error for update: {}", e),
        Err(_) => panic!("Timeout waiting for update message"),
    }

    // Clean up the test connection
    GLOBAL_STREAM_REGISTRY
        .remove_connection("/script_updates", &connection_id)
        .expect("Failed to remove test connection");

    info!("Script update streaming integration test completed successfully");
}

#[tokio::test]
async fn test_script_update_message_format() {
    // Test that the script update message format is correct and contains all expected fields

    let core_script_content = r#"
        registerWebStream('/script_updates_test2');

        function broadcastScriptUpdate(uri, action, details = {}) {
            const message = {
                type: 'script_update',
                uri: uri,
                action: action,
                timestamp: new Date().toISOString(),
                ...details
            };
            
            sendStreamMessageToPath('/script_updates_test2', JSON.stringify(message));
        }

        function test_message_format(req) {
            // Test different message formats
            broadcastScriptUpdate('test1.js', 'inserted', {
                contentLength: 100,
                previousExists: false
            });
            
            broadcastScriptUpdate('test2.js', 'updated', {
                contentLength: 150,
                previousExists: true,
                via: 'rest'
            });
            
            broadcastScriptUpdate('test3.js', 'removed', {
                via: 'graphql'
            });
            
            return { status: 200, body: 'Messages sent' };
        }

        register('/test_message_format', 'test_message_format', 'GET');
    "#;

    // Store the script in the repository and execute it
    aiwebengine::repository_safe::upsert_script("test_message_format.js", core_script_content);
    let result =
        aiwebengine::js_engine::execute_script("test_message_format.js", core_script_content);
    assert!(
        result.success,
        "Script execution failed: {:?}",
        result.error
    );

    // Create connection and trigger the test
    let connection = aiwebengine::stream_manager::StreamConnectionManager::new()
        .create_connection("/script_updates_test2", None)
        .await
        .expect("Failed to create connection");

    let mut receiver = connection.receiver;
    let connection_id = connection.connection_id;

    // Trigger the message format test
    let test_result = aiwebengine::js_engine::execute_script_for_request(
        "test_message_format.js",
        "test_message_format",
        "/test_message_format",
        "GET",
        None,
        None,
        None,
    );

    assert!(
        test_result.is_ok(),
        "Message format test failed: {:?}",
        test_result
    );

    // Verify all three messages
    let messages = vec!["inserted", "updated", "removed"];

    for expected_action in messages {
        match tokio::time::timeout(Duration::from_secs(1), receiver.recv()).await {
            Ok(Ok(message)) => {
                info!("Received {} message: {}", expected_action, message);
                let parsed: serde_json::Value =
                    serde_json::from_str(&message).expect("Failed to parse message as JSON");

                // Verify required fields
                assert_eq!(parsed["type"], "script_update");
                assert_eq!(parsed["action"], expected_action);
                assert!(parsed["uri"].as_str().unwrap().starts_with("test"));
                assert!(parsed["timestamp"].as_str().is_some());

                // Verify action-specific fields
                match expected_action {
                    "inserted" => {
                        assert_eq!(parsed["contentLength"], 100);
                        assert_eq!(parsed["previousExists"], false);
                    }
                    "updated" => {
                        assert_eq!(parsed["contentLength"], 150);
                        assert_eq!(parsed["previousExists"], true);
                        assert_eq!(parsed["via"], "rest");
                    }
                    "removed" => {
                        assert_eq!(parsed["via"], "graphql");
                    }
                    _ => {}
                }
            }
            Ok(Err(e)) => panic!("Receiver error for {}: {}", expected_action, e),
            Err(_) => panic!("Timeout waiting for {} message", expected_action),
        }
    }

    // Clean up
    GLOBAL_STREAM_REGISTRY
        .remove_connection("/script_updates", &connection_id)
        .expect("Failed to remove connection");

    info!("Script update message format test completed successfully");
}
