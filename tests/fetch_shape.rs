//! The shape `fetch()` hands back.
//!
//! One object serves three habits at once: `await fetch(url)` for anyone
//! arriving from the browser, `fetch(url).status` for direct access, and
//! `JSON.parse(fetch(url))` for the scripts written against the JSON string
//! `fetch` used to return. What it does not buy is concurrency — the request
//! has already finished by the time `fetch` returns.
//!
//! The transport is stubbed. `fetch` resolves `__hostFetch` — the Rust half —
//! from the global scope on every call, so replacing it exercises the whole
//! wrapper without a network, and without asking the engine to relax the SSRF
//! rules that stop a script reaching a private address. What the transport
//! does with a URL is `http_fetch.rs`'s subject; the seam between the two is
//! covered by `an_error_from_the_host_call_surfaces_through_fetch`, which lets
//! a real request fail and follows the error out.

mod common;

use common::{setup_env, test_mutex};

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use serde_json::json;

/// A stand-in `__hostFetch` returning the envelope the Rust half produces, plus
/// a counter so a test can tell how many requests actually happened.
const STUB: &str = r#"
    globalThis.__calls = 0;
    globalThis.__hostFetch = function (url, options) {
      globalThis.__calls += 1;
      return JSON.stringify({
        status: url.indexOf("/missing") >= 0 ? 404 : 200,
        ok: url.indexOf("/missing") < 0,
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ url: url, method: (options && options.method) || "GET" }),
      });
    };
"#;

/// Evaluates `source` against a deployed script that does nothing itself.
///
/// Run on a blocking thread, as the engine runs every handler: `fetch` uses a
/// blocking HTTP client, and driving one from inside an async context panics on
/// the runtime it manages internally.
async fn eval(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    let request = EvalRequest {
        script_uri: uri.to_string(),
        source: source.to_string(),
        user_context: UserContext::admin("fetch-shape".to_string()),
        timeout_ms: Some(10_000),
        rollback: true,
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// Evaluates `source` with the transport stubbed out.
async fn eval_stubbed(uri: &str, source: &str) -> EvalReport {
    eval(uri, &format!("{}\n{}", STUB, source)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_response_can_be_awaited_like_the_browsers() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_stubbed(
        "test://fetch-shape/await",
        r#"
        (async function () {
          const res = await fetch("https://example.test/data");
          const body = await res.json();
          return { status: res.status, ok: res.ok, url: body.url };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["status"], json!(200));
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["url"], json!("https://example.test/data"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_json_string_form_still_parses() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // What scripts written before `fetch` grew a shape do. `JSON.parse`
    // converts its argument with ToString first, so the envelope comes back.
    let report = eval_stubbed(
        "test://fetch-shape/legacy",
        r#"
        const parsed = JSON.parse(fetch("https://example.test/data"));
        ({ status: parsed.status, ok: parsed.ok, hasBody: parsed.body.length > 0 })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["status"], json!(200));
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["hasBody"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn fields_are_readable_without_awaiting() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_stubbed(
        "test://fetch-shape/direct",
        r#"
        const res = fetch("https://example.test/data");
        ({
          status: res.status,
          ok: res.ok,
          contentType: res.headers["content-type"],
          parsedUrl: res.json().url,
          hasText: res.text().length > 0,
        })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["status"], json!(200));
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["contentType"], json!("application/json"));
    assert_eq!(value["parsedUrl"], json!("https://example.test/data"));
    assert_eq!(value["hasText"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn options_reach_the_host_call_unchanged() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The wrapper passes its second argument straight through, so whatever the
    // calling convention was before it still holds.
    let report = eval_stubbed(
        "test://fetch-shape/options",
        r#"
        (async function () {
          const res = await fetch("https://example.test/data", { method: "POST" });
          return (await res.json()).method;
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!("POST")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_status_is_reported_rather_than_thrown() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // As in a browser: a 404 is an answer, not an error.
    let report = eval_stubbed(
        "test://fetch-shape/status",
        r#"
        (async function () {
          const res = await fetch("https://example.test/missing");
          return { status: res.status, ok: res.ok };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["status"], json!(404));
    assert_eq!(value["ok"], json!(false));
}

#[tokio::test(flavor = "multi_thread")]
async fn promise_all_answers_correctly_but_runs_one_at_a_time() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The shape invites `Promise.all`, so what it does there is worth pinning:
    // the right answers, and both requests already finished before the await —
    // which is the plainest evidence that `await` sequences rather than
    // overlaps.
    let report = eval_stubbed(
        "test://fetch-shape/all",
        r#"
        (async function () {
          const pending = [fetch("https://example.test/a"), fetch("https://example.test/b")];
          const callsBeforeAwait = globalThis.__calls;
          const [a, b] = await Promise.all(pending);
          return {
            first: (await a.json()).url,
            second: (await b.json()).url,
            callsBeforeAwait: callsBeforeAwait,
          };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["first"], json!("https://example.test/a"));
    assert_eq!(value["second"], json!("https://example.test/b"));
    assert_eq!(
        value["callsBeforeAwait"],
        json!(2),
        "both requests finish before the await: `await` sequences, it does not overlap"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn awaiting_a_response_twice_settles_both_times() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // `then` resolves to a twin without `then`. Resolving to the response
    // itself would hand the promise machinery another thenable to unwrap,
    // forever, so this is the regression test for that trap.
    let report = eval_stubbed(
        "test://fetch-shape/twice",
        r#"
        (async function () {
          const res = fetch("https://example.test/data");
          const once = await res;
          const twice = await res;
          return { once: once.status, twice: twice.status, thenGone: typeof once.then };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["once"], json!(200));
    assert_eq!(value["twice"], json!(200));
    assert_eq!(
        value["thenGone"],
        json!("undefined"),
        "the awaited value must not itself be thenable"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_returned_response_is_settled_by_the_caller() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Returning the response without awaiting hands the engine a thenable. It
    // has to settle it rather than report the object or run out of budget.
    let report = eval_stubbed(
        "test://fetch-shape/returned",
        r#"fetch("https://example.test/data")"#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["status"], json!(200));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_from_the_host_call_surfaces_through_fetch() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // No stub: the real transport refuses a private address, which is the
    // cheapest way to make a genuine request fail. What matters is that the
    // refusal reaches the script as an exception it can catch, rather than
    // being swallowed by the wrapper or turned into a promise nothing settles.
    let report = eval(
        "test://fetch-shape/host-error",
        r#"
        (function () {
          try {
            fetch("http://127.0.0.1:9/nothing");
            return "no error";
          } catch (e) {
            return String(e.message || e);
          }
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report
        .outcome
        .value
        .expect("a value")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        value.contains("Private IP") || value.contains("Blocked"),
        "the host call's refusal should reach the script, got: {}",
        value
    );
}
