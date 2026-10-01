//! How a stream refuses a connection.
//!
//! A stream's customization function is the only place per-resource
//! authorization can live — whose order `/orders/1234/events` is, is a fact
//! about the script's data model, and a capability names a verb the engine
//! understands. But it could not refuse cleanly: it returned a map of filter
//! criteria, so the only way to deny was to **throw**, which landed in
//! `build_stream_error_response` as an HTTP 500 with the throw message in the
//! body. A 500 tells the client to retry something that will be refused
//! again, and it leaks whatever the script said.
//!
//! A refusal is now a value: `{ deny: 401 | 403 | 404 | …, reason? }`. A
//! throw still means the function itself failed, which is the distinction
//! that was missing.

mod common;

use std::collections::HashMap;

use aiwebengine::auth::JsAuthContext;
use aiwebengine::repository;
use aiwebengine::resource_access::AccessDecision;
use common::{setup_env, test_mutex};

const URI: &str = "test_stream_authorization";

const SCRIPT: &str = r#"
function plainDeny(context) {
  return { deny: true };
}

function signIn(context) {
  return { deny: 401, reason: "sign in first" };
}

function notYours(context) {
  return { deny: 403, reason: "  not your order  " };
}

function hidden(context) {
  return { deny: 404 };
}

function serverError(context) {
  return { deny: 500 };
}

function allowWithCriteria(context) {
  return { orderId: "1234" };
}

function allowWithNothing(context) {
  return {};
}

function broken(context) {
  throw new Error("the callback itself is wrong");
}

function init(context) {
  routeRegistry.registerRoute("/stream-authorization", { stream: true, authorize: "plainDeny" });
}
"#;

async fn decide(function: &str) -> Result<AccessDecision, String> {
    let _ = repository::upsert_script(URI, SCRIPT);
    aiwebengine::js_engine::execute_stream_customization_function(
        URI,
        function,
        "/stream-authorization",
        &HashMap::new(),
        None::<JsAuthContext>,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bare_deny_is_forbidden() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    assert_eq!(
        decide("plainDeny").await.expect("should decide"),
        AccessDecision::Deny {
            status: 403,
            reason: None
        }
    );
}

/// "Sign in" and "not yours" are different answers, and a stream could not
/// tell them apart when the only way to refuse was to throw.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_carries_the_status_and_reason_it_chose() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    assert_eq!(
        decide("signIn").await.expect("should decide"),
        AccessDecision::Deny {
            status: 401,
            reason: Some("sign in first".to_string())
        }
    );
    assert_eq!(
        decide("notYours").await.expect("should decide"),
        AccessDecision::Deny {
            status: 403,
            reason: Some("not your order".to_string()),
        }
    );
    assert_eq!(
        decide("hidden").await.expect("should decide"),
        AccessDecision::Deny {
            status: 404,
            reason: None
        }
    );
}

/// A refusal is not the server failing, so a script cannot ask for one that
/// says it is — that is precisely the answer this shape replaced.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_cannot_deny_with_a_server_error() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    assert_eq!(
        decide("serverError").await.expect("should decide"),
        AccessDecision::Deny {
            status: 403,
            reason: None
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn criteria_still_mean_allowed() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let allowed = decide("allowWithCriteria").await.expect("should decide");
    match allowed {
        AccessDecision::Allow(criteria) => {
            assert_eq!(criteria.get("orderId").map(String::as_str), Some("1234"));
        }
        other => panic!("expected an allow, got {:?}", other),
    }

    assert_eq!(
        decide("allowWithNothing").await.expect("should decide"),
        AccessDecision::allow(),
        "a stream with no criteria to filter by is still a stream anyone may open"
    );
}

/// The distinction the deny shape exists to draw: deciding to refuse is not
/// the same as failing, and only the second is a 500.
#[tokio::test(flavor = "multi_thread")]
async fn a_throw_is_still_a_failure_rather_than_a_refusal() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let error = decide("broken")
        .await
        .expect_err("a throw should not decide");
    assert!(
        error.contains("the callback itself is wrong"),
        "unexpected: {error}"
    );
}
