//! A tool asking the person a question, over two round trips.
//!
//! The thing these hold is that nothing is suspended. The first call *ends* —
//! the handler runs, reaches `mcp.ask`, and the engine answers the client with
//! `input_required`. The second call is an independent request that runs the
//! same handler from the top again, and `ask` returns an answer that time.
//!
//! So the assertions are as much about what does not happen as about what
//! does: no execution is held open, the second call carries everything the
//! first left behind, and state minted for one call opens for no other.

mod common;

use common::{TestServer, wait_for_server};
use serde_json::{Value, json};

const TOOL_SCRIPT: &str = r#"
function init() {
  mcpRegistry.registerTool("ask_a_thing", "Asks for a branch", JSON.stringify({
    type: "object",
    properties: { repo: { type: "string" } }
  }), "askAThing");

  mcpRegistry.registerTool("count_passes", "Counts how often it ran", JSON.stringify({
    type: "object",
    properties: {}
  }), "countPasses");

  mcpRegistry.registerTool("never_asks", "Answers straight away", JSON.stringify({
    type: "object",
    properties: {}
  }), "neverAsks");
}

function askAThing(context) {
  if (!mcp.canAsk()) {
    return { asked: false, fallback: "main" };
  }
  var answer = mcp.ask({
    message: "Which branch?",
    schema: {
      type: "object",
      properties: { branch: { type: "string" } },
      required: ["branch"]
    }
  });
  return { asked: true, action: answer.action, branch: (answer.content || {}).branch || null };
}

// Proves the handler really does re-run: `passes` counts the passes, and
// `once` must not.
var onceCalls = 0;
function countPasses(context) {
  var pass = mcp.once("first", function () {
    onceCalls = onceCalls + 1;
    return "computed-once";
  });
  if (!mcp.canAsk()) {
    return { memo: pass };
  }
  var answer = mcp.ask({ message: "Ready?", schema: { type: "object", properties: {} } });
  return { memo: pass, action: answer.action };
}

function neverAsks(context) {
  return { fine: true };
}
"#;

async fn server() -> anyhow::Result<(TestServer, reqwest::Client, String)> {
    let server = TestServer::start().await?;
    wait_for_server(server.port(), 30).await?;
    aiwebengine::repository::upsert_script("test://mcp/elicit", TOOL_SCRIPT)
        .expect("script should store");
    // Tools are registered by the script's init(), not by storing it.
    aiwebengine::script_init::ScriptInitializer::with_configured_timeout()
        .initialize_script("test://mcp/elicit", false)
        .await
        .ok();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let url = format!("http://127.0.0.1:{}/mcp", server.port());
    Ok((server, http, url))
}

/// A `tools/call` the way a modern client that can be asked sends one.
fn call(tool: &str, extra: Value) -> Value {
    let mut params = json!({
        "name": tool,
        "arguments": {},
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": { "elicitation": { "form": {} } }
        }
    });
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            params.insert(key.clone(), value.clone());
        }
    }
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params })
}

async fn post(http: &reqwest::Client, url: &str, body: &Value) -> anyhow::Result<Value> {
    Ok(http.post(url).json(body).send().await?.json().await?)
}

/// The whole exchange: ask, answer, finish.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_asks_and_the_answer_comes_back_on_the_retry() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let asked = post(&http, &url, &call("ask_a_thing", json!({}))).await?;
    let result = &asked["result"];
    assert_eq!(
        result["resultType"], "input_required",
        "the first call ends asking rather than answering: {asked}"
    );
    let requests = result["inputRequests"]
        .as_object()
        .expect("an input_required carries what it wants");
    assert_eq!(requests.len(), 1);
    let (key, request) = requests.iter().next().expect("one request");
    assert_eq!(request["method"], "elicitation/create");
    assert_eq!(request["params"]["mode"], "form");
    assert_eq!(request["params"]["message"], "Which branch?");
    let state = result["requestState"]
        .as_str()
        .expect("and the state to carry it on");

    // The client asks the person, then retries the original request.
    let answered = post(
        &http,
        &url,
        &call(
            "ask_a_thing",
            json!({
                "requestState": state,
                "inputResponses": { key.clone(): { "action": "accept", "content": { "branch": "release" } } }
            }),
        ),
    )
    .await?;
    assert_eq!(
        answered["result"]["resultType"], "complete",
        "the second call finishes: {answered}"
    );
    let text = answered["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let payload: Value = serde_json::from_str(text).expect("the tool's result is JSON");
    assert_eq!(payload["asked"], true);
    assert_eq!(payload["action"], "accept");
    assert_eq!(
        payload["branch"], "release",
        "what the person said reaches the handler"
    );

    server.shutdown().await;
    Ok(())
}

/// A handler really does run again — and `mcp.once` really does not.
#[tokio::test(flavor = "multi_thread")]
async fn the_handler_re_runs_but_once_does_not() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let asked = post(&http, &url, &call("count_passes", json!({}))).await?;
    let state = asked["result"]["requestState"]
        .as_str()
        .expect("asked, so there is state");
    let key = asked["result"]["inputRequests"]
        .as_object()
        .and_then(|requests| requests.keys().next().cloned())
        .expect("one request");

    let answered = post(
        &http,
        &url,
        &call(
            "count_passes",
            json!({
                "requestState": state,
                "inputResponses": { key: { "action": "decline" } }
            }),
        ),
    )
    .await?;
    let text = answered["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let payload: Value = serde_json::from_str(text).expect("JSON");
    assert_eq!(
        payload["memo"], "computed-once",
        "the memo survived into the second pass: {answered}"
    );
    assert_eq!(
        payload["action"], "decline",
        "declining is a decision the handler sees, not an error"
    );

    server.shutdown().await;
    Ok(())
}

/// A client that did not declare elicitation must not be asked, and a handler
/// that can manage without takes its other path.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_cannot_be_asked_is_not() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let silent = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ask_a_thing",
            "arguments": {},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });
    let answered = post(&http, &url, &silent).await?;
    assert_eq!(
        answered["result"]["resultType"], "complete",
        "a server must not send a request the client never declared: {answered}"
    );
    let text = answered["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let payload: Value = serde_json::from_str(text).expect("JSON");
    assert_eq!(payload["asked"], false);
    assert_eq!(payload["fallback"], "main");

    server.shutdown().await;
    Ok(())
}

/// A legacy client cannot be asked either, whatever it declares: the result
/// type that would carry the question does not exist before `2026-07-28`.
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_client_is_never_asked() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let legacy = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "ask_a_thing", "arguments": {} }
    });
    let answered = post(&http, &url, &legacy).await?;
    let text = answered["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let payload: Value = serde_json::from_str(text).expect("JSON");
    assert_eq!(
        payload["asked"], false,
        "input_required is a resultType, and a legacy client has no case for it: {answered}"
    );

    server.shutdown().await;
    Ok(())
}

/// Request state is bound to the call that minted it.
#[tokio::test(flavor = "multi_thread")]
async fn state_from_one_call_does_not_open_another() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let asked = post(&http, &url, &call("ask_a_thing", json!({}))).await?;
    let state = asked["result"]["requestState"]
        .as_str()
        .expect("asked")
        .to_string();

    // Same state, different tool.
    let moved = post(
        &http,
        &url,
        &call("count_passes", json!({ "requestState": state.clone() })),
    )
    .await?;
    assert_eq!(
        moved["error"]["code"], -32602,
        "answers given to one question must not arrive at another tool: {moved}"
    );

    // Same tool, different arguments.
    let mut altered = call("ask_a_thing", json!({ "requestState": state.clone() }));
    altered["params"]["arguments"] = json!({ "repo": "somewhere/else" });
    let refused = post(&http, &url, &altered).await?;
    assert_eq!(refused["error"]["code"], -32602);

    // And something the engine never wrote.
    let forged = post(
        &http,
        &url,
        &call("ask_a_thing", json!({ "requestState": "made up" })),
    )
    .await?;
    assert_eq!(forged["error"]["code"], -32602);

    server.shutdown().await;
    Ok(())
}

/// A tool that never asks is untouched by any of this.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_that_does_not_ask_answers_as_it_always_did() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answered = post(&http, &url, &call("never_asks", json!({}))).await?;
    assert_eq!(answered["result"]["resultType"], "complete", "{answered}");
    assert_eq!(answered["result"]["isError"], false);
    assert!(
        answered["result"]["requestState"].is_null(),
        "nothing to carry, so nothing is minted"
    );

    server.shutdown().await;
    Ok(())
}
