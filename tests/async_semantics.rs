//! Promises and `async`/`await` in scripts.
//!
//! Scripts run on a single thread with no timers, and every host call blocks
//! rather than yielding. The engine therefore runs the microtask queue to a
//! fixed point after each invocation and settles whatever the handler returned.
//! `await` sequences work; it never makes anything concurrent.

mod common;

use aiwebengine::repository;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

/// Deploys `script` and serves it, returning the running server's base URL.
async fn serve(context: &TestContext, script_uri: &str, script: &str) -> String {
    let _ = repository::upsert_script(script_uri, script);
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    format!("http://127.0.0.1:{}", port)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_async_handler_resumes_after_its_awaits() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let base = serve(
        &context,
        "test_async_handler",
        r#"
        async function handler(context) {
          const first = await Promise.resolve("one");
          const second = await Promise.resolve("two");
          const [a, b] = await Promise.all([first, second]);
          return { status: 200, body: a + "/" + b };
        }

        function init(context) {
          routeRegistry.registerRoute("/async/resumes", "handler", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let response = reqwest::get(format!("{}/async/resumes", base))
        .await
        .expect("route request failed");
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.expect("body"),
        "one/two",
        "everything after an await should have run"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_rejecting_after_an_await_fails_the_request() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let base = serve(
        &context,
        "test_async_rejects",
        r#"
        async function handler(context) {
          await Promise.resolve();
          throw new Error("late failure");
        }

        function init(context) {
          routeRegistry.registerRoute("/async/rejects", "handler", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let response = reqwest::get(format!("{}/async/rejects", base))
        .await
        .expect("route request failed");
    assert_eq!(response.status(), 500);
    let body = response.text().await.expect("body");
    assert!(
        body.contains("late failure"),
        "a rejection should surface like any thrown error, got: {}",
        body
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_returning_an_unsettleable_promise_is_told_so() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    let base = serve(
        &context,
        "test_async_pending",
        r#"
        function handler(context) {
          // Nothing can settle this: there are no timers, and every host call
          // has already returned by the time the queue is drained.
          return new Promise(function () {});
        }

        function init(context) {
          routeRegistry.registerRoute("/async/pending", "handler", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let response = reqwest::get(format!("{}/async/pending", base))
        .await
        .expect("route request failed");
    assert_eq!(response.status(), 500);
    let body = response.text().await.expect("body");
    assert!(
        body.contains("never settled") && body.contains("handler"),
        "an unsettleable promise should be diagnosed by name, got: {}",
        body
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thenable_that_resolves_to_itself_does_not_hang_the_request() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // A thenable whose `then` resolves with the same object re-enters the
    // promise-resolution job forever. The drain is bounded by the execution
    // deadline, so this has to end as a failed request rather than a hang.
    let base = serve(
        &context,
        "test_async_self_thenable",
        r#"
        function handler(context) {
          const self = { status: 200, body: "never" };
          self.then = function (onFulfilled) { onFulfilled(self); };
          return self;
        }

        function init(context) {
          routeRegistry.registerRoute("/async/self-thenable", "handler", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let response = reqwest::get(format!("{}/async/self-thenable", base))
        .await
        .expect("route request failed");
    assert_eq!(
        response.status(),
        500,
        "a self-resolving thenable must fail, not hang"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runaway_await_loop_is_stopped_by_the_deadline() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // The async analogue of `while (true) {}`: a chain that re-enqueues itself
    // forever. The runtime's interrupt handler bounds the drain.
    let base = serve(
        &context,
        "test_async_runaway",
        r#"
        async function handler(context) {
          while (true) { await Promise.resolve(1); }
        }

        function init(context) {
          routeRegistry.registerRoute("/async/runaway", "handler", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let response = reqwest::get(format!("{}/async/runaway", base))
        .await
        .expect("route request failed");
    assert_eq!(response.status(), 500);

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejection_after_an_await_leaves_what_a_synchronous_throw_leaves() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // Settling a handler's promise must not change when its transaction is
    // finished relative to its writes. Asserting *parity* rather than rollback
    // is deliberate: `database.beginTransaction()` currently ends the
    // transaction as soon as it starts (the `TransactionGuard` returned by
    // `Database::begin_transaction` is dropped immediately in
    // `secure_globals.rs`, and dropping it rolls back), so writes never join a
    // transaction at all. That is a pre-existing bug, not one settling
    // introduced — and this test fails the moment the two paths diverge.
    let base = serve(
        &context,
        "test_tx_parity",
        r#"
        function prepare(context) {
          database.dropTable("parity");
          database.createTable("parity");
          database.addTextColumn("parity", "label", true);
          return { status: 200, body: "prepared" };
        }

        function syncHandler(context) {
          database.beginTransaction(5000);
          database.insert("parity", JSON.stringify({ label: "sync" }));
          throw new Error("sync failure");
        }

        async function asyncHandler(context) {
          database.beginTransaction(5000);
          await Promise.resolve();
          database.insert("parity", JSON.stringify({ label: "async" }));
          throw new Error("async failure");
        }

        function readBack(context) {
          return { status: 200, body: database.query("parity") };
        }

        function init(context) {
          routeRegistry.registerRoute("/parity/prepare", "prepare", "POST");
          routeRegistry.registerRoute("/parity/sync", "syncHandler", "POST");
          routeRegistry.registerRoute("/parity/async", "asyncHandler", "POST");
          routeRegistry.registerRoute("/parity", "readBack", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let client = reqwest::Client::new();
    let read_back = |client: reqwest::Client, base: String| async move {
        client
            .get(format!("{}/parity", base))
            .send()
            .await
            .expect("read request failed")
            .text()
            .await
            .expect("body")
    };

    assert_eq!(
        client
            .post(format!("{}/parity/prepare", base))
            .send()
            .await
            .expect("prepare failed")
            .status(),
        200
    );

    let sync_failed = client
        .post(format!("{}/parity/sync", base))
        .send()
        .await
        .expect("sync request failed");
    assert_eq!(sync_failed.status(), 500);
    let after_sync = read_back(client.clone(), base.clone()).await;

    let async_failed = client
        .post(format!("{}/parity/async", base))
        .send()
        .await
        .expect("async request failed");
    assert_eq!(async_failed.status(), 500);
    let after_async = read_back(client.clone(), base.clone()).await;

    // Whatever the engine does with a failed handler's writes, it must do the
    // same whether the failure arrived synchronously or through a rejection.
    let sync_kept = after_sync.contains("\"sync\"");
    let async_kept = after_async.contains("\"async\"");
    assert_eq!(
        sync_kept, async_kept,
        "sync and async failures must leave the same state; after sync: {}, after async: {}",
        after_sync, after_async
    );

    context.cleanup().await.expect("Failed to cleanup");
}
