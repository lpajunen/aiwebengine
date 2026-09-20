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
///
/// It takes its options the way the Rust half takes them — as JSON text, which
/// is what a host binding declared `Option<String>` can accept. The stub used
/// to read them as an object (`options.method`), a contract the host never
/// had: QuickJS raises `TypeError: Error converting from js 'object' into type
/// 'string'` rather than coercing one. So the one test covering options was
/// passing against a transport more forgiving than the real one, which is how
/// `fetch(url, { method: "POST" })` — the call the type declarations document
/// — came to throw for every script that made it.
const STUB: &str = r#"
    globalThis.__calls = 0;
    globalThis.__hostFetch = function (url, optionsJson) {
      globalThis.__calls += 1;
      if (optionsJson !== undefined && typeof optionsJson !== "string") {
        throw new TypeError(
          "Error converting from js '" + typeof optionsJson + "' into type 'string'"
        );
      }
      const options = optionsJson ? JSON.parse(optionsJson) : {};
      return JSON.stringify({
        status: url.indexOf("/missing") >= 0 ? 404 : 200,
        ok: url.indexOf("/missing") < 0,
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ url: url, method: options.method || "GET" }),
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
        timeout_ms: Some(10_000),
        rollback: true,
        ..EvalRequest::new(
            uri.to_string(),
            source.to_string(),
            UserContext::admin("fetch-shape".to_string()),
        )
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
async fn the_options_object_reaches_the_host_call_as_json() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The options are an object in the type declarations and in every example
    // in them, and JSON text at the host binding. The wrapper is what bridges
    // the two; passing the object through is what used to throw.
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

/// A script written against the host binding sends JSON text already, and it
/// must not be encoded a second time.
#[tokio::test(flavor = "multi_thread")]
async fn options_already_in_json_are_passed_through() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_stubbed(
        "test://fetch-shape/options-json",
        r#"
        (async function () {
          const res = await fetch(
            "https://example.test/data",
            JSON.stringify({ method: "PUT" })
          );
          return (await res.json()).method;
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!("PUT")));
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

// ---------------------------------------------------------------------------
// Several at once, and one a piece at a time
// ---------------------------------------------------------------------------
//
// The wrappers, not the transport. Whether requests really overlap and
// whether a character split across chunks survives are claims about the Rust
// half and are measured against a real socket in `fetch_concurrency.rs`;
// what is left over is the shape JavaScript sees, which is what these cover.

/// A stand-in for the parallel and streaming host calls, shaped the way the
/// Rust half shapes them.
const STREAM_STUB: &str = r#"
    globalThis.__hostFetchAll = function (requestsJson) {
      const requests = JSON.parse(requestsJson);
      return JSON.stringify(requests.map(function (request) {
        if (request.url.indexOf("/refused") >= 0) {
          return { ok: false, error: "Blocked URL: " + request.url };
        }
        return {
          ok: true,
          response: {
            status: 200,
            ok: true,
            headers: {},
            body: JSON.stringify({
              url: request.url,
              method: (request.options && request.options.method) || "GET",
            }),
          },
        };
      }));
    };

    globalThis.__streamReads = 0;
    globalThis.__closed = [];
    globalThis.__hostFetchStreamStart = function (url, optionsJson) {
      globalThis.__streamReads = 0;
      return JSON.stringify({
        streamId: "7",
        status: 200,
        ok: true,
        headers: { "content-type": "text/plain" },
      });
    };
    globalThis.__hostFetchStreamRead = function (id) {
      globalThis.__streamReads += 1;
      if (globalThis.__streamReads > 3) {
        return JSON.stringify({ done: true });
      }
      return JSON.stringify({ done: false, value: "chunk" + globalThis.__streamReads });
    };
    globalThis.__hostFetchStreamClose = function (id) {
      globalThis.__closed.push(id);
      return true;
    };
"#;

async fn eval_streaming(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    let request = EvalRequest {
        timeout_ms: Some(10_000),
        rollback: true,
        ..EvalRequest::new(
            uri.to_string(),
            format!("{}\n{}", STREAM_STUB, source),
            UserContext::admin("fetch-shape".to_string()),
        )
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// One host call for the batch, and responses that read like `fetch`'s.
#[tokio::test(flavor = "multi_thread")]
async fn fetch_all_answers_one_response_per_request() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_streaming(
        "test://fetch-shape/all",
        r#"
        (function () {
          const answers = fetchAll([
            "https://example.test/a",
            { url: "https://example.test/b", options: { method: "POST" } },
          ]);
          return {
            count: answers.length,
            first: answers[0].json().url,
            method: answers[1].json().method,
            status: answers[0].status,
          };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["count"], json!(2));
    assert_eq!(value["first"], json!("https://example.test/a"));
    assert_eq!(
        value["method"],
        json!("POST"),
        "an object entry should carry its options"
    );
    assert_eq!(value["status"], json!(200));
}

/// A refused URL is one refused answer. The caller asked for several and has
/// a use for the ones that arrived, so the throw waits until that slot is
/// touched rather than taking the batch down.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_request_does_not_take_the_batch_with_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_streaming(
        "test://fetch-shape/all-partial",
        r#"
        (function () {
          const answers = fetchAll([
            "https://example.test/fine",
            "https://example.test/refused",
          ]);

          let threw = null;
          try {
            answers[1].json();
          } catch (e) {
            threw = String(e.message || e);
          }

          return {
            good: answers[0].json().url,
            badOk: answers[1].ok,
            threw: threw,
          };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["good"], json!("https://example.test/fine"));
    assert_eq!(value["badOk"], json!(false));
    assert!(
        value["threw"]
            .as_str()
            .unwrap_or_default()
            .contains("Blocked URL"),
        "touching the failed slot should throw its reason: {}",
        value["threw"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_all_refuses_something_that_is_not_a_list() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_streaming(
        "test://fetch-shape/all-bad",
        r#"
        (function () {
          try {
            fetchAll("https://example.test/a");
            return "no error";
          } catch (e) {
            return String(e.message || e);
          }
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    assert!(
        report
            .outcome
            .value
            .expect("a value")
            .as_str()
            .unwrap_or_default()
            .contains("array"),
        "a single URL is a common mistake and should say so"
    );
}

/// The status and headers are there before the body is, which is the whole
/// difference from `fetch`, and the body iterates.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_yields_its_head_first_and_then_its_pieces() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_streaming(
        "test://fetch-shape/stream",
        r#"
        (function () {
          const stream = fetchStream("https://example.test/events");
          const head = { status: stream.status, ok: stream.ok };

          const pieces = [];
          for (const chunk of stream) {
            pieces.push(chunk);
          }

          return { head: head, pieces: pieces, readsAfterEnd: stream.read().done };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["head"]["status"], json!(200));
    assert_eq!(value["head"]["ok"], json!(true));
    assert_eq!(value["pieces"], json!(["chunk1", "chunk2", "chunk3"]));
    assert_eq!(
        value["readsAfterEnd"],
        json!(true),
        "an ended stream keeps answering done rather than starting again"
    );
}

/// `text()` is for a caller that wanted the headers early and the body whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_can_be_drained_to_one_string() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_streaming(
        "test://fetch-shape/stream-text",
        r#"fetchStream("https://example.test/events").text()"#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(
        report.outcome.value.expect("a value"),
        json!("chunk1chunk2chunk3")
    );
}

/// Closing early reaches the host, so a script that has read enough gives
/// the socket back rather than holding it to the end of the execution.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_stream_early_reaches_the_host() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_streaming(
        "test://fetch-shape/stream-close",
        r#"
        (function () {
          const stream = fetchStream("https://example.test/events");
          stream.read();
          stream.close();
          return { closed: globalThis.__closed, after: stream.read().done };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["closed"], json!(["7"]));
    assert_eq!(
        value["after"],
        json!(true),
        "a closed stream must not go back to the host for more"
    );
}
