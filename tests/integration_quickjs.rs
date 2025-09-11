use aiwebengine::start_server_with_script;
use std::time::Duration;

#[tokio::test]
async fn js_registered_route_returns_expected() {
    // start server in background task
    // ensure repository scripts are present (core/debug are included by default)
    // start server in background task
    tokio::spawn(async move {
        let _ = start_server_with_script().await;
    });

    // give server a moment to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // send request to /debug which now returns listLogs() as JSON
    let res = reqwest::get("http://127.0.0.1:4000/debug")
        .await
        .expect("request failed");
    let body = res.text().await.expect("read body");
    // parse JSON array and ensure it contains at least one string
    let v: serde_json::Value = serde_json::from_str(&body).expect("expected JSON array");
    let arr = v.as_array().expect("expected JSON array");
    assert!(!arr.is_empty(), "expected at least one log entry");
    // ensure first entry contains 'server started' (timestamp varies)
    if let Some(first) = arr.get(0) {
        let s = first.as_str().unwrap_or("");
        assert!(
            s.contains("server started"),
            "expected startup log in first entry"
        );
    }
}
