use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn js_write_log_and_listlogs() {
    // upsert the js_log_test script so it registers its routes
    repository::upsert_script(
        "https://example.com/js-log-test",
        include_str!("../scripts/js_log_test.js"),
    );

    // start server with the js_log_test script
    tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
    });

    // allow server to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // call the route which should call writeLog
    let res = reqwest::get("http://127.0.0.1:4000/js-log-test")
        .await
        .expect("request failed");
    let body = res.text().await.expect("read body");
    assert!(body.contains("logged"));

    // now verify the log message was written
    // verify via Rust API
    let msgs = repository::fetch_log_messages();
    assert!(
        msgs.iter().any(|m| m == "js-log-test-called"),
        "expected log entry"
    );

    // verify via JS-exposed route that calls listLogs()
    // retry a few times to allow any small propagation/timing delays
    let mut found = false;
    let mut last_body = String::new();
    for i in 0..10 {
        let res2 = reqwest::get("http://127.0.0.1:4000/js-list").await;
        if let Ok(r) = res2 {
            if let Ok(body2) = r.text().await {
                println!("attempt {}: /js-list -> {}", i, body2);
                last_body = body2.clone();
                if body2.contains("js-log-test-called") {
                    found = true;
                    break;
                }
            }
        } else {
            println!("attempt {}: request failed", i);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if !found {
        println!("/js-list last body: {}", last_body);
    }
    assert!(found, "expected log entry in /js-list output");
}
