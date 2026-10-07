//! A script's log: writing lines under an invocation, querying them, and pruning.

use super::*;
use crate::error::{AppError, AppResult};
use crate::log_retention::LogRetention;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::time::SystemTime;
use tracing::{debug, error};

/// Advisory lock key for the log pruning pass, so that in a multi-instance
/// cluster one instance does the work on any given tick.
pub(super) const LOG_PRUNE_LOCK: &str = "log-prune";

/// Log entry with timestamp information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// Script the message was logged by. Callers that fetch logs across every
    /// script rely on this to attribute each entry.
    pub script_uri: String,
    pub message: String,
    pub level: String,
    pub timestamp: SystemTime,
    /// Monotonic write order. Breaks `timestamp` ties, and doubles as the
    /// cursor a caller pages or tails from. Zero for entries that were never
    /// read back from the database (error placeholders).
    pub seq: i64,
    /// Which invocation emitted this line, when one did. See [`LogContext`].
    pub context: LogContext,
}

impl LogEntry {
    pub fn new(script_uri: String, message: String, level: String, timestamp: SystemTime) -> Self {
        Self {
            script_uri,
            message,
            level,
            timestamp,
            seq: 0,
            context: LogContext::default(),
        }
    }

    /// Attach the invocation this entry was emitted by.
    pub fn with_context(mut self, seq: i64, context: LogContext) -> Self {
        self.seq = seq;
        self.context = context;
        self
    }
}

/// Identifies the invocation a log line was emitted by.
///
/// A script's output is otherwise one undifferentiated stream: the lines from a
/// route call, a scheduler tick and a stream connection interleave and cannot be
/// separated again. Carrying this down to each write is what lets a caller ask
/// for "the lines this one request produced" or "everything this route logged".
///
/// Every field is optional because engine-internal writes — startup, transpiler
/// diagnostics — have no invocation to name.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogContext {
    /// Groups the lines one invocation emitted. For HTTP this is the request's
    /// `x-request-id`, so a client holding the response header can ask for
    /// exactly the lines its own call produced; other invocation kinds generate
    /// one per run.
    pub request_id: Option<String>,
    /// What sort of invocation this was: `httpRoute`, `scheduled`,
    /// `streamCustomization`, … — the names
    /// `js_engine::HandlerInvocationKind` uses.
    pub kind: Option<String>,
    /// The registered route pattern (`/things/:id`), not the concrete path, so
    /// that filtering by it aggregates every call to the same handler. For
    /// invocations that are not HTTP routes this names the job, stream or tool.
    pub route: Option<String>,
    /// Which revision of the script was running.
    ///
    /// Captured once when the invocation starts rather than read per line: a
    /// long-running handler can span a write, and every line it produced came
    /// from the version it started under.
    pub revision: Option<i32>,
}

impl LogContext {
    /// True when this context names nothing, i.e. there is no invocation to
    /// attribute the line to.
    pub fn is_empty(&self) -> bool {
        // `revision` is deliberately not part of this. It says which version
        // wrote the line, not which invocation did, so a context carrying only
        // a revision still names no invocation.
        self.request_id.is_none() && self.kind.is_none() && self.route.is_none()
    }
}

/// Filters for a log query. Every field is optional; `None` means "no filter".
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Restrict to one script; `None` spans every script.
    pub script_uri: Option<String>,
    /// Restrict to one log level, e.g. `ERROR`. Matched case-insensitively.
    pub level: Option<String>,
    /// Keep only entries logged at or after this instant.
    pub since: Option<SystemTime>,
    /// Keep only entries written after this [`LogEntry::seq`]. Used to page or
    /// tail forward without re-reading what the caller already has, and unlike
    /// `since` it cannot repeat or skip entries that share a timestamp. Set
    /// with `limit`, it takes the *oldest* entries past the cursor, so reading
    /// forward one page at a time cannot skip what falls between pages.
    pub after_seq: Option<i64>,
    /// Keep only entries whose message contains this substring, matched
    /// case-insensitively.
    pub contains: Option<String>,
    /// Keep only the entries one invocation emitted. See
    /// [`LogContext::request_id`].
    pub request_id: Option<String>,
    /// Keep only entries from invocations of this kind, e.g. `scheduled`.
    /// Matched case-insensitively.
    pub kind: Option<String>,
    /// Keep only entries logged while serving this registered route pattern.
    pub route: Option<String>,
    /// Keep only entries written while this revision of the script was
    /// running. What makes "the errors started at revision 41" a query rather
    /// than a wall-clock comparison against a deploy time.
    pub revision: Option<i32>,
    /// Keep at most this many of the *newest* matching entries.
    pub limit: Option<i64>,
    /// Keep only entries of scripts this account owns. Set by the
    /// authorization layer for a caller who is not an administrator, so a
    /// query spanning "every script" spans every script *of theirs*.
    pub owned_by: Option<String>,
}

impl LogQuery {
    /// All logs for one script, unfiltered.
    pub fn for_uri(script_uri: &str) -> Self {
        Self {
            script_uri: Some(script_uri.to_string()),
            ..Self::default()
        }
    }
}

/// Database-backed insert log message
pub(super) async fn db_insert_log_message<'e, E>(
    executor: E,
    script_uri: &str,
    message: &str,
    log_level: &str,
    context: &LogContext,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        // `clock_timestamp()`, not `NOW()`: `NOW()` is the *transaction's*
        // start time, so every line a script wrote inside one transaction —
        // which is how `/engine/eval_script` and the test runner execute — would carry
        // the same timestamp and lose its order.
        r#"
        INSERT INTO logs (script_uri, message, log_level, created_at, request_id, kind, route, revision)
        VALUES ($1, $2, $3, clock_timestamp(), $4, $5, $6, $7)
        "#,
    )
    .bind(script_uri)
    .bind(message)
    .bind(log_level)
    .bind(context.request_id.as_deref())
    .bind(context.kind.as_deref())
    .bind(context.route.as_deref())
    .bind(context.revision)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error inserting log message: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Inserted log message to database for script: {}",
        script_uri
    );
    Ok(())
}

/// Database-backed log query. Filters are applied in SQL so callers never pull
/// the whole table just to throw most of it away, and `limit` keeps the
/// *newest* matching entries — rows come back newest-first.
///
/// With `after_seq` the limit works the other way round, keeping the *oldest*
/// entries past the cursor: a caller reading forward wants the next page after
/// what it has, and keeping the newest would silently skip everything in
/// between. Rows still come back newest-first either way.
pub(super) async fn db_query_log_messages<'e, E>(
    executor: E,
    query: &LogQuery,
) -> AppResult<Vec<LogEntry>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    // Levels are stored upper-case; normalise so `?level=error` matches.
    let level = query.level.as_ref().map(|l| l.to_uppercase());
    let since = query.since.map(DateTime::<Utc>::from);
    // `contains` is a literal substring, not a pattern: escape what LIKE would
    // otherwise read as wildcards so searching for `a_b` or `100%` works.
    let contains = query.contains.as_ref().map(|needle| {
        format!(
            "%{}%",
            needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });

    // A NULL bind disables the corresponding filter, and `LIMIT NULL` means
    // "no limit" in Postgres, so one statement covers every combination.
    let rows = sqlx::query(
        // `seq` breaks `created_at` ties, so a listing has one stable order
        // rather than an arbitrary one per query — which is what makes paging
        // and tailing by `seq` reliable.
        r#"
        WITH matching AS (
            SELECT script_uri, message, log_level, created_at, seq, request_id, kind, route,
                   revision
            FROM logs
            WHERE ($1::text IS NULL OR script_uri = $1)
              AND ($2::text IS NULL OR log_level = $2)
              AND ($3::timestamptz IS NULL OR created_at >= $3)
              AND ($4::text IS NULL OR message ILIKE $4)
              AND ($5::text IS NULL OR request_id = $5)
              AND ($6::text IS NULL OR lower(kind) = lower($6))
              AND ($7::text IS NULL OR route = $7)
              AND ($8::bigint IS NULL OR seq > $8)
              AND ($10::int4 IS NULL OR revision = $10)
              AND ($11::text IS NULL OR script_uri IN (
                    SELECT script_uri FROM script_owners WHERE user_id = $11))
            -- Without a cursor the constant leaves the ordering to the keys
            -- that follow, so the limit keeps the newest entries; with one it
            -- orders oldest-first, so the limit keeps the next page instead.
            ORDER BY
              CASE WHEN $8::bigint IS NULL THEN 0 ELSE seq END ASC,
              created_at DESC,
              seq DESC
            LIMIT $9::bigint
        )
        SELECT * FROM matching ORDER BY created_at DESC, seq DESC
        "#,
    )
    .bind(query.script_uri.as_deref())
    .bind(level.as_deref())
    .bind(since)
    .bind(contains.as_deref())
    .bind(query.request_id.as_deref())
    .bind(query.kind.as_deref())
    .bind(query.route.as_deref())
    .bind(query.after_seq)
    .bind(query.limit)
    .bind(query.revision)
    .bind(query.owned_by.as_deref())
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error fetching log messages: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let messages = rows
        .into_iter()
        .map(|row| {
            let script_uri: String = row.try_get("script_uri")?;
            let message: String = row.try_get("message")?;
            let log_level: String = row.try_get("log_level")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            let seq: i64 = row.try_get("seq")?;
            let context = LogContext {
                request_id: row.try_get("request_id")?,
                kind: row.try_get("kind")?,
                route: row.try_get("route")?,
                revision: row.try_get("revision")?,
            };
            // Convert chrono DateTime to SystemTime
            let system_time = SystemTime::from(created_at);
            Ok(LogEntry::new(script_uri, message, log_level, system_time)
                .with_context(seq, context))
        })
        .collect::<Result<Vec<LogEntry>, sqlx::Error>>()
        .map_err(|e| {
            error!("Database error getting message/level/timestamp: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    Ok(messages)
}

/// Database-backed fetch log messages for a script, oldest first.
pub(super) async fn db_fetch_log_messages<'e, E>(
    executor: E,
    script_uri: &str,
) -> AppResult<Vec<LogEntry>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let mut messages = db_query_log_messages(executor, &LogQuery::for_uri(script_uri)).await?;
    messages.reverse();
    Ok(messages)
}

/// Database-backed fetch all log messages, newest first.
pub(super) async fn db_fetch_all_log_messages<'e, E>(executor: E) -> AppResult<Vec<LogEntry>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    db_query_log_messages(executor, &LogQuery::default()).await
}

/// Database-backed clear log messages for a script
pub(super) async fn db_clear_log_messages<'e, E>(executor: E, script_uri: &str) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        r#"
        DELETE FROM logs WHERE script_uri = $1
        "#,
    )
    .bind(script_uri)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error clearing log messages: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Cleared log messages from database for script: {}",
        script_uri
    );
    Ok(())
}

/// Database-backed prune log messages (keep only latest 20 per script)
pub(super) async fn db_prune_log_messages(
    pool: &PgPool,
    retention: LogRetention,
) -> AppResult<u64> {
    // Both dials are enforced, and either one alone is enough to delete a
    // line: the count bounds a script that logs in a loop, and the age bounds
    // the thousand dormant scripts the count would let sit on their quota
    // forever. Revisions deliberately require both, because a revision is
    // history someone may still need; a log line is a diagnostic.
    //
    // Recency is by `seq`, not `created_at`: two lines written in the same
    // microsecond tie on the timestamp and the tie breaks differently on every
    // query, so a count-based cut would keep a different hundred each pass.
    // `seq` is also what the log view orders by, so "the newest hundred" means
    // the same hundred here and there.
    let mut tx = pool.begin().await.map_err(|e| {
        error!("Database error starting log prune: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    // One instance prunes per tick. The delete is idempotent, so two at once
    // would be correct but would scan the same rows to reach the same answer.
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1))")
        .bind(LOG_PRUNE_LOCK)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            error!("Database error locking for log prune: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    if !acquired {
        debug!("Another instance is pruning logs; skipping this pass");
        return Ok(0);
    }

    let deleted = sqlx::query(
        r#"
        WITH ranked AS (
            SELECT id,
                   created_at,
                   row_number() OVER (
                       PARTITION BY script_uri ORDER BY seq DESC
                   ) AS recency
            FROM logs
        )
        DELETE FROM logs
        WHERE id IN (
            SELECT id FROM ranked
            WHERE recency > $1
               OR ($2 > 0 AND created_at < now() - make_interval(hours => $2))
        )
        "#,
    )
    .bind(retention.keep_per_script)
    .bind(retention.keep_hours)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        error!("Database error pruning log messages: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?
    .rows_affected();

    tx.commit().await.map_err(|e| {
        error!("Database error committing log prune: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if deleted > 0 {
        debug!(
            deleted = deleted,
            keep_per_script = retention.keep_per_script,
            keep_hours = retention.keep_hours,
            "Pruned log messages"
        );
    }

    Ok(deleted)
}

/// Insert log message with error handling
pub fn insert_log_message(script_uri: &str, message: &str, log_level: &str) {
    insert_log_message_in_context(script_uri, message, log_level, &LogContext::default())
}

/// Insert a log message attributed to the invocation that emitted it.
///
/// Engine-internal writes keep using [`insert_log_message`]; a line written on
/// behalf of a script's `console` should come through here so it can be traced
/// back to the request, tick or connection it belongs to.
pub fn insert_log_message_in_context(
    script_uri: &str,
    message: &str,
    log_level: &str,
    context: &LogContext,
) {
    run_blocking(insert_log_message_async_in_context(
        script_uri, message, log_level, context,
    ))
}

/// Async variant of [`insert_log_message`] for callers already in async context
pub async fn insert_log_message_async(script_uri: &str, message: &str, log_level: &str) {
    insert_log_message_async_in_context(script_uri, message, log_level, &LogContext::default())
        .await
}

/// Async variant of [`insert_log_message_in_context`].
pub async fn insert_log_message_async_in_context(
    script_uri: &str,
    message: &str,
    log_level: &str,
    context: &LogContext,
) {
    let repo = get_repository();
    if let Err(e) = repo
        .insert_log(script_uri, message, log_level, context)
        .await
    {
        error!(
            "Failed to insert log message for {}: {}. Message: {}",
            script_uri, e, message
        );
        // Log to system instead as fallback
        error!("FALLBACK LOG [{}]: {}", script_uri, message);
    }
}

/// Fetch log messages with error handling
pub fn fetch_log_messages(script_uri: &str) -> Vec<LogEntry> {
    let repo = get_repository();
    let result = run_bounded(async { repo.fetch_logs(script_uri).await });

    match result {
        Ok(messages) => messages,
        Err(e) => {
            error!("Failed to fetch log messages for {}: {}", script_uri, e);
            let now = SystemTime::now();
            vec![LogEntry::new(
                script_uri.to_string(),
                format!("Error: Could not retrieve logs - {}", e),
                "ERROR".to_string(),
                now,
            )]
        }
    }
}

/// Fetch ALL log messages from all script URIs
pub fn fetch_all_log_messages() -> Vec<LogEntry> {
    // Try database first if configured
    let repo = get_repository();
    let result = run_bounded(async { repo.fetch_all_logs().await });

    match result {
        Ok(messages) => messages,
        Err(e) => {
            error!("Failed to fetch all log messages: {}", e);
            vec![LogEntry::new(
                String::new(),
                format!("Error: Could not retrieve logs - {}", e),
                "ERROR".to_string(),
                SystemTime::now(),
            )]
        }
    }
}

/// Fetch log messages matching `query`, newest first.
///
/// Unlike [`fetch_log_messages`] this surfaces database errors instead of
/// folding them into a synthetic entry, so HTTP callers can answer with a real
/// status code.
pub fn query_log_messages(query: &LogQuery) -> AppResult<Vec<LogEntry>> {
    let repo = get_repository();
    run_bounded(async { repo.query_logs(query).await })
}

/// Clear log messages for a script
pub fn clear_log_messages(script_uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.clear_logs(script_uri).await })
}

/// Applies `retention` to every script's log, returning the number of lines
/// removed.
///
/// The async form is what the background pruner uses; the blocking wrapper is
/// for callers already on a blocking thread, which must not block on a runtime
/// they are running inside.
pub async fn prune_log_messages_async(retention: LogRetention) -> AppResult<u64> {
    let Some(repo) = get_repository_opt() else {
        return Ok(0);
    };
    repo.prune_logs(retention).await
}

/// The same, for a blocking caller.
pub fn prune_log_messages(retention: LogRetention) -> AppResult<u64> {
    let repo = get_repository();
    run_bounded(async move { repo.prune_logs(retention).await })
}
