//! `POST /engine/edit_script` and the `edit_file` MCP tool.
//!
//! A script's modules could already be changed a few lines at a time; its root
//! source — the file that registers every route the modules serve — could only
//! be replaced whole. These tests cover the half of that gap that is not
//! shared with `assets_patch.rs`: that a root-source patch is authorized as a
//! script write rather than as an asset write, that it cannot leave a script
//! with no content at all, and that the digest a caller needs to aim one is
//! something a read already hands back.

mod common;

use common::{setup_env, test_mutex};

use aiwebengine::auth::AuthUser;
use aiwebengine::engine_api::{
    ScriptParams, StringEdit, edit_script_route, execute_native_mcp_tool, patch_script_authorized,
    read_script_route,
};
use aiwebengine::repository;
use aiwebengine::security::{Capability, UserContext};
use axum::Extension;
use axum::extract::Query;
use axum::response::Response;
use serde_json::{Value, json};
use std::collections::HashSet;

fn deploy(script_uri: &str, content: &str) {
    repository::upsert_script(script_uri, content).expect("script should be stored");
}

fn sha256_hex(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// An admin caller, so the tests exercise the handler rather than its guard.
fn admin_extension() -> Option<Extension<AuthUser>> {
    Some(Extension(AuthUser::new(
        "editor".to_string(),
        "test".to_string(),
        "session".to_string(),
        /* is_admin */ true,
        /* is_editor */ true,
        None,
        None,
    )))
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    serde_json::from_slice(&bytes).expect("body should be JSON")
}

fn query(raw: &str) -> Query<ScriptParams> {
    Query(serde_urlencoded::from_str::<ScriptParams>(raw).expect("query should parse"))
}

async fn edit(body: Value) -> (axum::http::StatusCode, Value) {
    let response = edit_script_route(
        admin_extension(),
        query(""),
        axum::body::Bytes::from(body.to_string()),
    )
    .await;

    let status = response.status();
    (status, body_json(response).await)
}

fn stored(script_uri: &str) -> String {
    repository::fetch_script(script_uri).expect("script should be stored")
}

fn registered_paths(script_uri: &str) -> HashSet<String> {
    repository::get_script_metadata(script_uri)
        .expect("script metadata should load")
        .registrations
        .keys()
        .map(|(path, _method)| path.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edit_rewrites_the_root_source_and_runs_init_once() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/edit";
    let source = "function handler(context) { return ResponseBuilder.json({}); }\n\
                  globalThis.handler = handler;\n\
                  function init() { routeRegistry.registerRoute(\"/script-edit/before\", \"handler\", \"GET\"); }\n";
    deploy(uri, source);

    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [{ "old_string": "/script-edit/before", "new_string": "/script-edit/after" }]
    }))
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["status"], json!("updated"), "{}", body);
    assert_eq!(body["replacements"], json!(1), "{}", body);

    let expected = source.replace("/script-edit/before", "/script-edit/after");
    assert_eq!(stored(uri), expected);
    // The digest echoed back is of the content that now stands, so the next
    // patch can send it as base_sha256 without reading the file again.
    assert_eq!(body["sha256"], json!(sha256_hex(expected.as_bytes())));
    assert_eq!(body["bytes"], json!(expected.len()));

    assert_eq!(body["init"]["ran"], json!(true), "{}", body["init"]);
    assert!(
        registered_paths(uri).contains("/script-edit/after"),
        "init() should have re-registered from the edited source, got {:?}",
        registered_paths(uri)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_string_that_is_not_there_changes_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/missing";
    let source = "function init() { /* nothing */ }\n";
    deploy(uri, source);

    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [{ "old_string": "function start", "new_string": "function begin" }]
    }))
    .await;

    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not found"),
        "the refusal should say the text was not there, got {}",
        body["error"]
    );
    assert_eq!(stored(uri), source);
}

/// The property that makes a content-addressed edit safe: text that appears
/// twice cannot be aimed at, so the engine refuses rather than picking one.
#[tokio::test(flavor = "multi_thread")]
async fn an_ambiguous_edit_is_refused_unless_it_says_replace_all() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/ambiguous";
    let source = "const a = limit;\nconst b = limit;\nfunction init() { const c = limit; }\n";
    deploy(uri, source);

    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [{ "old_string": "limit", "new_string": "cap" }],
        "reinit": "never"
    }))
    .await;

    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("3 times"),
        "the refusal should count the matches, got {}",
        body["error"]
    );
    assert_eq!(stored(uri), source);

    // The same edit, with the caller saying it means all of them.
    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [{ "old_string": "limit", "new_string": "cap", "replace_all": true }],
        "reinit": "never"
    }))
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["replacements"], json!(3), "{}", body);
    assert_eq!(stored(uri), source.replace("limit", "cap"));
}

/// Several edits are one change: if the last one does not apply, none of them
/// did.
#[tokio::test(flavor = "multi_thread")]
async fn edits_apply_in_order_and_all_or_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/sequence";
    let source = "const first = 1;\nconst second = 2;\nfunction init() {}\n";
    deploy(uri, source);

    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [
            { "old_string": "const first = 1;", "new_string": "const first = 10;" },
            { "old_string": "const third = 3;", "new_string": "const third = 30;" },
        ],
        "reinit": "never"
    }))
    .await;

    assert_eq!(status, 400, "{}", body);
    assert_eq!(
        stored(uri),
        source,
        "a patch whose second edit fails must leave the script as it was"
    );

    // Applied in order against what the previous edit left behind.
    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [
            { "old_string": "const first = 1;", "new_string": "const first = 10;" },
            { "old_string": "const first = 10;", "new_string": "const first = 100;" },
        ],
        "reinit": "never"
    }))
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["replacements"], json!(2), "{}", body);
    assert_eq!(
        stored(uri),
        "const first = 100;\nconst second = 2;\nfunction init() {}\n"
    );
}

/// The guard that makes editing-without-sending safe: the caller says which
/// version it edited, and a patch against a version that has been replaced is
/// refused with the digest it would need to rebase on.
#[tokio::test(flavor = "multi_thread")]
async fn a_patch_against_a_version_that_moved_on_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/conflict";
    let read_earlier = "const n = 1;\nfunction init() {}\n";
    let stale_digest = sha256_hex(read_earlier.as_bytes());

    // Someone else wrote the script between the read and the patch.
    let current = "const n = 2;\nfunction init() {}\n";
    deploy(uri, current);

    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [{ "old_string": "const n", "new_string": "const total" }],
        "base_sha256": stale_digest,
    }))
    .await;

    assert_eq!(status, 409, "{}", body);
    assert_eq!(body["expected_sha256"], json!(stale_digest), "{}", body);
    assert_eq!(
        body["sha256"],
        json!(sha256_hex(current.as_bytes())),
        "the refusal should carry the digest to rebase on, got {}",
        body
    );
    assert_eq!(stored(uri), current);

    // The same patch, aimed at the version that is actually stored.
    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [{ "old_string": "const n", "new_string": "const total" }],
        "base_sha256": sha256_hex(current.as_bytes()),
        "reinit": "never"
    }))
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(stored(uri), "const total = 2;\nfunction init() {}\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_that_cancel_out_write_nothing_and_leave_init_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/unchanged";
    let source = "const n = 1;\nfunction init() {}\n";
    deploy(uri, source);

    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [
            { "old_string": "= 1;", "new_string": "= 2;" },
            { "old_string": "= 2;", "new_string": "= 1;" },
        ]
    }))
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["status"], json!("unchanged"), "{}", body);
    assert_eq!(body["revision"], json!(null), "{}", body);
    assert_eq!(body["init"]["ran"], json!(false), "{}", body["init"]);
    assert_eq!(stored(uri), source);
}

/// A script with no content is one the write path refuses, so a patch that
/// would produce one is refused too rather than storing what cannot be stored.
#[tokio::test(flavor = "multi_thread")]
async fn the_edits_cannot_leave_a_script_empty() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/empty";
    let source = "function init() {}\n";
    deploy(uri, source);

    let (status, body) = edit(json!({
        "uri": uri,
        "edits": [{ "old_string": source, "new_string": "" }]
    }))
    .await;

    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"].as_str().unwrap_or_default().contains("empty"),
        "the refusal should say what the edits would have left, got {}",
        body["error"]
    );
    assert_eq!(stored(uri), source);
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_a_script_that_is_not_there_is_not_a_way_to_create_one() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let (status, body) = edit(json!({
        "uri": "test://script-edit/absent",
        "edits": [{ "old_string": "a", "new_string": "b" }]
    }))
    .await;

    assert_eq!(status, 404, "{}", body);
    assert!(
        repository::fetch_script("test://script-edit/absent").is_none(),
        "a patch of nothing must not have written anything"
    );
}

/// The one place a root-source patch differs from an asset patch: what it
/// takes to be allowed. Editing a script is writing a script, so `WriteAssets`
/// is not the question and reading it is not enough.
#[tokio::test(flavor = "multi_thread")]
async fn editing_a_script_takes_what_writing_one_takes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/authz";
    let source = "const n = 1;\nfunction init() {}\n";
    deploy(uri, source);

    let edits = [StringEdit {
        old_string: "const n".to_string(),
        new_string: "const hacked".to_string(),
        replace_all: false,
    }];

    let reader = UserContext {
        user_id: Some("reader".to_string()),
        is_authenticated: true,
        capabilities: [
            Capability::ReadScripts,
            Capability::ReadAssets,
            Capability::WriteAssets,
        ]
        .into_iter()
        .collect(),
    };
    assert!(
        patch_script_authorized(&reader, uri, &edits, None, None).is_err(),
        "being allowed to write a script's assets is not being allowed to write the script"
    );

    // An editor holds `WriteScripts`, which is the capability — but this
    // script is not theirs, and a patch is a write like any other.
    let stranger = UserContext::editor("stranger".to_string());
    assert!(
        patch_script_authorized(&stranger, uri, &edits, None, None).is_err(),
        "an editor who does not own the script must not edit its source"
    );

    assert_eq!(stored(uri), source);

    let admin = UserContext::admin("root".to_string());
    assert!(
        patch_script_authorized(&admin, uri, &edits, None, None).is_ok(),
        "an administrator acts on what they do not own"
    );
    assert_eq!(stored(uri), "const hacked = 1;\nfunction init() {}\n");
}

/// The digest a patch is aimed with has to be obtainable, or every edit is a
/// blind one.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_reports_the_digest_an_edit_takes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/digest";
    let source = "const n = 1;\nfunction init() {}\n";
    deploy(uri, source);
    let digest = sha256_hex(source.as_bytes());

    let response = read_script_route(admin_extension(), query(&format!("uri={}", uri))).await;

    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok()),
        Some(format!("\"{}\"", digest).as_str()),
        "the body is the script itself, so the digest travels as an ETag"
    );

    let read = execute_native_mcp_tool(
        "read_file",
        &json!({ "uri": uri }),
        &UserContext::admin("editor".to_string()),
    )
    .expect("read_file should dispatch");
    assert_eq!(read["sha256"], json!(digest), "{}", read);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_tool_edits_the_same_way() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://script-edit/mcp";
    let source = "function handler(context) { return ResponseBuilder.json({}); }\n\
                  globalThis.handler = handler;\n\
                  function init() { routeRegistry.registerRoute(\"/script-edit/mcp-before\", \"handler\", \"GET\"); }\n";
    deploy(uri, source);

    let read = execute_native_mcp_tool(
        "read_file",
        &json!({ "uri": uri }),
        &UserContext::admin("editor".to_string()),
    )
    .expect("read_file should dispatch");
    let digest = read["sha256"]
        .as_str()
        .expect("read should report a digest");

    let result = execute_native_mcp_tool(
        "edit_file",
        &json!({
            "uri": uri,
            "base_sha256": digest,
            "edits": [{
                "old_string": "/script-edit/mcp-before",
                "new_string": "/script-edit/mcp-after",
            }],
        }),
        &UserContext::admin("editor".to_string()),
    )
    .expect("edit_file should dispatch");

    assert_eq!(result["success"], json!(true), "{}", result);
    assert_eq!(result["replacements"], json!(1), "{}", result);
    assert_eq!(result["init"]["ran"], json!(true), "{}", result["init"]);
    assert!(
        registered_paths(uri).contains("/script-edit/mcp-after"),
        "the tool should leave the script initialized from what it edited, got {:?}",
        registered_paths(uri)
    );

    // The digest it reported back is the one the next edit is aimed with.
    let next = execute_native_mcp_tool(
        "edit_file",
        &json!({
            "uri": uri,
            "base_sha256": result["sha256"],
            "reinit": "never",
            "edits": [{
                "old_string": "/script-edit/mcp-after",
                "new_string": "/script-edit/mcp-last",
            }],
        }),
        &UserContext::admin("editor".to_string()),
    )
    .expect("edit_file should dispatch");

    assert_eq!(next["success"], json!(true), "{}", next);
    assert_eq!(
        next["init"],
        json!({ "ran": false, "reason": "reinit=never" }),
        "{}",
        next
    );
    assert!(stored(uri).contains("/script-edit/mcp-last"));
}
