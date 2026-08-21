//! `/engine/check`: findings about a script that only the engine can produce,
//! and the isolation that makes producing them safe.

use aiwebengine::auth::AuthUser;
use aiwebengine::engine_api::{
    CheckRefusal, authorize_check, check_route, execute_native_mcp_tool,
    native_mcp_tool_descriptors,
};
use aiwebengine::repository;
use aiwebengine::script_check::{CheckReport, CheckRequest, check_blocking};
use aiwebengine::security::UserContext;
use axum::Extension;
use axum::extract::Query;
use axum::response::Response;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tokio::sync::{Mutex, OnceCell};

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// Checks run a script's `init()`, and `init()` reaches process-global state.
/// One at a time, so a test asserting that a dry run left a registry alone is
/// not reading another test's writes.
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

const INIT_BUDGET_MS: u64 = 5_000;

fn deploy(script_uri: &str, content: &str) {
    repository::upsert_script(script_uri, content).expect("script should be stored");
}

/// Store a script plus the asset modules it imports, clearing anything a
/// previous run left behind.
///
/// Asset paths key on the path alone across the whole assets table, so every
/// test in this file uses paths of its own rather than sharing names.
fn deploy_with_assets(script_uri: &str, content: &str, assets: &[(&str, &str)]) {
    deploy(script_uri, content);

    for existing in repository::fetch_assets(script_uri).keys() {
        repository::delete_asset(script_uri, existing);
    }

    let now = std::time::SystemTime::now();
    for (path, source) in assets {
        repository::upsert_asset(repository::Asset {
            uri: path.to_string(),
            name: Some(path.to_string()),
            mimetype: "text/plain".to_string(),
            content: source.as_bytes().to_vec(),
            created_at: now,
            updated_at: now,
            script_uri: script_uri.to_string(),
        })
        .expect("asset module should be stored");
    }
}

fn check(script_uri: &str) -> CheckReport {
    check_blocking(
        CheckRequest {
            script_uri: script_uri.to_string(),
            content: None,
            rollback: true,
        },
        INIT_BUDGET_MS,
    )
}

fn check_candidate(script_uri: &str, content: &str) -> CheckReport {
    check_blocking(
        CheckRequest {
            script_uri: script_uri.to_string(),
            content: Some(content.to_string()),
            rollback: true,
        },
        INIT_BUDGET_MS,
    )
}

fn codes(report: &CheckReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn message_for<'a>(report: &'a CheckReport, code: &str) -> &'a str {
    report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == code)
        .map(|diagnostic| diagnostic.message.as_str())
        .unwrap_or_else(|| panic!("expected a '{}' diagnostic, got {:?}", code, codes(report)))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_working_script_reports_its_registrations_and_no_errors() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/clean";
    deploy(
        uri,
        r#"
        function listUsers(context) { return ResponseBuilder.json({ users: [] }); }
        function init() {
            routeRegistry.registerRoute("/check-clean/users", "listUsers", "GET");
        }
        "#,
    );

    let report = check(uri);

    assert!(
        report.ok,
        "expected a clean report, got {:?}",
        report.diagnostics
    );
    assert_eq!(report.registrations.len(), 1);
    assert_eq!(report.registrations[0].name, "/check-clean/users");
    assert_eq!(
        report.registrations[0].handler.as_deref(),
        Some("listUsers")
    );

    let init = report.init.expect("init should have been reported");
    assert!(init.ran, "the script defines init()");
    assert_eq!(init.budget_ms, INIT_BUDGET_MS);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_named_but_not_defined_is_reported_before_it_can_500() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/missing-handler";
    deploy(
        uri,
        r#"
        function init() {
            routeRegistry.registerRoute("/check-missing/users", "listUsers", "GET");
        }
        "#,
    );

    let report = check(uri);

    assert!(!report.ok);
    let message = message_for(&report, "missing-handler");
    assert!(message.contains("listUsers"), "{}", message);
    assert!(message.contains("/check-missing/users"), "{}", message);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_defined_only_inside_an_imported_module_is_reported() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The failure this endpoint exists for: a thin entrypoint that registers a
    // handler name, with the function itself defined in a module. `tsc` sees a
    // perfectly good export; the engine resolves handlers as globals and finds
    // nothing.
    let uri = "test://check/delegate-in-module";
    deploy_with_assets(
        uri,
        r#"
        import { listUsers } from "./check_delegate/handlers.ts";
        function init() {
            routeRegistry.registerRoute("/check-delegate/users", "listUsers", "GET");
        }
        "#,
        &[(
            "check_delegate/handlers.ts",
            "export function listUsers(context) { return ResponseBuilder.json({}); }",
        )],
    );

    let report = check(uri);

    assert!(!report.ok, "{:?}", report.diagnostics);
    let message = message_for(&report, "missing-handler");
    assert!(message.contains("listUsers"), "{}", message);
    assert!(
        message.contains("globalThis"),
        "the fix should be spelled out: {}",
        message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_that_is_defined_as_a_global_by_the_entrypoint_passes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/delegate-assigned";
    deploy_with_assets(
        uri,
        r#"
        import { listUsers } from "./check_assigned/handlers.ts";
        globalThis.listUsers = listUsers;
        function init() {
            routeRegistry.registerRoute("/check-assigned/users", "listUsers", "GET");
        }
        "#,
        &[(
            "check_assigned/handlers.ts",
            "export function listUsers(context) { return ResponseBuilder.json({}); }",
        )],
    );

    let report = check(uri);

    assert!(report.ok, "{:?}", report.diagnostics);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_circular_asset_import_is_reported_with_the_chain_that_closes_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/cycle";
    deploy_with_assets(
        uri,
        r#"
        import { a } from "./check_cycle/a.ts";
        function init() { console.log(a); }
        "#,
        &[
            (
                "check_cycle/a.ts",
                "import { b } from \"./b.ts\";\nexport const a = 1 + b;",
            ),
            (
                "check_cycle/b.ts",
                "import { a } from \"./a.ts\";\nexport const b = 1 + a;",
            ),
        ],
    );

    let report = check(uri);

    assert!(!report.ok);
    let message = message_for(&report, "circular-import");
    assert!(
        message.contains("check_cycle/a.ts") && message.contains("check_cycle/b.ts"),
        "the chain should name both modules: {}",
        message
    );
    assert!(
        message.contains("->"),
        "the chain should be rendered as a path: {}",
        message
    );
    // A bundle that does not build has no init() to time.
    assert!(report.init.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cycle_that_closes_on_the_entrypoint_names_the_entrypoint() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/cycle-root";
    deploy_with_assets(
        uri,
        r#"
        import { helper } from "./check_root_cycle/helper.ts";
        function init() { console.log(helper); }
        "#,
        &[(
            "check_root_cycle/helper.ts",
            // Imports the root module back by its own logical path. A root
            // module cannot export, so the cycle can only be a side-effect
            // import — which is exactly the shape that used to be caught one
            // level too deep, naming the wrong module.
            "import \"../cycle-root\";\nexport const helper = 1;",
        )],
    );

    let report = check(uri);

    assert!(!report.ok, "{:?}", report.diagnostics);
    assert!(
        codes(&report).contains(&"circular-import"),
        "expected a cycle, got {:?}",
        report.diagnostics
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_with_no_init_is_told_it_registers_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/no-init";
    deploy(
        uri,
        "function handler(context) { return ResponseBuilder.json({}); }",
    );

    let report = check(uri);

    // A warning, not an error: the script is valid, it just serves nothing.
    assert!(report.ok);
    assert_eq!(codes(&report), vec!["no-init"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_init_that_throws_is_reported_with_what_it_registered_first() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/init-throws";
    deploy(
        uri,
        r#"
        function ok(context) { return ResponseBuilder.json({}); }
        function init() {
            routeRegistry.registerRoute("/check-throws/ok", "ok", "GET");
            throw new Error("setup exploded");
        }
        "#,
    );

    let report = check(uri);

    assert!(!report.ok);
    assert!(
        message_for(&report, "init-failed").contains("setup exploded"),
        "{:?}",
        report.diagnostics
    );
    // The route it managed to register before throwing is still reported: that
    // is what a deploy would install, so it is what the author needs to see.
    assert_eq!(report.registrations.len(), 1);
    assert_eq!(report.registrations[0].name, "/check-throws/ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn candidate_content_is_checked_without_being_deployed() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/candidate";
    let deployed = r#"
        function original(context) { return ResponseBuilder.json({}); }
        function init() { routeRegistry.registerRoute("/check-candidate/v1", "original", "GET"); }
        "#;
    deploy(uri, deployed);

    let report = check_candidate(
        uri,
        r#"
        function init() { routeRegistry.registerRoute("/check-candidate/v2", "replacement", "GET"); }
        "#,
    );

    // The candidate's fault is reported...
    assert!(!report.ok);
    assert!(message_for(&report, "missing-handler").contains("replacement"));
    assert_eq!(report.registrations[0].name, "/check-candidate/v2");

    // ...and the deployed script is untouched.
    assert_eq!(repository::fetch_script(uri).as_deref(), Some(deployed));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_leaves_the_graphql_registry_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/graphql-isolation";
    deploy(uri, "function init() {}");

    let report = check_candidate(
        uri,
        r#"
        function resolveThing(context) { return {}; }
        function init() {
            graphQLRegistry.registerQuery(
                "checkIsolationThing",
                "checkIsolationThing: String",
                "resolveThing",
                "external",
            );
        }
        "#,
    );

    assert!(report.ok, "{:?}", report.diagnostics);
    // The registration is reported...
    assert_eq!(report.registrations.len(), 1);
    assert_eq!(report.registrations[0].name, "checkIsolationThing");

    // ...but never reached the process-wide registry. Without this, checking a
    // candidate would replace the deployed script's resolvers with the
    // candidate's, and a broken candidate would take the live schema down.
    let registry = aiwebengine::graphql::get_registry();
    let registered = registry
        .read()
        .expect("registry lock")
        .queries
        .contains_key("checkIsolationThing");
    assert!(
        !registered,
        "a dry run must not write to the GraphQL registry"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_leaves_the_dispatcher_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/dispatcher-isolation";
    deploy(uri, "function init() {}");

    let message_type = "check.isolation.message";
    let report = check_candidate(
        uri,
        &format!(
            r#"
            function onMessage(context) {{ return "ok"; }}
            function init() {{
                dispatcher.registerListener("{}", "onMessage");
            }}
            "#,
            message_type
        ),
    );

    assert!(report.ok, "{:?}", report.diagnostics);
    assert_eq!(report.registrations.len(), 1);

    let listeners = aiwebengine::dispatcher::GLOBAL_DISPATCHER
        .get_listeners(message_type)
        .expect("listener lookup should succeed");
    assert!(
        listeners.is_empty(),
        "a dry run must not register a listener, found {:?}",
        listeners
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_leaves_the_stream_registry_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/stream-isolation";
    deploy(uri, "function init() {}");

    let path = "/check-isolation/events";
    let report = check_candidate(
        uri,
        &format!(
            r#"function init() {{ routeRegistry.registerStreamRoute("{}"); }}"#,
            path
        ),
    );

    assert!(report.ok, "{:?}", report.diagnostics);
    assert_eq!(report.registrations.len(), 1);
    assert!(
        aiwebengine::stream_registry::GLOBAL_STREAM_REGISTRY
            .get_stream_info(path)
            .is_none(),
        "a dry run must not register a stream"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_does_not_dispatch_messages_to_live_listeners() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/no-dispatch";
    deploy(uri, "function init() {}");

    // The reply is what tells the script nothing was dispatched; the point of
    // the assertion is that init() completes rather than setting the rest of
    // the engine in motion.
    let report = check_candidate(
        uri,
        r#"
        function init() {
            const reply = dispatcher.sendMessage("check.no.dispatch", "{}");
            if (reply.indexOf("dry run") === -1) {
                throw new Error("expected the dispatch to be suppressed, got: " + reply);
            }
        }
        "#,
    );

    assert!(report.ok, "{:?}", report.diagnostics);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_another_script_already_serves_is_reported_as_a_conflict() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let incumbent = "test://check/conflict-incumbent";
    let contender = "test://check/conflict-contender";
    let path = "/check-conflict/shared";

    deploy(
        incumbent,
        &format!(
            r#"
            function serve(context) {{ return ResponseBuilder.json({{}}); }}
            function init() {{ routeRegistry.registerRoute("{}", "serve", "GET"); }}
            "#,
            path
        ),
    );
    // The conflict is read from what the incumbent has *installed*, so it has to
    // be marked initialized with its registrations, the way a deploy would.
    // Listing first is what pulls the freshly stored script into the metadata
    // cache that init status is recorded against.
    repository::get_all_script_metadata().expect("metadata should list");
    let mut registrations = std::collections::HashMap::new();
    registrations.insert(
        (path.to_string(), "GET".to_string()),
        repository::RouteMetadata::simple("serve".to_string()),
    );
    repository::mark_script_initialized_with_registrations(incumbent, registrations)
        .expect("incumbent should be marked initialized");

    deploy(
        contender,
        &format!(
            r#"
            function serve(context) {{ return ResponseBuilder.json({{}}); }}
            function init() {{ routeRegistry.registerRoute("{}", "serve", "GET"); }}
            "#,
            path
        ),
    );

    let report = check(contender);

    let message = message_for(&report, "route-conflict");
    assert!(message.contains(incumbent), "{}", message);
    assert!(message.contains(path), "{}", message);
    // A conflict does not stop a deploy — one of the two simply stops answering.
    assert!(report.ok, "a conflict is a warning, not an error");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_that_is_not_deployed_needs_candidate_content() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let user = UserContext::admin("checker".to_string());
    let uri = "test://check/never-deployed";

    assert!(matches!(
        authorize_check(&user, uri, false),
        Err(CheckRefusal::NotFound)
    ));
    // With content there is nothing to be missing: checking it is a preview of
    // writing it, which this user may do.
    assert!(authorize_check(&user, uri, true).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_user_who_may_not_write_scripts_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/authz";
    deploy(uri, "function init() {}");

    assert!(matches!(
        authorize_check(&UserContext::anonymous(), uri, false),
        Err(CheckRefusal::AccessDenied)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_check_tool_is_advertised_and_dispatches_over_mcp() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let advertised = native_mcp_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == "check_script")
        .expect("check_script should be a native MCP tool");
    let schema: &Value = &advertised.input_schema;
    assert_eq!(schema["required"], json!(["uri"]));
    assert!(schema["properties"]["content"].is_object());

    let uri = "test://check/mcp";
    deploy(
        uri,
        r#"function init() { routeRegistry.registerRoute("/check-mcp/x", "absent", "GET"); }"#,
    );

    let result = execute_native_mcp_tool(
        "check_script",
        &json!({ "uri": uri }),
        &UserContext::admin("checker".to_string()),
    )
    .expect("check_script should dispatch");

    assert_eq!(result["ok"], json!(false));
    let diagnostics = result["diagnostics"]
        .as_array()
        .expect("diagnostics should be a list");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == "missing-handler"),
        "{:?}",
        diagnostics
    );
}

/// An admin caller, so the HTTP tests exercise the handler rather than its
/// authorization gate.
fn admin_extension() -> Option<Extension<AuthUser>> {
    Some(Extension(AuthUser::new(
        "checker".to_string(),
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

async fn post_check(
    query: &str,
    content_type: Option<&str>,
    body: &str,
) -> (axum::http::StatusCode, Value) {
    let mut headers = axum::http::HeaderMap::new();
    if let Some(content_type) = content_type {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            content_type.parse().expect("content type should parse"),
        );
    }

    let response = check_route(
        admin_extension(),
        headers,
        Query(serde_urlencoded::from_str(query).expect("query should parse")),
        axum::body::Bytes::from(body.to_string()),
    )
    .await;

    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_reports_diagnostics_with_a_200() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/http-report";
    deploy(
        uri,
        r#"function init() { routeRegistry.registerRoute("/check-http/x", "absent", "GET"); }"#,
    );

    let (status, body) = post_check(&format!("uri={}", uri), None, "").await;

    // A script full of errors still answers 200: the request succeeded, the
    // script did not. Callers read `ok`.
    assert_eq!(status, 200);
    assert_eq!(body["ok"], json!(false));
    assert!(body["timestamp"].is_string());
    assert!(
        body["diagnostics"]
            .as_array()
            .expect("diagnostics should be a list")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "missing-handler")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_raw_body_is_taken_as_candidate_source() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/http-raw";
    let deployed = r#"function init() { routeRegistry.registerRoute("/check-raw/v1", "serve", "GET"); }
                      function serve(context) { return ResponseBuilder.json({}); }"#;
    deploy(uri, deployed);

    let (status, body) = post_check(
        &format!("uri={}", uri),
        Some("text/plain"),
        r#"function init() { routeRegistry.registerRoute("/check-raw/v2", "gone", "GET"); }"#,
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["registrations"][0]["name"], json!("/check-raw/v2"));
    assert_eq!(repository::fetch_script(uri).as_deref(), Some(deployed));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_json_body_carries_the_uri_and_the_candidate() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/http-json";
    deploy(uri, "function init() {}");

    let request = json!({
        "uri": uri,
        "content": r#"function serve(context) { return ResponseBuilder.json({}); }
                      function init() { routeRegistry.registerRoute("/check-json/ok", "serve", "GET"); }"#,
    });
    let (status, body) = post_check("", Some("application/json"), &request.to_string()).await;

    assert_eq!(status, 200);
    assert_eq!(body["ok"], json!(true), "{}", body);
    assert_eq!(body["registrations"][0]["handler"], json!("serve"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_json_body_that_does_not_parse_is_a_400() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let (status, _) = post_check("", Some("application/json"), "{not json").await;
    assert_eq!(status, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_reports_its_missing_parameters_and_refusals() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://check/http-authz";
    deploy(uri, "function init() {}");

    let (missing, _) = post_check("", None, "").await;
    assert_eq!(missing, 400);

    let (not_found, body) = post_check("uri=test://check/http-nope", None, "").await;
    assert_eq!(not_found, 404);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("candidate source")),
        "a 404 should point at the way to check undeployed code: {}",
        body
    );

    let anonymous = check_route(
        None,
        axum::http::HeaderMap::new(),
        Query(serde_urlencoded::from_str(&format!("uri={}", uri)).expect("query should parse")),
        axum::body::Bytes::new(),
    )
    .await;
    assert_eq!(
        anonymous.status(),
        403,
        "running a script's code must not be open to anonymous callers"
    );
}
