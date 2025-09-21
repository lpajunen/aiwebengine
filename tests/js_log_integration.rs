use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn js_write_log_and_listlogs() {
    // upsert the js_log_test script so it registers its routes
    repository::upsert_script(
        "https://example.com/js-log-test",
        include_str!("../scripts/js_log_test.js"),
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
    let msgs = repository::fetch_log_messages();
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
