use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn js_registered_route_returns_expected() {
    // start server in background task
    // ensure repository scripts are present (core/debug are included by default)
    // start server in background task
    tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
    });

    // give server a moment to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

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

    // send request to `/` which should be registered by `scripts/core.js`.
    // Retry a few times to avoid races while the server finishes startup.
    let mut got = false;
    let mut last_body = String::new();
    for i in 0..10 {
        println!(
            "DEBUG: Test attempt {} - making request to http://127.0.0.1:4000/",
            i
        );
        let res_root = reqwest::get("http://127.0.0.1:4000/").await;
        match res_root {
            Ok(res) => {
                println!(
                    "DEBUG: Test attempt {} - got response with status: {}",
                    i,
                    res.status()
                );
                if res.status() == reqwest::StatusCode::OK {
                    let body_root = res.text().await.expect("read root body");
                    println!("DEBUG: Test attempt {} - response body: '{}'", i, body_root);
                    last_body = body_root.clone();
                    if !body_root.is_empty() && body_root.contains("Core handler: OK") {
                        got = true;
                        break;
                    }
                } else {
                    last_body = format!("HTTP {}", res.status());
                }
            }
            Err(e) => {
                println!("DEBUG: Test attempt {} - request failed: {}", i, e);
                last_body = format!("Request failed: {}", e);
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        got,
        "expected / to return Core handler response, last body: {}",
        last_body
    );
}

#[tokio::test]
async fn core_js_registers_root_path() {
    // ensure core.js contains a registration for '/'
    let core = repository::fetch_script("https://example.com/core").expect("core script missing");
    assert!(
        core.contains("register('/") || core.contains("register(\"/\""),
        "core.js must register '/' path"
    );
}
