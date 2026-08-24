//! Whether an execution budget reaches a call that has left JavaScript.
//!
//! The runtime's interrupt handler enforces the budget between bytecode
//! operations, so it stops a runaway loop and is blind to a call waiting on
//! Postgres — the one kind of work most likely to exceed the budget. The
//! timeout around the request is no help either: it abandons the blocking
//! thread while the work continues on it, which is why `/engine/eval` could be
//! given a timeout and still never answer.
//!
//! The budget now follows the execution across that boundary.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use common::should_skip_integration_tests;
use sqlx::Row;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OnceCell};

const SCRIPT_URI: &str = "test://host-budget/blocked";

/// The budget the blocked evaluation is given.
const BUDGET_MS: u64 = 1_000;

/// What the call would have waited for without a budget reaching it.
///
/// The session's `lock_timeout` — five seconds by default — is the next line of
/// defence down, so an answer that arrives well inside it can only have come
/// from the budget.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn test_mutex() -> &'static Mutex<()> {
    TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

async fn setup_env() -> sqlx::PgPool {
    let config = aiwebengine::config::AppConfig::test_config_postgres(0);
    let db = aiwebengine::database::Database::new(&config.repository)
        .await
        .expect("connect");
    let pool = db.pool().clone();

    INIT.get_or_init(|| async {
        let db_arc = std::sync::Arc::new(db);
        aiwebengine::database::initialize_global_database(db_arc.clone());
        repository::initialize_repository(repository::PostgresRepository::new(
            db_arc.pool().clone(),
            "test".to_string(),
        ));
    })
    .await;

    pool
}

async fn eval(source: &str, timeout_ms: u64) -> EvalReport {
    let request = EvalRequest {
        script_uri: SCRIPT_URI.to_string(),
        source: source.to_string(),
        user_context: UserContext::admin("host-budget".to_string()),
        timeout_ms: Some(timeout_ms),
        rollback: false,
    };

    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// The physical table behind a script's logical one.
async fn physical_table(pool: &sqlx::PgPool, logical: &str) -> String {
    sqlx::query(
        "SELECT physical_table_name FROM script_tables \
         WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(SCRIPT_URI)
    .bind(logical)
    .fetch_one(pool)
    .await
    .expect("the table should be registered")
    .get::<String, _>(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_call_answers_within_the_budget_it_was_given() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    let pool = setup_env().await;

    repository::upsert_script(SCRIPT_URI, "function init() {}").expect("script should be stored");

    let prepared = eval(
        r#"
        database.dropTable("held");
        database.createTable("held");
        database.addTextColumn("held", "label", true);
        database.insert("held", JSON.stringify({ label: "one" }));
        ({ ready: true })
        "#,
        10_000,
    )
    .await;
    assert!(prepared.ok, "{:?}", prepared.outcome.error);

    let table = physical_table(&pool, "held").await;

    // Stands in for the handler whose transaction was abandoned mid-request:
    // it holds the table and never comes back.
    let mut holder = pool.begin().await.expect("begin");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "LOCK TABLE {} IN ACCESS EXCLUSIVE MODE",
        table
    )))
    .execute(&mut *holder)
    .await
    .expect("take the lock");

    let started = Instant::now();
    // Read as a string: these calls answer with an error envelope rather than
    // throwing, so the evaluation itself succeeds and the verdict is in the
    // value it produced.
    let blocked = eval(r#"String(database.query("held"))"#, BUDGET_MS).await;
    let waited = started.elapsed();

    assert!(
        waited < LOCK_TIMEOUT,
        "the evaluation took {:?}, which is the lock timeout rather than its {}ms budget",
        waited,
        BUDGET_MS
    );
    assert!(blocked.ok, "{:?}", blocked.outcome.error);
    let answer = blocked
        .outcome
        .value
        .expect("a value")
        .as_str()
        .expect("a string answer")
        .to_string();
    assert!(
        answer.contains("timeout"),
        "the blocked call should report running out of budget, got: {}",
        answer
    );

    drop(holder);

    // The connection whose query was dropped mid-flight must not poison the
    // pool: the engine has to keep working once the lock is gone.
    let after = eval(r#"database.query("held").json().length"#, 10_000).await;
    assert!(after.ok, "{:?}", after.outcome.error);
    assert_eq!(after.outcome.value.expect("a value"), serde_json::json!(1));

    let _ = eval(r#"database.dropTable("held")"#, 10_000).await;
}
