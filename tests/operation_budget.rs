//! Engine operations that write are budgeted per account, at the one place
//! every operation passes through — so `/engine/*`, `/mcp` and `engine.call`
//! share a budget. Reads are not counted.

mod common;

use aiwebengine::engine_api::execute_native_mcp_tool;
use aiwebengine::security::UserContext;
use aiwebengine::security::rate_limiting::{self, RateLimitKey, RateLimiter};
use common::{setup_env, test_pool};
use serde_json::{Value, json};
use std::sync::Arc;

fn status(answer: &Value) -> Option<u64> {
    answer.get("status").and_then(Value::as_u64)
}

/// A write that refuses for its own reason — no such script — so an answer
/// that is *not* 429 shows the operation ran.
fn write_as(user: &UserContext) -> Value {
    execute_native_mcp_tool(
        "label_revision",
        &json!({ "script": "test://budget/nothing", "revision": "1", "label": "x" }),
        user,
    )
    .expect("label_revision is an operation")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_account_that_spends_its_write_budget_is_refused_until_it_refills() {
    setup_env().await;
    let limiter = Arc::new(RateLimiter::new(test_pool().await));
    rate_limiting::initialize_shared(limiter.clone());

    let looping = UserContext::admin("budget-looping".to_string());
    let other = UserContext::admin("budget-other".to_string());

    assert_ne!(
        status(&write_as(&looping)),
        Some(429),
        "within budget it runs"
    );

    // Spend what is left in one go rather than in two hundred calls.
    let budget = limiter
        .config_named("operation")
        .expect("the operation budget is configured")
        .max_tokens;
    limiter
        .check_rate_limit(
            RateLimitKey::Operation("budget-looping".to_string()),
            budget - 1,
        )
        .await;

    let refused = write_as(&looping);
    assert_eq!(status(&refused), Some(429), "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("try again"),
        "the refusal says when: {refused}"
    );

    let read = execute_native_mcp_tool("list_scripts", &json!({}), &looping)
        .expect("list_scripts is an operation");
    assert_ne!(status(&read), Some(429), "reads are not counted: {read}");

    assert_ne!(
        status(&write_as(&other)),
        Some(429),
        "another account has its own budget"
    );
}
