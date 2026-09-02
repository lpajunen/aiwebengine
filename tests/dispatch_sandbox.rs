//! A dispatched message listener runs in the same sandbox as anything else.
//!
//! `dispatcher.sendMessage` executes the listening script's handler on the
//! calling thread. It used to build a bare QuickJS runtime — no memory limit,
//! no stack limit, no interrupt handler — which made it the one execution path
//! a script could reach where `javascript.execution_timeout_ms` and the memory
//! and stack limits did not apply.

mod common;

use std::time::{Duration, Instant};

use aiwebengine::repository;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

/// A listener that never returns must not take its thread with it.
///
/// The request itself proves nothing: its own timeout answers the caller
/// either way, abandoning the thread. What separates a bounded listener from
/// an unbounded one is whether that thread ever comes back — which is what the
/// worker census counts. With no interrupt handler the spinning listener holds
/// its thread, its pooled connection and its locks for the life of the
/// process; with one, it is stopped, the handler unwinds, and the abandoned
/// worker is recorded as recovered.
#[tokio::test(flavor = "multi_thread")]
async fn a_looping_listener_releases_its_thread() {
    if should_skip_integration_tests() {
        return;
    }

    let context = TestContext::new();
    let uri = "test_dispatch_sandbox_loop";

    // The sender registers a listener on itself and dispatches to it.
    // `while (true) {}` never leaves JavaScript, so only the interrupt handler
    // a sandboxed runtime arms can stop it.
    let _ = repository::upsert_script(
        uri,
        r#"
        function spin(context) {
          while (true) {}
        }

        function handler(context) {
          // The payload is a JSON string, which is what the API takes.
          dispatcher.sendMessage("dispatch.sandbox.loop", JSON.stringify({ n: 1 }));
          return { status: 200, body: "dispatched" };
        }

        function init(context) {
          dispatcher.registerListener("dispatch.sandbox.loop", "spin");
          routeRegistry.registerRoute("/dispatch-sandbox/loop", "handler", "GET");
        }
        "#,
    );

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let before = aiwebengine::worker_census::snapshot();

    let _ = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("client")
        .get(format!("http://127.0.0.1:{}/dispatch-sandbox/loop", port))
        .send()
        .await;

    // The worker is abandoned when the request's own budget runs out; what is
    // being waited for here is the thread coming back afterwards.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut census = aiwebengine::worker_census::snapshot();
    while census.in_flight > before.in_flight && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        census = aiwebengine::worker_census::snapshot();
    }

    assert_eq!(
        census.in_flight, before.in_flight,
        "the spinning listener kept its thread: in_flight {} -> {}. Without an \
         interrupt handler nothing can stop a listener that never leaves \
         JavaScript, and the thread and everything it holds are gone for the \
         life of the process.",
        before.in_flight, census.in_flight
    );

    context.cleanup().await.expect("Failed to cleanup");
}
