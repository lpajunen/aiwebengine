//! What a script's value is bound as, and what happens when it does not fit.
//!
//! Parameters are typed by the column, and the type is pinned in the SQL, so a
//! value can only be bound one way. Typing by the shape of the JSON would let
//! one SQL string arrive with `int8` on one call and `float8` on the next, and
//! since sqlx caches a prepared statement under the string alone, the first
//! call's types would outlive it: the same value could round to `2` on a fresh
//! connection and fail as "integer out of range" on a used one.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::{setup_env, test_mutex};
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
        // A refusal throws; these read it back as an answer.
        function attempt(f) {{
            try {{ return f(); }} catch (e) {{ return {{ error: e.message }}; }}
        }}
        // Runs `f` in a transaction and rolls it back on purpose.
        function rolledBack(f) {{
            const ROLLBACK = {{}};
            let out;
            try {{
                database.transaction(() => {{ out = f(); throw ROLLBACK; }}, {{ timeoutMs: 5000 }});
            }} catch (e) {{
                if (e !== ROLLBACK) throw e;
            }}
            return out;
        }}
        try {{ database.dropTable("readings"); }} catch (e) {{}}
        database.ensureTable("readings", {{ columns: [{{ name: "amount", type: "integer", nullable: true }}] }});
        {}
        "#,
        source
    );

    let request = EvalRequest {
        timeout_ms: Some(10_000),
        rollback: false,
        ..EvalRequest::new(
            uri.to_string(),
            prepared,
            UserContext::admin("database-binding".to_string()),
        )
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Rounding it to 2 would store a number the script never computed, and
    // hide the bug that produced 1.57 behind a plausible-looking row.
    let report = eval_against_table(
        "test://db-binding/fractional",
        r#"
        attempt(() => database.insert("readings", { amount: 1.57 }))
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Both inserts are the same SQL text on the same connection, so the
    // second must not be bound against whatever the first had the statement
    // prepared as.
    let report = eval_against_table(
        "test://db-binding/poisoning",
        r#"
        const { first, second } = rolledBack(() => ({
            first: database.insert("readings", { amount: 2 }),
            second: attempt(() => database.insert("readings", { amount: 1.57 })),
        }));
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The inconsistency that made this hard to see: the same value rounded on
    // one path and raised "integer out of range" on the other.
    let report = eval_against_table(
        "test://db-binding/consistency",
        r#"
        const outside = attempt(() => database.insert("readings", { amount: 1.57 }));
        const inside = rolledBack(() => attempt(() => database.insert("readings", { amount: 1.57 })));
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // JavaScript has one numeric type, so a whole number that has been through
    // arithmetic is a float. Refusing it would refuse ordinary integer work.
    let report = eval_against_table(
        "test://db-binding/whole-float",
        r#"
        database.insert("readings", { amount: 9 / 3 })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    assert_eq!(answer["amount"], 3, "3.0 is a whole number: {:?}", answer);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_null_reaches_a_column_that_is_not_text() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // A null is bound as the column's type, not as `text`, which Postgres
    // refuses for every column type but one.
    let report = eval_against_table(
        "test://db-binding/null",
        r#"
        database.insert("readings", { amount: null })
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The refusal is a validation error raised before anything is sent, so
    // there is no failed statement for the transaction to be poisoned by, and
    // the tick's other writes commit.
    let report = eval_against_table(
        "test://db-binding/survivable",
        r#"
        database.transaction(() => {
            database.insert("readings", { amount: 1 });
            attempt(() => database.insert("readings", { amount: 1.57 }));
            database.insert("readings", { amount: 3 });
        }, { timeoutMs: 5000 });
        database.query("readings").map(row => row.amount).sort((a, b) => a - b)
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Reads go through the same binding, so a filter carries the same
    // guarantee a write does.
    let report = eval_against_table(
        "test://db-binding/filter",
        r#"
        database.insert("readings", { amount: 5 });
        database.insert("readings", { amount: 50 });
        const above = database.query("readings", { where: { amount: { $gt: 9 } } });
        const fractional = attempt(() => database.query("readings", { where: { amount: 1.57 } }));
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Not every bad write can be caught before it is sent: a duplicate key is
    // only knowable at the server. Postgres aborts a transaction on any error,
    // so each statement is bracketed by a savepoint: the failure stops at the
    // statement that caused it rather than discarding the writes either side
    // of it.
    let report = eval_against_table(
        "test://db-binding/survives-a-real-error",
        r#"
        database.ensureTable("readings", { columns: [], uniqueIndexes: [["amount"]] });
        const duplicate = database.transaction(() => {
            database.insert("readings", { amount: 1 });
            const duplicate = attempt(() => database.insert("readings", { amount: 1 }));
            database.insert("readings", { amount: 2 });
            return duplicate;
        }, { timeoutMs: 5000 });
        ({
          duplicate: duplicate,
          amounts: database.query("readings").map(row => row.amount).sort((a, b) => a - b),
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The column type a JavaScript number has an exact home in. Storing it as
    // scaled integers or text was the alternative, and text compares
    // lexically — "10" sorts below "9" — which makes a filter quietly wrong.
    let report = eval_against_table(
        "test://db-binding/float-column",
        r#"
        database.ensureTable("readings", { columns: [{ name: "celsius", type: "float", nullable: true }] });
        database.insert("readings", { amount: 1, celsius: 21.5 });
        database.insert("readings", { amount: 2, celsius: 3.25 });
        const warm = database.query("readings", { where: { celsius: { $gt: 10 } } });
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
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // `Date.now()` is past 1.7 trillion, so an INTEGER column refuses it —
    // with the same "out of range" wording the reinterpreted-float bug used
    // to produce, which is half of why that report was hard to read.
    let report = eval_against_table(
        "test://db-binding/bigint-column",
        r#"
        database.ensureTable("readings", { columns: [{ name: "occurred_at_ms", type: "bigint", nullable: true }] });
        const now = Date.now();
        const refused = attempt(() => database.insert("readings", { amount: now }));
        const stored = database.insert("readings", { occurred_at_ms: now });
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
