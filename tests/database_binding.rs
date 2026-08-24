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
use common::should_skip_integration_tests;
use serde_json::Value;
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
