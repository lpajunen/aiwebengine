//! How many listeners a script has after it has been written more than once.
//!
//! The dispatcher appends listeners and never replaced them, while every
//! re-initialisation — startup, a local upsert, a peer's notification — runs
//! the script's program and its `init()` again. So a script registering a
//! listener ended up handling each message once per time it had been written
//! since the engine started, which makes a listener with a side effect run a
//! number of times that depends on the deploy history rather than on the
//! message.

mod common;

use std::time::Duration;

use aiwebengine::repository;
use common::{AdminServer, should_skip_integration_tests};

/// A script that dispatches to its own listener and reports the summary
/// `sendMessage` returns, which counts one invocation per registered listener.
///
/// `version` is echoed at the front of the body, so a test can tell the
/// re-initialised script from the one it replaced rather than guessing how
/// long the init spawned by the upsert takes.
///
/// The message type is per-test because the dispatcher registry is keyed by it
/// and every script in the shared test database registers at startup.
fn listener_script(message_type: &str, path: &str, version: &str) -> String {
    format!(
        r#"
function onMessage(context) {{
  return true;
}}

function handler(context) {{
  const summary = dispatcher.sendMessage("{message_type}", JSON.stringify({{}}));
  return {{ status: 200, body: "{version}|" + summary }};
}}

function init(context) {{
  dispatcher.registerListener("{message_type}", "onMessage");
  routeRegistry.registerRoute("{path}", "handler", "GET");
}}
"#
    )
}

/// Fetch `path` until the body names `version`, so the assertions that follow
/// are made against the deployment under test.
async fn wait_for_version(engine: &AdminServer, path: &str, version: &str) -> String {
    for _ in 0..60 {
        let body = engine
            .client()
            .get(engine.url(path))
            .send()
            .await
            .expect("request should reach the server")
            .text()
            .await
            .expect("body");
        if body.starts_with(&format!("{version}|")) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("script never came up as version '{version}'");
}

/// Writing a script again leaves it with one listener, not two.
#[tokio::test(flavor = "multi_thread")]
async fn a_rewritten_script_listens_once() {
    if should_skip_integration_tests() {
        return;
    }

    common::setup_env().await;
    let uri = "test_dispatch_registration_rewrite";
    let path = "/dispatch-registration/rewrite";
    let message_type = "dispatch.registration.rewrite";
    let _ = repository::upsert_script(uri, &listener_script(message_type, path, "v1"));

    let engine = AdminServer::start().await.expect("server failed to start");

    let first = wait_for_version(&engine, path, "v1").await;
    assert!(
        first.contains("1 successful, 0 failed"),
        "one listener before any rewrite; the dispatch reported: {first}"
    );

    // Twice, since one extra copy per write is what the append produced.
    for version in ["v2", "v3"] {
        let response = engine
            .client()
            .post(engine.url("/engine/upsert_script"))
            .form(&[
                ("uri", uri),
                ("content", &listener_script(message_type, path, version)),
            ])
            .send()
            .await
            .expect("upsert should reach the server");
        assert_eq!(response.status(), 200, "upsert of {version} should succeed");

        let body = wait_for_version(&engine, path, version).await;
        assert!(
            body.contains("1 successful, 0 failed"),
            "a script written {version} times still has one listener, not one \
             per write; the dispatch reported: {body}"
        );
    }

    engine.shutdown().await;
}

/// A deleted script stops listening. Its listeners named a script the
/// dispatcher could no longer fetch, so every later dispatch of that message
/// type counted a failure against a script that does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_script_stops_listening() {
    if should_skip_integration_tests() {
        return;
    }

    common::setup_env().await;
    let uri = "test_dispatch_registration_delete";
    let path = "/dispatch-registration/delete";
    let message_type = "dispatch.registration.delete";
    let _ = repository::upsert_script(uri, &listener_script(message_type, path, "v1"));

    // A second script dispatches the same type, so there is still somewhere to
    // ask from once the listening script is gone.
    let sender_uri = "test_dispatch_registration_delete_sender";
    let sender_path = "/dispatch-registration/delete-sender";
    let sender = format!(
        r#"
function handler(context) {{
  const summary = dispatcher.sendMessage("{message_type}", JSON.stringify({{}}));
  return {{ status: 200, body: "sender|" + summary }};
}}

function init(context) {{
  routeRegistry.registerRoute("{sender_path}", "handler", "GET");
}}
"#
    );
    let _ = repository::upsert_script(sender_uri, &sender);

    let engine = AdminServer::start().await.expect("server failed to start");

    let before = wait_for_version(&engine, sender_path, "sender").await;
    assert!(
        before.contains("1 successful, 0 failed"),
        "the listening script answers while it exists; the dispatch reported: {before}"
    );

    let response = engine
        .client()
        .post(engine.url("/engine/delete_script"))
        .form(&[("uri", uri)])
        .send()
        .await
        .expect("delete should reach the server");
    assert_eq!(response.status(), 200, "delete should succeed");

    let after = wait_for_version(&engine, sender_path, "sender").await;
    assert!(
        after.contains("No listeners"),
        "a deleted script leaves no listener behind; the dispatch reported: {after}"
    );

    engine.shutdown().await;
}
