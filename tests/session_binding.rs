//! What a session is bound to, and what a mismatch means.
//!
//! The rule used to unpick itself: a User-Agent mismatch was forgiven for a
//! caller whose User-Agent *said* it was an editor or an MCP client, and any
//! mismatch at all was forgiven once the address had changed — so the more of
//! the fingerprint differed, the more likely the session was accepted. Both
//! halves are set by whoever holds the token.
//!
//! What replaces it: a browser session is bound to its client, an API token is
//! bound by its audience and realm instead, and the address is binding for
//! either when the operator says so.

mod common;

use aiwebengine::security::{CreateSessionParams, SecureSessionManager, SecurityAuditor};
use common::{setup_env, should_skip_integration_tests};
use std::sync::Arc;

const HOST: &str = "binding.example.com";
const IP: &str = "192.0.2.10";
const OTHER_IP: &str = "198.51.100.20";
const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";
const SOMETHING_ELSE: &str = "curl/8.4.0";

async fn manager(strict: bool) -> SecureSessionManager {
    setup_env().await;
    let pool = aiwebengine::database::get_global_database()
        .expect("the suite's database should be up")
        .pool()
        .clone();
    let key: [u8; 32] = rand::random();
    let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
    SecureSessionManager::new(pool, &key, 3600, 86400 * 30, 3, auditor)
        .expect("session manager should build")
        .with_strict_ip_validation(strict)
}

fn params(audience: Option<&str>) -> CreateSessionParams {
    CreateSessionParams {
        user_id: format!("binding-{}", uuid::Uuid::new_v4()),
        provider: "google".to_string(),
        email: None,
        name: None,
        is_admin: false,
        is_editor: false,
        ip_addr: IP.to_string(),
        user_agent: BROWSER.to_string(),
        refresh_token: None,
        audience: audience.map(str::to_string),
        realm: HOST.to_string(),
    }
}

/// The inversion. A cookie presented by a different client from a different
/// address was accepted, because the changed address was read as evidence of a
/// roaming user rather than of a stolen token.
#[tokio::test(flavor = "multi_thread")]
async fn a_cookie_presented_by_another_client_is_refused_wherever_it_comes_from() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager(false).await;
    let session = manager
        .create_session(params(None))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, IP, SOMETHING_ELSE, HOST)
            .await
            .is_err(),
        "a browser does not change its User-Agent mid-session"
    );

    assert!(
        manager
            .validate_session(&session.token, OTHER_IP, SOMETHING_ELSE, HOST)
            .await
            .is_err(),
        "and changing address as well must not be what makes it acceptable"
    );
}

/// The usability half, which is why the address is not binding by default: a
/// phone changing networks keeps its session.
#[tokio::test(flavor = "multi_thread")]
async fn a_cookie_survives_a_change_of_address() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager(false).await;
    let session = manager
        .create_session(params(None))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, OTHER_IP, BROWSER, HOST)
            .await
            .is_ok(),
        "same client, new network"
    );
}

/// An API token is bound by its audience and its realm rather than by a
/// User-Agent it reports itself: an OAuth client legitimately runs the token
/// exchange in one HTTP stack and its API calls in another.
#[tokio::test(flavor = "multi_thread")]
async fn an_api_token_is_not_pinned_to_a_user_agent() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager(false).await;
    let session = manager
        .create_session(params(Some("binding.example.com/mcp")))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, IP, SOMETHING_ELSE, HOST)
            .await
            .is_ok(),
        "the client that exchanged the code is not the one that calls the API"
    );
}

/// Configured strictness, now that an address is established from the
/// connection rather than read out of a header the caller wrote.
#[tokio::test(flavor = "multi_thread")]
async fn strict_validation_holds_a_session_to_the_address_it_was_minted_from() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager(true).await;
    let session = manager
        .create_session(params(None))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, IP, BROWSER, HOST)
            .await
            .is_ok(),
        "where it was minted still works"
    );
    assert!(
        manager
            .validate_session(&session.token, OTHER_IP, BROWSER, HOST)
            .await
            .is_err(),
        "and nowhere else does"
    );
}

/// Strictness applies to an API token too. Its audience says where it may be
/// used, not from where.
#[tokio::test(flavor = "multi_thread")]
async fn strict_validation_covers_api_tokens() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager(true).await;
    let session = manager
        .create_session(params(Some("binding.example.com/mcp")))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, OTHER_IP, SOMETHING_ELSE, HOST)
            .await
            .is_err()
    );
}
