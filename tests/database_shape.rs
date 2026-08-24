//! The shape a `database` call answers with.
//!
//! Phase 2 gave `fetch` a response carrying `json()`; these calls answered with
//! a bare JSON string, so reading a result depended on which API produced it.
//! They now answer with the same affordances — and, because the value is a
//! `String` object rather than a plain one, every string operation written
//! against the old return keeps working to the letter.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::{TestContext, should_skip_integration_tests, wait_for_server};
use serde_json::json;
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

/// Creates a table with one row, then evaluates `source` against it.
///
/// Run on a blocking thread, as the engine runs every handler. The whole
/// evaluation is rolled back, table included.
async fn eval_with_rows(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");

    let prepared = format!(
        r#"
        database.dropTable("notes");
        database.createTable("notes");
        database.addTextColumn("notes", "label", true);
        database.insert("notes", JSON.stringify({{ label: "one" }}));
        {}
        "#,
        source
    );

    let request = EvalRequest {
        script_uri: uri.to_string(),
        source: prepared,
        user_context: UserContext::admin("database-shape".to_string()),
        timeout_ms: Some(10_000),
        rollback: true,
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_json_string_form_still_parses() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The idiom every deployed script uses. `JSON.parse` converts its argument
    // with ToString first, so the raw envelope comes back.
    let report = eval_with_rows(
        "test://db-shape/legacy",
        r#"
        const rows = JSON.parse(database.query("notes"));
        ({ count: rows.length, label: rows[0].label })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["count"], json!(1));
    assert_eq!(value["label"], json!("one"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_parses_itself() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The same affordance a `fetch` response has, so reading a result no longer
    // depends on which API produced it.
    let report = eval_with_rows(
        "test://db-shape/json",
        r#"
        const rows = database.query("notes").json();
        ({ count: rows.length, label: rows[0].label })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["count"], json!(1));
    assert_eq!(value["label"], json!("one"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_can_be_awaited() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_with_rows(
        "test://db-shape/await",
        r#"
        (async function () {
          const answer = await database.query("notes");
          const rows = answer.json();
          return { count: rows.length, label: rows[0].label, thenGone: typeof answer.then };
        })()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["count"], json!(1));
    assert_eq!(value["label"], json!("one"));
    assert_eq!(
        value["thenGone"],
        json!("undefined"),
        "the awaited value must not itself be thenable"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn string_operations_on_a_result_still_work() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The reason the value is a `String` object rather than a plain one: code
    // written against the string these calls used to return needs no change.
    let report = eval_with_rows(
        "test://db-shape/string-ops",
        r#"
        const answer = database.query("notes");
        ({
          hasLength: answer.length > 0,
          mentionsLabel: answer.indexOf("label") > 0,
          startsAsArray: answer.slice(0, 1),
          concatenates: ("rows=" + answer).slice(0, 5),
          truthy: !!answer,
        })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["hasLength"], json!(true));
    assert_eq!(value["mentionsLabel"], json!(true));
    assert_eq!(value["startsAsArray"], json!("["));
    assert_eq!(value["concatenates"], json!("rows="));
    assert_eq!(value["truthy"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn typeof_is_the_one_thing_that_changed() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Pinned rather than hidden: a `String` object reports "object". A script
    // testing the answer this way has to use `String(result)` or `.json()`.
    let report = eval_with_rows(
        "test://db-shape/typeof",
        r#"
        const answer = database.query("notes");
        ({ direct: typeof answer, coerced: typeof String(answer) })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["direct"], json!("object"));
    assert_eq!(value["coerced"], json!("string"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_answers_with_the_same_shape() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Not just `query`: every call in the namespace answers the same way.
    let report = eval_with_rows(
        "test://db-shape/write",
        r#"
        const inserted = database.insert("notes", JSON.stringify({ label: "two" }));
        const parsedOldWay = JSON.parse(inserted);
        ({
          viaJson: inserted.json().label,
          viaParse: parsedOldWay.label,
          rowsNow: database.query("notes").json().length,
        })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["viaJson"], json!("two"));
    assert_eq!(value["viaParse"], json!("two"));
    assert_eq!(value["rowsNow"], json!(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_answer_is_readable_both_ways() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // These calls report failure in the answer rather than by throwing, so the
    // failure has to survive the wrapper intact.
    let report = eval_with_rows(
        "test://db-shape/error",
        r#"
        const answer = database.query("no_such_table");
        ({ viaJson: !!answer.json().error, viaParse: !!JSON.parse(answer).error })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["viaJson"], json!(true));
    assert_eq!(value["viaParse"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_carrying_quotes_still_parses() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The envelope used to be assembled by formatting the message into a JSON
    // literal, which held up only while the message contained no JSON syntax of
    // its own. Postgres names the constraint it rejected in double quotes, so
    // `.json()` threw on exactly the errors worth reading and the only way to
    // see one was to treat the answer as a string.
    let report = eval_with_rows(
        "test://db-shape/quoted-error",
        r#"
        database.insert("notes", JSON.stringify({ label: "one" }));
        const answer = database.addUniqueIndex("notes", JSON.stringify(["label"]));
        ({ viaJson: answer.json().error, viaParse: JSON.parse(answer).error })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");

    let message = value["viaJson"].as_str().expect("an error message");
    assert_eq!(value["viaParse"].as_str(), Some(message));

    // Without this the test would keep passing if the message ever stopped
    // carrying the syntax that broke the envelope, and stop proving anything.
    assert!(
        message.contains('"'),
        "the driver's message should carry quotes of its own, got: {}",
        message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_can_be_returned_as_a_response_body() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    let context = TestContext::new();

    // A result is a `String` object, not a primitive, so the engine takes its
    // object branch when shaping the response and converts it with `toString`.
    // That conversion has to bind a receiver — `String`'s own `toString` reads
    // the value off `this` — and the result carries its own besides. Handing a
    // result straight back is common enough that it is worth its own name:
    // when it broke, the only tests that noticed were about transactions.
    let script = r#"
        function seed(context) {
          database.dropTable("passthrough");
          database.createTable("passthrough");
          database.addTextColumn("passthrough", "label", true);
          database.insert("passthrough", JSON.stringify({ label: "straight through" }));
          return { status: 200, body: "seeded" };
        }

        function rows(context) {
          return { status: 200, body: database.query("passthrough") };
        }

        function init(context) {
          routeRegistry.registerRoute("/passthrough/seed", "seed", "POST");
          routeRegistry.registerRoute("/passthrough", "rows", "GET");
          return { success: true };
        }
    "#;
    let _ = repository::upsert_script("test_db_body", script);

    let port = context
        .start_server()
        .await
        .expect("server failed to start");
    wait_for_server(port, 20).await.expect("Server not ready");
    let base = format!("http://127.0.0.1:{}", port);
    let client = reqwest::Client::new();

    assert_eq!(
        client
            .post(format!("{}/passthrough/seed", base))
            .send()
            .await
            .expect("seed failed")
            .status(),
        200
    );

    let response = client
        .get(format!("{}/passthrough", base))
        .send()
        .await
        .expect("read failed");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert!(
        body.contains("straight through"),
        "a result handed back as a body should serialise to its JSON, got: {}",
        body
    );

    context.cleanup().await.expect("Failed to cleanup");
}
