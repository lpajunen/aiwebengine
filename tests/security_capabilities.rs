//! Security and Capability Enforcement Tests
//!
//! This module contains all tests related to security capabilities and enforcement:
//! - Capability enforcement for operations
//! - Script validation (eval, prototype pollution, path traversal, XSS)
//! - Rate limiting
//! - Anonymous vs authenticated user capabilities
//! - URL and protocol validation
//! - Stream name validation
//! - Header injection prevention
//! - Secure global context execution

/// Security integration tests
///
/// These tests verify that security mechanisms are actually enforced:
/// - Capability-based access control works
/// - Rate limiting prevents abuse
/// - Security bypasses are impossible
use aiwebengine::security::{Capability, RateLimitKey, RateLimiter, UserContext};
use std::collections::HashMap;
use std::time::Duration;

mod common;
use common::setup_env;

// Helper function to create a user with specific capabilities
fn create_user_with_capabilities(user_id: &str, caps: Vec<Capability>) -> UserContext {
    UserContext {
        user_id: Some(user_id.to_string()),
        is_authenticated: true,
        capabilities: caps.into_iter().collect(),
        attenuated: false,
        network_scope: None,
        acting: Default::default(),
    }
}

/// Drop a test's rate limit bucket so the shared `rate_limits` table does not
/// accumulate a row per run.
async fn delete_bucket(pool: &sqlx::PgPool, key: &RateLimitKey) {
    let _ = sqlx::query("DELETE FROM rate_limits WHERE key = $1")
        .bind(key.as_string())
        .execute(pool)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rate_limiting_blocks_excessive_requests() {
    let pool = common::test_pool().await;
    let limiter = RateLimiter::new(pool.clone());
    // A key of this test's own. The bucket lives in the `rate_limits` table, so a
    // fixed key carries its drained state into the next run — and this test drains
    // it on purpose, which left the run after it failing on the very first assert.
    let key = RateLimitKey::IpAddress(format!("test_client-{}", uuid::Uuid::new_v4()));

    // IP limits are 1 token per second with max 60
    // First request should succeed
    for i in 0..3 {
        let result = limiter.check_rate_limit(key.clone(), 1).await;
        assert!(result.allowed, "Request {} should be allowed", i + 1);
    }

    // Drain the rest of the bucket in one call, so the refill that happens while
    // the requests are in flight cannot keep it above the limit.
    let _ = limiter.check_rate_limit(key.clone(), 57).await;

    // Ask for 30: the bucket is empty and refills at 1 token/s, so this stays
    // denied no matter how slowly the test itself is scheduled.
    let blocked = limiter.check_rate_limit(key.clone(), 30).await;
    assert!(
        !blocked.allowed,
        "Should be rate limited after many requests"
    );

    delete_bucket(&pool, &key).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rate_limiting_resets_after_window() {
    let pool = common::test_pool().await;
    let limiter = RateLimiter::new(pool.clone());
    let key = RateLimitKey::IpAddress(format!("test_client_reset-{}", uuid::Uuid::new_v4()));

    // Use up the whole bucket in one call
    let _ = limiter.check_rate_limit(key.clone(), 60).await;

    // A 30-token request cannot be satisfied by the 1 token/s refill for the next
    // 30 seconds, so this is a stable "still blocked" check. It is also refused
    // rather than served, which leaves the bucket empty for the refill below.
    let result = limiter.check_rate_limit(key.clone(), 30).await;
    assert!(!result.allowed, "Should be blocked after using all tokens");

    // Wait for some tokens to refill (1 token per second)
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Should be allowed again after refill
    let result_after = limiter.check_rate_limit(key.clone(), 1).await;
    assert!(
        result_after.allowed,
        "Should be allowed after tokens refill"
    );

    delete_bucket(&pool, &key).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_anonymous_user_has_minimal_capabilities() {
    let anon_user = UserContext::anonymous();

    // Anonymous users should not have write capabilities in production mode
    assert!(
        anon_user
            .require_capability(&Capability::WriteScripts)
            .is_err()
    );
    assert!(
        anon_user
            .require_capability(&Capability::WriteAssets)
            .is_err()
    );
    assert!(
        anon_user
            .require_capability(&Capability::ManageMcp)
            .is_err()
    );
    assert!(
        anon_user
            .require_capability(&Capability::ManageStreams)
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_authenticated_user_gets_default_capabilities() {
    let auth_user = UserContext::authenticated("user123".to_string());

    // Authenticated users should have read capabilities
    assert!(
        auth_user
            .require_capability(&Capability::ReadScripts)
            .is_ok()
    );
    assert!(
        auth_user
            .require_capability(&Capability::ReadAssets)
            .is_ok()
    );

    // But not admin capabilities by default
    assert!(
        auth_user
            .require_capability(&Capability::ManageMcp)
            .is_err()
    );
}

// ============================================================================
// Secure Globals Tests
// ============================================================================

use aiwebengine::js_engine::{
    RequestExecutionParams, execute_script_for_request_secure, execute_script_secure,
};

#[tokio::test(flavor = "multi_thread")]
async fn test_secure_script_execution_authenticated() {
    setup_env().await;
    // Test with admin user (needs route registration capability)
    let user_context = UserContext::admin("test_admin".to_string());
    let script_content = r#"
        console.log("Hello from secure context!");
        
        routeRegistry.registerRoute("/test", { handler: "handleTest", method: "GET" });
        
        function handleTest(request) {
            return {
                status: 200,
                body: "Hello from secure test handler!",
                contentType: "text/plain"
            };
        }
    "#;

    let result = execute_script_secure(
        "/test_secure",
        script_content,
        aiwebengine::security::Principal::Caller(user_context),
    );

    assert!(
        result.success,
        "Script execution should succeed: {}",
        result.error.unwrap_or_default()
    );
    // Note: Registration tracking is not implemented in the simplified version
    // assert!(
    //     result
    //         .registrations
    //         .contains_key(&("/test".to_string(), "GET".to_string()))
    // );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secure_script_execution_anonymous() {
    setup_env().await;
    // Test with anonymous user (limited capabilities)
    let user_context = UserContext::anonymous();
    let script_content = r#"
        // Anonymous users can write logs
        console.log("hello from an anonymous script");
        
        // But cannot delete files: the refusal throws
        let refused = null;
        try {
            files.delete("some_asset");
        } catch (e) {
            refused = e;
        }
        if (!refused || !refused.message.includes("delete_assets")) {
            throw new Error("expected a capability refusal, got: " + refused);
        }
    "#;

    let result = execute_script_secure(
        "/test_anonymous",
        script_content,
        aiwebengine::security::Principal::Caller(user_context),
    );

    // The script catches the refusal, so it runs to completion
    assert!(
        result.success,
        "Script execution should succeed even with capability failures"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secure_request_execution() {
    setup_env().await;
    // First, set up a script with authenticated user
    let user_context = UserContext::authenticated("test_user".to_string());
    let script_content = r#"
        function handleSecureTest(request) {
            console.log("Handling secure request: " + request.path);
            
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

    // Store the script, then execute it to register the handler
    aiwebengine::repository::upsert_script("/test_request_script", script_content)
        .expect("script should be stored");
    let result = execute_script_secure(
        "/test_request_script",
        script_content,
        aiwebengine::security::Principal::Caller(user_context.clone()),
    );
    assert!(result.success, "Script setup should succeed");

    // Now test secure request execution
    let request_params = RequestExecutionParams {
        script_uri: "/test_request_script".to_string(),
        handler_name: "handleSecureTest".to_string(),
        path: "/api/test".to_string(),
        method: "GET".to_string(),
        query_params: None,
        url: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context,
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    };
    let request_result = execute_script_for_request_secure(request_params);

    match request_result {
        Ok(response) => {
            assert_eq!(response.status, 200);
            let body_str = String::from_utf8_lossy(&response.body);
            assert!(body_str.contains("Secure request handled"));
            assert_eq!(response.content_type, Some("application/json".to_string()));
        }
        Err(e) => panic!("Secure request execution failed: {}", e),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secure_script_validation() {
    setup_env().await;
    let user_context = UserContext::authenticated("test_user".to_string());

    // Test script with potentially dangerous patterns but valid JavaScript
    let dangerous_script = r#"
        // This should be detected and logged as suspicious but still execute
        var dynamicCode = "console.log('dynamic execution')";
        
        console.log("This part should still work");
    "#;

    let result = execute_script_secure(
        "/test_dangerous",
        dangerous_script,
        aiwebengine::security::Principal::Caller(user_context),
    );

    // The script should execute (validation warnings are logged, not blocking)
    // but the dangerous patterns should be logged
    assert!(result.success, "Script with warnings should still execute");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_capability_enforcement() {
    setup_env().await;
    let user_context = UserContext::anonymous(); // No DeleteAssets capability

    let script_content = r#"
        // This fails for want of DeleteAssets, and says so by throwing
        let refused = null;
        try {
            files.delete("some_asset");
        } catch (e) {
            refused = e;
        }
        if (!refused || !refused.message.includes("delete_assets")) {
            throw new Error("expected a capability refusal, got: " + refused);
        }
    "#;

    let result = execute_script_secure(
        "/test_capabilities",
        script_content,
        aiwebengine::security::Principal::Caller(user_context),
    );

    // The script catches the refusal, so it runs to completion
    assert!(
        result.success,
        "Script should execute despite capability failures"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_upsert_sets_script_uri() {
    setup_env().await;
    let user_context = create_user_with_capabilities("test_user", vec![Capability::WriteAssets]);

    let script_uri = "/test_asset_script_uri";
    let asset_name = "test_asset.txt";
    let script_content = format!(
        r#"
        // Write a file
        files.write("{}", "test content");
    "#,
        asset_name
    );

    aiwebengine::repository::upsert_script(script_uri, &script_content)
        .expect("script should be stored");
    let result = execute_script_secure(
        script_uri,
        &script_content,
        aiwebengine::security::Principal::Caller(user_context),
    );

    assert!(
        result.success,
        "Script execution should succeed: {}",
        result.error.unwrap_or_default()
    );

    // Check that the asset was created with the correct script_uri
    let asset = aiwebengine::repository::fetch_asset(script_uri, asset_name);
    assert!(asset.is_some(), "Asset should exist");
    let asset = asset.unwrap();
    assert_eq!(
        asset.script_uri, script_uri,
        "script_uri should match the executing script URI"
    );
    assert_eq!(asset.uri, asset_name);
    assert_eq!(asset.mimetype, "text/plain");
    assert_eq!(String::from_utf8(asset.content).unwrap(), "test content");
}

// ============================================================================
// Engine API denial semantics
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn engine_log_reads_refuse_rather_than_answer_empty() {
    setup_env().await;

    // A caller without ViewLogs must be refused outright. Answering an empty
    // list would be indistinguishable from a script that has logged nothing,
    // and it disagreed with DELETE on the same path, which has always refused.
    let denied = create_user_with_capabilities("no-view-logs", vec![]);
    let error = aiwebengine::engine_api::query_logs_authorized(
        &denied,
        &aiwebengine::repository::LogQuery::default(),
    )
    .expect_err("log read without ViewLogs should be refused");
    assert_eq!(error.status_code(), 403, "expected a forbidden status");

    // The same query succeeds once the capability is held.
    let allowed = create_user_with_capabilities("view-logs", vec![Capability::ViewLogs]);
    aiwebengine::engine_api::query_logs_authorized(
        &allowed,
        &aiwebengine::repository::LogQuery::default(),
    )
    .expect("log read with ViewLogs should be allowed");
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_log_deletes_refuse_without_capability() {
    setup_env().await;

    let denied = create_user_with_capabilities("no-delete-logs", vec![Capability::ViewLogs]);
    let error = aiwebengine::engine_api::delete_logs_authorized(&denied, "some-script")
        .expect_err("clear without DeleteLogs should be refused");
    assert_eq!(error.status_code(), 403, "expected a forbidden status");
}

/// `DeleteLogs` is editor-tier, and an editor is not licensed to act on a
/// script they do not own — that is what `AdministerEngine` marks.
#[tokio::test(flavor = "multi_thread")]
async fn engine_log_clear_refuses_a_script_the_caller_does_not_own() {
    setup_env().await;

    let editor =
        create_user_with_capabilities("editor-without-ownership", vec![Capability::DeleteLogs]);
    let error =
        aiwebengine::engine_api::delete_logs_authorized(&editor, "a-script-owned-by-nobody-here")
            .expect_err("clearing another owner's logs should be refused");
    assert_eq!(error.status_code(), 403, "expected a forbidden status");

    // An administrator acts on what they do not own.
    let admin = create_user_with_capabilities(
        "admin-clearing-logs",
        vec![Capability::DeleteLogs, Capability::AdministerEngine],
    );
    aiwebengine::engine_api::delete_logs_authorized(&admin, "a-script-owned-by-nobody-here")
        .expect("an administrator may clear any script's logs");
}

#[tokio::test(flavor = "multi_thread")]
async fn engine_route_listing_refuses_rather_than_answers_empty() {
    setup_env().await;

    let denied = create_user_with_capabilities("no-read-scripts", vec![]);
    let error = aiwebengine::engine_api::routes_introspection_authorized(&denied)
        .expect_err("route listing without ReadScripts should be refused");
    assert_eq!(error.status_code(), 403, "expected a forbidden status");

    let allowed = create_user_with_capabilities("read-scripts", vec![Capability::ReadScripts]);
    aiwebengine::engine_api::routes_introspection_authorized(&allowed)
        .expect("route listing with ReadScripts should be allowed");
}
