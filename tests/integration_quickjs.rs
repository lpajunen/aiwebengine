use aiwebengine::{start_server_with_script};
use std::time::Duration;

#[tokio::test]
async fn js_registered_route_returns_expected() {
    // start server in background task
    tokio::spawn(async move {
        let _ = start_server_with_script("scripts/example.js").await;
    });

    // give server a moment to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // send request to /hello
    let res = reqwest::get("http://127.0.0.1:4000/hello").await.expect("request failed");
    let body = res.text().await.expect("read body");
    assert!(body.contains("Hello from JS!"));
}
