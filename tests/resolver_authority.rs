//! What a GraphQL resolver is allowed to do.
//!
//! A resolver used to run as `UserContext::admin("graphql-resolver")` while the
//! caller's real identity was passed alongside for the JavaScript `auth` object
//! to read. Anyone able to send a query therefore reached the full
//! administrator set — `ManageScriptDatabase`, `WriteAssets`,
//! `AdministerEngine` — with no script cooperation required, which made this
//! the most directly reachable of the engine's admin contexts. A resolver
//! serves a request, so it runs as the requester.

mod common;

use std::time::Duration;

use aiwebengine::repository;
use common::{AdminServer, TestContext, should_skip_integration_tests, wait_for_server};

/// A resolver attempts a schema change, which needs `ManageScriptDatabase`.
///
/// It answers with what it saw rather than throwing, so the verdict travels
/// back in the GraphQL response itself — the one channel a caller of any tier
/// is guaranteed to have.
fn script(field: &str) -> String {
    format!(
        r#"
graphQLRegistry.registerQuery(
  "{field}",
  "type Query {{ {field}: String }}",
  "{field}Resolver",
  "external",
);

function {field}Resolver() {{
  return String(database.createTable("resolver_authority_probe"));
}}
"#
    )
}

async fn query(client: &reqwest::Client, url: &str, field: &str) -> String {
    let response = client
        .post(url)
        .json(&serde_json::json!({ "query": format!("{{ {field} }}") }))
        .send()
        .await
        .expect("the GraphQL endpoint should answer");
    assert_eq!(response.status(), 200, "GraphQL answers 200 for a query");
    response.text().await.expect("body")
}

/// An anonymous caller cannot reach a schema change through a resolver.
#[tokio::test(flavor = "multi_thread")]
async fn a_resolver_does_not_hold_more_than_the_caller_that_queried_it() {
    if should_skip_integration_tests() {
        return;
    }

    let context = TestContext::new();
    let field = "resolverAuthorityAnonymous";
    let _ = repository::upsert_script("test_resolver_authority_anonymous", &script(field));

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let body = query(
        &client,
        &format!("http://127.0.0.1:{}/graphql", port),
        field,
    )
    .await;

    assert!(
        body.contains("Insufficient permissions"),
        "an anonymous caller must not gain schema powers through a resolver; \
         the resolver reported: {body}"
    );

    context.cleanup().await.expect("Failed to cleanup");
}

/// The other half: an administrator's query still carries an administrator's
/// authority into the resolver.
#[tokio::test(flavor = "multi_thread")]
async fn a_resolver_keeps_what_the_caller_that_queried_it_holds() {
    if should_skip_integration_tests() {
        return;
    }

    common::setup_env().await;
    let field = "resolverAuthorityAdmin";
    let _ = repository::upsert_script("test_resolver_authority_admin", &script(field));

    let engine = AdminServer::start().await.expect("server failed to start");
    let body = query(&engine.client(), &engine.url("/graphql"), field).await;

    assert!(
        !body.contains("Insufficient permissions"),
        "an administrator's query should carry their own authority into the \
         resolver; the resolver reported: {body}"
    );

    engine.shutdown().await;
}
