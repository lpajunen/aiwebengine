use aiwebengine::{js_engine, stream_registry::GLOBAL_STREAM_REGISTRY};

#[test]
fn test_simple_stream_registration() {
    // Test just the stream registration part first

    let test_script = r#"
        registerWebStream('/simple_test');
        writeLog('Stream registered successfully');
    "#;

    println!("Testing simple stream registration");

    // Execute the test script
    let result = js_engine::execute_script("simple_test.js", test_script);

    if !result.success {
        println!("Script execution failed: {:?}", result.error);
    }

    assert!(
        result.success,
        "Simple stream registration failed: {:?}",
        result.error
    );

    // Verify the stream was registered
    assert!(
        GLOBAL_STREAM_REGISTRY.is_stream_registered("/simple_test"),
        "Simple test stream should be registered"
    );

    println!("Simple stream registration test passed!");
}

#[test]
fn test_simple_message_sending() {
    // Test just the message sending part

    let test_script = r#"
        try {
            sendStreamMessage({ type: 'test', message: 'hello' });
            writeLog('Message sent successfully');
        } catch (error) {
            writeLog('Error sending message: ' + error.message);
        }
    "#;

    println!("Testing simple message sending");

    // Execute the test script
    let result = js_engine::execute_script("simple_send.js", test_script);

    println!(
        "Script result: success={}, error={:?}",
        result.success, result.error
    );

    // This might fail if no streams are registered, but we want to see the error
    if !result.success {
        println!("Expected failure - no streams registered");
    }
}

#[test]
fn test_combined_functionality() {
    // Test both registration and sending together

    let test_script = r#"
        registerWebStream('/combined_test');
        writeLog('Stream registered');
        
        try {
            sendStreamMessage({ type: 'combined_test', message: 'hello combined' });
            writeLog('Message sent successfully');
        } catch (error) {
            writeLog('Error sending message: ' + error.message);
        }
    "#;

    println!("Testing combined functionality");

    let result = js_engine::execute_script("combined_test.js", test_script);

    println!(
        "Combined result: success={}, error={:?}",
        result.success, result.error
    );

    // This should succeed
    assert!(result.success, "Combined test failed: {:?}", result.error);

    // Verify the stream was registered
    assert!(
        GLOBAL_STREAM_REGISTRY.is_stream_registered("/combined_test"),
        "Combined test stream should be registered"
    );

    println!("Combined test passed!");
}
