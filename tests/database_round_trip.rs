//! What a column gives back is the type it was declared.
//!
//! A row used to be decoded by trying each Rust type in turn and keeping the
//! first that succeeded. That works only where the wire format carries the
//! column's own type — Postgres does, so a boolean fails the integer attempt
//! and falls through to the right one. A backend that stores a boolean as 0 or
//! 1 does not: the integer attempt succeeds first, and `true` comes back to
//! the script as `1`. Nothing errors and nothing is logged.
//!
//! So these are not Postgres tests. They are the contract every backend has to
//! meet, written against the one that exists — what a script stores is what it
//! reads back, in the type it declared the column with.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::{setup_env, should_skip_integration_tests, test_mutex};
use serde_json::Value;

/// Evaluate `source` against a fresh, empty table named `probe`.
///
/// The columns are the test's to declare — which types a table has is most of
/// what these tests are about.
async fn eval_with_probe_table(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");

    let prepared = format!(
        r#"
        database.dropTable("probe");
        database.createTable("probe");
        {}
        "#,
        source
    );

    let request = EvalRequest {
        script_uri: uri.to_string(),
        source: prepared,
        user_context: UserContext::admin("database-round-trip".to_string()),
        timeout_ms: Some(10_000),
        rollback: false,
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// The value an eval produced, or a panic naming why there was not one.
fn value_of(report: &EvalReport) -> &Value {
    assert!(report.ok, "eval failed: {:?}", report.outcome.error);
    report
        .outcome
        .value
        .as_ref()
        .expect("the eval should have produced a value")
}

#[tokio::test(flavor = "multi_thread")]
async fn every_column_type_reads_back_as_the_type_it_was_declared() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval_with_probe_table(
        "test://db-round-trip/types",
        r#"
        database.addIntegerColumn("probe", "count", true);
        database.addBigintColumn("probe", "at_ms", true);
        database.addFloatColumn("probe", "celsius", true);
        database.addTextColumn("probe", "label", true);
        database.addBooleanColumn("probe", "active", true);
        database.addTimestampColumn("probe", "seen_at", true);

        database.insert("probe", JSON.stringify({
            count: 7,
            at_ms: 1700000000000,
            celsius: 21.5,
            label: "hello",
            active: true,
            seen_at: "2024-03-01T12:30:00Z",
        })).json();

        database.query("probe").json()[0]
        "#,
    )
    .await;

    let row = value_of(&report);

    assert_eq!(row.get("count"), Some(&Value::from(7)));
    assert_eq!(row.get("at_ms"), Some(&Value::from(1_700_000_000_000i64)));
    assert_eq!(row.get("celsius"), Some(&Value::from(21.5)));
    assert_eq!(row.get("label"), Some(&Value::from("hello")));

    // The one a type-affinity backend gets wrong by default: stored as 0 or 1,
    // decoded as a number, and handed to a script that wrote a boolean.
    assert_eq!(
        row.get("active"),
        Some(&Value::Bool(true)),
        "a BOOLEAN column must read back as a boolean, not as a number: {}",
        row
    );

    let seen_at = row
        .get("seen_at")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("seen_at should be a string: {}", row));
    assert!(
        seen_at.starts_with("2024-03-01T12:30:00"),
        "a TIMESTAMP column reads back as the instant it was given, in ISO 8601: {}",
        seen_at
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_false_is_not_mistaken_for_a_zero() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // `false` and `0` are the same byte in some backends. A script that stores
    // `false` and reads `0` gets a value that is still falsy, so the bug
    // survives every truthiness check and surfaces somewhere far away — in
    // `JSON.stringify`, in a strict equality, in a GraphQL Boolean field.
    let report = eval_with_probe_table(
        "test://db-round-trip/false",
        r#"
        database.addBooleanColumn("probe", "active", true);
        database.insert("probe", JSON.stringify({ active: false })).json();
        database.query("probe").json()[0]
        "#,
    )
    .await;

    let row = value_of(&report);
    assert_eq!(
        row.get("active"),
        Some(&Value::Bool(false)),
        "false must survive the round trip as false: {}",
        row
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_null_reads_back_as_null_in_every_column_type() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Decoding by declared type must not turn an absent value into that type's
    // zero — a null integer is not 0 and a null boolean is not false.
    let report = eval_with_probe_table(
        "test://db-round-trip/nulls",
        r#"
        database.addIntegerColumn("probe", "count", true);
        database.addBigintColumn("probe", "at_ms", true);
        database.addFloatColumn("probe", "celsius", true);
        database.addTextColumn("probe", "label", true);
        database.addBooleanColumn("probe", "active", true);
        database.addTimestampColumn("probe", "seen_at", true);

        database.insert("probe", JSON.stringify({ label: "only this one" })).json();
        database.query("probe").json()[0]
        "#,
    )
    .await;

    let row = value_of(&report);
    for column in ["count", "at_ms", "celsius", "active", "seen_at"] {
        assert_eq!(
            row.get(column),
            Some(&Value::Null),
            "{} was never written, so it must read back as null: {}",
            column,
            row
        );
    }
    assert_eq!(row.get("label"), Some(&Value::from("only this one")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_answers_with_the_same_row_a_query_returns() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // `insert`, `update` and `upsert` each answer with the row they wrote, and
    // all four paths decode through the same place. If one of them decoded
    // differently, a script would see a value change type by being read a
    // second time.
    let report = eval_with_probe_table(
        "test://db-round-trip/write-answers",
        r#"
        database.addTextColumn("probe", "label", true);
        database.addBooleanColumn("probe", "active", true);
        database.addUniqueIndex("probe", JSON.stringify(["label"]));

        const inserted = database
            .insert("probe", JSON.stringify({ label: "a", active: true }))
            .json();
        const updated = database
            .update("probe", inserted.id, JSON.stringify({ active: false }))
            .json();
        const upserted = database
            .upsert("probe", JSON.stringify(["label"]),
                    JSON.stringify({ label: "a", active: true }))
            .json();
        const queried = database.query("probe").json()[0];

        ({ inserted: inserted, updated: updated, upserted: upserted, queried: queried })
        "#,
    )
    .await;

    let answer = value_of(&report);

    assert_eq!(answer.pointer("/inserted/active"), Some(&Value::Bool(true)));
    assert_eq!(answer.pointer("/updated/active"), Some(&Value::Bool(false)));
    assert_eq!(answer.pointer("/upserted/active"), Some(&Value::Bool(true)));
    assert_eq!(answer.pointer("/queried/active"), Some(&Value::Bool(true)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_default_reaches_the_column_as_the_value_it_names() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Defaults are carried as values now rather than as SQL text, so what the
    // script wrote and what the column holds have to still agree — including
    // an apostrophe, which is where quoting a value by hand goes wrong.
    let report = eval_with_probe_table(
        "test://db-round-trip/defaults",
        r#"
        database.addTextColumn("probe", "marker", true);
        database.addTextColumn("probe", "label", false, "it's default");
        database.addIntegerColumn("probe", "count", false, "7");
        database.addBooleanColumn("probe", "active", false, "true");
        database.addFloatColumn("probe", "celsius", false, "21.5");

        // A row naming only the column with no default, so every other value
        // in it is the one the default put there.
        database.insert("probe", JSON.stringify({ marker: "x" })).json();
        database.query("probe").json()[0]
        "#,
    )
    .await;

    let row = value_of(&report);
    assert_eq!(row.get("label"), Some(&Value::from("it's default")));
    assert_eq!(row.get("count"), Some(&Value::from(7)));
    assert_eq!(row.get("active"), Some(&Value::Bool(true)));
    assert_eq!(row.get("celsius"), Some(&Value::from(21.5)));
}

#[tokio::test(flavor = "multi_thread")]
async fn now_is_accepted_by_either_name() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // A script cannot know which backend it is talking to, so both spellings
    // of "when the row is written" mean the same thing and neither reaches the
    // database as the script typed it.
    let report = eval_with_probe_table(
        "test://db-round-trip/now",
        r#"
        database.addTextColumn("probe", "marker", true);
        database.addTimestampColumn("probe", "made_at", false, "NOW()");
        database.addTimestampColumn("probe", "seen_at", false, "CURRENT_TIMESTAMP");

        database.insert("probe", JSON.stringify({ marker: "x" })).json();
        database.query("probe").json()[0]
        "#,
    )
    .await;

    let row = value_of(&report);
    for column in ["made_at", "seen_at"] {
        let written = row
            .get(column)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{} should be an ISO 8601 string: {}", column, row));
        assert!(
            chrono::DateTime::parse_from_rfc3339(written).is_ok(),
            "{} should hold the moment the row was written: {}",
            column,
            written
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timestamp_default_that_names_no_instant_is_refused() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // This used to be forwarded to the database quoted, so what counted as a
    // time was whatever that database's parser happened to take. A default is
    // read back by whichever engine holds the table later, so the set of
    // strings that mean a time has to be the engine's own.
    let report = eval_with_probe_table(
        "test://db-round-trip/bad-default",
        r#"
        database.addTimestampColumn("probe", "seen_at", false, "whenever").json()
        "#,
    )
    .await;

    let answer = value_of(&report);
    let error = answer
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("'whenever' should have been refused, got {}", answer));
    assert!(
        error.contains("whenever"),
        "the refusal should name the value it refused: {}",
        error
    );
}
