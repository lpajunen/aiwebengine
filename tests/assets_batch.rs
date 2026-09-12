//! `POST /engine/assets/batch`: a script's files written as one unit.
//!
//! What these tests are really about is the *unit*. A script's modules are one
//! change, and the engine acts on every write — invalidating the prepared
//! program, notifying the cluster, re-registering routes — so writing them one
//! at a time makes it act on states the author never meant to deploy.

mod common;

use aiwebengine::auth::AuthUser;
use aiwebengine::engine_api::{AssetQuery, assets_batch_route, execute_native_mcp_tool};
use aiwebengine::repository;
use aiwebengine::security::{Capability, UserContext};
use axum::Extension;
use axum::extract::Query;
use axum::response::Response;
use base64::Engine as _;
use common::{AdminServer, setup_env, test_mutex};
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

fn b64(content: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(content.as_bytes())
}

fn sha256_hex(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// An admin caller, so the tests exercise the handler rather than its guard.
fn admin_extension() -> Option<Extension<AuthUser>> {
    Some(Extension(AuthUser::new(
        "batcher".to_string(),
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

async fn post_batch(query: &str, body: Value) -> (axum::http::StatusCode, Value) {
    let response = assets_batch_route(
        admin_extension(),
        Query(serde_urlencoded::from_str::<AssetQuery>(query).expect("query should parse")),
        axum::body::Bytes::from(body.to_string()),
    )
    .await;

    let status = response.status();
    (status, body_json(response).await)
}

fn stored(script_uri: &str, asset_uri: &str) -> Option<repository::Asset> {
    repository::fetch_asset(script_uri, asset_uri)
}

fn stored_text(script_uri: &str, asset_uri: &str) -> String {
    let asset = stored(script_uri, asset_uri).expect("asset should be stored");
    String::from_utf8(asset.content).expect("asset should be UTF-8")
}

fn statuses(body: &Value) -> Vec<(String, String)> {
    body["results"]
        .as_array()
        .expect("results should be a list")
        .iter()
        .map(|result| {
            (
                result["name"].as_str().unwrap_or_default().to_string(),
                result["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
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
async fn a_batch_writes_every_file_and_runs_init_once() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/deploy";
    deploy(
        uri,
        r#"
        import { PATH, listItems } from "./assets_batch_deploy/handlers.ts";
        globalThis.listItems = listItems;
        function init() {
            routeRegistry.registerRoute(PATH, "listItems", "GET");
        }
        "#,
    );

    let handlers = r#"
        import { items } from "./data.ts";
        export const PATH = "/assets-batch/items";
        export function listItems(context) { return ResponseBuilder.json({ items }); }
    "#;
    let data = "export const items = [\"first\", \"second\"];";

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [
                { "name": "assets_batch_deploy/handlers.ts", "content_base64": b64(handlers) },
                { "name": "assets_batch_deploy/data.ts", "content_base64": b64(data) },
            ]
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["written"], json!(2), "{}", body);
    assert_eq!(
        statuses(&body),
        vec![
            (
                "assets_batch_deploy/handlers.ts".to_string(),
                "created".to_string()
            ),
            (
                "assets_batch_deploy/data.ts".to_string(),
                "created".to_string()
            ),
        ]
    );

    // The digest is echoed so a caller can verify what landed without reading
    // it back.
    assert_eq!(body["results"][0]["sha256"], json!(sha256_hex(handlers)));
    assert_eq!(body["results"][1]["bytes"], json!(data.len()));

    // One init(), reported rather than left to run in the background — and it
    // saw both modules, since the route path it registered comes from one of
    // them.
    assert_eq!(body["init"]["ran"], json!(true), "{}", body["init"]);
    assert_eq!(body["init"]["success"], json!(true), "{}", body["init"]);
    assert!(
        registered_paths(uri).contains("/assets-batch/items"),
        "init() should have registered the path the batch-written module exports, got {:?}",
        registered_paths(uri)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn one_bad_file_leaves_the_whole_batch_unwritten() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/atomic";
    deploy(uri, "function init() {}");

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [
                { "name": "assets_batch_atomic/good.ts", "content_base64": b64("export const ok = 1;") },
                { "name": "assets_batch_atomic/bad.ts", "content_base64": "not valid base64!!" },
            ]
        }),
    )
    .await;

    assert_eq!(status, 400, "{}", body);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("assets_batch_atomic/bad.ts"),
        "the error should name the file that failed: {}",
        error
    );

    // The whole point: the valid file ahead of the bad one did not land, so the
    // script never sees half a change.
    assert!(
        stored(uri, "assets_batch_atomic/good.ts").is_none(),
        "a rejected batch must write nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_path_that_escapes_the_script_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/traversal";
    deploy(uri, "function init() {}");

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [
                { "name": "assets_batch_traversal/ok.ts", "content_base64": b64("export const ok = 1;") },
                { "name": "../escaped.ts", "content_base64": b64("export const bad = 1;") },
            ]
        }),
    )
    .await;

    assert_eq!(status, 400, "{}", body);
    assert!(
        stored(uri, "assets_batch_traversal/ok.ts").is_none(),
        "a rejected batch must write nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sha256_that_does_not_match_rejects_the_batch() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/digest";
    deploy(uri, "function init() {}");

    let source = "export const value = 1;";
    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [
                {
                    "name": "assets_batch_digest/value.ts",
                    "content_base64": b64(source),
                    "sha256": sha256_hex("export const value = 2;"),
                }
            ]
        }),
    )
    .await;

    assert_eq!(status, 400, "{}", body);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("assets_batch_digest/value.ts") && error.contains(&sha256_hex(source)),
        "the mismatch should name the file and both digests: {}",
        error
    );
    assert!(stored(uri, "assets_batch_digest/value.ts").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_already_holds_the_content_is_not_rewritten() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/unchanged";
    deploy(uri, "function init() {}");

    let files = json!({
        "files": [
            { "name": "assets_batch_unchanged/util.ts", "content_base64": b64("export const n = 1;") }
        ]
    });

    let (status, first) = post_batch(&format!("script={}", uri), files.clone()).await;
    assert_eq!(status, 200, "{}", first);
    assert_eq!(first["written"], json!(1));

    let (status, second) = post_batch(&format!("script={}", uri), files).await;
    assert_eq!(status, 200, "{}", second);
    assert_eq!(
        statuses(&second),
        vec![(
            "assets_batch_unchanged/util.ts".to_string(),
            "unchanged".to_string()
        )]
    );
    assert_eq!(second["written"], json!(0));

    // Nothing changed, so there is nothing to re-register: running init() here
    // would tear down and rebuild registrations that are already correct.
    assert_eq!(second["init"]["ran"], json!(false), "{}", second["init"]);

    // A changed file in the same place is an update, not a create.
    let (status, third) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [
                { "name": "assets_batch_unchanged/util.ts", "content_base64": b64("export const n = 2;") }
            ]
        }),
    )
    .await;
    assert_eq!(status, 200, "{}", third);
    assert_eq!(third["results"][0]["status"], json!("updated"));
    assert_eq!(
        stored_text(uri, "assets_batch_unchanged/util.ts"),
        "export const n = 2;"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reinit_never_writes_the_files_and_leaves_init_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/no-reinit";
    deploy(
        uri,
        r#"
        function handler(context) { return ResponseBuilder.json({}); }
        globalThis.handler = handler;
        function init() { routeRegistry.registerRoute("/assets-batch/no-reinit", "handler", "GET"); }
        "#,
    );

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "reinit": "never",
            "files": [
                { "name": "assets_batch_no_reinit/part.ts", "content_base64": b64("export const part = 1;") }
            ]
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["written"], json!(1));
    assert_eq!(body["init"]["ran"], json!(false), "{}", body["init"]);
    assert_eq!(
        stored_text(uri, "assets_batch_no_reinit/part.ts"),
        "export const part = 1;"
    );
    assert!(
        !registered_paths(uri).contains("/assets-batch/no-reinit"),
        "reinit=never means init() did not run, so its route is not registered yet"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_reinit_mode_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/bad-reinit";
    deploy(uri, "function init() {}");

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "reinit": "later",
            "files": [
                { "name": "assets_batch_bad_reinit/part.ts", "content_base64": b64("export const part = 1;") }
            ]
        }),
    )
    .await;

    assert_eq!(status, 400, "{}", body);
    assert!(stored(uri, "assets_batch_bad_reinit/part.ts").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_mimetype_is_inferred_from_the_extension() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/mimetype";
    deploy(uri, "function init() {}");

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [
                { "name": "assets_batch_mime/util.ts", "content_base64": b64("export const n = 1;") },
                { "name": "assets_batch_mime/page.html", "content_base64": b64("<p>hi</p>") },
                { "name": "assets_batch_mime/blob.bin", "content_base64": b64("raw") },
                {
                    "name": "assets_batch_mime/typed.ts",
                    "content_base64": b64("export const n = 2;"),
                    "mimetype": "text/plain",
                },
            ]
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    let mimetype = |path: &str| stored(uri, path).expect("asset should be stored").mimetype;
    assert_eq!(mimetype("assets_batch_mime/util.ts"), "text/typescript");
    assert_eq!(mimetype("assets_batch_mime/page.html"), "text/html");
    assert_eq!(
        mimetype("assets_batch_mime/blob.bin"),
        "application/octet-stream"
    );
    // An explicit type still wins over the guess.
    assert_eq!(mimetype("assets_batch_mime/typed.ts"), "text/plain");
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_alone_cannot_write_a_batch() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/authz";
    deploy(uri, "function init() {}");

    let reader = UserContext {
        user_id: Some("reader".to_string()),
        is_authenticated: true,
        capabilities: [Capability::ReadScripts, Capability::ReadAssets]
            .into_iter()
            .collect(),
    };

    let result = aiwebengine::engine_api::upsert_assets_authorized(
        &reader,
        uri,
        &[aiwebengine::engine_api::AssetWrite {
            name: "assets_batch_authz/util.ts".to_string(),
            mimetype: None,
            content_base64: b64("export const n = 1;"),
            expected_sha256: None,
        }],
    );

    assert!(
        result.is_err(),
        "a reader who does not own the script must not write its assets"
    );
    assert!(stored(uri, "assets_batch_authz/util.ts").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_tool_writes_the_same_batch() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/mcp";
    deploy(
        uri,
        r#"
        import { PATH } from "./assets_batch_mcp/routes.ts";
        function handler(context) { return ResponseBuilder.json({}); }
        globalThis.handler = handler;
        function init() { routeRegistry.registerRoute(PATH, "handler", "GET"); }
        "#,
    );

    let source = "export const PATH = \"/assets-batch/mcp\";";
    let result = execute_native_mcp_tool(
        "write_assets",
        &json!({
            "script": uri,
            "files": [
                {
                    "name": "assets_batch_mcp/routes.ts",
                    "content_base64": base64::engine::general_purpose::STANDARD.encode(source),
                    "sha256": sha256_hex(source),
                }
            ]
        }),
        &UserContext::admin("batcher".to_string()),
    )
    .expect("write_assets should dispatch");

    assert_eq!(result["success"], json!(true), "{}", result);
    assert_eq!(result["written"], json!(1), "{}", result);
    assert_eq!(result["init"]["ran"], json!(true), "{}", result["init"]);
    assert_eq!(result["init"]["success"], json!(true), "{}", result["init"]);
    assert!(
        registered_paths(uri).contains("/assets-batch/mcp"),
        "the tool should leave the script initialized from what it wrote, got {:?}",
        registered_paths(uri)
    );
}

/// The claim the endpoint rests on: writing a script's files as a batch tells
/// the rest of the cluster *once*.
///
/// Every asset write sends a `script_upserted` notification, and every instance
/// that receives one reinitializes the script from scratch. Written one at a
/// time, an N-file change costs N reinitializations per instance, each from a
/// tree that is still incomplete.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_notifies_the_cluster_once_where_single_writes_notify_each_time() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/notify";

    let Some(database) = aiwebengine::database::get_global_database() else {
        return;
    };
    let mut listener = sqlx::postgres::PgListener::connect_with(database.pool())
        .await
        .expect("listener should connect");
    listener
        .listen("script_upserted")
        .await
        .expect("listener should subscribe");

    // Storing the script, and clearing the assets an earlier run left under it,
    // announce changes of their own. Take those off the wire first, so what is
    // counted below is the writes under test and nothing else.
    deploy(uri, "function init() {}");
    drain_notifications(&mut listener, uri).await;

    // Three files, one request.
    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "reinit": "never",
            "files": [
                { "name": "assets_batch_notify/a.ts", "content_base64": b64("export const a = 1;") },
                { "name": "assets_batch_notify/b.ts", "content_base64": b64("export const b = 1;") },
                { "name": "assets_batch_notify/c.ts", "content_base64": b64("export const c = 1;") },
            ]
        }),
    )
    .await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["written"], json!(3));

    assert_eq!(
        drain_notifications(&mut listener, uri).await,
        1,
        "three files in one batch should announce one change"
    );

    // The same three files written the old way, for contrast.
    for (path, source) in [
        ("assets_batch_notify/a.ts", "export const a = 2;"),
        ("assets_batch_notify/b.ts", "export const b = 2;"),
        ("assets_batch_notify/c.ts", "export const c = 2;"),
    ] {
        let now = std::time::SystemTime::now();
        repository::upsert_asset(repository::Asset {
            uri: path.to_string(),
            name: Some(path.to_string()),
            mimetype: "text/typescript".to_string(),
            content: source.as_bytes().to_vec(),
            created_at: now,
            updated_at: now,
            script_uri: uri.to_string(),
        })
        .expect("asset should be stored");
    }

    assert_eq!(
        drain_notifications(&mut listener, uri).await,
        3,
        "three separate writes announce three changes"
    );
}

/// Count the `script_upserted` notifications for `script_uri` that are already
/// queued, stopping at the first quiet moment.
async fn drain_notifications(listener: &mut sqlx::postgres::PgListener, script_uri: &str) -> usize {
    let mut count = 0;
    loop {
        let received =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.recv()).await;
        match received {
            Ok(Ok(notification)) => {
                let payload: Value = serde_json::from_str(notification.payload())
                    .expect("notification payload should be JSON");
                if payload["uri"] == json!(script_uri) {
                    count += 1;
                }
            }
            Ok(Err(e)) => panic!("listener failed: {}", e),
            // Nothing more is coming.
            Err(_) => return count,
        }
    }
}

/// A batch carries what a single asset write never had to: a script's whole
/// module tree in one body.
///
/// The management router caps request bodies at
/// `security.max_request_body_bytes` — 1MB by default — which a tree of source
/// files passes without trying. The batch route raises that ceiling to what it
/// actually enforces on content, and this is that wiring, over a real server:
/// nothing below the handler can be tested for it, since the limit is applied
/// while the body is read.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_over_the_management_body_limit_is_still_accepted() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/large";
    deploy(uri, "function init() {}");

    let engine = AdminServer::start().await.expect("server failed to start");
    let port = engine.port();

    // Over the 1MB default before base64 even widens it.
    let source = format!("export const filler = \"{}\";", "x".repeat(1_500_000));

    let response = engine
        .client()
        .post(format!(
            "http://127.0.0.1:{}/engine/assets/batch?script={}",
            port, uri
        ))
        .json(&json!({
            "reinit": "never",
            "files": [
                { "name": "assets_batch_large/filler.ts", "content_base64": b64(&source) }
            ]
        }))
        .send()
        .await
        .expect("batch request failed");

    let status = response.status();
    let body: Value = response.json().await.expect("batch response not JSON");
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["written"], json!(1), "{}", body);
    assert_eq!(
        stored(uri, "assets_batch_large/filler.ts")
            .expect("asset should be stored")
            .content
            .len(),
        source.len()
    );

    engine.shutdown().await;
}

/// Writes and removals are one transaction, so a sync's tree either becomes
/// what the caller described or stays as it was. Asserted through the batch
/// path because that is where both halves meet.
#[tokio::test(flavor = "multi_thread")]
async fn a_sync_writes_and_removes_as_one_act() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/sync";
    deploy(uri, "function init() {}");

    let user = UserContext::admin("batcher".to_string());
    let keep = "assets_batch_sync/keep.ts";
    let drop = "assets_batch_sync/drop.ts";

    aiwebengine::engine_api::upsert_assets_authorized(
        &user,
        uri,
        &[
            aiwebengine::engine_api::AssetWrite {
                name: keep.to_string(),
                mimetype: None,
                content_base64: b64("export const keep = 1;"),
                expected_sha256: None,
            },
            aiwebengine::engine_api::AssetWrite {
                name: drop.to_string(),
                mimetype: None,
                content_base64: b64("export const drop = 2;"),
                expected_sha256: None,
            },
        ],
    )
    .expect("initial write should succeed");

    let outcome = aiwebengine::engine_api::upsert_assets_synced(
        &user,
        uri,
        &[aiwebengine::engine_api::AssetWrite {
            name: keep.to_string(),
            mimetype: None,
            content_base64: b64("export const keep = 3;"),
            expected_sha256: None,
        }],
        aiwebengine::engine_api::AssetSyncOptions {
            delete: &[drop.to_string()],
            ..Default::default()
        },
    )
    .expect("sync should succeed");

    assert_eq!(outcome.written, 1);
    assert_eq!(outcome.deleted, 1);
    assert_eq!(stored_text(uri, keep), "export const keep = 3;");
    assert!(stored(uri, drop).is_none(), "the removal landed too");
    assert!(
        outcome.revision.is_some(),
        "and the whole change is one revision"
    );
}

/// The whole of a change, not the assets of one. A change touching the root
/// and the modules it imports used to be two writes — two revisions, two
/// notifications, two init() runs — even though `/engine/check` would check
/// exactly that change in one request.
#[tokio::test(flavor = "multi_thread")]
async fn the_root_source_travels_with_the_modules_as_one_change() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/root";
    deploy(
        uri,
        r#"
        import { PATH } from "./assets_batch_root/routes.ts";
        function handler(context) { return ResponseBuilder.json({}); }
        globalThis.handler = handler;
        function init() { routeRegistry.registerRoute(PATH, "handler", "GET"); }
        "#,
    );

    let root = "import { PATH } from \"./assets_batch_root/routes.ts\";\n\
                function handler(context) { return ResponseBuilder.json({ v: 2 }); }\n\
                globalThis.handler = handler;\n\
                function init() { routeRegistry.registerRoute(PATH, \"handler\", \"POST\"); }\n";

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "content": root,
            "files": [{
                "name": "assets_batch_root/routes.ts",
                "content_base64": b64("export const PATH = \"/assets-batch/root\";\n"),
            }],
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["root"], json!("updated"), "{}", body);
    assert_eq!(body["written"], json!(1), "{}", body);
    assert_eq!(
        repository::fetch_script(uri).as_deref(),
        Some(root),
        "the root source should have been stored with the modules"
    );

    // One revision for the whole change, rather than one per file: that is the
    // unit a person reverts.
    let revision = body["revision"].as_i64().expect("a revision was recorded");
    assert_eq!(body["init"]["ran"], json!(true), "{}", body["init"]);
    assert!(
        registered_paths(uri).contains("/assets-batch/root"),
        "init() should have run once, over the whole change, got {:?}",
        registered_paths(uri)
    );

    // A second identical write changes nothing, so it records no revision and
    // leaves init() alone.
    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "content": root,
            "files": [{
                "name": "assets_batch_root/routes.ts",
                "content_base64": b64("export const PATH = \"/assets-batch/root\";\n"),
            }],
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["root"], json!("unchanged"), "{}", body);
    assert_eq!(body["revision"], json!(null), "{}", body);
    assert_eq!(body["init"]["ran"], json!(false), "{}", body["init"]);
    assert!(
        revision > 0,
        "the first write should have recorded a revision"
    );
}

/// Removing a module is as much a change as rewriting one, and belongs in the
/// same request as the change to the code that stopped importing it.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_removes_the_files_a_change_drops() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/remove";
    deploy(uri, "function init() {}");

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [
                { "name": "assets_batch_remove/keep.ts", "content_base64": b64("export const a = 1;\n") },
                { "name": "assets_batch_remove/drop.ts", "content_base64": b64("export const b = 2;\n") },
            ],
            "reinit": "never",
        }),
    )
    .await;
    assert_eq!(status, 200, "{}", body);

    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "files": [{
                "name": "assets_batch_remove/keep.ts",
                "content_base64": b64("export const a = 10;\n"),
            }],
            "remove": ["assets_batch_remove/drop.ts"],
            "reinit": "never",
        }),
    )
    .await;

    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["deleted"], json!(1), "{}", body);
    assert_eq!(
        stored_text(uri, "assets_batch_remove/keep.ts"),
        "export const a = 10;\n"
    );
    assert!(
        stored(uri, "assets_batch_remove/drop.ts").is_none(),
        "the removed module should be gone"
    );

    // Naming a file the script no longer has is a change that has already
    // happened, not an error.
    let (status, body) = post_batch(
        &format!("script={}", uri),
        json!({
            "remove": ["assets_batch_remove/drop.ts"],
            "reinit": "never",
        }),
    )
    .await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body["revision"], json!(null), "{}", body);
}

/// `files` stays required for a caller that sent neither of the other two, so
/// a request that meant to carry files and did not is still told so.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_carrying_nothing_at_all_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/nothing";
    deploy(uri, "function init() {}");

    let (status, body) = post_batch(&format!("script={}", uri), json!({ "reinit": "never" })).await;
    assert_eq!(status, 400, "{}", body);
    assert!(
        body["error"].as_str().unwrap_or_default().contains("files"),
        "{}",
        body["error"]
    );
}

/// Writing the root takes what writing a script takes, even inside a request
/// whose other files only take what writing an asset takes.
#[tokio::test(flavor = "multi_thread")]
async fn writing_the_root_in_a_batch_takes_script_write_rights() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/root-authz";
    let source = "function init() {}";
    deploy(uri, source);

    let asset_writer = UserContext {
        user_id: Some("asset-writer".to_string()),
        is_authenticated: true,
        capabilities: [
            Capability::ReadScripts,
            Capability::ReadAssets,
            Capability::WriteAssets,
        ]
        .into_iter()
        .collect(),
    };

    let result = aiwebengine::engine_api::write_script_files_authorized(
        &asset_writer,
        uri,
        aiwebengine::engine_api::ScriptFilesChange {
            root: Some("function init() { /* mine now */ }"),
            writes: &[],
            delete: &[],
        },
        aiwebengine::engine_api::ScriptWriteOptions::default(),
    );

    assert!(
        result.is_err(),
        "WriteAssets must not be a way to write a script's root source"
    );
    assert_eq!(
        repository::fetch_script(uri).as_deref(),
        Some(source),
        "nothing should have been written"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_tool_writes_the_whole_change_too() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/mcp-root";
    deploy(uri, "function init() {}");

    let seed = execute_native_mcp_tool(
        "write_assets",
        &json!({
            "script": uri,
            "files": [{
                "name": "assets_batch_mcp_root/old.ts",
                "content_base64": b64("export const gone = true;\n"),
            }],
            "reinit": "never",
        }),
        &UserContext::admin("batcher".to_string()),
    )
    .expect("write_assets should dispatch");
    assert_eq!(seed["success"], json!(true), "{}", seed);

    let root = "function init() { /* rewritten */ }";
    let result = execute_native_mcp_tool(
        "write_assets",
        &json!({
            "script": uri,
            "content": root,
            "files": [{
                "name": "assets_batch_mcp_root/new.ts",
                "content_base64": b64("export const here = true;\n"),
            }],
            "remove": ["assets_batch_mcp_root/old.ts"],
            "reinit": "never",
        }),
        &UserContext::admin("batcher".to_string()),
    )
    .expect("write_assets should dispatch");

    assert_eq!(result["success"], json!(true), "{}", result);
    assert_eq!(result["root"], json!("updated"), "{}", result);
    assert_eq!(result["written"], json!(1), "{}", result);
    assert_eq!(result["deleted"], json!(1), "{}", result);
    assert_eq!(repository::fetch_script(uri).as_deref(), Some(root));
    assert!(stored(uri, "assets_batch_mcp_root/old.ts").is_none());
}

/// `create_file` could say "this is new, fail if it is not" and the asset
/// write could not, so creating a module and overwriting somebody's were the
/// same request.
#[tokio::test(flavor = "multi_thread")]
async fn creating_an_asset_refuses_to_overwrite_one() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/create";
    deploy(uri, "function init() {}");

    let created = execute_native_mcp_tool(
        "create_asset",
        &json!({
            "script": uri,
            "asset": "assets_batch_create/util.ts",
            "content": b64("export const n = 1;\n"),
        }),
        &UserContext::admin("batcher".to_string()),
    )
    .expect("create_asset should dispatch");

    assert_eq!(created["success"], json!(true), "{}", created);
    assert_eq!(
        stored_text(uri, "assets_batch_create/util.ts"),
        "export const n = 1;\n"
    );
    // The type is inferred from the extension, as a batch write infers it.
    assert_eq!(
        stored(uri, "assets_batch_create/util.ts")
            .expect("asset should be stored")
            .mimetype,
        "text/typescript"
    );

    let again = execute_native_mcp_tool(
        "create_asset",
        &json!({
            "script": uri,
            "asset": "assets_batch_create/util.ts",
            "content": b64("export const n = 2;\n"),
        }),
        &UserContext::admin("batcher".to_string()),
    )
    .expect("create_asset should dispatch");

    assert!(
        again["error"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists"),
        "the second create should be refused, got {}",
        again
    );
    assert_eq!(
        stored_text(uri, "assets_batch_create/util.ts"),
        "export const n = 1;\n",
        "the refusal must not have overwritten anything"
    );

    // write_asset is still the way to say "overwrite".
    let overwritten = execute_native_mcp_tool(
        "write_asset",
        &json!({
            "script": uri,
            "asset": "assets_batch_create/util.ts",
            "mimetype": "text/typescript",
            "content": b64("export const n = 2;\n"),
        }),
        &UserContext::admin("batcher".to_string()),
    )
    .expect("write_asset should dispatch");
    assert_eq!(overwritten["success"], json!(true), "{}", overwritten);
    assert_eq!(
        stored_text(uri, "assets_batch_create/util.ts"),
        "export const n = 2;\n"
    );
}

/// Over HTTP the same precondition is the one HTTP already has a name for.
#[tokio::test(flavor = "multi_thread")]
async fn if_none_match_makes_the_asset_write_a_create() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://assets-batch/if-none-match";
    deploy(uri, "function init() {}");

    let post = |headers: axum::http::HeaderMap, content: &str| {
        let body = json!({
            "asset": "assets_batch_inm/util.ts",
            "mimetype": "text/typescript",
            "content": b64(content),
        });
        aiwebengine::engine_api::assets_post_route(
            admin_extension(),
            Query(
                serde_urlencoded::from_str::<AssetQuery>(&format!("script={}", uri))
                    .expect("query should parse"),
            ),
            headers,
            axum::body::Bytes::from(body.to_string()),
        )
    };

    let mut create = axum::http::HeaderMap::new();
    create.insert("if-none-match", "*".parse().expect("header should parse"));

    let response = post(create.clone(), "export const n = 1;\n").await;
    assert_eq!(response.status(), 201);

    let response = post(create, "export const n = 2;\n").await;
    assert_eq!(
        response.status(),
        409,
        "a create over something that is there is a conflict"
    );
    assert_eq!(
        stored_text(uri, "assets_batch_inm/util.ts"),
        "export const n = 1;\n"
    );

    // Without the header it is the upsert it always was.
    let response = post(axum::http::HeaderMap::new(), "export const n = 3;\n").await;
    assert_eq!(response.status(), 201);
    assert_eq!(
        stored_text(uri, "assets_batch_inm/util.ts"),
        "export const n = 3;\n"
    );
}
