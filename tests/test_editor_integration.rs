use aiwebengine::repository;
use aiwebengine::{config, start_server_with_config};
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn test_test_editor_api_endpoints() {
    // Create custom config with unique port
    let mut test_config = config::Config::from_env();
    test_config.port = 4001;
    test_config.host = "127.0.0.1".to_string();

    // Load test scripts dynamically using upsert_script
    repository::upsert_script(
        "https://example.com/test_editor",
        include_str!("../scripts/test_editor.js"),
    );
    repository::upsert_script(
        "https://example.com/test_editor_api",
        include_str!("../scripts/test_editor_api.js"),
    );

    // Start server with custom config
    let (_tx, rx) = oneshot::channel();
    let port = test_config.port;
    tokio::spawn(async move {
        let _ = start_server_with_config(test_config, rx).await;
    });

    // Give server more time to start
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // Check if server is actually running by trying to connect
    let client = reqwest::Client::new();

    // Try multiple times to connect to ensure server is ready
    let mut server_ready = false;
    for attempt in 1..=5 {
        match client
            .get(format!("http://127.0.0.1:{}/", port))
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    server_ready = true;
                    println!("Server is ready on attempt {}", attempt);
                    break;
                }
            }
            Err(e) => {
                println!("Server not ready on attempt {}: {}", attempt, e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    if !server_ready {
        panic!("Server failed to start properly");
    }

    // Test the root endpoint first
    let root_response = client
        .get(format!("http://127.0.0.1:{}/", port))
        .send()
        .await
        .expect("Root request failed");

    println!("Root response status: {}", root_response.status());
    assert_eq!(root_response.status(), 200);

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
}

#[tokio::test]
async fn test_test_editor_functionality() {
    // Create custom config with unique port
    let mut test_config = config::Config::from_env();
    test_config.port = 4002;
    test_config.host = "127.0.0.1".to_string();

    // Load test scripts dynamically using upsert_script
    repository::upsert_script(
        "https://example.com/test_editor",
        include_str!("../scripts/test_editor.js"),
    );
    repository::upsert_script(
        "https://example.com/test_editor_api",
        include_str!("../scripts/test_editor_api.js"),
    );

    // Start server with custom config
    let (_tx, rx) = oneshot::channel();
    let port = test_config.port;
    tokio::spawn(async move {
        let _ = start_server_with_config(test_config, rx).await;
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

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
}
