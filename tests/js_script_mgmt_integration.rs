use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn js_script_mgmt_functions_work() {
    // upsert the management test script so it registers /js-mgmt-check and the upsert logic
    repository::upsert_script(
        "https://example.com/js-mgmt-test",
        include_str!("../scripts/js_script_mgmt_test.js"),
    );

    tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let res = reqwest::get("http://127.0.0.1:4000/js-mgmt-check")
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
    let res2 = reqwest::get("http://127.0.0.1:4000/from-js")
        .await
        .expect("request failed");
    let status = res2.status();
    // The endpoint should now return 404 since the script was deleted
    assert_eq!(status, 404);
}

#[tokio::test]
async fn test_upsert_script_endpoint() {
    // Start server in background task
    let _server_handle = tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
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
        .post("http://127.0.0.1:4000/upsert_script")
        .form(&[
            ("uri", "https://example.com/test-endpoint-script"),
            ("content", test_script_content),
        ])
        .send()
        .await
        .expect("POST request to /upsert_script failed");

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.expect("Failed to parse JSON response");
    assert_eq!(body["success"], true);
    assert_eq!(body["uri"], "https://example.com/test-endpoint-script");
    assert!(body["contentLength"].as_u64().unwrap() > 0);

    // Verify the script was actually upserted by calling the new endpoint
    tokio::time::sleep(Duration::from_millis(100)).await; // Give time for script to be processed

    let test_response = client
        .get("http://127.0.0.1:4000/test-endpoint")
        .send()
        .await
        .expect("GET request to test endpoint failed");

    assert_eq!(test_response.status(), 200);
    let test_body = test_response.text().await.expect("Failed to read test response");
    assert_eq!(test_body, "Test endpoint works!");
}
