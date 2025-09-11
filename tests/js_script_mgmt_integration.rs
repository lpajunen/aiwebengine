use aiwebengine::start_server_with_script;
use std::time::Duration;

#[tokio::test]
async fn js_script_mgmt_functions_work() {
    tokio::spawn(async move {
        let _ = start_server_with_script("scripts/js_script_mgmt_test.js").await;
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
    let body2 = res2.text().await.expect("read body");
    assert!(!body2.contains("from-js"));
}
