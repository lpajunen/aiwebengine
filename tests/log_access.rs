//! Who may read a script's log: whoever answers for the script.
//!
//! `ViewLogs` belongs to every signed-in tier, so it cannot be the whole check:
//! a log carries whatever a script wrote while serving people.

mod common;

use aiwebengine::engine_api::{execute_native_mcp_tool, upsert_root_authorized};
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use common::setup_env;
use serde_json::json;

fn messages(answer: &serde_json::Value) -> Vec<String> {
    answer["logs"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing: {answer}"))
        .iter()
        .filter_map(|entry| entry["message"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_or_an_administrator_reads_a_scripts_log() {
    setup_env().await;

    let owner = UserContext::editor("log-owner".to_string());
    let other = UserContext::editor("log-stranger".to_string());
    let uri = "log-access-probe";
    upsert_root_authorized(&owner, uri, Some("main.ts"), "// logs\n", None)
        .expect("the owner writes the script");
    repository::insert_log_message(uri, "a customer's address", "INFO");

    let read = |user: &UserContext, args: serde_json::Value| {
        execute_native_mcp_tool("read_logs", &args, user).expect("read_logs is an operation")
    };

    let by_owner = read(&owner, json!({ "script": uri }));
    assert!(
        messages(&by_owner).contains(&"a customer's address".to_string()),
        "{by_owner}"
    );

    let by_stranger = read(&other, json!({ "script": uri }));
    assert_eq!(by_stranger["status"], 403, "{by_stranger}");

    // Naming no script spans the caller's own, so it cannot be used to read
    // everybody else's either.
    let stranger_all = read(&other, json!({}));
    assert!(
        !messages(&stranger_all).contains(&"a customer's address".to_string()),
        "{stranger_all}"
    );
    let owner_all = read(&owner, json!({}));
    assert!(
        messages(&owner_all).contains(&"a customer's address".to_string()),
        "{owner_all}"
    );

    let anonymous = read(&UserContext::anonymous(), json!({ "script": uri }));
    assert!(
        anonymous["status"].as_u64().is_some_and(|s| s >= 400),
        "{anonymous}"
    );

    let admin = read(
        &UserContext::admin("log-admin".to_string()),
        json!({ "script": uri }),
    );
    assert!(
        messages(&admin).contains(&"a customer's address".to_string()),
        "{admin}"
    );
}
