//! What a script's value is bound as, and what happens when it does not fit.
//!
//! A parameter used to be typed by the shape of the JSON that carried it, so
//! one SQL string could arrive with `int8` on one call and `float8` on the
//! next. sqlx caches a prepared statement under that string alone, which meant
//! the first call's types outlived it: a float bound against a parameter
//! prepared as `int8` was reinterpreted rather than refused, and the same
//! value that rounded to `2` on a fresh connection came back as "integer out
//! of range" on a used one. Parameters are now typed by the column, and the
//! type is pinned in the SQL, so a value can only be bound one way.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::{setup_env, should_skip_integration_tests, test_mutex};
use serde_json::Value;

/// Evaluates `source` against a fresh `readings` table holding one integer
/// column.
///
/// Committed rather than rolled back: the point of several of these is what a
/// connection remembers between statements, and the eval-wide rollback would
/// hold every one of them inside a single transaction.
async fn eval_against_table(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");

    let prepared = format!(
        r#"
        database.dropTable("readings");
        database.createTable("readings");
        database.addIntegerColumn("readings", "amount", true);
        {}
        "#,
        source
    );

    let request = EvalRequest {
        script_uri: uri.to_string(),
        source: prepared,
        user_context: UserContext::admin("database-binding".to_string()),
        timeout_ms: Some(10_000),
        rollback: false,
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// The `error` an answer carries, or a panic naming what came back instead.
fn error_of(answer: &Value, what: &str) -> String {
    answer
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or_else(|| panic!("{} should have been refused, got {}", what, answer))
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fractional_value_is_refused_by_an_integer_column() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Rounding it to 2 would store a number the script never computed, and
    // hide the bug that produced 1.57 behind a plausible-looking row.
    let report = eval_against_table(
        "test://db-binding/fractional",
        r#"
        database.insert("readings", JSON.stringify({ amount: 1.57 })).json()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    let error = error_of(&answer, "1.57 in an INTEGER column");
    assert!(
        error.contains("amount") && error.contains("INTEGER"),
        "the refusal should name the column and its type: {}",
        error
    );
    assert!(
        !error.contains("out of range"),
        "1.57 is not out of range, it is not whole: {}",
        error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_integer_bind_does_not_poison_a_later_fractional_one() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The original report. Both inserts are the same SQL text on the same
    // connection, so the second one used to be bound against whatever the
    // first one had the statement prepared as.
    let report = eval_against_table(
        "test://db-binding/poisoning",
        r#"
        database.beginTransaction(5000);
        const first = database.insert("readings", JSON.stringify({ amount: 2 })).json();
        const second = database.insert("readings", JSON.stringify({ amount: 1.57 })).json();
        database.rollbackTransaction();
        ({ first: first, second: second })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    assert_eq!(
        answer["first"]["amount"], 2,
        "the whole number should have been stored"
    );
    let error = error_of(&answer["second"], "1.57 after an integer bind");
    assert!(
        error.contains("amount") && error.contains("whole number"),
        "the second insert should be refused for what is wrong with it: {}",
        error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_refusal_is_the_same_in_and_out_of_a_transaction() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The inconsistency that made this hard to see: the same value rounded on
    // one path and raised "integer out of range" on the other.
    let report = eval_against_table(
        "test://db-binding/consistency",
        r#"
        const outside = database.insert("readings", JSON.stringify({ amount: 1.57 })).json();
        database.beginTransaction(5000);
        const inside = database.insert("readings", JSON.stringify({ amount: 1.57 })).json();
        database.rollbackTransaction();
        ({ outside: outside, inside: inside })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    let outside = error_of(&answer["outside"], "1.57 outside a transaction");
    let inside = error_of(&answer["inside"], "1.57 inside a transaction");
    assert_eq!(
        outside, inside,
        "a value should be refused the same way wherever it is bound"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_whole_number_that_arrived_as_a_float_is_accepted() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // JavaScript has one numeric type, so a whole number that has been through
    // arithmetic is a float. Refusing it would refuse ordinary integer work.
    let report = eval_against_table(
        "test://db-binding/whole-float",
        r#"
        database.insert("readings", JSON.stringify({ amount: 9 / 3 })).json()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    assert_eq!(answer["amount"], 3, "3.0 is a whole number: {:?}", answer);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_null_reaches_a_column_that_is_not_text() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // A null used to be bound as `text` whatever it was going into, which
    // Postgres refused for every column type but one.
    let report = eval_against_table(
        "test://db-binding/null",
        r#"
        database.insert("readings", JSON.stringify({ amount: null })).json()
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    assert!(
        answer.get("error").is_none(),
        "a null belongs in a nullable integer column: {:?}",
        answer
    );
    assert_eq!(answer["amount"], Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_value_leaves_the_transaction_usable() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The refusal is a validation error raised before anything is sent, so
    // there is no failed statement for the transaction to be poisoned by, and
    // the tick's other writes commit.
    let report = eval_against_table(
        "test://db-binding/survivable",
        r#"
        database.beginTransaction(5000);
        database.insert("readings", JSON.stringify({ amount: 1 }));
        database.insert("readings", JSON.stringify({ amount: 1.57 }));
        database.insert("readings", JSON.stringify({ amount: 3 }));
        database.commitTransaction();
        database.query("readings").json().map(row => row.amount).sort((a, b) => a - b)
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let amounts = report.outcome.value.expect("a value");
    assert_eq!(
        amounts,
        Value::from(vec![1, 3]),
        "one bad bind should not take the tick's other writes with it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filter_is_bound_as_the_column_it_compares() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Reads go through the same binding, so a filter carries the same
    // guarantee a write does.
    let report = eval_against_table(
        "test://db-binding/filter",
        r#"
        database.insert("readings", JSON.stringify({ amount: 5 }));
        database.insert("readings", JSON.stringify({ amount: 50 }));
        const above = database.query("readings", JSON.stringify({ amount: { $gt: 9 } })).json();
        const fractional = database.query("readings", JSON.stringify({ amount: 1.57 })).json();
        ({ above: above, fractional: fractional })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    let above = answer["above"].as_array().expect("rows");
    assert_eq!(above.len(), 1, "only 50 is above 9: {:?}", above);
    assert_eq!(above[0]["amount"], 50);
    error_of(
        &answer["fractional"],
        "a fractional filter on an INTEGER column",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_statement_no_longer_takes_the_transaction_with_it() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Not every bad write can be caught before it is sent: a duplicate key is
    // only knowable at the server. Postgres aborts a transaction on any error,
    // so this used to discard the writes either side of it — an unrelated
    // tick's work included — and the script's `catch` ran with nothing left to
    // save. Each statement is bracketed by a savepoint now, so the failure
    // stops at the statement that caused it.
    let report = eval_against_table(
        "test://db-binding/survives-a-real-error",
        r#"
        database.addUniqueIndex("readings", JSON.stringify(["amount"]));
        database.beginTransaction(5000);
        database.insert("readings", JSON.stringify({ amount: 1 }));
        const duplicate = database.insert("readings", JSON.stringify({ amount: 1 })).json();
        database.insert("readings", JSON.stringify({ amount: 2 }));
        database.commitTransaction();
        ({
          duplicate: duplicate,
          amounts: database.query("readings").json().map(row => row.amount).sort((a, b) => a - b),
        })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    error_of(&answer["duplicate"], "a duplicate key");
    assert_eq!(
        answer["amounts"],
        Value::from(vec![1, 2]),
        "the writes either side of the failure should have committed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_float_column_holds_a_javascript_number() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The column type a JavaScript number has an exact home in. Storing it as
    // scaled integers or text was the alternative, and text compares
    // lexically — "10" sorts below "9" — which makes a filter quietly wrong.
    let report = eval_against_table(
        "test://db-binding/float-column",
        r#"
        database.addFloatColumn("readings", "celsius", true);
        database.insert("readings", JSON.stringify({ amount: 1, celsius: 21.5 }));
        database.insert("readings", JSON.stringify({ amount: 2, celsius: 3.25 }));
        const warm = database.query("readings", JSON.stringify({ celsius: { $gt: 10 } })).json();
        ({ stored: warm.length, celsius: warm[0].celsius })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    assert_eq!(answer["stored"], 1, "only 21.5 is above 10: {:?}", answer);
    assert_eq!(
        answer["celsius"], 21.5,
        "the value should come back as it went in, not as null: {:?}",
        answer
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bigint_column_holds_epoch_milliseconds() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // `Date.now()` is past 1.7 trillion, so an INTEGER column refuses it —
    // with the same "out of range" wording the reinterpreted-float bug used
    // to produce, which is half of why that report was hard to read.
    let report = eval_against_table(
        "test://db-binding/bigint-column",
        r#"
        database.addBigintColumn("readings", "occurred_at_ms", true);
        const now = Date.now();
        const refused = database.insert("readings", JSON.stringify({ amount: now })).json();
        const stored = database.insert("readings", JSON.stringify({ occurred_at_ms: now })).json();
        ({ refused: refused, stored: stored, now: now })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    error_of(
        &answer["refused"],
        "epoch milliseconds in an INTEGER column",
    );
    assert_eq!(
        answer["stored"]["occurred_at_ms"], answer["now"],
        "a BIGINT column should hold it exactly: {:?}",
        answer
    );
}
