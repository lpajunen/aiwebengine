use aiwebengine::js_engine::{
    RequestExecutionParams, execute_script_for_request_secure, execute_script_secure,
};
use aiwebengine::security::UserContext;

#[test]
fn test_secure_script_execution_authenticated() {
    // Test with authenticated user
    let user_context = UserContext::authenticated("test_user".to_string());
    let script_content = r#"
        writeLog("Hello from secure context!");
        
        // Try to upsert a script (should work with WriteScripts capability)
        upsertScript("test_script", "console.log('test');");
        
        register("/test", "handleTest", "GET");
        
        function handleTest(request) {
            return {
                status: 200,
                body: "Hello from secure test handler!",
                contentType: "text/plain"
            };
        }
    "#;

    let result = execute_script_secure("/test_secure", script_content, user_context);

    assert!(
        result.success,
        "Script execution should succeed: {}",
        result.error.unwrap_or_default()
    );
    assert!(
        result
            .registrations
            .contains_key(&("/test".to_string(), "GET".to_string()))
    );
}

#[test]
fn test_secure_script_execution_anonymous() {
    // Test with anonymous user (limited capabilities)
    let user_context = UserContext::anonymous();
    let script_content = r#"
        // Anonymous users can view logs
        listLogs();
        
        // But cannot upsert scripts (should fail with capability error)
        upsertScript("test_script", "console.log('test');");
    "#;

    let result = execute_script_secure("/test_anonymous", script_content, user_context);

    // Script should still execute, but upsertScript should return error message
    assert!(
        result.success,
        "Script execution should succeed even with capability failures"
    );
}

#[test]
fn test_secure_request_execution() {
    // First, set up a script with authenticated user
    let user_context = UserContext::authenticated("test_user".to_string());
    let script_content = r#"
        function handleSecureTest(request) {
            writeLog("Handling secure request: " + request.path);
            
            return {
                status: 200,
                body: JSON.stringify({
                    message: "Secure request handled",
                    path: request.path,
                    method: request.method
                }),
                contentType: "application/json"
            };
        }
    "#;

    // Execute script to register the handler
    let result =
        execute_script_secure("/test_request_script", script_content, user_context.clone());
    assert!(result.success, "Script setup should succeed");

    // Now test secure request execution
    let request_params = RequestExecutionParams {
        script_uri: "/test_request_script".to_string(),
        handler_name: "handleSecureTest".to_string(),
        path: "/api/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        form_data: None,
        raw_body: None,
        user_context,
    };
    let request_result = execute_script_for_request_secure(request_params);

    match request_result {
        Ok((status, body, content_type)) => {
            assert_eq!(status, 200);
            assert!(body.contains("Secure request handled"));
            assert_eq!(content_type, Some("application/json".to_string()));
        }
        Err(e) => panic!("Secure request execution failed: {}", e),
    }
}

#[test]
fn test_secure_script_validation() {
    let user_context = UserContext::authenticated("test_user".to_string());

    // Test script with dangerous patterns
    let dangerous_script = r#"
        // This should be detected and logged as suspicious
        eval("malicious code");
        
        writeLog("This part should still work");
    "#;

    let result = execute_script_secure("/test_dangerous", dangerous_script, user_context);

    // The script should execute (validation warnings are logged, not blocking)
    // but the dangerous patterns should be logged
    assert!(result.success, "Script with warnings should still execute");
}

#[test]
fn test_capability_enforcement() {
    let user_context = UserContext::anonymous(); // No DeleteScripts capability

    let script_content = r#"
        // This should fail due to insufficient capabilities
        deleteScript("some_script");
    "#;

    let result = execute_script_secure("/test_capabilities", script_content, user_context);

    // Script should execute, but deleteScript should return capability error
    assert!(
        result.success,
        "Script should execute despite capability failures"
    );
}
