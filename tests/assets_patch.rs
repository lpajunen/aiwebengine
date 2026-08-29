//! `PATCH /engine/assets` and the scoped reads that feed it.
//!
//! The endpoint's reason to exist is what its requests do *not* carry. A
//! caller changing three lines of a module sends three lines, and a caller
//! looking for those lines fetches a range or a set of matches rather than the
//! module. These tests are mostly about what that costs in safety: an edit
//! aimed by content alone has to be unaimed if the content is ambiguous, and a
//! patch computed against a version someone else has replaced has to be
//! refused rather than merged blind.

mod common;

use common::{setup_env, test_mutex};

use aiwebengine::auth::AuthUser;
use aiwebengine::engine_api::{
    AssetQuery, assets_get_route, assets_patch_route, execute_native_mcp_tool,
};
use aiwebengine::repository;
use aiwebengine::security::{Capability, UserContext};
use axum::Extension;
use axum::extract::Query;
use axum::response::Response;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Store a script and clear whatever assets a previous run left under it.
///
/// Asset paths key on the path alone across the whole table, so each test uses
/// paths of its own.
fn deploy(script_uri: &str, content: &str) {
    repository::upsert_script(script_uri, content).expect("script should be stored");
    for existing in repository::fetch_assets(script_uri).keys() {
        repository::delete_asset(script_uri, existing);
    }
}

/// Store one asset directly, as the state a patch starts from.
fn store_asset(script_uri: &str, asset_uri: &str, content: impl Into<Vec<u8>>) {
    let now = std::time::SystemTime::now();
    repository::upsert_asset(repository::Asset {
        uri: asset_uri.to_string(),
        name: Some(asset_uri.to_string()),
        mimetype: "text/typescript".to_string(),
        content: content.into(),
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    })
    .expect("asset should be stored");
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
        "patcher".to_string(),
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

async fn patch(query: &str, body: Value) -> (axum::http::StatusCode, Value) {
    let response = assets_patch_route(
        admin_extension(),
        Query(serde_urlencoded::from_str::<AssetQuery>(query).expect("query should parse")),
        axum::body::Bytes::from(body.to_string()),
    )
    .await;

    let status = response.status();
    (status, body_json(response).await)
}

async fn get(query: &str) -> (axum::http::StatusCode, Value) {
    let response = assets_get_route(
        admin_extension(),
        Query(serde_urlencoded::from_str::<AssetQuery>(query).expect("query should parse")),
    )
    .await;

    let status = response.status();
    (status, body_json(response).await)
}

fn stored_text(script_uri: &str, asset_uri: &str) -> String {
    let asset = repository::fetch_asset(script_uri, asset_uri).expect("asset should be stored");
    String::from_utf8(asset.content).expect("asset should be UTF-8")
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
async fn an_edit_rewrites_the_file_and_runs_init_once() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/edit";
    deploy(
        uri,
        r#"
        import { PATH, handler } from "./assets_patch_edit/routes.ts";
        globalThis.handler = handler;
        function init() { routeRegistry.registerRoute(PATH, "handler", "GET"); }
        "#,
    );
    let source = "export const PATH = \"/assets-patch/before\";\n\
                  export function handler(context) { return ResponseBuilder.json({}); }\n";
    store_asset(uri, "assets_patch_edit/routes.ts", source);

    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_edit/routes.ts", uri),
        json!({
            "edits": [{ "old_string": "/assets-patch/before", "new_string": "/assets-patch/after" }]
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["status"], json!("updated"), "{}", body);
    assert_eq!(body["replacements"], json!(1), "{}", body);

    let expected = source.replace("/assets-patch/before", "/assets-patch/after");
    assert_eq!(stored_text(uri, "assets_patch_edit/routes.ts"), expected);
    // The digest echoed back is of the content that now stands, so the next
    // patch can send it as base_sha256 without reading the file again.
    assert_eq!(body["sha256"], json!(sha256_hex(expected.as_bytes())));
    assert_eq!(body["bytes"], json!(expected.len()));

    assert_eq!(body["init"]["ran"], json!(true), "{}", body["init"]);
    assert!(
        registered_paths(uri).contains("/assets-patch/after"),
        "init() should have re-registered from the edited module, got {:?}",
        registered_paths(uri)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_string_that_is_not_there_changes_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/missing";
    deploy(uri, "function init() {}");
    let source = "export const n = 1;\n";
    store_asset(uri, "assets_patch_missing/util.ts", source);

    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_missing/util.ts", uri),
        json!({ "edits": [{ "old_string": "export const m", "new_string": "export const k" }] }),
    )
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
    assert_eq!(stored_text(uri, "assets_patch_missing/util.ts"), source);
}

/// The property that makes a content-addressed edit safe: text that appears
/// twice cannot be aimed at, so the engine refuses rather than picking one.
#[tokio::test(flavor = "multi_thread")]
async fn an_ambiguous_edit_is_refused_unless_it_says_replace_all() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/ambiguous";
    deploy(uri, "function init() {}");
    let source = "const a = limit;\nconst b = limit;\nconst c = limit;\n";
    store_asset(uri, "assets_patch_ambiguous/util.ts", source);

    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_ambiguous/util.ts", uri),
        json!({ "edits": [{ "old_string": "limit", "new_string": "cap" }] }),
    )
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
    assert_eq!(stored_text(uri, "assets_patch_ambiguous/util.ts"), source);

    // The same edit, with the caller saying it means all of them.
    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_ambiguous/util.ts", uri),
        json!({
            "edits": [{ "old_string": "limit", "new_string": "cap", "replace_all": true }],
            "reinit": "never"
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["replacements"], json!(3), "{}", body);
    assert_eq!(
        stored_text(uri, "assets_patch_ambiguous/util.ts"),
        "const a = cap;\nconst b = cap;\nconst c = cap;\n"
    );
}

/// Several edits are one change: if the last one does not apply, none of them
/// did.
#[tokio::test(flavor = "multi_thread")]
async fn edits_apply_in_order_and_all_or_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/sequence";
    deploy(uri, "function init() {}");
    let source = "const first = 1;\nconst second = 2;\n";
    store_asset(uri, "assets_patch_sequence/util.ts", source);

    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_sequence/util.ts", uri),
        json!({
            "edits": [
                { "old_string": "const first = 1;", "new_string": "const first = 10;" },
                { "old_string": "const third = 3;", "new_string": "const third = 30;" },
            ],
            "reinit": "never"
        }),
    )
    .await;

    assert_eq!(status, 400, "{}", body);
    assert_eq!(
        stored_text(uri, "assets_patch_sequence/util.ts"),
        source,
        "a patch whose second edit fails must leave the file as it was"
    );

    // Applied in order against what the previous edit left behind.
    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_sequence/util.ts", uri),
        json!({
            "edits": [
                { "old_string": "const first = 1;", "new_string": "const first = 10;" },
                { "old_string": "const first = 10;", "new_string": "const first = 100;" },
            ],
            "reinit": "never"
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["replacements"], json!(2), "{}", body);
    assert_eq!(
        stored_text(uri, "assets_patch_sequence/util.ts"),
        "const first = 100;\nconst second = 2;\n"
    );
}

/// The guard that makes editing-without-sending safe: the caller says which
/// version it edited, and a patch against a version that has been replaced is
/// refused with the digest it would need to rebase on.
#[tokio::test(flavor = "multi_thread")]
async fn a_patch_against_a_version_that_moved_on_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/conflict";
    deploy(uri, "function init() {}");
    let read_earlier = "export const n = 1;\n";
    let stale_digest = sha256_hex(read_earlier.as_bytes());

    // Someone else wrote the file between the read and the patch.
    let current = "export const n = 2;\n";
    store_asset(uri, "assets_patch_conflict/util.ts", current);

    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_conflict/util.ts", uri),
        json!({
            "edits": [{ "old_string": "export const n", "new_string": "export const total" }],
            "base_sha256": stale_digest,
        }),
    )
    .await;

    assert_eq!(status, 409, "{}", body);
    assert_eq!(body["expected_sha256"], json!(stale_digest), "{}", body);
    assert_eq!(
        body["sha256"],
        json!(sha256_hex(current.as_bytes())),
        "the refusal should carry the digest to rebase on, got {}",
        body
    );
    assert_eq!(stored_text(uri, "assets_patch_conflict/util.ts"), current);

    // The same patch, aimed at the version that is actually stored.
    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_conflict/util.ts", uri),
        json!({
            "edits": [{ "old_string": "export const n", "new_string": "export const total" }],
            "base_sha256": sha256_hex(current.as_bytes()),
            "reinit": "never"
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(
        stored_text(uri, "assets_patch_conflict/util.ts"),
        "export const total = 2;\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_that_cancel_out_write_nothing_and_leave_init_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/unchanged";
    deploy(uri, "function init() {}");
    let source = "export const n = 1;\n";
    store_asset(uri, "assets_patch_unchanged/util.ts", source);
    let updated_before = repository::fetch_asset(uri, "assets_patch_unchanged/util.ts")
        .expect("asset should be stored")
        .updated_at;

    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_unchanged/util.ts", uri),
        json!({
            "edits": [
                { "old_string": "= 1;", "new_string": "= 2;" },
                { "old_string": "= 2;", "new_string": "= 1;" },
            ]
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["status"], json!("unchanged"), "{}", body);
    assert_eq!(body["init"]["ran"], json!(false), "{}", body["init"]);
    assert_eq!(stored_text(uri, "assets_patch_unchanged/util.ts"), source);
    assert_eq!(
        repository::fetch_asset(uri, "assets_patch_unchanged/util.ts")
            .expect("asset should be stored")
            .updated_at,
        updated_before,
        "content that did not change should not have been rewritten"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_binary_asset_cannot_be_edited_as_strings() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/binary";
    deploy(uri, "function init() {}");
    store_asset(
        uri,
        "assets_patch_binary/logo.png",
        vec![0x89, 0x50, 0xff, 0xfe],
    );

    let (status, body) = patch(
        &format!("script={}&asset=assets_patch_binary/logo.png", uri),
        json!({ "edits": [{ "old_string": "PNG", "new_string": "GIF" }] }),
    )
    .await;

    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"].as_str().unwrap_or_default().contains("UTF-8"),
        "the refusal should say why, got {}",
        body["error"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_alone_cannot_patch_an_asset() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/authz";
    deploy(uri, "function init() {}");
    let source = "export const n = 1;\n";
    store_asset(uri, "assets_patch_authz/util.ts", source);

    let reader = UserContext {
        user_id: Some("reader".to_string()),
        is_authenticated: true,
        capabilities: [Capability::ReadScripts, Capability::ReadAssets]
            .into_iter()
            .collect(),
    };

    let result = aiwebengine::engine_api::patch_asset_authorized(
        &reader,
        uri,
        "assets_patch_authz/util.ts",
        &[aiwebengine::engine_api::AssetEdit {
            old_string: "export const n".to_string(),
            new_string: "export const hacked".to_string(),
            replace_all: false,
        }],
        None,
    );

    assert!(
        result.is_err(),
        "a reader who does not own the script must not edit its assets"
    );
    assert_eq!(stored_text(uri, "assets_patch_authz/util.ts"), source);
}

/// Reported from testing: `GET /engine/assets` answered `200` with no token at
/// all. An anonymous caller holds `ReadAssets` so that a script serving a
/// public request can read its own files through the sandbox — but that is not
/// the same permission as reading the tree through `/engine/*`, and `grep=`
/// sharpens the difference from "download the file" to "search the tree".
#[tokio::test(flavor = "multi_thread")]
async fn engine_asset_reads_are_closed_to_anonymous_callers() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The production rule. Development mode grants anonymous callers elevated
    // capabilities by design, and is checked separately below.
    unsafe {
        std::env::set_var("AIWEBENGINE_MODE", "production");
    }

    let uri = "test://assets-patch/anonymous";
    deploy(uri, "function init() {}");
    store_asset(
        uri,
        "assets_patch_anonymous/secrets.ts",
        "export const API_KEY = \"sk-live-do-not-leak\";\n",
    );

    let anonymous = UserContext::anonymous();
    assert!(
        anonymous.has_capability(&Capability::ReadAssets),
        "the sandbox still needs this capability for public requests"
    );

    let read = aiwebengine::engine_api::read_asset_authorized(
        &anonymous,
        uri,
        "assets_patch_anonymous/secrets.ts",
        &aiwebengine::engine_api::AssetReadOptions::default(),
    );
    assert!(
        read.is_err(),
        "an unauthenticated caller must not download a script's assets"
    );

    let search = aiwebengine::engine_api::read_asset_authorized(
        &anonymous,
        uri,
        "assets_patch_anonymous/secrets.ts",
        &aiwebengine::engine_api::AssetReadOptions {
            lines: None,
            grep: Some("API_KEY".to_string()),
        },
    );
    assert!(
        search.is_err(),
        "nor search them, which is the sharper version of the same access"
    );

    assert!(
        aiwebengine::engine_api::list_assets_authorized(&anonymous, uri).is_empty(),
        "nor enumerate them"
    );

    // The rest of the read surface the same capabilities opened.
    assert!(
        aiwebengine::engine_api::get_script_authorized(&anonymous, uri).is_none(),
        "nor read a script's source, which is where a credential is as likely to be"
    );
    assert!(
        aiwebengine::engine_api::list_scripts_authorized(&anonymous).is_empty(),
        "nor list what is deployed"
    );

    // An authenticated caller is unaffected.
    let authenticated = UserContext::authenticated("someone".to_string());
    assert!(
        aiwebengine::engine_api::read_asset_authorized(
            &authenticated,
            uri,
            "assets_patch_anonymous/secrets.ts",
            &aiwebengine::engine_api::AssetReadOptions::default(),
        )
        .is_ok(),
        "a caller with ReadAssets should still read"
    );

    // Development mode is the documented escape hatch, and stays open.
    unsafe {
        std::env::set_var("AIWEBENGINE_MODE", "development");
    }
    let dev_anonymous = UserContext::anonymous();
    assert!(
        aiwebengine::engine_api::read_asset_authorized(
            &dev_anonymous,
            uri,
            "assets_patch_anonymous/secrets.ts",
            &aiwebengine::engine_api::AssetReadOptions::default(),
        )
        .is_ok(),
        "development mode drives a local instance without a login"
    );
    unsafe {
        std::env::remove_var("AIWEBENGINE_MODE");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_line_range_reads_part_of_a_file_and_reports_the_whole_of_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/lines";
    deploy(uri, "function init() {}");
    let source = (1..=10)
        .map(|n| format!("const line{} = {};", n, n))
        .collect::<Vec<_>>()
        .join("\n");
    store_asset(uri, "assets_patch_lines/util.ts", source.clone());

    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_lines/util.ts&lines=3-5",
        uri
    ))
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(
        body["content"],
        json!("const line3 = 3;\nconst line4 = 4;\nconst line5 = 5;"),
        "{}",
        body
    );
    assert_eq!(body["start_line"], json!(3), "{}", body);
    assert_eq!(body["end_line"], json!(5), "{}", body);
    assert_eq!(body["total_lines"], json!(10), "{}", body);
    // The digest is of the whole asset, not of the range, so a ranged read
    // feeds straight into a patch.
    assert_eq!(body["sha256"], json!(sha256_hex(source.as_bytes())));

    // An open-ended range runs to the end of the file.
    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_lines/util.ts&lines=9-",
        uri
    ))
    .await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["end_line"], json!(10), "{}", body);

    // An end past the end of the file is a request for the rest of it.
    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_lines/util.ts&lines=8-1000",
        uri
    ))
    .await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["end_line"], json!(10), "{}", body);

    // A start past the end is not: the caller is reading a range that no
    // longer exists, and an empty 200 would hide that.
    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_lines/util.ts&lines=99999-100000",
        uri
    ))
    .await;
    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("10 lines"),
        "the refusal should say how long the file actually is, got {}",
        body["error"]
    );

    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_lines/util.ts&lines=oops",
        uri
    ))
    .await;
    assert_eq!(status, 400, "{}", body);
}

#[tokio::test(flavor = "multi_thread")]
async fn grep_locates_lines_without_returning_the_file() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/grep";
    deploy(uri, "function init() {}");
    let source = "import { a } from \"./a.ts\";\n\
                  export function movePlayer(id) {\n\
                  \x20 return id;\n\
                  }\n\
                  export function movePiece(id) {\n\
                  \x20 return id;\n\
                  }\n";
    store_asset(uri, "assets_patch_grep/moves.ts", source);

    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_grep/moves.ts&grep=^export%20function%20move",
        uri
    ))
    .await;

    assert_eq!(status, 200, "{}", body);
    assert!(
        body.get("content").is_none(),
        "grep should not ship the file, got {}",
        body
    );
    assert_eq!(body["match_count"], json!(2), "{}", body);
    assert_eq!(body["matches"][0]["line"], json!(2), "{}", body);
    assert_eq!(body["matches"][1]["line"], json!(5), "{}", body);
    assert_eq!(body["total_lines"], json!(7), "{}", body);

    // Composed with a range, the search is confined to that range.
    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_grep/moves.ts&grep=^export%20function%20move&lines=4-7",
        uri
    ))
    .await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["match_count"], json!(1), "{}", body);
    assert_eq!(body["matches"][0]["line"], json!(5), "{}", body);

    let (status, body) = get(&format!(
        "script={}&asset=assets_patch_grep/moves.ts&grep=%5B",
        uri
    ))
    .await;
    assert_eq!(status, 400, "{}", body);
}

/// The unscoped read is the older contract, and callers already parse it.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_without_lines_or_grep_still_answers_with_base64() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/whole";
    deploy(uri, "function init() {}");
    let source = "export const n = 1;\n";
    store_asset(uri, "assets_patch_whole/util.ts", source);

    let (status, body) = get(&format!("script={}&asset=assets_patch_whole/util.ts", uri)).await;

    assert_eq!(status, 200, "{}", body);
    use base64::Engine as _;
    assert_eq!(
        body["content"],
        json!(base64::engine::general_purpose::STANDARD.encode(source)),
        "{}",
        body
    );
    assert_eq!(body["sha256"], json!(sha256_hex(source.as_bytes())));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_tool_edits_the_same_way() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-patch/mcp";
    deploy(
        uri,
        r#"
        import { PATH } from "./assets_patch_mcp/routes.ts";
        function handler(context) { return ResponseBuilder.json({}); }
        globalThis.handler = handler;
        function init() { routeRegistry.registerRoute(PATH, "handler", "GET"); }
        "#,
    );
    let source = "export const PATH = \"/assets-patch/mcp-before\";\n";
    store_asset(uri, "assets_patch_mcp/routes.ts", source);

    let read = execute_native_mcp_tool(
        "read_asset",
        &json!({ "script": uri, "asset": "assets_patch_mcp/routes.ts", "grep": "PATH" }),
        &UserContext::admin("patcher".to_string()),
    )
    .expect("read_asset should dispatch");

    assert_eq!(read["match_count"], json!(1), "{}", read);
    let digest = read["sha256"]
        .as_str()
        .expect("read should report a digest");
    assert_eq!(digest, sha256_hex(source.as_bytes()));

    let result = execute_native_mcp_tool(
        "edit_asset",
        &json!({
            "script": uri,
            "asset": "assets_patch_mcp/routes.ts",
            "base_sha256": digest,
            "edits": [{
                "old_string": "/assets-patch/mcp-before",
                "new_string": "/assets-patch/mcp-after",
            }],
        }),
        &UserContext::admin("patcher".to_string()),
    )
    .expect("edit_asset should dispatch");

    assert_eq!(result["success"], json!(true), "{}", result);
    assert_eq!(result["replacements"], json!(1), "{}", result);
    assert_eq!(result["init"]["ran"], json!(true), "{}", result["init"]);
    assert!(
        registered_paths(uri).contains("/assets-patch/mcp-after"),
        "the tool should leave the script initialized from what it edited, got {:?}",
        registered_paths(uri)
    );
}
