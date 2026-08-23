//! `/engine/eval`: running a snippet against a deployed script's sandbox.

use aiwebengine::auth::AuthUser;
use aiwebengine::engine_api::{
    CheckRefusal, authorize_eval, eval_route, execute_native_mcp_tool, native_mcp_tool_descriptors,
};
use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use axum::Extension;
use axum::extract::Query;
use axum::response::Response;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tokio::sync::{Mutex, OnceCell};

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// Evaluations share the process-global repository and script caches, so they
/// run one at a time.
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

fn deploy(script_uri: &str, content: &str) {
    repository::upsert_script(script_uri, content).expect("script should be stored");
}

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

fn eval(script_uri: &str, source: &str) -> EvalReport {
    eval_with(script_uri, source, true)
}

fn eval_with(script_uri: &str, source: &str, rollback: bool) -> EvalReport {
    eval_blocking(EvalRequest {
        script_uri: script_uri.to_string(),
        source: source.to_string(),
        user_context: UserContext::admin("evaluator".to_string()),
        timeout_ms: None,
        rollback,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_returns_its_value_and_its_type() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/value";
    deploy(uri, "function init() {}");

    let report = eval(uri, "1 + 40");
    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(41)));
    assert_eq!(report.outcome.value_type.as_deref(), Some("number"));

    let object = eval(uri, r#"({ a: 1, b: ["x"] })"#);
    assert_eq!(object.outcome.value, Some(json!({"a": 1, "b": ["x"]})));
}

#[tokio::test(flavor = "multi_thread")]
async fn undefined_and_null_are_told_apart() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/nullish";
    deploy(uri, "function init() {}");

    // Both have no useful JSON form, so `value` alone cannot distinguish them —
    // which is exactly why the type is always reported.
    let undefined = eval(uri, "undefined");
    assert!(undefined.ok);
    assert_eq!(undefined.outcome.value, None);
    assert_eq!(undefined.outcome.value_type.as_deref(), Some("undefined"));

    let null = eval(uri, "null");
    assert!(null.ok);
    assert_eq!(null.outcome.value, Some(json!(null)));
    assert_eq!(null.outcome.value_type.as_deref(), Some("null"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_can_call_the_scripts_own_functions() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/own-functions";
    deploy(
        uri,
        r#"
        function totalCents(items) { return items.reduce((sum, item) => sum + item.cents, 0); }
        function init() {}
        "#,
    );

    let report = eval(uri, "totalCents([{ cents: 300 }, { cents: 45 }])");
    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(345)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_sees_the_bindings_the_entrypoint_imported() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The linker rewrites the entrypoint's imports into top-level declarations,
    // and the program is compiled with JS_EVAL_TYPE_GLOBAL — so they land in
    // the realm and a later eval in the same context can reach them. This test
    // is what holds that mechanism in place.
    let uri = "test://eval/imported-bindings";
    deploy_with_assets(
        uri,
        r#"
        import { greet } from "./eval_modules/greeter.ts";
        function init() {}
        "#,
        &[(
            "eval_modules/greeter.ts",
            "export function greet(name) { return \"hello \" + name; }",
        )],
    );

    let report = eval(uri, r#"greet("world")"#);
    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!("hello world")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_can_reach_a_module_the_entrypoint_never_imported() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/module-require";
    deploy_with_assets(
        uri,
        r#"
        import { used } from "./eval_reach/used.ts";
        function init() { console.log(used); }
        "#,
        &[
            ("eval_reach/used.ts", "export const used = 1;"),
            (
                "eval_reach/hidden.ts",
                "export function secretly(n) { return n * 2; }",
            ),
        ],
    );

    // `hidden.ts` is not imported by the entrypoint, so it is not in the
    // bundle's module table at all — reaching it must fail, and say so.
    let missing = eval(uri, r#"__asset_module_require__("eval_reach/hidden.ts")"#);
    assert!(!missing.ok);
    assert!(
        missing
            .outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Unknown asset module")),
        "{:?}",
        missing.outcome.error
    );

    // A module the entrypoint *did* import is reachable through the same door,
    // which is the affordance worth documenting.
    let reachable = eval(
        uri,
        r#"__asset_module_require__("eval_reach/used.ts").used"#,
    );
    assert!(reachable.ok, "{:?}", reachable.outcome.error);
    assert_eq!(reachable.outcome.value, Some(json!(1)));
}

#[tokio::test(flavor = "multi_thread")]
async fn console_output_is_captured_and_survives_the_rollback() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/console";
    deploy(uri, "function init() {}");

    // The point of capturing at all: console writes go through the repository,
    // so they join the run's transaction and vanish with it. Capture is what
    // keeps the output the caller asked for.
    let report = eval(
        uri,
        r#"
        console.log("first");
        console.warn("second");
        "done";
        "#,
    );

    assert!(report.ok, "{:?}", report.outcome.error);
    assert!(report.outcome.rolled_back);
    assert_eq!(report.outcome.value, Some(json!("done")));

    let lines: Vec<(&str, &str)> = report
        .outcome
        .console
        .iter()
        .map(|line| (line.level.as_str(), line.message.as_str()))
        .collect();
    assert_eq!(lines, vec![("LOG", "first"), ("WARN", "second")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn database_writes_roll_back_by_default() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/rollback";
    deploy(uri, "function init() {}");

    let key = "eval-rollback-probe";
    let written = eval(
        uri,
        &format!(
            r#"sharedStorage.setItem("{}", "written"); sharedStorage.getItem("{}");"#,
            key, key
        ),
    );
    assert!(written.ok, "{:?}", written.outcome.error);
    // Inside the run the write is visible...
    assert_eq!(written.outcome.value, Some(json!("written")));
    assert!(written.outcome.rolled_back);

    // ...and gone once it ends.
    let after = eval(uri, &format!(r#"sharedStorage.getItem("{}")"#, key));
    assert!(after.ok, "{:?}", after.outcome.error);
    assert_ne!(
        after.outcome.value,
        Some(json!("written")),
        "the write should not have outlived the evaluation"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn registrations_made_by_a_snippet_do_not_take_effect() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/registrations";
    deploy(uri, "function init() {}");

    let path = "/eval-registration/never";
    let report = eval(
        uri,
        &format!(
            r#"routeRegistry.registerStreamRoute("{}"); "registered";"#,
            path
        ),
    );

    assert!(report.ok, "{:?}", report.outcome.error);
    assert!(
        aiwebengine::stream_registry::GLOBAL_STREAM_REGISTRY
            .get_stream_info(path)
            .is_none(),
        "a registration made from a snippet must not outlive it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_that_throws_is_reported_rather_than_hidden() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/throws";
    deploy(uri, "function init() {}");

    let report = eval(uri, r#"throw new Error("deliberate");"#);
    assert!(!report.ok);
    assert!(
        report
            .outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("deliberate")),
        "{:?}",
        report.outcome.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_that_awaits_reports_the_settled_value() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/promise";
    deploy(uri, "function init() {}");

    // A resolved promise is reported as the value it resolved to, not as the
    // opaque object the caller did not mean to ask for.
    let report = eval(uri, "Promise.resolve(1)");
    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(1)));
    assert_eq!(report.outcome.value_type.as_deref(), Some("number"));

    // Awaits resume, so the type reported is the awaited value's.
    let awaited = eval(
        uri,
        "(async () => { const n = await 20; return n + 22; })()",
    );
    assert!(awaited.ok, "{:?}", awaited.outcome.error);
    assert_eq!(awaited.outcome.value, Some(json!(42)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_returning_an_unsettleable_promise_is_told_so() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/pending";
    deploy(uri, "function init() {}");

    // Nothing can settle this: there are no timers, and every host call has
    // already returned by the time the queue is drained.
    let report = eval(uri, "new Promise(() => {})");
    assert!(!report.ok);
    let error = report.outcome.error.as_deref().unwrap_or_default();
    assert!(error.contains("never settled"), "{}", error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_snippet_reports_the_rejection() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/rejected";
    deploy(uri, "function init() {}");

    let report = eval(uri, "(async () => { throw new Error('boom'); })()");
    assert!(!report.ok);
    let error = report.outcome.error.as_deref().unwrap_or_default();
    assert!(error.contains("boom"), "{}", error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_that_cannot_be_serialized_is_reported_without_failing_the_run() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/circular";
    deploy(uri, "function init() {}");

    let report = eval(uri, "const a = {}; a.self = a; a;");
    // The snippet ran fine — only the value could not be rendered, and that is
    // reported separately from an error.
    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, None);
    assert_eq!(report.outcome.value_type.as_deref(), Some("object"));
    assert!(
        report.outcome.value_error.is_some(),
        "a circular structure should be explained"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runaway_snippet_is_stopped_by_the_budget() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/runaway";
    deploy(uri, "function init() {}");

    let started = std::time::Instant::now();
    let report = eval_blocking(EvalRequest {
        script_uri: uri.to_string(),
        source: "while (true) {}".to_string(),
        user_context: UserContext::admin("evaluator".to_string()),
        timeout_ms: Some(300),
        rollback: true,
    });

    assert!(!report.ok);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "the interrupt should fire near the budget, took {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_requested_budget_cannot_exceed_the_engines_own() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/clamp";
    deploy(uri, "function init() {}");

    let ceiling = aiwebengine::script_eval::default_eval_timeout_ms();
    let started = std::time::Instant::now();
    let report = eval_blocking(EvalRequest {
        script_uri: uri.to_string(),
        source: "while (true) {}".to_string(),
        user_context: UserContext::admin("evaluator".to_string()),
        // Asking for an hour must not buy an hour of a blocking thread.
        timeout_ms: Some(3_600_000),
        rollback: true,
    });

    assert!(!report.ok);
    assert!(
        started.elapsed() < std::time::Duration::from_millis(ceiling + 10_000),
        "the request should have been clamped to {}ms, took {:?}",
        ceiling,
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn evaluation_is_refused_without_the_right_to_change_the_script() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/authz";
    deploy(uri, "function init() {}");

    assert!(matches!(
        authorize_eval(&UserContext::anonymous(), uri),
        Err(CheckRefusal::AccessDenied)
    ));
    assert!(matches!(
        authorize_eval(
            &UserContext::admin("evaluator".to_string()),
            "test://eval/never-deployed"
        ),
        Err(CheckRefusal::NotFound)
    ));
    assert!(authorize_eval(&UserContext::admin("evaluator".to_string()), uri).is_ok());
}

fn admin_extension() -> Option<Extension<AuthUser>> {
    Some(Extension(AuthUser::new(
        "evaluator".to_string(),
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

async fn post_eval(
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

    let response = eval_route(
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
async fn the_endpoint_takes_a_raw_snippet_body() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/http-raw";
    deploy(
        uri,
        "function double(n) { return n * 2; } function init() {}",
    );

    let (status, body) = post_eval(&format!("uri={}", uri), None, "double(21)").await;

    assert_eq!(status, 200);
    assert_eq!(body["ok"], json!(true), "{}", body);
    assert_eq!(body["value"], json!(42));
    assert!(body["timestamp"].is_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_takes_a_json_envelope() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/http-json";
    deploy(uri, "function init() {}");

    let request = json!({ "uri": uri, "source": r#"console.log("hi"); 7;"# });
    let (status, body) = post_eval("", Some("application/json"), &request.to_string()).await;

    assert_eq!(status, 200);
    assert_eq!(body["value"], json!(7));
    assert_eq!(body["console"][0]["message"], json!("hi"));
    assert_eq!(body["console"][0]["level"], json!("LOG"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_that_throws_still_answers_200() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/http-throws";
    deploy(uri, "function init() {}");

    let (status, body) = post_eval(&format!("uri={}", uri), None, "nope()").await;

    // The request succeeded; the snippet did not. Callers read `ok`.
    assert_eq!(status, 200);
    assert_eq!(body["ok"], json!(false));
    assert!(body["error"].is_string());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_reports_its_missing_parameters_and_refusals() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/http-authz";
    deploy(uri, "function init() {}");

    let (missing_uri, _) = post_eval("", None, "1").await;
    assert_eq!(missing_uri, 400);

    let (missing_source, _) = post_eval(&format!("uri={}", uri), None, "   ").await;
    assert_eq!(
        missing_source, 400,
        "a blank snippet is a missing parameter, not an empty program"
    );

    let (not_found, _) = post_eval("uri=test://eval/http-nope", None, "1").await;
    assert_eq!(not_found, 404);

    let anonymous = eval_route(
        None,
        axum::http::HeaderMap::new(),
        Query(serde_urlencoded::from_str(&format!("uri={}", uri)).expect("query should parse")),
        axum::body::Bytes::from("1"),
    )
    .await;
    assert_eq!(anonymous.status(), 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_eval_tool_is_advertised_and_dispatches_over_mcp() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let advertised = native_mcp_tool_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.name == "eval_script")
        .expect("eval_script should be a native MCP tool");
    assert_eq!(
        advertised.input_schema["required"],
        json!(["uri", "source"])
    );

    let uri = "test://eval/mcp";
    deploy(uri, "function answer() { return 42; } function init() {}");

    let result = execute_native_mcp_tool(
        "eval_script",
        &json!({ "uri": uri, "source": "answer()" }),
        &UserContext::admin("evaluator".to_string()),
    )
    .expect("eval_script should dispatch");

    assert_eq!(result["ok"], json!(true), "{}", result);
    assert_eq!(result["value"], json!(42));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_can_import_a_module_the_entrypoint_imports() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/import-direct";
    deploy_with_assets(
        uri,
        r#"
        import { total } from "./eval_import/basket.ts";
        function init() { console.log(total([])); }
        "#,
        &[(
            "eval_import/basket.ts",
            "export function total(items) { return items.reduce((n, i) => n + i.cents, 0); }",
        )],
    );

    let report = eval(
        uri,
        r#"
        import { total } from "./eval_import/basket.ts";
        total([{ cents: 300 }, { cents: 45 }]);
        "#,
    );

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(345)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_one_line_snippet_import_works() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The shape that actually gets typed into a request body.
    let uri = "test://eval/import-oneline";
    deploy_with_assets(
        uri,
        r#"
        import { double } from "./eval_oneline/math.ts";
        function init() { console.log(double(1)); }
        "#,
        &[(
            "eval_oneline/math.ts",
            "export function double(n) { return n * 2; }",
        )],
    );

    let report = eval(
        uri,
        r#"import { double } from "./eval_oneline/math.ts"; double(21)"#,
    );

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(42)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_can_import_a_module_only_reached_through_another() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The entrypoint never names `deep.ts`; it reaches it through `mid.ts`.
    // The bundle is the transitive closure, so a snippet can import it — which
    // is what makes "importable" mean "part of the running application" rather
    // than "named by the entrypoint".
    let uri = "test://eval/import-transitive";
    deploy_with_assets(
        uri,
        r#"
        import { mid } from "./eval_deep/mid.ts";
        function init() { console.log(mid()); }
        "#,
        &[
            (
                "eval_deep/mid.ts",
                "import { deep } from \"./deep.ts\";\nexport function mid() { return deep() + 1; }",
            ),
            ("eval_deep/deep.ts", "export function deep() { return 41; }"),
        ],
    );

    let report = eval(uri, r#"import { deep } from "./eval_deep/deep.ts"; deep()"#);

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(41)));
}

#[tokio::test(flavor = "multi_thread")]
async fn importing_a_module_outside_the_graph_says_what_is_importable() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/import-missing";
    deploy_with_assets(
        uri,
        r#"
        import { used } from "./eval_missing/used.ts";
        function init() { console.log(used); }
        "#,
        &[
            ("eval_missing/used.ts", "export const used = 1;"),
            (
                "eval_missing/orphan.ts",
                "export function orphan() { return 1; }",
            ),
        ],
    );

    let report = eval(
        uri,
        r#"import { orphan } from "./eval_missing/orphan.ts"; orphan()"#,
    );

    assert!(!report.ok);
    let error = report.outcome.error.as_deref().unwrap_or_default();
    assert!(error.contains("eval_missing/orphan.ts"), "{}", error);
    assert!(error.contains("module graph"), "{}", error);
    // Naming what *is* importable turns a typo into a one-line fix.
    assert!(error.contains("eval_missing/used.ts"), "{}", error);
}

#[tokio::test(flavor = "multi_thread")]
async fn require_is_available_as_an_alias() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/require-alias";
    deploy_with_assets(
        uri,
        r#"
        import { value } from "./eval_alias/mod.ts";
        function init() { console.log(value); }
        "#,
        &[("eval_alias/mod.ts", "export const value = 7;")],
    );

    let report = eval(uri, r#"require("eval_alias/mod.ts").value"#);
    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(7)));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupported_import_form_names_the_ones_that_work() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/import-namespace";
    deploy(uri, "function init() {}");

    let report = eval(uri, r#"import * as everything from "./m.ts"; everything.x"#);

    assert!(!report.ok);
    let error = report.outcome.error.as_deref().unwrap_or_default();
    assert!(error.contains("import { a, b }"), "{}", error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snippet_cannot_export() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://eval/export";
    deploy(uri, "function init() {}");

    let report = eval(uri, "export const x = 1;");
    assert!(!report.ok);
    assert!(
        report
            .outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("export syntax")),
        "{:?}",
        report.outcome.error
    );
}
