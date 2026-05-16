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

#[test]
fn root_module_path_keeps_last_path_segment() {
    let path = module_loader::root_module_path("https://example.com/scripts/app/main.ts")
        .expect("script uri should yield root module path");
    assert_eq!(path, "main.ts");
}
