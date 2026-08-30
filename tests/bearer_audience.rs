//! Tests for what a session token is allowed to be presented as.
//!
//! A session cookie and an API bearer token are different credentials that
//! happen to share a representation. The cookie is bound to a host by the
//! browser; a bearer token is bound by nothing, so a session established on one
//! host reaches every host the process serves unless something says otherwise.
//!
//! What says otherwise is the audience. The OAuth2 token endpoint puts one on
//! every token it issues; a browser login has none. Requiring one at a
//! resource-scoped endpoint is therefore what stops a cookie from working as an
//! API credential, and matching it on host is what stops a token minted for a
//! solution from reaching the management surface.

mod common;

use aiwebengine::security::{CreateSessionParams, SecureSessionManager, SecurityAuditor};
use common::{setup_env, should_skip_integration_tests};
use std::sync::Arc;

const IP: &str = "192.168.1.1";
const UA: &str = "test-agent";
/// The host these sessions belong to. Realm scoping is exercised in
/// `tests/realms.rs`; here it is held constant so the audience is what varies.
const HOST: &str = "game.example.com";

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

fn params(audience: Option<&str>) -> CreateSessionParams {
    CreateSessionParams {
        user_id: format!("audience-{}", uuid::Uuid::new_v4()),
        provider: "guest".to_string(),
        email: None,
        name: None,
        is_admin: false,
        is_editor: false,
        ip_addr: IP.to_string(),
        user_agent: UA.to_string(),
        refresh_token: None,
        audience: audience.map(str::to_string),
        realm: HOST.to_string(),
    }
}

/// The finding this change closes. A browser login carries no audience, so
/// presenting that cookie as a bearer token at `/mcp` must be refused — the
/// cookie's host scoping is the only thing bounding it, and a bearer header
/// discards that.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_with_no_audience_is_not_an_api_credential() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager().await;

    let cookie_session = manager
        .create_session(params(None))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session_with_resource(
                &cookie_session.token,
                IP,
                UA,
                HOST,
                Some("game.example.com/mcp"),
            )
            .await
            .is_err(),
        "a browser session must not be usable as a bearer token"
    );

    // It is still a perfectly good session for the browser it was minted for.
    assert!(
        manager
            .validate_session_with_resource(&cookie_session.token, IP, UA, HOST, None)
            .await
            .is_ok(),
        "the paths that are not resource-scoped are unaffected"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_minted_for_one_host_does_not_reach_another() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager().await;

    let token = manager
        .create_session(params(Some("https://game.example.com/mcp")))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session_with_resource(
                &token.token,
                IP,
                UA,
                HOST,
                Some("game.example.com/mcp")
            )
            .await
            .is_ok(),
        "the host it was issued for still works"
    );

    assert!(
        manager
            .validate_session_with_resource(
                &token.token,
                IP,
                UA,
                HOST,
                Some("manage.example.com/mcp")
            )
            .await
            .is_err(),
        "the management host is a different resource and must refuse it"
    );
}

/// Clients write resource indicators as absolute URIs; the engine names the
/// requested resource as host plus path. Both must be understood as the same
/// endpoint, or enforcement would reject every legitimate client.
#[tokio::test(flavor = "multi_thread")]
async fn an_absolute_uri_and_a_host_qualified_path_are_one_resource() {
    if should_skip_integration_tests() {
        return;
    }
    let manager = manager().await;

    let token = manager
        .create_session(params(Some("https://game.example.com:443/mcp/")))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session_with_resource(
                &token.token,
                IP,
                UA,
                HOST,
                Some("game.example.com/mcp")
            )
            .await
            .is_ok(),
        "scheme, default port and trailing slash are not part of the name"
    );
}
