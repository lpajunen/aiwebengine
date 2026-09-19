//! Keeping a worker's claim on a row alive for as long as it is working on it.
//!
//! Two tables are worked the same way — `scheduler_jobs` and `script_tasks`.
//! A worker claims a due row by stamping `locked_by` and `lock_expires_at`,
//! runs it, and writes the outcome under `WHERE ... AND locked_by = <me>`. The
//! guard is what stops two instances finishing the same row, and it is exactly
//! what breaks when the claim lapses mid-run: another instance re-claims the
//! row, the guard stops matching, and the worker that actually did the work
//! writes nothing — reporting success, having changed no rows, so the failure
//! is silent and the work is done twice.
//!
//! The lease is therefore renewed rather than sized. Sizing it means predicting
//! how long the work takes, which cannot be done: a run's own budget is
//! bounded, but the wait for an execution slot happens after the claim and is
//! bounded by nothing. A renewed lease needs no prediction, and it stays short,
//! so a worker that dies returns its rows in one lease rather than one budget.
//!
//! One implementation rather than one per table, because the second hand-rolled
//! copy is where the two drift apart.

use std::time::Duration as StdDuration;

use tokio::task::JoinHandle;
use tracing::warn;
use uuid::Uuid;

/// How long a claim is good for before another worker may take the row.
pub const TTL_SECONDS: i64 = 30;

/// How often a claim is extended while the work is in flight.
///
/// A third of the lease, so two renewals can be missed — a slow query, a busy
/// runtime — before anything else believes the row abandoned.
pub const RENEW_EVERY_SECONDS: u64 = 10;

/// What a lease is held against.
///
/// Each variant carries its whole statement as a literal rather than a table
/// name to interpolate. Building the SQL by `format!` would work and would be
/// safe — the names are compile-time constants — but sqlx refuses a query it
/// cannot see as a literal, and it is right to: a shared helper is exactly
/// where a caller-supplied name would eventually be threaded through. Two
/// literals cost a line each and close the question.
#[derive(Debug, Clone, Copy)]
pub enum Leased {
    SchedulerJob,
    ScriptTask,
}

impl Leased {
    /// Extend this row's lease. `$1` is the new length in seconds, `$2` the
    /// row's id, `$3` the worker that must still hold the claim.
    const fn renew_statement(self) -> &'static str {
        match self {
            Leased::SchedulerJob => {
                r#"
                UPDATE scheduler_jobs
                SET lock_expires_at = NOW() + make_interval(secs => $1),
                    updated_at = NOW()
                WHERE job_id = $2 AND locked_by = $3
                "#
            }
            Leased::ScriptTask => {
                r#"
                UPDATE script_tasks
                SET lock_expires_at = NOW() + make_interval(secs => $1),
                    updated_at = NOW()
                WHERE task_id = $2 AND locked_by = $3
                "#
            }
        }
    }

    const fn noun(self) -> &'static str {
        match self {
            Leased::SchedulerJob => "scheduler job",
            Leased::ScriptTask => "script task",
        }
    }
}

/// Extend `worker_id`'s claim on one row until the returned handle is aborted.
///
/// `label` is what the row is called in a log line — a job's key, a task's
/// handler — since an id alone tells a reader nothing.
///
/// The renewal names the worker, so once the claim has genuinely moved on the
/// update stops matching and the task stops rather than taking the row back.
/// Matching nothing also covers a row that is not there at all, and stopping is
/// right for that too: there is no claim to extend.
pub fn spawn_renewal(leased: Leased, id: Uuid, worker_id: String, label: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(StdDuration::from_secs(RENEW_EVERY_SECONDS));
        // The first tick is immediate; the claim was just taken, so skip it.
        ticker.tick().await;

        loop {
            ticker.tick().await;

            let Some(db) = crate::database::get_global_database() else {
                return;
            };

            let renewed = sqlx::query(leased.renew_statement())
                .bind(TTL_SECONDS)
                .bind(id)
                .bind(&worker_id)
                .execute(db.pool())
                .await;

            match renewed {
                Ok(result) if result.rows_affected() == 0 => {
                    warn!(
                        row = %label,
                        "{} has no claim held by this worker; stopping renewal",
                        leased.noun()
                    );
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        row = %label,
                        error = %e,
                        "Failed renewing the claim on a {}",
                        leased.noun()
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A claim has to be renewed several times over before it lapses, so a slow
    /// query or a busy runtime costs a renewal rather than the row.
    #[test]
    fn a_claim_is_renewed_well_inside_its_lease() {
        assert!(
            (RENEW_EVERY_SECONDS as i64) * 3 <= TTL_SECONDS,
            "renewing every {}s does not leave room to miss one inside a {}s lease",
            RENEW_EVERY_SECONDS,
            TTL_SECONDS
        );
    }

    /// Every variant renews by naming the worker. Without that clause a
    /// renewal would take back a row whose claim had moved on, which is the
    /// failure the lease exists to prevent.
    #[test]
    fn every_renewal_is_scoped_to_the_worker_holding_the_claim() {
        for leased in [Leased::SchedulerJob, Leased::ScriptTask] {
            let statement = leased.renew_statement();
            assert!(
                statement.contains("locked_by = $3"),
                "{} renews without checking who holds the claim",
                leased.noun()
            );
            assert!(
                statement.contains("lock_expires_at = NOW()"),
                "{} does not push its lease out",
                leased.noun()
            );
        }
    }
}
