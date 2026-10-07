//! Another script's MCP tools, called from a script with `tools`.
//!
//! A tool called this way runs as whoever the calling execution acts for —
//! the in-process twin of `tools/call` on `/mcp`. These cover what the global
//! adds: that it reaches script tools and not the engine's own, that a
//! read-only call cannot write, and that the engine's own executions, which
//! act for nobody, cannot call at all.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::{setup_env, test_mutex};
use serde_json::{Value, json};

const PROVIDER: &str = "test://tools-from-js/provider";
const CALLER: &str = "test://tools-from-js/caller";

const PROVIDER_SOURCE: &str = r#"
function echo(context) {
  return { echoed: context.args };
}

function remember(context) {
  try {
    scriptStorage.setItem("tfj:last", String(context.args.value));
    return { wrote: true };
  } catch (e) {
    return { wrote: false, why: e.name + ": " + e.message };
  }
}

function init() {
  mcpRegistry.registerTool("tfj_echo", {
    description: "Answers with its arguments",
    inputSchema: { type: "object", properties: { x: { type: "number" } } },
    handler: "echo",
  });
  mcpRegistry.registerTool("tfj_remember", {
    description: "Stores a value",
    inputSchema: { type: "object", properties: { value: { type: "string" } } },
    handler: "remember",
  });
}
"#;

async fn publish_provider() {
    repository::upsert_script(PROVIDER, PROVIDER_SOURCE).expect("provider should store");
    aiwebengine::script_init::ScriptInitializer::new(5_000)
        .initialize_script(PROVIDER, false)
        .await
        .expect("the provider should initialize and register its tools");
    repository::upsert_script(CALLER, "function init() {}").expect("caller should store");
}

async fn run(source: &str, user: UserContext) -> EvalReport {
    let request = EvalRequest {
        timeout_ms: Some(15_000),
        rollback: false,
        ..EvalRequest::new(CALLER.to_string(), source.to_string(), user)
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

fn value_of(report: EvalReport) -> Value {
    assert!(
        report.ok,
        "the snippet should run: {:?}",
        report.outcome.error
    );
    report
        .outcome
        .value
        .expect("the snippet answers with a value")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_lists_and_calls_another_scripts_tool() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    publish_provider().await;

    let listed = value_of(
        run(
            "tools.list('tfj_').map(function (t) { return t.name + '@' + t.script; })",
            UserContext::authenticated("u1".to_string()),
        )
        .await,
    );
    assert_eq!(
        listed,
        json!([
            format!("tfj_echo@{PROVIDER}"),
            format!("tfj_remember@{PROVIDER}")
        ])
    );

    let answer = value_of(
        run(
            "tools.call('tfj_echo', { x: 7 })",
            UserContext::authenticated("u1".to_string()),
        )
        .await,
    );
    assert_eq!(answer, json!({ "echoed": { "x": 7 } }));
}

/// `readOnly` is enforced underneath the tool, not asked of it: the same
/// call writes without it and is refused with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_call_cannot_write() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    publish_provider().await;

    let read_only = value_of(
        run(
            "tools.call('tfj_remember', { value: 'a' }, { readOnly: true })",
            UserContext::authenticated("u1".to_string()),
        )
        .await,
    );
    assert_eq!(read_only["wrote"], json!(false), "{read_only}");
    assert!(
        read_only["why"]
            .as_str()
            .is_some_and(|why| why.contains("write_storage")),
        "the refusal should name what was withheld: {read_only}"
    );

    let writing = value_of(
        run(
            "tools.call('tfj_remember', { value: 'b' })",
            UserContext::authenticated("u1".to_string()),
        )
        .await,
    );
    assert_eq!(writing, json!({ "wrote": true }));
}

/// The engine's own tools are `engine.call`'s, behind its own grant, so
/// naming one here finds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_engines_own_tools_are_not_reachable() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    publish_provider().await;

    let thrown = value_of(
        run(
            "try { tools.call('list_users', {}); 'called' } catch (e) { e.name }",
            UserContext::admin("administers".to_string()),
        )
        .await,
    );
    assert_eq!(thrown, json!("NotFoundError"));

    let listed = value_of(
        run(
            "tools.list('list_users').length",
            UserContext::admin("administers".to_string()),
        )
        .await,
    );
    assert_eq!(listed, json!(0));
}

/// A scheduled job or `init()` acts for nobody, so there is nobody to call a
/// tool as.
#[tokio::test(flavor = "multi_thread")]
async fn an_execution_acting_for_nobody_cannot_call() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    publish_provider().await;

    let thrown = value_of(
        run(
            "try { tools.call('tfj_echo', {}); 'called' } catch (e) { e.name + ': ' + e.message }",
            UserContext::engine_actor("scheduler".to_string()),
        )
        .await,
    );
    let thrown = thrown.as_str().unwrap_or_default().to_string();
    assert!(
        thrown.starts_with("SecurityError") && thrown.contains("call_tools"),
        "got: {thrown}"
    );
}

/// Code narrowed with `sandbox.run` holds the call only when given it by
/// name, which is how an agent's model-written code stays without it.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandbox_holds_the_call_only_when_named() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    publish_provider().await;

    let answer = value_of(
        run(
            r#"[
              sandbox.run("try { tools.call('tfj_echo', {}); 'called' } catch (e) { e.name }",
                          { capabilities: ['read_assets'] }).value,
              sandbox.run("tools.call('tfj_echo', { x: 1 }).echoed.x",
                          { capabilities: ['call_tools'] }).value
            ]"#,
            UserContext::authenticated("u1".to_string()),
        )
        .await,
    );
    assert_eq!(answer, json!(["SecurityError", 1]));
}
