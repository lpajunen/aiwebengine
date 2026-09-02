//! Tests for the native user-administration surface.
//!
//! Listing users and changing their roles is engine functionality, exposed over
//! HTTP (`/engine/users`, `/engine/user_roles`) and MCP (`list_users`,
//! `add_user_role`, `remove_user_role`); these tests cover the shared
//! authorization layer both entry points call into.

mod common;

use aiwebengine::engine_api::{
    UserAdminError, add_user_role_authorized, execute_native_mcp_tool, list_users_authorized,
    native_mcp_tool_descriptors, remove_user_role_authorized,
};
use aiwebengine::security::{Capability, UserContext};
use aiwebengine::user_repository::{self, UserRole};
use common::{AdminServer, setup_env, should_skip_integration_tests};
use serde_json::json;

/// Create a fresh user and return its id. The email is unique per call so
/// tests stay independent of each other and of the shared database.
async fn create_user(label: &str) -> String {
    let unique = uuid::Uuid::new_v4();
    user_repository::upsert_user(
        format!("{}-{}@example.com", label, unique),
        Some(format!("Test {}", label)),
        "test".to_string(),
        format!("{}-{}", label, unique),
        "test.example.com".to_string(),
    )
    .await
    .expect("failed to create test user")
}

fn admin() -> UserContext {
    UserContext::admin("test-admin".to_string())
}

// ============================================================================
// Authorization
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn list_users_denied_for_anonymous_and_authenticated() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    for user in [
        UserContext::anonymous(),
        UserContext::authenticated("plain-user".to_string()),
    ] {
        assert!(
            matches!(
                list_users_authorized(&user),
                Err(UserAdminError::AccessDenied)
            ),
            "non-admin must be denied outright, not handed an empty list"
        );
    }
}

/// Development mode grants anonymous callers `AdministerEngine`, the engine's
/// admin marker. User administration must not accept that on its own: it
/// requires an authenticated session as well.
#[tokio::test(flavor = "multi_thread")]
async fn list_users_denied_for_unauthenticated_holder_of_the_admin_capability() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let dev_anonymous = UserContext {
        user_id: None,
        is_authenticated: false,
        capabilities: [Capability::AdministerEngine, Capability::ReadScripts]
            .into_iter()
            .collect(),
    };

    assert!(matches!(
        list_users_authorized(&dev_anonymous),
        Err(UserAdminError::AccessDenied)
    ));
    assert!(matches!(
        add_user_role_authorized(&dev_anonymous, "someone", "Administrator"),
        Err(UserAdminError::AccessDenied)
    ));
    assert!(matches!(
        remove_user_role_authorized(&dev_anonymous, "someone", "Editor"),
        Err(UserAdminError::AccessDenied)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn list_users_returns_created_user_for_admin() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_user("listed").await;
    let users = list_users_authorized(&admin()).expect("admin may list users");

    let found = users
        .iter()
        .find(|u| u["id"] == json!(user_id))
        .expect("created user should appear in the listing");
    assert!(found["email"].is_string());
    assert_eq!(found["roles"], json!(["Authenticated"]));
    assert!(found["createdAt"].is_number());
}

#[tokio::test(flavor = "multi_thread")]
async fn role_changes_denied_for_non_admin() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_user("target").await;
    let caller = UserContext::authenticated("plain-user".to_string());

    assert!(matches!(
        add_user_role_authorized(&caller, &user_id, "Administrator"),
        Err(UserAdminError::AccessDenied)
    ));
    assert!(matches!(
        remove_user_role_authorized(&caller, &user_id, "Editor"),
        Err(UserAdminError::AccessDenied)
    ));

    // The denial must not have changed anything.
    let stored = user_repository::get_user(&user_id).expect("user should still exist");
    assert!(!stored.has_role(&UserRole::Administrator));
}

// ============================================================================
// Role mutation
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn add_and_remove_editor_role_round_trips() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_user("editor").await;

    let roles = add_user_role_authorized(&admin(), &user_id, "Editor").expect("grant should work");
    assert!(roles.contains(&"Editor".to_string()));
    assert!(
        user_repository::get_user(&user_id)
            .expect("user exists")
            .has_role(&UserRole::Editor)
    );

    // Granting a role the user already holds is a no-op, not an error.
    let roles =
        add_user_role_authorized(&admin(), &user_id, "Editor").expect("regrant should be a no-op");
    assert_eq!(
        roles.iter().filter(|r| *r == "Editor").count(),
        1,
        "regranting must not duplicate the role"
    );

    let roles = remove_user_role_authorized(&admin(), &user_id, "Editor").expect("revoke works");
    assert!(!roles.contains(&"Editor".to_string()));
    assert!(roles.contains(&"Authenticated".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_role_and_unknown_user_are_distinguished() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_user("errors").await;

    match add_user_role_authorized(&admin(), &user_id, "Wizard") {
        Err(UserAdminError::Validation(message)) => assert!(message.contains("Wizard")),
        other => panic!(
            "expected a validation error for an unknown role, got {:?}",
            other
        ),
    }

    match add_user_role_authorized(&admin(), "no-such-user-id", "Editor") {
        Err(UserAdminError::UserNotFound(id)) => assert_eq!(id, "no-such-user-id"),
        _ => panic!("expected UserNotFound for an unknown user id"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_role_cannot_be_revoked() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_user("authenticated").await;

    match remove_user_role_authorized(&admin(), &user_id, "Authenticated") {
        Err(UserAdminError::Validation(message)) => {
            assert!(message.contains("Authenticated"));
        }
        _ => panic!("removing the Authenticated role must be rejected"),
    }
}

/// The last-administrator guard only blocks the *final* admin, so with two
/// administrators present a revocation must still succeed.
#[tokio::test(flavor = "multi_thread")]
async fn administrator_can_be_revoked_while_another_remains() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let keeper = create_user("admin-keeper").await;
    let revoked = create_user("admin-revoked").await;
    add_user_role_authorized(&admin(), &keeper, "Administrator").expect("grant keeper");
    add_user_role_authorized(&admin(), &revoked, "Administrator").expect("grant revoked");

    let roles =
        remove_user_role_authorized(&admin(), &revoked, "Administrator").expect("revoke works");
    assert!(!roles.contains(&"Administrator".to_string()));
    assert!(
        user_repository::get_user(&keeper)
            .expect("keeper exists")
            .has_role(&UserRole::Administrator),
        "the remaining administrator must be untouched"
    );
}

// ============================================================================
// MCP surface
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn user_tools_are_exposed_over_mcp() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let names: Vec<&str> = native_mcp_tool_descriptors()
        .iter()
        .map(|d| d.name)
        .collect();
    for expected in ["list_users", "add_user_role", "remove_user_role"] {
        assert!(
            names.contains(&expected),
            "MCP tools/list should advertise {}",
            expected
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_user_tools_enforce_admin_and_report_errors() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let caller = UserContext::authenticated("plain-user".to_string());
    let result = execute_native_mcp_tool("list_users", &json!({}), &caller)
        .expect("list_users is a native tool");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Administrator"),
        "non-admin should get a permission error, not a user list"
    );
    assert!(result["users"].is_null());

    // Missing arguments are reported rather than silently ignored.
    let result = execute_native_mcp_tool("add_user_role", &json!({ "role": "Editor" }), &admin())
        .expect("add_user_role is a native tool");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("user_id")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_add_user_role_grants_for_admin() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_user("mcp-grant").await;
    let result = execute_native_mcp_tool(
        "add_user_role",
        &json!({ "user_id": user_id, "role": "Editor" }),
        &admin(),
    )
    .expect("add_user_role is a native tool");

    assert_eq!(result["success"], json!(true));
    assert_eq!(result["roles"], json!(["Authenticated", "Editor"]));
}

// ============================================================================
// HTTP surface
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn user_endpoints_reject_unauthenticated_callers() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.anonymous().clone();
    let base = format!("http://127.0.0.1:{}", port);

    let response = client
        .get(format!("{}/engine/users", base))
        .send()
        .await
        .expect("users request failed");
    assert_eq!(
        response.status(),
        403,
        "the user directory must not be readable without an admin session"
    );

    let response = client
        .post(format!("{}/engine/user_roles", base))
        .json(&json!({ "user_id": "someone", "role": "Administrator" }))
        .send()
        .await
        .expect("user_roles request failed");
    assert_eq!(response.status(), 403);

    let response = client
        .delete(format!(
            "{}/engine/user_roles?user_id=someone&role=Administrator",
            base
        ))
        .send()
        .await
        .expect("user_roles delete failed");
    assert_eq!(response.status(), 403);

    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn user_role_endpoint_reports_missing_parameters() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let client = engine.client();
    let base = format!("http://127.0.0.1:{}", port);

    // Parameter validation precedes the authorization check, same as the
    // script_owners and secrets endpoints.
    let response = client
        .post(format!("{}/engine/user_roles", base))
        .json(&json!({ "role": "Editor" }))
        .send()
        .await
        .expect("user_roles request failed");
    assert_eq!(response.status(), 400);
    let body: serde_json::Value = response.json().await.expect("response not JSON");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("user_id")
    );

    let response = client
        .delete(format!("{}/engine/user_roles?user_id=someone", base))
        .send()
        .await
        .expect("user_roles delete failed");
    assert_eq!(response.status(), 400);

    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn user_endpoints_are_documented_in_openapi() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    let spec: serde_json::Value =
        reqwest::get(format!("http://127.0.0.1:{}/engine/openapi.json", port))
            .await
            .expect("openapi request failed")
            .json()
            .await
            .expect("openapi spec not JSON");

    let paths = spec["paths"].as_object().expect("spec should have paths");
    assert!(paths["/engine/users"].get("get").is_some());
    assert!(paths["/engine/user_roles"].get("post").is_some());
    assert!(paths["/engine/user_roles"].get("delete").is_some());

    engine.shutdown().await;
}

// ============================================================================
// Roles on sessions minted by the OAuth token exchange
// ============================================================================

/// The `/auth/token` exchange used to hardcode `is_admin: false`, so a Bearer
/// token obtained through the PKCE flow never reached the administrator-only
/// engine APIs even for an administrator. The roles must come from the user
/// repository, like they do for a browser login.
#[tokio::test(flavor = "multi_thread")]
async fn session_identity_carries_repository_roles() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_user("session-identity").await;
    let plain = aiwebengine::auth::routes::session_identity_for_user(&user_id).await;
    assert!(!plain.is_admin, "a new user is not an administrator");
    assert!(!plain.is_editor, "a new user is not an editor");
    assert!(plain.email.is_some(), "identity should carry the email");

    user_repository::add_user_role(&user_id, UserRole::Administrator)
        .expect("failed to grant Administrator");
    user_repository::add_user_role(&user_id, UserRole::Editor).expect("failed to grant Editor");

    let elevated = aiwebengine::auth::routes::session_identity_for_user(&user_id).await;
    assert!(
        elevated.is_admin,
        "an Administrator's session must be marked admin"
    );
    assert!(
        elevated.is_editor,
        "an Editor's session must be marked editor"
    );
}

/// A user the repository cannot resolve gets a session with no roles rather
/// than one that guesses at them.
#[tokio::test(flavor = "multi_thread")]
async fn session_identity_for_unknown_user_has_no_roles() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let identity = aiwebengine::auth::routes::session_identity_for_user("no-such-user-id").await;
    assert_eq!(
        identity,
        aiwebengine::auth::routes::SessionIdentity::default()
    );
}
