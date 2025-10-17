//! Editor API Integration Tests
//!
//! This module contains tests for editor-specific API endpoints including:
//! - Script listing via /api/scripts
//! - Getting individual scripts
//! - Saving scripts
//! - Editor UI functionality

mod common;

use aiwebengine::repository;
use common::{TestContext, wait_for_server};

// ============================================================================
// Editor API Endpoint Tests
// ============================================================================

#[tokio::test]
async fn test_editor_api_endpoints() {
    let context = TestContext::new();

    // Start server
    let port = context
        .start_server()
        .await
        .expect("Server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::new();

    // Test GET /api/scripts - list scripts
    let list_response = client
        .get(format!("http://127.0.0.1:{}/api/scripts", port))
        .send()
        .await
        .expect("List scripts request failed");

    assert_eq!(list_response.status(), 200);
    let list_body = list_response
        .text()
        .await
        .expect("Failed to read list response");
    println!("Scripts list: {}", list_body);

    // Parse the JSON response
    let scripts: Vec<serde_json::Value> =
        serde_json::from_str(&list_body).expect("Failed to parse scripts JSON");

    // Test GET /api/scripts/{name} - get specific script
    if !scripts.is_empty() {
        let first_script = &scripts[0];
        let script_name = first_script["name"].as_str().unwrap_or("core");
        println!("Trying to get script: {}", script_name);

        // Try with just the short name first
        let short_name = "core";
        let get_response = client
            .get(format!(
                "http://127.0.0.1:{}/api/scripts/{}",
                port, short_name
            ))
            .send()
            .await
            .expect("Get script request failed");

        println!(
            "Response status for {}: {}",
            short_name,
            get_response.status()
        );

        if get_response.status() == 200 {
            let get_body = get_response
                .text()
                .await
                .expect("Failed to read get response");
            println!("Script {} content length: {}", short_name, get_body.len());
            assert!(!get_body.is_empty(), "Script content should not be empty");
        } else {
            println!("Short name failed with status {}", get_response.status());
        }
    }

    // Test POST /api/scripts/{name} - save script
    let test_script_name = "test_script";
    let test_content =
        "// Test script content\nfunction test() {\n    return 'Hello from test!';\n}";

    let save_response = client
        .post(format!(
            "http://127.0.0.1:{}/api/scripts/{}",
            port, test_script_name
        ))
        .body(test_content.to_string())
        .send()
        .await
        .expect("Save script request failed");

    let save_status = save_response.status();
    println!("Save response status: {}", save_status);

    if save_status != 200 {
        let error_body = save_response.text().await.unwrap_or_default();
        println!("Save error: {}", error_body);
    }

    assert_eq!(save_status, 200);

    // Verify the script was saved by retrieving it
    let verify_response = client
        .get(format!(
            "http://127.0.0.1:{}/api/scripts/{}",
            port, test_script_name
        ))
        .send()
        .await
        .expect("Verify script request failed");

    println!("Verify response status: {}", verify_response.status());

    if verify_response.status() == 200 {
        let verify_body = verify_response
            .text()
            .await
            .expect("Failed to read verify response");
        println!("Retrieved content length: {}", verify_body.len());
        println!("Expected content length: {}", test_content.len());
        println!("Content matches: {}", verify_body == test_content);
        assert_eq!(
            verify_body, test_content,
            "Retrieved script should match saved content"
        );
    } else {
        let error_body = verify_response.text().await.unwrap_or_default();
        println!("Verify error: {}", error_body);
        panic!("Script verification failed");
    }

    println!("All editor API tests passed!");

    // Cleanup
    context.cleanup().await.expect("Failed to cleanup");
}

// ============================================================================
// Test Editor API Tests
// ============================================================================

#[tokio::test]
async fn test_editor_test_api_endpoints() {
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
async fn test_editor_functionality() {
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
