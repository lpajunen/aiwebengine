use super::*;
use crate::repository;
use crate::security::UserContext;
use crate::security::secure_globals::Principal;
use crate::stream_registry;
use rquickjs::Context;
use std::collections::HashMap;
use std::sync::{Arc, Once, OnceLock};
use std::time::{Duration, Instant};

static INIT: Once = Once::new();
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn setup_db() {
    INIT.call_once(|| {
        // Skip when the database server will not answer.
        if crate::test_db::connection_string_blocking().is_none() {
            return;
        }

        let pool = crate::test_db::pool();
        let db = Arc::new(crate::database::Database::from_pool(pool.clone()));
        crate::database::initialize_global_database(db);

        // Generate and initialize server ID
        let server_id = crate::notifications::generate_server_id();
        crate::notifications::initialize_server_id(server_id.clone());

        // Initialize PostgresRepository with pool and server_id
        let repo = crate::repository::PostgresRepository::new(pool, server_id);
        crate::repository::initialize_repository(repo);
    });
}

fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
}

fn unique_test_id(prefix: &str) -> String {
    format!("{}-{}", prefix, rand::random::<u64>())
}

#[test]
fn test_interrupt_handler_stops_infinite_loop() {
    let limits = ExecutionLimits {
        timeout_ms: 200,
        ..ExecutionLimits::default()
    };
    let (rt, _budget) = create_sandboxed_runtime(&limits).expect("runtime creation failed");
    let ctx = Context::full(&rt).expect("context creation failed");
    let start = Instant::now();
    let result: Result<(), rquickjs::Error> = ctx.with(|ctx| ctx.eval::<(), _>("while(true){}"));
    assert!(result.is_err(), "infinite loop must be interrupted");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "interrupt should fire near the {}ms deadline, took {:?}",
        limits.timeout_ms,
        start.elapsed()
    );
}

#[test]
fn test_memory_limit_stops_runaway_allocation() {
    let limits = ExecutionLimits {
        timeout_ms: 10_000,
        max_memory_mb: 8,
        ..ExecutionLimits::default()
    };
    let (rt, _budget) = create_sandboxed_runtime(&limits).expect("runtime creation failed");
    let ctx = Context::full(&rt).expect("context creation failed");
    let start = Instant::now();
    let result: Result<(), rquickjs::Error> = ctx.with(|ctx| {
        ctx.eval::<(), _>("const a = []; while(true) { a.push('x'.repeat(1024 * 1024)); }")
    });
    assert!(
        result.is_err(),
        "allocation beyond the memory limit must fail"
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "memory limit (not the timeout) should stop the script, took {:?}",
        start.elapsed()
    );
}

// Check if we should skip database-dependent tests
fn should_skip_db_tests() -> bool {
    crate::test_db::connection_string_blocking().is_none()
}

// Shadow execute_script_secure
fn execute_script_secure(uri: &str, content: &str, principal: Principal) -> ScriptExecutionResult {
    if should_skip_db_tests() {
        return ScriptExecutionResult {
            registrations: HashMap::new(),
            success: false,
            error: Some("Test skipped: no test database".to_string()),
            execution_time_ms: 0,
        };
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    super::execute_script_secure(uri, content, principal)
}

// Shadow execute_script_for_request_secure
fn execute_script_for_request_secure(
    params: RequestExecutionParams,
) -> Result<JsHttpResponse, String> {
    if should_skip_db_tests() {
        return Err("Test skipped: no test database".to_string());
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    super::execute_script_for_request_secure(params)
}

/// These tests predate elicitation and call tools that do not ask, so the
/// shim runs them unattended — `mcp.canAsk()` false, `mcp.ask` throwing —
/// and reads the result out of the outcome. A test that wants the other
/// branch builds its own `Exchange`.
fn execute_mcp_tool_handler(
    script_uri: &str,
    handler_function: &str,
    tool_name: &str,
    arguments: serde_json::Value,
    auth_context: Option<crate::auth::JsAuthContext>,
    user_context: crate::security::UserContext,
) -> Result<String, String> {
    if should_skip_db_tests() {
        return Err("Test skipped: no test database".to_string());
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    match super::execute_mcp_tool_handler(
        script_uri,
        handler_function,
        tool_name,
        arguments,
        auth_context,
        crate::security::Principal::Caller(user_context),
        crate::mcp_elicitation::Exchange::unattended(),
    )? {
        crate::mcp::ToolOutcome::Complete(result) => Ok(result),
        crate::mcp::ToolOutcome::InputRequired(_) => {
            Err("handler asked for input, which this shim does not answer".to_string())
        }
        crate::mcp::ToolOutcome::Handed(_) => {
            Err("handler handed off to a task, which this shim does not poll".to_string())
        }
    }
}

fn setup_db_for_test() {
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
}

#[test]
fn test_execute_script_simple_registration() {
    if should_skip_db_tests() {
        return;
    }
    let content = r#"
        routeRegistry.registerRoute("/test", { handler: "handler_function", method: "GET" });
    "#;

    let result = execute_script_secure("test-script", content, Principal::Engine("test"));

    assert!(result.success, "Script execution should succeed");
    assert!(result.error.is_none(), "Should not have error");
    assert_eq!(result.registrations.len(), 1);
    let route_meta = result
        .registrations
        .get(&("/test".to_string(), "GET".to_string()));
    assert!(route_meta.is_some());
    assert_eq!(route_meta.unwrap().handler_name, "handler_function");
}

#[test]
fn test_execute_mcp_tool_handler_includes_request_auth_context() {
    if should_skip_db_tests() {
        return;
    }

    let _lock = crate::repository::GLOBAL_TEST_LOCK.lock().unwrap();
    setup_db_for_test();

    let script_uri = unique_test_id("test-mcp-auth-context");
    let user_id = unique_test_id("user");
    let content = r#"
        function toolHandler(context) {
            return {
                kind: context.kind,
                toolName: context.meta.mcp.toolName,
                auth: {
                    isAuthenticated: context.request.auth.isAuthenticated,
                    isAdmin: context.request.auth.isAdmin,
                    isEditor: context.request.auth.isEditor,
                    userId: context.request.auth.userId,
                    userEmail: context.request.auth.userEmail,
                    userName: context.request.auth.userName,
                    provider: context.request.auth.provider
                }
            };
        }
    "#;

    let _ = repository::upsert_script(&script_uri, content);
    let result = execute_mcp_tool_handler(
        &script_uri,
        "toolHandler",
        "whoami",
        serde_json::json!({}),
        Some(crate::auth::JsAuthContext::authenticated(
            user_id.clone(),
            Some("user@example.com".to_string()),
            Some("Test User".to_string()),
            "github".to_string(),
            false,
            true,
        )),
        UserContext::authenticated(user_id.clone()),
    )
    .expect("MCP tool handler should execute successfully");

    let parsed: serde_json::Value =
        serde_json::from_str(&result).expect("Result should be valid JSON");

    assert_eq!(parsed["kind"], "mcpTool");
    assert_eq!(parsed["toolName"], "whoami");
    assert_eq!(parsed["auth"]["isAuthenticated"], true);
    assert_eq!(parsed["auth"]["isAdmin"], false);
    assert_eq!(parsed["auth"]["isEditor"], true);
    assert_eq!(parsed["auth"]["userId"], user_id);
    assert_eq!(parsed["auth"]["userEmail"], "user@example.com");
    assert_eq!(parsed["auth"]["userName"], "Test User");
    assert_eq!(parsed["auth"]["provider"], "github");
}

#[test]
fn test_execute_mcp_tool_handler_uses_caller_capabilities() {
    if should_skip_db_tests() {
        return;
    }

    let _lock = crate::repository::GLOBAL_TEST_LOCK.lock().unwrap();
    setup_db_for_test();

    let script_uri = unique_test_id("test-mcp-caller-capabilities");
    let user_id = unique_test_id("user");
    let target_asset = unique_test_id("asset");
    let content = r#"
        function toolHandler(context) {
            let deleteResult;
            try {
                deleteResult = String(files.delete(context.args.targetAsset));
            } catch (e) {
                deleteResult = "refused: " + e.message;
            }
            return {
                deleteResult: deleteResult,
                isAdmin: context.request.auth.isAdmin,
                userId: context.request.auth.userId
            };
        }
    "#;

    let _ = repository::upsert_script(&script_uri, content);
    let result = execute_mcp_tool_handler(
        &script_uri,
        "toolHandler",
        "delete-check",
        serde_json::json!({
            "targetAsset": target_asset,
        }),
        Some(crate::auth::JsAuthContext::authenticated(
            user_id.clone(),
            Some("member@example.com".to_string()),
            Some("Member User".to_string()),
            "github".to_string(),
            false,
            false,
        )),
        UserContext::authenticated(user_id.clone()),
    )
    .expect("MCP tool handler should execute successfully");

    let parsed: serde_json::Value =
        serde_json::from_str(&result).expect("Result should be valid JSON");

    assert_eq!(parsed["isAdmin"], false);
    assert_eq!(parsed["userId"], user_id);
    // The caller holds no DeleteAssets capability, so the handler's delete
    // is refused rather than running with the engine's own rights.
    let delete_result = parsed["deleteResult"]
        .as_str()
        .expect("deleteResult should be a string");
    assert!(
        delete_result.starts_with("refused: ") && delete_result.contains("delete_assets"),
        "expected a capability error, got: {}",
        delete_result
    );
}

#[test]
fn test_execute_script_multiple_registrations() {
    if should_skip_db_tests() {
        return;
    }
    let content = r#"
        routeRegistry.registerRoute("/api/users", { handler: "getUsers", method: "GET" });
        routeRegistry.registerRoute("/api/users", { handler: "createUser", method: "POST" });
        routeRegistry.registerRoute("/api/users/:id", { handler: "updateUser", method: "PUT" });
    "#;

    let result = execute_script_secure("multi-script", content, Principal::Engine("test"));

    assert!(result.success);
    assert_eq!(result.registrations.len(), 3);
    assert!(
        result
            .registrations
            .contains_key(&("/api/users".to_string(), "GET".to_string()))
    );
    assert!(
        result
            .registrations
            .contains_key(&("/api/users".to_string(), "POST".to_string()))
    );
    assert!(
        result
            .registrations
            .contains_key(&("/api/users/:id".to_string(), "PUT".to_string()))
    );
}

#[test]
fn test_execute_script_with_default_method() {
    if should_skip_db_tests() {
        return;
    }
    let content = r#"
        routeRegistry.registerRoute("/default-method", { handler: "handler", method: "GET" });
    "#;

    let result = execute_script_secure("default-method-script", content, Principal::Engine("test"));

    if !result.success {
        println!("Default method test failed with error: {:?}", result.error);
    }
    assert!(
        result.success,
        "Script execution failed: {:?}",
        result.error
    );
    let route_meta = result
        .registrations
        .get(&("/default-method".to_string(), "GET".to_string()));
    assert!(route_meta.is_some());
    assert_eq!(route_meta.unwrap().handler_name, "handler");
}

#[test]
fn test_execute_script_with_syntax_error() {
    let content = r#"
        routeRegistry.registerRoute("/test", "handler"
        // Missing closing parenthesis - syntax error
    "#;

    let result = execute_script_secure("error-script", content, Principal::Engine("test"));

    assert!(!result.success, "Script with syntax error should fail");
    assert!(result.error.is_some(), "Should have error message");
    assert!(
        result.registrations.is_empty(),
        "Should not have registrations on error"
    );
}

#[test]
fn test_execute_script_with_runtime_error() {
    let content = r#"
        throw new Error("Runtime error test");
    "#;

    let result = execute_script_secure("runtime-error-script", content, Principal::Engine("test"));

    assert!(!result.success);
    assert!(result.error.is_some());
    assert!(result.registrations.is_empty());
}

#[test]
fn test_execute_script_with_complex_javascript() {
    if should_skip_db_tests() {
        return;
    }
    let content = r#"
        function setupRoutes() {
            routeRegistry.registerRoute("/api/health", { handler: "healthCheck", method: "GET" });
            routeRegistry.registerRoute("/api/status", { handler: "statusCheck", method: "GET" });
        }

        setupRoutes();
    "#;

    let result = execute_script_secure("complex-script", content, Principal::Engine("test"));

    assert!(
        result.success,
        "Complex JavaScript should execute successfully. Error: {:?}",
        result.error
    );
    assert_eq!(result.registrations.len(), 2);
    assert!(
        result
            .registrations
            .contains_key(&("/api/health".to_string(), "GET".to_string()))
    );
    assert!(
        result
            .registrations
            .contains_key(&("/api/status".to_string(), "GET".to_string()))
    );
}

#[test]
fn test_execute_script_empty_content() {
    if should_skip_db_tests() {
        return;
    }
    let result = execute_script_secure("empty-script", "", Principal::Engine("test"));

    assert!(result.success, "Empty script should succeed");
    assert!(result.error.is_none());
    assert!(result.registrations.is_empty());
}

#[test]
fn test_execute_script_with_console_log() {
    if should_skip_db_tests() {
        return;
    }
    let content = r#"
        routeRegistry.registerRoute("/logged", { handler: "loggedHandler", method: "GET" });
    "#;

    let result = execute_script_secure("console-script", content, Principal::Engine("test"));

    // Should succeed even with console.log (which may not be available)
    // The important thing is it doesn't crash
    // Console.log may fail, so the script might not succeed, but it shouldn't crash
    if result.success {
        assert_eq!(result.registrations.len(), 1);
    } else {
        // If console.log failed, that's ok, we just check it didn't crash
        assert!(result.error.is_some());
    }
}

#[test]
fn test_script_execution_result_debug_format() {
    let mut registrations = HashMap::new();
    registrations.insert(
        ("/test".to_string(), "GET".to_string()),
        repository::RouteMetadata::simple("handler".to_string()),
    );

    let result = ScriptExecutionResult {
        registrations,
        success: true,
        error: None,
        execution_time_ms: 100,
    };

    let debug_str = format!("{:?}", result);
    assert!(debug_str.contains("ScriptExecutionResult"));
    assert!(debug_str.contains("/test"));
    assert!(debug_str.contains("success: true"));
}

#[test]
fn test_script_execution_result_clone() {
    let mut registrations = HashMap::new();
    registrations.insert(
        ("/api".to_string(), "POST".to_string()),
        repository::RouteMetadata::simple("handler".to_string()),
    );

    let original = ScriptExecutionResult {
        registrations,
        success: false,
        error: Some("Test error".to_string()),
        execution_time_ms: 200,
    };

    let cloned = original.clone();

    assert_eq!(original.success, cloned.success);
    assert_eq!(original.error, cloned.error);
    assert_eq!(original.registrations.len(), cloned.registrations.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_web_stream_function() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    if should_skip_db_tests() {
        return;
    }
    // Setup database first
    setup_db();

    // Ensure we clear streams only once per test run
    INIT.call_once(|| {
        let _ = stream_registry::GLOBAL_STREAM_REGISTRY.clear_all_streams();
    });

    let script_content = r#"
        routeRegistry.registerRoute('/test-stream-func', { stream: true });
        console.log('Stream registered successfully');
    "#;

    let _ = repository::upsert_script("stream-test-func", script_content);
    // Use secure execution with admin privileges for testing
    let result = execute_script_secure(
        "stream-test-func",
        script_content,
        Principal::Engine("test-admin"),
    );

    assert!(
        result.success,
        "Script should execute successfully: {:?}",
        result.error
    );
    assert!(result.error.is_none(), "Should not have any errors");

    // A stream is a registration of the script, reported the way its
    // routes are, rather than a write to a registry of its own.
    let registered = result.registrations.get(&(
        "/test-stream-func".to_string(),
        repository::STREAM_METHOD.to_string(),
    ));
    assert!(
        registered.is_some(),
        "Stream should be registered: {:?}",
        result.registrations
    );
    assert_eq!(
        registered.map(|meta| meta.kind),
        Some(repository::RouteKind::Stream)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_web_stream_invalid_path() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_content = r#"
        try {
            routeRegistry.registerRoute('invalid-path-test', { stream: true });
            console.error('ERROR: Should have failed');
        } catch (e) {
            console.log('Expected error: ' + String(e));
        }
    "#;

    let _ = repository::upsert_script("stream-invalid-test", script_content);
    let result = execute_script_secure(
        "stream-invalid-test",
        script_content,
        Principal::Engine("test"),
    );

    assert!(
        result.success,
        "Script should execute successfully even with caught exception"
    );

    // Small delay to ensure any registration attempts are complete
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Verify the invalid stream was NOT registered
    assert!(
        !result.registrations.contains_key(&(
            "invalid-path-test".to_string(),
            repository::STREAM_METHOD.to_string()
        )),
        "Invalid stream should not be registered"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_stream_message_function() {
    if should_skip_db_tests() {
        return;
    }
    setup_db();

    let script_content = r#"
        // Register a stream first
        routeRegistry.registerRoute('/test-message-stream', { stream: true });

        // Send a message to the specific stream
        routeRegistry.sendStreamMessage('/test-message-stream', '{"type": "test", "data": "Hello World"}');

        console.log('Message sent successfully');
    "#;

    let _ = repository::upsert_script("stream-message-test", script_content);
    // Use secure execution with admin privileges for testing
    let result = execute_script_secure(
        "stream-message-test",
        script_content,
        Principal::Engine("test-admin"),
    );

    assert!(
        result.success,
        "Script should execute successfully: {:?}",
        result.error
    );

    // Small delay to ensure the message is processed
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify the stream was registered
    assert!(
        result.registrations.contains_key(&(
            "/test-message-stream".to_string(),
            repository::STREAM_METHOD.to_string()
        )),
        "Stream should be registered"
    );

    // Check that logs were written (indicating successful execution)
    let logs = repository::fetch_log_messages("stream-message-test");
    assert!(
        logs.iter()
            .any(|log| log.message.contains("Message sent successfully")),
        "Should have logged successful message sending"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_stream_message_json_object() {
    if should_skip_db_tests() {
        return;
    }
    setup_db();

    let script_content = r#"
        // Register a stream first
        routeRegistry.registerRoute('/test-json-stream', { stream: true });

        // Send a complex JSON message
        var messageObj = {
            type: "notification",
            user: "testUser",
            data: {
                id: 123,
                text: "Hello from JavaScript",
                timestamp: new Date().getTime()
            },
            metadata: ["tag1", "tag2"]
        };

        // JavaScript must stringify the object before sending
        routeRegistry.sendStreamMessage('/test-json-stream', JSON.stringify(messageObj));

        console.log('Complex JSON message sent');
    "#;

    let _ = repository::upsert_script("stream-json-test", script_content);
    // Use secure execution with admin privileges for testing
    let result = execute_script_secure(
        "stream-json-test",
        script_content,
        Principal::Engine("test-admin"),
    );

    assert!(
        result.success,
        "Script should execute successfully: {:?}",
        result.error
    );

    // Small delay to ensure the message is processed
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify the stream was registered
    assert!(
        result.registrations.contains_key(&(
            "/test-json-stream".to_string(),
            repository::STREAM_METHOD.to_string()
        )),
        "Stream should be registered"
    );

    // Check that logs were written (indicating successful execution)
    let logs = repository::fetch_log_messages("stream-json-test");
    assert!(
        logs.iter()
            .any(|log| log.message.contains("Complex JSON message sent")),
        "Should have logged successful JSON message sending"
    );
}

#[test]
fn test_script_properties_validation() {
    if should_skip_db_tests() {
        return;
    }
    // Test with a script that exceeds the default 1MB limit
    let large_script =
        "// ".repeat(600_000) + "routeRegistry.registerRoute('/test', { handler: 'handler' });";
    assert!(large_script.len() > 1_000_000);

    let result = execute_script_secure(
        "test-large-script",
        &large_script,
        Principal::Engine("test"),
    );

    assert!(!result.success);
    assert!(result.error.is_some());
    assert!(result.error.unwrap().contains("Script too large"));
    // Execution time is always recorded
    println!("Validation took {} ms", result.execution_time_ms);
}

#[test]
fn test_script_validation_infinite_loop_warning() {
    let script_with_infinite_loop = "while(true) { console.log('infinite'); }";

    // This should still execute (just warn), but we can test that the validation function works
    let limits = ExecutionLimits::default();
    let validation_result = validate_script(script_with_infinite_loop, &limits);

    // Should pass validation (just warning), but our logs would show the warning
    assert!(validation_result.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_default_content_types() {
    use crate::security::UserContext;
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    // Test default content type for text body
    let text_script = r#"
        function testTextHandler(request) {
            return {
                status: 200,
                body: "Hello World"
            };
        }
    "#;

    let _ = repository::upsert_script("test-text-content-type", text_script);
    let params = RequestExecutionParams {
        script_uri: "test-text-content-type".to_string(),
        handler_name: "testTextHandler".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    assert_eq!(
        response.content_type,
        Some("text/plain; charset=UTF-8".to_string())
    );

    // Test default content type for bodyBase64
    let binary_script = r#"
        function testBinaryHandler(request) {
            return {
                status: 200,
                bodyBase64: "SGVsbG8gV29ybGQ="  // "Hello World" in base64
            };
        }
    "#;

    let _ = repository::upsert_script("test-binary-content-type", binary_script);
    let params = RequestExecutionParams {
        script_uri: "test-binary-content-type".to_string(),
        handler_name: "testBinaryHandler".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    assert_eq!(
        response.content_type,
        Some("application/octet-stream".to_string())
    );

    // Test explicit content type overrides default
    let explicit_script = r#"
        function testExplicitHandler(request) {
            return {
                status: 200,
                body: "Hello World",
                contentType: "application/json"
            };
        }
    "#;

    let _ = repository::upsert_script("test-explicit-content-type", explicit_script);
    let params = RequestExecutionParams {
        script_uri: "test-explicit-content-type".to_string(),
        handler_name: "testExplicitHandler".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    assert_eq!(response.content_type, Some("application/json".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_markdown_to_html_simple() {
    use crate::security::UserContext;
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    let script_content = r#"
        function testConvert(context) {
            const markdown = `# Hello World

This is **bold** text.`;
            const html = convert.markdown_to_html(markdown);
            return {
                status: 200,
                body: html,
                contentType: "text/html"
            };
        }
    "#;

    let _ = repository::upsert_script("test-convert-simple", script_content);
    let params = RequestExecutionParams {
        script_uri: "test-convert-simple".to_string(),
        handler_name: "testConvert".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    let body = String::from_utf8(response.body).unwrap();

    assert!(
        body.contains("<h1>Hello World</h1>"),
        "Should contain heading"
    );
    assert!(
        body.contains("<strong>bold</strong>"),
        "Should contain bold text"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_markdown_to_html_code_block() {
    use crate::security::UserContext;
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    let script_content = r#"
        function testConvertCode(context) {
            const markdown = '```javascript\nconst x = 42;\n```';
            const html = convert.markdown_to_html(markdown);
            return {
                status: 200,
                body: html,
                contentType: "text/html"
            };
        }
    "#;

    let _ = repository::upsert_script("test-convert-code", script_content);
    let params = RequestExecutionParams {
        script_uri: "test-convert-code".to_string(),
        handler_name: "testConvertCode".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    let body = String::from_utf8(response.body).unwrap();

    assert!(body.contains("<pre><code"), "Should contain code block");
    assert!(
        body.contains("const x = 42;"),
        "Should contain code content"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_markdown_to_html_list() {
    use crate::security::UserContext;
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    let script_content = r#"
        function testConvertList(context) {
            const markdown = '- Item 1\n- Item 2\n- Item 3';
            const html = convert.markdown_to_html(markdown);
            return {
                status: 200,
                body: html,
                contentType: "text/html"
            };
        }
    "#;

    let _ = repository::upsert_script("test-convert-list", script_content);
    let params = RequestExecutionParams {
        script_uri: "test-convert-list".to_string(),
        handler_name: "testConvertList".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    let body = String::from_utf8(response.body).unwrap();

    assert!(body.contains("<ul>"), "Should contain unordered list");
    assert!(
        body.contains("<li>Item 1</li>"),
        "Should contain list items"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_markdown_to_html_table() {
    use crate::security::UserContext;
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    let script_content = r#"
        function testConvertTable(context) {
            const markdown = '| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |';
            const html = convert.markdown_to_html(markdown);
            return {
                status: 200,
                body: html,
                contentType: "text/html"
            };
        }
    "#;

    let _ = repository::upsert_script("test-convert-table", script_content);
    let params = RequestExecutionParams {
        script_uri: "test-convert-table".to_string(),
        handler_name: "testConvertTable".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    let body = String::from_utf8(response.body).unwrap();

    assert!(body.contains("<table>"), "Should contain table");
    assert!(
        body.contains("<th>Header 1</th>"),
        "Should contain table headers"
    );
    assert!(
        body.contains("<td>Cell 1</td>"),
        "Should contain table cells"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_markdown_to_html_empty_input() {
    use crate::security::UserContext;
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    let script_content = r#"
        function testConvertEmpty(context) {
            const markdown = '';
            try {
                convert.markdown_to_html(markdown);
                return { status: 200, body: "converted", contentType: "text/plain" };
            } catch (e) {
                return { status: 200, body: e.name + ": " + e.message, contentType: "text/plain" };
            }
        }
    "#;

    let _ = repository::upsert_script("test-convert-empty", script_content);
    let params = RequestExecutionParams {
        script_uri: "test-convert-empty".to_string(),
        handler_name: "testConvertEmpty".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    let body = String::from_utf8(response.body).unwrap();

    assert!(
        body.starts_with("Error: convert.markdown_to_html: "),
        "empty input should throw a named error, got: {}",
        body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_markdown_to_html_complex() {
    use crate::security::UserContext;
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    let script_content = r#"
        function testConvertComplex(context) {
            const markdown = `# My Blog Post

This is a **blog post** with *italic* text.

## Features

- Markdown support
- Code highlighting
- Tables

### Code Example

\`\`\`javascript
function hello() {
return "world";
}
\`\`\`

[Link to example](https://example.com)
`;
            const html = convert.markdown_to_html(markdown);
            return {
                status: 200,
                body: html,
                contentType: "text/html"
            };
        }
    "#;

    let _ = repository::upsert_script("test-convert-complex", script_content);
    let params = RequestExecutionParams {
        script_uri: "test-convert-complex".to_string(),
        handler_name: "testConvertComplex".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let result = execute_script_for_request_secure(params);

    assert!(result.is_ok(), "Request should execute successfully");
    let response = result.unwrap();
    let body = String::from_utf8(response.body).unwrap();

    assert!(body.contains("<h1>My Blog Post</h1>"), "Should contain h1");
    assert!(body.contains("<h2>Features</h2>"), "Should contain h2");
    assert!(
        body.contains("<strong>blog post</strong>"),
        "Should contain bold"
    );
    assert!(body.contains("<em>italic</em>"), "Should contain italic");
    assert!(body.contains("<ul>"), "Should contain list");
    assert!(body.contains("<pre><code"), "Should contain code block");
    assert!(
        body.contains("function hello()"),
        "Should contain code content"
    );
    assert!(
        body.contains("<a href=\"https://example.com\">Link to example</a>"),
        "Should contain link"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_response_builders() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_content = r#"
        function testHandler(context) {
            // Test ResponseBuilder.json
            const jsonResponse = ResponseBuilder.json({ message: "Hello World", code: 200 });
            if (jsonResponse.status !== 200 || jsonResponse.contentType !== "application/json") {
                throw new Error("JSON response failed");
            }

            // Test ResponseBuilder.text
            const textResponse = ResponseBuilder.text("Plain text response", 201);
            if (textResponse.status !== 201 || textResponse.contentType !== "text/plain; charset=UTF-8") {
                throw new Error("Text response failed");
            }

            // Test ResponseBuilder.html
            const htmlResponse = ResponseBuilder.html("<h1>Hello</h1>", 200);
            if (htmlResponse.status !== 200 || htmlResponse.contentType !== "text/html; charset=UTF-8") {
                throw new Error("HTML response failed");
            }

            // Test ResponseBuilder.error
            const errorResponse = ResponseBuilder.error(400, "Bad Request");
            if (errorResponse.status !== 400 || !errorResponse.body.includes("Bad Request")) {
                throw new Error("Error response failed");
            }

            // Test ResponseBuilder.noContent
            const noContentResponse = ResponseBuilder.noContent();
            if (noContentResponse.status !== 204 || noContentResponse.body !== "") {
                throw new Error("No content response failed");
            }

            // Test ResponseBuilder.redirect
            const redirectResponse = ResponseBuilder.redirect("https://example.com", 302);
            if (redirectResponse.status !== 302 || !redirectResponse.headers.Location) {
                throw new Error("Redirect response failed");
            }

            return ResponseBuilder.json({ success: true });
        }
    "#;

    let _ = repository::upsert_script("response-builder-test", script_content);

    let params = RequestExecutionParams {
        script_uri: "response-builder-test".to_string(),
        handler_name: "testHandler".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        auth_context: None,
        uploaded_files: None,
        route_params: None,
        request_id: None,
        route_pattern: None,
    };

    let result = execute_script_for_request_secure(params);
    if let Err(ref e) = result {
        eprintln!("Test error: {}", e);
    }
    assert!(result.is_ok(), "Response builder test should succeed");

    let response = result.unwrap();
    assert_eq!(response.status, 200);
    let body_str = String::from_utf8_lossy(&response.body);
    assert!(body_str.contains("success"));
    assert!(body_str.contains("true"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_query_object_guarantees() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_content = r#"
        function testHandler(context) {
            // Test that context.request.query is always available
            if (!context.request || !context.request.query) {
                throw new Error("context.request.query should always be available");
            }

            // Test that it's an object (even if empty)
            if (typeof context.request.query !== 'object') {
                throw new Error("context.request.query should be an object");
            }

            // Test that we can safely access properties
            const param1 = context.request.query.param1 || "default";
            const param2 = context.request.query.param2 || "default2";

            return ResponseBuilder.json({
                param1: param1,
                param2: param2,
                queryType: typeof context.request.query
            });
        }
    "#;

    let _ = repository::upsert_script("query-guarantees-test", script_content);

    let params = RequestExecutionParams {
        script_uri: "query-guarantees-test".to_string(),
        handler_name: "testHandler".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: Some(HashMap::from([
            ("param1".to_string(), "value1".to_string()),
            ("param2".to_string(), "value2".to_string()),
        ])),
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        auth_context: None,
        uploaded_files: None,
        route_params: None,
        request_id: None,
        route_pattern: None,
    };

    let result = execute_script_for_request_secure(params);
    assert!(result.is_ok(), "Query guarantees test should succeed");

    let response = result.unwrap();
    assert_eq!(response.status, 200);
    let body_str = String::from_utf8_lossy(&response.body);
    assert!(body_str.contains("value1"));
    assert!(body_str.contains("value2"));
    assert!(body_str.contains("object"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_query_object_guarantees_empty_params() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_content = r#"
        function testHandler(context) {
            // Test that context.request.query is available even with no query params
            if (!context.request || !context.request.query) {
                throw new Error("context.request.query should always be available");
            }

            // Should be an empty object
            const keys = Object.keys(context.request.query);
            if (keys.length !== 0) {
                throw new Error("Expected empty query object, got: " + keys.length + " keys");
            }

            return ResponseBuilder.json({ queryEmpty: true });
        }
    "#;

    let _ = repository::upsert_script("query-empty-test", script_content);

    let params = RequestExecutionParams {
        script_uri: "query-empty-test".to_string(),
        handler_name: "testHandler".to_string(),
        path: "/test".to_string(),
        method: "GET".to_string(),
        query_params: None, // No query params
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        auth_context: None,
        uploaded_files: None,
        route_params: None,
        request_id: None,
        route_pattern: None,
    };

    let result = execute_script_for_request_secure(params);
    assert!(result.is_ok(), "Empty query test should succeed");

    let response = result.unwrap();
    assert_eq!(response.status, 200);
    let body_str = String::from_utf8_lossy(&response.body);
    assert!(body_str.contains("queryEmpty"));
    assert!(body_str.contains("true"));
}
#[tokio::test(flavor = "multi_thread")]
async fn test_automatic_path_parameters() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_content = r#"
        function testHandler(context) {
            // Test that context.request.params contains extracted path parameters
            if (!context.request || !context.request.params) {
                throw new Error("context.request.params should be available");
            }

            // Check that path parameters are correctly extracted
            const userId = context.request.params.userId;
            const postId = context.request.params.postId;

            if (!userId || !postId) {
                throw new Error("Path parameters not extracted correctly");
            }

            return ResponseBuilder.json({
                userId: userId,
                postId: postId,
                paramsType: typeof context.request.params
            });
        }
    "#;

    let _ = repository::upsert_script("path-params-test", script_content);

    let params = RequestExecutionParams {
        script_uri: "path-params-test".to_string(),
        handler_name: "testHandler".to_string(),
        path: "/api/users/123/posts/456".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::admin("test".to_string()),
        auth_context: None,
        uploaded_files: None,
        route_params: Some(HashMap::from([
            ("userId".to_string(), "123".to_string()),
            ("postId".to_string(), "456".to_string()),
        ])),
        request_id: None,
        route_pattern: None,
    };

    let result = execute_script_for_request_secure(params);
    if let Err(e) = &result {
        eprintln!("Test failed with error: {:?}", e);
    }
    assert!(
        result.is_ok(),
        "Path parameters test should succeed: {:?}",
        result.as_ref().err()
    );

    let response = result.unwrap();
    assert_eq!(response.status, 200);
    let body_str = String::from_utf8_lossy(&response.body);
    assert!(body_str.contains("123"));
    assert!(body_str.contains("456"));
    assert!(body_str.contains("object"));
}
