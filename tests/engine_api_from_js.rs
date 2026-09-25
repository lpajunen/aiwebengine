//! The engine's own management tools, called from a script.
//!
//! What matters here is not that the call works but *who it works for*. Every
//! call goes through `engine_api::execute_native_mcp_tool` — the same function
//! `/mcp` calls — so the tools' own authorization is already covered by their
//! own tests. These cover the two things installing them in JavaScript adds:
//! that the global appears only where a credential put it there, and that the
//! calling context is what decides, not the fact of being a script.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::{AdminServer, setup_env, test_mutex};
use serde_json::Value;

/// `script_eval` deliberately withholds the engine API — it is shared by
/// `/engine/eval` and `sandbox.run`, and the second must not have it. So these
/// reach a *request* execution, which is where it lives.
async fn in_a_request(uri: &str, source: &str, user: UserContext) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should store");
    let request = EvalRequest {
        timeout_ms: Some(15_000),
        rollback: false,
        ..EvalRequest::new(uri.to_string(), source.to_string(), user)
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// An evaluation is not a request, so the global is absent there — and its
/// absence is the point rather than an inconvenience.
///
/// `sandbox.run` shares this path. Model-authored code that could name a tool
/// would reach `read_logs` with the `view_logs` an agent grants for `console`,
/// and `read_logs` takes any script's URI as an argument.
#[tokio::test(flavor = "multi_thread")]
async fn an_evaluation_and_a_sandbox_do_not_get_the_engine_api() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = in_a_request(
        "test://engine-api/absent",
        "typeof engine",
        UserContext::admin("administers".to_string()),
    )
    .await;

    assert!(
        report.ok,
        "the snippet should run: {:?}",
        report.outcome.error
    );
    assert_eq!(
        report.outcome.value,
        Some(Value::from("undefined")),
        "an evaluation must not reach the engine's management tools, \
         because `sandbox.run` shares the path"
    );
}

/// Every name the engine serves is discoverable, so a script does not embed a
/// list that drifts from what this deployment actually has.
#[tokio::test(flavor = "multi_thread")]
async fn the_tool_list_comes_from_the_engine_rather_than_from_the_script() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Read through the same function the global exposes, since the global
    // itself is only installed on a request execution.
    let descriptors = aiwebengine::engine_api::native_mcp_tool_descriptors();

    assert!(
        descriptors.iter().any(|tool| tool.name == "list_users"),
        "the engine should publish the tool an agent asked 'who are the users' needs"
    );
    assert!(
        descriptors.iter().all(|tool| !tool.description.is_empty()),
        "a tool with no description is one a model cannot choose"
    );
}

/// The whole authorization story, at the boundary the global adds: the same
/// tool, the same arguments, two contexts, two answers.
///
/// `list_users` takes `AdministerEngine`, so an ordinary caller is refused and
/// an administrator is not — decided by `execute_native_mcp_tool`, which is
/// the function `/mcp` calls. There is deliberately no second answer to this
/// question.
#[tokio::test(flavor = "multi_thread")]
async fn the_calling_context_decides_and_not_the_fact_of_being_a_script() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let refused = aiwebengine::engine_api::execute_native_mcp_tool(
        "list_users",
        &serde_json::json!({}),
        &UserContext::authenticated("ordinary".to_string()),
    )
    .expect("the tool exists");
    assert!(
        refused.get("error").is_some(),
        "an ordinary caller must not list the engine's users: {refused}"
    );

    let allowed = aiwebengine::engine_api::execute_native_mcp_tool(
        "list_users",
        &serde_json::json!({}),
        &UserContext::admin("administers".to_string()),
    )
    .expect("the tool exists");
    assert!(
        allowed.get("users").is_some(),
        "an administrator should get the listing: {allowed}"
    );
}

/// A name this engine does not serve is a mistake in the script, not a
/// permission problem, and says so.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_tool_is_not_a_refusal() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    assert!(
        aiwebengine::engine_api::execute_native_mcp_tool(
            "list_everything_please",
            &serde_json::json!({}),
            &UserContext::admin("administers".to_string()),
        )
        .is_none(),
        "an unknown tool should be distinguishable from one that refused"
    );
}

/// The positive case, through a real request: a script route calls
/// `engine.call("list_users")` and answers with what the engine told it.
///
/// This is the test that decides whether the feature exists. Everything above
/// covers a boundary; this covers the path an agent actually takes — globals
/// installed on a request execution, a tool dispatched, the caller's own
/// authority deciding, and JSON coming back out through the handler.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_route_can_list_users_as_the_administrator_calling_it() {
    let _guard = test_mutex().lock().await;
    let engine = AdminServer::start().await.expect("server should start");

    let script_uri = "test://engine-api/route";
    repository::upsert_script(
        script_uri,
        r#"
        function whoAreTheUsers() {
          // The shape an agent's tool call reduces to.
          const answer = engine.call("list_users", {});
          return {
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({ count: answer.count, sawEngine: typeof engine }),
          };
        }

        function init() {
          routeRegistry.registerRoute('/who-are-the-users', 'whoAreTheUsers', 'GET');
        }
        "#,
    )
    .expect("script should store");

    // Through the initializer rather than `call_init_if_exists`: the latter
    // runs `init()` and hands back what it registered, while the initializer
    // is what *stores* those registrations on the script's metadata — which is
    // where `find_route_handler` looks.
    aiwebengine::script_init::ScriptInitializer::new(5_000)
        .initialize_script(script_uri, false)
        .await
        .expect("the script should initialize and register its route");

    // As the administrator the harness signs in as: the route runs under the
    // requesting user's context, which is the whole authorization story.
    let response = engine
        .request(reqwest::Method::GET, "/who-are-the-users")
        .send()
        .await
        .expect("the request should be served");

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "the route should answer: {body}"
    );

    let answer: Value = serde_json::from_str(&body).expect("the handler returns JSON");
    assert_eq!(
        answer["sawEngine"], "object",
        "a request execution should have the engine global"
    );
    assert!(
        answer["count"].as_i64().is_some_and(|count| count >= 1),
        "the engine has at least the administrator this test signed in as: {body}"
    );

    engine.shutdown().await;
}
