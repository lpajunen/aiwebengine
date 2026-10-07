//! `audit.record`: the script says what happened, the engine says who, and
//! nothing a script or an operation can reach takes it back.

mod common;

use aiwebengine::engine_api::execute_native_mcp_tool;
use aiwebengine::security::UserContext;
use common::AdminServer;
use serde_json::json;

const URI: &str = "audit-probe";

const SCRIPT: &str = r#"
function refund(context) {
  const id = audit.record("refund.issued", { orderId: "o-1", amount: 10 });
  return ResponseBuilder.json({ id });
}

function refundThenFail(context) {
  audit.record("refund.attempted", { orderId: "o-2" });
  throw new Error("payment provider down");
}

function badEvent(context) {
  try {
    audit.record("", {});
    return ResponseBuilder.json({ error: null });
  } catch (e) {
    return ResponseBuilder.json({ error: e.message });
  }
}

function init() {
  routeRegistry.registerRoute("/audit/refund", { handler: "refund", method: "POST" });
  routeRegistry.registerRoute("/audit/refund-then-fail", { handler: "refundThenFail", method: "POST" });
  routeRegistry.registerRoute("/audit/bad", { handler: "badEvent" });
}
"#;

fn read(user: &UserContext) -> serde_json::Value {
    execute_native_mcp_tool("read_audit", &json!({ "script": URI }), user)
        .expect("read_audit is an operation")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_event_is_attributed_by_the_engine_and_outlives_what_clears_logs() {
    let server = AdminServer::start().await.expect("server should start");
    server.deploy_script(URI, SCRIPT).await;

    let signed_in = server
        .post("/audit/refund")
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .expect("request should be answered");
    assert_eq!(signed_in.status(), 200);
    let anonymous = server
        .anonymous()
        .post(server.url("/audit/refund"))
        .send()
        .await
        .expect("request should be answered");
    assert_eq!(anonymous.status(), 200);
    let failed = server
        .anonymous()
        .post(server.url("/audit/refund-then-fail"))
        .send()
        .await
        .expect("request should be answered");
    assert_eq!(failed.status(), 500, "the handler throws");

    let admin = UserContext::admin("audit-reader".to_string());
    let cleared = execute_native_mcp_tool("clear_logs", &json!({ "script": URI }), &admin)
        .expect("clear_logs is an operation");
    assert!(cleared.get("error").is_none(), "{cleared}");

    let answer = read(&admin);
    let events = answer["events"].as_array().expect("events");
    assert_eq!(events.len(), 3, "{answer}");

    // Newest first: the failed handler's record survived its failure.
    assert_eq!(events[0]["action"], "refund.attempted");
    assert_eq!(events[1]["action"], "refund.issued");
    assert_eq!(events[1]["actorKind"], "caller");
    assert!(
        events[1].get("actorId").is_none(),
        "an anonymous caller is nobody: {}",
        events[1]
    );
    assert!(events[1]["clientIp"].is_string(), "{}", events[1]);
    assert_eq!(events[2]["details"]["amount"], 10);
    assert!(
        events[2]["actorId"].is_string(),
        "the signed-in person is named by the engine: {}",
        events[2]
    );

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_or_an_administrator_reads_the_trail() {
    let server = AdminServer::start().await.expect("server should start");
    server.deploy_script(URI, SCRIPT).await;

    let stranger = read(&UserContext::authenticated("someone-else".to_string()));
    assert_eq!(stranger["status"], 403, "{stranger}");

    let bad = server
        .anonymous()
        .get(server.url("/audit/bad"))
        .send()
        .await
        .expect("request should be answered")
        .json::<serde_json::Value>()
        .await
        .expect("JSON");
    assert!(
        bad["error"].as_str().is_some_and(|e| e.contains("named")),
        "{bad}"
    );

    server.shutdown().await;
}
