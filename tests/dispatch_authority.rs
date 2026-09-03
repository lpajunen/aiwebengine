//! What a dispatched message listener is allowed to do.
//!
//! `dispatcher.sendMessage` runs another script's handler, and it is reachable
//! from inside any script serving any request. The listener used to run as
//! `UserContext::admin("dispatcher")`, so passing through it turned whatever
//! the caller held into the full administrator set — `ManageScriptDatabase`,
//! `WriteAssets`, `AdministerEngine`. A listener is part of serving the
//! invocation that dispatched to it, so it holds what the sender held.

mod common;

use std::time::Duration;

use aiwebengine::repository;
use common::{AdminServer, TestContext, should_skip_integration_tests, wait_for_server};

/// A listener attempts a schema change, which needs `ManageScriptDatabase` — a
/// capability an editor holds and a solution's users do not.
///
/// The listener reports by throwing rather than by logging: writing to the
/// script log is itself capability-gated, so in the very case under test the
/// log is not a channel the listener has. What every caller does see is the
/// summary `sendMessage` returns, which counts a throwing listener as failed.
///
/// The message type is per-test because the dispatcher registry is keyed by it
/// and every script in the shared test database registers at startup — two
/// tests sharing a type would each set off the other's listener.
fn script(message_type: &str, path: &str) -> String {
    format!(
        r#"
function onMessage(context) {{
  const result = database.createTable("dispatch_authority_probe");
  if (String(result).indexOf("Insufficient permissions") >= 0) {{
    throw new Error("listener lacks schema authority");
  }}
}}

function handler(context) {{
  const summary = dispatcher.sendMessage("{message_type}", JSON.stringify({{}}));
  return {{ status: 200, body: summary }};
}}

function init(context) {{
  dispatcher.registerListener("{message_type}", "onMessage");
  routeRegistry.registerRoute("{path}", "handler", "GET");
}}
"#
    )
}

/// An anonymous caller cannot reach a schema change by dispatching to a
/// listener that makes one.
#[tokio::test(flavor = "multi_thread")]
async fn a_listener_does_not_hold_more_than_the_caller_that_dispatched_it() {
    if should_skip_integration_tests() {
        return;
    }

    let context = TestContext::new();
    let uri = "test_dispatch_authority_anonymous";
    let path = "/dispatch-authority/anonymous";
    let _ = repository::upsert_script(uri, &script("dispatch.authority.anonymous", path));

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client")
        .get(format!("http://127.0.0.1:{}{}", port, path))
        .send()
        .await
        .expect("request should reach the server");
    assert_eq!(
        response.status(),
        200,
        "the dispatching route still answers"
    );
    let summary = response.text().await.expect("body");

    // On the count rather than the exact tally: what matters here is that no
    // invocation got the schema through, whatever the listener count is. That
    // the count is one is `dispatch_registration.rs`'s subject.
    assert!(
        summary.contains("0 successful"),
        "an anonymous caller must not gain schema powers by dispatching a \
         message; the dispatch reported: {summary}"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// The other half: the listener is not gratuitously stripped either. An
/// administrator's request dispatches a listener that still holds what the
/// administrator holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_listener_keeps_what_the_caller_that_dispatched_it_holds() {
    if should_skip_integration_tests() {
        return;
    }

    common::setup_env().await;
    let uri = "test_dispatch_authority_admin";
    let path = "/dispatch-authority/admin";
    let _ = repository::upsert_script(uri, &script("dispatch.authority.admin", path));

    let engine = AdminServer::start().await.expect("server failed to start");

    let response = engine
        .client()
        .get(engine.url(path))
        .send()
        .await
        .expect("request should reach the server");
    assert_eq!(response.status(), 200);
    let summary = response.text().await.expect("body");

    assert!(
        summary.contains("0 failed") && !summary.contains("0 successful"),
        "an administrator's dispatch should carry their own authority into the \
         listener; the dispatch reported: {summary}"
    );

    engine.shutdown().await;
}
