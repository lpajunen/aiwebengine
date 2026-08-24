//! What an operator can see when the database stops answering.
//!
//! A wedged table used to look, from inside the engine, exactly like a slow
//! script: requests that never returned. Telling the two apart meant reaching
//! for `psql` against the production database and knowing which catalogue views
//! to join. These two facts — who is waiting on a lock, and who is in front of
//! them — are what identify a wedge, and they now come back from the health
//! check itself.

mod common;

use aiwebengine::config::{AppConfig, RepositoryConfig};
use aiwebengine::database::Database;
use aiwebengine::engine_api::lock_diagnostics;
use common::should_skip_integration_tests;
use std::time::{Duration, Instant};

/// Long enough that the blocked reader is still waiting when the diagnostic
/// runs. The point is to observe the wedge, not to survive it.
fn patient_config() -> RepositoryConfig {
    let mut repository = AppConfig::test_config_postgres(0).repository;
    repository.max_connections = 4;
    repository.lock_timeout_ms = 30_000;
    repository.statement_timeout_ms = 30_000;
    repository
}

/// Waits for `pool` to report at least one statement blocked on a lock.
async fn wait_for_a_blocked_statement(pool: &sqlx::PgPool) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let report = lock_diagnostics(pool).await;
        if report["blocked_statements"].as_u64().unwrap_or(0) > 0 {
            return report;
        }
        assert!(
            Instant::now() < deadline,
            "no blocked statement appeared: {}",
            report
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wedged_table_names_both_the_waiter_and_the_holder() {
    if should_skip_integration_tests() {
        return;
    }
    let config = patient_config();
    let observer = Database::new(&config).await.expect("connect");
    let holder = Database::new(&config).await.expect("connect holder");
    let waiter = Database::new(&config).await.expect("connect waiter");

    sqlx::query("CREATE TABLE IF NOT EXISTS lock_diagnostic_probe (id integer)")
        .execute(observer.pool())
        .await
        .expect("probe table");

    // The abandoned transaction: holds the table and never comes back.
    let mut held = holder.pool().begin().await.expect("begin");
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .expect("holder pid");
    sqlx::query("LOCK TABLE lock_diagnostic_probe IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .expect("take the lock");

    // A request that will never finish while that transaction is open.
    let waiter_pool = waiter.pool().clone();
    let blocked = tokio::spawn(async move {
        let _ = sqlx::query("SELECT id FROM lock_diagnostic_probe")
            .fetch_all(&waiter_pool)
            .await;
    });

    let report = wait_for_a_blocked_statement(observer.pool()).await;

    assert_eq!(report["available"], serde_json::json!(true));

    let waiting = report["waiting"].as_array().expect("a waiting list");
    let names_the_holder = waiting.iter().any(|entry| {
        entry["blocked_by"].as_array().is_some_and(|pids| {
            pids.iter()
                .any(|pid| pid.as_i64() == Some(holder_pid as i64))
        })
    });
    assert!(
        names_the_holder,
        "the diagnostic must name whoever is in front of the waiter, got: {}",
        report["waiting"]
    );

    let shows_the_query = waiting.iter().any(|entry| {
        entry["query"]
            .as_str()
            .is_some_and(|q| q.contains("lock_diagnostic_probe"))
    });
    assert!(
        shows_the_query,
        "the diagnostic must show what the waiter was trying to run, got: {}",
        report["waiting"]
    );

    // The holder is the oldest transaction on the server, which is the shape a
    // lock nobody can break almost always takes.
    assert!(
        report["oldest_transaction"]["age_seconds"]
            .as_f64()
            .is_some_and(|age| age >= 0.0),
        "the oldest transaction should be reported, got: {}",
        report["oldest_transaction"]
    );

    drop(held);
    let _ = blocked.await;

    // With nothing blocked, the diagnostic says so rather than going quiet.
    let calm = lock_diagnostics(observer.pool()).await;
    assert_eq!(calm["available"], serde_json::json!(true));
    assert_eq!(calm["blocked_statements"], serde_json::json!(0));

    sqlx::query("DROP TABLE IF EXISTS lock_diagnostic_probe")
        .execute(observer.pool())
        .await
        .expect("clean up");
}
