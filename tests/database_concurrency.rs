//! What a transaction guarantees when another one is running.
//!
//! `beginTransaction` gives atomicity — a rolled-back write leaves nothing
//! behind — and it never gave isolation. Postgres runs scripts at READ
//! COMMITTED, where a plain `SELECT` takes no lock, so the textbook guarded
//! counter is not guarded at all:
//!
//! ```text
//! BEGIN; read seq; write seq + 1; COMMIT;   ×10 concurrently
//! expected  2 3 4 5 6 7 8 9 10 11
//! actual    2 2 2 2 2 3 3  3  3  3
//! ```
//!
//! Every one of those transactions committed successfully and five of the
//! writes were lost. That is Postgres behaving as documented, not the engine
//! misbehaving — but until `forUpdate` there was no way for a script to ask
//! for the lock that makes the pattern correct, which left a script with no
//! way to write it right.
//!
//! These tests pin both halves: unguarded still loses updates (so nobody is
//! surprised again), and guarded does not.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::should_skip_integration_tests;
use std::sync::OnceLock;
use tokio::sync::{Mutex, OnceCell};

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// Enough racers that a lost update is a certainty rather than a coin toss.
const RACERS: usize = 10;

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

fn run(uri: &str, source: &str) -> EvalReport {
    eval_blocking(EvalRequest {
        script_uri: uri.to_string(),
        source: source.to_string(),
        // Committed, not rolled back: an eval-wide rollback would hold every
        // racer inside one transaction, which is the opposite of the point.
        rollback: false,
        user_context: UserContext::admin("concurrency".to_string()),
        timeout_ms: Some(30_000),
    })
}

/// A one-row counter table belonging to `uri`, reset to `seq = 1`.
async fn fresh_counter(uri: &'static str) {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");

    tokio::task::spawn_blocking(move || {
        let _ = repository::drop_script_table(uri, "counter");
        let report = run(
            uri,
            r#"
            database.createTable("counter");
            database.addIntegerColumn("counter", "seq", false, "1");
            database.insert("counter", JSON.stringify({ seq: 1 })).json()
            "#,
        );
        assert!(
            report.ok,
            "counter setup failed: {:?}",
            report.outcome.error
        );
    })
    .await
    .expect("counter setup panicked");
}

/// Race `RACERS` copies of `source` and collect the number each one produced.
async fn race(uri: &'static str, source: &'static str) -> Vec<i64> {
    let mut racers = Vec::new();
    for _ in 0..RACERS {
        racers.push(tokio::task::spawn_blocking(move || run(uri, source)));
    }

    let mut produced = Vec::new();
    for racer in racers {
        let report = racer.await.expect("racer panicked");
        assert!(
            report.ok,
            "every racer should commit, not fail: {:?}",
            report.outcome.error
        );
        let value = report
            .outcome
            .value
            .as_ref()
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| panic!("a racer should return its number: {:?}", report.outcome));
        produced.push(value);
    }
    produced.sort_unstable();
    produced
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_guarded_read_lets_ten_transactions_each_take_the_next_number() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    const URI: &str = "test://concurrency/guarded";
    fresh_counter(URI).await;

    // `forUpdate` holds the row until this transaction commits, so the next
    // racer waits and then reads what this one wrote.
    let produced = race(
        URI,
        r#"
        database.beginTransaction(20000);
        const row = database
            .query("counter", null, 1, null, "asc", JSON.stringify({ forUpdate: true }))
            .json()[0];
        const next = row.seq + 1;
        database.update("counter", row.id, JSON.stringify({ seq: next })).json();
        database.commitTransaction();
        next
        "#,
    )
    .await;

    let expected: Vec<i64> = (2..=(RACERS as i64 + 1)).collect();
    assert_eq!(
        produced, expected,
        "each racer should take a distinct number; a repeat is a lost update"
    );

    // And the row agrees with the last number handed out.
    let final_seq = tokio::task::spawn_blocking(|| {
        run(URI, r#"database.query("counter").json()[0].seq"#)
            .outcome
            .value
            .and_then(|v| v.as_i64())
    })
    .await
    .expect("read-back panicked");
    assert_eq!(final_seq, Some(RACERS as i64 + 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_unguarded_read_still_loses_updates() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    const URI: &str = "test://concurrency/unguarded";
    fresh_counter(URI).await;

    // Not a wish, a warning. This is what READ COMMITTED does with an
    // unguarded read-modify-write, and a script that writes one gets it. The
    // test exists so the behaviour is recorded rather than rediscovered, and
    // so that anything which accidentally makes plain reads block shows up
    // here as a surprise rather than as a slowdown nobody can place.
    let produced = race(
        URI,
        r#"
        database.beginTransaction(20000);
        const row = database.query("counter").json()[0];
        const next = row.seq + 1;
        database.update("counter", row.id, JSON.stringify({ seq: next })).json();
        database.commitTransaction();
        next
        "#,
    )
    .await;

    let distinct: std::collections::BTreeSet<i64> = produced.iter().copied().collect();
    assert!(
        distinct.len() < RACERS,
        "an unguarded counter is expected to collide; got {} distinct values from {}: {:?}",
        distinct.len(),
        RACERS,
        produced
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn for_update_outside_a_transaction_is_refused() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    const URI: &str = "test://concurrency/no-transaction";
    fresh_counter(URI).await;

    // A lock taken outside a transaction is released the moment the statement
    // finishes, so honouring the request would hand back a query that reads
    // like a guarded one and is not.
    let report = tokio::task::spawn_blocking(|| {
        run(
            URI,
            r#"
            database
                .query("counter", null, 1, null, "asc", JSON.stringify({ forUpdate: true }))
                .json()
            "#,
        )
    })
    .await
    .expect("eval panicked");

    let answer = report.outcome.value.expect("a value");
    let error = answer
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or_else(|| panic!("forUpdate without a transaction should be refused: {answer}"));
    assert!(
        error.contains("beginTransaction"),
        "the refusal should say what to do about it: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_misspelled_option_is_refused_rather_than_ignored() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    const URI: &str = "test://concurrency/typo";
    fresh_counter(URI).await;

    // Silently dropping an unrecognised option would hand back an unguarded
    // query to a caller who asked for a guarded one — the exact failure the
    // option exists to prevent, arriving without a word.
    let report = tokio::task::spawn_blocking(|| {
        run(
            URI,
            r#"
            database.beginTransaction(5000);
            const answer = database
                .query("counter", null, 1, null, "asc", JSON.stringify({ forupdate: true }))
                .json();
            database.rollbackTransaction();
            answer
            "#,
        )
    })
    .await
    .expect("eval panicked");

    let answer = report.outcome.value.expect("a value");
    let error = answer
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or_else(|| panic!("a misspelled option should be refused: {answer}"));
    assert!(
        error.contains("forupdate") && error.contains("forUpdate"),
        "the refusal should name what was passed and what is supported: {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_skipped_positional_argument_may_be_written_as_null() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    const URI: &str = "test://concurrency/null-args";
    fresh_counter(URI).await;

    // The documented calling convention — `query(table, null, 100, "ts",
    // "desc")` — used to raise a type error naming a conversion the script
    // never asked for. There is no way to reach a later argument without it.
    let report = tokio::task::spawn_blocking(|| {
        run(
            URI,
            r#"
            const all = database.query("counter", null, 10).json();
            const ordered = database.query("counter", null, 10, "seq", "desc").json();
            const skipped = database.query("counter", null, null, null, null).json();
            ({ all: all.length, ordered: ordered.length, skipped: skipped.length })
            "#,
        )
    })
    .await
    .expect("eval panicked");

    assert!(report.ok, "{:?}", report.outcome.error);
    let answer = report.outcome.value.expect("a value");
    for key in ["all", "ordered", "skipped"] {
        assert_eq!(
            answer.get(key).and_then(|v| v.as_i64()),
            Some(1),
            "null should skip an argument, not fail the call: {answer}"
        );
    }
}
