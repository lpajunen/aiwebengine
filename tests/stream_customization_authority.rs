//! What a stream customization function is allowed to do.
//!
//! A customization function decides which messages a connecting client should
//! receive. It used to run as `UserContext::admin("stream-customization")`,
//! so opening an SSE connection — something any visitor can do — ran script
//! code holding `ManageScriptDatabase`, `WriteAssets` and `AdministerEngine`.
//! It serves a connection request, so it runs as whoever made it.
//!
//! Driven through `execute_stream_customization_function` directly, because
//! what the function returns is connection filter criteria: it never reaches
//! the client, so an HTTP test could not read the verdict back.
//!
//! Its other half — that the function is also where a connection is
//! *refused*, and how — is `tests/stream_authorization.rs`.

mod common;

use std::collections::HashMap;

use aiwebengine::auth::JsAuthContext;
use aiwebengine::repository;
use common::{setup_env, test_mutex};

/// The customization function reports what a schema change did, as a filter
/// criterion — the only thing it can return.
const SCRIPT: &str = r#"
function customize(context) {
  try {
    return { verdict: JSON.stringify(database.ensureTable("stream_authority_probe", { columns: [] })) };
  } catch (e) {
    return { verdict: e.message };
  }
}

function init(context) {
  routeRegistry.registerRoute("/stream-authority", { stream: true, authorize: "customize" });
}
"#;

async fn verdict_for(auth: Option<JsAuthContext>) -> String {
    let uri = "test_stream_customization_authority";
    let _ = repository::upsert_script(uri, SCRIPT);

    let decision = aiwebengine::js_engine::execute_stream_customization_function(
        uri,
        "customize",
        "/stream-authority",
        &HashMap::new(),
        auth,
    )
    .expect("the customization function should run");

    match decision {
        aiwebengine::resource_access::AccessDecision::Allow(criteria) => criteria
            .get("verdict")
            .cloned()
            .unwrap_or_else(|| "<no verdict>".to_string()),
        other => panic!("expected criteria, got {:?}", other),
    }
}

/// A connection with no session cannot reach a schema change.
#[tokio::test(flavor = "multi_thread")]
async fn a_customization_function_does_not_hold_more_than_the_connecting_caller() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let verdict = verdict_for(None).await;
    assert!(
        verdict.contains("Insufficient permissions"),
        "an anonymous connection must not gain schema powers through a stream \
         customization function; it reported: {verdict}"
    );
}

/// An administrator's connection still carries an administrator's authority.
#[tokio::test(flavor = "multi_thread")]
async fn a_customization_function_keeps_what_the_connecting_caller_holds() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let admin = JsAuthContext::authenticated(
        "stream-authority-admin".to_string(),
        Some("admin@example.com".to_string()),
        Some("Stream Authority Admin".to_string()),
        "local".to_string(),
        true,
        true,
    );

    let verdict = verdict_for(Some(admin)).await;
    assert!(
        !verdict.contains("Insufficient permissions"),
        "an administrator's connection should carry their own authority into \
         the customization function; it reported: {verdict}"
    );
}
