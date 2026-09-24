//! A tool call that does not finish inside the request.
//!
//! The `io.modelcontextprotocol/tasks` extension: rather than holding a
//! connection open, the server hands back a durable handle and the client polls
//! `tasks/get` until the status is terminal. The engine had the durable half
//! already — `tasks.rs` is a queue with attempts, leases and lanes — and what
//! these cover is the mapping onto it.
//!
//! The whole loop is walked here rather than each piece being unit-tested,
//! because every piece of it had somewhere to be checked while the loop itself
//! is what a client actually does: call, get a handle, poll, read the result.
//! That is the same argument `mcp_oauth_flow.rs` makes.

mod common;

use common::{TestServer, wait_for_server};
use serde_json::{Value, json};

use aiwebengine::repository;

const SCRIPT_URI: &str = "test://mcp/tasks";

/// A tool that hands its work off, a handler to do it, and one that fails.
///
/// `slowTool` is the shape the extension is for: check whether the client can
/// be handed a handle, hand off if so, and answer synchronously if not.
const TASK_SCRIPT: &str = r#"
function init() {
  mcpRegistry.registerTool(
    "slow",
    "Work that takes a while",
    JSON.stringify({ type: "object", properties: { n: { type: "number" } } }),
    "slowTool"
  );
  mcpRegistry.registerTool(
    "breaks",
    "Work that fails",
    JSON.stringify({ type: "object", properties: {} }),
    "breakingTool"
  );
  mcpRegistry.registerTool(
    "quick",
    "Work that answers at once",
    JSON.stringify({ type: "object", properties: {} }),
    "quickTool"
  );
}

function slowTool(context) {
  if (mcp.canTask()) {
    // Does not return: the work is queued and the call ends with a handle.
    mcp.task({
      handler: "runSlow",
      payload: { n: context.args.n },
      statusMessage: "counting",
      target: "slow"
    });
  }
  // The synchronous fallback, for a client that did not declare the extension.
  return { content: [{ type: "text", text: "inline" }] };
}

function runSlow(context) {
  const n = context.meta.task.payload.n;
  return { content: [{ type: "text", text: "counted to " + n }] };
}

function breakingTool(context) {
  if (mcp.canTask()) {
    mcp.task({ handler: "runBroken", payload: {}, target: "breaks" });
  }
  return { content: [{ type: "text", text: "inline" }] };
}

function runBroken(context) {
  throw new Error("the work did not work");
}

function quickTool(context) {
  return { content: [{ type: "text", text: "immediate" }] };
}
"#;

async fn server() -> anyhow::Result<(TestServer, reqwest::Client, String)> {
    let server = TestServer::start().await?;
    wait_for_server(server.port(), 30).await?;

    repository::upsert_script(SCRIPT_URI, TASK_SCRIPT).expect("script should store");
    aiwebengine::script_init::ScriptInitializer::with_configured_timeout()
        .initialize_script(SCRIPT_URI, false)
        .await
        .ok();

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let url = format!("http://127.0.0.1:{}/mcp", server.port());
    Ok((server, http, url))
}

/// A `tools/call` from a client that declared the extension.
fn call(tool: &str, arguments: Value, declares_tasks: bool) -> Value {
    let capabilities = if declares_tasks {
        json!({ "extensions": { "io.modelcontextprotocol/tasks": {} } })
    } else {
        json!({})
    };
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool,
            "arguments": arguments,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": capabilities
            }
        }
    })
}

fn task_request(method: &str, task_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": {
            "taskId": task_id,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {
                    "extensions": { "io.modelcontextprotocol/tasks": {} }
                }
            }
        }
    })
}

async fn post(http: &reqwest::Client, url: &str, body: &Value) -> anyhow::Result<Value> {
    Ok(http.post(url).json(body).send().await?.json().await?)
}

/// Poll until the task stops changing, the way a client does.
///
/// Deliberately a poll rather than one drain of the queue. The server under
/// test runs its own worker, so a test that drained the queue itself would be
/// racing it — and losing that race looks exactly like the feature not working,
/// which is the most expensive kind of flake to read. Polling is also what the
/// extension tells a client to do, so this exercises the same path.
async fn poll_until_terminal(
    http: &reqwest::Client,
    url: &str,
    task_id: &str,
) -> anyhow::Result<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        // Nudge the queue as well as polling: the worker's own tick is on a
        // timer, and waiting for it would make every one of these tests as slow
        // as that interval.
        aiwebengine::tasks::run_due_now("mcp-tasks-test").await;

        let answer = post(http, url, &task_request("tasks/get", task_id)).await?;
        let status = answer["result"]["status"].as_str().unwrap_or_default();
        if matches!(status, "completed" | "failed" | "cancelled") {
            return Ok(answer["result"].clone());
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!("task never reached a terminal status: {}", answer);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The whole loop: call, handle, poll, result.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_hands_back_a_handle_and_the_result_arrives_through_it() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(&http, &url, &call("slow", json!({ "n": 7 }), true)).await?;
    let result = &answer["result"];

    // `resultType: "task"` is what tells a client this is a handle and not the
    // answer — the one field a client keys the whole flow off.
    assert_eq!(result["resultType"], "task", "got: {}", answer);
    assert_eq!(result["status"], "working");
    assert_eq!(result["statusMessage"], "counting");
    assert!(
        result["pollIntervalMs"].is_number(),
        "a working task should say how often to poll: {}",
        answer
    );
    let task_id = result["taskId"]
        .as_str()
        .expect("a handle must carry an id")
        .to_string();

    // Polling before the worker has run reports the work still going, and the
    // poll itself is a complete answer — `resultType` describes the response,
    // not the work it reports on.
    let polled = post(&http, &url, &task_request("tasks/get", &task_id)).await?;
    assert_eq!(polled["result"]["resultType"], "complete", "{}", polled);
    assert_eq!(polled["result"]["status"], "working");
    assert!(polled["result"].get("result").is_none());

    let finished = poll_until_terminal(&http, &url, &task_id).await?;
    let finished = &finished;
    assert_eq!(finished["status"], "completed", "got: {}", finished);
    // What the original call would have returned synchronously, which is the
    // handler's own return value — the reason a task handler's result stopped
    // being discarded.
    // The same envelope a synchronous call would have produced, which is the
    // point: a client must not have to branch on whether its work was queued.
    assert!(
        finished["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("counted to 7"),
        "a task's result should be shaped like the synchronous one: {}",
        finished
    );
    assert_eq!(finished["result"]["isError"], false);
    assert!(
        finished.get("pollIntervalMs").is_none(),
        "a finished task should not still be told to poll: {}",
        finished
    );

    drop(server);
    Ok(())
}

/// A client that did not declare the extension is answered synchronously.
///
/// The specification is explicit that a server must never hand a task to a
/// client that did not opt in, and the reason is not politeness: a client that
/// does not know `resultType: "task"` reads the handle as the answer, and a
/// tool call comes back as a small object full of fields nobody asked for.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_did_not_opt_in_is_never_handed_a_task() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(&http, &url, &call("slow", json!({ "n": 1 }), false)).await?;
    let result = &answer["result"];

    assert_eq!(result["resultType"], "complete", "got: {}", answer);
    assert!(
        result.get("taskId").is_none(),
        "no handle may reach a client that did not declare the extension: {}",
        answer
    );
    // And the script's own fallback ran, which is what `mcp.canTask()` is for.
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("inline"),
        "the fallback answer should come back: {}",
        answer
    );

    drop(server);
    Ok(())
}

/// A handler that throws puts the task in `failed`, with the error on it.
///
/// The client is waiting on this handle and will poll until it is terminal, so
/// a failure that left the task `working` would be a client polling forever
/// over work that had already stopped.
#[tokio::test(flavor = "multi_thread")]
async fn work_that_fails_reaches_the_client_as_a_failed_task() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(&http, &url, &call("breaks", json!({}), true)).await?;
    let task_id = answer["result"]["taskId"]
        .as_str()
        .expect("a handle must carry an id")
        .to_string();

    let finished = poll_until_terminal(&http, &url, &task_id).await?;
    let finished = &finished;
    assert_eq!(finished["status"], "failed", "got: {}", finished);
    assert!(
        finished["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("did not work"),
        "a failed task should say why: {}",
        finished
    );
    assert!(finished.get("result").is_none());

    drop(server);
    Ok(())
}

/// Cancelling a task the worker has not claimed actually stops it.
///
/// Set up directly rather than through a `tools/call`, and deliberately so: the
/// server under test runs its own worker, so a task queued for *now* may well
/// have run before a cancellation could reach it. That race is the extension's
/// cooperative semantics rather than a defect — "the server acknowledges the
/// intent but is not obligated to stop the work" — and a test that raced it
/// would be asserting something the engine does not promise.
///
/// So the task is queued for later, which is the case the guarantee covers:
/// nothing has claimed it, and cancelling must both stop it and say so.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_before_the_worker_claims_it_stops_the_work() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let queued = aiwebengine::tasks::enqueue(aiwebengine::tasks::NewTask {
        script_uri: SCRIPT_URI.to_string(),
        handler_name: "runSlow".to_string(),
        payload: json!({ "n": 5 }),
        // Far enough out that no worker tick can reach it first.
        run_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
        max_attempts: Some(1),
        enqueued_by: None,
        kind: aiwebengine::tasks::TaskKind::Task,
        run_as: None,
        lane: None,
    })
    .await
    .expect("the work should queue");

    let task_id = uuid::Uuid::new_v4();
    aiwebengine::mcp_tasks::create(
        task_id,
        queued.task_id,
        SCRIPT_URI,
        "tools/call",
        "slow",
        None,
    )
    .await
    .expect("the handle should record");

    let cancelled = post(
        &http,
        &url,
        &task_request("tasks/cancel", &task_id.to_string()),
    )
    .await?;
    assert_eq!(
        cancelled["result"]["resultType"], "complete",
        "cancellation is acknowledged: {}",
        cancelled
    );

    let polled = post(
        &http,
        &url,
        &task_request("tasks/get", &task_id.to_string()),
    )
    .await?;
    assert_eq!(
        polled["result"]["status"], "cancelled",
        "a task cancelled before it ran should say so: {}",
        polled
    );
    assert!(
        polled["result"].get("result").is_none(),
        "cancelled work produced nothing: {}",
        polled
    );

    // And the queue row went with it, so nothing runs it later.
    assert!(
        aiwebengine::tasks::get(queued.task_id)
            .await
            .expect("the queue should answer")
            .is_none_or(|task| task.state == "cancelled"),
        "the queued work should be stopped, not merely reported as stopped"
    );

    drop(server);
    Ok(())
}

/// An id nobody issued is refused, and refused the same way an expired one is.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_task_is_refused() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    for id in ["00000000-0000-0000-0000-000000000000", "not-a-uuid-at-all"] {
        let answer = post(&http, &url, &task_request("tasks/get", id)).await?;
        assert_eq!(
            answer["error"]["code"], -32602,
            "an unknown task is invalid params: {}",
            answer
        );
    }

    drop(server);
    Ok(())
}

/// `tasks/update` is served, and acknowledges.
///
/// Nothing here reaches `input_required` — a queued run asking a person a
/// question is a design question rather than a plumbing one — so every key a
/// client could send is one that is not outstanding, which the extension says
/// to ignore. Served rather than refused because the method exists and a client
/// is entitled to call it.
#[tokio::test(flavor = "multi_thread")]
async fn updating_a_task_is_acknowledged() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(&http, &url, &call("slow", json!({ "n": 2 }), true)).await?;
    let task_id = answer["result"]["taskId"]
        .as_str()
        .expect("a handle must carry an id")
        .to_string();

    let updated = post(
        &http,
        &url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tasks/update",
            "params": {
                "taskId": task_id,
                "inputResponses": { "nothing": { "action": "accept" } },
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {
                        "extensions": { "io.modelcontextprotocol/tasks": {} }
                    }
                }
            }
        }),
    )
    .await?;

    assert_eq!(updated["result"]["resultType"], "complete", "{}", updated);
    assert!(updated["error"].is_null());

    drop(server);
    Ok(())
}

/// A tool that does not hand off is untouched by any of this.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_that_answers_at_once_still_does() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(&http, &url, &call("quick", json!({}), true)).await?;
    assert_eq!(answer["result"]["resultType"], "complete", "{}", answer);
    assert!(
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("immediate"),
        "got: {}",
        answer
    );

    drop(server);
    Ok(())
}

/// The server says it speaks the extension, which is what a client reads before
/// declaring it.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_advertises_the_extension() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let discovered = post(
        &http,
        &url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        }),
    )
    .await?;

    assert!(
        discovered["result"]["capabilities"]["extensions"]
            .get("io.modelcontextprotocol/tasks")
            .is_some(),
        "server/discover should advertise the tasks extension: {}",
        discovered
    );

    drop(server);
    Ok(())
}
