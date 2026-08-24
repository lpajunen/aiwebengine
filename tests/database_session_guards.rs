//! What bounds a database call the engine cannot interrupt.
//!
//! Postgres is the only party in this system that can stop one. A script's
//! execution budget is enforced by the runtime's interrupt handler, which runs
//! between JavaScript operations and cannot reach inside a host call; the
//! timeout around a request abandons the blocking thread without stopping the
//! work on it. So a statement waiting on a lock waits forever, holding whatever
//! locks it already took, with every later writer queued behind it.
//!
//! Every pooled connection now carries `lock_timeout`, `statement_timeout` and
//! `idle_in_transaction_session_timeout` from the moment it connects.

mod common;

use aiwebengine::config::{AppConfig, RepositoryConfig};
use aiwebengine::database::Database;
use common::should_skip_integration_tests;
use sqlx::Row;
use std::time::{Duration, Instant};

/// The guards a test wants: short enough to observe, long enough not to fire
/// on an unloaded scratch table.
fn guarded_config() -> RepositoryConfig {
    let mut repository = AppConfig::test_config_postgres(0).repository;
    repository.max_connections = 4;
    repository.lock_timeout_ms = 750;
    repository.statement_timeout_ms = 5_000;
    repository.idle_in_transaction_timeout_ms = 60_000;
    repository
}

async fn setting(pool: &sqlx::PgPool, name: &str) -> String {
    sqlx::query(sqlx::AssertSqlSafe(format!("SHOW {}", name)))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("SHOW {} failed: {}", name, e))
        .get::<String, _>(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pooled_connection_arrives_already_guarded() {
    if should_skip_integration_tests() {
        return;
    }
    let config = guarded_config();
    let db = Database::new(&config).await.expect("connect");

    // Carried in the startup packet, so there is no window in which a
    // connection is live but unguarded.
    assert_eq!(setting(db.pool(), "lock_timeout").await, "750ms");
    assert_eq!(setting(db.pool(), "statement_timeout").await, "5s");
    assert_eq!(
        setting(db.pool(), "idle_in_transaction_session_timeout").await,
        "1min"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_statement_gives_up_instead_of_waiting_forever() {
    if should_skip_integration_tests() {
        return;
    }
    let config = guarded_config();
    let db = Database::new(&config).await.expect("connect");

    // A second connection stands in for the handler whose transaction was
    // abandoned mid-request: it holds a table lock and never comes back.
    let holder = Database::new(&config).await.expect("connect holder");
    sqlx::query("CREATE TABLE IF NOT EXISTS session_guard_probe (id integer)")
        .execute(holder.pool())
        .await
        .expect("probe table");

    let mut abandoned = holder.pool().begin().await.expect("begin");
    sqlx::query("LOCK TABLE session_guard_probe IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *abandoned)
        .await
        .expect("take the lock");

    let started = Instant::now();
    let blocked = sqlx::query("SELECT id FROM session_guard_probe")
        .fetch_all(db.pool())
        .await;
    let waited = started.elapsed();

    let error = blocked.expect_err("a blocked read must give up, not wait");
    assert!(
        error.to_string().contains("lock timeout"),
        "expected a lock timeout, got: {}",
        error
    );
    assert!(
        waited < Duration::from_secs(5),
        "gave up after {:?}, which is not the configured 750ms",
        waited
    );

    drop(abandoned);
    sqlx::query("DROP TABLE IF EXISTS session_guard_probe")
        .execute(holder.pool())
        .await
        .expect("clean up");
}

#[tokio::test(flavor = "multi_thread")]
async fn migrations_run_unguarded_without_unguarding_the_pool() {
    if should_skip_integration_tests() {
        return;
    }
    // One connection, so the pool cannot hand back a different one and hide a
    // migration connection that was returned with its guards cleared.
    let mut config = guarded_config();
    config.max_connections = 1;
    let db = Database::new(&config).await.expect("connect");

    // Migrations are the one place a long statement and a long lock wait are
    // both expected, so they clear the guards — on a connection that is then
    // dropped rather than returned. A pooled connection that had been used for
    // migrations and handed back would run every later script unbounded.
    db.migrate().await.expect("migrate");

    assert_eq!(setting(db.pool(), "lock_timeout").await, "750ms");
    assert_eq!(setting(db.pool(), "statement_timeout").await, "5s");
}
