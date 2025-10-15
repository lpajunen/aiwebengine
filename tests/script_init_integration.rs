use aiwebengine::js_engine::call_init_if_exists;
use aiwebengine::repository::{get_script_metadata, upsert_script};
use aiwebengine::script_init::{InitContext, ScriptInitializer};

#[tokio::test]
async fn test_init_function_called_successfully() {
    let script_uri = "test://init-success";
    let script_content = r#"
        let initWasCalled = false;
        
        function init(context) {
            initWasCalled = true;
            writeLog("Init called for: " + context.scriptName);
            writeLog("Is startup: " + context.isStartup);
        }
        
        function getInitStatus() {
            return initWasCalled;
        }
    "#;

    // Upsert the script first
    upsert_script(script_uri, script_content).expect("Should upsert script");

    // Create init context
    let context = InitContext::new(script_uri.to_string(), true);

    // Call init function directly (without ScriptInitializer)
    let result = call_init_if_exists(script_uri, script_content, context);

    assert!(result.is_ok(), "Should execute without error");
    assert!(
        result.unwrap().is_some(),
        "Should return Some(registrations) indicating init was called"
    );

    // Note: call_init_if_exists doesn't update metadata - that's done by ScriptInitializer
}

#[tokio::test]
async fn test_script_initializer_updates_metadata() {
    let script_uri = "test://init-metadata";
    let script_content = r#"
        function init(context) {
            writeLog("Updating metadata test");
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    // Use ScriptInitializer which handles metadata updates
    let initializer = ScriptInitializer::new(5000);
    let result = initializer
        .initialize_script(script_uri, true)
        .await
        .expect("Should initialize");

    assert!(result.success, "Initialization should succeed");

    // Now verify metadata was updated
    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert!(
        metadata.initialized,
        "Script should be marked as initialized"
    );
    assert!(metadata.init_error.is_none(), "Should have no init error");
    assert!(
        metadata.last_init_time.is_some(),
        "Should have init timestamp"
    );
}

#[tokio::test]
async fn test_script_without_init_function() {
    let script_uri = "test://no-init";
    let script_content = r#"
        function handleRequest(request) {
            return { status: 200, body: "Hello" };
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    let context = InitContext::new(script_uri.to_string(), false);
    let result = call_init_if_exists(script_uri, script_content, context);

    assert!(result.is_ok(), "Should execute without error");
    assert!(
        result.unwrap().is_none(),
        "Should return None when no init function exists"
    );
}

#[tokio::test]
async fn test_init_function_with_error() {
    let script_uri = "test://init-error";
    let script_content = r#"
        function init(context) {
            throw new Error("Init failed intentionally");
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    // Use ScriptInitializer to handle errors properly
    let initializer = ScriptInitializer::new(5000);
    let result = initializer
        .initialize_script(script_uri, true)
        .await
        .expect("Should return InitResult");

    assert!(!result.success, "Initialization should fail");
    assert!(result.error.is_some(), "Should have error message");

    // Debug print
    println!("Error message: {:?}", result.error);

    let error_msg = result.error.unwrap();
    assert!(
        error_msg.contains("Init") || error_msg.contains("failed"),
        "Error message should contain init-related text, got: {}",
        error_msg
    );

    // Verify metadata was updated with error
    let metadata = get_script_metadata(script_uri).expect("Should get metadata");
    assert!(
        !metadata.initialized,
        "Script should not be marked as initialized"
    );
    assert!(metadata.init_error.is_some(), "Should have init error");
}

#[tokio::test]
async fn test_script_initializer_single_script() {
    let script_uri = "test://initializer-test";
    let script_content = r#"
        function init(context) {
            writeLog("Initialized: " + context.scriptName);
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    let initializer = ScriptInitializer::new(5000); // 5 second timeout
    let result = initializer
        .initialize_script(script_uri, true)
        .await
        .expect("Should initialize");

    assert!(result.success, "Initialization should succeed");
    assert!(result.error.is_none(), "Should have no error");
    assert!(result.duration_ms > 0, "Should have measurable duration");
}

#[tokio::test]
async fn test_script_initializer_all_scripts() {
    // Create multiple test scripts
    let scripts = vec![
        (
            "test://multi-init-1",
            r#"function init(ctx) { writeLog("Init 1"); }"#,
        ),
        (
            "test://multi-init-2",
            r#"function init(ctx) { writeLog("Init 2"); }"#,
        ),
        ("test://multi-no-init", r#"function handler() { }"#),
    ];

    for (uri, content) in &scripts {
        upsert_script(uri, content).expect("Should upsert script");
    }

    let initializer = ScriptInitializer::new(5000);
    let results = initializer
        .initialize_all_scripts()
        .await
        .expect("Should initialize all");

    // Should have initialized all dynamic scripts (not static ones)
    assert!(results.len() >= 3, "Should have at least 3 results");

    // Count successful initializations
    let successful = results.iter().filter(|r| r.success).count();
    assert!(successful >= 3, "At least 3 scripts should succeed");
}

#[tokio::test]
async fn test_init_context_properties() {
    let script_uri = "test://context-test";
    let script_content = r#"
        let capturedContext = null;
        
        function init(context) {
            capturedContext = context;
            writeLog("ScriptName: " + context.scriptName);
            writeLog("IsStartup: " + context.isStartup);
            writeLog("Timestamp: " + context.timestamp);
        }
    "#;

    upsert_script(script_uri, script_content).expect("Should upsert script");

    let context = InitContext::new(script_uri.to_string(), true);
    let result = call_init_if_exists(script_uri, script_content, context);

    assert!(result.is_ok(), "Should execute successfully");
    assert!(result.unwrap().is_some(), "Init should be called");
}
