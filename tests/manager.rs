// Test script for admin.js
// This verifies that the admin script loads and initializes correctly

use aiwebengine::{js_engine, repository, security::UserContext};
use std::sync::Once;

static INIT: Once = Once::new();

fn setup() {
    INIT.call_once(|| {
        // Initialize Repository to Memory FIRST to ensure we have scripts
        // This prevents get_repository() from defaulting to Postgres when GLOBAL_DATABASE is set
        repository::initialize_repository(repository::UnifiedRepository::new_memory());

        // Initialize DB for SecureGlobals
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = aiwebengine::config::AppConfig::test_config_with_port(0);
            if let Ok(db) = aiwebengine::database::Database::new(&config.repository).await {
                aiwebengine::database::initialize_global_database(std::sync::Arc::new(db));
            }
        });
    });
}

#[test]
fn test_manager_script_loads() {
    setup();
    // Ensure admin script is in the repository
    let script = repository::fetch_script("https://example.com/admin");
    assert!(script.is_some(), "Admin script should be in repository");

    let script_content = script.unwrap();
    assert!(
        script_content.contains("function init("),
        "Admin script should have init function"
    );
    assert!(
        script_content.contains("handleManagerUI"),
        "Admin script should have handleManagerUI function"
    );
    assert!(
        script_content.contains("handleListUsers"),
        "Admin script should have handleListUsers function"
    );
    assert!(
        script_content.contains("handleUpdateUserRole"),
        "Admin script should have handleUpdateUserRole function"
    );
}

#[test]
fn test_manager_script_executes() {
    setup();
    let script_uri = "https://example.com/admin";
    let script_content =
        repository::fetch_script(script_uri).expect("Admin script should be in repository");

    // Execute the script with admin user context
    let result = js_engine::execute_script_secure(
        script_uri,
        &script_content,
        UserContext::admin("test_admin".to_string()),
    );

    assert!(
        result.success,
        "Admin script should execute successfully: {:?}",
        result.error
    );
}

#[test]
fn test_manager_script_init() {
    setup();
    use aiwebengine::script_init;

    let script_uri = "https://example.com/admin";

    // Initialize the script
    let init_context = script_init::InitContext::new(script_uri.to_string(), false);
    let registrations = js_engine::call_init_if_exists(
        script_uri,
        &repository::fetch_script(script_uri).unwrap(),
        init_context,
    );

    assert!(
        registrations.is_ok(),
        "Admin script init should succeed: {:?}",
        registrations.err()
    );
    let registrations = registrations.unwrap();
    assert!(
        registrations.is_some(),
        "Admin script should have init function"
    );

    let routes = registrations.unwrap();
    assert!(!routes.is_empty(), "Admin script should register routes");

    // Check that the admin route is registered
    assert!(
        routes.contains_key(&("/engine/admin".to_string(), "GET".to_string())),
        "Admin script should register /engine/admin GET route"
    );

    // Check that the API routes are registered
    assert!(
        routes.contains_key(&("/api/engine/admin/users".to_string(), "GET".to_string())),
        "Admin script should register /api/engine/admin/users GET route"
    );

    // Check for wildcard route
    assert!(
        routes.contains_key(&("/api/engine/admin/users/*".to_string(), "POST".to_string())),
        "Admin script should register user role update wildcard route"
    );
}
