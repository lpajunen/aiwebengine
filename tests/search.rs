//! `GET /engine/search` and the `search_files` MCP tool.
//!
//! The counterpart of the `grep` on a single-file read, for the caller that
//! does not yet know which file to name. What these tests are mostly about is
//! that it reads the *modules*: a solution's code is mostly its assets, and a
//! search that read only root sources answered "which file mentions this" with
//! the smaller half of the tree.

mod common;

use common::{setup_env, test_mutex};

use aiwebengine::auth::AuthUser;
use aiwebengine::engine_api::{SearchQuery, execute_native_mcp_tool, search_route};
use aiwebengine::repository;
use aiwebengine::security::{Capability, UserContext};
use axum::Extension;
use axum::extract::Query;
use serde_json::{Value, json};

fn deploy(script_uri: &str, content: &str) {
    repository::upsert_script(script_uri, content).expect("script should be stored");
    for existing in repository::fetch_assets(script_uri).keys() {
        repository::delete_asset(script_uri, existing);
    }
}

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

fn admin_extension() -> Option<Extension<AuthUser>> {
    Some(Extension(AuthUser::new(
        "searcher".to_string(),
        "test".to_string(),
        "session".to_string(),
        /* is_admin */ true,
        /* is_editor */ true,
        None,
        None,
    )))
}

async fn search(raw: &str) -> (axum::http::StatusCode, Value) {
    let response = search_route(
        admin_extension(),
        Query(serde_urlencoded::from_str::<SearchQuery>(raw).expect("query should parse")),
    )
    .await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    (
        status,
        serde_json::from_slice(&bytes).expect("body should be JSON"),
    )
}

/// Where a file appears in the results, as `(uri, asset)`.
fn hits(body: &Value) -> Vec<(String, Option<String>)> {
    body["results"]
        .as_array()
        .expect("results should be a list")
        .iter()
        .map(|result| {
            (
                result["uri"].as_str().unwrap_or_default().to_string(),
                result["asset"].as_str().map(str::to_string),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_search_reads_the_modules_as_well_as_the_root() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://search/modules";
    deploy(
        uri,
        "import \"./search_modules/util.ts\";\nfunction init() {}\n",
    );
    store_asset(
        uri,
        "search_modules/util.ts",
        "export function movePlayerSearch(id) { return id; }\n",
    );
    store_asset(uri, "search_modules/other.ts", "export const n = 1;\n");

    let (status, body) = search(&format!("query=movePlayerSearch&script={}", uri)).await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(
        hits(&body),
        vec![(uri.to_string(), Some("search_modules/util.ts".to_string()))],
        "the match is in a module, which is where a solution's code is: {}",
        body
    );
    assert_eq!(body["results"][0]["matchCount"], json!(1), "{}", body);
    assert_eq!(
        body["results"][0]["matches"][0]["line"],
        json!(1),
        "{}",
        body
    );

    // Scoped to root sources, the same search finds nothing — which is what
    // this answered before it could read assets.
    let (status, body) = search(&format!(
        "query=movePlayerSearch&script={}&scope=scripts",
        uri
    ))
    .await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["filesMatched"], json!(0), "{}", body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_root_source_and_its_module_are_separate_hits() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://search/both";
    deploy(uri, "// searchBothMarker in the root\nfunction init() {}\n");
    store_asset(
        uri,
        "search_both/util.ts",
        "// searchBothMarker in a module\n",
    );

    let (status, body) = search(&format!("query=searchBothMarker&script={}", uri)).await;
    assert_eq!(status, 200, "{}", body);

    let found = hits(&body);
    assert!(found.contains(&(uri.to_string(), None)), "{:?}", found);
    assert!(
        found.contains(&(uri.to_string(), Some("search_both/util.ts".to_string()))),
        "{:?}",
        found
    );
    assert_eq!(body["filesMatched"], json!(2), "{}", body);

    // Assets only, for the caller who knows the root is not where to look.
    let (_, body) = search(&format!(
        "query=searchBothMarker&script={}&scope=assets",
        uri
    ))
    .await;
    assert_eq!(
        hits(&body),
        vec![(uri.to_string(), Some("search_both/util.ts".to_string()))],
        "{}",
        body
    );
}

/// A search across a tree that happens to hold a PNG is a reasonable thing to
/// ask for; failing it because of the PNG would not be.
#[tokio::test(flavor = "multi_thread")]
async fn a_binary_asset_is_skipped_rather_than_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://search/binary";
    deploy(uri, "function init() {}");
    store_asset(uri, "search_binary/logo.png", vec![0x89, 0x50, 0xff, 0xfe]);
    store_asset(
        uri,
        "search_binary/util.ts",
        "export const searchBinaryMarker = 1;\n",
    );

    let (status, body) = search(&format!("query=searchBinaryMarker&script={}", uri)).await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(
        hits(&body),
        vec![(uri.to_string(), Some("search_binary/util.ts".to_string()))],
        "{}",
        body
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_pattern_is_refused_rather_than_matching_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let (status, body) = search("query=%5Bunclosed").await;
    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("pattern"),
        "{}",
        body["error"]
    );

    let (status, body) = search("query=a&scope=everything").await;
    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"].as_str().unwrap_or_default().contains("scope"),
        "{}",
        body["error"]
    );
}

/// Reading a script's assets through `/engine/*` is a permission of its own,
/// and a search is the sharper version of reading them: it reads every file of
/// every script at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_search_reads_only_what_the_caller_may_read() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://search/authz";
    deploy(uri, "function init() {}");
    store_asset(
        uri,
        "search_authz/secrets.ts",
        "export const API_KEY = \"sk-live-searchAuthzMarker\";\n",
    );

    let anonymous = UserContext::anonymous();
    let body = aiwebengine::engine_api::search_files_authorized(
        &anonymous,
        "searchAuthzMarker",
        &aiwebengine::engine_api::SearchOptions::default(),
    )
    .expect("a search with no results is not an error");
    assert_eq!(body["filesMatched"], json!(0), "{}", body);

    // A signed-in caller who does not own the script is closed out too: they
    // hold ReadAssets so the sandbox can serve their requests, which is why
    // the capability alone cannot open the management surface.
    let authenticated = UserContext::authenticated("someone".to_string());
    let body = aiwebengine::engine_api::search_files_authorized(
        &authenticated,
        "searchAuthzMarker",
        &aiwebengine::engine_api::SearchOptions::default(),
    )
    .expect("a search with no results is not an error");
    assert_eq!(body["filesMatched"], json!(0), "{}", body);
    assert!(
        authenticated.has_capability(&Capability::ReadAssets),
        "the sandbox still needs this capability for public requests"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_tool_searches_the_same_way() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://search/mcp";
    deploy(uri, "function init() {}");
    store_asset(
        uri,
        "search_mcp/util.ts",
        "export const searchMcpMarker = 1;\n",
    );

    let result = execute_native_mcp_tool(
        "search_files",
        &json!({ "query": "SEARCHMCPMARKER", "script": uri }),
        &UserContext::admin("searcher".to_string()),
    )
    .expect("search_files should dispatch");

    // Case-insensitive by default, as it has always been.
    assert_eq!(result["filesMatched"], json!(1), "{}", result);
    assert_eq!(
        result["results"][0]["asset"],
        json!("search_mcp/util.ts"),
        "{}",
        result
    );

    let result = execute_native_mcp_tool(
        "search_files",
        &json!({ "query": "SEARCHMCPMARKER", "script": uri, "caseInsensitive": false }),
        &UserContext::admin("searcher".to_string()),
    )
    .expect("search_files should dispatch");
    assert_eq!(result["filesMatched"], json!(0), "{}", result);
}
