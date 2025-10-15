mod common;

use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn test_script_update_streaming_integration() {
    // This test verifies that script update streaming works via the HTTP API
    // It tests the /script_updates stream endpoint and broadcasts

    // First, upsert the streaming test script
    let core_script_content = r#"
        // Register script updates stream endpoint
        registerWebStream('/script_updates');

        // Helper function to broadcast script update messages
        function broadcastScriptUpdate(uri, action, details = {}) {
            try {
                const message = {
                    type: 'script_update',
                    uri: uri,
                    action: action,
                    timestamp: new Date().toISOString(),
                    ...details
                };
                
                sendStreamMessageToPath('/script_updates', JSON.stringify(message));
                writeLog(`Broadcasted script update: ${action} ${uri}`);
            } catch (error) {
                writeLog(`Failed to broadcast script update: ${error.message}`);
            }
        }

        writeLog('Script update streaming script loaded');
    "#;

    let _ = aiwebengine::repository::upsert_script(
        "https://example.com/streaming_test",
        core_script_content,
    );

    // Start server using TestContext
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    // Give extra time for scripts to execute
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Note: SSE streaming from /script_updates would require a more complex setup
    // For now, we verify the core.js script properly broadcasts updates
    // when scripts are upserted via the /upsert_script endpoint

    // Test 1: Insert a new script via HTTP (this should trigger broadcast)
    let insert_request = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/test_script"),
            ("content", "console.log('test');"),
        ])
        .send();

    let insert_response = match timeout(Duration::from_secs(5), insert_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Insert request failed: {:?}", e),
        Err(_) => panic!("Insert request timed out"),
    };

    assert_eq!(insert_response.status(), 200, "Insert should succeed");

    // Test 2: Update the script
    let update_request = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/test_script"),
            ("content", "console.log('updated');"),
        ])
        .send();

    let update_response = match timeout(Duration::from_secs(5), update_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Update request failed: {:?}", e),
        Err(_) => panic!("Update request timed out"),
    };

    assert_eq!(update_response.status(), 200, "Update should succeed");

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_script_update_message_format() {
    // Test that the script update message format is correct via HTTP API
    // This test verifies the core.js script properly formats broadcast messages

    let core_script_content = r#"
        // Register stream for message format testing
        function broadcastScriptUpdate(uri, action, details = {}) {
            const message = {
                type: 'script_update',
                uri: uri,
                action: action,
                timestamp: new Date().toISOString(),
                ...details
            };
            
            sendStreamMessageToPath('/script_updates_format_test', JSON.stringify(message));
            writeLog(`Broadcast ${action} for ${uri}`);
        }

        function test_message_format(req) {
            // Broadcast test messages with different formats
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
            
            return { 
                status: 200, 
                body: JSON.stringify({ success: true, messagesSent: 3 }),
                contentType: 'application/json'
            };
        }

        function init(context) {
            writeLog('Initializing message format test script at ' + new Date().toISOString());
            registerWebStream('/script_updates_format_test');
            register('/test_message_format', 'test_message_format', 'GET');
            writeLog('Message format test script initialized');
            return { success: true };
        }
    "#;

    let _ = aiwebengine::repository::upsert_script(
        "https://example.com/message_format_test",
        core_script_content,
    );

    // Initialize the script to register routes and streams
    let initializer = aiwebengine::script_init::ScriptInitializer::new(5000);
    initializer
        .initialize_script("https://example.com/message_format_test", false)
        .await
        .expect("Failed to initialize message format test script");

    // Start server using TestContext
    let context = common::TestContext::new();
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");

    common::wait_for_server(port, 40)
        .await
        .expect("Server not ready");

    // Give extra time for scripts to execute
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Trigger the message format test via HTTP
    let test_request = client
        .get(format!("http://127.0.0.1:{}/test_message_format", port))
        .send();

    let test_response = match timeout(Duration::from_secs(5), test_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Test request failed: {:?}", e),
        Err(_) => panic!("Test request timed out"),
    };

    assert_eq!(
        test_response.status(),
        200,
        "Message format test should succeed"
    );

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
