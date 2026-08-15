//! API Boundary Tests
//!
//! This module verifies that public vs engine-internal API boundaries are correctly enforced.
//! All scripts are equal — every script sees the same API surface, and each call is
//! authorized against the calling *user*. Tests ensure that:
//! - Users without the required capability cannot access engine-internal APIs
//! - All users can access public APIs
//! - Any script can register routes on non-reserved paths; engine-owned
//!   prefixes (/health, /graphql, /mcp, /auth, /.well-known, /engine) are rejected
//! - Administrators and script owners can access cross-script functionality
//!
//! ## Test Coverage
//!
//! ### Engine-internal APIs (require specific capabilities):
//! - **RouteRegistry**: listRoutes(), listStreams(), generateOpenApi() - require ReadScripts
//! - **ScriptStorage**: all 11 methods - require ReadScripts/WriteScripts/DeleteScripts
//! - **SecretStorage**: list() - requires admin privileges  
//! - **Console**: listLogs(), listLogsForUri(), pruneLogs() - require ViewLogs
//! - **UserStorage**: listUsers(), addUserRole(), removeUserRole() - require admin privileges
//! - **AssetStorage**: listAssetsForUri(), fetchAssetForUri(), upsertAssetForUri(), deleteAssetForUri()
//!
//! ### Public APIs (available to all scripts):
//! - **SecretStorage**: exists()
//! - **Convert**: markdown_to_html(), render_handlebars_template(), btoa(), atob()
//! - **Console**: log(), error(), warn(), info()
//! - **SchedulerService**: registerOnce(), registerRecurring(), clearAll()
//!
//! ### Cross-script access enforcement:
//! - Route/stream/asset registration is open to all scripts on non-reserved paths
//! - SecretStorage *ForUri methods require the calling user to be an administrator
//!   or an owner of the target script
//!
//! ## Ignored Tests
//!
//! Some tests are marked with `#[ignore]` because the underlying functionality is not yet
//! implemented or behaves differently than expected. These serve as documentation of what
//! needs to be implemented or fixed:
//! - Some API methods return undefined instead of null on capability denial
//! - userStorage.listUsers() throws errors instead of returning empty array

use aiwebengine::js_engine::execute_script_secure;
use aiwebengine::security::{Capability, UserContext};
use aiwebengine::{database, repository};
use tokio::sync::OnceCell;

static INIT: OnceCell<()> = OnceCell::const_new();

async fn setup_env() {
    INIT.get_or_init(|| async {
        let config = aiwebengine::config::AppConfig::test_config_postgres(0);
        if let Ok(db) = database::Database::new(&config.repository).await {
            let db_arc = std::sync::Arc::new(db);
            database::initialize_global_database(db_arc.clone());

            // Initialize repository with PostgreSQL
            repository::initialize_repository(repository::PostgresRepository::new(
                db_arc.pool().clone(),
                "test".to_string(),
            ));
        }
    })
    .await;
}

fn create_user_with_capabilities(user_id: &str, caps: Vec<Capability>) -> UserContext {
    UserContext {
        user_id: Some(user_id.to_string()),
        is_authenticated: true,
        capabilities: caps.into_iter().collect(),
    }
}

// ============================================================================
// Engine-internal API Tests - RouteRegistry
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_route_registry_list_routes_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const routes = routeRegistry.listRoutes();
        // Should return "[]" for denied access
        if (routes !== "[]") {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_route_registry_list_streams_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const streams = routeRegistry.listStreams();
        if (streams !== "[]") {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_route_registry_generate_openapi_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const spec = routeRegistry.generateOpenApi();
        // Should return empty/error for denied access
        const parsed = JSON.parse(spec);
        if (parsed && parsed.paths && Object.keys(parsed.paths).length > 0) {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_route_registry_introspection_available_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    let script = r#"
        // Admin should be able to call these functions
        const routes = routeRegistry.listRoutes();
        const streams = routeRegistry.listStreams();
        const spec = routeRegistry.generateOpenApi();
        // Should not throw errors
    "#;

    let result = execute_script_secure("test://api-test-admin", script, admin);
    assert!(
        result.success,
        "Admin should access APIs: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_route_registry_list_streams_allowed_with_read_scripts() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());
    let reader = create_user_with_capabilities("reader", vec![Capability::ReadScripts]);

    repository::upsert_script("test://list-streams-source", "").expect("Failed to create script");

    let register_script = r#"
        routeRegistry.registerStreamRoute("/test-list-streams-readable", "streamCustomizer");
    "#;

    let register_result =
        execute_script_secure("test://list-streams-source", register_script, admin);
    assert!(
        register_result.success,
        "Script should register stream route: {:?}",
        register_result.error
    );

    let inspect_script = r#"
        const streams = JSON.parse(routeRegistry.listStreams());
        const stream = streams.find((entry) => entry.path === "/test-list-streams-readable");

        if (!stream) {
            throw new Error("Expected stream in listStreams output");
        }

        if (stream.script_uri !== "test://list-streams-source") {
            throw new Error("Expected stream script_uri in listStreams output");
        }
    "#;

    let inspect_result =
        execute_script_secure("test://read-scripts-viewer", inspect_script, reader);
    assert!(
        inspect_result.success,
        "ReadScripts user should access listStreams: {:?}",
        inspect_result.error
    );
}

// ============================================================================
// Engine-internal API Tests - ScriptStorage
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_list_scripts_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const scripts = scriptStorage.listScripts();
        if (scripts !== "[]") {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_get_script_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        // Without ReadScripts capability, should return undefined
        const content = scriptStorage.getScript("nonexistent_test_script_12345");
        if (content !== undefined) {
            throw new Error("Should return undefined without capability, got: " + typeof content);
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_upsert_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const result = scriptStorage.upsertScript("test", "content");
        if (!result.startsWith("Error:")) {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_delete_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const result = scriptStorage.deleteScript("test");
        if (result !== false) {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_get_init_status_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const status = scriptStorage.getScriptInitStatus("nonexistent_test_script_status_54321");
        if (status !== undefined) {
            throw new Error("Should return undefined without capability");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_get_owners_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const owners = scriptStorage.getScriptOwners("test");
        if (owners !== "[]") {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_add_owner_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const result = scriptStorage.addScriptOwner("test", "user123");
        if (!result.startsWith("Error:")) {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_remove_owner_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const result = scriptStorage.removeScriptOwner("test", "user123");
        if (!result.startsWith("Error:")) {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_storage_available_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    let script = r#"
        // Admin should be able to call these functions
        const scripts = scriptStorage.listScripts();
        const content = scriptStorage.getScript("nonexistent");
        const owners = scriptStorage.getScriptOwners("nonexistent");
        // Should not throw errors
    "#;

    let result = execute_script_secure("test://api-test-admin", script, admin);
    assert!(
        result.success,
        "Admin should access APIs: {:?}",
        result.error
    );
}

// ============================================================================
// Engine-internal API Tests - SecretStorage
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_secret_storage_for_uri_denied_without_admin_or_ownership() {
    setup_env().await;
    let user = create_user_with_capabilities("outsider", vec![]);

    repository::upsert_script("test://secret-foruri-outsider", "")
        .expect("Failed to create script");

    // The methods are always present; each call is authorized against the user.
    let script = r#"
        const calls = [
            ["listForUri", () => secretStorage.listForUri("test://secret-foruri-outsider")],
            ["setSecretForUri", () => secretStorage.setSecretForUri("test://secret-foruri-outsider", "k", "v")],
            ["removeSecretForUri", () => secretStorage.removeSecretForUri("test://secret-foruri-outsider", "k")],
            ["clearForUri", () => secretStorage.clearForUri("test://secret-foruri-outsider")],
        ];
        for (const [name, call] of calls) {
            if (typeof secretStorage[name] !== "function") {
                throw new Error(name + " should be defined for every script");
            }
            let threw = false;
            try {
                call();
            } catch (e) {
                threw = true;
            }
            if (!threw) {
                throw new Error(name + " should be denied for a non-owner, non-admin user");
            }
        }
    "#;

    let result = execute_script_secure("test://secret-foruri-outsider", script, user);
    assert!(
        result.success,
        "Non-owner should be denied all *ForUri methods: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secret_storage_for_uri_allowed_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://secret-foruri-admin", "").expect("Failed to create script");

    // An admin may manage secrets of any script, from any script.
    let script = r#"
        const target = "test://secret-foruri-admin";
        const secrets = secretStorage.listForUri(target);
        if (!Array.isArray(secrets)) {
            throw new Error("Expected array from listForUri");
        }
        const setResult = secretStorage.setSecretForUri(target, "test-key", "test-value");
        if (!setResult.includes("successfully")) {
            throw new Error("Expected success from setSecretForUri, got: " + setResult);
        }
        const removeResult = secretStorage.removeSecretForUri(target, "test-key");
        if (typeof removeResult !== "boolean") {
            throw new Error("Expected boolean from removeSecretForUri");
        }
        const clearResult = secretStorage.clearForUri(target);
        if (!clearResult.includes("successfully")) {
            throw new Error("Expected success from clearForUri, got: " + clearResult);
        }
    "#;

    let result = execute_script_secure("test://secret-caller-admin", script, admin);
    assert!(
        result.success,
        "Admin should be allowed all *ForUri methods: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secret_storage_for_uri_allowed_for_script_owner() {
    setup_env().await;
    let owner = create_user_with_capabilities("secret-owner", vec![]);

    repository::upsert_script("test://secret-foruri-owned", "").expect("Failed to create script");
    repository::add_script_owner("test://secret-foruri-owned", "secret-owner")
        .expect("Failed to add owner");

    // Ownership of the *target* script grants secret management, without admin.
    let script = r#"
        const target = "test://secret-foruri-owned";
        const secrets = secretStorage.listForUri(target);
        if (!Array.isArray(secrets)) {
            throw new Error("Expected array from listForUri");
        }
        const setResult = secretStorage.setSecretForUri(target, "test-key", "test-value");
        if (!setResult.includes("successfully")) {
            throw new Error("Expected success from setSecretForUri, got: " + setResult);
        }
        const clearResult = secretStorage.clearForUri(target);
        if (!clearResult.includes("successfully")) {
            throw new Error("Expected success from clearForUri, got: " + clearResult);
        }
    "#;

    let result = execute_script_secure("test://secret-caller-owner", script, owner);
    assert!(
        result.success,
        "Script owner should be allowed *ForUri methods on their script: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secret_storage_for_uri_ownership_does_not_leak_to_other_scripts() {
    setup_env().await;
    let owner = create_user_with_capabilities("partial-owner", vec![]);

    repository::upsert_script("test://secret-owned-by-partial", "")
        .expect("Failed to create script");
    repository::add_script_owner("test://secret-owned-by-partial", "partial-owner")
        .expect("Failed to add owner");
    repository::upsert_script("test://secret-owned-by-nobody", "")
        .expect("Failed to create script");

    // Owning one script must not grant access to another script's secrets.
    let script = r#"
        let threw = false;
        try {
            secretStorage.setSecretForUri("test://secret-owned-by-nobody", "k", "v");
        } catch (e) {
            threw = true;
        }
        if (!threw) {
            throw new Error("Owning one script must not grant access to another");
        }
    "#;

    let result = execute_script_secure("test://secret-owned-by-partial", script, owner);
    assert!(
        result.success,
        "Ownership must not leak across scripts: {:?}",
        result.error
    );
}

// ============================================================================
// Engine-internal API Tests - Console
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_console_list_logs_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const logs = console.listLogs();
        if (logs !== "[]") {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_console_list_logs_for_uri_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const logs = console.listLogsForUri("test");
        if (logs !== "[]") {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_console_prune_logs_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        try {
            const result = console.pruneLogs();
            throw new Error("Should have thrown error");
        } catch (e) {
            // Expected to throw
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_console_engine_internal_methods_available_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    let script = r#"
        const logs = console.listLogs();
        const uriLogs = console.listLogsForUri("test");
        // Should not throw errors
    "#;

    let result = execute_script_secure("test://api-test-admin", script, admin);
    assert!(
        result.success,
        "Admin should access APIs: {:?}",
        result.error
    );
}

// ============================================================================
// Engine-internal API Tests - UserStorage
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_user_storage_list_users_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        // Without admin capability, should return empty array as JSON string
        const users = userStorage.listUsers();
        const parsed = JSON.parse(users);
        if (!Array.isArray(parsed) || parsed.length !== 0) {
            throw new Error("Should return empty array without capability");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_storage_add_role_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        try {
            userStorage.addUserRole("user123", "Editor");
            throw new Error("Should have thrown error");
        } catch (e) {
            // Expected to throw
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_storage_remove_role_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        try {
            userStorage.removeUserRole("user123", "Editor");
            throw new Error("Should have thrown error");
        } catch (e) {
            // Expected to throw
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_storage_available_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    let script = r#"
        const users = userStorage.listUsers();
        // Should not throw error
    "#;

    let result = execute_script_secure("test://api-test-admin", script, admin);
    assert!(
        result.success,
        "Admin should access API: {:?}",
        result.error
    );
}

// ============================================================================
// Engine-internal API Tests - AssetStorage
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_storage_list_for_uri_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const assets = assetStorage.listAssetsForUri("test");
        if (assets !== "[]") {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_storage_fetch_for_uri_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const content = assetStorage.fetchAssetForUri("test", "asset.txt");
        if (!content.startsWith("Error:")) {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_storage_upsert_for_uri_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const result = assetStorage.upsertAssetForUri("test", "asset.txt", "text/plain", "");
        // Should return error message
        if (typeof result !== "string" || !result.includes("Error")) {
            throw new Error("Should return error, got: " + result);
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_storage_delete_for_uri_denied_without_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const result = assetStorage.deleteAssetForUri("test", "asset.txt");
        if (!result.startsWith("Error:")) {
            throw new Error("Should deny access");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(result.success, "Script should execute: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_asset_storage_engine_internal_methods_available_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    let script = r#"
        const assets = assetStorage.listAssetsForUri("test");
        // Should not throw error
    "#;

    let result = execute_script_secure("test://api-test-admin", script, admin);
    assert!(
        result.success,
        "Admin should access API: {:?}",
        result.error
    );
}

// ============================================================================
// Public API Tests - SecretStorage.exists()
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_secret_storage_exists_available_for_all() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        // exists() should be available to all scripts
        const exists = secretStorage.exists("API_KEY");
        // Should return boolean, not throw error
        if (typeof exists !== "boolean") {
            throw new Error("exists() should be available");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(
        result.success,
        "exists() should be public: {:?}",
        result.error
    );
}

// ============================================================================
// Public API Tests - SchedulerService
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_scheduler_service_available_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://scheduler-admin", "").expect("Failed to create script");

    let script = r#"
        // SchedulerService should be available
        schedulerService.clearAll();
        const result = schedulerService.registerOnce({
            handler: "testHandler",
            runAt: new Date(Date.now() + 60000).toISOString(),
            name: "test-job"
        });
        // Should not throw error
    "#;

    let result = execute_script_secure("test://scheduler-admin", script, admin);
    assert!(
        result.success,
        "Script should access schedulerService: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scheduler_service_available_for_any_script() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://scheduler-any", "").expect("Failed to create script");

    let script = r#"
        if (typeof schedulerService === "undefined") {
            throw new Error("schedulerService should be defined");
        }
        // SchedulerService should be available for all scripts
        schedulerService.clearAll();
        const result = schedulerService.registerOnce({
            handler: "testHandler",
            runAt: new Date(Date.now() + 60000).toISOString(),
            name: "test-job-non-priv"
        });
        // Should not throw error
    "#;

    let result = execute_script_secure("test://scheduler-any", script, admin);
    assert!(
        result.success,
        "Any script should access schedulerService: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scheduler_register_once_available_for_any_script() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://non-priv-sched", "").expect("Failed to create script");

    let script = r#"
        if (typeof schedulerService === "undefined") {
            throw new Error("schedulerService should be defined");
        }
        // registerOnce should be available for all scripts
        const result = schedulerService.registerOnce({
            handler: "test",
            runAt: new Date(Date.now() + 60000).toISOString()
        });
        if (!result || result.length === 0) {
            throw new Error("registerOnce should return a result");
        }
    "#;

    let result = execute_script_secure("test://non-priv-sched", script, admin);
    assert!(result.success, "Should be available: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_scheduler_register_recurring_available_for_any_script() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://non-priv-recur", "").expect("Failed to create script");

    let script = r#"
        if (typeof schedulerService === "undefined") {
            throw new Error("schedulerService should be defined");
        }
        // registerRecurring should be available for all scripts
        const result = schedulerService.registerRecurring({
            handler: "test",
            intervalMinutes: 60
        });
        if (!result || result.length === 0) {
            throw new Error("registerRecurring should return a result");
        }
    "#;

    let result = execute_script_secure("test://non-priv-recur", script, admin);
    assert!(result.success, "Should be available: {:?}", result.error);
}

// ============================================================================
// Public API Tests - Convert
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_btoa_available_for_all() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        if (typeof convert === "undefined") {
            throw new Error("convert object should be defined");
        }
        if (typeof convert.btoa !== "function") {
            throw new Error("btoa should be a function, got: " + typeof convert.btoa);
        }
        const encoded = convert.btoa("Hello World");
        if (!encoded || encoded.length === 0) {
            throw new Error("btoa() should return encoded string");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(
        result.success,
        "convert.btoa() should be public: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_atob_available_for_all() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        if (typeof convert === "undefined") {
            throw new Error("convert object should be defined");
        }
        if (typeof convert.atob !== "function") {
            throw new Error("atob should be a function, got: " + typeof convert.atob);
        }
        const decoded = convert.atob("SGVsbG8gV29ybGQ=");
        if (decoded !== "Hello World") {
            throw new Error("atob() should decode correctly");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(
        result.success,
        "convert.atob() should be public: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_markdown_to_html_available_for_all() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r##"
        const html = convert.markdown_to_html("# Hello");
        if (!html || html.length === 0) {
            throw new Error("markdown_to_html() should be available");
        }
    "##;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(
        result.success,
        "convert.markdown_to_html() should be public: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_convert_render_handlebars_available_for_all() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        const result = convert.render_handlebars_template(
            "Hello {{name}}",
            JSON.stringify({ name: "World" })
        );
        if (result !== "Hello World") {
            throw new Error("render_handlebars_template() should be available");
        }
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(
        result.success,
        "convert.render_handlebars_template() should be public: {:?}",
        result.error
    );
}

// ============================================================================
// Public API Tests - Console logging
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_console_logging_available_for_all() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    let script = r#"
        // Basic console methods should be available to all
        console.log("test");
        console.error("error");
        console.warn("warning");
        console.info("info");
        // Should not throw errors
    "#;

    let result = execute_script_secure("test://api-test", script, user);
    assert!(
        result.success,
        "Console logging should be public: {:?}",
        result.error
    );
}

// ============================================================================
// Route Registration Tests - Reserved Prefix Policy
//
// Any script may register routes/streams/asset routes on non-reserved paths;
// paths under the engine-owned prefixes (/health, /graphql, /mcp, /auth,
// /.well-known, /engine) are rejected regardless of script privilege.
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_register_route_allowed_for_any_script() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![Capability::WriteScripts]);

    repository::upsert_script("test://any-script-routes", "").expect("Failed to create script");

    let script = r#"
        routeRegistry.registerRoute("/test", "handler", "GET");
        // OAuth2 now lives entirely under /auth, so the top-level names it
        // used to occupy are available to solution developers.
        routeRegistry.registerRoute("/token", "handler", "GET");
        routeRegistry.registerRoute("/authorize", "handler", "GET");
        routeRegistry.registerRoute("/oauth2/token", "handler", "GET");
        // Should not throw
    "#;

    let result = execute_script_secure("test://any-script-routes", script, user);
    assert!(
        result.success,
        "Any script should be able to register non-reserved routes: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_route_denied_for_reserved_path() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![Capability::WriteScripts]);

    repository::upsert_script("test://reserved-path-routes", "").expect("Failed to create script");

    let script = r#"
        const reserved = ["/engine/fake", "/health", "/graphql", "/mcp", "/auth/login", "/auth/oauth2/token", "/.well-known/x"];
        for (const path of reserved) {
            try {
                routeRegistry.registerRoute(path, "handler", "GET");
                throw new Error("Should have been denied: " + path);
            } catch (e) {
                if (!e.message.includes("reserved")) {
                    throw e;
                }
            }
        }
    "#;

    let result = execute_script_secure("test://reserved-path-routes", script, user);
    assert!(
        result.success,
        "Reserved paths should be denied for route registration: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_stream_route_allowed_for_any_script() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![Capability::ManageStreams]);

    repository::upsert_script("test://any-script-streams", "").expect("Failed to create script");

    let script = r#"
        if (typeof routeRegistry === "undefined" || typeof routeRegistry.registerStreamRoute !== "function") {
            throw new Error("routeRegistry.registerStreamRoute should be defined");
        }
        routeRegistry.registerStreamRoute("/test-stream-any-script");
        // Should not throw
    "#;

    let result = execute_script_secure("test://any-script-streams", script, user);
    assert!(
        result.success,
        "Any script should be able to register non-reserved stream routes: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_stream_route_denied_for_reserved_path() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![Capability::ManageStreams]);

    repository::upsert_script("test://reserved-path-streams", "").expect("Failed to create script");

    let script = r#"
        try {
            routeRegistry.registerStreamRoute("/engine/fake-stream");
            throw new Error("Should have been denied");
        } catch (e) {
            if (!e.message.includes("reserved")) {
                throw e;
            }
        }
    "#;

    let result = execute_script_secure("test://reserved-path-streams", script, user);
    assert!(
        result.success,
        "Reserved paths should be denied for stream registration: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_asset_route_denied_for_reserved_path() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![Capability::WriteAssets]);

    repository::upsert_script("test://reserved-path-assets", "").expect("Failed to create script");

    let script = r#"
        try {
            routeRegistry.registerAssetRoute("/engine/fake.css", "test.css");
            throw new Error("Should have been denied");
        } catch (e) {
            if (!e.message.includes("reserved")) {
                throw e;
            }
        }
    "#;

    let result = execute_script_secure("test://reserved-path-assets", script, user);
    assert!(
        result.success,
        "Reserved paths should be denied for asset route registration: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_routes_allowed_for_any_script() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://register-routes", "").expect("Failed to create script");

    let script = r#"
        // Any script should be able to register routes
        routeRegistry.registerRoute("/test-priv", "handler", "GET");
        routeRegistry.registerStreamRoute("/test-stream-priv");
        routeRegistry.registerAssetRoute("/test-priv.css", "test.css");
        // Should not throw errors
    "#;

    let result = execute_script_secure("test://register-routes", script, admin);
    assert!(
        result.success,
        "Script should register routes: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_route_registry_list_routes_includes_stream_and_asset_routes() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://route-introspection", "").expect("Failed to create script");
    repository::upsert_asset(repository::Asset {
        uri: "test-introspection.css".to_string(),
        mimetype: "text/css".to_string(),
        content: b"body { color: red; }".to_vec(),
        name: Some("test-introspection.css".to_string()),
        script_uri: "test://route-introspection".to_string(),
        created_at: std::time::SystemTime::now(),
        updated_at: std::time::SystemTime::now(),
    })
    .expect("Failed to create asset for route introspection test");

    let script = r#"
        routeRegistry.registerStreamRoute("/test-introspection-stream", "streamCustomizer");
        routeRegistry.registerAssetRoute("/test-introspection.css", "test-introspection.css");

        const routes = JSON.parse(routeRegistry.listRoutes());
        const streamRoute = routes.find(
            (route) => route.path === "/test-introspection-stream" && route.method === "STREAM"
        );
        const assetRoute = routes.find(
            (route) => route.path === "/test-introspection.css" && route.method === "ASSET"
        );

        if (!streamRoute) {
            throw new Error("Expected stream route in listRoutes output");
        }

        if (streamRoute.handler !== "streamCustomizer") {
            throw new Error("Expected stream customization handler in listRoutes output");
        }

        if (!assetRoute) {
            throw new Error("Expected asset route in listRoutes output");
        }

        if (assetRoute.handler !== "test-introspection.css") {
            throw new Error("Expected asset name in listRoutes output");
        }
    "#;

    let result = execute_script_secure("test://route-introspection", script, admin);
    assert!(
        result.success,
        "Script should see stream routes in listRoutes: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_stream_message_requires_manage_streams() {
    setup_env().await;
    let user = create_user_with_capabilities("stream-user", vec![]);

    repository::upsert_script("test://stream-msg-no-cap", "").expect("Failed to create script");

    // The function is available to every script; the ManageStreams capability of
    // the calling user is what decides the outcome.
    let script = r#"
        if (typeof routeRegistry === "undefined" || typeof routeRegistry.sendStreamMessage !== "function") {
            throw new Error("routeRegistry.sendStreamMessage should be defined");
        }
        const result = routeRegistry.sendStreamMessage("/stream", "message");
        if (!result.startsWith("Error:")) {
            throw new Error("Expected capability error, got: " + result);
        }
    "#;

    let result = execute_script_secure("test://stream-msg-no-cap", script, user);
    assert!(result.success, "Should be denied: {:?}", result.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_stream_message_filtered_accepts_optional_match_mode() {
    setup_env().await;
    if database::get_global_database().is_none() {
        return;
    }
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://filtered-stream-msg", "").expect("Failed to create script");

    let script = r#"
        const result = routeRegistry.sendStreamMessageFiltered(
            "/missing-stream",
            JSON.stringify({ kind: "test" }),
            JSON.stringify({ recipient_id: "a" }),
            "overlap"
        );

        if (typeof result !== "string") {
            throw new Error("Expected string result from sendStreamMessageFiltered");
        }
    "#;

    let result = execute_script_secure("test://filtered-stream-msg", script, admin);
    assert!(
        result.success,
        "Script should accept optional match mode: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_stream_message_filtered_rejects_invalid_match_mode() {
    setup_env().await;
    if database::get_global_database().is_none() {
        return;
    }
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://filtered-stream-msg-invalid", "")
        .expect("Failed to create script");

    let script = r#"
        routeRegistry.sendStreamMessageFiltered(
            "/missing-stream",
            JSON.stringify({ kind: "test" }),
            JSON.stringify({ recipient_id: "a" }),
            "invalid-mode"
        );
    "#;

    let result = execute_script_secure("test://filtered-stream-msg-invalid", script, admin);
    assert!(!result.success, "Invalid match mode should fail");
    assert!(
        result
            .error
            .unwrap_or_default()
            .contains("Expected 'subset' or 'overlap'"),
        "Expected invalid match mode error"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_send_subscription_message_filtered_accepts_optional_match_mode() {
    setup_env().await;
    if database::get_global_database().is_none() {
        return;
    }
    let admin = UserContext::admin("admin".to_string());

    repository::upsert_script("test://filtered-subscription-msg", "")
        .expect("Failed to create script");

    let script = r#"
        const result = graphQLRegistry.sendSubscriptionMessageFiltered(
            "missingSubscription",
            JSON.stringify({ kind: "test" }),
            JSON.stringify({ recipient_id: "a" }),
            "overlap"
        );

        if (typeof result !== "string") {
            throw new Error("Expected string result from sendSubscriptionMessageFiltered");
        }
    "#;

    let result = execute_script_secure("test://filtered-subscription-msg", script, admin);
    assert!(
        result.success,
        "Script should accept optional subscription match mode: {:?}",
        result.error
    );
}

/// The engine's script-update stream lives under the reserved `/engine` prefix
/// so a script cannot register a stream on it. The stream registry replaces a
/// registration that has no active connections, so an unreserved path would let
/// any script take ownership of the engine's stream.
#[tokio::test(flavor = "multi_thread")]
async fn test_engine_script_updates_stream_cannot_be_claimed() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![Capability::ManageStreams]);

    repository::upsert_script("test://engine-stream-claim", "").expect("Failed to create script");

    let script = r#"
        try {
            routeRegistry.registerStreamRoute("/engine/script_updates");
            throw new Error("Should have been denied");
        } catch (e) {
            if (!e.message.includes("reserved")) {
                throw e;
            }
        }
    "#;

    let result = execute_script_secure("test://engine-stream-claim", script, user);
    assert!(
        result.success,
        "The engine script-updates stream must be unclaimable: {:?}",
        result.error
    );
}

/// Broadcasting used to skip the `ManageStreams` check for `/script_updates`,
/// which let a script with no capabilities forge engine script-change
/// notifications to every subscriber. Only the shared `/system/` namespace is
/// exempt now.
#[tokio::test(flavor = "multi_thread")]
async fn test_script_update_broadcast_requires_capability() {
    setup_env().await;
    let user = create_user_with_capabilities("user", vec![]);

    repository::upsert_script("test://forge-script-updates", "").expect("Failed to create script");

    let script = r#"
        const forged = JSON.stringify({ type: "script_update", uri: "victim.js", action: "deleted" });
        const engineStream = routeRegistry.sendStreamMessage("/engine/script_updates", forged);
        if (!engineStream.startsWith("Error:")) {
            throw new Error("Engine stream broadcast should be denied, got: " + engineStream);
        }
        const legacyPath = routeRegistry.sendStreamMessage("/script_updates", forged);
        if (!legacyPath.startsWith("Error:")) {
            throw new Error("Legacy path broadcast should be denied, got: " + legacyPath);
        }
    "#;

    let result = execute_script_secure("test://forge-script-updates", script, user);
    assert!(
        result.success,
        "Broadcasting script updates without ManageStreams must be denied: {:?}",
        result.error
    );
}

/// Both GraphQL subscription publish paths require `ManageGraphQL`.
///
/// They broadcast to the same `/engine/graphql/subscription/{name}` stream, and
/// a null filter matches every connection, so exempting the filtered variant
/// made it a drop-in bypass of the check on the unfiltered one.
#[tokio::test(flavor = "multi_thread")]
async fn test_subscription_publish_requires_capability_on_both_paths() {
    setup_env().await;
    if database::get_global_database().is_none() {
        return;
    }
    let user = create_user_with_capabilities("user", vec![]);

    repository::upsert_script("test://subscription-publish-authz", "")
        .expect("Failed to create script");

    let script = r#"
        const payload = JSON.stringify({ kind: "forged" });

        const plain = graphQLRegistry.sendSubscriptionMessage("someSubscription", payload);
        if (!plain.startsWith("Error:")) {
            throw new Error("sendSubscriptionMessage should be denied, got: " + plain);
        }

        // Null filter: matches every connection, same reach as above.
        const filtered = graphQLRegistry.sendSubscriptionMessageFiltered(
            "someSubscription",
            payload,
            null,
            null
        );
        if (!filtered.startsWith("Error:")) {
            throw new Error("sendSubscriptionMessageFiltered should be denied, got: " + filtered);
        }
    "#;

    let result = execute_script_secure("test://subscription-publish-authz", script, user);
    assert!(
        result.success,
        "Subscription publishing without ManageGraphQL must be denied on both paths: {:?}",
        result.error
    );
}
