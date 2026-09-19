use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use sqlx::{Postgres, Row, pool::PoolConnection};
use tokio::sync::{Notify, oneshot};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{js_engine, repository};

static GLOBAL_SCHEDULER: OnceLock<Arc<Scheduler>> = OnceLock::new();
pub const MIN_RECURRING_INTERVAL_MS: i64 = 100;
/// Longest a scheduled job's name may be. One to this many characters: a name
/// is a key a later `registerOnce` replaces by, so an empty one names nothing
/// and an unbounded one is a column this engine would have to widen.
pub const MAX_JOB_NAME_CHARS: usize = 64;
const DB_CLAIM_BATCH_SIZE: i64 = 32;

/// How long a claim on a job is good for, and how often it is extended while
/// the job runs. Both live in [`crate::lease`], which holds the reasoning and
/// the one implementation `script_tasks` shares.
use crate::lease::TTL_SECONDS as DB_LOCK_TTL_SECONDS;

/// How many times a one-off job is run before the engine stops trying.
///
/// A one-off used to retry every two seconds for as long as the engine was up,
/// so a job failing on something that would never change — a handler that no
/// longer exists, a payload the script cannot parse — was an endless loop
/// writing an endless log. Five attempts over a few minutes is enough to ride
/// out a restarting dependency and short enough that a genuine failure stops
/// being one.
const MAX_ONE_OFF_ATTEMPTS: i32 = 5;

/// The first retry delay; each attempt after that waits twice as long.
const DB_ONE_OFF_RETRY_DELAY_SECONDS: i64 = 2;

/// The ceiling on that doubling, so the last attempts are still minutes rather
/// than hours apart.
const DB_ONE_OFF_MAX_RETRY_DELAY_SECONDS: i64 = 300;

/// The budget for one scheduled handler, from configuration, set once at
/// startup.
static CONFIGURED_JOB_TIMEOUT_MS: OnceLock<u64> = OnceLock::new();

/// Record the configured job budget (`javascript.job_timeout_ms`, falling back
/// to `javascript.execution_timeout_ms`). Returns false if already set.
pub fn configure_job_timeout(timeout_ms: u64) -> bool {
    CONFIGURED_JOB_TIMEOUT_MS.set(timeout_ms).is_ok()
}

/// The job budget in effect.
///
/// Read by the two things that have to agree about it: the runtime a handler
/// executes in, and the lock that stops a second instance claiming the job
/// while it is still running. They are the same number because a claim shorter
/// than the run is what let a completed job be run twice.
pub fn configured_job_timeout_ms() -> u64 {
    CONFIGURED_JOB_TIMEOUT_MS
        .get()
        .copied()
        .unwrap_or_else(|| crate::js_engine::current_execution_limits().timeout_ms)
}

/// How long to wait before attempt number `attempts + 1` of a one-off job.
///
/// Doubling rather than flat, because the two things a retry rides out want
/// different waits: a dependency that is restarting wants seconds, and one that
/// is down wants the engine to stop asking every two seconds until it is back.
fn one_off_retry_delay(attempts: i32) -> Duration {
    let doublings = attempts.saturating_sub(1).clamp(0, 16) as u32;
    let seconds = DB_ONE_OFF_RETRY_DELAY_SECONDS
        .saturating_mul(1i64 << doublings)
        .min(DB_ONE_OFF_MAX_RETRY_DELAY_SECONDS);
    Duration::seconds(seconds)
}

/// Errors returned by scheduler operations
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("handler name is required")]
    MissingHandler,
    #[error("runAt must be a UTC timestamp ending with 'Z'")]
    InvalidTimestamp,
    #[error("scheduled time must be in the future")]
    TimeInPast,
    #[error("recurring interval must be >= 100 milliseconds")]
    InvalidInterval,
    #[error("job name must be 1-64 characters")]
    InvalidJobName,
}

/// Schedule variants supported by the worker
#[derive(Debug, Clone)]
pub enum ScheduleKind {
    OneOff {
        run_at: DateTime<Utc>,
    },
    Recurring {
        interval: Duration,
        next_run: DateTime<Utc>,
    },
}

impl ScheduleKind {
    pub fn next_run(&self) -> DateTime<Utc> {
        match self {
            ScheduleKind::OneOff { run_at } => *run_at,
            ScheduleKind::Recurring { next_run, .. } => *next_run,
        }
    }
}

/// Stored job definition
#[derive(Debug, Clone)]
pub struct ScheduledJob {
    pub id: Uuid,
    pub key: String,
    pub script_uri: String,
    pub handler_name: String,
    pub schedule: ScheduleKind,
    pub created_at: DateTime<Utc>,
}

impl ScheduledJob {
    fn new(script_uri: &str, handler_name: &str, key: String, schedule: ScheduleKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            key,
            script_uri: script_uri.to_string(),
            handler_name: handler_name.to_string(),
            schedule,
            created_at: Utc::now(),
        }
    }
}

/// Snapshot passed to the JS runtime for execution context
#[derive(Debug, Clone)]
pub struct ScheduledInvocation {
    pub job_id: Uuid,
    /// Identifies this one run of the job, as distinct from `job_id`, which is
    /// the same for every run. Both the engine's own lines about the run and
    /// the lines the job itself logs are filed under it, so a single tick can
    /// be pulled out of a busy script's log.
    pub invocation_id: String,
    pub key: String,
    pub script_uri: String,
    pub handler_name: String,
    pub kind: ScheduledInvocationKind,
    pub scheduled_for: DateTime<Utc>,
    pub interval_seconds: Option<i64>,
    pub interval_milliseconds: Option<i64>,
    /// How many times this job has already been run and failed. Zero on a
    /// first run, and the basis for both the retry delay and the decision to
    /// stop retrying.
    pub attempts: i32,
}

#[derive(Debug, Clone, Copy)]
pub enum ScheduledInvocationKind {
    OneOff,
    Recurring,
}

impl ScheduledInvocationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScheduledInvocationKind::OneOff => "one-off",
            ScheduledInvocationKind::Recurring => "recurring",
        }
    }
}

/// In-memory scheduler registry with background worker
#[derive(Debug)]
pub struct Scheduler {
    jobs_by_script: Mutex<HashMap<String, Vec<ScheduledJob>>>,
    wake_signal: Notify,
    worker_id: String,
}

enum JobLockGuard {
    Pg {
        conn: PoolConnection<Postgres>,
        lock_key: String,
    },
    Noop,
}

enum JobLockAcquireError {
    HeldByOther,
    Unavailable,
}

impl JobLockGuard {
    async fn release(self) {
        match self {
            JobLockGuard::Noop => {}
            JobLockGuard::Pg { mut conn, lock_key } => {
                match sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock(hashtext($1))")
                    .bind(&lock_key)
                    .fetch_one(&mut *conn)
                    .await
                {
                    Ok(released) => {
                        if released {
                            debug!("Released advisory lock for job: {}", lock_key);
                        } else {
                            warn!(
                                "Failed to release advisory lock for job: {} (was not held)",
                                lock_key
                            );
                        }
                    }
                    Err(e) => {
                        error!("Error releasing advisory lock for job {}: {}", lock_key, e);
                    }
                }
            }
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            jobs_by_script: Mutex::new(HashMap::new()),
            wake_signal: Notify::new(),
            worker_id: format!("scheduler:{}", Uuid::new_v4()),
        }
    }

    fn has_database() -> bool {
        crate::database::get_global_database().is_some()
    }

    fn run_db_blocking<F, Fut, T>(future_factory: F) -> Option<T>
    where
        F: FnOnce(std::sync::Arc<crate::database::Database>) -> Fut,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let db = crate::database::get_global_database()?;

        let handle = match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle,
            Err(_) => return None,
        };

        Some(tokio::task::block_in_place(move || {
            handle.block_on(future_factory(db))
        }))
    }

    fn persist_job_in_db(
        &self,
        id: Uuid,
        key: &str,
        script_uri: &str,
        handler_name: &str,
        schedule: &ScheduleKind,
    ) -> bool {
        let kind = match schedule {
            ScheduleKind::OneOff { .. } => "one_off",
            ScheduleKind::Recurring { .. } => "recurring",
        };

        let run_at = schedule.next_run();
        let interval_ms = match schedule {
            ScheduleKind::OneOff { .. } => None,
            ScheduleKind::Recurring { interval, .. } => Some(interval.num_milliseconds()),
        };
        let key = key.to_string();
        let script_uri = script_uri.to_string();
        let handler_name = handler_name.to_string();
        let kind = kind.to_string();

        let persisted = Self::run_db_blocking(move |db| async move {
            let result = sqlx::query(
                    r#"
                    INSERT INTO scheduler_jobs (job_id, script_uri, handler_name, job_key, kind, run_at, interval_ms)
                    VALUES ($1, $2, $3, $4, $5, $6, $7)
                    ON CONFLICT (script_uri, job_key)
                    DO UPDATE SET
                        job_id = EXCLUDED.job_id,
                        handler_name = EXCLUDED.handler_name,
                        kind = EXCLUDED.kind,
                        run_at = EXCLUDED.run_at,
                        interval_ms = EXCLUDED.interval_ms,
                        locked_by = NULL,
                        locked_at = NULL,
                        lock_expires_at = NULL,
                        -- Registering again is a new piece of work that reuses
                        -- the key, not a continuation of the one that failed.
                        attempts = 0,
                        updated_at = NOW()
                    "#,
                )
                .bind(id)
                .bind(&script_uri)
                .bind(&handler_name)
                .bind(&key)
                .bind(&kind)
                .bind(run_at)
                .bind(interval_ms)
                .execute(db.pool())
                .await;

            match result {
                Ok(_) => true,
                Err(e) => {
                    warn!(
                        script = %script_uri,
                        handler = %handler_name,
                        job = %key,
                        error = %e,
                        "Failed to persist scheduler job in database"
                    );
                    false
                }
            }
        });

        persisted.unwrap_or(false)
    }

    /// Register a one-off job
    pub fn register_one_off(
        &self,
        script_uri: &str,
        handler_name: &str,
        key: Option<String>,
        run_at: DateTime<Utc>,
    ) -> Result<ScheduledJob, SchedulerError> {
        if handler_name.trim().is_empty() {
            return Err(SchedulerError::MissingHandler);
        }

        if run_at <= Utc::now() {
            return Err(SchedulerError::TimeInPast);
        }

        let key = Self::normalize_key(handler_name, key)?;
        let job = ScheduledJob::new(
            script_uri,
            handler_name,
            key.clone(),
            ScheduleKind::OneOff { run_at },
        );

        let persisted = self.persist_job_in_db(
            job.id,
            &job.key,
            &job.script_uri,
            &job.handler_name,
            &job.schedule,
        );

        if !persisted {
            let mut guard = self.lock_jobs();
            Self::remove_job_with_key(guard.entry(script_uri.to_string()).or_default(), &key);
            guard
                .entry(script_uri.to_string())
                .or_default()
                .push(job.clone());
            drop(guard);
        }

        self.wake_signal.notify_waiters();
        Ok(job)
    }

    /// Register a recurring job
    pub fn register_recurring(
        &self,
        script_uri: &str,
        handler_name: &str,
        key: Option<String>,
        interval: Duration,
        first_run: Option<DateTime<Utc>>,
    ) -> Result<ScheduledJob, SchedulerError> {
        if handler_name.trim().is_empty() {
            return Err(SchedulerError::MissingHandler);
        }

        if interval.num_milliseconds() < MIN_RECURRING_INTERVAL_MS {
            return Err(SchedulerError::InvalidInterval);
        }

        let mut next_run = first_run.unwrap_or_else(|| Utc::now() + interval);
        if next_run <= Utc::now() {
            next_run = Utc::now() + interval;
        }

        let key = Self::normalize_key(handler_name, key)?;
        let job = ScheduledJob::new(
            script_uri,
            handler_name,
            key.clone(),
            ScheduleKind::Recurring { interval, next_run },
        );

        let persisted = self.persist_job_in_db(
            job.id,
            &job.key,
            &job.script_uri,
            &job.handler_name,
            &job.schedule,
        );

        if !persisted {
            let mut guard = self.lock_jobs();
            Self::remove_job_with_key(guard.entry(script_uri.to_string()).or_default(), &key);
            guard
                .entry(script_uri.to_string())
                .or_default()
                .push(job.clone());
            drop(guard);
        }

        self.wake_signal.notify_waiters();
        Ok(job)
    }

    /// Delete a script's persisted jobs.
    async fn delete_jobs_in_db(script_uri: &str) -> usize {
        let Some(db) = crate::database::get_global_database() else {
            return 0;
        };
        match sqlx::query("DELETE FROM scheduler_jobs WHERE script_uri = $1")
            .bind(script_uri)
            .execute(db.pool())
            .await
        {
            Ok(result) => result.rows_affected() as usize,
            Err(e) => {
                warn!(script = %script_uri, error = %e, "Failed clearing scheduler jobs from DB");
                0
            }
        }
    }

    /// Drop a script's jobs from the in-memory table and report the total.
    fn forget_jobs(&self, script_uri: &str, removed_db: usize) -> usize {
        let mut guard = self.lock_jobs();
        let removed = guard.remove(script_uri).map(|v| v.len()).unwrap_or(0);
        let total_removed = removed + removed_db;
        if total_removed > 0 {
            debug!(
                script_uri,
                removed_memory = removed,
                removed_db,
                "Cleared scheduled jobs for script"
            );
        }
        total_removed
    }

    /// Remove all jobs for a script (returns number removed).
    ///
    /// For callers that are genuinely blocking — a script calling
    /// `schedulerService.clearAll` from inside the sandbox, or the synchronous
    /// repository wrapper. An async caller wants [`Scheduler::clear_script_async`]
    /// instead: see what it says about why.
    pub fn clear_script(&self, script_uri: &str) -> usize {
        let removed_db = Self::run_db_blocking({
            let script_uri = script_uri.to_string();
            move |_db| async move { Self::delete_jobs_in_db(&script_uri).await }
        })
        .unwrap_or(0);

        self.forget_jobs(script_uri, removed_db)
    }

    /// [`Scheduler::clear_script`] for callers that are already async.
    ///
    /// Not a convenience. The blocking form reaches the database through
    /// `block_in_place(|| handle.block_on(..))`, which drives the query on a
    /// thread parked outside the runtime while the readiness that would wake
    /// it has to come from the runtime's IO driver. Startup calls this once per
    /// script with `init_concurrency()` inits in flight, and enough of them
    /// inside `block_in_place` at once leaves nothing driving the driver —
    /// every one of them then waits forever, which is a hang with no CPU, no
    /// database activity and nothing in the logs.
    ///
    /// Awaiting the query has none of that shape.
    pub async fn clear_script_async(&self, script_uri: &str) -> usize {
        let removed_db = Self::delete_jobs_in_db(script_uri).await;
        self.forget_jobs(script_uri, removed_db)
    }

    /// Get job counts per script for monitoring
    pub fn get_job_counts(&self) -> HashMap<String, usize> {
        let guard = self.lock_jobs();
        let mut counts = HashMap::new();
        for (script_uri, jobs_vec) in guard.iter() {
            counts.insert(script_uri.clone(), jobs_vec.len());
        }
        counts
    }

    fn normalize_key(handler_name: &str, key: Option<String>) -> Result<String, SchedulerError> {
        let chosen = key.unwrap_or_else(|| handler_name.to_string());
        let trimmed = chosen.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_JOB_NAME_CHARS {
            return Err(SchedulerError::InvalidJobName);
        }
        Ok(trimmed.to_string())
    }

    fn remove_job_with_key(jobs: &mut Vec<ScheduledJob>, key: &str) {
        if let Some(index) = jobs.iter().position(|job| job.key == key) {
            jobs.remove(index);
        }
    }

    fn lock_jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<ScheduledJob>>> {
        match self.jobs_by_script.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("Scheduler state mutex poisoned; recovering");
                poisoned.into_inner()
            }
        }
    }

    fn collect_due_jobs(&self, now: DateTime<Utc>) -> Vec<ScheduledInvocation> {
        let mut guard = self.lock_jobs();
        let mut due = Vec::new();

        guard.retain(|_script, jobs| {
            let mut idx = 0;
            while idx < jobs.len() {
                let mut remove_job = false;
                let mut pending_invocation: Option<ScheduledInvocation> = None;
                let snapshot = jobs[idx].clone();

                {
                    let job = &mut jobs[idx];
                    match &mut job.schedule {
                        ScheduleKind::OneOff { run_at } => {
                            if *run_at <= now {
                                pending_invocation =
                                    Some(Self::build_invocation(&snapshot, *run_at));
                                remove_job = true;
                            }
                        }
                        ScheduleKind::Recurring { interval, next_run } => {
                            if *next_run <= now {
                                let scheduled_for = *next_run;
                                let mut invocation =
                                    Self::build_invocation(&snapshot, scheduled_for);
                                invocation.kind = ScheduledInvocationKind::Recurring;
                                invocation.interval_seconds = Some(interval.num_seconds());
                                invocation.interval_milliseconds =
                                    Some(interval.num_milliseconds());

                                // Advance next_run so we don't rerun immediately
                                let mut upcoming = *next_run + *interval;
                                while upcoming <= now {
                                    upcoming += *interval;
                                }
                                *next_run = upcoming;
                                pending_invocation = Some(invocation);
                            }
                        }
                    }
                }

                if let Some(invocation) = pending_invocation {
                    due.push(invocation);
                }

                if remove_job {
                    jobs.remove(idx);
                } else {
                    idx += 1;
                }
            }
            !jobs.is_empty()
        });

        due
    }

    fn next_trigger_at(&self) -> Option<DateTime<Utc>> {
        let guard = self.lock_jobs();
        guard
            .values()
            .flat_map(|jobs| jobs.iter().map(|job| job.schedule.next_run()))
            .min()
    }

    fn build_invocation(job: &ScheduledJob, scheduled_for: DateTime<Utc>) -> ScheduledInvocation {
        ScheduledInvocation {
            job_id: job.id,
            invocation_id: crate::middleware::generate_request_id(),
            key: job.key.clone(),
            script_uri: job.script_uri.clone(),
            handler_name: job.handler_name.clone(),
            kind: ScheduledInvocationKind::OneOff,
            scheduled_for,
            interval_seconds: None,
            interval_milliseconds: None,
            // Nothing counts attempts on this path. It is the in-memory
            // table, reached with no database or when the row could not be
            // written, and it drops a one-off as it comes due rather than
            // retrying it at all.
            attempts: 0,
        }
    }

    /// File a line the engine writes about a run under the same invocation the
    /// job's own output is filed under.
    fn invocation_log_context(invocation: &ScheduledInvocation) -> repository::LogContext {
        js_engine::HandlerInvocationKind::Scheduled.log_context(
            &invocation.script_uri,
            invocation.invocation_id.clone(),
            Some(invocation.key.clone()),
        )
    }

    /// Generate a unique lock key for a job
    /// Uses the job's script URI, handler name, and key to create a deterministic lock ID
    fn generate_lock_key(invocation: &ScheduledInvocation) -> String {
        format!(
            "{}:{}:{}",
            invocation.script_uri, invocation.handler_name, invocation.key
        )
    }

    /// Try to acquire PostgreSQL advisory lock for a job.
    /// Returns a guard that keeps the same DB connection until unlock.
    async fn try_acquire_job_lock(
        invocation: &ScheduledInvocation,
    ) -> Result<JobLockGuard, JobLockAcquireError> {
        // Get database pool if available
        let db = match crate::database::get_global_database() {
            Some(db) => db,
            None => {
                debug!(
                    script = %invocation.script_uri,
                    handler = %invocation.handler_name,
                    job = %invocation.key,
                    "Scheduler running without database; executing job without advisory lock"
                );
                return Ok(JobLockGuard::Noop);
            }
        };

        let lock_key = Self::generate_lock_key(invocation);
        let mut conn = match db.pool().acquire().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!(
                    "Error acquiring DB connection for advisory lock {}: {}. Skipping execution.",
                    lock_key, e
                );
                return Err(JobLockAcquireError::Unavailable);
            }
        };

        // Use pg_try_advisory_lock with hashtext for deterministic lock ID
        // Returns true if lock was acquired, false if already held
        match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock(hashtext($1))")
            .bind(&lock_key)
            .fetch_one(&mut *conn)
            .await
        {
            Ok(acquired) => {
                if acquired {
                    debug!("Acquired advisory lock for job: {}", lock_key);
                    Ok(JobLockGuard::Pg { conn, lock_key })
                } else {
                    debug!(
                        "Failed to acquire advisory lock for job: {} (another instance has it)",
                        lock_key
                    );
                    Err(JobLockAcquireError::HeldByOther)
                }
            }
            Err(e) => {
                warn!(
                    "Error trying to acquire advisory lock for job {}: {}. Skipping execution.",
                    lock_key, e
                );
                Err(JobLockAcquireError::Unavailable)
            }
        }
    }

    fn requeue_one_off_after_lock_failure(&self, invocation: &ScheduledInvocation) {
        let run_at = Utc::now() + Duration::seconds(1);
        let _ = self.register_one_off(
            &invocation.script_uri,
            &invocation.handler_name,
            Some(invocation.key.clone()),
            run_at,
        );
    }

    async fn claim_due_jobs_from_db(&self, now: DateTime<Utc>) -> Vec<ScheduledInvocation> {
        let db = match crate::database::get_global_database() {
            Some(db) => db,
            None => return Vec::new(),
        };

        let rows = match sqlx::query(
            r#"
            WITH candidates AS (
                SELECT job_id
                FROM scheduler_jobs
                WHERE run_at <= $1
                  AND (lock_expires_at IS NULL OR lock_expires_at <= $1)
                ORDER BY run_at ASC
                LIMIT $2
                FOR UPDATE SKIP LOCKED
            )
            UPDATE scheduler_jobs AS jobs
            SET locked_by = $3,
                locked_at = $1,
                lock_expires_at = $1 + make_interval(secs => $4),
                updated_at = NOW()
            FROM candidates
            WHERE jobs.job_id = candidates.job_id
            RETURNING jobs.job_id, jobs.script_uri, jobs.handler_name, jobs.job_key, jobs.kind, jobs.run_at, jobs.interval_ms, jobs.attempts
            "#,
        )
        .bind(now)
        .bind(DB_CLAIM_BATCH_SIZE)
        .bind(&self.worker_id)
        .bind(DB_LOCK_TTL_SECONDS)
        .fetch_all(db.pool())
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                warn!(error = %e, "Failed claiming due scheduler jobs from DB");
                return Vec::new();
            }
        };

        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            let kind = match row.get::<String, _>("kind").as_str() {
                "recurring" => ScheduledInvocationKind::Recurring,
                _ => ScheduledInvocationKind::OneOff,
            };

            let interval_seconds = if matches!(kind, ScheduledInvocationKind::Recurring) {
                row.get::<Option<i64>, _>("interval_ms")
                    .map(|interval_ms| interval_ms / 1000)
            } else {
                None
            };

            let interval_milliseconds = if matches!(kind, ScheduledInvocationKind::Recurring) {
                row.get::<Option<i64>, _>("interval_ms")
            } else {
                None
            };

            claimed.push(ScheduledInvocation {
                job_id: row.get("job_id"),
                invocation_id: crate::middleware::generate_request_id(),
                key: row.get("job_key"),
                script_uri: row.get("script_uri"),
                handler_name: row.get("handler_name"),
                kind,
                scheduled_for: row.get("run_at"),
                interval_seconds,
                interval_milliseconds,
                attempts: row.get("attempts"),
            });
        }

        claimed
    }

    async fn finalize_db_job_execution(&self, invocation: &ScheduledInvocation, succeeded: bool) {
        let db = match crate::database::get_global_database() {
            Some(db) => db,
            None => return,
        };

        match invocation.kind {
            ScheduledInvocationKind::OneOff => {
                if succeeded {
                    if let Err(e) = sqlx::query(
                        "DELETE FROM scheduler_jobs WHERE job_id = $1 AND locked_by = $2",
                    )
                    .bind(invocation.job_id)
                    .bind(&self.worker_id)
                    .execute(db.pool())
                    .await
                    {
                        warn!(job = %invocation.key, error = %e, "Failed deleting completed one-off scheduler job");
                    }
                } else {
                    let attempts = invocation.attempts.saturating_add(1);

                    // Out of attempts: the row goes, because leaving it would
                    // requeue it forever, and a job nobody will run again is
                    // not a job that is still scheduled.
                    if attempts >= MAX_ONE_OFF_ATTEMPTS {
                        if let Err(e) = sqlx::query(
                            "DELETE FROM scheduler_jobs WHERE job_id = $1 AND locked_by = $2",
                        )
                        .bind(invocation.job_id)
                        .bind(&self.worker_id)
                        .execute(db.pool())
                        .await
                        {
                            warn!(job = %invocation.key, error = %e, "Failed dropping exhausted one-off scheduler job");
                        }

                        warn!(
                            script = %invocation.script_uri,
                            handler = %invocation.handler_name,
                            job = %invocation.key,
                            attempts,
                            "Scheduler job failed every attempt; giving up"
                        );

                        // In the script's own log as well as the engine's: the
                        // person who scheduled the work reads one of those and
                        // not the other.
                        repository::insert_log_message_async_in_context(
                            &invocation.script_uri,
                            &format!(
                                "scheduler job '{}' failed {} times and will not be retried",
                                invocation.key, attempts
                            ),
                            "FATAL",
                            &Self::invocation_log_context(invocation),
                        )
                        .await;
                        return;
                    }

                    let retry_at = Utc::now() + one_off_retry_delay(attempts);
                    if let Err(e) = sqlx::query(
                        r#"
                        UPDATE scheduler_jobs
                        SET run_at = $1,
                            attempts = $2,
                            locked_by = NULL,
                            locked_at = NULL,
                            lock_expires_at = NULL,
                            updated_at = NOW()
                        WHERE job_id = $3 AND locked_by = $4
                        "#,
                    )
                    .bind(retry_at)
                    .bind(attempts)
                    .bind(invocation.job_id)
                    .bind(&self.worker_id)
                    .execute(db.pool())
                    .await
                    {
                        warn!(job = %invocation.key, error = %e, "Failed requeueing failed one-off scheduler job");
                    } else {
                        debug!(
                            job = %invocation.key,
                            attempts,
                            retry_at = %retry_at.to_rfc3339(),
                            "Scheduler job requeued after failure"
                        );
                    }
                }
            }
            ScheduledInvocationKind::Recurring => {
                let interval_milliseconds = invocation
                    .interval_milliseconds
                    .unwrap_or(1000)
                    .max(MIN_RECURRING_INTERVAL_MS);
                let interval = Duration::milliseconds(interval_milliseconds);
                let mut next_run = invocation.scheduled_for + interval;
                let now = Utc::now();
                while next_run <= now {
                    next_run += interval;
                }

                if let Err(e) = sqlx::query(
                    r#"
                    UPDATE scheduler_jobs
                    SET run_at = $1,
                        locked_by = NULL,
                        locked_at = NULL,
                        lock_expires_at = NULL,
                        updated_at = NOW()
                    WHERE job_id = $2 AND locked_by = $3
                    "#,
                )
                .bind(next_run)
                .bind(invocation.job_id)
                .bind(&self.worker_id)
                .execute(db.pool())
                .await
                {
                    warn!(job = %invocation.key, error = %e, "Failed updating recurring scheduler job next_run");
                }
            }
        }
    }

    async fn dispatch(self: Arc<Self>, invocation: ScheduledInvocation) {
        let script_uri = invocation.script_uri.clone();
        let handler_name = invocation.handler_name.clone();
        let job_key = invocation.key.clone();
        let invocation_for_engine = invocation.clone();

        // Try to acquire PostgreSQL advisory lock for this job
        // This ensures only one instance executes the job
        let lock_guard = match Self::try_acquire_job_lock(&invocation).await {
            Ok(guard) => guard,
            Err(JobLockAcquireError::HeldByOther) => {
                debug!(
                    script = %script_uri,
                    handler = %handler_name,
                    job = %job_key,
                    "Skipping job execution - another instance has the lock"
                );
                return;
            }
            Err(JobLockAcquireError::Unavailable) => {
                warn!(
                    script = %script_uri,
                    handler = %handler_name,
                    job = %job_key,
                    "Skipping job execution - advisory lock unavailable"
                );
                if matches!(invocation.kind, ScheduledInvocationKind::OneOff) {
                    self.requeue_one_off_after_lock_failure(&invocation);
                }
                return;
            }
        };

        debug!(
            script = %script_uri,
            handler = %handler_name,
            job = %job_key,
            "Acquired lock for job execution"
        );

        // Before the slot wait, not after: waiting for a slot is unbounded and
        // is as much a part of holding the job as running it is.
        let renewal = if Self::has_database() {
            Some(crate::lease::spawn_renewal(
                crate::lease::Leased::SchedulerJob,
                invocation.job_id,
                self.worker_id.clone(),
                invocation.key.clone(),
            ))
        } else {
            None
        };

        let permit = crate::execution_slots::acquire().await;
        let execution = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            js_engine::execute_scheduled_handler(&script_uri, &handler_name, &invocation_for_engine)
        })
        .await;

        // The run is over either way, so the claim stops being extended before
        // anything is written about how it ended.
        if let Some(renewal) = renewal {
            renewal.abort();
        }

        let mut succeeded = false;
        match execution {
            Ok(Ok(())) => {
                succeeded = true;
                debug!(
                    script = invocation.script_uri,
                    handler = invocation.handler_name,
                    job = invocation.key,
                    "Scheduler job completed"
                );
            }
            Ok(Err(err)) => {
                warn!(
                    script = invocation.script_uri,
                    handler = invocation.handler_name,
                    job = invocation.key,
                    error = %err,
                    "Scheduler job failed"
                );
                repository::insert_log_message_async_in_context(
                    &invocation.script_uri,
                    &format!(
                        "scheduler job '{}' failed at {}: {}",
                        invocation.key,
                        invocation.scheduled_for.to_rfc3339(),
                        err
                    ),
                    "FATAL",
                    &Self::invocation_log_context(&invocation),
                )
                .await;
            }
            Err(join_err) => {
                error!(
                    script = invocation.script_uri,
                    handler = invocation.handler_name,
                    job = invocation.key,
                    error = %join_err,
                    "Scheduler job panicked"
                );
                repository::insert_log_message_async_in_context(
                    &invocation.script_uri,
                    &format!(
                        "scheduler job '{}' panicked at {}: {}",
                        invocation.key,
                        invocation.scheduled_for.to_rfc3339(),
                        join_err
                    ),
                    "FATAL",
                    &Self::invocation_log_context(&invocation),
                )
                .await;
            }
        }

        if Self::has_database() {
            self.finalize_db_job_execution(&invocation, succeeded).await;
        }

        // Release the advisory lock using the same DB connection used to acquire it.
        lock_guard.release().await;
    }

    fn sleep_duration_until(&self, next: Option<DateTime<Utc>>) -> StdDuration {
        if Self::has_database() {
            return StdDuration::from_millis(500);
        }

        if let Some(next_run) = next {
            let now = Utc::now();
            if next_run <= now {
                return StdDuration::from_millis(100);
            }
            let diff = next_run - now;
            diff.to_std()
                .unwrap_or_else(|_| StdDuration::from_millis(100))
        } else {
            StdDuration::from_secs(5)
        }
    }

    pub async fn run(self: Arc<Self>, mut shutdown: oneshot::Receiver<()>) {
        info!("Scheduler worker started");
        loop {
            let now = Utc::now();
            let mut due_jobs = self.collect_due_jobs(now);

            if Self::has_database() {
                due_jobs.extend(self.claim_due_jobs_from_db(now).await);
            }

            for invocation in due_jobs {
                tokio::spawn(self.clone().dispatch(invocation));
            }

            let sleep_duration = self.sleep_duration_until(self.next_trigger_at());

            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {}
                _ = self.wake_signal.notified() => {}
                _ = &mut shutdown => {
                    info!("Scheduler worker shutting down");
                    break;
                }
            }
        }
    }
}

/// Initialize global scheduler state if it is not already created
pub fn initialize_global_scheduler() -> Arc<Scheduler> {
    GLOBAL_SCHEDULER
        .get_or_init(|| {
            info!("Initializing scheduler service");
            Arc::new(Scheduler::new())
        })
        .clone()
}

/// Obtain a handle to the global scheduler
pub fn get_scheduler() -> Arc<Scheduler> {
    initialize_global_scheduler()
}

/// Clear all jobs for a script if the scheduler is available
pub fn clear_script_jobs(script_uri: &str) -> usize {
    GLOBAL_SCHEDULER
        .get()
        .map(|scheduler| scheduler.clear_script(script_uri))
        .unwrap_or(0)
}

/// [`clear_script_jobs`] for callers that are already async.
pub async fn clear_script_jobs_async(script_uri: &str) -> usize {
    match GLOBAL_SCHEDULER.get() {
        Some(scheduler) => scheduler.clear_script_async(script_uri).await,
        None => 0,
    }
}

/// Spawn the background worker. This should be called once during server startup.
pub fn spawn_worker(shutdown: oneshot::Receiver<()>) {
    let scheduler = get_scheduler();
    tokio::spawn(scheduler.run(shutdown));
}

/// Parse an RFC3339 timestamp that must end with 'Z' (UTC)
pub fn parse_utc_timestamp(value: &str) -> Result<DateTime<Utc>, SchedulerError> {
    match DateTime::parse_from_rfc3339(value) {
        Ok(dt) if dt.offset().local_minus_utc() == 0 => Ok(dt.with_timezone(&Utc)),
        _ => Err(SchedulerError::InvalidTimestamp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_once_stores_job() {
        let scheduler = Scheduler::new();
        let run_at = Utc::now() + Duration::minutes(5);
        let job = scheduler
            .register_one_off("script.js", "handler", None, run_at)
            .expect("registration should succeed");
        assert_eq!(job.schedule.next_run(), run_at);
    }

    #[test]
    fn register_recurring_requires_valid_interval() {
        let scheduler = Scheduler::new();
        let result =
            scheduler.register_recurring("script.js", "handler", None, Duration::minutes(0), None);
        assert!(matches!(result, Err(SchedulerError::InvalidInterval)));

        let too_small = scheduler.register_recurring(
            "script.js",
            "handler",
            None,
            Duration::milliseconds(99),
            None,
        );
        assert!(matches!(too_small, Err(SchedulerError::InvalidInterval)));

        let valid = scheduler.register_recurring(
            "script.js",
            "handler",
            None,
            Duration::milliseconds(100),
            None,
        );
        assert!(valid.is_ok());
    }

    #[test]
    fn collect_due_jobs_returns_ready_items() {
        let scheduler = Scheduler::new();
        let now = Utc::now();
        scheduler
            .register_one_off(
                "script.js",
                "handler",
                None,
                now + Duration::milliseconds(10),
            )
            .unwrap();
        let due = scheduler.collect_due_jobs(now + Duration::seconds(1));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].handler_name, "handler");
    }

    #[test]
    fn the_retry_delay_doubles_and_then_stops_growing() {
        assert_eq!(
            one_off_retry_delay(1),
            Duration::seconds(DB_ONE_OFF_RETRY_DELAY_SECONDS)
        );
        assert_eq!(
            one_off_retry_delay(2),
            Duration::seconds(DB_ONE_OFF_RETRY_DELAY_SECONDS * 2)
        );
        assert_eq!(
            one_off_retry_delay(3),
            Duration::seconds(DB_ONE_OFF_RETRY_DELAY_SECONDS * 4)
        );

        // Far past the cap, and past the shift width, so neither overflows.
        assert_eq!(
            one_off_retry_delay(i32::MAX),
            Duration::seconds(DB_ONE_OFF_MAX_RETRY_DELAY_SECONDS)
        );
    }

    /// A first failure still waits before retrying. `attempts` counts runs that
    /// have happened, so nothing ever asks for the delay before attempt zero.
    #[test]
    fn the_retry_delay_is_never_zero() {
        for attempts in 0..=MAX_ONE_OFF_ATTEMPTS {
            assert!(
                one_off_retry_delay(attempts) > Duration::zero(),
                "attempt {} would retry immediately",
                attempts
            );
        }
    }

    /// Five attempts at a doubling delay is minutes, not hours: long enough to
    /// ride out a restarting dependency, short enough that the work is not
    /// still being retried when someone comes to look at it.
    #[test]
    fn giving_up_takes_minutes() {
        let total: i64 = (1..MAX_ONE_OFF_ATTEMPTS)
            .map(|attempts| one_off_retry_delay(attempts).num_seconds())
            .sum();

        assert!(
            (10..=900).contains(&total),
            "the whole retry sequence spans {}s",
            total
        );
    }

    /// Everything above is arithmetic. This is the half that is SQL: the
    /// `locked_by` guard, the increment, and the delete that has to happen
    /// once the attempts are spent.
    mod retries {
        use super::*;

        fn use_the_test_database() -> sqlx::PgPool {
            let pool = crate::test_db::pool();
            // One process per test under nextest, so claiming the global is
            // this test's own business.
            crate::database::initialize_global_database(std::sync::Arc::new(
                crate::database::Database::from_pool(pool.clone()),
            ));
            pool
        }

        /// A job row already claimed by `scheduler`, as the worker leaves it
        /// just before running the handler.
        async fn claimed_job(
            pool: &sqlx::PgPool,
            scheduler: &Scheduler,
            key: &str,
            attempts: i32,
        ) -> ScheduledInvocation {
            let job_id = Uuid::new_v4();
            let script_uri = format!("test://scheduler-retries/{}", key);

            sqlx::query(
                r#"
                INSERT INTO scheduler_jobs
                    (job_id, script_uri, handler_name, job_key, kind, run_at, attempts,
                     locked_by, locked_at, lock_expires_at)
                VALUES ($1, $2, 'handler', $3, 'one_off', NOW(), $4, $5, NOW(), NOW() + INTERVAL '1 minute')
                "#,
            )
            .bind(job_id)
            .bind(&script_uri)
            .bind(key)
            .bind(attempts)
            .bind(&scheduler.worker_id)
            .execute(pool)
            .await
            .expect("the job row should insert");

            ScheduledInvocation {
                job_id,
                invocation_id: "test-invocation".to_string(),
                key: key.to_string(),
                script_uri,
                handler_name: "handler".to_string(),
                kind: ScheduledInvocationKind::OneOff,
                scheduled_for: Utc::now(),
                interval_seconds: None,
                interval_milliseconds: None,
                attempts,
            }
        }

        async fn row_of(pool: &sqlx::PgPool, job_id: Uuid) -> Option<(i32, DateTime<Utc>)> {
            sqlx::query("SELECT attempts, run_at FROM scheduler_jobs WHERE job_id = $1")
                .bind(job_id)
                .fetch_optional(pool)
                .await
                .expect("the lookup should succeed")
                .map(|row| (row.get("attempts"), row.get("run_at")))
        }

        #[tokio::test]
        async fn a_failure_counts_the_attempt_and_waits_before_the_next() {
            let pool = use_the_test_database();
            let scheduler = Scheduler::new();
            let invocation = claimed_job(&pool, &scheduler, "counts-attempts", 0).await;

            let before = Utc::now();
            scheduler
                .finalize_db_job_execution(&invocation, false)
                .await;

            let (attempts, run_at) = row_of(&pool, invocation.job_id)
                .await
                .expect("a job with attempts left should still be scheduled");

            assert_eq!(attempts, 1, "the failure should have been counted");
            assert!(
                run_at > before,
                "the retry should be scheduled for later, not run again at once"
            );
        }

        /// The flat two-second requeue meant a job failing on something that
        /// would never change retried for as long as the engine was up.
        #[tokio::test]
        async fn the_last_failure_drops_the_job_rather_than_requeueing_it() {
            let pool = use_the_test_database();
            let scheduler = Scheduler::new();
            let invocation = claimed_job(
                &pool,
                &scheduler,
                "gives-up",
                MAX_ONE_OFF_ATTEMPTS.saturating_sub(1),
            )
            .await;

            scheduler
                .finalize_db_job_execution(&invocation, false)
                .await;

            assert!(
                row_of(&pool, invocation.job_id).await.is_none(),
                "a job out of attempts should not still be scheduled"
            );
        }

        /// The completing worker names itself in the WHERE clause. Another
        /// instance's row is not this worker's to retry or to drop.
        #[tokio::test]
        async fn a_job_claimed_by_another_worker_is_left_alone() {
            let pool = use_the_test_database();
            let owner = Scheduler::new();
            let stranger = Scheduler::new();
            let invocation = claimed_job(&pool, &owner, "not-yours", 0).await;

            stranger.finalize_db_job_execution(&invocation, false).await;

            let (attempts, _) = row_of(&pool, invocation.job_id)
                .await
                .expect("the row should survive a stranger finalizing it");
            assert_eq!(attempts, 0, "a stranger should not count an attempt");
        }

        /// The bug this replaced: a claim lapsed a fixed time after it was
        /// taken, however long the run turned out to be, so a long job had its
        /// row re-claimed while it was still running and its own completing
        /// statement then matched nothing.
        #[tokio::test]
        async fn a_claim_is_extended_while_the_job_runs() {
            let pool = use_the_test_database();
            let scheduler = Scheduler::new();
            let invocation = claimed_job(&pool, &scheduler, "renews", 0).await;

            // Insert left the lease a minute out; wind it back to just before
            // lapsing, so a renewal is visible as an increase.
            sqlx::query(
                "UPDATE scheduler_jobs SET lock_expires_at = NOW() + INTERVAL '1 second' \
                 WHERE job_id = $1",
            )
            .bind(invocation.job_id)
            .execute(&pool)
            .await
            .expect("the lease should wind back");

            let renewal = crate::lease::spawn_renewal(
                crate::lease::Leased::SchedulerJob,
                invocation.job_id,
                scheduler.worker_id.clone(),
                invocation.key.clone(),
            );
            tokio::time::sleep(StdDuration::from_secs(
                crate::lease::RENEW_EVERY_SECONDS + 2,
            ))
            .await;
            renewal.abort();

            let expires: DateTime<Utc> =
                sqlx::query("SELECT lock_expires_at FROM scheduler_jobs WHERE job_id = $1")
                    .bind(invocation.job_id)
                    .fetch_one(&pool)
                    .await
                    .expect("the row should still be there")
                    .get("lock_expires_at");

            assert!(
                expires > Utc::now() + Duration::seconds(DB_LOCK_TTL_SECONDS / 2),
                "the claim should have been pushed out, not left to lapse"
            );
        }

        /// Renewal is scoped to the worker holding the claim, so a job that has
        /// genuinely moved on is not taken back by the worker that lost it.
        #[tokio::test]
        async fn renewal_stops_once_the_claim_has_moved_on() {
            let pool = use_the_test_database();
            let owner = Scheduler::new();
            let stranger = Scheduler::new();
            let invocation = claimed_job(&pool, &owner, "moved-on", 0).await;

            let renewal = crate::lease::spawn_renewal(
                crate::lease::Leased::SchedulerJob,
                invocation.job_id,
                stranger.worker_id.clone(),
                invocation.key.clone(),
            );
            tokio::time::sleep(StdDuration::from_secs(
                crate::lease::RENEW_EVERY_SECONDS + 2,
            ))
            .await;

            assert!(
                renewal.is_finished(),
                "a worker that does not hold the claim should stop renewing it"
            );
            renewal.abort();
        }

        #[tokio::test]
        async fn a_successful_one_off_is_deleted() {
            let pool = use_the_test_database();
            let scheduler = Scheduler::new();
            let invocation = claimed_job(&pool, &scheduler, "succeeds", 0).await;

            scheduler.finalize_db_job_execution(&invocation, true).await;

            assert!(
                row_of(&pool, invocation.job_id).await.is_none(),
                "a completed one-off should not still be scheduled"
            );
        }
    }

    #[test]
    fn parse_timestamp_requires_utc() {
        assert!(parse_utc_timestamp("2024-01-01T00:00:00Z").is_ok());
        assert!(parse_utc_timestamp("2024-01-01T00:00:00+02:00").is_err());
    }
}
