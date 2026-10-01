//! The shape a `database` call answers in.
//!
//! The calls used to answer with JSON text — and, for a while, with a `String`
//! object that could also `.json()` itself — and reported a failure as
//! `{"error": ...}` inside that text, so every caller parsed and then checked.
//! They answer with values now, and a failure throws.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::{AdminServer, setup_env, test_mutex};
use serde_json::json;

/// Creates a table with one row, then evaluates `source` against it.
///
/// Run on a blocking thread, as the engine runs every handler. The whole
/// evaluation is rolled back, table included.
async fn eval_with_rows(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");

    let prepared = format!(
        r#"
        try {{ database.dropTable("notes"); }} catch (e) {{}}
        database.ensureTable("notes", {{ columns: [{{ name: "label", type: "text", nullable: true }}] }});
        database.insert("notes", {{ label: "one" }});
        {}
        "#,
        source
    );

    let request = EvalRequest {
        timeout_ms: Some(10_000),
        rollback: true,
        ..EvalRequest::new(
            uri.to_string(),
            prepared,
            UserContext::admin("database-shape".to_string()),
        )
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

async fn value_of(uri: &str, source: &str) -> serde_json::Value {
    let report = eval_with_rows(uri, source).await;
    assert!(report.ok, "{:?}", report.outcome.error);
    report.outcome.value.expect("a value")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_answers_with_rows() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let value = value_of(
        "test://db-shape/rows",
        r#"
        const rows = database.query("notes");
        ({ isArray: Array.isArray(rows), count: rows.length, label: rows[0].label })
        "#,
    )
    .await;
    assert_eq!(
        value,
        json!({ "isArray": true, "count": 1, "label": "one" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_answers_with_the_row_and_no_success_flag() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    // A failure throws, so a `success: true` riding along with every answer
    // would be the one field nobody could ever see false.
    let value = value_of(
        "test://db-shape/write",
        r#"
        const row = database.insert("notes", { label: "two" });
        const ensured = database.ensureTable("notes", { columns: [{ name: "label", type: "text" }] });
        ({ label: row.label, hasId: typeof row.id === "number",
           ensuredHasSuccess: "success" in ensured, created: ensured.created })
        "#,
    )
    .await;
    assert_eq!(
        value,
        json!({ "label": "two", "hasId": true, "ensuredHasSuccess": false, "created": false })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_throws_with_the_driver_message_intact() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    // Postgres names the constraint it rejected in double quotes. The answer
    // used to be assembled by formatting that into a JSON literal, so the
    // errors worth reading were the ones that broke the envelope; the message
    // has to reach the script whole.
    let value = value_of(
        "test://db-shape/quoted-error",
        r#"
        database.insert("notes", { label: "one" });
        let caught = null;
        try {
            database.ensureTable("notes", { columns: [], uniqueIndexes: [["label"]] });
        } catch (e) {
            caught = { name: e.name, message: e.message };
        }
        caught
        "#,
    )
    .await;
    assert_eq!(value["name"], "Error");
    let message = value["message"].as_str().expect("an error message");
    assert!(
        message.starts_with("database.ensureTable: ") && message.contains('"'),
        "the driver's message should arrive whole, quotes included: {}",
        message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_misspelt_query_option_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    // Silently ignoring `{ limt: 5 }` would answer every row.
    let value = value_of(
        "test://db-shape/option",
        r#"
        let caught = null;
        try { database.query("notes", { limt: 5 }); } catch (e) { caught = e.name; }
        caught
        "#,
    )
    .await;
    assert_eq!(value, json!("TypeError"));
}

/// `transaction(fn)` commits what `fn` did when it returns, undoes it when it
/// throws, and inside another transaction undoes only its own work — which is
/// what the six begin/commit/rollback/savepoint calls it replaced were for,
/// without the one thing they made easy: leaving a transaction open.
#[tokio::test(flavor = "multi_thread")]
async fn a_transaction_commits_rolls_back_and_nests() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let value = value_of(
        "test://db-shape/transaction",
        r#"
        const labels = () => database.query("notes", { orderBy: "label" }).map((r) => r.label);

        const returned = database.transaction(() => {
            database.insert("notes", { label: "committed" });
            return "the answer";
        });

        let thrown = null;
        try {
            database.transaction(() => {
                database.insert("notes", { label: "rolled back" });
                throw new Error("no");
            });
        } catch (e) {
            thrown = e.message;
        }

        database.transaction(() => {
            database.insert("notes", { label: "outer" });
            try {
                database.transaction(() => {
                    database.insert("notes", { label: "inner" });
                    throw new Error("inner only");
                });
            } catch (e) {}
        });

        ({ returned, thrown, labels: labels() })
        "#,
    )
    .await;
    assert_eq!(
        value,
        json!({
            "returned": "the answer",
            "thrown": "no",
            "labels": ["committed", "one", "outer"],
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_can_be_returned_as_a_response_body() {
    let _guard = test_mutex().lock().await;
    // The script is written before the server starts, so startup registers its
    // routes — which means the database has to be up before the write.
    setup_env().await;

    // Handing a result straight back as a body is common enough to be worth
    // its own name: the rows are an array, and the engine serialises it.
    let script = r#"
        function seed(context) {
          try { database.dropTable("passthrough"); } catch (e) {}
          database.ensureTable("passthrough", { columns: [{ name: "label", type: "text", nullable: true }] });
          database.insert("passthrough", { label: "straight through" });
          return { status: 200, body: "seeded" };
        }

        function rows(context) {
          return { status: 200, body: database.query("passthrough") };
        }

        function init(context) {
          routeRegistry.registerRoute("/passthrough/seed", { handler: "seed", method: "POST" });
          routeRegistry.registerRoute("/passthrough", { handler: "rows", method: "GET" });
          return { success: true };
        }
    "#;
    let _ = repository::upsert_script("test_db_body", script);

    // Signed in: a handler that reads the script database is called by
    // somebody, and `UseScriptDatabase` belongs to a solution's users.
    let engine = AdminServer::start().await.expect("server failed to start");
    let base = format!("http://127.0.0.1:{}", engine.port());
    let client = engine.client();

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

    engine.shutdown().await;
}
