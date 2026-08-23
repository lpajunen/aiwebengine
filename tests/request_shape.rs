//! The shape `context.request` presents to a handler.
//!
//! The response side of a script grew methods when `fetch` did. The request
//! side did not, so the same script parsed a body it received differently from
//! a body it fetched, read headers out of an object where the case had to match
//! the client's exactly, and could not see which host a request arrived on.
//!
//! These run against a real server, because the things under test — the Host
//! header, a repeated query parameter, the capitalisation a client chose — only
//! exist once a request has actually been made.

mod common;

use aiwebengine::repository;
use common::{TestContext, should_skip_integration_tests, wait_for_server};

/// Deploys `script` and serves it, returning the running server's base URL.
async fn serve(context: &TestContext, script_uri: &str, script: &str) -> String {
    let _ = repository::upsert_script(script_uri, script);
    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    format!("http://127.0.0.1:{}", port)
}

/// A handler that answers with whatever the test asked about the request.
///
/// Each test claims its own `route`. Every test in this file runs against the
/// same repository and the same route index, so two of them registering one
/// path would race for it and each would sometimes get the other's handler.
fn probe_script(route: &str, body: &str) -> String {
    format!(
        r#"
        function handler(context) {{
          const request = context.request;
          const answer = (function () {{ {body} }})();
          return {{ status: 200, body: JSON.stringify(answer), contentType: "application/json" }};
        }}

        function init(context) {{
          routeRegistry.registerRoute("{route}", "handler", "GET");
          routeRegistry.registerRoute("{route}", "handler", "POST");
          return {{ success: true }};
        }}
        "#,
        route = route,
        body = body
    )
}

/// A header the client spelled one way and the script reads another. This was
/// the trap: which capitalisation arrives is the client's choice, and the plain
/// object made the script depend on it.
#[tokio::test(flavor = "multi_thread")]
async fn a_header_is_found_whatever_its_case() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_headers",
        &probe_script(
            "/probe/headers",
            r#"
            return {
              get: request.headers.get("X-Custom-Token"),
              lowerGet: request.headers.get("x-custom-token"),
              has: request.headers.has("X-CUSTOM-TOKEN"),
              missing: request.headers.get("x-absent"),
              // The plain-object reading that scripts already use.
              indexed: request.headers["x-custom-token"],
            };
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/probe/headers", base))
        .header("X-Custom-Token", "abc123")
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["get"], "abc123");
    assert_eq!(body["lowerGet"], "abc123");
    assert_eq!(body["has"], true);
    assert_eq!(body["missing"], serde_json::Value::Null);
    assert_eq!(body["indexed"], "abc123");

    context.cleanup().await.expect("Failed to cleanup");
}

/// Every `request.headers[name]` already written has to keep working, and
/// enumeration with it.
#[tokio::test(flavor = "multi_thread")]
async fn headers_still_read_as_a_plain_object() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_headers_object",
        &probe_script(
            "/probe/headers-object",
            r#"
            const names = Object.keys(request.headers);
            return {
              hasHost: names.indexOf("host") !== -1,
              enumerable: names.indexOf("x-custom-token") !== -1,
              spread: { ...request.headers }["x-custom-token"],
              inOperator: "x-custom-token" in request.headers,
              absent: "x-absent" in request.headers,
            };
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/probe/headers-object", base))
        .header("X-Custom-Token", "abc123")
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["hasHost"], true);
    assert_eq!(body["enumerable"], true);
    assert_eq!(body["spread"], "abc123");
    assert_eq!(body["inOperator"], true);
    assert_eq!(body["absent"], false);

    context.cleanup().await.expect("Failed to cleanup");
}

/// The same parse, spelled the same way, whichever end the body came from.
#[tokio::test(flavor = "multi_thread")]
async fn a_body_reads_the_way_a_fetch_response_does() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_body",
        &probe_script(
            "/probe/body",
            r#"
            const parsed = request.json();
            return {
              text: request.text(),
              name: parsed.name,
              count: parsed.count,
              // Still the raw string it always was.
              body: request.body,
            };
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/probe/body", base))
        .header("content-type", "application/json")
        .body(r#"{"name":"ada","count":2}"#)
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["name"], "ada");
    assert_eq!(body["count"], 2);
    assert_eq!(body["text"], r#"{"name":"ada","count":2}"#);
    assert_eq!(body["body"], r#"{"name":"ada","count":2}"#);

    context.cleanup().await.expect("Failed to cleanup");
}

/// A body that is not JSON fails where the script asks for the parse, not on
/// the way in from a request that arrived perfectly well.
#[tokio::test(flavor = "multi_thread")]
async fn a_body_that_is_not_json_throws_at_the_parse() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_body_invalid",
        &probe_script(
            "/probe/body-invalid",
            r#"
            try {
              request.json();
              return { threw: false };
            } catch (e) {
              return { threw: true, name: e.name, text: request.text() };
            }
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/probe/body-invalid", base))
        .body("not json at all")
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["threw"], true);
    assert_eq!(body["name"], "SyntaxError");
    assert_eq!(body["text"], "not json at all");

    context.cleanup().await.expect("Failed to cleanup");
}

/// The gap `request.query` cannot close: it is built from a map, so one of a
/// repeated parameter's values is already gone by the time a script sees it.
/// `searchParams` is parsed from the URL, where both survive.
#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_query_parameter_survives_in_search_params() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_query",
        &probe_script(
            "/probe/query",
            r#"
            return {
              all: request.searchParams.getAll("tag"),
              first: request.searchParams.get("tag"),
              size: request.searchParams.size,
              has: request.searchParams.has("tag"),
              missing: request.searchParams.get("absent"),
              spaces: request.searchParams.get("note"),
              // The plain object keeps only one of the two.
              query: request.query.tag,
            };
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/probe/query?tag=a&tag=b&note=hello+world", base))
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["all"], serde_json::json!(["a", "b"]));
    assert_eq!(body["first"], "a");
    assert_eq!(body["size"], 3);
    assert_eq!(body["has"], true);
    assert_eq!(body["missing"], serde_json::Value::Null);
    assert_eq!(body["spaces"], "hello world");
    // What `query` can manage: one value, and no way to know the other existed.
    assert!(body["query"] == "a" || body["query"] == "b");

    context.cleanup().await.expect("Failed to cleanup");
}

/// `path` cannot say which of the engine's hosts served a request. `url` can.
#[tokio::test(flavor = "multi_thread")]
async fn the_request_carries_the_absolute_url_it_arrived_on() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_url",
        &probe_script(
            "/probe/url",
            r#"
            return {
              url: request.url,
              path: request.path,
              hasOrigin: /^https?:\/\/[^/]+\//.test(request.url),
              carriesQuery: request.url.indexOf("?tag=a") !== -1,
            };
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/probe/url?tag=a", base))
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["path"], "/probe/url");
    assert_eq!(body["hasOrigin"], true, "url was {:?}", body["url"]);
    assert_eq!(body["carriesQuery"], true, "url was {:?}", body["url"]);

    context.cleanup().await.expect("Failed to cleanup");
}

/// `Headers` and `URLSearchParams` are ordinary globals, usable for building a
/// request as much as for reading one.
#[tokio::test(flavor = "multi_thread")]
async fn headers_and_search_params_are_constructible() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_globals",
        &probe_script(
            "/probe/globals",
            r#"
            const headers = new Headers({ "Content-Type": "text/plain" });
            headers.append("Accept", "text/html");
            headers.append("Accept", "application/json");

            const params = new URLSearchParams("a=1&b=2");
            params.append("a", "3");
            const collected = [];
            for (const [name, value] of params) {
              collected.push(name + "=" + value);
            }
            params.set("a", "9");

            return {
              contentType: headers.get("content-type"),
              // Repeated headers combine, as the spec says they read.
              accept: headers.get("accept"),
              iterated: collected,
              // `set` replaces the first and drops the rest.
              afterSet: params.getAll("a"),
              serialised: params.toString(),
              fromObject: new URLSearchParams({ x: "1" }).toString(),
            };
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/probe/globals", base))
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["contentType"], "text/plain");
    assert_eq!(body["accept"], "text/html, application/json");
    assert_eq!(body["iterated"], serde_json::json!(["a=1", "b=2", "a=3"]));
    assert_eq!(body["afterSet"], serde_json::json!(["9"]));
    assert_eq!(body["serialised"], "a=9&b=2");
    assert_eq!(body["fromObject"], "x=1");

    context.cleanup().await.expect("Failed to cleanup");
}

/// Nothing above changes the fields a handler already reads.
#[tokio::test(flavor = "multi_thread")]
async fn the_fields_a_handler_already_used_are_untouched() {
    if should_skip_integration_tests() {
        return;
    }
    let context = TestContext::new();
    let base = serve(
        &context,
        "test_request_untouched",
        &probe_script(
            "/probe/untouched",
            r#"
            return {
              method: request.method,
              path: request.path,
              queryType: typeof request.query,
              formType: typeof request.form,
              paramsType: typeof request.params,
              filesIsArray: Array.isArray(request.files),
              authPresent: typeof request.auth === "object",
              bodyType: typeof request.body,
            };
            "#,
        ),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/probe/untouched", base))
        .body("x=1")
        .send()
        .await
        .expect("request failed");
    let body: serde_json::Value = response.json().await.expect("json");

    assert_eq!(body["method"], "POST");
    assert_eq!(body["path"], "/probe/untouched");
    assert_eq!(body["queryType"], "object");
    assert_eq!(body["formType"], "object");
    assert_eq!(body["paramsType"], "object");
    assert_eq!(body["filesIsArray"], true);
    assert_eq!(body["authPresent"], true);
    assert_eq!(body["bodyType"], "string");

    context.cleanup().await.expect("Failed to cleanup");
}
