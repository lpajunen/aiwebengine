use aiwebengine::repository;
use aiwebengine::{config, start_server_without_shutdown_with_config};
use std::time::Duration;

#[tokio::test]
async fn js_script_mgmt_functions_work() {
    // Create custom config with unique port
    let mut test_config = config::Config::from_env();
    test_config.port = 4001;

    // upsert the management test script so it registers /js-mgmt-check and the upsert logic
    repository::upsert_script(
        "https://example.com/js-mgmt-test",
        include_str!("../scripts/js_script_mgmt_test.js"),
    );

    let port = start_server_without_shutdown_with_config(test_config).await.expect("server failed to start");

    tokio::time::sleep(Duration::from_millis(500)).await;

    let res = reqwest::get(format!("http://127.0.0.1:{}/js-mgmt-check", port))
        .await
        .expect("request failed");
    let body = res.text().await.expect("read body");

    let v: serde_json::Value = serde_json::from_str(&body).expect("expected json");
    let obj = v.as_object().expect("expected object");

    // got may be null or a string
    assert!(obj.contains_key("got"));
    assert!(obj.contains_key("list"));
    assert!(obj.contains_key("deleted_before"));
    assert!(obj.contains_key("deleted"));
    assert!(obj.contains_key("after"));

    // list should be an array containing some known URIs
    let list = obj.get("list").unwrap().as_array().expect("list array");
    assert!(list.iter().any(|v| {
        v.as_str()
            .map(|s| s.contains("example.com/core"))
            .unwrap_or(false)
    }));

    // verify the upserted script was deleted via deleteScript and is no longer callable
    let res2 = reqwest::get(format!("http://127.0.0.1:{}/from-js", port))
        .await
        .expect("request failed");
    let status = res2.status();
    // The endpoint should now return 404 since the script was deleted
    assert_eq!(status, 404);
}

#[tokio::test]
async fn test_upsert_script_endpoint() {
    // Create custom config with unique port
    let mut test_config = config::Config::from_env();
    test_config.port = 4002;

    // Start server in background task
    let port = start_server_without_shutdown_with_config(test_config).await.expect("server failed to start");
    let _server_handle = tokio::spawn(async move {
        // Server is already started, just keep it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Test the upsert_script endpoint
    let test_script_content = r#"
function test_endpoint_handler(req) {
    return { status: 200, body: 'Test endpoint works!' };
}
register('/test-endpoint', 'test_endpoint_handler', 'GET');
"#;

    let response = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/test-endpoint-script"),
            ("content", test_script_content),
        ])
        .send()
        .await
        .expect("POST request to /upsert_script failed");

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response
        .json()
        .await
        .expect("Failed to parse JSON response");
    assert_eq!(body["success"], true);
    assert_eq!(body["uri"], "https://example.com/test-endpoint-script");
    assert!(body["contentLength"].as_u64().unwrap() > 0);

    // Verify the script was actually upserted by calling the new endpoint
    tokio::time::sleep(Duration::from_millis(100)).await; // Give time for script to be processed

    let test_response = client
        .get(format!("http://127.0.0.1:{}/test-endpoint", port))
        .send()
        .await
        .expect("GET request to test endpoint failed");

    assert_eq!(test_response.status(), 200);
    let test_body = test_response
        .text()
        .await
        .expect("Failed to read test response");
    assert_eq!(test_body, "Test endpoint works!");
}

#[tokio::test]
async fn test_delete_script_endpoint() {
    // Create custom config with unique port
    let mut test_config = config::Config::from_env();
    test_config.port = 4003;

    // Start server in background task
    let port = start_server_without_shutdown_with_config(test_config).await.expect("server failed to start");
    let _server_handle = tokio::spawn(async move {
        // Server is already started, just keep it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // First, upsert a test script
    let test_script_content = r#"
function delete_test_handler(req) {
    return { status: 200, body: 'Delete test endpoint works!' };
}
register('/delete-test-endpoint', 'delete_test_handler', 'GET');
"#;

    let upsert_response = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/delete-test-script"),
            ("content", test_script_content),
        ])
        .send()
        .await
        .expect("POST request to /upsert_script failed");

    assert_eq!(upsert_response.status(), 200);

    // Verify the script was upserted
    tokio::time::sleep(Duration::from_millis(100)).await;
    let test_response = client
        .get(format!("http://127.0.0.1:{}/delete-test-endpoint", port))
        .send()
        .await
        .expect("GET request to delete test endpoint failed");

    assert_eq!(test_response.status(), 200);

    // Now test the delete_script endpoint
    let delete_response = client
        .post(format!("http://127.0.0.1:{}/delete_script", port))
        .form(&[("uri", "https://example.com/delete-test-script")])
        .send()
        .await
        .expect("POST request to /delete_script failed");

    assert_eq!(delete_response.status(), 200);

    let delete_body: serde_json::Value = delete_response
        .json()
        .await
        .expect("Failed to parse JSON response");
    assert_eq!(delete_body["success"], true);
    assert_eq!(delete_body["uri"], "https://example.com/delete-test-script");

    // Verify the script was actually deleted by checking the endpoint returns 404
    tokio::time::sleep(Duration::from_millis(100)).await; // Give time for script to be deleted

    let after_delete_response = client
        .get(format!("http://127.0.0.1:{}/delete-test-endpoint", port))
        .send()
        .await
        .expect("GET request to delete test endpoint after deletion failed");

    assert_eq!(after_delete_response.status(), 404);

    // Test deleting a non-existent script
    let nonexistent_delete_response = client
        .post(format!("http://127.0.0.1:{}/delete_script", port))
        .form(&[("uri", "https://example.com/nonexistent-script")])
        .send()
        .await
        .expect("POST request to /delete_script for nonexistent script failed");

    assert_eq!(nonexistent_delete_response.status(), 404);

    let nonexistent_body: serde_json::Value = nonexistent_delete_response
        .json()
        .await
        .expect("Failed to parse JSON response");
    assert_eq!(nonexistent_body["error"], "Script not found");
}

#[tokio::test]
async fn test_script_lifecycle_via_http_api() {
    // Create custom config with unique port
    let mut test_config = config::Config::from_env();
    test_config.port = 4004;

    // Start server in background task
    let port = start_server_without_shutdown_with_config(test_config).await.expect("server failed to start");
    let _server_handle = tokio::spawn(async move {
        // Server is already started, just keep it running
        tokio::time::sleep(Duration::from_secs(10)).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();

    // Test script content
    let script_content = r#"
function lifecycle_test_handler(req) {
    return { status: 200, body: 'Lifecycle test successful!' };
}
register('/lifecycle-test', 'lifecycle_test_handler', 'GET');
"#;

    // 1. Create script via HTTP API
    let create_response = client
        .post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[
            ("uri", "https://example.com/lifecycle-test-script"),
            ("content", script_content),
        ])
        .send()
        .await
        .expect("Failed to create script via HTTP API");

    assert_eq!(create_response.status(), 200);

    // 2. Verify script works
    tokio::time::sleep(Duration::from_millis(100)).await;
    let test_response = client
        .get(format!("http://127.0.0.1:{}/lifecycle-test", port))
        .send()
        .await
        .expect("Failed to test script endpoint");

    assert_eq!(test_response.status(), 200);
    let test_body = test_response.text().await.expect("Failed to read test response");
    assert_eq!(test_body, "Lifecycle test successful!");

    // 3. Delete script via HTTP API
    let delete_response = client
        .post(format!("http://127.0.0.1:{}/delete_script", port))
        .form(&[("uri", "https://example.com/lifecycle-test-script")])
        .send()
        .await
        .expect("Failed to delete script via HTTP API");

    assert_eq!(delete_response.status(), 200);

    // 4. Verify script is gone
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after_delete_response = client
        .get(format!("http://127.0.0.1:{}/lifecycle-test", port))
        .send()
        .await
        .expect("Failed to check deleted script endpoint");

    assert_eq!(after_delete_response.status(), 404);
}
