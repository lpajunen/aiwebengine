//! What one script may spend, when that differs from what the engine allows
//! everyone.
//!
//! `javascript.execution_timeout_ms` and its neighbours are process-wide, and
//! that is the problem: an operator hosting one agent beside twenty ordinary
//! solutions had to raise the ceiling for all of them. The number that lets an
//! agent wait out a model call is also the number a runaway route handler now
//! gets, so making one solution work made every other one worse.
//!
//! An override is per script and useful in both directions. Raising is the
//! obvious one. Lowering is the one that gets used: a script that has started
//! holding execution slots can be contained without restarting the engine or
//! touching anyone else.
//!
//! # Who may set one
//!
//! An administrator, and deliberately not the script's owner. Ownership is the
//! right test for changing what a script *does*; this is a claim on the
//! engine's execution slots, its threads and its memory, which are shared with
//! every other tenant. A script owner who could raise their own ceiling would
//! be back to one script setting the policy for all of them, which is the thing
//! this exists to stop.
//!
//! # Why it is cached
//!
//! Read on every execution, so it cannot be a query — the same reasoning
//! [`crate::deployments`] gives for its pins, and the cache is built the same
//! way: loaded once at startup before anything runs, and refreshed through
//! `notifications` when a peer changes one.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use sqlx::Row;

use crate::error::{AppError, AppResult};
use crate::js_engine::ExecutionLimits;

/// The longest a request-shaped invocation may be allowed to run.
///
/// A ceiling on the override rather than on the engine's own setting: the
/// stored value takes effect without a restart, so a mistyped one would hold a
/// slot until somebody noticed. An administrator who genuinely wants longer
/// than this is asking for a job, which has its own budget.
pub const MAX_TIMEOUT_MS: u64 = 10 * 60 * 1000;

/// The longest a scheduled job or queued task may be allowed to run.
///
/// Wider than a request's, and still bounded. Work that needs longer wants
/// splitting across tasks, which survives a restart where one long run does
/// not — see `docs/SCRIPT_TASKS.md`.
pub const MAX_JOB_TIMEOUT_MS: u64 = 60 * 60 * 1000;

/// The largest heap one script's runtime may be given.
pub const MAX_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The smallest anything may be set to.
///
/// Zero would not be a limit, it would be a script that cannot run, and
/// "disable this script" is deleting it or unbinding its host rather than
/// giving it an impossible budget.
pub const MIN_TIMEOUT_MS: u64 = 100;
pub const MIN_MEMORY_BYTES: u64 = 8 * 1024 * 1024;

/// One script's overrides. Every field is optional and `None` means "whatever
/// the engine allows", so a row can raise a timeout without also restating a
/// memory ceiling it does not care about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub timeout_ms: Option<u64>,
    pub job_timeout_ms: Option<u64>,
    pub max_memory_bytes: Option<u64>,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.timeout_ms.is_none()
            && self.job_timeout_ms.is_none()
            && self.max_memory_bytes.is_none()
    }

    /// Bring every value inside the bounds above.
    ///
    /// Clamped rather than refused, for the values that are merely out of
    /// range: the caller asked for "as much as possible" and gets it. A value
    /// that is not a number at all is refused by the parser instead.
    pub fn clamped(self) -> Self {
        Self {
            timeout_ms: self
                .timeout_ms
                .map(|ms| ms.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS)),
            job_timeout_ms: self
                .job_timeout_ms
                .map(|ms| ms.clamp(MIN_TIMEOUT_MS, MAX_JOB_TIMEOUT_MS)),
            max_memory_bytes: self
                .max_memory_bytes
                .map(|bytes| bytes.clamp(MIN_MEMORY_BYTES, MAX_MEMORY_BYTES)),
        }
    }
}

/// A stored row, with the note saying why it is there.
#[derive(Debug, Clone)]
pub struct ScriptLimits {
    pub script_uri: String,
    pub overrides: Overrides,
    pub note: Option<String>,
    pub set_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn row_to_limits(row: &sqlx::postgres::PgRow) -> ScriptLimits {
    let as_u64 = |name: &str| row.get::<Option<i64>, _>(name).map(|v| v.max(0) as u64);
    ScriptLimits {
        script_uri: row.get("script_uri"),
        overrides: Overrides {
            timeout_ms: as_u64("timeout_ms"),
            job_timeout_ms: as_u64("job_timeout_ms"),
            max_memory_bytes: as_u64("max_memory_bytes"),
        },
        note: row.get("note"),
        set_by: row.get("set_by"),
        updated_at: row.get("updated_at"),
    }
}

fn pool() -> AppResult<sqlx::PgPool> {
    crate::repository::get_db_pool()
        .map(|db| db.pool().clone())
        .ok_or_else(|| AppError::Database {
            message: "No database configured".to_string(),
            source: None,
        })
}

// ============================================================================
// What this instance believes
// ============================================================================

static OVERRIDES: OnceLock<RwLock<HashMap<String, Overrides>>> = OnceLock::new();

fn overrides_map() -> &'static RwLock<HashMap<String, Overrides>> {
    OVERRIDES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Whether any script has an override at all.
///
/// Asked before the map lookup, so an engine where nobody overrides anything
/// pays one atomic read per execution rather than hashing a URI. The same
/// shortcut `deployments::any_pinned` takes, and for the same reason: this is
/// on the path of every script execution in the engine.
pub fn any_overrides() -> bool {
    overrides_map()
        .read()
        .map(|map| !map.is_empty())
        .unwrap_or(false)
}

/// What `script_uri` may spend, as far as this instance knows.
pub fn overrides_for(script_uri: &str) -> Overrides {
    if !any_overrides() {
        return Overrides::default();
    }
    overrides_map()
        .read()
        .ok()
        .and_then(|map| map.get(script_uri).cloned())
        .unwrap_or_default()
}

/// The limits one script's request-shaped execution runs under.
///
/// The engine's configured limits with this script's overrides laid over them,
/// so a script with no row gets exactly what it got before this existed.
pub fn for_script(script_uri: &str) -> ExecutionLimits {
    let base = crate::js_engine::current_execution_limits();
    apply(base, &overrides_for(script_uri), false)
}

/// The limits one script's *background* execution runs under — a scheduled job
/// or a queued task.
///
/// Differs from [`for_script`] in which timeout it takes: a job is not
/// answering a request, so it is held to the job budget, overridden per script
/// or otherwise the engine's `javascript.job_timeout_ms`.
pub fn for_script_job(script_uri: &str) -> ExecutionLimits {
    let base = crate::js_engine::current_execution_limits();
    apply(base, &overrides_for(script_uri), true)
}

/// The wall clock a script's background work gets, which the scheduler also
/// needs on its own to size nothing else — it is the number the runtime is
/// built with.
pub fn job_timeout_ms_for(script_uri: &str) -> u64 {
    overrides_for(script_uri)
        .job_timeout_ms
        .unwrap_or_else(crate::scheduler::configured_job_timeout_ms)
}

fn apply(base: ExecutionLimits, overrides: &Overrides, background: bool) -> ExecutionLimits {
    let timeout_ms = if background {
        overrides
            .job_timeout_ms
            .unwrap_or_else(crate::scheduler::configured_job_timeout_ms)
    } else {
        overrides.timeout_ms.unwrap_or(base.timeout_ms)
    };

    ExecutionLimits {
        timeout_ms,
        max_memory_mb: overrides
            .max_memory_bytes
            .map(|bytes| (bytes / (1024 * 1024)).max(1) as usize)
            .unwrap_or(base.max_memory_mb),
        ..base
    }
}

/// Fill the cache before anything executes.
///
/// At startup, ahead of the first script running, so an instance coming up does
/// not briefly run somebody's contained script at the engine's own ceiling.
pub async fn load() {
    let Ok(pool) = pool() else {
        return;
    };
    match sqlx::query(
        "SELECT script_uri, timeout_ms, job_timeout_ms, max_memory_bytes, note, set_by, updated_at \
         FROM script_limits",
    )
    .fetch_all(&pool)
    .await
    {
        Ok(rows) => {
            if let Ok(mut map) = overrides_map().write() {
                map.clear();
                for row in &rows {
                    let limits = row_to_limits(row);
                    if !limits.overrides.is_empty() {
                        map.insert(limits.script_uri, limits.overrides);
                    }
                }
            }
        }
        Err(e) => tracing::warn!("Could not load script limits: {}", e),
    }
}

/// Re-read one script's overrides, for an instance picking up someone else's
/// change.
pub async fn refresh(script_uri: &str) {
    match get(script_uri).await {
        Ok(Some(limits)) if !limits.overrides.is_empty() => {
            if let Ok(mut map) = overrides_map().write() {
                map.insert(script_uri.to_string(), limits.overrides);
            }
        }
        Ok(_) => forget(script_uri),
        Err(e) => tracing::warn!("Could not refresh limits for {}: {}", script_uri, e),
    }
}

/// Drop a script's overrides from this instance's view.
pub fn forget(script_uri: &str) {
    if let Ok(mut map) = overrides_map().write() {
        map.remove(script_uri);
    }
}

// ============================================================================
// Reading and writing
// ============================================================================

pub async fn get(script_uri: &str) -> AppResult<Option<ScriptLimits>> {
    let pool = pool()?;
    let row = sqlx::query(
        "SELECT script_uri, timeout_ms, job_timeout_ms, max_memory_bytes, note, set_by, updated_at \
         FROM script_limits WHERE script_uri = $1",
    )
    .bind(script_uri)
    .fetch_optional(&pool)
    .await
    .map_err(|e| AppError::Database {
        message: format!("Could not read script limits: {}", e),
        source: None,
    })?;

    Ok(row.as_ref().map(row_to_limits))
}

/// Every override in the engine, for an operator asking what has been changed.
pub async fn list() -> AppResult<Vec<ScriptLimits>> {
    let pool = pool()?;
    let rows = sqlx::query(
        "SELECT script_uri, timeout_ms, job_timeout_ms, max_memory_bytes, note, set_by, updated_at \
         FROM script_limits ORDER BY script_uri",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| AppError::Database {
        message: format!("Could not list script limits: {}", e),
        source: None,
    })?;

    Ok(rows.iter().map(row_to_limits).collect())
}

/// Record what one script may spend, replacing whatever it had.
///
/// Replacing rather than merging, so the row always says the whole of what is
/// in force: a caller that omits a field is saying that field follows the
/// engine, not that it keeps an earlier override they cannot see.
pub async fn set(
    script_uri: &str,
    overrides: Overrides,
    note: Option<&str>,
    set_by: Option<&str>,
) -> AppResult<ScriptLimits> {
    let overrides = overrides.clamped();
    let pool = pool()?;

    let row = sqlx::query(
        r#"
        INSERT INTO script_limits
            (script_uri, timeout_ms, job_timeout_ms, max_memory_bytes, note, set_by, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, NOW())
        ON CONFLICT (script_uri)
        DO UPDATE SET timeout_ms = EXCLUDED.timeout_ms,
                      job_timeout_ms = EXCLUDED.job_timeout_ms,
                      max_memory_bytes = EXCLUDED.max_memory_bytes,
                      note = EXCLUDED.note,
                      set_by = EXCLUDED.set_by,
                      updated_at = NOW()
        RETURNING script_uri, timeout_ms, job_timeout_ms, max_memory_bytes, note, set_by, updated_at
        "#,
    )
    .bind(script_uri)
    .bind(overrides.timeout_ms.map(|v| v as i64))
    .bind(overrides.job_timeout_ms.map(|v| v as i64))
    .bind(overrides.max_memory_bytes.map(|v| v as i64))
    .bind(note)
    .bind(set_by)
    .fetch_one(&pool)
    .await
    .map_err(|e| AppError::Database {
        message: format!("Could not set script limits: {}", e),
        source: None,
    })?;

    let stored = row_to_limits(&row);
    if let Ok(mut map) = overrides_map().write() {
        if stored.overrides.is_empty() {
            map.remove(script_uri);
        } else {
            map.insert(script_uri.to_string(), stored.overrides.clone());
        }
    }

    Ok(stored)
}

/// Put a script back on the engine's own limits.
pub async fn clear(script_uri: &str) -> AppResult<bool> {
    let pool = pool()?;
    let result = sqlx::query("DELETE FROM script_limits WHERE script_uri = $1")
        .bind(script_uri)
        .execute(&pool)
        .await
        .map_err(|e| AppError::Database {
            message: format!("Could not clear script limits: {}", e),
            source: None,
        })?;

    forget(script_uri);
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ExecutionLimits {
        ExecutionLimits {
            timeout_ms: 10_000,
            max_memory_mb: 128,
            max_script_size_bytes: 1_000_000,
            stack_size_bytes: 512 * 1024,
        }
    }

    /// A script with no row behaves exactly as it did before any of this
    /// existed. That is the property that makes the feature safe to ship.
    #[test]
    fn no_override_is_the_engines_own_limits() {
        let applied = apply(base(), &Overrides::default(), false);
        assert_eq!(applied.timeout_ms, base().timeout_ms);
        assert_eq!(applied.max_memory_mb, base().max_memory_mb);
        assert_eq!(applied.stack_size_bytes, base().stack_size_bytes);
    }

    /// Overriding one thing must not restate the others.
    #[test]
    fn one_override_leaves_everything_else_alone() {
        let applied = apply(
            base(),
            &Overrides {
                timeout_ms: Some(45_000),
                ..Default::default()
            },
            false,
        );
        assert_eq!(applied.timeout_ms, 45_000);
        assert_eq!(applied.max_memory_mb, base().max_memory_mb);
    }

    /// Lowering is the direction that gets used: containing a script that has
    /// started holding execution slots, without restarting the engine.
    #[test]
    fn a_limit_can_be_lowered_as_well_as_raised() {
        let contained = apply(
            base(),
            &Overrides {
                timeout_ms: Some(1_000),
                ..Default::default()
            },
            false,
        );
        assert!(contained.timeout_ms < base().timeout_ms);
    }

    #[test]
    fn memory_is_carried_across_in_the_unit_the_runtime_wants() {
        let applied = apply(
            base(),
            &Overrides {
                max_memory_bytes: Some(256 * 1024 * 1024),
                ..Default::default()
            },
            false,
        );
        assert_eq!(applied.max_memory_mb, 256);
    }

    /// A value the runtime would read as "no memory at all" is not a limit,
    /// it is a script that cannot run.
    #[test]
    fn a_memory_override_never_rounds_down_to_nothing() {
        let applied = apply(
            base(),
            &Overrides {
                max_memory_bytes: Some(1),
                ..Default::default()
            },
            false,
        );
        assert!(applied.max_memory_mb >= 1);
    }

    #[test]
    fn values_are_clamped_into_range_rather_than_taken_as_written() {
        let clamped = Overrides {
            timeout_ms: Some(u64::MAX),
            job_timeout_ms: Some(u64::MAX),
            max_memory_bytes: Some(u64::MAX),
        }
        .clamped();

        assert_eq!(clamped.timeout_ms, Some(MAX_TIMEOUT_MS));
        assert_eq!(clamped.job_timeout_ms, Some(MAX_JOB_TIMEOUT_MS));
        assert_eq!(clamped.max_memory_bytes, Some(MAX_MEMORY_BYTES));

        let floored = Overrides {
            timeout_ms: Some(0),
            job_timeout_ms: Some(0),
            max_memory_bytes: Some(0),
        }
        .clamped();

        assert_eq!(floored.timeout_ms, Some(MIN_TIMEOUT_MS));
        assert_eq!(floored.job_timeout_ms, Some(MIN_TIMEOUT_MS));
        assert_eq!(floored.max_memory_bytes, Some(MIN_MEMORY_BYTES));
    }

    /// Clamping never turns an absent override into a present one: a field the
    /// caller did not set still follows the engine.
    #[test]
    fn clamping_does_not_invent_an_override() {
        assert_eq!(Overrides::default().clamped(), Overrides::default());
        assert!(Overrides::default().clamped().is_empty());
    }

    /// A job is not answering a request, so background work takes the job
    /// budget even when a request override is set beside it.
    #[test]
    fn background_work_takes_the_job_budget_not_the_request_one() {
        let overrides = Overrides {
            timeout_ms: Some(1_000),
            job_timeout_ms: Some(120_000),
            ..Default::default()
        };

        assert_eq!(apply(base(), &overrides, false).timeout_ms, 1_000);
        assert_eq!(apply(base(), &overrides, true).timeout_ms, 120_000);
    }
}
