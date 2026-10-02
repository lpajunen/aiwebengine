//! The engine's HTTP surface, generated from its operation table.
//!
//! `engine_api::native_tools` is the one description of what the engine can
//! be asked to do: a name, a schema, and a function over the arguments and the
//! caller. `/mcp` `tools/call` and `engine.call` read it directly. This module
//! is the third reader — it publishes every entry as `POST /engine/{name}`
//! (and `GET /engine/{name}?arg=...` for the ones that only read), and renders
//! the same entries into `/engine/openapi.json`.
//!
//! Before it, each operation was also a hand-written axum handler with its own
//! argument parsing, its own error mapping and its own response shape, plus a
//! `#[utoipa::path]` annotation describing the handler a third time. Two
//! callers of the same core disagreed about field names (`size` against
//! `bytes`) and about what a refusal looks like.
//!
//! What an operation answers is its result, as JSON. A failure is the
//! `{ "error": ... }` object the operation already returns to MCP; the one
//! thing this layer adds is the status line, chosen in [`status_for_error`]
//! rather than at every call site.

use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, RawQuery};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use tracing::warn;

use crate::auth::AuthUser;
use crate::engine_api;
use crate::security::UserContext;

/// Operations that only read, and so are also offered as `GET` with their
/// arguments in the query string.
///
/// A name here is a promise: nothing under it writes, enqueues or deletes.
/// `POST` stays available for all of them, since an argument that does not
/// fit a query string (a long `pattern`) still needs somewhere to go.
const READ_ONLY: &[&str] = &[
    "list_scripts",
    "search_files",
    "read_logs",
    "list_routes",
    "exposure_report",
    "read_init_status",
    "list_files",
    "read_file",
    "list_script_owners",
    "list_secrets",
    "list_users",
    "get_script_hosts",
    "list_revisions",
    "diff_revisions",
    "get_script_limits",
    "list_tasks",
    "list_git_credentials",
    "list_git_bindings",
    "get_git_status",
    "get_deployment",
];

/// Whether `name` is offered as `GET` as well as `POST`.
pub fn is_read_only(name: &str) -> bool {
    READ_ONLY.contains(&name)
}

/// The status line for an operation that answered `{ "error": message }`.
///
/// A refusal says what kind it is in a `status` field, set where it is made
/// ([`engine_api::refuse`]). A result without one — an error a layer below
/// still reports as a string — falls back to what the text implies
/// ([`engine_api::Refusal::from_message`]).
pub fn status_of(result: &Value, message: &str) -> StatusCode {
    result
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|code| u16::try_from(code).ok())
        .and_then(|code| StatusCode::from_u16(code).ok())
        .filter(|status| status.is_client_error() || status.is_server_error())
        .unwrap_or_else(|| {
            StatusCode::from_u16(engine_api::Refusal::from_message(message).status())
                .unwrap_or(StatusCode::BAD_REQUEST)
        })
}

/// Query-string arguments as the JSON object the operation expects.
///
/// A query carries only strings, so each value is read against the type its
/// schema property names: `limit=5` becomes the integer 5 and `flag=true` the
/// boolean, and an array or object is accepted as JSON text. A key the schema
/// does not mention stays a string and the operation decides what to make of
/// it.
fn args_from_query(query: &str, schema: &Value) -> Value {
    let properties = schema.get("properties");
    let mut args = Map::new();
    for (key, raw) in url::form_urlencoded::parse(query.as_bytes()) {
        let declared = properties
            .and_then(|p| p.get(key.as_ref()))
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str);
        let value = match declared {
            Some("integer") | Some("number") => raw
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| raw.parse::<f64>().map(Value::from))
                .unwrap_or_else(|_| Value::String(raw.to_string())),
            Some("boolean") => match raw.as_ref() {
                "true" | "1" => Value::Bool(true),
                "false" | "0" => Value::Bool(false),
                _ => Value::String(raw.to_string()),
            },
            Some("array") | Some("object") => {
                serde_json::from_str(&raw).unwrap_or_else(|_| Value::String(raw.to_string()))
            }
            _ => Value::String(raw.to_string()),
        };
        args.insert(key.into_owned(), value);
    }
    Value::Object(args)
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn error_body(message: &str) -> Value {
    json!({ "error": message })
}

/// Run one operation for one HTTP request.
///
/// Public so a test can drive an operation the way the router does, session
/// and all, without a server.
pub async fn call_operation(
    name: &'static str,
    method: Method,
    auth_user: Option<Extension<AuthUser>>,
    query: Option<String>,
    body: Bytes,
) -> Response {
    let user: UserContext = UserContext::for_session(
        auth_user
            .as_deref()
            .map(AuthUser::roles)
            .unwrap_or_default(),
    );

    let Some(schema) = engine_api::native_mcp_tool_descriptors()
        .into_iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.input_schema)
    else {
        return json_response(StatusCode::NOT_FOUND, &error_body("no such operation"));
    };

    let args = if method == Method::GET {
        if !is_read_only(name) {
            return json_response(
                StatusCode::METHOD_NOT_ALLOWED,
                &error_body(&format!("{name} changes something: use POST")),
            );
        }
        args_from_query(query.as_deref().unwrap_or(""), &schema)
    } else if body.iter().all(u8::is_ascii_whitespace) {
        Value::Object(Map::new())
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(value @ Value::Object(_)) => value,
            Ok(_) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    &error_body("the request body must be a JSON object"),
                );
            }
            Err(e) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    &error_body(&format!("the request body is not valid JSON: {e}")),
                );
            }
        }
    };

    let ceiling = engine_api::native_tool_ceiling_ms(name).unwrap_or(30_000);
    let work = tokio::task::spawn_blocking(move || {
        engine_api::execute_native_mcp_tool(name, &args, &user)
    });
    let result = match tokio::time::timeout(Duration::from_millis(ceiling + 5_000), work).await {
        Ok(Ok(Some(result))) => result,
        Ok(Ok(None)) => {
            return json_response(StatusCode::NOT_FOUND, &error_body("no such operation"));
        }
        Ok(Err(e)) => {
            warn!("operation {name} panicked or was cancelled: {e}");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &error_body("the operation did not complete"),
            );
        }
        Err(_) => {
            return json_response(
                StatusCode::GATEWAY_TIMEOUT,
                &error_body(&format!("{name} timed out")),
            );
        }
    };

    // A report that says `ok: false` is an answer: the snippet that threw, the
    // check that found a problem. The request itself worked.
    let is_report = result.get("ok").is_some_and(Value::is_boolean);
    match result
        .get("error")
        .and_then(Value::as_str)
        .filter(|_| !is_report)
    {
        Some(message) => json_response(
            status_of(&result, message),
            &result_with_error(&result, message),
        ),
        None => json_response(StatusCode::OK, &result),
    }
}

/// An error result with a timestamp, so a failure carries the same field a
/// success does.
fn result_with_error(result: &Value, message: &str) -> Value {
    let mut body = result.clone();
    if let Value::Object(map) = &mut body {
        map.entry("error")
            .or_insert_with(|| Value::String(message.to_string()));
        map.entry("timestamp")
            .or_insert_with(|| Value::String(chrono::Utc::now().to_rfc3339()));
    }
    body
}

/// Largest request body an operation takes, in bytes, or `None` to inherit the
/// management router's `security.max_request_body_bytes`.
///
/// A file's content travels base64-encoded, so the three operations that carry
/// one need more room than the default allows — the reasoning
/// [`engine_api::MAX_ASSET_BODY_BYTES`] records.
fn body_limit(name: &str) -> Option<usize> {
    match name {
        "write_files" => Some(engine_api::MAX_BATCH_BODY_BYTES),
        "write_file" | "create_file" | "edit_file" => Some(engine_api::MAX_ASSET_BODY_BYTES),
        _ => None,
    }
}

/// One axum route per operation in the table.
///
/// Registered by name rather than as a `/engine/{operation}` wildcard: a
/// wildcard would answer for every single-segment path under `/engine/`,
/// including the ones that belong to the dynamic router (`script_updates`
/// being a stream, not an operation).
pub fn router() -> Router {
    let mut router = Router::new();
    for tool in engine_api::native_mcp_tool_descriptors() {
        let name: &'static str = tool.name;
        let handler =
            move |method: Method,
                  auth_user: Option<Extension<AuthUser>>,
                  RawQuery(query): RawQuery,
                  body: Bytes| call_operation(name, method, auth_user, query, body);
        let mut method_router = axum::routing::post(handler);
        if is_read_only(name) {
            method_router = method_router.get(handler);
        }
        if let Some(limit) = body_limit(name) {
            method_router = method_router.layer(DefaultBodyLimit::max(limit));
        }
        router = router.route(&format!("/engine/{name}"), method_router);
    }
    router
}

/// The operations as OpenAPI `paths`, for merging into `/engine/openapi.json`.
///
/// Generated from the same schemas `tools/list` publishes, so the document
/// cannot describe an argument the operation does not take.
pub fn openapi_paths() -> Map<String, Value> {
    let mut paths = Map::new();
    for tool in engine_api::native_mcp_tool_descriptors() {
        let mut item = Map::new();
        let summary = tool
            .description
            .split(". ")
            .next()
            .unwrap_or(tool.description);
        let responses = json!({
            "200": {
                "description": "The operation's result",
                "content": { "application/json": { "schema": { "type": "object" } } }
            },
            "400": { "description": "The arguments were missing or malformed" },
            "403": { "description": "The caller may not do this" },
            "404": { "description": "What the arguments name does not exist" },
            "409": { "description": "What is stored conflicts: it already exists, or it has changed since it was read" }
        });
        item.insert(
            "post".into(),
            json!({
                "operationId": tool.name,
                "tags": ["Engine"],
                "summary": summary,
                "description": tool.description,
                "requestBody": {
                    "required": false,
                    "content": { "application/json": { "schema": tool.input_schema.clone() } }
                },
                "responses": responses,
            }),
        );
        if is_read_only(tool.name) {
            let required: Vec<&str> = tool
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let parameters: Vec<Value> = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|props| {
                    props
                        .iter()
                        .map(|(key, schema)| {
                            json!({
                                "name": key,
                                "in": "query",
                                "required": required.contains(&key.as_str()),
                                "description": schema.get("description").cloned().unwrap_or(Value::Null),
                                "schema": schema,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            item.insert(
                "get".into(),
                json!({
                    "operationId": format!("{}_get", tool.name),
                    "tags": ["Engine"],
                    "summary": summary,
                    "parameters": parameters,
                    "responses": responses,
                }),
            );
        }
        paths.insert(format!("/engine/{}", tool.name), Value::Object(item));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_says_what_kind_it_is() {
        use engine_api::{Refusal, refuse};
        for (kind, status) in [
            (Refusal::BadRequest, StatusCode::BAD_REQUEST),
            (Refusal::Forbidden, StatusCode::FORBIDDEN),
            (Refusal::NotFound, StatusCode::NOT_FOUND),
            (Refusal::Conflict, StatusCode::CONFLICT),
            (Refusal::Failed, StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            // The text is a decoy: the kind decides, not what the message says.
            let result = refuse(kind, "not found, access denied, already exists");
            assert_eq!(status_of(&result, "not found"), status);
        }
    }

    #[test]
    fn a_result_with_no_kind_is_read_from_its_text() {
        let bare = |message: &str| json!({ "error": message });
        assert_eq!(
            status_of(
                &bare("Failed to deploy: Access denied"),
                "Failed to deploy: Access denied"
            ),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(&bare("Script not found: x"), "Script not found: x"),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status_of(&bare("File already exists: a"), "File already exists: a"),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(&bare("Failed to list: db down"), "Failed to list: db down"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// A `status` that is not an error status is not trusted to be one.
    #[test]
    fn a_status_that_is_not_a_refusal_is_ignored() {
        let result = json!({ "error": "x is required", "status": 200 });
        assert_eq!(status_of(&result, "x is required"), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_query_is_read_against_the_schema() {
        let schema = json!({
            "properties": {
                "limit": { "type": "integer" },
                "caseInsensitive": { "type": "boolean" },
                "uri": { "type": "string" }
            }
        });
        let args = args_from_query("limit=5&caseInsensitive=false&uri=a%2Fb&extra=1", &schema);
        assert_eq!(
            args,
            json!({ "limit": 5, "caseInsensitive": false, "uri": "a/b", "extra": "1" })
        );
    }

    #[test]
    fn every_read_only_name_is_an_operation() {
        let names: Vec<_> = engine_api::native_mcp_tool_descriptors()
            .into_iter()
            .map(|t| t.name)
            .collect();
        for name in READ_ONLY {
            assert!(names.contains(name), "{name} is not an operation");
        }
    }
}
