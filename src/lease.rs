//! Keeping a worker's claim on a row alive for as long as it is working on it.
//!
//! Two tables are worked the same way — `scheduler_jobs` and `script_tasks`.
//! A worker claims a due row by stamping `locked_by` and `lock_expires_at`,
//! runs it, and writes the outcome under a guard naming its claim. The
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
//!
//! # Fencing
//!
//! `locked_by` names an instance, and an instance can claim the same row twice:
//! its claim lapses, its own next tick takes the row again, and two runs on one
//! instance both match `locked_by = <me>`. So every claim increments the row's
//! `claim_epoch`, and everything written under the claim — renewal, success,
//! failure — is guarded by the epoch it was claimed with. A run whose claim has
//! moved on writes nothing, whoever took it.
//!
//! The guard covers the row. It cannot cover what the run does meanwhile, so a
//! run also stops itself: the renewal raises a [`Fence`] once it finds the
//! claim gone, or can no longer show it is held before it would lapse, and the
//! runtime's interrupt handler ends the script at its next operation. A host
//! call already in flight finishes first. The epoch is handed to the handler as
//! its fencing token, for work outside the engine to check.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// row's id, `$3` the epoch the claim was taken with.
    const fn renew_statement(self) -> &'static str {
        match self {
            Leased::SchedulerJob => {
                r#"
                UPDATE scheduler_jobs
                SET lock_expires_at = NOW() + make_interval(secs => $1),
                    updated_at = NOW()
                WHERE job_id = $2 AND claim_epoch = $3
                "#
            }
            Leased::ScriptTask => {
                r#"
                UPDATE script_tasks
                SET lock_expires_at = NOW() + make_interval(secs => $1),
                    updated_at = NOW()
                WHERE task_id = $2 AND claim_epoch = $3
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

/// Raised once a run can no longer show it holds its claim.
///
/// Read by the runtime's interrupt handler, so a script whose claim is gone
/// stops rather than running beside whoever holds it now.
#[derive(Debug, Clone, Default)]
pub struct Fence(Arc<AtomicBool>);

impl Fence {
    pub fn is_raised(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn raise(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// A claim being kept alive. Dropping it stops the renewal.
pub struct Lease {
    renewal: JoinHandle<()>,
    fence: Fence,
}

impl Lease {
    /// What the run checks to know it still holds the claim.
    pub fn fence(&self) -> Fence {
        self.fence.clone()
    }

    /// Whether the renewal has stopped — on its own, because the claim is gone.
    pub fn is_finished(&self) -> bool {
        self.renewal.is_finished()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.renewal.abort();
    }
}

/// Keep the claim taken with `epoch` on one row alive until the returned
/// [`Lease`] is dropped.
///
/// `label` is what the row is called in a log line — a job's key, a task's
/// handler — since an id alone tells a reader nothing.
///
/// The renewal names the epoch, so once the row has been claimed again the
/// update stops matching and the renewal raises the fence and stops, rather
/// than taking the row back. Matching nothing also covers a row that is not
/// there at all: there is no claim to extend.
///
/// A renewal that fails is retried at the next tick, as long as the claim
/// would outlast it. Once it would not, nothing this worker knows shows the
/// claim is still held, so the fence is raised then — before the lease can
/// lapse and another worker take the row.
pub fn spawn_renewal(leased: Leased, id: Uuid, epoch: i64, label: String) -> Lease {
    let fence = Fence::default();
    let raised = fence.clone();
    let renewal = tokio::spawn(async move {
        let lease = StdDuration::from_secs(TTL_SECONDS.unsigned_abs());
        let margin = StdDuration::from_secs(RENEW_EVERY_SECONDS);
        let mut held_until = tokio::time::Instant::now() + lease;

        let mut ticker = tokio::time::interval(StdDuration::from_secs(RENEW_EVERY_SECONDS));
        // The first tick is immediate; the claim was just taken, so skip it.
        ticker.tick().await;

        loop {
            ticker.tick().await;

            let Some(db) = crate::database::get_global_database() else {
                raised.raise();
                return;
            };

            let sent = tokio::time::Instant::now();
            let renewed = sqlx::query(leased.renew_statement())
                .bind(TTL_SECONDS)
                .bind(id)
                .bind(epoch)
                .execute(db.pool())
                .await;

            match renewed {
                Ok(result) if result.rows_affected() == 0 => {
                    warn!(
                        row = %label,
                        epoch,
                        "{} has been claimed again or removed; stopping this run",
                        leased.noun()
                    );
                    raised.raise();
                    return;
                }
                Ok(_) => held_until = sent + lease,
                Err(e) => {
                    warn!(
                        row = %label,
                        error = %e,
                        "Failed renewing the claim on a {}",
                        leased.noun()
                    );
                    if tokio::time::Instant::now() + margin >= held_until {
                        warn!(
                            row = %label,
                            epoch,
                            "Cannot show the claim on a {} is held before it lapses; stopping this run",
                            leased.noun()
                        );
                        raised.raise();
                        return;
                    }
                }
            }
        }
    });
    Lease { renewal, fence }
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

    /// Every variant renews by naming the claim's epoch. Without that clause a
    /// renewal would take back a row whose claim had moved on, which is the
    /// failure the lease exists to prevent.
    #[test]
    fn every_renewal_is_scoped_to_the_claim_it_was_taken_with() {
        for leased in [Leased::SchedulerJob, Leased::ScriptTask] {
            let statement = leased.renew_statement();
            assert!(
                statement.contains("claim_epoch = $3"),
                "{} renews without checking which claim it holds",
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
