//! A durable queue of work a script enqueued for itself.
//!
//! The shape the engine had no expression for: start something during a
//! request, answer the request, and finish the work afterwards.
//!
//! # A task and a schedule
//!
//! A schedule ([`crate::schedules`]) is a *declaration*: keyed, so saying it
//! again replaces what it said, and either the script's own (declared in
//! `init()`, belonging to the version of the code) or a person's. It never runs
//! anything itself; when it is due it inserts a task here.
//!
//! A task is the other thing. It exists because something happened, it carries
//! what happened as a payload, and two of them are two pieces of work rather
//! than one restatement. So it survives `init()` — deploying a new version of
//! a script must not discard work that was already accepted — and enqueueing
//! is allowed from anywhere rather than only during registration.
//!
//! # What runs it
//!
//! [`spawn_worker`] polls for due tasks, claims them with a renewed lease
//! ([`crate::lease`]), and runs the named handler under
//! the job budget (`javascript.job_timeout_ms`). A task runs in **script
//! context**: it holds what the script holds and nothing belonging to whoever
//! enqueued it. Running as a person is a consent question — what they
//! authorised, for which script, for how long, and how they revoke it — and it
//! is answered by a delegation record, not by widening this.

use std::sync::OnceLock;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::Row;
use tokio::sync::{Notify, oneshot};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{js_engine, repository};

/// Longest a handler name may be, matching the scheduler's limit on a job name.
pub const MAX_HANDLER_NAME_CHARS: usize = 64;

/// Largest payload a task may carry, as serialised JSON.
///
/// A payload describes work; it is not where the work's data lives. Something
/// larger belongs in `scriptStorage` or the script's own tables with the task
/// carrying the key, which also means a retry re-reads the current data rather
/// than a copy taken when the task was enqueued.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// How many times a task is attempted before it is given up on, unless the
/// caller asks for fewer.
pub const DEFAULT_MAX_ATTEMPTS: i32 = 5;

/// The ceiling on what a caller may ask for.
pub const ATTEMPTS_LIMIT: i32 = 25;

/// The first retry delay; each attempt after that waits twice as long.
const RETRY_BASE_SECONDS: i64 = 5;

/// The ceiling on that doubling.
const RETRY_MAX_SECONDS: i64 = 3600;

/// How many due tasks one worker takes per tick.
const CLAIM_BATCH_SIZE: i64 = 16;

/// The budget for one task's handler, from configuration, set once at
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
/// executes in, and the lease that stops a second instance claiming the task
/// while it is still running. They are the same number because a claim shorter
/// than the run is what lets a completed run happen twice.
pub fn configured_job_timeout_ms() -> u64 {
    CONFIGURED_JOB_TIMEOUT_MS
        .get()
        .copied()
        .unwrap_or_else(|| crate::js_engine::current_execution_limits().timeout_ms)
}

/// An RFC 3339 timestamp in UTC (`Z`, or an offset of zero), or `None`.
pub fn parse_utc_timestamp(value: &str) -> Option<DateTime<Utc>> {
    match DateTime::parse_from_rfc3339(value) {
        Ok(dt) if dt.offset().local_minus_utc() == 0 => Some(dt.with_timezone(&Utc)),
        _ => None,
    }
}

/// How often the worker looks for due tasks when nothing has woken it.
const POLL_INTERVAL_MS: u64 = 500;

static WAKE: OnceLock<Notify> = OnceLock::new();

/// Woken when a task is enqueued, so work that is due now does not wait out a
/// poll interval before starting.
fn wake_signal() -> &'static Notify {
    WAKE.get_or_init(Notify::new)
}

/// Wake the worker, for a caller that queued work through its own
/// transaction and has just committed it.
pub fn wake() {
    wake_signal().notify_waiters();
}

/// Why an enqueue was refused.
///
/// Separate variants rather than one string because the JavaScript side turns
/// each into a different message, and because a caller can act on some of them.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnqueueError {
    #[error("handler name is required")]
    MissingHandler,
    #[error("handler name must be 1-{MAX_HANDLER_NAME_CHARS} characters")]
    InvalidHandler,
    #[error("payload must be a JSON object")]
    PayloadNotAnObject,
    #[error("payload must be at most {MAX_PAYLOAD_BYTES} bytes")]
    PayloadTooLarge,
    #[error("maxAttempts must be between 1 and {ATTEMPTS_LIMIT}")]
    InvalidMaxAttempts,
    #[error("runAt must be a UTC timestamp ending with 'Z'")]
    InvalidRunAt,
    #[error("no database is available to store the task")]
    Unavailable,
    #[error("could not store the task: {0}")]
    Storage(String),
}

/// What a caller is asking to have run.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub script_uri: String,
    pub handler_name: String,
    pub payload: Value,
    /// When it becomes due. `None` means now.
    pub run_at: Option<DateTime<Utc>>,
    /// How many attempts it gets. `None` takes [`DEFAULT_MAX_ATTEMPTS`].
    pub max_attempts: Option<i32>,
    pub enqueued_by: Option<String>,
    /// Who this runs as. `None` is script context — what every task was
    /// before delegation existed, and still the default.
    pub run_as: Option<String>,
    /// What this must not run beside.
    ///
    /// At most one task per `(script, lane)` runs at a time; the rest stay
    /// pending until it finishes. `None` is no lane, the default for
    /// `scriptTasks`: claimed and run alongside anything else.
    pub lane: Option<String>,
}

/// A task as stored.
#[derive(Debug, Clone)]
pub struct Task {
    pub task_id: Uuid,
    pub script_uri: String,
    pub handler_name: String,
    pub payload: Value,
    pub state: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub run_at: DateTime<Utc>,
    pub enqueued_by: Option<String>,
    pub run_as: Option<String>,
    pub lane: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    fn from_row(row: &sqlx::postgres::PgRow) -> Self {
        Self {
            task_id: row.get("task_id"),
            script_uri: row.get("script_uri"),
            handler_name: row.get("handler_name"),
            payload: row.get("payload"),
            state: row.get("state"),
            attempts: row.get("attempts"),
            max_attempts: row.get("max_attempts"),
            last_error: row.get("last_error"),
            run_at: row.get("run_at"),
            enqueued_by: row.get("enqueued_by"),
            run_as: row.get("run_as"),
            lane: row.get("lane"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }
}

/// One run of a task, as handed to the JavaScript side.
#[derive(Debug, Clone)]
pub struct TaskInvocation {
    pub task_id: Uuid,
    /// This run, as distinct from `task_id`, which is the same across retries.
    /// The engine's lines about the run and the lines the handler writes share
    /// it, so one attempt can be pulled out of a busy script's log.
    pub invocation_id: String,
    pub script_uri: String,
    pub handler_name: String,
    pub payload: Value,
    /// Attempts before this one, so a handler can tell a retry from a first
    /// run without keeping its own count.
    pub attempts: i32,
    pub max_attempts: i32,
    /// Who this runs as, as *recorded*. Never trusted on its own: what it may
    /// do is worked out by `delegation::resolve` when the task runs.
    pub run_as: Option<String>,
    /// The claim this run holds the row under ([`crate::lease`]). Every write
    /// about how the run ended is guarded by it.
    pub claim_epoch: i64,
    /// The schedule that fired this task, when one did.
    pub schedule: Option<ScheduleRun>,
}

/// Which schedule a task is a run of, and which slot.
#[derive(Debug, Clone)]
pub struct ScheduleRun {
    pub schedule_id: Uuid,
    /// The schedule's key, as the handler reads it in `context.meta.schedule`.
    /// `None` when the schedule was deleted after it fired.
    pub key: Option<String>,
    /// The slot this run is for, which retries do not move.
    pub scheduled_for: DateTime<Utc>,
}

/// How long to wait before the attempt after `attempts` failures.
///
/// Doubling rather than flat, for the reason the scheduler's is: a dependency
/// that is restarting wants seconds, and one that is down wants the engine to
/// stop asking at the same rate until it is back. The base is longer than the
/// scheduler's because a task is likelier to be waiting on something outside
/// the engine.
fn retry_delay(attempts: i32) -> Duration {
    let doublings = attempts.saturating_sub(1).clamp(0, 16) as u32;
    let seconds = RETRY_BASE_SECONDS
        .saturating_mul(1i64 << doublings)
        .min(RETRY_MAX_SECONDS);
    Duration::seconds(seconds)
}

/// The longest a lane name may be.
///
/// A lane is compared and never interpreted, so the cap is about what is
/// worth storing and indexing rather than about safety. Generous enough for
/// the shapes that matter — a user id, a conversation id, the two joined.
const MAX_LANE_CHARS: usize = 128;

/// Trim a lane, and treat an empty one as no lane.
///
/// Refusing an empty string instead would turn `lane: ""` — which a script
/// gets from an interpolation whose variable was empty — into an error at
/// the enqueue rather than into the unconstrained behaviour it plainly
/// means. An over-long one is truncated rather than refused for the same
/// reason: the caller's intent is legible and losing the tail still
/// serialises everything that shares the prefix, which is the direction that
/// errs toward running less at once rather than more.
pub(crate) fn normalize_lane(lane: Option<&str>) -> Option<String> {
    let lane = lane?.trim();
    if lane.is_empty() {
        return None;
    }
    Some(lane.chars().take(MAX_LANE_CHARS).collect())
}

/// Check and normalise what a caller asked for.
///
/// Separate from the insert so the whole of it is testable without a database,
/// and so the JavaScript side gets one refusal per thing that can be wrong.
fn validate(task: &NewTask) -> Result<(i32, String), EnqueueError> {
    let handler = task.handler_name.trim();
    if handler.is_empty() {
        return Err(EnqueueError::MissingHandler);
    }
    if handler.chars().count() > MAX_HANDLER_NAME_CHARS {
        return Err(EnqueueError::InvalidHandler);
    }

    // An object rather than any JSON: a handler reads named fields, and a bare
    // string or array is almost always a caller who meant to wrap it.
    if !task.payload.is_object() {
        return Err(EnqueueError::PayloadNotAnObject);
    }
    let payload_len = serde_json::to_vec(&task.payload)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(EnqueueError::PayloadTooLarge);
    }

    let max_attempts = task.max_attempts.unwrap_or(DEFAULT_MAX_ATTEMPTS);
    if !(1..=ATTEMPTS_LIMIT).contains(&max_attempts) {
        return Err(EnqueueError::InvalidMaxAttempts);
    }

    Ok((max_attempts, handler.to_string()))
}

/// Accept a piece of work.
pub async fn enqueue(task: NewTask) -> Result<Task, EnqueueError> {
    let Some(db) = crate::database::get_global_database() else {
        return Err(EnqueueError::Unavailable);
    };
    let task = insert(db.pool(), task).await?;
    // Work that is due now should not wait out a poll interval.
    wake_signal().notify_waiters();
    Ok(task)
}

/// Write the row for a piece of work, on whichever connection is given.
async fn insert<'e, E>(executor: E, task: NewTask) -> Result<Task, EnqueueError>
where
    E: sqlx::PgExecutor<'e>,
{
    insert_row(executor, task, None).await
}

/// Write the row for one run of a personal schedule, inside the transaction
/// that fires it ([`crate::schedules::fire_due`]).
pub(crate) async fn insert_scheduled<'e, E>(
    executor: E,
    task: NewTask,
    schedule_id: Uuid,
    scheduled_for: DateTime<Utc>,
) -> Result<Task, EnqueueError>
where
    E: sqlx::PgExecutor<'e>,
{
    insert_row(executor, task, Some((schedule_id, scheduled_for))).await
}

async fn insert_row<'e, E>(
    executor: E,
    task: NewTask,
    schedule: Option<(Uuid, DateTime<Utc>)>,
) -> Result<Task, EnqueueError>
where
    E: sqlx::PgExecutor<'e>,
{
    let (max_attempts, handler) = validate(&task)?;
    let lane = normalize_lane(task.lane.as_deref());

    let task_id = Uuid::new_v4();
    let run_at = task.run_at.unwrap_or_else(Utc::now);

    let row = sqlx::query(
        r#"
        INSERT INTO script_tasks
            (task_id, script_uri, handler_name, payload, state, max_attempts, run_at, enqueued_by,
             run_as, lane, schedule_id, scheduled_for)
        VALUES ($1, $2, $3, $4, 'pending', $5, $6, $7, $8, $9, $10, $11)
        RETURNING task_id, script_uri, handler_name, payload, state, attempts, max_attempts,
                  last_error, run_at, enqueued_by, run_as, lane, created_at, updated_at
        "#,
    )
    .bind(task_id)
    .bind(&task.script_uri)
    .bind(&handler)
    .bind(&task.payload)
    .bind(max_attempts)
    .bind(run_at)
    .bind(task.enqueued_by.as_deref())
    .bind(task.run_as.as_deref())
    .bind(lane.as_deref())
    .bind(schedule.map(|(id, _)| id))
    .bind(schedule.map(|(_, at)| at))
    .fetch_one(executor)
    .await
    .map_err(|e| EnqueueError::Storage(e.to_string()))?;

    debug!(
        script = %task.script_uri,
        handler = %handler,
        task = %task_id,
        lane = ?lane,
        "Task enqueued"
    );

    Ok(Task::from_row(&row))
}

/// The tasks recorded for a script, newest first.
pub async fn list(script_uri: &str, limit: i64) -> Result<Vec<Task>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(Vec::new());
    };

    let rows = sqlx::query(
        r#"
        SELECT task_id, script_uri, handler_name, payload, state, attempts, max_attempts,
               last_error, run_at, enqueued_by, run_as, lane, created_at, updated_at
        FROM script_tasks
        WHERE script_uri = $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(script_uri)
    .bind(limit.clamp(1, 500))
    .fetch_all(db.pool())
    .await?;

    Ok(rows.iter().map(Task::from_row).collect())
}

/// One task, whoever it belongs to.
pub async fn get(task_id: Uuid) -> Result<Option<Task>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(None);
    };
    get_on(db.pool(), task_id).await
}

async fn get_on<'e, E>(executor: E, task_id: Uuid) -> Result<Option<Task>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let row = sqlx::query(
        r#"
        SELECT task_id, script_uri, handler_name, payload, state, attempts, max_attempts,
               last_error, run_at, enqueued_by, run_as, lane, created_at, updated_at
        FROM script_tasks
        WHERE task_id = $1
        "#,
    )
    .bind(task_id)
    .fetch_optional(executor)
    .await?;

    Ok(row.as_ref().map(Task::from_row))
}

/// Stop a task that has not finished.
///
/// Only a pending one. A task already claimed by a worker is running, and
/// marking it cancelled would not stop it — it would only make the worker's
/// own finalize, which names itself in `locked_by`, write to a row that has
/// since been given a different meaning.
pub async fn cancel(task_id: Uuid) -> Result<bool, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(false);
    };
    cancel_on(db.pool(), task_id).await
}

async fn cancel_on<'e, E>(executor: E, task_id: Uuid) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let result = sqlx::query(
        r#"
        UPDATE script_tasks
        SET state = 'cancelled',
            locked_by = NULL,
            locked_at = NULL,
            lock_expires_at = NULL,
            updated_at = NOW()
        WHERE task_id = $1 AND state = 'pending'
        "#,
    )
    .bind(task_id)
    .execute(executor)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Discard a script's finished tasks — the failed and the cancelled.
///
/// The counterpart to keeping them: they are kept so a failure explains
/// itself, and they go when whoever owns the script has read them. Nothing
/// unfinished is touched.
pub async fn clear_finished(script_uri: &str) -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };

    let result = sqlx::query(
        "DELETE FROM script_tasks WHERE script_uri = $1 AND state IN ('failed', 'cancelled')",
    )
    .bind(script_uri)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected())
}

/// Cancel the pending work queued to act as `user_id`.
///
/// `script_uri` narrows it to one script's work — a single grant being
/// withdrawn — and `None` takes all of it, which is what happens when an
/// account's access changes or it is deleted.
///
/// Only the pending ones, for the reason [`cancel`] gives: a task already
/// claimed is running, and marking its row would not stop it. That one is
/// caught by the run-time check instead, which reads the grant again and
/// refuses — so a revocation lands either way, here or there.
pub async fn cancel_delegated(user_id: &str, script_uri: Option<&str>) -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };

    let result = match script_uri {
        Some(script_uri) => {
            sqlx::query(
                r#"
                UPDATE script_tasks
                SET state = 'cancelled',
                    last_error = 'the authorisation to act for this person was withdrawn',
                    locked_by = NULL,
                    locked_at = NULL,
                    lock_expires_at = NULL,
                    updated_at = NOW()
                WHERE run_as = $1 AND script_uri = $2 AND state = 'pending'
                "#,
            )
            .bind(user_id)
            .bind(script_uri)
            .execute(db.pool())
            .await?
        }
        None => {
            sqlx::query(
                r#"
                UPDATE script_tasks
                SET state = 'cancelled',
                    last_error = 'the authorisation to act for this person was withdrawn',
                    locked_by = NULL,
                    locked_at = NULL,
                    lock_expires_at = NULL,
                    updated_at = NOW()
                WHERE run_as = $1 AND state = 'pending'
                "#,
            )
            .bind(user_id)
            .execute(db.pool())
            .await?
        }
    };

    Ok(result.rows_affected())
}

/// Drop every task belonging to a script.
///
/// For a script being deleted. Unlike the scheduler's equivalent this is *not*
/// called when a script is re-initialised: a task outlives the deployment that
/// accepted it.
pub async fn delete_for_script(script_uri: &str) -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };

    let result = sqlx::query("DELETE FROM script_tasks WHERE script_uri = $1")
        .bind(script_uri)
        .execute(db.pool())
        .await?;

    Ok(result.rows_affected())
}

/// Take up to [`CLAIM_BATCH_SIZE`] due tasks for this worker, at most one per
/// lane.
///
/// `FOR UPDATE SKIP LOCKED` so several instances claim disjoint batches rather
/// than queueing behind each other, and a lapsed lease makes a row claimable
/// again — which is how a task survives the worker that was running it dying.
///
/// # Holding one task per lane
///
/// "At most one running per `(script, lane)`" has to survive three different
/// ways two tasks in one lane can be claimed at once, and each needs its own
/// piece of the claim — the last of them two. Any one of those pieces alone
/// leaves a hole, and a lane key that occasionally lets two run is worse than
/// none — scripts would go on writing the workaround it exists to remove.
///
/// **A lane already busy.** The `NOT EXISTS` excludes a candidate whose lane
/// holds a live run. Live, not merely `state = 'running'`: a row left behind
/// by a worker that died has a lapsed lease and is claimable again, so it
/// must not hold its lane shut in the meantime.
///
/// **Two due tasks in one lane, one worker.** The `NOT EXISTS` cannot see
/// this, because neither row is running yet — both pass, and one statement
/// claims both. `ROW_NUMBER` over the locked candidates keeps the oldest of
/// each lane and leaves the rest for the next tick. It is a second CTE
/// because a window function and `FOR UPDATE` may not share a `SELECT`.
///
/// **Two due tasks in one lane, two workers.** Mostly impossible already:
/// `FOR UPDATE` locks *every* candidate this statement selected, not just the
/// ones that survive the filters, so a second worker's `SKIP LOCKED` passes
/// over the whole set. The gap is the `LIMIT` — a lane's second task falling
/// outside one worker's batch is not locked by it, and the other worker
/// evaluates `NOT EXISTS` against a snapshot where the first claim has not
/// committed. `pg_try_advisory_xact_lock` is the exclusion that closes it: a
/// lane is held by whichever claim is choosing from it, so the other worker
/// skips the lane rather than racing on it. The lock is re-entrant within one
/// transaction, which is why it does not also solve the case above.
///
/// # Why claiming is two statements
///
/// Exclusion alone does not hold that last case, and a CI run is what said
/// so. The lock answers "is anybody else choosing from this lane right now";
/// the `NOT EXISTS` answers "was anything running in it as of this
/// statement's snapshot" — and under `READ COMMITTED` that snapshot is taken
/// when the statement begins, which may be before the other worker committed
/// the very claim the lock was there to protect. Then the two workers miss
/// each other in both directions: the first holds the lane, so the second's
/// `pg_try_advisory_xact_lock` fails and that row is filtered out — but the
/// filter runs per candidate row, so the next row tries again, by which time
/// the first worker has committed and released, and the lane now looks both
/// free (the lock is gone) and idle (the snapshot predates the claim). The
/// window is not an instant but the length of the second worker's own
/// statement, and every row in the batch is another try, which is why forty
/// tasks in one lane on a loaded runner found it and two on a laptop never
/// did.
///
/// So the lock is taken in one statement and the decision made in the next,
/// inside one transaction: choose the candidates and hold their lanes, then
/// re-check each lane and claim what survives. `READ COMMITTED` takes a fresh
/// snapshot per statement, so the re-check sees everything that committed
/// before we held the lane, and nothing can commit into the lane while we
/// hold it. Exclusion from the lock, freshness from the second snapshot;
/// neither alone is enough.
///
/// The advisory lock is taken in a `WHERE` alongside a `LIMIT`, so Postgres
/// may take one for a row the statement never returns. That costs another
/// worker one tick's sight of that lane and nothing else, since the lock ends
/// with the transaction — as does a candidate the re-check refuses, which is
/// left pending for whoever comes round next.
pub async fn claim_due(worker_id: &str, now: DateTime<Utc>) -> Vec<TaskInvocation> {
    let Some(db) = crate::database::get_global_database() else {
        return Vec::new();
    };

    let mut tx = match db.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            warn!(error = %e, "Failed opening a transaction to claim script tasks");
            return Vec::new();
        }
    };

    // One: the rows this worker means to take, with each lane they belong to
    // held for the rest of the transaction.
    let candidates: Vec<Uuid> = match sqlx::query_scalar(
        r#"
        WITH candidates AS (
            SELECT task_id, script_uri, lane, run_at
            FROM script_tasks AS due
            WHERE run_at <= $1
              AND state IN ('pending', 'running')
              AND (lock_expires_at IS NULL OR lock_expires_at <= $1)
              AND (
                due.lane IS NULL
                OR (
                    NOT EXISTS (
                        SELECT 1
                        FROM script_tasks AS busy
                        WHERE busy.script_uri = due.script_uri
                          AND busy.lane = due.lane
                          AND busy.state = 'running'
                          AND busy.lock_expires_at > $1
                    )
                    AND pg_try_advisory_xact_lock(
                        hashtext(due.script_uri),
                        hashtext(due.lane)
                    )
                )
              )
            ORDER BY run_at ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        SELECT task_id
        FROM (
            SELECT
                task_id,
                lane,
                ROW_NUMBER() OVER (
                    PARTITION BY script_uri, lane
                    ORDER BY run_at ASC, task_id ASC
                ) AS position
            FROM candidates
        ) ranked
        WHERE lane IS NULL OR position = 1
        "#,
    )
    .bind(now)
    .bind(CLAIM_BATCH_SIZE)
    .fetch_all(&mut *tx)
    .await
    {
        Ok(ids) => ids,
        Err(e) => {
            warn!(error = %e, "Failed choosing due script tasks");
            return Vec::new();
        }
    };

    if candidates.is_empty() {
        let _ = tx.rollback().await;
        return Vec::new();
    }

    // Two: with the lanes held, and a snapshot taken after they were, the
    // busy check finally means what it says.
    let rows = match sqlx::query(
        r#"
        UPDATE script_tasks AS tasks
        SET state = 'running',
            locked_by = $3,
            locked_at = $1,
            lock_expires_at = $1 + make_interval(secs => $4),
            claim_epoch = tasks.claim_epoch + 1,
            updated_at = NOW()
        WHERE tasks.task_id = ANY($2)
          AND (
            tasks.lane IS NULL
            OR NOT EXISTS (
                SELECT 1
                FROM script_tasks AS busy
                WHERE busy.script_uri = tasks.script_uri
                  AND busy.lane = tasks.lane
                  AND busy.state = 'running'
                  AND busy.lock_expires_at > $1
            )
          )
        RETURNING tasks.task_id, tasks.script_uri, tasks.handler_name, tasks.payload,
                  tasks.attempts, tasks.max_attempts, tasks.run_as, tasks.claim_epoch,
                  tasks.schedule_id, tasks.scheduled_for,
                  (SELECT schedule_key FROM schedules AS ps
                   WHERE ps.schedule_id = tasks.schedule_id) AS schedule_key
        "#,
    )
    .bind(now)
    .bind(&candidates)
    .bind(worker_id)
    .bind(crate::lease::TTL_SECONDS)
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "Failed claiming due script tasks");
            return Vec::new();
        }
    };

    if let Err(e) = tx.commit().await {
        warn!(error = %e, "Failed committing a claim of due script tasks");
        return Vec::new();
    }

    rows.iter()
        .map(|row| TaskInvocation {
            task_id: row.get("task_id"),
            invocation_id: crate::middleware::generate_request_id(),
            script_uri: row.get("script_uri"),
            handler_name: row.get("handler_name"),
            payload: row.get("payload"),
            attempts: row.get("attempts"),
            max_attempts: row.get("max_attempts"),
            run_as: row.get("run_as"),
            claim_epoch: row.get("claim_epoch"),
            schedule: row
                .get::<Option<Uuid>, _>("schedule_id")
                .map(|schedule_id| ScheduleRun {
                    schedule_id,
                    key: row.get("schedule_key"),
                    scheduled_for: row
                        .get::<Option<DateTime<Utc>>, _>("scheduled_for")
                        .unwrap_or(now),
                }),
        })
        .collect()
}

/// Give up on a task without spending its remaining attempts.
///
/// For a refusal that will not clear by waiting — a withdrawn delegation, an
/// account that is gone. Retrying those would be the engine repeatedly asking
/// to act as somebody who has said no, and the attempts would only delay the
/// row reaching the state that explains itself.
async fn abandon(invocation: &TaskInvocation, reason: &str) {
    let Some(db) = crate::database::get_global_database() else {
        return;
    };

    if let Err(e) = sqlx::query(
        r#"
        UPDATE script_tasks
        SET state = 'failed',
            last_error = $1,
            locked_by = NULL,
            locked_at = NULL,
            lock_expires_at = NULL,
            updated_at = NOW()
        WHERE task_id = $2 AND claim_epoch = $3
        "#,
    )
    .bind(truncate_error(reason))
    .bind(invocation.task_id)
    .bind(invocation.claim_epoch)
    .execute(db.pool())
    .await
    {
        warn!(task = %invocation.task_id, error = %e, "Failed recording an abandoned task");
        return;
    }

    warn!(
        script = %invocation.script_uri,
        handler = %invocation.handler_name,
        task = %invocation.task_id,
        reason,
        "Task abandoned"
    );
    repository::insert_log_message_async_in_context(
        &invocation.script_uri,
        &format!("task '{}' was not run: {}", invocation.handler_name, reason),
        "FATAL",
        &invocation_log_context(invocation),
    )
    .await;
}

/// Record how a run ended.
///
/// Every statement names the claim's epoch, so a claim that has since moved on
/// writes nothing rather than overwriting whatever took it — even when what
/// took it is this same worker. The MCP handle a client may be polling is
/// settled in the same transaction, and only once the guarded write has
/// matched: a run that lost its claim tells nobody anything.
async fn finalize(invocation: &TaskInvocation, outcome: Result<Option<Value>, String>) {
    let Some(db) = crate::database::get_global_database() else {
        return;
    };

    let mut tx = match db.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            warn!(task = %invocation.task_id, error = %e, "Failed opening a transaction to record a task's outcome");
            return;
        }
    };

    let error = match outcome {
        Ok(value) => {
            // Succeeded: the row goes. What it did is in the script's log under
            // this invocation id, and a row per success would grow this table
            // for the one outcome nobody needs to look up.
            let deleted =
                sqlx::query("DELETE FROM script_tasks WHERE task_id = $1 AND claim_epoch = $2")
                    .bind(invocation.task_id)
                    .bind(invocation.claim_epoch)
                    .execute(&mut *tx)
                    .await;
            match deleted {
                Ok(result) if result.rows_affected() == 0 => {
                    let _ = tx.rollback().await;
                    claim_moved_on(invocation);
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(task = %invocation.task_id, error = %e, "Failed clearing a completed task");
                    return;
                }
            }
            if let Err(e) =
                crate::mcp_tasks::finish_on(&mut *tx, invocation.task_id, "completed", value, None)
                    .await
            {
                warn!(task = %invocation.task_id, error = %e, "Failed settling a completed task's handle");
                return;
            }
            if let Some(schedule) = &invocation.schedule
                && let Err(e) =
                    crate::schedules::record_outcome_on(&mut tx, schedule.schedule_id, true).await
            {
                warn!(task = %invocation.task_id, error = %e, "Failed recording a scheduled run's success");
                return;
            }
            if let Err(e) = tx.commit().await {
                warn!(task = %invocation.task_id, error = %e, "Failed recording a completed task");
            }
            return;
        }
        Err(error) => error,
    };

    let attempts = invocation.attempts.saturating_add(1);
    let exhausted = attempts >= invocation.max_attempts;

    // Kept rather than deleted, unlike a success: a failed task is the one
    // somebody needs to read, and `last_error` is what saves them correlating
    // timestamps against the script's log to find out why.
    let statement = if exhausted {
        r#"
        UPDATE script_tasks
        SET state = 'failed',
            attempts = $1,
            last_error = $2,
            locked_by = NULL,
            locked_at = NULL,
            lock_expires_at = NULL,
            updated_at = NOW()
        WHERE task_id = $3 AND claim_epoch = $4
        "#
    } else {
        r#"
        UPDATE script_tasks
        SET state = 'pending',
            attempts = $1,
            last_error = $2,
            run_at = NOW() + make_interval(secs => $5),
            locked_by = NULL,
            locked_at = NULL,
            lock_expires_at = NULL,
            updated_at = NOW()
        WHERE task_id = $3 AND claim_epoch = $4
        "#
    };

    let delay = retry_delay(attempts);
    let mut query = sqlx::query(statement)
        .bind(attempts)
        .bind(truncate_error(&error))
        .bind(invocation.task_id)
        .bind(invocation.claim_epoch);
    if !exhausted {
        query = query.bind(delay.num_seconds());
    }

    match query.execute(&mut *tx).await {
        Ok(result) if result.rows_affected() == 0 => {
            let _ = tx.rollback().await;
            claim_moved_on(invocation);
            return;
        }
        Ok(_) => {}
        Err(e) => {
            warn!(task = %invocation.task_id, error = %e, "Failed recording a task failure");
            return;
        }
    }

    // Only once there is nothing left to try. A client polling a task that is
    // being retried should go on seeing `working`, because that is what is
    // happening — reporting each attempt's failure would make a task that
    // eventually succeeds look like one that failed several times.
    if exhausted
        && let Err(e) = crate::mcp_tasks::finish_on(
            &mut *tx,
            invocation.task_id,
            "failed",
            None,
            Some(serde_json::json!({
                "code": -32603,
                "message": truncate_error(&error),
            })),
        )
        .await
    {
        warn!(task = %invocation.task_id, error = %e, "Failed settling a failed task's handle");
        return;
    }

    // A schedule counts given-up runs, not attempts: a retry that then
    // succeeds is a run that worked.
    if exhausted
        && let Some(schedule) = &invocation.schedule
        && let Err(e) =
            crate::schedules::record_outcome_on(&mut tx, schedule.schedule_id, false).await
    {
        warn!(task = %invocation.task_id, error = %e, "Failed recording a scheduled run's failure");
        return;
    }

    if let Err(e) = tx.commit().await {
        warn!(task = %invocation.task_id, error = %e, "Failed recording a task failure");
        return;
    }

    if exhausted {
        warn!(
            script = %invocation.script_uri,
            handler = %invocation.handler_name,
            task = %invocation.task_id,
            attempts,
            "Task failed every attempt; giving up"
        );
        repository::insert_log_message_async_in_context(
            &invocation.script_uri,
            &format!(
                "task '{}' failed {} times and will not be retried: {}",
                invocation.handler_name, attempts, error
            ),
            "FATAL",
            &invocation_log_context(invocation),
        )
        .await;
    } else {
        debug!(
            task = %invocation.task_id,
            attempts,
            retry_in_seconds = delay.num_seconds(),
            "Task requeued after failure"
        );
    }
}

/// A run finished after its claim was taken from it, so what it reports is
/// not recorded: the row belongs to the claim that took it.
fn claim_moved_on(invocation: &TaskInvocation) {
    warn!(
        script = %invocation.script_uri,
        handler = %invocation.handler_name,
        task = %invocation.task_id,
        epoch = invocation.claim_epoch,
        "Task run lost its claim before it finished; its outcome is not recorded"
    );
}

/// `last_error` is read by a person, not matched on, so a runaway message is
/// trimmed rather than stored whole.
fn truncate_error(error: &str) -> String {
    const MAX: usize = 2000;
    if error.len() <= MAX {
        return error.to_string();
    }
    let mut end = MAX;
    while end > 0 && !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (truncated)", &error[..end])
}

fn invocation_log_context(invocation: &TaskInvocation) -> repository::LogContext {
    js_engine::HandlerInvocationKind::Scheduled.log_context(
        &invocation.script_uri,
        invocation.invocation_id.clone(),
        Some(invocation.handler_name.clone()),
    )
}

/// Run one claimed task, keeping the claim alive for as long as it takes.
async fn run(invocation: TaskInvocation) {
    // Before the slot wait, not after: waiting for a slot is unbounded and is
    // as much a part of holding the task as running it is.
    let lease = crate::lease::spawn_renewal(
        crate::lease::Leased::ScriptTask,
        invocation.task_id,
        invocation.claim_epoch,
        invocation.handler_name.clone(),
    );
    let fence = lease.fence();

    // Who this may act as, worked out now rather than read from the row.
    //
    // A task records only *who*; every capability it gets is re-derived here,
    // so a grant withdrawn between queueing and running takes effect instead of
    // being carried forward. This is the argument `auth::refresh_tokens` makes
    // for minting a fresh session on each refresh, applied to the same problem.
    let delegated = match &invocation.run_as {
        Some(user_id) => {
            match crate::delegation::resolve(user_id, &invocation.script_uri).await {
                Ok(delegated) => Some(delegated),
                Err(refusal) => {
                    // Not a retry. A withdrawn or lapsed grant is not a
                    // condition that clears on its own, and going on trying
                    // would be the engine repeatedly asking to act as somebody
                    // who has said no.
                    drop(lease);
                    abandon(&invocation, &refusal.to_string()).await;
                    return;
                }
            }
        }
        None => None,
    };

    let permit = crate::execution_slots::acquire().await;
    let for_engine = invocation.clone();
    let fence_for_engine = fence.clone();
    let execution = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        js_engine::execute_task_handler(&for_engine, delegated.as_ref(), Some(fence_for_engine))
    })
    .await;

    drop(lease);

    let outcome = match execution {
        // Whatever the run answered, it was cut short or overtaken.
        _ if fence.is_raised() => {
            Err("the run lost its claim on the task and was stopped".to_string())
        }
        Ok(result) => result,
        Err(join_error) => Err(format!("task panicked: {}", join_error)),
    };

    if let Err(error) = &outcome {
        error!(
            script = %invocation.script_uri,
            handler = %invocation.handler_name,
            task = %invocation.task_id,
            error = %error,
            "Task failed"
        );
    }

    finalize(&invocation, outcome).await;
}

/// Claim every task that is due now and run them to completion.
///
/// The worker's tick without the concurrency: it awaits each run rather than
/// spawning it. For a caller that needs the queue drained before it looks at
/// the result — a test, or an operator draining before a shutdown — where the
/// worker's own loop is deliberately fire-and-forget.
///
/// Returns how many ran.
pub async fn run_due_now(worker_id: &str) -> usize {
    let claimed = claim_due(worker_id, Utc::now()).await;
    let count = claimed.len();
    for invocation in claimed {
        run(invocation).await;
    }
    count
}

/// The background worker. One per instance; several instances share the queue
/// through the claim above.
pub async fn worker(worker_id: String, mut shutdown: oneshot::Receiver<()>) {
    info!(worker = %worker_id, "Task worker started");

    loop {
        // Schedules first, so a slot that is due now is claimed in this tick.
        crate::schedules::fire_due(Utc::now()).await;

        for invocation in claim_due(&worker_id, Utc::now()).await {
            tokio::spawn(run(invocation));
        }

        tokio::select! {
            _ = tokio::time::sleep(StdDuration::from_millis(POLL_INTERVAL_MS)) => {}
            _ = wake_signal().notified() => {}
            _ = &mut shutdown => {
                info!("Task worker shutting down");
                break;
            }
        }
    }
}

/// Start the worker. Called once at startup, beside the scheduler's.
pub fn spawn_worker(worker_id: String, shutdown: oneshot::Receiver<()>) {
    tokio::spawn(worker(worker_id, shutdown));
}

/// What a task looks like to JavaScript and to the engine API.
pub fn to_json(task: &Task) -> Value {
    serde_json::json!({
        "taskId": task.task_id.to_string(),
        "script": task.script_uri,
        "handler": task.handler_name,
        "payload": task.payload,
        "state": task.state,
        "attempts": task.attempts,
        "maxAttempts": task.max_attempts,
        "lastError": task.last_error,
        "runAt": task.run_at.to_rfc3339(),
        "enqueuedBy": task.enqueued_by,
        "runAs": task.run_as,
        "lane": task.lane,
        "createdAt": task.created_at.to_rfc3339(),
        "updatedAt": task.updated_at.to_rfc3339(),
    })
}

/// The blocking face of this module, for the JavaScript host bindings.
///
/// A host call is synchronous — the script is stopped inside QuickJS while it
/// runs — so these drive the async work the way every other sandboxed database
/// call does.
///
/// Inside a script's transaction they join it, as every `database` call does.
/// A task enqueued there commits with the writes it was queued about and is
/// gone if they roll back; no worker can see it before the commit, so it never
/// runs ahead of the data it reads. The async functions above stay on the
/// pool: they run on runtime threads, where the thread's transaction is not
/// theirs.
pub mod blocking {
    use super::*;
    use crate::database::{TransactionExecutor, get_current_executor};

    pub fn enqueue(task: NewTask) -> Result<Task, EnqueueError> {
        let Some(db) = crate::database::get_global_database() else {
            return Err(EnqueueError::Unavailable);
        };
        let task = crate::database::run_blocking(async {
            match get_current_executor(db.pool()) {
                TransactionExecutor::Transaction(tx) => insert(&mut **tx, task).await,
                TransactionExecutor::Pool(pool) => insert(pool, task).await,
            }
        })?;
        // A worker woken before the commit would find nothing and wait out a
        // poll interval.
        let wake = || wake_signal().notify_waiters();
        if let Err(wake) = crate::database::defer_until_commit(wake) {
            wake();
        }
        Ok(task)
    }

    pub fn cancel(task_id: Uuid) -> Result<bool, sqlx::Error> {
        let Some(db) = crate::database::get_global_database() else {
            return Ok(false);
        };
        crate::database::run_blocking(async {
            match get_current_executor(db.pool()) {
                TransactionExecutor::Transaction(tx) => cancel_on(&mut **tx, task_id).await,
                TransactionExecutor::Pool(pool) => cancel_on(pool, task_id).await,
            }
        })
    }

    pub fn get(task_id: Uuid) -> Result<Option<Task>, sqlx::Error> {
        let Some(db) = crate::database::get_global_database() else {
            return Ok(None);
        };
        crate::database::run_blocking(async {
            match get_current_executor(db.pool()) {
                TransactionExecutor::Transaction(tx) => get_on(&mut **tx, task_id).await,
                TransactionExecutor::Pool(pool) => get_on(pool, task_id).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_task() -> NewTask {
        NewTask {
            script_uri: "test://tasks".to_string(),
            handler_name: "handle".to_string(),
            payload: serde_json::json!({ "id": 1 }),
            run_at: None,
            max_attempts: None,
            enqueued_by: None,
            run_as: None,
            lane: None,
        }
    }

    /// An empty lane is no lane rather than an error: a script gets `""`
    /// from an interpolation whose variable was empty, and that plainly
    /// means unconstrained rather than "refuse this enqueue".
    #[test]
    fn an_empty_lane_is_no_lane() {
        assert_eq!(normalize_lane(None), None);
        assert_eq!(normalize_lane(Some("")), None);
        assert_eq!(normalize_lane(Some("   ")), None);
        assert_eq!(normalize_lane(Some("  inbox  ")), Some("inbox".to_string()));
    }

    /// Truncated rather than refused, and the direction matters: losing the
    /// tail still serialises everything sharing the prefix, which errs
    /// toward running less at once rather than more.
    #[test]
    fn an_over_long_lane_is_truncated_rather_than_refused() {
        let long = "l".repeat(MAX_LANE_CHARS + 50);
        let normalized = normalize_lane(Some(&long)).expect("still a lane");
        assert_eq!(normalized.chars().count(), MAX_LANE_CHARS);
    }

    #[test]
    fn a_handler_name_is_required() {
        let mut task = a_task();
        task.handler_name = "   ".to_string();
        assert_eq!(validate(&task), Err(EnqueueError::MissingHandler));
    }

    #[test]
    fn a_handler_name_is_bounded() {
        let mut task = a_task();
        task.handler_name = "h".repeat(MAX_HANDLER_NAME_CHARS + 1);
        assert_eq!(validate(&task), Err(EnqueueError::InvalidHandler));
    }

    #[test]
    fn a_handler_name_is_trimmed_rather_than_refused() {
        let mut task = a_task();
        task.handler_name = "  handle  ".to_string();
        let (_, handler) = validate(&task).expect("surrounding space is not an error");
        assert_eq!(handler, "handle");
    }

    /// A handler reads named fields, so a bare string or array is a caller who
    /// meant to wrap it rather than a payload the handler can use.
    #[test]
    fn a_payload_must_be_an_object() {
        let mut task = a_task();
        task.payload = serde_json::json!("just a string");
        assert_eq!(validate(&task), Err(EnqueueError::PayloadNotAnObject));

        task.payload = serde_json::json!([1, 2, 3]);
        assert_eq!(validate(&task), Err(EnqueueError::PayloadNotAnObject));
    }

    #[test]
    fn a_payload_is_bounded() {
        let mut task = a_task();
        task.payload = serde_json::json!({ "blob": "x".repeat(MAX_PAYLOAD_BYTES) });
        assert_eq!(validate(&task), Err(EnqueueError::PayloadTooLarge));
    }

    #[test]
    fn attempts_are_bounded_in_both_directions() {
        let mut task = a_task();
        task.max_attempts = Some(0);
        assert_eq!(validate(&task), Err(EnqueueError::InvalidMaxAttempts));

        task.max_attempts = Some(ATTEMPTS_LIMIT + 1);
        assert_eq!(validate(&task), Err(EnqueueError::InvalidMaxAttempts));

        task.max_attempts = Some(1);
        assert!(validate(&task).is_ok());
    }

    #[test]
    fn the_default_attempt_count_applies_when_none_is_asked_for() {
        let (max_attempts, _) = validate(&a_task()).expect("the task is valid");
        assert_eq!(max_attempts, DEFAULT_MAX_ATTEMPTS);
    }

    #[test]
    fn the_retry_delay_doubles_and_then_stops_growing() {
        assert_eq!(retry_delay(1), Duration::seconds(RETRY_BASE_SECONDS));
        assert_eq!(retry_delay(2), Duration::seconds(RETRY_BASE_SECONDS * 2));
        assert_eq!(retry_delay(3), Duration::seconds(RETRY_BASE_SECONDS * 4));
        assert_eq!(retry_delay(i32::MAX), Duration::seconds(RETRY_MAX_SECONDS));
    }

    #[test]
    fn the_retry_delay_is_never_zero() {
        for attempts in 0..=ATTEMPTS_LIMIT {
            assert!(retry_delay(attempts) > Duration::zero());
        }
    }

    /// The message is trimmed on a character boundary, so a failure carrying
    /// multi-byte text does not panic the worker recording it.
    #[test]
    fn a_long_error_is_truncated_without_splitting_a_character() {
        let error = "é".repeat(4000);
        let truncated = truncate_error(&error);
        assert!(truncated.len() < error.len());
        assert!(truncated.ends_with("… (truncated)"));
    }

    #[test]
    fn a_short_error_is_kept_as_it_is() {
        assert_eq!(truncate_error("no such handler"), "no such handler");
    }

    /// The claim's epoch, against the database: a run whose task was claimed
    /// again — by the same worker, which is what `locked_by` could not tell
    /// apart — records nothing about how it ended, and settles nothing a
    /// client is polling.
    mod claims {
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

        async fn state_of(pool: &sqlx::PgPool, task_id: Uuid) -> Option<(String, i32, i64)> {
            sqlx::query("SELECT state, attempts, claim_epoch FROM script_tasks WHERE task_id = $1")
                .bind(task_id)
                .fetch_optional(pool)
                .await
                .expect("the lookup should run")
                .map(|row| {
                    (
                        row.get("state"),
                        row.get("attempts"),
                        row.get("claim_epoch"),
                    )
                })
        }

        async fn handle_status(pool: &sqlx::PgPool, task_id: Uuid) -> String {
            sqlx::query_scalar("SELECT status FROM mcp_tasks WHERE script_task_id = $1")
                .bind(task_id)
                .fetch_one(pool)
                .await
                .expect("the handle should be there")
        }

        #[tokio::test]
        async fn a_run_that_lost_its_claim_records_nothing() {
            let pool = use_the_test_database();
            let script_uri = "test://tasks/claims";
            sqlx::query("INSERT INTO scripts (uri, name) VALUES ($1, $1) ON CONFLICT DO NOTHING")
                .bind(script_uri)
                .execute(&pool)
                .await
                .expect("the script row should insert");

            let task = enqueue(NewTask {
                script_uri: script_uri.to_string(),
                ..a_task()
            })
            .await
            .expect("the task should be accepted");
            crate::mcp_tasks::create(
                Uuid::new_v4(),
                task.task_id,
                script_uri,
                "tools/call",
                "work",
                None,
            )
            .await
            .expect("the handle should be recorded");

            let stale = claim_due("worker", Utc::now()).await.remove(0);

            // The claim lapses, and the same worker takes the task again.
            sqlx::query(
                "UPDATE script_tasks SET lock_expires_at = NOW() - INTERVAL '1 second' \
                 WHERE task_id = $1",
            )
            .bind(task.task_id)
            .execute(&pool)
            .await
            .expect("the lease should lapse");
            let current = claim_due("worker", Utc::now()).await.remove(0);
            assert!(current.claim_epoch > stale.claim_epoch);

            finalize(&stale, Ok(None)).await;
            finalize(&stale, Err("too late".to_string())).await;
            assert_eq!(
                state_of(&pool, task.task_id).await,
                Some(("running".to_string(), 0, current.claim_epoch)),
                "the stale run should neither delete nor requeue the task"
            );
            assert_eq!(handle_status(&pool, task.task_id).await, "working");

            finalize(&current, Ok(Some(serde_json::json!(42)))).await;
            assert_eq!(state_of(&pool, task.task_id).await, None);
            assert_eq!(handle_status(&pool, task.task_id).await, "completed");
        }
    }
}
