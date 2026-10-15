//! Schedules that belong to a person: "every morning at seven, as me".
//!
//! `schedulerService` declares a schedule for the *script*: it runs as the
//! engine, and it is wiped and declared again on every `init()` because it
//! belongs to the version of the code. `personalTasks.enqueue({ runAt })` runs
//! once, as the person. Neither is a declaration that belongs to a person —
//! keyed so that saying it again replaces it, kept across deployments, bounded
//! by the grant that lets it act, and visible on the account page beside that
//! grant. This is.
//!
//! # What a row is
//!
//! A row in `personal_schedules` says *when*, never runs JavaScript itself.
//! When it is due, [`fire_due`] inserts an ordinary delegated task into
//! `script_tasks` (`run_as` = the person), and from there it is what any
//! `personalTasks.enqueue` task gets: the person's lane, retries, the job
//! budget, and the grant read again when it runs. How that task ends is
//! written back by [`record_outcome_on`], so a schedule whose handler is gone
//! stops costing runs instead of failing every morning forever.
//!
//! # Lifetime
//!
//! Deleted with the script, the account (foreign keys), or the grant
//! ([`delete_for_grant`], from `delegation::revoke` and `revoke_all`): a
//! withdrawal is the person saying stop. Paused when the grant lapses, and
//! resumed when it is given again ([`resume_expired`]): a deadline passing is
//! not the person saying stop, and making them set up their mornings again
//! because they renewed a day late would teach people to grant the longest
//! expiry offered.

use std::str::FromStr;
use std::sync::OnceLock;

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use serde_json::Value;
use sqlx::Row;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::tasks;

/// Longest a schedule key may be.
pub const MAX_KEY_CHARS: usize = 128;

/// How many attempts the task a firing creates gets.
const ATTEMPTS_PER_RUN: i32 = 3;

/// Given-up runs in a row after which a schedule pauses as `failing`.
pub const FAILURES_BEFORE_PAUSE: i32 = 3;

/// How many due schedules one worker fires per tick.
const FIRE_BATCH_SIZE: i64 = 32;

/// How many upcoming slots of a cron are checked against the interval floor.
///
/// A pattern's tightest gap repeats within a day for anything a person would
/// write ("0,5 7 * * *"), so a few days of slots find it.
const FLOOR_CHECK_SLOTS: usize = 64;

/// The limits from `[javascript]`, captured at startup.
#[derive(Debug, Clone, Copy)]
pub struct ScheduleLimits {
    pub min_interval_ms: u64,
    pub per_person_script: u32,
    pub per_person: u32,
}

impl ScheduleLimits {
    pub fn from_config(config: &crate::config::JavaScriptConfig) -> Self {
        Self {
            min_interval_ms: config.personal_schedule_min_interval_ms,
            per_person_script: config.personal_schedules_per_person_script,
            per_person: config.personal_schedules_per_person,
        }
    }
}

static CONFIGURED: OnceLock<ScheduleLimits> = OnceLock::new();

/// Record the configured limits. Returns false if they were already recorded.
pub fn configure(limits: ScheduleLimits) -> bool {
    CONFIGURED.set(limits).is_ok()
}

/// The limits in force: the configured ones, or the shipped defaults.
pub fn limits() -> ScheduleLimits {
    *CONFIGURED
        .get_or_init(|| ScheduleLimits::from_config(&crate::config::JavaScriptConfig::default()))
}

/// Why a declaration was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScheduleError {
    #[error("key is required")]
    MissingKey,
    #[error("key must be at most {MAX_KEY_CHARS} characters")]
    KeyTooLong,
    #[error("handler name is required")]
    MissingHandler,
    #[error("handler name must be 1-{} characters", tasks::MAX_HANDLER_NAME_CHARS)]
    InvalidHandler,
    #[error("payload must be a JSON object")]
    PayloadNotAnObject,
    #[error("payload must be at most {} bytes", tasks::MAX_PAYLOAD_BYTES)]
    PayloadTooLarge,
    #[error("give exactly one of intervalMinutes, intervalMilliseconds and cron")]
    CadenceAmbiguous,
    #[error("the interval must be a positive whole number")]
    InvalidInterval,
    #[error("cron must have five fields (minute hour day month weekday): {0}")]
    InvalidCron(String),
    #[error("timeZone is required with cron")]
    MissingTimeZone,
    #[error("timeZone is only for cron; an interval is elapsed time")]
    TimeZoneWithInterval,
    #[error("unknown time zone '{0}' - use an IANA name such as 'Europe/Helsinki'")]
    UnknownTimeZone(String),
    #[error("runs must be at least {0} minutes apart")]
    TooFrequent(u64),
    #[error("firstRunAfterMs must be a whole number of milliseconds, zero or more")]
    InvalidFirstRun,
    #[error("lane must be a string, or null for no lane")]
    InvalidLane,
    #[error("this person already has {0} schedules on this script")]
    TooManyForScript(u32),
    #[error("this person already has {0} schedules")]
    TooManyForPerson(u32),
    #[error("no database is available to store the schedule")]
    Unavailable,
    #[error("could not store the schedule: {0}")]
    Storage(String),
}

impl ScheduleError {
    /// Which kind of exception this becomes in a script.
    pub fn js_name(&self) -> &'static str {
        match self {
            ScheduleError::MissingKey
            | ScheduleError::MissingHandler
            | ScheduleError::InvalidHandler
            | ScheduleError::PayloadNotAnObject
            | ScheduleError::CadenceAmbiguous
            | ScheduleError::MissingTimeZone
            | ScheduleError::TimeZoneWithInterval
            | ScheduleError::InvalidLane => "TypeError",
            ScheduleError::KeyTooLong
            | ScheduleError::PayloadTooLarge
            | ScheduleError::InvalidInterval
            | ScheduleError::InvalidCron(_)
            | ScheduleError::UnknownTimeZone(_)
            | ScheduleError::TooFrequent(_)
            | ScheduleError::InvalidFirstRun
            | ScheduleError::TooManyForScript(_)
            | ScheduleError::TooManyForPerson(_) => "RangeError",
            ScheduleError::Unavailable | ScheduleError::Storage(_) => "Error",
        }
    }
}

/// When a schedule runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Cadence {
    /// Plain elapsed time from the previous slot.
    Interval { ms: i64 },
    /// A five-field cron, evaluated in a time zone so "07:00" stays at seven
    /// across daylight saving.
    Cron { pattern: String, time_zone: Tz },
}

/// A five-field cron, parsed. Seconds and years are refused rather than
/// accepted silently: a person's schedule is minutes at the finest, and the
/// floor would refuse anything finer anyway.
fn parse_cron(pattern: &str) -> Result<Cron, ScheduleError> {
    let pattern = pattern.trim();
    if pattern.split_whitespace().count() != 5 {
        return Err(ScheduleError::InvalidCron(pattern.to_string()));
    }
    CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .build()
        .parse(pattern)
        .map_err(|e| ScheduleError::InvalidCron(format!("{} ({})", pattern, e)))
}

impl Cadence {
    fn cron(pattern: &str, time_zone: &str) -> Result<Self, ScheduleError> {
        parse_cron(pattern)?;
        let time_zone = Tz::from_str(time_zone.trim())
            .map_err(|_| ScheduleError::UnknownTimeZone(time_zone.to_string()))?;
        Ok(Cadence::Cron {
            pattern: pattern.trim().to_string(),
            time_zone,
        })
    }

    fn from_row(interval_ms: Option<i64>, cron: Option<String>, time_zone: Option<String>) -> Self {
        match (interval_ms, cron) {
            (Some(ms), _) => Cadence::Interval { ms },
            (None, Some(pattern)) => Cadence::Cron {
                pattern,
                time_zone: time_zone
                    .as_deref()
                    .and_then(|name| Tz::from_str(name).ok())
                    .unwrap_or(Tz::UTC),
            },
            // The table's CHECK rules this out; a row that somehow breaks it
            // runs daily rather than in a loop.
            (None, None) => Cadence::Interval {
                ms: 24 * 60 * 60 * 1000,
            },
        }
    }

    /// The first slot strictly after `now`.
    ///
    /// `previous` anchors an interval: slots are `previous + k × interval`, so
    /// a schedule that was late does not drift, and after downtime it runs
    /// once rather than once per missed slot. A cron needs no anchor.
    pub fn next_after(&self, previous: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Cadence::Interval { ms } => {
                let step = Duration::milliseconds((*ms).max(1));
                let Some(previous) = previous else {
                    return now + step;
                };
                if previous > now {
                    return previous;
                }
                let elapsed = (now - previous).num_milliseconds();
                let steps = elapsed / step.num_milliseconds() + 1;
                previous + Duration::milliseconds(steps.saturating_mul(step.num_milliseconds()))
            }
            Cadence::Cron { pattern, time_zone } => parse_cron(pattern)
                .ok()
                .and_then(|cron| {
                    cron.find_next_occurrence(&now.with_timezone(time_zone), false)
                        .ok()
                })
                .map(|next| next.with_timezone(&Utc))
                // A pattern that matches nothing ("0 0 31 2 *") never fires.
                // Far enough out to be "never" and still a valid timestamp.
                .unwrap_or_else(|| now + Duration::days(365 * 100)),
        }
    }

    /// The shortest gap between two runs, as far as can be seen from `from`.
    fn tightest_gap(&self, from: DateTime<Utc>) -> Option<Duration> {
        match self {
            Cadence::Interval { ms } => Some(Duration::milliseconds(*ms)),
            Cadence::Cron { pattern, time_zone } => {
                let cron = parse_cron(pattern).ok()?;
                let slots: Vec<DateTime<Utc>> = cron
                    .iter_after(from.with_timezone(time_zone))
                    .take(FLOOR_CHECK_SLOTS)
                    .map(|slot| slot.with_timezone(&Utc))
                    .collect();
                slots.windows(2).map(|pair| pair[1] - pair[0]).min()
            }
        }
    }

    /// How a person reads it on the account page.
    pub fn describe(&self) -> String {
        match self {
            Cadence::Interval { ms } => describe_interval(*ms),
            Cadence::Cron { pattern, time_zone } => {
                let fields: Vec<&str> = pattern.split_whitespace().collect();
                let daily = fields.len() == 5
                    && fields[2] == "*"
                    && fields[3] == "*"
                    && fields[4] == "*"
                    && fields[0].parse::<u32>().is_ok()
                    && fields[1].parse::<u32>().is_ok();
                if daily {
                    format!(
                        "daily at {:0>2}:{:0>2} ({})",
                        fields[1],
                        fields[0],
                        time_zone.name()
                    )
                } else {
                    format!("cron \"{}\" ({})", pattern, time_zone.name())
                }
            }
        }
    }
}

fn describe_interval(ms: i64) -> String {
    let minutes = ms / 60_000;
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("every {}", unit)
        } else {
            format!("every {} {}s", n, unit)
        }
    };
    if minutes >= 24 * 60 && minutes % (24 * 60) == 0 {
        plural(minutes / (24 * 60), "day")
    } else if minutes >= 60 && minutes % 60 == 0 {
        plural(minutes / 60, "hour")
    } else if minutes >= 1 {
        plural(minutes, "minute")
    } else {
        format!("every {} ms", ms)
    }
}

/// What a script asked for, checked.
#[derive(Debug, Clone)]
pub struct ScheduleSpec {
    pub key: String,
    pub handler: String,
    pub payload: Value,
    pub cadence: Cadence,
    pub first_run_after_ms: Option<i64>,
    /// As `personalTasks.enqueue` reads it: `None` absent, `Some(None)` an
    /// explicit `null`, `Some(Some(lane))` named.
    pub lane: Option<Option<String>>,
}

fn whole_number(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        _ => None,
    }
}

impl ScheduleSpec {
    /// Read a `ScheduleSpec` as the script passed it, checking everything
    /// that does not need the database.
    pub fn from_json(options: &Value, limits: &ScheduleLimits) -> Result<Self, ScheduleError> {
        let key = options
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if key.is_empty() {
            return Err(ScheduleError::MissingKey);
        }
        if key.chars().count() > MAX_KEY_CHARS {
            return Err(ScheduleError::KeyTooLong);
        }

        let handler = options
            .get("handler")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if handler.is_empty() {
            return Err(ScheduleError::MissingHandler);
        }
        if handler.chars().count() > tasks::MAX_HANDLER_NAME_CHARS {
            return Err(ScheduleError::InvalidHandler);
        }

        let payload = match options.get("payload") {
            None | Some(Value::Null) => serde_json::json!({}),
            Some(payload) if payload.is_object() => payload.clone(),
            Some(_) => return Err(ScheduleError::PayloadNotAnObject),
        };
        let payload_len = serde_json::to_vec(&payload)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if payload_len > tasks::MAX_PAYLOAD_BYTES {
            return Err(ScheduleError::PayloadTooLarge);
        }

        let present = |name: &str| options.get(name).is_some_and(|v| !v.is_null());
        let minutes = present("intervalMinutes");
        let millis = present("intervalMilliseconds");
        let cron = present("cron");
        if [minutes, millis, cron]
            .iter()
            .filter(|given| **given)
            .count()
            != 1
        {
            return Err(ScheduleError::CadenceAmbiguous);
        }
        let time_zone = options.get("timeZone").filter(|v| !v.is_null());

        let cadence = if cron {
            let pattern = options
                .get("cron")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ScheduleError::InvalidCron(options["cron"].to_string()))?;
            let Some(time_zone) = time_zone else {
                return Err(ScheduleError::MissingTimeZone);
            };
            let time_zone = time_zone
                .as_str()
                .ok_or_else(|| ScheduleError::UnknownTimeZone(time_zone.to_string()))?;
            Cadence::cron(pattern, time_zone)?
        } else {
            if time_zone.is_some() {
                return Err(ScheduleError::TimeZoneWithInterval);
            }
            let ms = if minutes {
                whole_number(&options["intervalMinutes"]).and_then(|m| m.checked_mul(60_000))
            } else {
                whole_number(&options["intervalMilliseconds"])
            };
            match ms {
                Some(ms) if ms > 0 => Cadence::Interval { ms },
                _ => return Err(ScheduleError::InvalidInterval),
            }
        };

        let floor = Duration::milliseconds(limits.min_interval_ms as i64);
        if let Some(gap) = cadence.tightest_gap(Utc::now())
            && gap < floor
        {
            return Err(ScheduleError::TooFrequent(
                limits.min_interval_ms.div_ceil(60_000),
            ));
        }

        let first_run_after_ms = match options.get("firstRunAfterMs") {
            None | Some(Value::Null) => None,
            Some(value) => match whole_number(value) {
                Some(ms) if ms >= 0 => Some(ms),
                _ => return Err(ScheduleError::InvalidFirstRun),
            },
        };

        let lane = match options.get("lane") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(lane)) => Some(Some(lane.clone())),
            Some(_) => return Err(ScheduleError::InvalidLane),
        };

        Ok(Self {
            key,
            handler,
            payload,
            cadence,
            first_run_after_ms,
            lane,
        })
    }

    /// When the first run is.
    fn first_run(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self.first_run_after_ms {
            Some(ms) => now + Duration::milliseconds(ms),
            None => self.cadence.next_after(None, now),
        }
    }
}

/// A schedule as stored.
#[derive(Debug, Clone)]
pub struct PersonalSchedule {
    pub schedule_id: Uuid,
    pub script_uri: String,
    pub user_id: String,
    pub key: String,
    pub handler: String,
    pub payload: Value,
    pub cadence: Cadence,
    pub lane: Option<String>,
    pub next_run_at: DateTime<Utc>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub consecutive_failures: i32,
    pub state: String,
    pub paused_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// The columns [`PersonalSchedule::from_row`] reads, as a literal so it can be
/// spliced into a static statement.
macro_rules! columns {
    () => {
        "schedule_id, script_uri, user_id, schedule_key, handler_name, payload, \
         interval_ms, cron, time_zone, lane, next_run_at, last_run_at, \
         consecutive_failures, state, paused_reason, created_at"
    };
}

impl PersonalSchedule {
    fn from_row(row: &sqlx::postgres::PgRow) -> Self {
        Self {
            schedule_id: row.get("schedule_id"),
            script_uri: row.get("script_uri"),
            user_id: row.get("user_id"),
            key: row.get("schedule_key"),
            handler: row.get("handler_name"),
            payload: row.get("payload"),
            cadence: Cadence::from_row(
                row.get("interval_ms"),
                row.get("cron"),
                row.get("time_zone"),
            ),
            lane: row.get("lane"),
            next_run_at: row.get("next_run_at"),
            last_run_at: row.get("last_run_at"),
            consecutive_failures: row.get("consecutive_failures"),
            state: row.get("state"),
            paused_reason: row.get("paused_reason"),
            created_at: row.get("created_at"),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.state == "paused"
    }

    /// What a script sees. No user id: a script only ever reads its caller's
    /// own schedules, and an id would end up in whatever it logs.
    pub fn to_json(&self) -> Value {
        let (cron, time_zone, interval_minutes) = match &self.cadence {
            Cadence::Interval { ms } => {
                (None, None, Some(serde_json::json!(*ms as f64 / 60_000.0)))
            }
            Cadence::Cron { pattern, time_zone } => (
                Some(pattern.clone()),
                Some(time_zone.name().to_string()),
                None,
            ),
        };
        let mut json = serde_json::json!({
            "key": self.key,
            "handler": self.handler,
            "cron": cron,
            "intervalMinutes": interval_minutes,
            "timeZone": time_zone,
            "nextRunAt": self.next_run_at.to_rfc3339(),
            "lastRunAt": self.last_run_at.map(|at| at.to_rfc3339()),
            "state": self.state,
        });
        if let Some(reason) = &self.paused_reason {
            json["pausedReason"] = Value::String(reason.clone());
        }
        json
    }
}

/// Declare, or replace, `user_id`'s schedule under `spec.key` on `script_uri`.
///
/// `lane` is already resolved by the caller (the person's lane by default).
///
/// Restating a schedule with the same cadence keeps its next slot, so a
/// script that calls `schedule` on every visit does not push an interval
/// forward for ever. A changed cadence, or a paused schedule being declared
/// again, starts from the new spec. Either way the schedule is active
/// afterwards: declaring it again is how a paused one resumes.
pub async fn declare_on(
    conn: &mut sqlx::PgConnection,
    user_id: &str,
    script_uri: &str,
    spec: &ScheduleSpec,
    lane: Option<&str>,
    limits: &ScheduleLimits,
) -> Result<PersonalSchedule, ScheduleError> {
    let storage = |e: sqlx::Error| ScheduleError::Storage(e.to_string());

    // Counting and inserting must not interleave with another declaration by
    // the same person, or two at the limit both pass. Held to the end of the
    // enclosing transaction.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('personal_schedules'), hashtext($1))")
        .bind(user_id)
        .execute(&mut *conn)
        .await
        .map_err(storage)?;

    let (for_script, for_person): (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE script_uri = $2),
            COUNT(*)
        FROM personal_schedules
        WHERE user_id = $1
          AND NOT (script_uri = $2 AND schedule_key = $3)
        "#,
    )
    .bind(user_id)
    .bind(script_uri)
    .bind(&spec.key)
    .fetch_one(&mut *conn)
    .await
    .map_err(storage)?;

    if for_script >= limits.per_person_script as i64 {
        return Err(ScheduleError::TooManyForScript(limits.per_person_script));
    }
    if for_person >= limits.per_person as i64 {
        return Err(ScheduleError::TooManyForPerson(limits.per_person));
    }

    let (interval_ms, cron, time_zone) = match &spec.cadence {
        Cadence::Interval { ms } => (Some(*ms), None, None),
        Cadence::Cron { pattern, time_zone } => (
            None,
            Some(pattern.clone()),
            Some(time_zone.name().to_string()),
        ),
    };
    let next_run_at = spec.first_run(Utc::now());

    let row = sqlx::query(concat!(
        r#"
        INSERT INTO personal_schedules
            (schedule_id, script_uri, user_id, schedule_key, handler_name, payload,
             interval_ms, cron, time_zone, lane, next_run_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (script_uri, user_id, schedule_key) DO UPDATE SET
            handler_name = EXCLUDED.handler_name,
            payload = EXCLUDED.payload,
            next_run_at = CASE
                WHEN personal_schedules.state = 'active'
                 AND personal_schedules.interval_ms IS NOT DISTINCT FROM EXCLUDED.interval_ms
                 AND personal_schedules.cron IS NOT DISTINCT FROM EXCLUDED.cron
                 AND personal_schedules.time_zone IS NOT DISTINCT FROM EXCLUDED.time_zone
                THEN personal_schedules.next_run_at
                ELSE EXCLUDED.next_run_at
            END,
            interval_ms = EXCLUDED.interval_ms,
            cron = EXCLUDED.cron,
            time_zone = EXCLUDED.time_zone,
            lane = EXCLUDED.lane,
            state = 'active',
            paused_reason = NULL,
            consecutive_failures = 0,
            updated_at = NOW()
        RETURNING "#,
        columns!(),
        r#", (xmax = 0) AS created
        "#
    ))
    .bind(Uuid::new_v4())
    .bind(script_uri)
    .bind(user_id)
    .bind(&spec.key)
    .bind(&spec.handler)
    .bind(&spec.payload)
    .bind(interval_ms)
    .bind(cron)
    .bind(time_zone)
    .bind(lane)
    .bind(next_run_at)
    .fetch_one(&mut *conn)
    .await
    .map_err(storage)?;

    let schedule = PersonalSchedule::from_row(&row);
    let created: bool = row.get("created");
    debug!(
        script = %script_uri,
        user = %user_id,
        key = %schedule.key,
        created,
        next_run_at = %schedule.next_run_at,
        "Personal schedule declared"
    );
    Ok(schedule)
}

/// This person's schedules for this script, by key.
pub async fn list_on<'e, E>(
    executor: E,
    user_id: &str,
    script_uri: &str,
) -> Result<Vec<PersonalSchedule>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM personal_schedules \
         WHERE user_id = $1 AND script_uri = $2 ORDER BY schedule_key"
    ))
    .bind(user_id)
    .bind(script_uri)
    .fetch_all(executor)
    .await?;
    Ok(rows.iter().map(PersonalSchedule::from_row).collect())
}

/// Every schedule this person has, for the account page.
pub async fn list_for_user(user_id: &str) -> Result<Vec<PersonalSchedule>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM personal_schedules \
         WHERE user_id = $1 ORDER BY script_uri, schedule_key"
    ))
    .bind(user_id)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(PersonalSchedule::from_row).collect())
}

/// Remove one by key. False if there was nothing under it.
pub async fn remove_on<'e, E>(
    executor: E,
    user_id: &str,
    script_uri: &str,
    key: &str,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let result = sqlx::query(
        "DELETE FROM personal_schedules \
         WHERE user_id = $1 AND script_uri = $2 AND schedule_key = $3",
    )
    .bind(user_id)
    .bind(script_uri)
    .bind(key.trim())
    .execute(executor)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Remove one by id, for the account page. Scoped to its owner, so an id
/// from somewhere else removes nothing.
pub async fn delete_by_id(
    user_id: &str,
    schedule_id: Uuid,
) -> Result<Option<PersonalSchedule>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(None);
    };
    let row = sqlx::query(concat!(
        "DELETE FROM personal_schedules WHERE user_id = $1 AND schedule_id = $2 RETURNING ",
        columns!()
    ))
    .bind(user_id)
    .bind(schedule_id)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.as_ref().map(PersonalSchedule::from_row))
}

/// Pause or resume one, for the account page. Scoped to its owner.
///
/// Resuming clears the failure count and leaves the next slot where it was: a
/// schedule that missed slots while paused runs once soon, as it would after
/// downtime, and then on its cadence.
pub async fn set_paused(
    user_id: &str,
    schedule_id: Uuid,
    paused: bool,
) -> Result<Option<PersonalSchedule>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(None);
    };
    let statement = if paused {
        concat!(
            "UPDATE personal_schedules SET state = 'paused', paused_reason = 'person', \
             updated_at = NOW() WHERE user_id = $1 AND schedule_id = $2 RETURNING ",
            columns!()
        )
    } else {
        concat!(
            "UPDATE personal_schedules SET state = 'active', paused_reason = NULL, \
             consecutive_failures = 0, updated_at = NOW() \
             WHERE user_id = $1 AND schedule_id = $2 RETURNING ",
            columns!()
        )
    };
    let row = sqlx::query(statement)
        .bind(user_id)
        .bind(schedule_id)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.as_ref().map(PersonalSchedule::from_row))
}

/// Delete what a withdrawn grant allowed: one script's, or with `None` every
/// one of this person's.
pub async fn delete_for_grant(user_id: &str, script_uri: Option<&str>) -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };
    let result = match script_uri {
        Some(script_uri) => {
            sqlx::query("DELETE FROM personal_schedules WHERE user_id = $1 AND script_uri = $2")
                .bind(user_id)
                .bind(script_uri)
                .execute(db.pool())
                .await?
        }
        None => {
            sqlx::query("DELETE FROM personal_schedules WHERE user_id = $1")
                .bind(user_id)
                .execute(db.pool())
                .await?
        }
    };
    Ok(result.rows_affected())
}

/// Resume what paused because this grant lapsed, now that it is given again.
pub async fn resume_expired(user_id: &str, script_uri: &str) -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };
    let result = sqlx::query(
        r#"
        UPDATE personal_schedules
        SET state = 'active', paused_reason = NULL, updated_at = NOW()
        WHERE user_id = $1 AND script_uri = $2
          AND state = 'paused' AND paused_reason = 'expired'
        "#,
    )
    .bind(user_id)
    .bind(script_uri)
    .execute(db.pool())
    .await?;
    Ok(result.rows_affected())
}

/// The schedules a grant being given would start again, for the consent page.
pub async fn paused_expired(
    user_id: &str,
    script_uri: &str,
) -> Result<Vec<PersonalSchedule>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM personal_schedules \
         WHERE user_id = $1 AND script_uri = $2 \
           AND state = 'paused' AND paused_reason = 'expired' \
         ORDER BY schedule_key"
    ))
    .bind(user_id)
    .bind(script_uri)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(PersonalSchedule::from_row).collect())
}

/// Turn every due schedule into a task. Returns how many tasks were queued.
///
/// One transaction. `FOR UPDATE SKIP LOCKED` hands two workers different rows,
/// and since firing is the whole transaction rather than a claimed run there
/// is no lease to keep: a worker that dies mid-way rolls back and the rows are
/// due again.
///
/// The grant read here is about not queueing pointless work, not about
/// authority: the task re-resolves it when it runs, as every delegated task
/// does.
pub async fn fire_due(now: DateTime<Utc>) -> usize {
    let Some(db) = crate::database::get_global_database() else {
        return 0;
    };

    let mut tx = match db.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            warn!(error = %e, "Failed opening a transaction to fire personal schedules");
            return 0;
        }
    };

    let due = match sqlx::query(concat!(
        r#"
        SELECT "#,
        columns!(),
        r#", last_task_id
        FROM personal_schedules
        WHERE state = 'active' AND next_run_at <= $1
        ORDER BY next_run_at
        LIMIT $2
        FOR UPDATE SKIP LOCKED
        "#
    ))
    .bind(now)
    .bind(FIRE_BATCH_SIZE)
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "Failed reading due personal schedules");
            return 0;
        }
    };

    if due.is_empty() {
        let _ = tx.rollback().await;
        return 0;
    }

    let mut queued = 0;
    for row in &due {
        let schedule = PersonalSchedule::from_row(row);
        let last_task_id: Option<Uuid> = row.get("last_task_id");
        match fire_one(&mut tx, &schedule, last_task_id, now).await {
            Ok(true) => queued += 1,
            Ok(false) => {}
            Err(e) => {
                warn!(
                    schedule = %schedule.schedule_id,
                    error = %e,
                    "Failed firing a personal schedule"
                );
                let _ = tx.rollback().await;
                return 0;
            }
        }
    }

    if let Err(e) = tx.commit().await {
        warn!(error = %e, "Failed committing fired personal schedules");
        return 0;
    }
    if queued > 0 {
        tasks::wake();
    }
    queued
}

/// One due schedule. True when a task was queued.
async fn fire_one(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    schedule: &PersonalSchedule,
    last_task_id: Option<Uuid>,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let grant_expires: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT expires_at FROM script_delegations WHERE user_id = $1 AND script_uri = $2",
    )
    .bind(&schedule.user_id)
    .bind(&schedule.script_uri)
    .fetch_optional(&mut **tx)
    .await?;

    let next_run_at = schedule.cadence.next_after(Some(schedule.next_run_at), now);

    match grant_expires {
        // Withdrawing the grant deletes its schedules; this is a race with
        // that, finished here.
        None => {
            sqlx::query("DELETE FROM personal_schedules WHERE schedule_id = $1")
                .bind(schedule.schedule_id)
                .execute(&mut **tx)
                .await?;
            return Ok(false);
        }
        Some(expires_at) if expires_at <= now => {
            sqlx::query(
                "UPDATE personal_schedules SET state = 'paused', paused_reason = 'expired', \
                 next_run_at = $2, updated_at = NOW() WHERE schedule_id = $1",
            )
            .bind(schedule.schedule_id)
            .bind(next_run_at)
            .execute(&mut **tx)
            .await?;
            info!(
                script = %schedule.script_uri,
                user = %schedule.user_id,
                key = %schedule.key,
                "Personal schedule paused: the grant has expired"
            );
            return Ok(false);
        }
        Some(_) => {}
    }

    // Work that takes longer than its interval must not pile up behind itself.
    let unfinished = match last_task_id {
        Some(task_id) => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM script_tasks \
             WHERE task_id = $1 AND state IN ('pending', 'running'))",
            )
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await?
        }
        None => false,
    };

    if unfinished {
        sqlx::query(
            "UPDATE personal_schedules SET next_run_at = $2, updated_at = NOW() \
             WHERE schedule_id = $1",
        )
        .bind(schedule.schedule_id)
        .bind(next_run_at)
        .execute(&mut **tx)
        .await?;
        crate::repository::insert_log_message_async(
            &schedule.script_uri,
            &format!(
                "schedule '{}' skipped its {} slot: the previous run has not finished",
                schedule.key,
                schedule.next_run_at.to_rfc3339()
            ),
            "WARN",
        )
        .await;
        return Ok(false);
    }

    let task = tasks::insert_scheduled(
        &mut **tx,
        tasks::NewTask {
            script_uri: schedule.script_uri.clone(),
            handler_name: schedule.handler.clone(),
            payload: schedule.payload.clone(),
            run_at: Some(now),
            max_attempts: Some(ATTEMPTS_PER_RUN),
            enqueued_by: None,
            run_as: Some(schedule.user_id.clone()),
            lane: schedule.lane.clone(),
        },
        schedule.schedule_id,
        schedule.next_run_at,
    )
    .await
    .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

    sqlx::query(
        "UPDATE personal_schedules SET next_run_at = $2, last_run_at = $3, last_task_id = $4, \
         updated_at = NOW() WHERE schedule_id = $1",
    )
    .bind(schedule.schedule_id)
    .bind(next_run_at)
    .bind(now)
    .bind(task.task_id)
    .execute(&mut **tx)
    .await?;

    debug!(
        script = %schedule.script_uri,
        key = %schedule.key,
        task = %task.task_id,
        next_run_at = %next_run_at,
        "Personal schedule fired"
    );
    Ok(true)
}

/// Write how a scheduled run ended back to its schedule, inside the
/// transaction that records the task's outcome.
///
/// Only a success or a run that has given up counts; a retry in between says
/// nothing yet. Three given up in a row pause the schedule as `failing`.
pub async fn record_outcome_on(
    conn: &mut sqlx::PgConnection,
    schedule_id: Uuid,
    succeeded: bool,
) -> Result<(), sqlx::Error> {
    if succeeded {
        sqlx::query(
            "UPDATE personal_schedules SET consecutive_failures = 0, updated_at = NOW() \
             WHERE schedule_id = $1 AND consecutive_failures <> 0",
        )
        .bind(schedule_id)
        .execute(&mut *conn)
        .await?;
        return Ok(());
    }

    sqlx::query(
        r#"
        UPDATE personal_schedules
        SET consecutive_failures = consecutive_failures + 1,
            state = CASE WHEN consecutive_failures + 1 >= $2 THEN 'paused' ELSE state END,
            paused_reason = CASE WHEN consecutive_failures + 1 >= $2 THEN 'failing'
                                 ELSE paused_reason END,
            updated_at = NOW()
        WHERE schedule_id = $1
        "#,
    )
    .bind(schedule_id)
    .bind(FAILURES_BEFORE_PAUSE)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The blocking face, for the JavaScript host bindings. Inside a script's
/// transaction these join it, as `scriptTasks.enqueue` does: a schedule
/// declared there commits with the writes it was declared about.
pub mod blocking {
    use super::*;
    use crate::database::{TransactionExecutor, get_current_executor};

    pub fn declare(
        user_id: &str,
        script_uri: &str,
        spec: &ScheduleSpec,
        lane: Option<&str>,
    ) -> Result<PersonalSchedule, ScheduleError> {
        let Some(db) = crate::database::get_global_database() else {
            return Err(ScheduleError::Unavailable);
        };
        let limits = limits();
        crate::database::run_blocking(async {
            match get_current_executor(db.pool()) {
                TransactionExecutor::Transaction(tx) => {
                    declare_on(tx, user_id, script_uri, spec, lane, &limits).await
                }
                TransactionExecutor::Pool(pool) => {
                    let mut tx = pool
                        .begin()
                        .await
                        .map_err(|e| ScheduleError::Storage(e.to_string()))?;
                    let schedule =
                        declare_on(&mut tx, user_id, script_uri, spec, lane, &limits).await?;
                    tx.commit()
                        .await
                        .map_err(|e| ScheduleError::Storage(e.to_string()))?;
                    Ok(schedule)
                }
            }
        })
    }

    pub fn list(user_id: &str, script_uri: &str) -> Result<Vec<PersonalSchedule>, sqlx::Error> {
        let Some(db) = crate::database::get_global_database() else {
            return Ok(Vec::new());
        };
        crate::database::run_blocking(async {
            match get_current_executor(db.pool()) {
                TransactionExecutor::Transaction(tx) => {
                    list_on(&mut **tx, user_id, script_uri).await
                }
                TransactionExecutor::Pool(pool) => list_on(pool, user_id, script_uri).await,
            }
        })
    }

    pub fn remove(user_id: &str, script_uri: &str, key: &str) -> Result<bool, sqlx::Error> {
        let Some(db) = crate::database::get_global_database() else {
            return Ok(false);
        };
        crate::database::run_blocking(async {
            match get_current_executor(db.pool()) {
                TransactionExecutor::Transaction(tx) => {
                    remove_on(&mut **tx, user_id, script_uri, key).await
                }
                TransactionExecutor::Pool(pool) => remove_on(pool, user_id, script_uri, key).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn limits_with_floor(min_interval_ms: u64) -> ScheduleLimits {
        ScheduleLimits {
            min_interval_ms,
            per_person_script: 10,
            per_person: 50,
        }
    }

    fn spec(options: Value) -> Result<ScheduleSpec, ScheduleError> {
        ScheduleSpec::from_json(&options, &limits_with_floor(15 * 60 * 1000))
    }

    #[test]
    fn exactly_one_cadence_is_required() {
        let none = spec(serde_json::json!({ "key": "k", "handler": "h" }));
        assert_eq!(none.unwrap_err(), ScheduleError::CadenceAmbiguous);

        let two = spec(serde_json::json!({
            "key": "k", "handler": "h", "intervalMinutes": 60, "cron": "0 7 * * *",
            "timeZone": "UTC",
        }));
        assert_eq!(two.unwrap_err(), ScheduleError::CadenceAmbiguous);
    }

    #[test]
    fn a_cron_needs_a_time_zone_and_an_interval_refuses_one() {
        let cron = spec(serde_json::json!({ "key": "k", "handler": "h", "cron": "0 7 * * *" }));
        assert_eq!(cron.unwrap_err(), ScheduleError::MissingTimeZone);

        let interval = spec(serde_json::json!({
            "key": "k", "handler": "h", "intervalMinutes": 60, "timeZone": "UTC",
        }));
        assert_eq!(interval.unwrap_err(), ScheduleError::TimeZoneWithInterval);
    }

    #[test]
    fn an_unknown_time_zone_is_refused() {
        let result = spec(serde_json::json!({
            "key": "k", "handler": "h", "cron": "0 7 * * *", "timeZone": "Mars/Olympus",
        }));
        assert!(matches!(result, Err(ScheduleError::UnknownTimeZone(_))));
    }

    #[test]
    fn a_cron_must_have_five_fields() {
        for pattern in ["0 0 7 * * *", "@daily", "7 * *", "not a cron at all"] {
            let result = spec(serde_json::json!({
                "key": "k", "handler": "h", "cron": pattern, "timeZone": "UTC",
            }));
            assert!(
                matches!(result, Err(ScheduleError::InvalidCron(_))),
                "{pattern} should be refused"
            );
        }
    }

    #[test]
    fn the_floor_refuses_every_minute_and_passes_daily() {
        let every_minute = spec(serde_json::json!({
            "key": "k", "handler": "h", "cron": "* * * * *", "timeZone": "UTC",
        }));
        assert_eq!(every_minute.unwrap_err(), ScheduleError::TooFrequent(15));

        let close_pair = spec(serde_json::json!({
            "key": "k", "handler": "h", "cron": "0,5 7 * * *", "timeZone": "UTC",
        }));
        assert_eq!(close_pair.unwrap_err(), ScheduleError::TooFrequent(15));

        let short_interval = spec(serde_json::json!({
            "key": "k", "handler": "h", "intervalMinutes": 5,
        }));
        assert_eq!(short_interval.unwrap_err(), ScheduleError::TooFrequent(15));

        let daily = spec(serde_json::json!({
            "key": "k", "handler": "h", "cron": "0 7 * * *", "timeZone": "Europe/Helsinki",
        }));
        assert!(daily.is_ok());

        let quarter_hour = spec(serde_json::json!({
            "key": "k", "handler": "h", "intervalMinutes": 15,
        }));
        assert!(quarter_hour.is_ok());
    }

    #[test]
    fn a_key_is_required_and_bounded() {
        let missing =
            spec(serde_json::json!({ "key": "  ", "handler": "h", "intervalMinutes": 60 }));
        assert_eq!(missing.unwrap_err(), ScheduleError::MissingKey);

        let long = spec(serde_json::json!({
            "key": "k".repeat(MAX_KEY_CHARS + 1), "handler": "h", "intervalMinutes": 60,
        }));
        assert_eq!(long.unwrap_err(), ScheduleError::KeyTooLong);
    }

    #[test]
    fn the_lane_distinguishes_absent_from_null() {
        let absent = spec(serde_json::json!({ "key": "k", "handler": "h", "intervalMinutes": 60 }))
            .expect("valid");
        assert_eq!(absent.lane, None);

        let null = spec(serde_json::json!({
            "key": "k", "handler": "h", "intervalMinutes": 60, "lane": null,
        }))
        .expect("valid");
        assert_eq!(null.lane, Some(None));
    }

    /// After downtime an interval runs once, at the next slot after now, and
    /// stays on its grid rather than drifting to "now + interval".
    #[test]
    fn an_interval_skips_missed_slots_and_keeps_its_grid() {
        let hour = Cadence::Interval { ms: 3_600_000 };
        let previous = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 5, 30, 0).unwrap();
        assert_eq!(
            hour.next_after(Some(previous), now),
            Utc.with_ymd_and_hms(2026, 1, 1, 6, 0, 0).unwrap()
        );

        let exactly_on_a_slot = Utc.with_ymd_and_hms(2026, 1, 1, 6, 0, 0).unwrap();
        assert_eq!(
            hour.next_after(Some(previous), exactly_on_a_slot),
            Utc.with_ymd_and_hms(2026, 1, 1, 7, 0, 0).unwrap()
        );
    }

    fn helsinki_two_am() -> Cadence {
        Cadence::cron("0 2 * * *", "Europe/Helsinki").expect("valid")
    }

    /// Spring forward: in Helsinki 03:00 becomes 04:00 on 2026-03-29, so 02:00
    /// exists (EET, UTC+2) and runs at 00:00 UTC. The gap case is 03:30.
    #[test]
    fn a_local_time_in_the_spring_gap_runs_at_the_first_instant_after_it() {
        let gap = Cadence::cron("30 3 * * *", "Europe/Helsinki").expect("valid");
        let before = Utc.with_ymd_and_hms(2026, 3, 28, 12, 0, 0).unwrap();
        let next = gap.next_after(None, before);
        // 04:00 EEST is 01:00 UTC.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 29, 1, 0, 0).unwrap());

        let daily = helsinki_two_am();
        assert_eq!(
            daily.next_after(None, before),
            Utc.with_ymd_and_hms(2026, 3, 29, 0, 0, 0).unwrap()
        );
    }

    /// Fall back: in Helsinki 04:00 becomes 03:00 on 2026-10-25, so 03:30
    /// happens twice. It runs once, at the first.
    #[test]
    fn a_local_time_in_the_autumn_overlap_runs_once_at_the_first() {
        let overlap = Cadence::cron("30 3 * * *", "Europe/Helsinki").expect("valid");
        let before = Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap();
        let first = overlap.next_after(None, before);
        // 03:30 EEST is 00:30 UTC.
        assert_eq!(first, Utc.with_ymd_and_hms(2026, 10, 25, 0, 30, 0).unwrap());

        let second = overlap.next_after(Some(first), first);
        assert_eq!(
            second,
            Utc.with_ymd_and_hms(2026, 10, 26, 1, 30, 0).unwrap()
        );
    }

    /// "07:00" stays at seven local time across the change.
    #[test]
    fn a_daily_cron_follows_local_time_across_daylight_saving() {
        let seven = Cadence::cron("0 7 * * *", "Europe/Helsinki").expect("valid");
        // The clocks go back at 04:00 on 2026-10-25: the 24th is still EEST
        // (UTC+3), the 25th is EET (UTC+2).
        let summer = seven.next_after(None, Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap());
        let winter = seven.next_after(Some(summer), summer);
        assert_eq!(summer, Utc.with_ymd_and_hms(2026, 10, 24, 4, 0, 0).unwrap());
        assert_eq!(winter, Utc.with_ymd_and_hms(2026, 10, 25, 5, 0, 0).unwrap());
    }

    #[test]
    fn cadences_describe_themselves() {
        assert_eq!(
            Cadence::cron("0 7 * * *", "Europe/Helsinki")
                .unwrap()
                .describe(),
            "daily at 07:00 (Europe/Helsinki)"
        );
        assert_eq!(
            Cadence::Interval { ms: 6 * 3_600_000 }.describe(),
            "every 6 hours"
        );
        assert_eq!(Cadence::Interval { ms: 3_600_000 }.describe(), "every hour");
        assert_eq!(
            Cadence::Interval { ms: 90 * 60_000 }.describe(),
            "every 90 minutes"
        );
    }
}
