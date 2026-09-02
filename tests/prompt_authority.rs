//! What an MCP prompt handler is allowed to do.
//!
//! A prompt handler ran as `UserContext::admin("mcp-prompt")` and was passed no
//! caller identity at all — the one request-driven path with nothing threaded
//! through to run as. MCP *tools* already took a `user_context` from the
//! session, so prompts were the outlier of a pair, and answering a prompt ran
//! script code holding `ManageScriptDatabase`, `WriteAssets` and
//! `AdministerEngine` for whoever asked.

mod common;

use aiwebengine::mcp;
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

/// The prompt handler reports what a schema change did, inside the message it
/// returns — a prompt's answer is the channel it has.
const SCRIPT: &str = r#"
function handlePrompt(context) {
  const verdict = String(database.createTable("prompt_authority_probe"));
  return {
    messages: [
      { role: "user", content: { type: "text", text: verdict } }
    ]
  };
}

function init(context) {
  mcpRegistry.registerPrompt(
    "promptAuthority",
    "Reports what authority the handler runs with",
    JSON.stringify([]),
    "handlePrompt"
  );
}
"#;

fn verdict_for(user_context: UserContext) -> String {
    let result =
        mcp::execute_mcp_prompt("promptAuthority", serde_json::json!({}), None, user_context)
            .expect("the prompt should be answered");

    result["messages"][0]["content"]["text"]
        .as_str()
        .unwrap_or("<no verdict>")
        .to_string()
}

/// Starts an engine with the script deployed, so `init()` registers the prompt.
///
/// The registry is populated at startup, and a prompt that is not in it is
/// never dispatched — so the server has to come up after the write.
async fn engine_with_the_prompt() -> TestContext {
    let context = TestContext::new();
    let _ = repository::upsert_script("test_prompt_authority", SCRIPT);
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    context
}

/// A caller with no session cannot reach a schema change through a prompt.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_handler_does_not_hold_more_than_the_caller_that_asked() {
    if should_skip_integration_tests() {
        return;
    }
    let context = engine_with_the_prompt().await;

    let verdict = verdict_for(UserContext::anonymous());
    assert!(
        verdict.contains("Insufficient permissions"),
        "an anonymous caller must not gain schema powers through a prompt; \
         the handler reported: {verdict}"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// An administrator's request still carries an administrator's authority.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_handler_keeps_what_the_caller_that_asked_holds() {
    if should_skip_integration_tests() {
        return;
    }
    let context = engine_with_the_prompt().await;

    let verdict = verdict_for(UserContext::admin("prompt-authority-admin".to_string()));
    assert!(
        !verdict.contains("Insufficient permissions"),
        "an administrator's request should carry their own authority into the \
         prompt handler; the handler reported: {verdict}"
    );

    context.cleanup().await.expect("Failed to cleanup");
}
