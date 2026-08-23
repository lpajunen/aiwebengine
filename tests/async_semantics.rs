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
async fn writes_made_after_an_await_are_committed() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // The commit happens after the queue is drained. Committing when the
    // handler first returned would close the transaction while the write below
    // had not been made yet, losing it silently.
    let base = serve(
        &context,
        "test_async_commit",
        r#"
        function prepare(context) {
          database.dropTable("notes");
          database.createTable("notes");
          database.addTextColumn("notes", "label", true);
          return { status: 200, body: "prepared" };
        }

        async function handler(context) {
          database.beginTransaction(5000);
          await Promise.resolve();
          database.insert("notes", JSON.stringify({ label: "written after await" }));
          return { status: 200, body: "ok" };
        }

        function readBack(context) {
          return { status: 200, body: database.query("notes") };
        }

        function init(context) {
          routeRegistry.registerRoute("/async/commit/prepare", "prepare", "POST");
          routeRegistry.registerRoute("/async/commit", "handler", "POST");
          routeRegistry.registerRoute("/async/commit", "readBack", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(format!("{}/async/commit/prepare", base))
            .send()
            .await
            .expect("prepare failed")
            .status(),
        200
    );

    let written = client
        .post(format!("{}/async/commit", base))
        .send()
        .await
        .expect("write request failed");
    assert_eq!(
        written.status(),
        200,
        "{}",
        written.text().await.unwrap_or_default()
    );

    let rows = client
        .get(format!("{}/async/commit", base))
        .send()
        .await
        .expect("read request failed")
        .text()
        .await
        .expect("body");
    assert!(
        rows.contains("written after await"),
        "a write made after an await should be committed, got: {}",
        rows
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_are_rolled_back_when_the_handler_fails() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // Both failure shapes have to roll back, and roll back the same: a throw
    // before the handler returns, and a rejection that only arrives once the
    // queue is drained.
    let base = serve(
        &context,
        "test_rollback_both",
        r#"
        function prepare(context) {
          database.dropTable("ledger");
          database.createTable("ledger");
          database.addTextColumn("ledger", "label", true);
          return { status: 200, body: "prepared" };
        }

        function syncHandler(context) {
          database.beginTransaction(5000);
          database.insert("ledger", JSON.stringify({ label: "sync" }));
          throw new Error("sync failure");
        }

        async function asyncHandler(context) {
          database.beginTransaction(5000);
          await Promise.resolve();
          database.insert("ledger", JSON.stringify({ label: "async" }));
          throw new Error("async failure");
        }

        function readBack(context) {
          return { status: 200, body: database.query("ledger") };
        }

        function init(context) {
          routeRegistry.registerRoute("/rollback/prepare", "prepare", "POST");
          routeRegistry.registerRoute("/rollback/sync", "syncHandler", "POST");
          routeRegistry.registerRoute("/rollback/async", "asyncHandler", "POST");
          routeRegistry.registerRoute("/rollback", "readBack", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(format!("{}/rollback/prepare", base))
            .send()
            .await
            .expect("prepare failed")
            .status(),
        200
    );

    for path in ["sync", "async"] {
        let failed = client
            .post(format!("{}/rollback/{}", base, path))
            .send()
            .await
            .expect("write request failed");
        assert_eq!(failed.status(), 500, "{} handler should fail", path);
    }

    let rows = client
        .get(format!("{}/rollback", base))
        .send()
        .await
        .expect("read request failed")
        .text()
        .await
        .expect("body");
    assert!(
        !rows.contains("sync") && !rows.contains("async"),
        "a failing handler's writes should roll back however the failure arrived, got: {}",
        rows
    );

    context.cleanup().await.expect("Failed to cleanup");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transaction_left_open_does_not_leak_into_the_next_request() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();

    // A handler that opens a transaction and returns without finishing it is
    // committed at the boundary, so the thread it ran on must be left clean for
    // whatever request lands there next.
    let base = serve(
        &context,
        "test_tx_no_leak",
        r#"
        function prepare(context) {
          database.dropTable("leaky");
          database.createTable("leaky");
          database.addTextColumn("leaky", "label", true);
          return { status: 200, body: "prepared" };
        }

        function opener(context) {
          database.beginTransaction(5000);
          database.insert("leaky", JSON.stringify({ label: "first" }));
          return { status: 200, body: "opened" };
        }

        function follower(context) {
          database.insert("leaky", JSON.stringify({ label: "second" }));
          return { status: 200, body: "followed" };
        }

        function readBack(context) {
          return { status: 200, body: database.query("leaky") };
        }

        function init(context) {
          routeRegistry.registerRoute("/leak/prepare", "prepare", "POST");
          routeRegistry.registerRoute("/leak/open", "opener", "POST");
          routeRegistry.registerRoute("/leak/follow", "follower", "POST");
          routeRegistry.registerRoute("/leak", "readBack", "GET");
          return { success: true };
        }
        "#,
    )
    .await;

    let client = reqwest::Client::new();
    for path in ["prepare", "open", "follow"] {
        let response = client
            .post(format!("{}/leak/{}", base, path))
            .send()
            .await
            .expect("request failed");
        assert_eq!(response.status(), 200, "{} should succeed", path);
    }

    let rows = client
        .get(format!("{}/leak", base))
        .send()
        .await
        .expect("read request failed")
        .text()
        .await
        .expect("body");
    assert!(
        rows.contains("first") && rows.contains("second"),
        "both writes should be visible: the first committed at its boundary, \
         the second not swallowed by an inherited transaction, got: {}",
        rows
    );

    context.cleanup().await.expect("Failed to cleanup");
}
