//! Switching authority on deliberately, and what it takes to do it.
//!
//! `security::elevation` pins the arithmetic and `security::session` pins the
//! round trip. What these cover is the composition end to end: that a session
//! at the floor is refused the thing its account may do, that elevating puts
//! it back, that elevating takes proof of presence, and that the ceiling is
//! still the repository's.
//!
//! They drive `SecureSessionManager` and `UserContext::for_session` directly
//! rather than an HTTP server, because what is under test is the decision
//! rather than the page — and the page is `delegate_page` with the nouns
//! changed, which already has its own coverage.

mod common;

use aiwebengine::security::elevation::{self, Elevation, Grade, Method, Policy};
use aiwebengine::security::{
    Capability, CreateSessionParams, SecureSessionManager, SecurityAuditor, SessionRoles,
    UserContext,
};
use chrono::{Duration, Utc};
use std::sync::Arc;

const TEST_HOST: &str = "test.example.com";

async fn manager() -> SecureSessionManager {
    let key: [u8; 32] = rand::random();
    let pool = common::test_pool().await;
    let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
    SecureSessionManager::new(pool, &key, 3600, 86400 * 30, 5, auditor).unwrap()
}

fn params(user: &str) -> CreateSessionParams {
    CreateSessionParams {
        user_id: user.to_string(),
        provider: "internal".to_string(),
        email: None,
        name: None,
        is_admin: true,
        is_editor: true,
        ip_addr: "192.168.1.1".to_string(),
        user_agent: "Mozilla/5.0".to_string(),
        refresh_token: None,
        audience: None,
        realm: TEST_HOST.to_string(),
        elevation: None,
    }
}

/// The policy an operator turns on first: the dangerous half gated, the rest
/// left alone.
fn gating_administer() -> Policy {
    Policy {
        gated: vec![Grade::Administer],
        ..Policy::default()
    }
}

/// An administrator's session, freshly minted, does not administer anything —
/// and still does everything an editor does, because gating one bundle must
/// not disturb the other.
#[tokio::test]
async fn a_new_session_starts_at_the_floor() {
    let manager = manager().await;
    let token = manager.create_session(params("floor")).await.unwrap();
    let session = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
        .await
        .unwrap();

    assert!(session.elevation.is_none());
    assert!(
        session.reauthenticated_at.is_some(),
        "signing in is itself proof of presence"
    );

    let held = elevation::held_under(
        &gating_administer(),
        &UserContext::admin(session.user_id.clone()),
        session.elevation.as_ref(),
        Utc::now(),
    );

    assert!(!held.contains(&Capability::AdministerEngine));
    assert!(held.contains(&Capability::WriteScripts));
    assert!(held.contains(&Capability::ReadScripts));
}

/// Switching it on, reading it back through the credential, and handing it
/// back. The whole loop the endpoints are made of.
#[tokio::test]
async fn an_elevation_is_switched_on_read_back_and_handed_back() {
    let manager = manager().await;
    let token = manager.create_session(params("loop")).await.unwrap();
    let now = Utc::now();

    manager
        .set_elevation(
            &token.token,
            Some(Elevation::grant(
                &[Grade::Administer],
                Method::Password,
                30,
                now,
            )),
        )
        .await
        .unwrap();

    let elevated = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
        .await
        .unwrap();
    let held = elevation::held_under(
        &gating_administer(),
        &UserContext::admin(elevated.user_id.clone()),
        elevated.elevation.as_ref(),
        now,
    );
    assert!(held.contains(&Capability::AdministerEngine));

    // Handing it back is a decision, not a wait.
    manager.set_elevation(&token.token, None).await.unwrap();
    let dropped = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
        .await
        .unwrap();
    assert!(dropped.elevation.is_none());

    let held = elevation::held_under(
        &gating_administer(),
        &UserContext::admin(dropped.user_id.clone()),
        dropped.elevation.as_ref(),
        now,
    );
    assert!(!held.contains(&Capability::AdministerEngine));
}

/// The session outlives the elevation. Running out of the one must not sign
/// anybody out of the other — that is the difference between a step-up and a
/// shorter session.
#[tokio::test]
async fn an_elevation_expires_without_ending_the_session() {
    let manager = manager().await;
    let token = manager.create_session(params("expiry")).await.unwrap();
    let now = Utc::now();

    manager
        .set_elevation(
            &token.token,
            Some(Elevation {
                capabilities: vec![Capability::AdministerEngine.as_str().to_string()],
                granted_at: now - Duration::hours(2),
                expires_at: now - Duration::minutes(1),
                method: Method::Password,
            }),
        )
        .await
        .unwrap();

    let session = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
        .await
        .expect("a spent elevation must not end the session");

    assert_eq!(session.user_id, "expiry");
    assert!(
        session.elevation.is_none(),
        "validation should have dropped the spent elevation rather than leaving it stored"
    );
}

/// Using a session does not extend what it switched on.
///
/// The session's own expiry slides on use so that somebody working is not
/// signed out. Sliding the elevation with it would mean an agent looping every
/// thirty seconds holds an administrator's authority for as long as it keeps
/// looping, which is the thing this exists to stop.
#[tokio::test]
async fn using_a_session_does_not_extend_its_elevation() {
    let manager = manager().await;
    let token = manager.create_session(params("absolute")).await.unwrap();
    let now = Utc::now();
    let granted = Elevation::grant(&[Grade::Administer], Method::Password, 30, now);
    let expires_at = granted.expires_at;

    manager
        .set_elevation(&token.token, Some(granted))
        .await
        .unwrap();

    for _ in 0..3 {
        let session = manager
            .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
            .await
            .unwrap();
        assert_eq!(
            session.elevation.expect("still live").expires_at,
            expires_at,
            "the elevation window is absolute; only the session's slides"
        );
    }
}

/// Ending an account's sessions takes its elevations with them.
///
/// The reason there is no elevations table: `delete_sessions_for_user` already
/// runs when roles change, a realm narrows, an account is deleted or a
/// password is changed, and an elevation living inside the session is revoked
/// by every one of those with no second statement to remember.
#[tokio::test]
async fn ending_a_session_takes_its_elevation_with_it() {
    let pool = common::test_pool().await;
    let manager = manager().await;
    let token = manager.create_session(params("revoked")).await.unwrap();

    manager
        .set_elevation(
            &token.token,
            Some(Elevation::grant(
                &[Grade::Administer],
                Method::Password,
                30,
                Utc::now(),
            )),
        )
        .await
        .unwrap();

    aiwebengine::security::delete_sessions_for_user(&pool, "revoked")
        .await
        .unwrap();

    assert!(
        manager
            .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
            .await
            .is_err(),
        "the session and everything it carried should be gone"
    );
}

/// The ceiling stays in the repository. Writing an elevation into an ordinary
/// account's session does not make it an administrator, because the
/// composition is an intersection with the tier.
#[tokio::test]
async fn an_elevation_cannot_lift_an_account_past_its_roles() {
    let manager = manager().await;
    let mut ordinary = params("ordinary");
    ordinary.is_admin = false;
    ordinary.is_editor = false;
    let token = manager.create_session(ordinary).await.unwrap();
    let now = Utc::now();

    manager
        .set_elevation(
            &token.token,
            Some(Elevation {
                capabilities: vec![
                    Capability::AdministerEngine.as_str().to_string(),
                    Capability::WriteScripts.as_str().to_string(),
                ],
                granted_at: now,
                expires_at: now + Duration::hours(1),
                method: Method::Password,
            }),
        )
        .await
        .unwrap();

    let session = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
        .await
        .unwrap();

    let context = UserContext::for_session_at(
        SessionRoles {
            user_id: Some(&session.user_id),
            is_admin: session.is_admin,
            is_editor: session.is_editor,
            elevation: session.elevation.as_ref(),
        },
        now,
    );

    assert!(!context.has_capability(&Capability::AdministerEngine));
    assert!(!context.has_capability(&Capability::WriteScripts));
    assert!(context.has_capability(&Capability::ReadScripts));
}

/// Proving presence is recorded on the session, and it is what the elevation
/// endpoint reads before granting anything to an account with no password.
#[tokio::test]
async fn re_authenticating_is_recorded_on_the_session() {
    let manager = manager().await;
    let token = manager.create_session(params("presence")).await.unwrap();

    let before = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
        .await
        .unwrap()
        .reauthenticated_at
        .expect("minting a session records it");

    manager.mark_reauthenticated(&token.token).await.unwrap();

    let after = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0", TEST_HOST)
        .await
        .unwrap()
        .reauthenticated_at
        .expect("re-authenticating records it again");

    assert!(after >= before);
}
