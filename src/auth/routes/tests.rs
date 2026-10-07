use super::*;
use crate::security::client_ip;
use axum::http::HeaderValue;
use axum::http::{HeaderMap, header};
use sqlx::PgPool;

#[test]
fn test_get_user_agent() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        HeaderValue::from_static("Mozilla/5.0 Test"),
    );

    let ua = client_ip::user_agent_from_headers(&headers);
    assert_eq!(ua, "Mozilla/5.0 Test");
}

fn authorize_params(state: Option<&str>) -> AuthorizeParams {
    AuthorizeParams {
        response_type: "code".to_string(),
        client_id: "client-1".to_string(),
        redirect_uri: Some("http://127.0.0.1:6274/callback".to_string()),
        scope: None,
        state: state.map(|s| s.to_string()),
        code_challenge: Some("abc123".to_string()),
        code_challenge_method: Some("S256".to_string()),
        resource: None,
    }
}

/// The login bounce has to return the caller to the path the authorization
/// endpoint is actually mounted at. It once pointed at the withdrawn
/// `/authorize` alias, which sent everyone who had to log in to a 404.
#[test]
fn test_authorize_return_url_uses_the_mounted_path() {
    let url = authorize_return_url(&authorize_params(Some("xyz")));

    assert!(
        url.starts_with(&format!("{}?", AUTHORIZE_PATH)),
        "return URL {} should start with the mounted authorize path",
        url
    );
    assert_eq!(
        safe_redirect_target(Some(&url)),
        url,
        "must survive sanitisation"
    );
}

#[test]
fn test_authorize_return_url_encodes_and_skips_empty_params() {
    let url = authorize_return_url(&authorize_params(None));

    assert!(url.contains("response_type=code"));
    assert!(url.contains("client_id=client-1"));
    assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A6274%2Fcallback"));
    assert!(url.contains("code_challenge=abc123"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(
        !url.contains("state="),
        "absent state should be omitted: {}",
        url
    );
    assert!(
        !url.contains("scope="),
        "absent scope should be omitted: {}",
        url
    );
}

#[test]
fn test_get_request_host_normalises_case() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::HOST,
        HeaderValue::from_static("Manage.Softagen.Com"),
    );

    assert_eq!(
        get_request_host(&headers),
        Some("manage.softagen.com".to_string())
    );
}

#[test]
fn test_get_request_host_keeps_port() {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));

    assert_eq!(
        get_request_host(&headers),
        Some("localhost:3000".to_string())
    );
}

#[test]
fn test_get_request_host_absent() {
    assert_eq!(get_request_host(&HeaderMap::new()), None);
}

#[test]
fn test_safe_redirect_target_keeps_relative_paths() {
    assert_eq!(
        safe_redirect_target(Some("/engine/installed")),
        "/engine/installed"
    );
    assert_eq!(safe_redirect_target(Some("/a?b=c#d")), "/a?b=c#d");
}

#[test]
fn test_safe_redirect_target_rejects_other_origins() {
    // Absolute, protocol-relative and backslash forms would all take a
    // freshly authenticated user off the host that just set their cookie.
    for hostile in [
        "https://evil.test/",
        "http://evil.test/",
        "//evil.test/",
        "/\\evil.test/",
        "/ok\r\nLocation: https://evil.test/",
    ] {
        assert_eq!(
            safe_redirect_target(Some(hostile)),
            "/",
            "expected '{}' to be rejected",
            hostile
        );
    }
}

#[test]
fn test_safe_redirect_target_defaults_to_root() {
    assert_eq!(safe_redirect_target(None), "/");
    assert_eq!(safe_redirect_target(Some("")), "/");
}

// ---- The authorization endpoint's gate ----
//
// These cover the finding this endpoint was rebuilt for: it validated
// `response_type`, checked that `client_id` was non-empty, and stopped.
// Any site could navigate a signed-in browser to it and be handed an
// authorization code redirected wherever it asked.

use crate::auth::client_registration::{ClientRegistrationManager, ClientRegistrationRequest};
use crate::hosts::HostConfig;

fn test_pool() -> PgPool {
    crate::test_db::pool()
}

/// Register a public client with one redirect URI, the way an MCP client
/// does, and return its identifier.
async fn register_test_client(redirect_uris: Vec<String>) -> String {
    let manager = ClientRegistrationManager::new(90, test_pool());
    let response = manager
        .register_client(ClientRegistrationRequest {
            redirect_uris,
            client_name: Some("Test Client".to_string()),
            logo_uri: None,
            client_uri: None,
            contacts: None,
            tos_uri: None,
            policy_uri: None,
            token_endpoint_auth_method: Some("none".to_string()),
            grant_types: vec!["authorization_code".to_string()],
            response_types: vec!["code".to_string()],
            scope: None,
        })
        .await
        .expect("registration should succeed");
    response.client_id
}

/// A challenge of the shape a real S256 client sends: base64url of a
/// 32-byte digest, so 43 characters.
fn valid_challenge() -> String {
    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_string()
}

fn valid_params(client_id: &str, redirect_uri: &str) -> AuthorizeParams {
    AuthorizeParams {
        response_type: "code".to_string(),
        client_id: client_id.to_string(),
        redirect_uri: Some(redirect_uri.to_string()),
        scope: None,
        state: Some("opaque-state".to_string()),
        code_challenge: Some(valid_challenge()),
        code_challenge_method: Some("S256".to_string()),
        resource: None,
    }
}

/// The failure that stopped every MCP client from connecting: the client
/// asks for the origin, and an origin authorizes nothing, because an
/// audience is matched on host *and* path.
#[test]
fn a_resource_naming_only_a_host_becomes_that_host_s_mcp_endpoint() {
    assert_eq!(
        resource_audience("https://softagen.com/"),
        "softagen.com/mcp"
    );
    assert_eq!(
        resource_audience("https://softagen.com"),
        "softagen.com/mcp"
    );
    assert_eq!(resource_audience("softagen.com"), "softagen.com/mcp");
}

/// A resource that already names an endpoint is left as it is — narrowing
/// is for the case where there is nothing to narrow to, never a way to move
/// a token from the endpoint it was asked for.
#[test]
fn a_resource_that_names_an_endpoint_keeps_it() {
    assert_eq!(
        resource_audience("https://softagen.com/mcp"),
        "softagen.com/mcp"
    );
    assert_eq!(
        resource_audience("https://softagen.com/health"),
        "softagen.com/health"
    );
    assert_eq!(
        resource_audience("https://MANAGE.softagen.com:443/mcp/"),
        "manage.softagen.com/mcp"
    );
}

fn rejection_error(rejection: &AuthorizeRejection) -> &str {
    match rejection {
        AuthorizeRejection::Direct { error, .. } => error,
        AuthorizeRejection::Redirect { error, .. } => error,
    }
}

fn is_direct(rejection: &AuthorizeRejection) -> bool {
    matches!(rejection, AuthorizeRejection::Direct { .. })
}

#[tokio::test]
async fn a_client_id_that_was_never_registered_is_refused() {
    let pool = test_pool();
    let params = valid_params("client_never-registered", "https://example.com/cb");

    let rejection = validate_authorize_request(&pool, &params, None)
        .await
        .expect_err("an unregistered client must not reach the consent page");

    assert_eq!(rejection_error(&rejection), "invalid_client");
    assert!(
        is_direct(&rejection),
        "the answer must go to the browser, never to a redirect URI no client registered"
    );
}

/// The heart of it. A client exists, but the request names a redirect URI
/// that client never registered — which is how an authorization code ends
/// up at an attacker's server.
#[tokio::test]
async fn a_redirect_uri_the_client_did_not_register_is_refused() {
    let client_id = register_test_client(vec!["http://127.0.0.1:6274/callback".to_string()]).await;
    let pool = test_pool();
    let params = valid_params(&client_id, "https://attacker.example/cb");

    let rejection = validate_authorize_request(&pool, &params, None)
        .await
        .expect_err("an unregistered redirect URI must be refused");

    assert_eq!(rejection_error(&rejection), "invalid_request");
    assert!(
        is_direct(&rejection),
        "refusing by redirecting to the unregistered URI would tell it what it wanted to know"
    );
}

/// Simple string comparison, per RFC 6749 §3.1.2.3. A URI that differs by a
/// trailing slash, a case-folded path, or an extra segment is a different
/// URI, and normalising before comparing is how an allowlist develops
/// holes.
#[tokio::test]
async fn redirect_uri_matching_is_exact() {
    let registered = "http://127.0.0.1:6274/callback";
    let client_id = register_test_client(vec![registered.to_string()]).await;
    let pool = test_pool();

    for near_miss in [
        "http://127.0.0.1:6274/callback/",
        "http://127.0.0.1:6274/Callback",
        "http://127.0.0.1:6274/callback/../callback",
        "http://127.0.0.1:6274/callback?next=https://attacker.example",
        "https://127.0.0.1:6274/callback",
    ] {
        let params = valid_params(&client_id, near_miss);
        assert!(
            validate_authorize_request(&pool, &params, None)
                .await
                .is_err(),
            "{:?} is not the registered URI and must be refused",
            near_miss
        );
    }

    let params = valid_params(&client_id, registered);
    assert!(
        validate_authorize_request(&pool, &params, None)
            .await
            .is_ok(),
        "the registered URI itself must still work"
    );
}

/// PKCE was verified only when a challenge happened to have been stored, so
/// a caller who sent none was never asked for a verifier.
#[tokio::test]
async fn a_request_without_a_code_challenge_is_refused() {
    let redirect = "http://127.0.0.1:6274/callback";
    let client_id = register_test_client(vec![redirect.to_string()]).await;
    let pool = test_pool();

    let mut params = valid_params(&client_id, redirect);
    params.code_challenge = None;

    let rejection = validate_authorize_request(&pool, &params, None)
        .await
        .expect_err("PKCE is required, not optional");

    assert_eq!(rejection_error(&rejection), "invalid_request");
    assert!(
        !is_direct(&rejection),
        "the client and its redirect URI checked out, so this error is the client's to handle"
    );
}

#[tokio::test]
async fn plain_pkce_is_refused() {
    let redirect = "http://127.0.0.1:6274/callback";
    let client_id = register_test_client(vec![redirect.to_string()]).await;
    let pool = test_pool();

    let mut params = valid_params(&client_id, redirect);
    params.code_challenge_method = Some("plain".to_string());

    assert!(
        validate_authorize_request(&pool, &params, None)
            .await
            .is_err(),
        "a plain challenge travels in the same query string as the verifier would"
    );
}

#[tokio::test]
async fn a_client_without_the_authorization_code_grant_is_refused() {
    let redirect = "http://127.0.0.1:6274/callback";
    let manager = ClientRegistrationManager::new(90, test_pool());
    let client_id = manager
        .register_client(ClientRegistrationRequest {
            redirect_uris: vec![redirect.to_string()],
            client_name: Some("Refresh Only".to_string()),
            logo_uri: None,
            client_uri: None,
            contacts: None,
            tos_uri: None,
            policy_uri: None,
            token_endpoint_auth_method: Some("none".to_string()),
            grant_types: vec!["refresh_token".to_string()],
            response_types: vec!["code".to_string()],
            scope: None,
        })
        .await
        .expect("registration should succeed")
        .client_id;

    let pool = test_pool();
    let params = valid_params(&client_id, redirect);

    let rejection = validate_authorize_request(&pool, &params, None)
        .await
        .expect_err("a client that did not register this grant cannot use it");
    assert_eq!(rejection_error(&rejection), "unauthorized_client");
}

// ---- Resource indicators ----

/// The token endpoint copies `resource` onto the session's audience
/// verbatim, so an unchecked one makes the audience mean nothing.
#[test]
fn a_resource_must_name_the_host_the_authorization_is_happening_on() {
    let hosts = HostConfig::new(
        "https://game.example.com",
        &["https://manage.example.com".to_string()],
    );

    assert!(
        resource_is_acceptable_for(
            "https://game.example.com/mcp",
            Some("game.example.com"),
            &hosts
        ),
        "the host the flow is running on is the one it may mint tokens for"
    );

    assert!(
        !resource_is_acceptable_for(
            "https://manage.example.com/mcp",
            Some("game.example.com"),
            &hosts
        ),
        "a sign-in on a solution host must not hand out a management-host credential"
    );

    assert!(
        !resource_is_acceptable_for(
            "https://attacker.example/mcp",
            Some("game.example.com"),
            &hosts
        ),
        "a resource this engine does not serve names nothing"
    );

    assert!(
        !resource_is_acceptable_for("", Some("game.example.com"), &hosts),
        "an empty resource names nothing either"
    );
}

/// A deployment that never set a base URL has nothing to check against, and
/// refusing everything would leave it unable to issue any token at all.
#[test]
fn an_unconfigured_engine_accepts_any_resource() {
    let hosts = HostConfig::default();
    assert!(resource_is_acceptable_for(
        "https://anything.example/mcp",
        None,
        &hosts
    ));
}

// ---- Challenge and verifier shapes ----

#[test]
fn code_challenge_shapes_are_checked() {
    assert!(code_challenge_is_wellformed(&valid_challenge()));
    assert!(
        !code_challenge_is_wellformed("too-short"),
        "below the RFC 7636 floor of 43"
    );
    assert!(
        !code_challenge_is_wellformed(&"a".repeat(129)),
        "above the RFC 7636 ceiling of 128"
    );
    assert!(
        !code_challenge_is_wellformed(&format!("{}+", &valid_challenge()[..42])),
        "'+' is standard base64, not base64url"
    );
}

#[test]
fn a_verifier_matches_only_its_own_challenge() {
    // The worked example from RFC 7636 Appendix B.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    assert!(pkce_verifier_matches(verifier, challenge, Some("S256")));
    assert!(!pkce_verifier_matches(
        "something else",
        challenge,
        Some("S256")
    ));
    assert!(
        !pkce_verifier_matches(challenge, challenge, Some("plain")),
        "plain is refused even when the verifier equals the challenge"
    );
    assert!(
        !pkce_verifier_matches(verifier, challenge, None),
        "a code stored without a method cannot be verified"
    );
}

// ---- Consent ----

#[test]
fn a_stored_grant_covers_only_what_it_named() {
    assert!(scope_is_covered(Some("read write"), Some("read")));
    assert!(scope_is_covered(Some("read write"), Some("read write")));
    assert!(scope_is_covered(None, None));
    assert!(
        !scope_is_covered(None, Some("read")),
        "a grant that named no scope covers no scope"
    );
    assert!(
        !scope_is_covered(Some("read"), Some("read write")),
        "widening must not happen without being seen"
    );
}

/// Approving once means the client is not re-approved every time — but
/// asking for more than was approved sends the person back to the page.
#[tokio::test]
async fn consent_is_remembered_until_the_request_widens() {
    let redirect = "http://127.0.0.1:6274/callback";
    let client_id = register_test_client(vec![redirect.to_string()]).await;
    let pool = test_pool();
    let user_id = format!("consent-{}", uuid::Uuid::new_v4());

    let mut params = valid_params(&client_id, redirect);
    params.scope = Some("read".to_string());
    let validated = validate_authorize_request(&pool, &params, None)
        .await
        .expect("a well-formed request should validate");

    assert!(
        !consent_already_given(&pool, &user_id, &validated)
            .await
            .expect("consent lookup should succeed"),
        "a client nobody approved must be approved"
    );

    record_consent(&pool, &user_id, &validated)
        .await
        .expect("recording consent should succeed");

    assert!(
        consent_already_given(&pool, &user_id, &validated)
            .await
            .expect("consent lookup should succeed"),
        "the same request again must not ask twice"
    );

    let mut wider = valid_params(&client_id, redirect);
    wider.scope = Some("read write".to_string());
    let wider = validate_authorize_request(&pool, &wider, None)
        .await
        .expect("a well-formed request should validate");

    assert!(
        !consent_already_given(&pool, &user_id, &wider)
            .await
            .expect("consent lookup should succeed"),
        "asking for a scope nobody approved must go back to the consent page"
    );

    let _ = sqlx::query("DELETE FROM oauth_client_grants WHERE user_id = $1")
        .bind(&user_id)
        .execute(&pool)
        .await;
}

// ---- Token endpoint client authentication ----

#[test]
fn client_credentials_are_read_from_a_basic_header_or_the_body() {
    use base64::Engine;

    let body_only = TokenParams {
        grant_type: "authorization_code".to_string(),
        code: None,
        redirect_uri: None,
        code_verifier: None,
        refresh_token: None,
        client_id: Some("from-body".to_string()),
        client_secret: Some("body-secret".to_string()),
    };
    assert_eq!(
        client_credentials(&HeaderMap::new(), &body_only),
        (
            Some("from-body".to_string()),
            Some("body-secret".to_string())
        )
    );

    let mut headers = HeaderMap::new();
    let encoded = base64::engine::general_purpose::STANDARD.encode("from-header:header-secret");
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Basic {}", encoded)).expect("valid header"),
    );
    assert_eq!(
        client_credentials(&headers, &body_only),
        (
            Some("from-header".to_string()),
            Some("header-secret".to_string())
        ),
        "RFC 6749 §2.3.1 prefers the header when both arrive"
    );
}

#[test]
fn a_client_secret_matches_only_itself() {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(b"the-secret"));

    assert!(crate::auth::client_registration::client_secret_matches(
        "the-secret",
        &hash
    ));
    assert!(!crate::auth::client_registration::client_secret_matches(
        "the-secre",
        &hash
    ));
    assert!(!crate::auth::client_registration::client_secret_matches(
        "", &hash
    ));
}
