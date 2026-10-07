//! `rateLimit`: the script sizes a budget and the engine decides whose it is.

mod common;

use common::AdminServer;

const SCRIPT: &str = r#"
function spend(context) {
  const per = context.request.query.per || "caller";
  try {
    const result = rateLimit.consume("probe", { limit: 2, windowSeconds: 3600, per });
    return ResponseBuilder.json(result);
  } catch (e) {
    return ResponseBuilder.json({ error: e.message });
  }
}

function badName(context) {
  try {
    rateLimit.consume("no spaces", { limit: 1, windowSeconds: 1 });
    return ResponseBuilder.json({ error: null });
  } catch (e) {
    return ResponseBuilder.json({ error: e.message });
  }
}

function init() {
  routeRegistry.registerRoute("/rate-limit/spend", { handler: "spend" });
  routeRegistry.registerRoute("/rate-limit/bad-name", { handler: "badName" });
}
"#;

async fn spend(request: reqwest::RequestBuilder) -> serde_json::Value {
    request
        .send()
        .await
        .expect("request should be answered")
        .json()
        .await
        .expect("the handler answers JSON")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_spends_their_own_budget_and_no_one_elses() {
    let server = AdminServer::start().await.expect("server should start");
    server.deploy_script("rate-limit-probe", SCRIPT).await;
    let url = server.url("/rate-limit/spend");
    let anonymous = || server.anonymous().get(&url);

    let first = spend(anonymous()).await;
    assert_eq!(first["allowed"], true, "{first}");
    assert_eq!(first["remaining"], 1, "{first}");
    assert_eq!(spend(anonymous()).await["allowed"], true);

    let refused = spend(anonymous()).await;
    assert_eq!(refused["allowed"], false, "the third spend: {refused}");
    assert!(
        refused["retryAfterSeconds"].as_u64().is_some_and(|s| s > 0),
        "a refusal says when to come back: {refused}"
    );

    // The signed-in person has a bucket of their own, so another caller
    // exhausting theirs costs them nothing.
    let person = spend(server.get("/rate-limit/spend")).await;
    assert_eq!(person["allowed"], true, "{person}");

    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bucket_name_or_owner_the_engine_does_not_know_is_refused() {
    let server = AdminServer::start().await.expect("server should start");
    server.deploy_script("rate-limit-probe", SCRIPT).await;

    let bad_name = spend(server.anonymous().get(server.url("/rate-limit/bad-name"))).await;
    assert!(
        bad_name["error"]
            .as_str()
            .is_some_and(|e| e.contains("not a bucket name")),
        "{bad_name}"
    );

    let bad_per = spend(
        server
            .anonymous()
            .get(server.url("/rate-limit/spend?per=header")),
    )
    .await;
    assert!(
        bad_per["error"].as_str().is_some_and(|e| e.contains("per")),
        "{bad_per}"
    );

    server.shutdown().await;
}
