use aiwebengine::js_engine::execute_script_secure;
use aiwebengine::repository;
use aiwebengine::security::UserContext;
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
async fn test_secrets_list_returns_empty_without_manager() {
    setup_env().await;
    // Test that secretStorage.listForUri() returns empty array when no secrets are stored.
    // The script must be privileged to have listForUri available.
    repository::upsert_script("test://secrets-list", "").expect("Failed to create script");
    repository::set_script_privileged("test://secrets-list", true)
        .expect("Failed to set privileged");

    let script = r#"
        const result = secretStorage.listForUri('test://secrets-list');
        if (!Array.isArray(result)) {
            throw new Error('Expected array');
        }
        if (result.length !== 0) {
            throw new Error('Expected empty array when no secrets stored');
        }
    "#;

    let user_context = UserContext::admin("test".to_string());
    let result = execute_script_secure("test://secrets-list", script, user_context);

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
        const allowedMethods = [
            'exists', 'setSecret', 'removeSecret', 'clear',
            'listForUri', 'setSecretForUri', 'removeSecretForUri', 'clearForUri'
        ];
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
