use aiwebengine::engine_api::{
    SecretAccessError, clear_secrets_authorized, list_secrets_authorized, remove_secret_authorized,
    set_secret_authorized,
};
use aiwebengine::js_engine::execute_script_secure;
use aiwebengine::repository;
use aiwebengine::security::{Capability, UserContext};
use std::collections::HashSet;
use tokio::sync::OnceCell;

static INIT: OnceCell<()> = OnceCell::const_new();

async fn setup_env() {
    INIT.get_or_init(|| async {
        // Initialize DB first
        let config = aiwebengine::config::AppConfig::test_config_postgres(0);
        if let Ok(db) = aiwebengine::database::Database::new(&config.repository).await {
            let db_arc = std::sync::Arc::new(db);
            aiwebengine::database::initialize_global_database(db_arc.clone());

            // Initialize repository with PostgreSQL
            let repo =
                repository::PostgresRepository::new(db_arc.pool().clone(), "test".to_string());
            repository::initialize_repository(repo);
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secrets_exists_returns_false_without_manager() {
    setup_env().await;
    // Test that secretStorage.exists() returns false when no secrets manager is provided
    let script = r#"
        const result = secretStorage.exists('test_secret');
        if (result !== false) {
            throw new Error('Expected false when no secrets manager');
        }
    "#;

    let user_context = UserContext::admin("test".to_string());
    let result = execute_script_secure("test://secrets", script, user_context);

    assert!(
        result.success,
        "Script should execute successfully: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secrets_get_not_exposed() {
    setup_env().await;
    // Test that secretStorage.get() does NOT exist (security requirement)
    let script = r#"
        if (typeof secretStorage.get !== 'undefined') {
            throw new Error('secretStorage.get() should NOT be exposed to JavaScript');
        }
    "#;

    let user_context = UserContext::admin("test".to_string());
    let result = execute_script_secure("test://secrets", script, user_context);

    assert!(
        result.success,
        "Script should execute successfully: {:?}",
        result.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_secrets_cannot_access_values_directly() {
    setup_env().await;
    // Test that even with reflection tricks, secret values cannot be accessed
    let script = r#"
        // Try various tricks to access secret values
        try {
            // Try to call internal functions
            if (secretStorage.constructor) {
                throw new Error('Should not access constructor');
            }
        } catch (e) {
            // Expected - these should fail
        }
        
        // Verify only expected methods exist
        const allowedMethods = ['exists', 'setSecret', 'removeSecret', 'clear'];
        const actualMethods = Object.keys(secretStorage).filter(key => typeof secretStorage[key] === 'function');
        
        for (const method of actualMethods) {
            if (!allowedMethods.includes(method)) {
                throw new Error('Unexpected method exposed: ' + method);
            }
        }
    "#;

    let user_context = UserContext::admin("test".to_string());
    let result = execute_script_secure("test://secrets", script, user_context);

    assert!(
        result.success,
        "Script should execute successfully: {:?}",
        result.error
    );
}

// ============================================================================
// Cross-script secret management
//
// Secrets for another script are managed over `/engine/secrets` and the
// equivalent MCP tools, both of which authorize through the functions below:
// administrators and owners of the *target* script may manage its secrets.
// ============================================================================

fn user_without_capabilities(user_id: &str) -> UserContext {
    UserContext {
        user_id: Some(user_id.to_string()),
        is_authenticated: true,
        capabilities: HashSet::<Capability>::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cross_script_secrets_denied_without_admin_or_ownership() {
    setup_env().await;
    let outsider = user_without_capabilities("outsider");
    let target = "test://secret-foruri-outsider";
    repository::upsert_script(target, "").expect("Failed to create script");

    assert!(matches!(
        list_secrets_authorized(&outsider, target),
        Err(SecretAccessError::AccessDenied)
    ));
    assert!(matches!(
        set_secret_authorized(&outsider, target, "k", "v"),
        Err(SecretAccessError::AccessDenied)
    ));
    assert!(matches!(
        remove_secret_authorized(&outsider, target, "k"),
        Err(SecretAccessError::AccessDenied)
    ));
    assert!(matches!(
        clear_secrets_authorized(&outsider, target),
        Err(SecretAccessError::AccessDenied)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cross_script_secrets_allowed_for_admin() {
    setup_env().await;
    let admin = UserContext::admin("admin".to_string());
    let target = "test://secret-foruri-admin";
    repository::upsert_script(target, "").expect("Failed to create script");

    // An admin may manage the secrets of any script.
    list_secrets_authorized(&admin, target).expect("admin should list secret keys");
    set_secret_authorized(&admin, target, "test-key", "test-value")
        .expect("admin should store a secret");
    let keys = list_secrets_authorized(&admin, target).expect("admin should list secret keys");
    assert!(keys.contains(&"test-key".to_string()));
    assert!(
        remove_secret_authorized(&admin, target, "test-key").expect("admin should remove a secret")
    );
    clear_secrets_authorized(&admin, target).expect("admin should clear secrets");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cross_script_secrets_allowed_for_script_owner() {
    setup_env().await;
    let owner = user_without_capabilities("secret-owner");
    let target = "test://secret-foruri-owned";
    repository::upsert_script(target, "").expect("Failed to create script");
    repository::add_script_owner(target, "secret-owner").expect("Failed to add owner");

    // Ownership of the target script grants secret management, without admin.
    list_secrets_authorized(&owner, target).expect("owner should list secret keys");
    set_secret_authorized(&owner, target, "test-key", "test-value")
        .expect("owner should store a secret");
    clear_secrets_authorized(&owner, target).expect("owner should clear secrets");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cross_script_secret_ownership_does_not_leak_to_other_scripts() {
    setup_env().await;
    let owner = user_without_capabilities("partial-owner");
    let owned = "test://secret-owned-by-partial";
    let other = "test://secret-owned-by-nobody";
    repository::upsert_script(owned, "").expect("Failed to create script");
    repository::add_script_owner(owned, "partial-owner").expect("Failed to add owner");
    repository::upsert_script(other, "").expect("Failed to create script");

    // Owning one script must not grant access to another script's secrets.
    set_secret_authorized(&owner, owned, "k", "v").expect("owner may write to their own script");
    assert!(matches!(
        set_secret_authorized(&owner, other, "k", "v"),
        Err(SecretAccessError::AccessDenied)
    ));
    clear_secrets_authorized(&owner, owned).expect("owner should clear their own secrets");
}
