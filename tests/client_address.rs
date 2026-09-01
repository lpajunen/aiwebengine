//! Which address a request is taken to have come from.
//!
//! Rate limits and session fingerprints are keyed on it, and it used to be
//! whatever the caller wrote in `X-Forwarded-For`: an attacker rotating one
//! header got a fresh token bucket per request, and on a machine with nothing
//! in front of it every caller collapsed into the single string "unknown".
//!
//! These run against a real server because the thing under test is what a
//! request carries by the time anything reads it — the header is rewritten at
//! the edge, and only a real request has an edge to cross.

mod common;

use aiwebengine::repository;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

/// A handler that answers with the forwarding header as it reaches a script.
/// Scripts read the same header everything else does, so this is a faithful
/// window onto what the engine itself sees.
const PROBE: &str = r#"
    function handler(context) {
      return {
        status: 200,
        body: JSON.stringify({
          forwardedFor: context.request.headers.get("x-forwarded-for"),
          realIp: context.request.headers.get("x-real-ip"),
        }),
        contentType: "application/json",
      };
    }

    function init(context) {
      routeRegistry.registerRoute("/probe/client-address", "handler", "GET");
      return { success: true };
    }
"#;

async fn serve(context: &TestContext) -> String {
    let _ = repository::upsert_script("test_client_address", PROBE);
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    format!("http://127.0.0.1:{}", port)
}

/// The finding: nothing in `server.trusted_proxies`, so a forwarding header is
/// a claim by a stranger. It must not survive to anything that keys on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_forwarding_header_from_an_untrusted_caller_is_replaced() {
    if should_skip_integration_tests() {
        return;
    }

    let context = TestContext::new();
    let base = serve(&context).await;

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/probe/client-address", base))
        .header("X-Forwarded-For", "1.2.3.4")
        .header("X-Real-IP", "5.6.7.8")
        .send()
        .await
        .expect("request should be answered")
        .json()
        .await
        .expect("probe should answer with JSON");

    assert_eq!(
        body["forwardedFor"], "127.0.0.1",
        "the address the request actually came from, not the one it claimed"
    );
    assert_eq!(
        body["realIp"],
        serde_json::Value::Null,
        "the second claim is removed rather than left for someone to read"
    );

    context.cleanup().await.expect("cleanup should succeed");
}

/// The other half of it: a laptop with nothing in front of it used to key every
/// caller as "unknown", so one noisy client rate-limited everybody.
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_request_is_named_by_the_socket_it_arrived_on() {
    if should_skip_integration_tests() {
        return;
    }

    let context = TestContext::new();
    let base = serve(&context).await;

    let body: serde_json::Value = reqwest::get(format!("{}/probe/client-address", base))
        .await
        .expect("request should be answered")
        .json()
        .await
        .expect("probe should answer with JSON");

    assert_eq!(body["forwardedFor"], "127.0.0.1");

    context.cleanup().await.expect("cleanup should succeed");
}
