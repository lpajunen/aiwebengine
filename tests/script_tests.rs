//! Running a script's own test modules: discovery, execution, and reporting.

use aiwebengine::engine_api::{TestRunRefusal, authorize_test_run, run_tests_route};
use aiwebengine::js_engine::{TestRunParams, execute_test_run};
use aiwebengine::module_loader;
use aiwebengine::repository;
use aiwebengine::script_test::{RunOutcome, TestRunRequest, TestRunResult, TestRunner};
use aiwebengine::security::UserContext;
use axum::extract::Query;
use std::sync::OnceLock;
use tokio::sync::{Mutex, OnceCell};

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

/// Store a script with the given test modules as its assets, replacing whatever
/// was there before so a rerun of the suite starts clean.
fn script_with_test_modules(script_uri: &str, modules: &[(&str, &str)]) {
    repository::upsert_script(script_uri, "function init() {}").expect("script should be stored");

    for existing in repository::fetch_assets(script_uri).keys() {
        repository::delete_asset(script_uri, existing);
    }

    let now = std::time::SystemTime::now();
    for (path, source) in modules {
        repository::upsert_asset(repository::Asset {
            uri: path.to_string(),
            name: Some(path.to_string()),
            mimetype: "text/plain".to_string(),
            content: source.as_bytes().to_vec(),
            created_at: now,
            updated_at: now,
            script_uri: script_uri.to_string(),
        })
        .expect("test module should be stored");
    }
}

fn params(script_uri: &str) -> TestRunParams {
    TestRunParams {
        script_uri: script_uri.to_string(),
        user_context: UserContext::admin("test-runner".to_string()),
        timeout_ms: 5_000,
        run_timeout_ms: 60_000,
        filter: None,
        rollback: true,
    }
}

fn run(script_uri: &str) -> TestRunResult {
    let modules = module_loader::discover_test_modules(script_uri);
    execute_test_run(&params(script_uri), &modules)
}

fn case<'a>(result: &'a TestRunResult, name: &str) -> &'a aiwebengine::script_test::TestCaseResult {
    result
        .cases
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| {
            panic!(
                "no case named {:?}; got {:?}",
                name,
                result
                    .cases
                    .iter()
                    .map(|case| case.name.as_str())
                    .collect::<Vec<_>>()
            )
        })
}

#[tokio::test(flavor = "multi_thread")]
async fn cases_are_reported_per_file_with_their_verdicts() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-verdicts";
    script_with_test_modules(
        script_uri,
        &[
            (
                "server/basket.ts",
                "export function totalCents(items) {
                    return items.reduce((sum, item) => sum + item.cents, 0);
                }",
            ),
            (
                "tests/basket.test.ts",
                r#"
                import { totalCents } from "../server/basket.ts";

                test("an empty basket totals zero", () => {
                  expect(totalCents([])).toBe(0);
                });

                test("a basket sums its items", () => {
                  expect(totalCents([{ cents: 150 }, { cents: 50 }])).toBe(200);
                });
                "#,
            ),
            (
                "tests/broken.test.ts",
                r#"
                test("this one is wrong", () => {
                  expect(1 + 1).toBe(3);
                });
                "#,
            ),
        ],
    );

    let result = run(script_uri);

    assert_eq!(result.outcome, RunOutcome::Completed);
    assert_eq!(result.cases.len(), 3, "cases: {:?}", result.cases);
    assert_eq!(result.passed_count(), 2);
    assert_eq!(result.failed_count(), 1);
    assert!(!result.all_passed());

    let passing = case(&result, "an empty basket totals zero");
    assert!(passing.is_passed());
    assert_eq!(passing.file.as_deref(), Some("tests/basket.test.ts"));
    assert!(passing.error.is_none());

    let failing = case(&result, "this one is wrong");
    assert!(!failing.is_passed());
    assert_eq!(failing.file.as_deref(), Some("tests/broken.test.ts"));
    assert!(
        failing
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("Expected 2 to be 3"),
        "unexpected failure message: {:?}",
        failing.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn describe_qualifies_names_and_hooks_wrap_every_case() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-hooks";
    script_with_test_modules(
        script_uri,
        &[(
            "tests/hooks.test.ts",
            r#"
            let counter = 0;
            const seen = [];

            beforeEach(() => { counter += 1; });
            afterEach(() => { seen.push(counter); });

            describe("basket", () => {
              test("sees the first increment", () => {
                expect(counter).toBe(1);
              });

              test("sees the second increment", () => {
                expect(counter).toBe(2);
                expect(seen).toEqual([1]);
              });
            });
            "#,
        )],
    );

    let result = run(script_uri);

    assert!(result.all_passed(), "cases: {:?}", result.cases);
    assert_eq!(result.cases.len(), 2);
    case(&result, "basket > sees the first increment");
    case(&result, "basket > sees the second increment");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_module_that_cannot_load_is_reported_as_one_failed_case() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-broken-module";
    script_with_test_modules(
        script_uri,
        &[
            (
                "tests/missing-import.test.ts",
                r#"
                import { nothing } from "../server/does-not-exist.ts";

                test("never runs", () => {
                  expect(nothing).toBe(1);
                });
                "#,
            ),
            (
                "tests/healthy.test.ts",
                r#"
                test("still runs", () => {
                  expect(true).toBeTruthy();
                });
                "#,
            ),
        ],
    );

    let result = run(script_uri);

    assert_eq!(result.cases.len(), 2, "cases: {:?}", result.cases);
    assert!(!result.all_passed());

    // The broken file must not hide the healthy one.
    assert!(case(&result, "still runs").is_passed());

    let broken = case(&result, "tests/missing-import.test.ts");
    assert!(!broken.is_passed());
    assert_eq!(broken.file.as_deref(), Some("tests/missing-import.test.ts"));
    assert!(
        broken
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("was not found in assets"),
        "unexpected load error: {:?}",
        broken.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filter_runs_only_the_matching_cases() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-filter";
    script_with_test_modules(
        script_uri,
        &[(
            "tests/filter.test.ts",
            r#"
            test("keeps this one", () => { expect(1).toBe(1); });
            test("skips that one", () => { throw new Error("should not run"); });
            "#,
        )],
    );

    let modules = module_loader::discover_test_modules(script_uri);
    let result = execute_test_run(
        &TestRunParams {
            filter: Some("keeps".to_string()),
            ..params(script_uri)
        },
        &modules,
    );

    assert_eq!(result.cases.len(), 1, "cases: {:?}", result.cases);
    assert!(result.all_passed());
    case(&result, "keeps this one");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_async_test_body_fails_instead_of_silently_passing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-async";
    script_with_test_modules(
        script_uri,
        &[(
            "tests/async.test.ts",
            r#"
            test("returns a promise", async () => {
              expect(1).toBe(2);
            });
            "#,
        )],
    );

    let result = run(script_uri);

    let async_case = case(&result, "returns a promise");
    assert!(
        !async_case.is_passed(),
        "an async body must not report a pass it never earned"
    );
    assert!(
        async_case
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("returned a promise"),
        "unexpected error: {:?}",
        async_case.error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runaway_test_times_out_without_losing_earlier_cases() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-timeout";
    script_with_test_modules(
        script_uri,
        &[(
            "tests/runaway.test.ts",
            r#"
            test("finishes first", () => { expect(1).toBe(1); });
            test("never finishes", () => { while (true) {} });
            test("never starts", () => { expect(1).toBe(1); });
            "#,
        )],
    );

    let modules = module_loader::discover_test_modules(script_uri);
    let result = execute_test_run(
        &TestRunParams {
            timeout_ms: 500,
            ..params(script_uri)
        },
        &modules,
    );

    assert_eq!(result.outcome, RunOutcome::TimedOut);
    assert!(
        !result.all_passed(),
        "a run that never reached every case is not a pass"
    );
    assert!(
        case(&result, "finishes first").is_passed(),
        "the verdict reached before the interrupt must survive"
    );
    assert!(
        result.cases.iter().all(|case| case.name != "never starts"),
        "a case that never ran must not get a verdict"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn each_module_runs_in_its_own_context() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-isolation";
    script_with_test_modules(
        script_uri,
        &[
            (
                "tests/a-leaks.test.ts",
                r#"
                globalThis.leaked = "from a";
                test("leaks a global", () => {
                  expect(globalThis.leaked).toBe("from a");
                });
                "#,
            ),
            (
                "tests/b-observes.test.ts",
                r#"
                test("cannot see the other file's global", () => {
                  expect(globalThis.leaked).toBeUndefined();
                });
                "#,
            ),
        ],
    );

    let result = run(script_uri);

    assert!(result.all_passed(), "cases: {:?}", result.cases);
    assert_eq!(result.cases.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn database_writes_made_by_a_test_are_rolled_back() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-rollback";

    // Create the table for real: DDL inside the run would be rolled back with
    // everything else.
    script_with_test_modules(
        script_uri,
        &[(
            "tests/setup.test.ts",
            r#"
            test("prepares the table", () => {
              database.dropTable("boxes");
              const created = JSON.parse(database.createTable("boxes"));
              expect(created.success).toBeTruthy();
              const column = JSON.parse(database.addTextColumn("boxes", "label", true));
              expect(column.success).toBeTruthy();
            });
            "#,
        )],
    );
    let setup = execute_test_run(
        &TestRunParams {
            rollback: false,
            ..params(script_uri)
        },
        &module_loader::discover_test_modules(script_uri),
    );
    assert!(setup.all_passed(), "setup: {:?}", setup.cases);

    // A test writes a row and sees it, as it must for the test to be useful.
    script_with_test_modules(
        script_uri,
        &[(
            "tests/writes.test.ts",
            r#"
            test("inserts a row it can read back", () => {
              const inserted = JSON.parse(database.insert("boxes", JSON.stringify({ label: "one" })));
              expect(inserted.error).toBeUndefined();
              const rows = JSON.parse(database.query("boxes"));
              expect(rows).toHaveLength(1);
            });
            "#,
        )],
    );
    let writing = run(script_uri);
    assert!(writing.all_passed(), "writing: {:?}", writing.cases);

    // Once the run is over the row must be gone.
    script_with_test_modules(
        script_uri,
        &[(
            "tests/observes.test.ts",
            r#"
            test("sees no rows from the previous run", () => {
              const rows = JSON.parse(database.query("boxes"));
              expect(rows).toHaveLength(0);
            });
            "#,
        )],
    );
    let observing = execute_test_run(
        &TestRunParams {
            rollback: false,
            ..params(script_uri)
        },
        &module_loader::discover_test_modules(script_uri),
    );
    assert!(
        observing.all_passed(),
        "the write should not have survived the run: {:?}",
        observing.cases
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_run_ceiling_stops_further_modules_but_keeps_finished_verdicts() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-run-ceiling";
    script_with_test_modules(
        script_uri,
        &[
            (
                "tests/a-slow.test.ts",
                r#"
                test("burns the whole run budget", () => { while (true) {} });
                "#,
            ),
            (
                "tests/b-never-starts.test.ts",
                r#"
                test("never starts", () => { expect(1).toBe(1); });
                "#,
            ),
        ],
    );

    let modules = module_loader::discover_test_modules(script_uri);
    assert_eq!(modules.len(), 2, "both modules should be discovered");

    let result = execute_test_run(
        &TestRunParams {
            // The module budget alone would allow both files to run; the run
            // ceiling is what has to stop the second one.
            timeout_ms: 400,
            run_timeout_ms: 400,
            ..params(script_uri)
        },
        &modules,
    );

    assert_eq!(result.outcome, RunOutcome::TimedOut);
    assert!(
        result.cases.iter().all(|case| case.name != "never starts"),
        "the ceiling must stop the run before it starts work it cannot finish: {:?}",
        result.cases
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_script_without_test_modules_reports_no_cases_rather_than_success() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-empty";
    script_with_test_modules(
        script_uri,
        &[("server/only-code.ts", "export const x = 1;")],
    );

    let result = TestRunner::new(5_000, 30_000)
        .run(TestRunRequest {
            script_uri: script_uri.to_string(),
            user_context: UserContext::admin("test-runner".to_string()),
            filter: None,
            rollback: true,
        })
        .await;

    assert!(result.is_empty());
    assert!(
        !result.all_passed(),
        "a script with no tests has not passed anything"
    );
    assert!(result.error().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_administrator_or_owner_may_run_a_scripts_tests() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-authz";
    repository::upsert_script(script_uri, "function init() {}").expect("script should be stored");

    assert!(
        authorize_test_run(&UserContext::admin("admin".to_string()), script_uri).is_ok(),
        "an administrator may run any script's tests"
    );

    assert!(
        matches!(
            authorize_test_run(
                &UserContext::authenticated("someone-else".to_string()),
                script_uri
            ),
            Err(TestRunRefusal::AccessDenied)
        ),
        "a signed-in non-owner runs the script's code as themselves, so must be refused"
    );

    assert!(
        matches!(
            authorize_test_run(&UserContext::anonymous(), script_uri),
            Err(TestRunRefusal::AccessDenied)
        ),
        "an anonymous caller must be refused"
    );

    assert!(
        matches!(
            authorize_test_run(
                &UserContext::admin("admin".to_string()),
                "test://script-tests-does-not-exist"
            ),
            Err(TestRunRefusal::NotFound)
        ),
        "an unknown script is a 404, not a permission problem"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_maps_refusals_to_status_codes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script_uri = "test://script-tests-endpoint";
    repository::upsert_script(script_uri, "function init() {}").expect("script should be stored");

    let missing_uri = run_tests_route(
        None,
        Query(serde_urlencoded::from_str("").expect("empty query should parse")),
        axum::body::Bytes::new(),
    )
    .await;
    assert_eq!(missing_uri.status(), 400);

    let unknown_script = run_tests_route(
        None,
        Query(
            serde_urlencoded::from_str("uri=test://script-tests-nope").expect("query should parse"),
        ),
        axum::body::Bytes::new(),
    )
    .await;
    assert_eq!(unknown_script.status(), 404);

    let anonymous = run_tests_route(
        None,
        Query(
            serde_urlencoded::from_str(&format!("uri={}", script_uri)).expect("query should parse"),
        ),
        axum::body::Bytes::new(),
    )
    .await;
    assert_eq!(
        anonymous.status(),
        403,
        "running a script's code must not be open to anonymous callers"
    );
}
