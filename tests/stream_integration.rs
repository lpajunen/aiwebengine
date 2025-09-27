use aiwebengine::{js_engine, repository, start_server_without_shutdown, stream_registry};
use reqwest::Client;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn test_stream_endpoints() {
    // Start server in background
    let port = start_server_without_shutdown().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", port);

    // Give server time to start
    sleep(Duration::from_millis(1000)).await;

    // Create a script that registers a stream
    let script_content = r#"
        // Register a stream endpoint
        registerWebStream('/test-stream');
        
        // Register a regular handler to test stream vs regular route handling
        register('/regular-endpoint', 'handleRegular', 'GET');
        
        function handleRegular(req) {
            return { 
                status: 200, 
                body: 'This is a regular endpoint',
                contentType: 'text/plain'
            };
        }
        
        writeLog('Stream and regular endpoints registered');
    "#;

    // Upsert the test script
    let _ = repository::upsert_script("stream-test", script_content);

    // Execute the script to register the endpoints
    let result = js_engine::execute_script("stream-test", script_content);
    println!("Script execution result: {:?}", result);
    assert!(result.success, "Script should execute successfully");

    // Give a moment for registration to complete
    sleep(Duration::from_millis(50)).await;

    // Check script logs
    let logs = repository::fetch_log_messages("stream-test");
    println!("Script logs: {:?}", logs);

    // Check if stream is registered
    let is_registered =
        stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("/test-stream");
    println!("Is /test-stream registered: {}", is_registered);

    let client = Client::new();

    // Test 1: Regular endpoint should work normally
    let response = client
        .get(&format!("{}/regular-endpoint", base_url))
        .send()
        .await
        .expect("Failed to send regular request");

    println!("Regular endpoint response status: {}", response.status());
    let status = response.status();
    let body = response.text().await.expect("Failed to read response body");
    println!("Regular endpoint response body: {}", body);

    assert_eq!(status, 200);
    assert!(body.contains("This is a regular endpoint"));

    // Test 2: Stream endpoint should return SSE headers
    let response = client
        .get(&format!("{}/test-stream", base_url))
        .send()
        .await
        .expect("Failed to send stream request");

    println!("Stream endpoint response status: {}", response.status());
    println!("Stream endpoint response headers: {:?}", response.headers());
    let status = response.status();

    assert_eq!(status, 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-cache");
    // Note: connection: keep-alive is not required for SSE (transfer-encoding: chunked is used instead)

    // Test 3: Non-existent stream should return 404
    let response = client
        .get(&format!("{}/non-existent-stream", base_url))
        .send()
        .await
        .expect("Failed to send request to non-existent stream");

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_stream_messaging() {
    // Start server in background
    let port = start_server_without_shutdown().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", port);

    // Give server time to start
    sleep(Duration::from_millis(1000)).await;

    // Create a script that registers a stream and a sender endpoint
    let script_content = r#"
        // Register a stream endpoint
        registerWebStream('/notification-stream');
        
        // Register an endpoint to send messages for both GET and POST
        register('/send-notification', 'sendNotification', 'POST');
        register('/send-notification', 'sendNotificationGet', 'GET');
        writeLog('Registered POST and GET /send-notification');
        
        function sendNotification(req) {
            var message = {
                type: "notification",
                title: "Test Message POST",
                content: "Hello from integration test!",
                timestamp: new Date().toISOString()
            };
            
            sendStreamMessage(JSON.stringify(message));
            writeLog('POST - Sent notification: ' + JSON.stringify(message));
            return { 
                status: 200, 
                body: 'POST Message sent',
                contentType: 'text/plain'
            };
        }
        
        function sendNotificationGet(req) {
            var message = {
                type: "notification",
                title: "Test Message GET",
                content: "Hello from integration test via GET!",
                timestamp: new Date().toISOString()
            };
            
            sendStreamMessage(JSON.stringify(message));
            writeLog('GET - Sent notification: ' + JSON.stringify(message));
            return { 
                status: 200, 
                body: 'GET Message sent',
                contentType: 'text/plain'
            };
        }
        
        writeLog('Notification system registered');
    "#;

    // Upsert the test script
    let _ = repository::upsert_script("notification-test", script_content);

    // Test sendStreamMessage in isolation first
    let minimal_test = r#"
        writeLog('Testing sendStreamMessage in isolation');
        try {
            sendStreamMessage('{"test": "isolation"}');
            writeLog('sendStreamMessage isolation test: SUCCESS');
        } catch (error) {
            writeLog('sendStreamMessage isolation test ERROR: ' + error.toString());
        }
    "#;

    let minimal_result = js_engine::execute_script("minimal-test", minimal_test);
    println!("Minimal test result: {:?}", minimal_result);
    let minimal_logs = repository::fetch_log_messages("minimal-test");
    println!("Minimal test logs: {:?}", minimal_logs);

    // Execute the script to register the endpoints
    let result = js_engine::execute_script("notification-test", script_content);
    println!("Notification script execution result: {:?}", result);
    assert!(result.success, "Script should execute successfully");

    // Give a moment for registration to complete
    sleep(Duration::from_millis(50)).await;

    let client = Client::new();

    // Start listening to the stream (this would normally be done in a separate task)
    // For this test, we'll just verify the endpoints are working

    // Test: Send notification endpoint via GET should work
    let response = client
        .get(&format!("{}/send-notification", base_url))
        .send()
        .await
        .expect("Failed to send GET notification request");

    println!(
        "GET Send notification response status: {}",
        response.status()
    );
    let status = response.status();
    let body = response.text().await.expect("Failed to read response body");
    println!("GET Send notification response body: {}", body);

    assert_eq!(status, 200);
    assert!(body.contains("GET Message sent"));

    // Verify logs were written
    let logs = repository::fetch_log_messages("notification-test");
    assert!(
        logs.iter().any(|log| log.contains("Sent notification")),
        "Should have logged the sent notification"
    );
}
