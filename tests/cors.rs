//! Which origins may read the engine's own responses.
//!
//! `security.enable_cors` and `security.cors_allowed_origins` were parsed and
//! read by nothing, so no response ever carried a CORS header and the startup
//! log said otherwise. These tests drive the policy over real HTTP, because
//! what matters is the header a browser sees.

mod common;

use std::time::Duration;

use aiwebengine::repository;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

const ALLOWED: &str = "https://admin.example.com";
const DENIED: &str = "https://elsewhere.example.com";

/// A script route, so the "the engine does not speak for scripts" half is
/// testable against something a script actually serves.
const SCRIPT: &str = r#"
function handler(context) {
  return { status: 200, body: "from the script" };
}

function init(context) {
  routeRegistry.registerRoute("/cors-script-route", "handler", "GET");
}
"#;

async fn engine() -> (TestContext, String) {
    let context = TestContext::new();
    let _ = repository::upsert_script("test_cors_script_route", SCRIPT);

    // The layer reads its allowlist once, at startup.
    let port = context
        .start_server_customized(|config| {
            config.security.enable_cors = true;
            config.security.cors_allowed_origins = vec![ALLOWED.to_string()];
        })
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    let base = format!("http://127.0.0.1:{}", port);
    (context, base)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client")
}

fn header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// An allowlisted origin is echoed back, and may send its session with it.
#[tokio::test(flavor = "multi_thread")]
async fn an_allowed_origin_may_read_an_engine_response() {
    if should_skip_integration_tests() {
        return;
    }
    let (context, base) = engine().await;

    let response = client()
        .get(format!("{}/engine/installed", base))
        .header("Origin", ALLOWED)
        .send()
        .await
        .expect("request");

    assert_eq!(
        header(&response, "access-control-allow-origin").as_deref(),
        Some(ALLOWED),
        "an allowlisted origin should be echoed"
    );
    assert_eq!(
        header(&response, "access-control-allow-credentials").as_deref(),
        Some("true"),
        "a named origin is the only form that can carry a session"
    );
    assert!(
        header(&response, "vary")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("origin"),
        "the response varies by origin and a shared cache has to know"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// An origin nobody listed gets no header, which is what makes the browser
/// refuse the read.
#[tokio::test(flavor = "multi_thread")]
async fn an_unlisted_origin_is_refused() {
    if should_skip_integration_tests() {
        return;
    }
    let (context, base) = engine().await;

    let response = client()
        .get(format!("{}/engine/installed", base))
        .header("Origin", DENIED)
        .send()
        .await
        .expect("request");

    assert!(
        header(&response, "access-control-allow-origin").is_none(),
        "an unlisted origin must not be told it is allowed"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// A preflight is answered by the engine rather than by the router.
///
/// The handler for this path takes GET, so an OPTIONS reaching it would be a
/// 405 and the browser would report a CORS failure. It also arrives without
/// credentials, so it has to be answered before anything that wants a session.
#[tokio::test(flavor = "multi_thread")]
async fn a_preflight_is_answered_without_credentials() {
    if should_skip_integration_tests() {
        return;
    }
    let (context, base) = engine().await;

    let response = client()
        .request(reqwest::Method::OPTIONS, format!("{}/engine/scripts", base))
        .header("Origin", ALLOWED)
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Headers", "content-type")
        .send()
        .await
        .expect("request");

    assert_eq!(
        response.status(),
        204,
        "a preflight is answered, not routed"
    );
    assert_eq!(
        header(&response, "access-control-allow-origin").as_deref(),
        Some(ALLOWED)
    );
    assert!(
        header(&response, "access-control-allow-methods")
            .unwrap_or_default()
            .contains("POST"),
        "the method the caller asked about should be allowed"
    );
    assert_eq!(
        header(&response, "access-control-allow-headers").as_deref(),
        Some("content-type"),
        "the requested headers are reflected for an allowed origin"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// A preflight from an origin nobody listed is answered, but allows nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_preflight_from_an_unlisted_origin_allows_nothing() {
    if should_skip_integration_tests() {
        return;
    }
    let (context, base) = engine().await;

    let response = client()
        .request(reqwest::Method::OPTIONS, format!("{}/engine/scripts", base))
        .header("Origin", DENIED)
        .header("Access-Control-Request-Method", "POST")
        .send()
        .await
        .expect("request");

    assert!(
        header(&response, "access-control-allow-origin").is_none(),
        "a refused preflight must not name an allowed origin"
    );
    assert!(
        header(&response, "access-control-allow-methods").is_none(),
        "nor a method"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// The engine does not speak for a script's responses.
///
/// A solution serving a public API knows its callers; the engine does not, and
/// a policy applied on its behalf would either break it or over-permit it.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_route_is_left_to_the_script() {
    if should_skip_integration_tests() {
        return;
    }
    let (context, base) = engine().await;

    let response = client()
        .get(format!("{}/cors-script-route", base))
        .header("Origin", ALLOWED)
        .send()
        .await
        .expect("request");

    assert_eq!(response.status(), 200, "the script still answers");
    assert!(
        header(&response, "access-control-allow-origin").is_none(),
        "the engine must not add a policy to a response a script wrote"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// The OAuth2 token endpoint keeps the wide policy its clients need.
///
/// Browser-based MCP clients reach it from origins nobody can enumerate, and a
/// PKCE code exchange carries no cookie. Narrowing it to the engine's allowlist
/// would break them, so the layer it installs for itself has to win.
#[tokio::test(flavor = "multi_thread")]
async fn the_oauth_token_endpoint_keeps_its_own_policy() {
    if should_skip_integration_tests() {
        return;
    }
    // Needs auth enabled: without it the OAuth2 router — and the CORS layer it
    // installs for itself — is never mounted.
    let server = common::TestServer::start_with_auth_customized(|config| {
        config.security.enable_cors = true;
        config.security.cors_allowed_origins = vec![ALLOWED.to_string()];
    })
    .await
    .expect("server failed to start");
    let base = format!("http://127.0.0.1:{}", server.port());
    wait_for_server(server.port(), 20)
        .await
        .expect("Server not ready");

    let response = client()
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/auth/oauth2/token", base),
        )
        .header("Origin", DENIED)
        .header("Access-Control-Request-Method", "POST")
        .send()
        .await
        .expect("request");

    assert_eq!(
        header(&response, "access-control-allow-origin").as_deref(),
        Some("*"),
        "an MCP client from an unenumerable origin must still reach the token endpoint"
    );

    server.shutdown().await;
}
