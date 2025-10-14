mod common;

use aiwebengine::repository;
use common::{TestContext, wait_for_server};

#[tokio::test]
async fn test_test_editor_api_endpoints() {
    let context = TestContext::new();

    // Load test scripts dynamically using upsert_script
    let _ = repository::upsert_script(
        "https://example.com/test_editor",
        include_str!("../scripts/test_scripts/test_editor.js"),
    );
    let _ = repository::upsert_script(
        "https://example.com/test_editor_api",
        include_str!("../scripts/test_scripts/test_editor_api.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test the root endpoint first
    let root_response = client
        .get(format!("http://127.0.0.1:{}/", port))
        .send()
        .await;

    match root_response {
        Ok(resp) => {
            println!("Root request succeeded with status: {}", resp.status());
            let body = resp.text().await.unwrap_or_default();
            println!("Root response body: {}", body);
        }
        Err(e) => {
            println!("Root request failed: {}", e);
        }
    }

    // Now test the /test-editor-api endpoint
    println!("Making request to /test-editor-api...");
    let test_api_response = client
        .get(format!("http://127.0.0.1:{}/test-editor-api", port))
        .send()
        .await
        .expect("Test editor API request failed");

    let status = test_api_response.status();
    let test_api_body = test_api_response
        .text()
        .await
        .expect("Failed to read test editor API response");

    println!("Test editor API response status: {}", status);
    println!("Test editor API response: {}", test_api_body);

    if status != 200 {
        println!("Test editor API error body: {}", test_api_body);
    }

    assert_eq!(status, 200);
    assert!(test_api_body.contains("Testing editor API endpoints"));

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test]
async fn test_test_editor_functionality() {
    let context = TestContext::new();

    // Load test scripts dynamically using upsert_script
    let _ = repository::upsert_script(
        "https://example.com/test_editor",
        include_str!("../scripts/test_scripts/test_editor.js"),
    );
    let _ = repository::upsert_script(
        "https://example.com/test_editor_api",
        include_str!("../scripts/test_scripts/test_editor_api.js"),
    );

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test that the scripts list API includes the test scripts
    let scripts_response = client
        .get(format!("http://127.0.0.1:{}/api/scripts", port))
        .send()
        .await
        .expect("Scripts list request failed");

    assert_eq!(scripts_response.status(), 200);
    let scripts_body = scripts_response
        .text()
        .await
        .expect("Failed to read scripts response");

    // Parse the JSON response
    let scripts: Vec<serde_json::Value> =
        serde_json::from_str(&scripts_body).expect("Failed to parse scripts JSON");

    // Verify that test_editor and test_editor_api scripts are loaded
    let script_names: Vec<String> = scripts
        .iter()
        .filter_map(|s| s["name"].as_str())
        .map(|s| s.to_string())
        .collect();

    println!("Loaded scripts: {:?}", script_names);
    assert!(script_names.contains(&"https://example.com/test_editor".to_string()));
    assert!(script_names.contains(&"https://example.com/test_editor_api".to_string()));

    // Test retrieving the test_editor script content
    let test_editor_response = client
        .get(format!(
            "http://127.0.0.1:{}/api/scripts/https://example.com/test_editor",
            port
        ))
        .send()
        .await
        .expect("Test editor script request failed");

    assert_eq!(test_editor_response.status(), 200);
    let test_editor_body = test_editor_response
        .text()
        .await
        .expect("Failed to read test editor script response");

    assert!(test_editor_body.contains("testEditorAPI"));
    assert!(test_editor_body.contains("listScripts"));

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}
