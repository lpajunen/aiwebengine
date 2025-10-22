// Test script for manager.js
// This verifies that the manager script loads and initializes correctly

use aiwebengine::{js_engine, repository, security::UserContext};

#[test]
fn test_manager_script_loads() {
    // Ensure manager script is in the repository
    let script = repository::fetch_script("https://example.com/manager");
    assert!(script.is_some(), "Manager script should be in repository");

    let script_content = script.unwrap();
    assert!(
        script_content.contains("function init("),
        "Manager script should have init function"
    );
    assert!(
        script_content.contains("handleManagerUI"),
        "Manager script should have handleManagerUI function"
    );
    assert!(
        script_content.contains("handleListUsers"),
        "Manager script should have handleListUsers function"
    );
    assert!(
        script_content.contains("handleUpdateUserRole"),
        "Manager script should have handleUpdateUserRole function"
    );
}

#[test]
fn test_manager_script_executes() {
    let script_uri = "https://example.com/manager";
    let script_content =
        repository::fetch_script(script_uri).expect("Manager script should be in repository");

    // Execute the script with admin user context
    let result = js_engine::execute_script_secure(
        script_uri,
        &script_content,
        UserContext::admin("test_admin".to_string()),
    );

    assert!(
        result.success,
        "Manager script should execute successfully: {:?}",
        result.error
    );
}

#[test]
fn test_manager_script_init() {
    use aiwebengine::script_init;

    let script_uri = "https://example.com/manager";

    // Initialize the script
    let init_context = script_init::InitContext::new(script_uri.to_string(), false);
    let registrations = js_engine::call_init_if_exists(
        script_uri,
        &repository::fetch_script(script_uri).unwrap(),
        init_context,
    );

    assert!(
        registrations.is_ok(),
        "Manager script init should succeed: {:?}",
        registrations.err()
    );
    let registrations = registrations.unwrap();
    assert!(
        registrations.is_some(),
        "Manager script should have init function"
    );

    let routes = registrations.unwrap();
    assert!(!routes.is_empty(), "Manager script should register routes");

    // Check that the manager route is registered
    assert!(
        routes.contains_key(&("/manager".to_string(), "GET".to_string())),
        "Manager script should register /manager GET route"
    );

    // Check that the API routes are registered
    assert!(
        routes.contains_key(&("/api/manager/users".to_string(), "GET".to_string())),
        "Manager script should register /api/manager/users GET route"
    );

    // Check for wildcard route
    assert!(
        routes.contains_key(&("/api/manager/users/*".to_string(), "POST".to_string())),
        "Manager script should register user role update wildcard route"
    );
}
