//! Tests for where an identity is a principal.
//!
//! A session cookie is scoped to a host by the browser. A session token in an
//! `Authorization: Bearer` header is scoped by nothing, and both the engine API
//! middleware and the MCP endpoint accept one. So without a realm, an account
//! created by a solution's sign-up form authenticated on every host the process
//! serves — including a management host the cookie would never have reached.
//!
//! Capabilities bound what such an account could do. The realm is what bounds
//! where it exists.

mod common;

use aiwebengine::engine_api::{UserAdminError, set_user_realm_authorized};
use aiwebengine::security::{
    CreateSessionParams, SecureSessionManager, SecurityAuditor, UserContext,
};
use aiwebengine::user_repository::{self, GLOBAL_REALM, realm_authorizes_host};
use common::{setup_env, should_skip_integration_tests};
use std::sync::Arc;

const IP: &str = "192.168.1.1";
const UA: &str = "test-agent";
const GAME: &str = "game.example.com";
const MANAGE: &str = "manage.example.com";

async fn manager() -> SecureSessionManager {
    setup_env().await;
    let pool = aiwebengine::database::get_global_database()
        .expect("the suite's database should be up")
        .pool()
        .clone();
    let key: [u8; 32] = rand::random();
    let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
    SecureSessionManager::new(pool, &key, 3600, 86400 * 30, 3, auditor)
        .expect("session manager should build")
}

fn params(realm: &str) -> CreateSessionParams {
    CreateSessionParams {
        user_id: format!("realm-{}", uuid::Uuid::new_v4()),
        provider: "guest".to_string(),
        email: None,
        name: None,
        is_admin: false,
        is_editor: false,
        ip_addr: IP.to_string(),
        user_agent: UA.to_string(),
        refresh_token: None,
        audience: None,
        realm: realm.to_string(),
    }
}

// ============================================================================
// The rule
// ============================================================================

#[test]
fn a_realm_authorizes_its_own_host_and_no_other() {
    assert!(realm_authorizes_host(GAME, GAME));
    assert!(!realm_authorizes_host(GAME, MANAGE));
}

#[test]
fn the_global_realm_authorizes_every_host() {
    assert!(realm_authorizes_host(GLOBAL_REALM, GAME));
    assert!(realm_authorizes_host(GLOBAL_REALM, MANAGE));
}

/// A column added to bound accounts must not default to unbounded. Rows that
/// predate realms carry the empty string, and the next sign-in records a host —
/// so this costs a re-authentication, not an account.
#[test]
fn a_realm_that_was_never_recorded_authorizes_nothing() {
    assert!(!realm_authorizes_host("", GAME));
    assert!(!realm_authorizes_host("", MANAGE));
    assert!(!realm_authorizes_host("", ""));
}

#[test]
fn hosts_are_matched_without_regard_to_case() {
    assert!(realm_authorizes_host(GAME, "GAME.example.com"));
}

// ============================================================================
// Enforcement on sessions
// ============================================================================

/// The finding this closes: a session established by a solution's sign-up must
/// not authenticate on the management host, whatever header it arrives in.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_does_not_authenticate_outside_its_realm() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager().await;

    let session = manager
        .create_session(params(GAME))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, IP, UA, GAME)
            .await
            .is_ok(),
        "the host it was established on still works"
    );

    assert!(
        manager
            .validate_session(&session.token, IP, UA, MANAGE)
            .await
            .is_err(),
        "another host must refuse it, however the token was presented"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_global_session_authenticates_anywhere() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager().await;

    let session = manager
        .create_session(params(GLOBAL_REALM))
        .await
        .expect("session should be created");

    for host in [GAME, MANAGE] {
        assert!(
            manager
                .validate_session(&session.token, IP, UA, host)
                .await
                .is_ok(),
            "the global realm should authenticate on {}",
            host
        );
    }
}

/// Refreshing reads and rewrites a session without going through
/// `validate_session`, so a session close enough to expiry to be renewed would
/// otherwise be the one way onto a host it does not authenticate on.
#[tokio::test(flavor = "multi_thread")]
async fn refreshing_a_session_does_not_escape_its_realm() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager().await;

    let session = manager
        .create_session(params(GAME))
        .await
        .expect("session should be created");

    assert!(
        manager
            .refresh_session(&session.token, IP, UA, MANAGE, None)
            .await
            .is_err(),
        "refresh must apply the same rule validation does"
    );

    assert!(
        manager
            .refresh_session(&session.token, IP, UA, GAME, None)
            .await
            .is_ok(),
        "and must still work where the session belongs"
    );
}

// ============================================================================
// Where the realm comes from
// ============================================================================

/// An account anyone can create must not become a principal everywhere by
/// being created.
#[tokio::test(flavor = "multi_thread")]
async fn a_self_minted_account_is_bound_to_the_host_it_was_created_on() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = user_repository::upsert_internal_user(
        Some("Player".to_string()),
        "guest".to_string(),
        uuid::Uuid::new_v4().to_string(),
        GAME.to_string(),
    )
    .await
    .expect("guest should be created");

    let user = user_repository::get_user_async(&user_id)
        .await
        .expect("guest should be readable");

    assert_eq!(user.realm, GAME);
    assert_ne!(
        user.realm, GLOBAL_REALM,
        "no sign-in path may produce the global realm"
    );
}

/// Signing in on another host must not move an account there — otherwise
/// re-homing an account is as easy as visiting a different URL.
#[tokio::test(flavor = "multi_thread")]
async fn signing_in_elsewhere_does_not_move_an_account() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let provider_user_id = uuid::Uuid::new_v4().to_string();
    let email = format!("realm-{}@example.com", provider_user_id);

    let user_id = user_repository::upsert_user(
        email.clone(),
        Some("Person".to_string()),
        "test".to_string(),
        provider_user_id.clone(),
        GAME.to_string(),
    )
    .await
    .expect("user should be created");

    // The same account signs in again, on a different host.
    let same_user = user_repository::upsert_user(
        email,
        Some("Person".to_string()),
        "test".to_string(),
        provider_user_id,
        MANAGE.to_string(),
    )
    .await
    .expect("second sign-in should succeed");

    assert_eq!(same_user, user_id, "it is the same account");

    let user = user_repository::get_user_async(&user_id)
        .await
        .expect("user should be readable");
    assert_eq!(
        user.realm, GAME,
        "the realm recorded at creation must survive a sign-in elsewhere"
    );
}

// ============================================================================
// Granting the global realm
// ============================================================================

/// `*` is unreachable except through an administrator. An operator who works
/// across hosts needs it, and it must stay a deliberate act rather than
/// something a sign-in can arrange for itself.
#[tokio::test(flavor = "multi_thread")]
async fn only_an_administrator_moves_a_user_between_realms() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = user_repository::upsert_internal_user(
        Some("Player".to_string()),
        "guest".to_string(),
        uuid::Uuid::new_v4().to_string(),
        GAME.to_string(),
    )
    .await
    .expect("guest should be created");

    for caller in [
        UserContext::anonymous(),
        UserContext::authenticated("someone".to_string()),
        UserContext::editor("an-author".to_string()),
    ] {
        assert!(
            matches!(
                set_user_realm_authorized(&caller, &user_id, GLOBAL_REALM),
                Err(UserAdminError::AccessDenied)
            ),
            "nobody below administrator may widen a realm"
        );
    }

    assert_eq!(
        set_user_realm_authorized(
            &UserContext::admin("root".to_string()),
            &user_id,
            GLOBAL_REALM
        )
        .expect("an administrator may"),
        GLOBAL_REALM
    );

    let user = user_repository::get_user_async(&user_id)
        .await
        .expect("user should be readable");
    assert_eq!(user.realm, GLOBAL_REALM);
    assert!(realm_authorizes_host(&user.realm, MANAGE));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_realm_cannot_be_set_to_nothing() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = user_repository::upsert_internal_user(
        Some("Player".to_string()),
        "guest".to_string(),
        uuid::Uuid::new_v4().to_string(),
        GAME.to_string(),
    )
    .await
    .expect("guest should be created");

    assert!(
        set_user_realm_authorized(&UserContext::admin("root".to_string()), &user_id, "   ")
            .is_err(),
        "an empty realm authorizes nothing, so setting one would be a silent lockout"
    );

    let user = user_repository::get_user_async(&user_id)
        .await
        .expect("user should be readable");
    assert_eq!(user.realm, GAME, "and the stored realm must be untouched");
}

// ============================================================================
// A realm change reaching the sessions that already exist
// ============================================================================

/// Narrowing a realm has to end the sessions the account is already holding.
///
/// The realm is stamped into a session when it is minted and `validate_session`
/// compares the request's host against that stamped copy, never against the
/// `users` row. So an `UPDATE` on its own would move an account from `*` to one
/// host while every session it holds kept authenticating everywhere for the
/// rest of its life — thirty days, by default. This is the same class the role
/// revocation closes, and it takes the same statement.
#[tokio::test(flavor = "multi_thread")]
async fn narrowing_a_realm_ends_the_sessions_it_already_granted() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager().await;

    let user_id = user_repository::upsert_internal_user(
        Some("Operator".to_string()),
        "guest".to_string(),
        uuid::Uuid::new_v4().to_string(),
        GAME.to_string(),
    )
    .await
    .expect("account should be created");

    set_user_realm_authorized(
        &UserContext::admin("root".to_string()),
        &user_id,
        GLOBAL_REALM,
    )
    .expect("an administrator may widen a realm");

    let session = manager
        .create_session(CreateSessionParams {
            user_id: user_id.clone(),
            realm: GLOBAL_REALM.to_string(),
            ..params(GLOBAL_REALM)
        })
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, IP, UA, MANAGE)
            .await
            .is_ok(),
        "the session reaches every host while the realm says it may"
    );

    set_user_realm_authorized(&UserContext::admin("root".to_string()), &user_id, GAME)
        .expect("an administrator may narrow a realm");

    assert!(
        manager
            .validate_session(&session.token, IP, UA, MANAGE)
            .await
            .is_err(),
        "and the session it granted must not outlive the grant"
    );
    assert!(
        manager
            .validate_session(&session.token, IP, UA, GAME)
            .await
            .is_err(),
        "the session is ended, not merely re-scoped"
    );
}
