//! `javascript.max_concurrent_executions`: how many scripts run at once.
//!
//! The ceiling used to be Tokio's default blocking pool — 512 threads, a
//! number nobody chose — while the configured value was read by nothing. These
//! tests pin the two properties that matter: executions past the limit wait
//! rather than being refused, and the slot is held for as long as the work is,
//! not only for as long as the caller is still waiting for it.

mod common;

use std::time::Instant;

use aiwebengine::repository;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

/// Busy-waits for `ms` inside JavaScript.
///
/// Scripts have no timer to sleep on — a handler that wants to occupy its
/// thread has to spend the time. That is exactly what a slot accounts for.
fn busy_handler(path: &str, ms: u64) -> String {
    format!(
        r#"
        function handler(context) {{
          const until = Date.now() + {ms};
          while (Date.now() < until) {{}}
          return {{ status: 200, body: "done" }};
        }}

        function init(context) {{
          routeRegistry.registerRoute("{path}", "handler", "GET");
        }}
        "#
    )
}

/// With one slot, concurrent requests are served one at a time — and all of
/// them are still served.
///
/// Timing is the only way to observe this from outside: with a single slot the
/// engine cannot overlap the busy-waits, so the wall clock has to hold at
/// least as much as the sum of them. The margins are deliberately loose; what
/// would fail here is not a slow machine but a limit that does not bind.
#[tokio::test(flavor = "multi_thread")]
async fn executions_past_the_limit_wait_their_turn() {
    if should_skip_integration_tests() {
        return;
    }

    // Claim the process-global ceiling before the server sets it from the test
    // configuration. `configure` is first-writer-wins, so this has to run
    // before anything starts a server in this process.
    assert!(
        aiwebengine::execution_slots::configure(1),
        "this test owns the ceiling for its process"
    );

    let context = TestContext::new();
    let uri = "test_execution_slots_serialize";
    let path = "/execution-slots/serialize";
    let busy_ms = 150;
    let requests = 4;

    let _ = repository::upsert_script(uri, &busy_handler(path, busy_ms));
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    let base = format!("http://127.0.0.1:{}", port);

    let started = Instant::now();
    let mut inflight = Vec::new();
    for _ in 0..requests {
        let url = format!("{}{}", base, path);
        inflight.push(tokio::spawn(async move {
            reqwest::get(&url).await.map(|response| response.status())
        }));
    }

    let mut served = 0;
    for request in inflight {
        let status = request
            .await
            .expect("request task should not panic")
            .expect("request should reach the server");
        assert_eq!(status, 200, "a queued request is served, not refused");
        served += 1;
    }
    let elapsed = started.elapsed();

    assert_eq!(served, requests, "every request should be answered");

    // Serialised, the busy-waits cannot overlap. Allowing one request's worth
    // of slack keeps this about the limit rather than about scheduling noise.
    let floor = std::time::Duration::from_millis(busy_ms * (requests - 1));
    assert!(
        elapsed >= floor,
        "one slot should serialise the handlers: {} requests of {}ms took {:?}, \
         which is less than {:?} and means they overlapped",
        requests,
        busy_ms,
        elapsed,
        floor
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// Waiting for a slot does not make the engine think it lost a thread.
///
/// The census counts blocking workers the engine gave up on, because each one
/// keeps a thread and whatever it holds. A request that timed out waiting for
/// a slot never started a worker, and counting it would leave a permanent
/// phantom in the number an operator is meant to alert on — so the queue this
/// change introduces must not feed it.
#[tokio::test(flavor = "multi_thread")]
async fn waiting_for_a_slot_is_not_recorded_as_an_abandoned_worker() {
    if should_skip_integration_tests() {
        return;
    }

    assert!(
        aiwebengine::execution_slots::configure(1),
        "this test owns the ceiling for its process"
    );

    let context = TestContext::new();
    let uri = "test_execution_slots_census";
    let path = "/execution-slots/census";

    // Longer than the engine's own execution budget, so the second request
    // cannot get a slot before its timeout fires.
    let _ = repository::upsert_script(uri, &busy_handler(path, 8_000));
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    let base = format!("http://127.0.0.1:{}", port);

    let before = aiwebengine::worker_census::snapshot();

    let occupier = tokio::spawn({
        let url = format!("{}{}", base, path);
        async move { reqwest::get(&url).await.map(|r| r.status()) }
    });
    // Let the first request take the only slot.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let queued = reqwest::get(format!("{}{}", base, path)).await;
    assert!(queued.is_ok(), "the queued request should reach the server");

    let after = aiwebengine::worker_census::snapshot();

    // The occupier is genuinely abandoned and genuinely counted: it outran its
    // own budget, so its thread really is still out there busy-waiting. What
    // must not be counted is the second request, which never got a thread at
    // all — so the census should have moved by one, not by two.
    let lost = after.in_flight.saturating_sub(before.in_flight);
    assert_eq!(
        lost, 1,
        "only the executing worker should count as lost; the request that \
         merely waited for a slot never had a thread (in_flight {} -> {})",
        before.in_flight, after.in_flight
    );

    let _ = occupier.await;
    context.cleanup().await.expect("Failed to cleanup");
}
