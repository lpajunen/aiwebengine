use aiwebengine::repository;
use aiwebengine::start_server_without_shutdown;
use std::time::Duration;

#[tokio::test]
async fn test_form_data() {
    // Dynamically load the form test script
    repository::upsert_script(
        "https://example.com/form_test",
        include_str!("../scripts/form_test.js"),
    );

    // Start server in background task
    tokio::spawn(async move {
        let _ = start_server_without_shutdown().await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = reqwest::Client::new();

    // Test POST request to /api/form without form data
    let response_no_form = client
        .post("http://127.0.0.1:4000/api/form")
        .send()
        .await
        .expect("POST request without form data failed");
    assert_eq!(response_no_form.status(), 200);
    let body_no_form = response_no_form
        .text()
        .await
        .expect("Failed to read response without form data");
    assert!(
        body_no_form.contains("Path: /api/form"),
        "Response should contain correct path: {}",
        body_no_form
    );
    assert!(
        body_no_form.contains("Method: POST"),
        "Response should contain correct method: {}",
        body_no_form
    );
    assert!(
        body_no_form.contains("Form: none"),
        "Response should indicate no form data: {}",
        body_no_form
    );

    // Test POST request to /api/form with form data
    let response_with_form = client
        .post("http://127.0.0.1:4000/api/form")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("id=456&name=form_test&email=test@example.com")
        .send()
        .await
        .expect("POST request with form data failed");
    assert_eq!(response_with_form.status(), 200);
    let body_with_form = response_with_form
        .text()
        .await
        .expect("Failed to read response with form data");
    assert!(
        body_with_form.contains("Path: /api/form"),
        "Response should contain correct path: {}",
        body_with_form
    );
    assert!(
        body_with_form.contains("Method: POST"),
        "Response should contain correct method: {}",
        body_with_form
    );
    assert!(
        body_with_form.contains("Form:")
            && body_with_form.contains("id=456")
            && body_with_form.contains("name=form_test")
            && body_with_form.contains("email=test@example.com"),
        "Response should contain parsed form data: {}",
        body_with_form
    );
}
