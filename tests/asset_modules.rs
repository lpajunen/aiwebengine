use aiwebengine::js_engine::{
    RequestExecutionParams, call_init_if_exists, execute_scheduled_handler,
    execute_script_for_request_secure, execute_script_secure,
};
use aiwebengine::module_loader;
use aiwebengine::repository;
use aiwebengine::scheduler::{ScheduledInvocation, ScheduledInvocationKind};
use aiwebengine::script_init::InitContext;
use aiwebengine::security::UserContext;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::{Mutex, OnceCell};
use uuid::Uuid;

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn test_mutex() -> &'static Mutex<()> {
    TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

async fn setup_env() {
    INIT.get_or_init(|| async {
        let config = aiwebengine::config::AppConfig::test_config_postgres(0);
        if let Ok(db) = aiwebengine::database::Database::new(&config.repository).await {
            let db_arc = std::sync::Arc::new(db);
            aiwebengine::database::initialize_global_database(db_arc.clone());
            repository::initialize_repository(repository::PostgresRepository::new(
                db_arc.pool().clone(),
                "test".to_string(),
            ));
        }
    })
    .await;
}

fn test_asset(script_uri: &str, uri: &str, mimetype: &str, content: &[u8]) -> repository::Asset {
    let now = std::time::SystemTime::now();
    repository::Asset {
        uri: uri.to_string(),
        name: Some(uri.to_string()),
        mimetype: mimetype.to_string(),
        content: content.to_vec(),
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    }
}

fn ensure_script(script_uri: &str) {
    repository::upsert_script(script_uri, "export function init() {};")
        .expect("script should be stored");
}

fn imported_helper_asset(script_uri: &str, asset_uri: &str) -> repository::Asset {
    test_asset(
        script_uri,
        asset_uri,
        "text/plain",
        br#"
            export function buildMessage(target: string) {
                return `hello-from-${target}`;
            }
        "#,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn module_loader_uses_root_script_owned_assets_only() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let root_script_uri = "test://asset-module-owner-root";
    let foreign_script_uri = "test://asset-module-owner-foreign";

    ensure_script(root_script_uri);
    ensure_script(foreign_script_uri);

    repository::upsert_asset(test_asset(
        foreign_script_uri,
        "server/shared.ts",
        "text/plain",
        b"export const shared = 'foreign';",
    ))
    .expect("foreign asset should be stored");

    let error =
        module_loader::load_owned_asset_module(root_script_uri, "main.ts", "./server/shared.ts")
            .expect_err("foreign script asset should not resolve for root script");

    assert_eq!(
        error.to_string(),
        "Module './server/shared.ts' imported from 'main.ts' was not found in assets for 'test://asset-module-owner-root'"
    );

    assert!(repository::delete_asset(
        foreign_script_uri,
        "server/shared.ts"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn module_loader_reads_same_script_asset() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-same-script";
    let asset_uri = "server/helper-same.ts";
    ensure_script(script_uri);

    repository::upsert_asset(test_asset(
        script_uri,
        asset_uri,
        "text/plain",
        b"export const helper = () => 'ok';",
    ))
    .expect("asset should be stored");

    assert!(
        repository::fetch_asset(script_uri, asset_uri).is_some(),
        "stored asset should be readable directly from repository"
    );

    let module =
        module_loader::load_owned_asset_module(script_uri, "main.ts", "./server/helper-same.ts")
            .expect("same-script asset module should load");

    assert_eq!(module.logical_path, asset_uri);
    assert!(module.content.contains("helper"));

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn module_loader_rejects_missing_asset() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    ensure_script("test://asset-module-missing");

    let error = module_loader::load_owned_asset_module(
        "test://asset-module-missing",
        "main.ts",
        "./server/missing.ts",
    )
    .expect_err("missing asset should be rejected");

    assert_eq!(
        error.to_string(),
        "Module './server/missing.ts' imported from 'main.ts' was not found in assets for 'test://asset-module-missing'"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn module_loader_rejects_binary_asset_content() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-binary";
    let asset_uri = "server/helper-binary.ts";
    ensure_script(script_uri);

    repository::upsert_asset(test_asset(
        script_uri,
        asset_uri,
        "application/javascript",
        &[0xff, 0xfe, 0xfd],
    ))
    .expect("asset should be stored");

    let error =
        module_loader::load_owned_asset_module(script_uri, "main.ts", "./server/helper-binary.ts")
            .expect_err("binary asset should be rejected");

    assert_eq!(
        error.to_string(),
        "Module 'server/helper-binary.ts' must be valid UTF-8 text content"
    );

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn module_loader_rejects_unsupported_asset_type() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-unsupported";
    let asset_uri = "server/helper-unsupported.css";
    ensure_script(script_uri);

    repository::upsert_asset(test_asset(
        script_uri,
        asset_uri,
        "text/css",
        b"body { color: red; }",
    ))
    .expect("asset should be stored");

    let error = module_loader::load_owned_asset_module(
        script_uri,
        "main.ts",
        "./server/helper-unsupported.css",
    )
    .expect_err("unsupported asset type should be rejected");

    assert_eq!(
        error.to_string(),
        "Module 'server/helper-unsupported.css' has unsupported asset type 'text/css'"
    );

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn imported_asset_module_executes_in_request_path() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-request.ts";
    let asset_uri = "server/request-helper.ts";
    ensure_script(script_uri);
    repository::upsert_asset(imported_helper_asset(script_uri, asset_uri))
        .expect("asset should be stored");

    let script_content = r#"
        import { buildMessage } from "./server/request-helper.ts";

        function handleImportedRequest(context) {
            return ResponseBuilder.text(buildMessage("request"));
        }
    "#;

    let setup_result = execute_script_secure(
        script_uri,
        script_content,
        UserContext::authenticated("asset-request-user".to_string()),
    );
    assert!(
        setup_result.success,
        "script setup should succeed: {:?}",
        setup_result.error
    );

    let response = execute_script_for_request_secure(RequestExecutionParams {
        script_uri: script_uri.to_string(),
        handler_name: "handleImportedRequest".to_string(),
        path: "/asset-request".to_string(),
        method: "GET".to_string(),
        query_params: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::authenticated("asset-request-user".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
    })
    .expect("request execution should succeed");

    let body = String::from_utf8(response.body).expect("response should be utf-8 text");
    assert_eq!(body, "hello-from-request");

    assert!(repository::delete_asset(script_uri, asset_uri));
}

/// Module sources are cached across builds, so the cache must not outlive the
/// asset it was read from — and must survive edits that cannot have changed it.
#[tokio::test(flavor = "multi_thread")]
async fn cached_module_sources_track_asset_edits() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-cache-invalidation.ts";
    let asset_uri = "server/versioned-helper.ts";
    let script_content = r#"
        import { version } from "./server/versioned-helper.ts";

        function handleVersionRequest(context) {
            return ResponseBuilder.text(version());
        }
    "#;

    let store_version = |version: &str| {
        repository::upsert_asset(test_asset(
            script_uri,
            asset_uri,
            "text/plain",
            format!("export function version() {{ return \"{}\"; }}", version).as_bytes(),
        ))
        .expect("asset should be stored");
    };

    let served_version = || {
        let response = execute_script_for_request_secure(RequestExecutionParams {
            script_uri: script_uri.to_string(),
            handler_name: "handleVersionRequest".to_string(),
            path: "/asset-cache".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::authenticated("asset-cache-user".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
        })
        .expect("request execution should succeed");
        String::from_utf8(response.body).expect("response should be utf-8 text")
    };

    ensure_script(script_uri);
    store_version("v1");
    repository::upsert_script(script_uri, script_content).expect("script should be stored");
    assert_eq!(served_version(), "v1");

    // Editing the asset must reach the next build even though its source is cached.
    store_version("v2");
    assert_eq!(
        served_version(),
        "v2",
        "an asset edit must invalidate its cached module source"
    );

    // Re-upserting the root script leaves the asset untouched, so the cached
    // module source is reused — it must still be the current one.
    repository::upsert_script(script_uri, script_content).expect("script should be stored");
    assert_eq!(
        served_version(),
        "v2",
        "a script edit must not resurrect a stale module source"
    );

    assert!(repository::delete_asset(script_uri, asset_uri));

    // With the asset gone, the import can no longer resolve.
    let error = module_loader::load_owned_asset_module(
        script_uri,
        "main.ts",
        "./server/versioned-helper.ts",
    )
    .expect_err("a deleted asset must not be served from cache");
    assert!(
        error.to_string().contains("was not found in assets"),
        "unexpected error: {}",
        error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn imported_asset_root_module_executes_in_request_path() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-root-module-request.ts";
    let asset_uri = "server/request-helper-root.ts";
    ensure_script(script_uri);
    repository::upsert_asset(imported_helper_asset(script_uri, asset_uri))
        .expect("asset should be stored");

    let script_content = r#"
        import { buildMessage } from "server/request-helper-root.ts";

        function handleImportedRequest(context) {
            return ResponseBuilder.text(buildMessage("request-root"));
        }
    "#;

    let setup_result = execute_script_secure(
        script_uri,
        script_content,
        UserContext::authenticated("asset-root-request-user".to_string()),
    );
    assert!(
        setup_result.success,
        "script setup should succeed: {:?}",
        setup_result.error
    );

    let response = execute_script_for_request_secure(RequestExecutionParams {
        script_uri: script_uri.to_string(),
        handler_name: "handleImportedRequest".to_string(),
        path: "/asset-root-request".to_string(),
        method: "GET".to_string(),
        query_params: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::authenticated("asset-root-request-user".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
    })
    .expect("request execution should succeed");

    let body = String::from_utf8(response.body).expect("response should be utf-8 text");
    assert_eq!(body, "hello-from-request-root");

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn imported_multiline_asset_root_module_executes_in_request_path() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-root-module-multiline-request.ts";
    let asset_uri = "server/request-helper-root-multiline.ts";
    ensure_script(script_uri);
    repository::upsert_asset(test_asset(
        script_uri,
        asset_uri,
        "text/plain",
        br#"
            export const buildMessage = (target) => `hello-from-${target}`;
            export const buildSecondary = (target) => `ignored-${target}`;
        "#,
    ))
    .expect("asset should be stored");

    let script_content = r#"
        import {
            buildMessage,
            buildSecondary,
        } from "server/request-helper-root-multiline.ts";

        function handleImportedRequest(context) {
            buildSecondary("unused");
            return ResponseBuilder.text(buildMessage("request-root-multiline"));
        }
    "#;

    let setup_result = execute_script_secure(
        script_uri,
        script_content,
        UserContext::authenticated("asset-root-multiline-request-user".to_string()),
    );
    assert!(
        setup_result.success,
        "script setup should succeed: {:?}",
        setup_result.error
    );

    let response = execute_script_for_request_secure(RequestExecutionParams {
        script_uri: script_uri.to_string(),
        handler_name: "handleImportedRequest".to_string(),
        path: "/asset-root-request-multiline".to_string(),
        method: "GET".to_string(),
        query_params: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::authenticated("asset-root-multiline-request-user".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
    })
    .expect("request execution should succeed");

    let body = String::from_utf8(response.body).expect("response should be utf-8 text");
    assert_eq!(body, "hello-from-request-root-multiline");

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn imported_typescript_asset_module_with_type_exports_executes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-root-module-type-exports.ts";
    let asset_uri = "server/world-domain.ts";
    ensure_script(script_uri);
    repository::upsert_asset(test_asset(
        script_uri,
        asset_uri,
        "text/plain",
        br#"
            export type WorldType = "forest" | "cave";

            export interface WorldTileDef {
                value: number;
                walkable: boolean;
            }

            export const WORLD_TILE_DEFS: Record<string, WorldTileDef> = {
                ground: { value: 1, walkable: true },
            };

            export function getWorldTileDef(tileName: string): WorldTileDef {
                return WORLD_TILE_DEFS[tileName] || WORLD_TILE_DEFS.ground;
            }

            export function worldTileValueForName(tileName: string): number {
                return getWorldTileDef(tileName).value;
            }
        "#,
    ))
    .expect("asset should be stored");

    let script_content = r#"
        import {
            getWorldTileDef,
            worldTileValueForName,
        } from "server/world-domain.ts";

        function handleImportedRequest(context) {
            const tile = getWorldTileDef("ground");
            return ResponseBuilder.json({
                value: worldTileValueForName("ground"),
                walkable: tile.walkable,
            });
        }
    "#;

    let setup_result = execute_script_secure(
        script_uri,
        script_content,
        UserContext::authenticated("asset-root-type-exports-user".to_string()),
    );
    assert!(
        setup_result.success,
        "script setup should succeed: {:?}",
        setup_result.error
    );

    let response = execute_script_for_request_secure(RequestExecutionParams {
        script_uri: script_uri.to_string(),
        handler_name: "handleImportedRequest".to_string(),
        path: "/asset-root-type-exports".to_string(),
        method: "GET".to_string(),
        query_params: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::authenticated("asset-root-type-exports-user".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
    })
    .expect("request execution should succeed");

    let body = String::from_utf8(response.body).expect("response should be utf-8 text");
    assert_eq!(body, "{\"value\":1,\"walkable\":true}");

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn imported_asset_module_executes_in_init_path() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-init.ts";
    let asset_uri = "server/init-helper.ts";
    ensure_script(script_uri);
    repository::upsert_asset(imported_helper_asset(script_uri, asset_uri))
        .expect("asset should be stored");

    let script_content = r#"
        import { buildMessage } from "./server/init-helper.ts";

        function importedInitHandler(context) {
            return ResponseBuilder.text(buildMessage("init"));
        }

        function init(context) {
            console.info(buildMessage("init-log"));
            routeRegistry.registerRoute("/asset-init", "importedInitHandler", "GET");
        }
    "#;

    repository::upsert_script(script_uri, script_content).expect("script should be stored");

    let result = call_init_if_exists(
        script_uri,
        script_content,
        InitContext::new(script_uri.to_string(), true),
    )
    .expect("init execution should succeed")
    .expect("init should be called");

    let route = result
        .get(&("/asset-init".to_string(), "GET".to_string()))
        .expect("init should register route using imported helper module");
    assert_eq!(route.handler_name, "importedInitHandler");

    let logs = repository::fetch_log_messages(script_uri);
    assert!(
        logs.iter()
            .any(|entry| entry.message.contains("hello-from-init-log")),
        "init path should log imported helper output"
    );

    assert!(repository::delete_asset(script_uri, asset_uri));
}

/// End-to-end: an asset route registered from JS with a `metadata` object must
/// carry the given tags/summary into the asset registry (which the OpenAPI
/// generator then reads).
#[tokio::test(flavor = "multi_thread")]
async fn register_asset_route_records_metadata_tags() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-metadata-tags";
    let asset_uri = "repro.css";
    ensure_script(script_uri);
    repository::upsert_asset(test_asset(script_uri, asset_uri, "text/css", b"body{}"))
        .expect("asset should be stored");

    // Ensure a clean registry so we only observe this test's registration.
    aiwebengine::asset_registry::get_global_registry().clear();

    let script_content = r#"
        function init(context) {
          routeRegistry.registerAssetRoute("/repro-asset.css", "repro.css", {
            tags: ["ReproGroup"],
            summary: "Repro asset",
          });
        }
    "#;

    call_init_if_exists(
        script_uri,
        script_content,
        InitContext::new(script_uri.to_string(), true),
    )
    .expect("init execution should succeed");

    let registrations = aiwebengine::asset_registry::get_global_registry().get_all_registrations();
    let (_, reg) = registrations
        .iter()
        .find(|(path, _)| path == "/repro-asset.css")
        .expect("asset route should be registered");

    assert_eq!(
        reg.metadata.tags,
        vec!["ReproGroup".to_string()],
        "asset route metadata should carry the tags passed from JS"
    );
    assert_eq!(reg.metadata.summary.as_deref(), Some("Repro asset"));

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn imported_asset_module_executes_in_scheduled_path() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-scheduled.ts";
    let asset_uri = "server/scheduled-helper.ts";
    ensure_script(script_uri);
    repository::upsert_asset(imported_helper_asset(script_uri, asset_uri))
        .expect("asset should be stored");

    let script_content = r#"
        import { buildMessage } from "./server/scheduled-helper.ts";

        function runImportedSchedule(context) {
            console.info(buildMessage("scheduled"));
        }
    "#;

    repository::upsert_script(script_uri, script_content).expect("script should be stored");
    repository::clear_log_messages(script_uri).expect("logs should be clearable");

    let invocation = ScheduledInvocation {
        job_id: Uuid::new_v4(),
        key: "asset-module-schedule".to_string(),
        script_uri: script_uri.to_string(),
        handler_name: "runImportedSchedule".to_string(),
        kind: ScheduledInvocationKind::OneOff,
        scheduled_for: Utc::now(),
        interval_seconds: None,
        interval_milliseconds: None,
    };

    execute_scheduled_handler(script_uri, "runImportedSchedule", &invocation)
        .expect("scheduled handler should execute");

    let logs = repository::fetch_log_messages(script_uri);
    assert!(
        logs.iter()
            .any(|entry| entry.message.contains("hello-from-scheduled")),
        "scheduled path should log imported helper output"
    );

    assert!(repository::delete_asset(script_uri, asset_uri));
}

#[tokio::test(flavor = "multi_thread")]
async fn nested_asset_relative_import_chain_executes_in_request_path() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-module-nested-chain.ts";
    let world_domain_uri = "server/world-domain.ts";
    let world_map_uri = "server/world-map.ts";
    ensure_script(script_uri);

    repository::upsert_asset(test_asset(
        script_uri,
        world_domain_uri,
        "text/plain",
        br#"
            export const WORLD_TILE_GROUND = "ground";

            export function worldTileValueForName(tileName: string): number {
                return tileName === WORLD_TILE_GROUND ? 7 : 0;
            }
        "#,
    ))
    .expect("world-domain asset should be stored");

    repository::upsert_asset(test_asset(
        script_uri,
        world_map_uri,
        "text/plain",
        br#"
            import {
                WORLD_TILE_GROUND,
                worldTileValueForName,
            } from "./world-domain.ts";

            export function generateWorldMap(): number[][] {
                return [[worldTileValueForName(WORLD_TILE_GROUND)]];
            }
        "#,
    ))
    .expect("world-map asset should be stored");

    let script_content = r#"
        import { generateWorldMap } from "./server/world-map.ts";

        function handleImportedRequest(context) {
            const map = generateWorldMap();
            return ResponseBuilder.json({ tile: map[0][0] });
        }
    "#;

    let setup_result = execute_script_secure(
        script_uri,
        script_content,
        UserContext::authenticated("asset-nested-chain-user".to_string()),
    );
    assert!(
        setup_result.success,
        "script setup should succeed: {:?}",
        setup_result.error
    );

    let response = execute_script_for_request_secure(RequestExecutionParams {
        script_uri: script_uri.to_string(),
        handler_name: "handleImportedRequest".to_string(),
        path: "/asset-nested-chain".to_string(),
        method: "GET".to_string(),
        query_params: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::authenticated("asset-nested-chain-user".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
    })
    .expect("request execution should succeed");

    let body = String::from_utf8(response.body).expect("response should be utf-8 text");
    assert_eq!(body, "{\"tile\":7}");

    assert!(repository::delete_asset(script_uri, world_map_uri));
    assert!(repository::delete_asset(script_uri, world_domain_uri));
}

#[test]
fn root_module_path_keeps_last_path_segment() {
    let path = module_loader::root_module_path("https://example.com/scripts/app/main.ts")
        .expect("script uri should yield root module path");
    assert_eq!(path, "main.ts");
}

/// `fetch_script` serves source from the in-memory metadata cache. An upsert
/// must evict that entry so a subsequent fetch returns the new source rather
/// than a stale cached copy.
#[tokio::test(flavor = "multi_thread")]
async fn upsert_evicts_cached_script_source() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://source-cache-evict.ts";
    repository::upsert_script(
        uri,
        "function h(c) { return { status: 200, body: \"v1\" }; }",
    )
    .expect("store v1");

    // Populate the source cache the way a route-index rebuild does in production.
    let _ = repository::get_script_metadata(uri);
    assert!(
        repository::fetch_script(uri)
            .expect("fetch v1")
            .contains("v1"),
        "cached fetch should return v1"
    );

    repository::upsert_script(
        uri,
        "function h(c) { return { status: 200, body: \"v2\" }; }",
    )
    .expect("store v2");
    assert!(
        repository::fetch_script(uri)
            .expect("fetch v2")
            .contains("v2"),
        "upsert must evict the cached source so fetch returns v2"
    );

    let _ = repository::delete_script(uri);
}

/// The prepared-program cache must not serve a stale bundle after an imported
/// asset changes. Because the cache key hashes only the root script (unchanged
/// here), correctness depends on `upsert_asset` invalidating the cache.
#[tokio::test(flavor = "multi_thread")]
async fn edited_imported_asset_invalidates_prepared_program_cache() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-cache-invalidation.ts";
    let asset_uri = "server/edit-helper.ts";

    let script_content = r#"
        import { buildMessage } from "./server/edit-helper.ts";

        function editHandler(context) {
            return { status: 200, body: buildMessage("x"), contentType: "text/plain" };
        }
    "#;
    repository::upsert_script(script_uri, script_content).expect("store root script");

    repository::upsert_asset(test_asset(
        script_uri,
        asset_uri,
        "text/plain",
        b"export function buildMessage(t) { return `v1-${t}`; }",
    ))
    .expect("store v1 asset");

    let run = || {
        let response = execute_script_for_request_secure(RequestExecutionParams {
            script_uri: script_uri.to_string(),
            handler_name: "editHandler".to_string(),
            path: "/asset-edit".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::authenticated("asset-edit-user".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
        })
        .expect("request execution should succeed");
        String::from_utf8(response.body).expect("utf-8 body")
    };

    // First request populates the prepared-program cache with v1.
    assert_eq!(run(), "v1-x");

    // Edit the imported asset. Root script is byte-identical, so only the
    // asset-change invalidation can prevent a stale cache hit.
    repository::upsert_asset(test_asset(
        script_uri,
        asset_uri,
        "text/plain",
        b"export function buildMessage(t) { return `v2-${t}`; }",
    ))
    .expect("store v2 asset");

    assert_eq!(
        run(),
        "v2-x",
        "edited asset must take effect (cache invalidated)"
    );

    assert!(repository::delete_asset(script_uri, asset_uri));
    let _ = repository::delete_script(script_uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_scripts_can_own_the_same_asset_path() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let first_uri = "test://asset-path-sharing-first";
    let second_uri = "test://asset-path-sharing-second";
    let shared_path = "server/shared-name.ts";

    ensure_script(first_uri);
    ensure_script(second_uri);
    repository::delete_asset(first_uri, shared_path);
    repository::delete_asset(second_uri, shared_path);

    repository::upsert_asset(test_asset(
        first_uri,
        shared_path,
        "text/plain",
        b"export const owner = 'first';",
    ))
    .expect("the first script's asset should store");

    // The same path under a different script is a different asset, not a
    // collision with the first script's row.
    repository::upsert_asset(test_asset(
        second_uri,
        shared_path,
        "text/plain",
        b"export const owner = 'second';",
    ))
    .expect("the second script should be able to use the same path");

    let first = repository::fetch_asset(first_uri, shared_path).expect("first asset should exist");
    let second =
        repository::fetch_asset(second_uri, shared_path).expect("second asset should exist");

    assert_eq!(
        String::from_utf8_lossy(&first.content),
        "export const owner = 'first';",
        "the second script's write must not have taken over the first script's asset"
    );
    assert_eq!(
        String::from_utf8_lossy(&second.content),
        "export const owner = 'second';"
    );
    assert_eq!(first.script_uri, first_uri);
    assert_eq!(second.script_uri, second_uri);

    // Deleting one leaves the other alone.
    assert!(repository::delete_asset(second_uri, shared_path));
    assert!(repository::fetch_asset(first_uri, shared_path).is_some());

    repository::delete_asset(first_uri, shared_path);
}

#[tokio::test(flavor = "multi_thread")]
async fn rewriting_an_asset_keeps_it_with_its_script() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://asset-path-rewrite-owner";
    let asset_path = "server/rewritten.ts";
    ensure_script(script_uri);
    repository::delete_asset(script_uri, asset_path);

    repository::upsert_asset(test_asset(
        script_uri,
        asset_path,
        "text/plain",
        b"export const version = 1;",
    ))
    .expect("asset should store");

    repository::upsert_asset(test_asset(
        script_uri,
        asset_path,
        "text/plain",
        b"export const version = 2;",
    ))
    .expect("asset should update in place");

    let assets = repository::fetch_assets(script_uri);
    let stored = assets.get(asset_path).expect("asset should still exist");
    assert_eq!(
        String::from_utf8_lossy(&stored.content),
        "export const version = 2;",
        "an upsert by the owning script updates rather than duplicating"
    );

    repository::delete_asset(script_uri, asset_path);
}
