use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn js_write_log_and_listlogs() {
    // upsert the js_log_test script so it registers its routes
    let _ = repository::upsert_script(
        "https://example.com/js-log-test",
        include_str!("../scripts/test_scripts/js_log_test.js"),
    );

    // Start server with timeout
    let server_future = start_server_without_shutdown();
    let port = match timeout(Duration::from_secs(5), server_future).await {
        Ok(Ok(port)) => port,
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    println!("Server started on port: {}", port);

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Call the route which should call writeLog with timeout
    let log_request = client
        .get(format!("http://127.0.0.1:{}/js-log-test", port))
        .send();

    let res = match timeout(Duration::from_secs(5), log_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("Log test request failed: {:?}", e),
        Err(_) => panic!("Log test request timed out"),
    };

    let body = match timeout(Duration::from_secs(5), res.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read log test response: {:?}", e),
        Err(_) => panic!("Reading log test response timed out"),
    };

    assert!(
        body.contains("logged"),
        "Expected 'logged' in response, got: {}",
        body
    );

    // Verify the log message was written via Rust API
    let msgs = repository::fetch_log_messages("https://example.com/js-log-test");
    assert!(
        msgs.iter().any(|m| m == "js-log-test-called"),
        "Expected log entry 'js-log-test-called' not found in logs: {:?}",
        msgs
    );

    // Verify via JS-exposed route that calls listLogs()
    // Retry a few times to allow any small propagation/timing delays
    let mut found = false;
    let mut last_body = String::new();

    for i in 0..10 {
        let list_request = client
            .get(format!("http://127.0.0.1:{}/js-list", port))
            .send();

        let res2 = match timeout(Duration::from_secs(5), list_request).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                println!("attempt {}: request failed: {:?}", i, e);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                println!("attempt {}: request timed out", i);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        let body2 = match timeout(Duration::from_secs(5), res2.text()).await {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => {
                println!("attempt {}: failed to read response: {:?}", i, e);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(_) => {
                println!("attempt {}: reading response timed out", i);
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };

        println!("attempt {}: /js-list -> {}", i, body2);
        last_body = body2.clone();

        if body2.contains("js-log-test-called") {
            found = true;
            break;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    if !found {
        println!("/js-list last body: {}", last_body);
        panic!(
            "Expected log entry 'js-log-test-called' not found in /js-list output after 10 attempts"
        );
    }
}

#[tokio::test]
async fn js_list_logs_for_uri() {
    // Insert some test log messages for different URIs
    repository::insert_log_message("https://example.com/js-log-test-uri", "test-message-1");
    repository::insert_log_message("https://example.com/js-log-test-uri", "test-message-2");
    repository::insert_log_message("https://example.com/other-script", "other-message");

    // upsert the js_log_test_uri script so it registers its routes
    let _ = repository::upsert_script(
        "https://example.com/js-log-test-uri-script",
        include_str!("../scripts/test_scripts/js_log_test_uri.js"),
    );

    // Start server with timeout
    let server_future = start_server_without_shutdown();
    let port = match timeout(Duration::from_secs(5), server_future).await {
        Ok(Ok(port)) => port,
        Ok(Err(e)) => panic!("Server failed to start: {:?}", e),
        Err(_) => panic!("Server startup timed out"),
    };

    println!("Server started on port: {}", port);

    // Wait for server to be ready to accept connections
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Call the route which should call listLogsForUri
    let list_request = client
        .get(format!("http://127.0.0.1:{}/js-list-for-uri", port))
        .send();

    let res = match timeout(Duration::from_secs(5), list_request).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => panic!("List logs for URI request failed: {:?}", e),
        Err(_) => panic!("List logs for URI request timed out"),
    };

    let body = match timeout(Duration::from_secs(5), res.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => panic!("Failed to read list logs for URI response: {:?}", e),
        Err(_) => panic!("Reading list logs for URI response timed out"),
    };

    println!("Response body: {}", body);

    // Parse the JSON response
    let response: serde_json::Value =
        serde_json::from_str(&body).expect("Failed to parse JSON response");

    // Check that current logs contain the expected messages
    let current_logs = response["current"]
        .as_array()
        .expect("current should be an array");
    assert!(
        current_logs.iter().any(|v| v == "test-message-1"),
        "Expected 'test-message-1' in current logs"
    );
    assert!(
        current_logs.iter().any(|v| v == "test-message-2"),
        "Expected 'test-message-2' in current logs"
    );

    // Check that other logs contain the expected message
    let other_logs = response["other"]
        .as_array()
        .expect("other should be an array");
    assert!(
        other_logs.iter().any(|v| v == "other-message"),
        "Expected 'other-message' in other logs"
    );

    // Verify that logs are properly separated by URI
    assert!(
        !current_logs.iter().any(|v| v == "other-message"),
        "Current logs should not contain messages from other URI"
    );
    assert!(
        !other_logs.iter().any(|v| v == "test-message-1"),
        "Other logs should not contain messages from current URI"
    );
}
