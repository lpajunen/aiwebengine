//! Following a repository on a schedule.
//!
//! A follow (`git_follows`) records what this engine takes from a repository,
//! branch and prefix. Pulling it by hand needs nothing more. Scheduling it
//! adds how often, what a run does (`Mode`), and whose authority it runs
//! with — because a scheduled pull writes scripts while nobody is there.
//!
//! A run acts as the account that set the schedule. That account is read again
//! on every run, capped at editor and narrowed to reading and writing scripts,
//! so it writes only scripts the account owns, and a role taken away takes
//! effect on the next run. Anything that changes what the account may do
//! pauses its follows (`pause_for`, from `security::delete_sessions_for_user`),
//! as it withdraws the account's delegations. Only the person can set one up,
//! in their own request: background work acting for them cannot extend itself
//! past its grant by scheduling a follow.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use sqlx::Row;
use tracing::warn;

use crate::git_sync::{PullReport, PullRequest, SyncError};
use crate::security::Capability;
use crate::security::capabilities::{ActingKind, SessionRoles, UserContext};

/// The shortest interval a schedule may name. An unchanged repository costs
/// two GitHub API calls a run, and an unauthenticated caller has sixty an hour.
pub const MIN_EVERY_MINUTES: i32 = 10;

/// The longest: a week.
pub const MAX_EVERY_MINUTES: i32 = 7 * 24 * 60;

/// How many due follows one tick takes, so one slow repository does not hold
/// the rest of a large backlog for a whole pass.
const BATCH: i64 = 10;

/// What a scheduled run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Record whether the repository has moved, and write nothing.
    Check,
    /// Pull, and keep every followed script serving the revision it served
    /// before: each is pinned first, so the pull lands in head for
    /// `deploy_script` to serve. A script the pull creates has nothing to keep
    /// serving and is live at once.
    Stage,
    /// Pull and serve what was pulled, unless a script is already pinned.
    Apply,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Check => "check",
            Mode::Stage => "stage",
            Mode::Apply => "apply",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "check" => Some(Mode::Check),
            "stage" => Some(Mode::Stage),
            "apply" => Some(Mode::Apply),
            _ => None,
        }
    }
}

/// How a follow's last run went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// It ran: pulled, found nothing to pull, or checked.
    Ok,
    /// A script it follows was changed here, so the pull was refused; it stays
    /// refused until the change is pushed or pulled over with force.
    Blocked,
    /// The repository needs a newer engine; the first run after an update
    /// goes through.
    Incompatible,
    /// Anything else: the repository could not be read, a script belongs to
    /// another account, the budget was spent.
    Failing,
    /// It no longer runs until scheduled again.
    Paused,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Ok => "ok",
            State::Blocked => "blocked",
            State::Incompatible => "incompatible",
            State::Failing => "failing",
            State::Paused => "paused",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ok" => Some(State::Ok),
            "blocked" => Some(State::Blocked),
            "incompatible" => Some(State::Incompatible),
            "failing" => Some(State::Failing),
            "paused" => Some(State::Paused),
            _ => None,
        }
    }
}

/// How one run went.
#[derive(Debug, Clone)]
pub struct Ran {
    pub state: State,
    pub outcome: String,
    /// The scripts the outcome is about, whose logs hear of it when the state
    /// changes; empty means every script the follow has written.
    pub about: Vec<String>,
}

impl Ran {
    fn new(state: State, outcome: impl Into<String>) -> Self {
        Ran {
            state,
            outcome: outcome.into(),
            about: Vec::new(),
        }
    }

    fn from_result(result: Result<String, SyncError>) -> Self {
        match result {
            Ok(outcome) => Ran::new(State::Ok, outcome),
            Err(SyncError::ChangedHere { scripts, message }) => Ran {
                state: State::Blocked,
                outcome: message,
                about: scripts,
            },
            Err(e @ SyncError::Incompatible(_)) => Ran::new(State::Incompatible, e.to_string()),
            Err(e) => Ran::new(State::Failing, e.to_string()),
        }
    }
}

/// A follow as stored.
#[derive(Debug, Clone)]
pub struct Follow {
    pub remote: String,
    pub branch: String,
    pub uri_base: String,
    /// The directories followed, or `None` for the whole repository.
    pub scripts: Option<BTreeSet<String>>,
    pub every_minutes: Option<i32>,
    pub mode: Mode,
    pub acting_user: Option<String>,
    /// Who first recorded it, by pulling or scheduling.
    pub created_by: Option<String>,
    pub next_due_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_state: Option<State>,
    pub last_outcome: Option<String>,
}

/// What a caller asks for when scheduling a follow.
#[derive(Debug, Clone)]
pub struct FollowRequest {
    pub repo: String,
    /// `None` reads the repository's default branch.
    pub branch: Option<String>,
    pub prefix: Option<String>,
    /// Directories to add to what is followed, as a pull naming them would.
    pub scripts: Option<Vec<String>>,
    /// Minutes between runs; 0 stops the schedule and keeps the follow.
    pub every_minutes: i32,
    pub mode: Mode,
}

fn pool() -> Result<sqlx::PgPool, SyncError> {
    crate::repository::get_db_pool()
        .map(|db| db.pool().clone())
        .ok_or_else(|| SyncError::Storage("No database is configured".to_string()))
}

fn storage(e: sqlx::Error) -> SyncError {
    SyncError::Storage(format!("Database error on a git follow: {}", e))
}

/// Every column a [`Follow`] reads, as a literal so the statements stay ones
/// sqlx can see whole.
macro_rules! columns {
    () => {
        "remote, branch, uri_base, scripts, every_minutes, mode, acting_user, created_by, \
         next_due_at, last_run_at, last_state, last_outcome"
    };
}

fn follow_from(row: &sqlx::postgres::PgRow) -> Follow {
    Follow {
        remote: row.get("remote"),
        branch: row.get("branch"),
        uri_base: row.get("uri_base"),
        scripts: row
            .get::<Option<Vec<String>>, _>("scripts")
            .map(|scripts| scripts.into_iter().collect()),
        every_minutes: row.get("every_minutes"),
        mode: Mode::parse(row.get::<String, _>("mode").as_str()).unwrap_or(Mode::Check),
        acting_user: row.get("acting_user"),
        created_by: row.get("created_by"),
        next_due_at: row.get("next_due_at"),
        last_run_at: row.get("last_run_at"),
        last_state: row
            .get::<Option<String>, _>("last_state")
            .as_deref()
            .and_then(State::parse),
        last_outcome: row.get("last_outcome"),
    }
}

/// One follow, if this engine has it.
pub async fn get(remote: &str, branch: &str, uri_base: &str) -> Result<Option<Follow>, SyncError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM git_follows WHERE remote = $1 AND branch = $2 AND uri_base = $3"
    ))
    .bind(remote)
    .bind(branch)
    .bind(uri_base)
    .fetch_optional(&pool()?)
    .await
    .map_err(storage)?;
    Ok(row.as_ref().map(follow_from))
}

/// Every follow this engine has. Filtered by the caller with [`visible_to`].
pub async fn list() -> Result<Vec<Follow>, SyncError> {
    let rows = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM git_follows ORDER BY remote, branch, uri_base"
    ))
    .fetch_all(&pool()?)
    .await
    .map_err(storage)?;
    Ok(rows.iter().map(follow_from).collect())
}

/// The follow that covers `script_uri`, if one does: the one recorded for the
/// repository, branch and prefix it was synced from, following its directory.
pub async fn for_script(script_uri: &str) -> Result<Option<Follow>, SyncError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM git_follows f WHERE EXISTS ( \
             SELECT 1 FROM script_git_sync g \
             WHERE g.script_uri = $1 AND g.remote = f.remote AND g.branch = f.branch \
               AND g.uri_base = f.uri_base \
               AND (f.scripts IS NULL OR g.repo_dir = ANY(f.scripts)))"
    ))
    .bind(script_uri)
    .fetch_optional(&pool()?)
    .await
    .map_err(storage)?;
    Ok(row.as_ref().map(follow_from))
}

/// Whether `user` may see `follow`: an administrator, the account it runs as,
/// and whoever recorded it.
pub fn visible_to(follow: &Follow, user: &UserContext) -> bool {
    if user.has_capability(&Capability::AdministerEngine) {
        return true;
    }
    let Some(user_id) = user.user_id.as_deref() else {
        return false;
    };
    follow.acting_user.as_deref() == Some(user_id) || follow.created_by.as_deref() == Some(user_id)
}

// ============================================================================
// Setting a schedule
// ============================================================================

/// Schedule, change or stop a follow.
pub async fn schedule(user: &UserContext, request: FollowRequest) -> Result<Follow, SyncError> {
    let client = crate::git_sync::client_for(user).await?;
    schedule_with(client, user, request).await
}

/// [`schedule`], against a GitHub client the caller supplies.
pub async fn schedule_with(
    client: crate::git_github::GitHubClient,
    user: &UserContext,
    request: FollowRequest,
) -> Result<Follow, SyncError> {
    if !user.has_capability(&Capability::WriteScripts) {
        return Err(SyncError::AccessDenied(
            "Following a repository writes scripts, which takes write_scripts".to_string(),
        ));
    }
    // Background work under a delegation holds what its grant says, until the
    // grant ends. A follow it scheduled would go on writing after that.
    if !matches!(user.acting.kind, ActingKind::Session | ActingKind::Token) {
        return Err(SyncError::AccessDenied(
            "A follow acts as you while you are away, so only you can set one up, not work \
             running for you"
                .to_string(),
        ));
    }
    let Some(user_id) = user.user_id.clone() else {
        return Err(SyncError::AccessDenied(
            "Sign in to follow a repository".to_string(),
        ));
    };
    let every = request.every_minutes;
    if every != 0 && !(MIN_EVERY_MINUTES..=MAX_EVERY_MINUTES).contains(&every) {
        return Err(SyncError::Layout(format!(
            "every_minutes is {} to {}, or 0 to stop",
            MIN_EVERY_MINUTES, MAX_EVERY_MINUTES
        )));
    }

    let repo = crate::git_github::RepoRef::parse(&request.repo)?;
    let remote = repo.to_string();
    let branch = match request.branch.clone() {
        Some(branch) => branch,
        None => tokio::task::spawn_blocking(move || client.default_branch(&repo))
            .await
            .map_err(|e| SyncError::Storage(format!("Could not read the branch: {}", e)))??,
    };
    let uri_base =
        crate::git_sync::normalize_prefix(request.prefix.as_deref())?.unwrap_or_default();
    let named = crate::git_sync::selection(request.scripts.as_deref())?;

    let existing = get(&remote, &branch, &uri_base).await?;
    if let Some(acting) = existing.as_ref().and_then(|f| f.acting_user.as_deref())
        && acting != user_id
        && !user.has_capability(&Capability::AdministerEngine)
    {
        return Err(SyncError::AccessDenied(
            "This follow runs as another account. Only that account or an administrator can \
             change it."
                .to_string(),
        ));
    }

    if every == 0 {
        if existing.is_none() {
            return Err(SyncError::Layout(format!(
                "This engine does not follow {} {} under '{}'",
                remote, branch, uri_base
            )));
        }
        sqlx::query(
            "UPDATE git_follows SET every_minutes = NULL, acting_user = NULL, \
             next_due_at = NULL, updated_at = NOW() \
             WHERE remote = $1 AND branch = $2 AND uri_base = $3",
        )
        .bind(&remote)
        .bind(&branch)
        .bind(&uri_base)
        .execute(&pool()?)
        .await
        .map_err(storage)?;
    } else {
        let known = crate::git_sync::last_synced(&remote, &branch)
            .await
            .map_err(|e| SyncError::Storage(e.to_string()))?;
        let binding: Vec<&crate::git_sync::SyncedScript> = known
            .iter()
            .filter(|row| row.uri_base.as_deref() == Some(uri_base.as_str()))
            .collect();
        let before = crate::git_sync::with_older_bindings(
            existing.as_ref().map(|f| f.scripts.clone()),
            &binding,
        );
        let scripts: Option<Vec<String>> = crate::git_sync::following_after(before, named.as_ref())
            .map(|scripts| scripts.into_iter().collect());

        sqlx::query(
            r#"
            INSERT INTO git_follows
                (remote, branch, uri_base, scripts, created_by,
                 every_minutes, mode, acting_user, next_due_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $5, NOW())
            ON CONFLICT (remote, branch, uri_base) DO UPDATE SET
                scripts = EXCLUDED.scripts,
                every_minutes = EXCLUDED.every_minutes,
                mode = EXCLUDED.mode,
                acting_user = EXCLUDED.acting_user,
                next_due_at = EXCLUDED.next_due_at,
                updated_at = NOW()
            "#,
        )
        .bind(&remote)
        .bind(&branch)
        .bind(&uri_base)
        .bind(scripts)
        .bind(&user_id)
        .bind(every)
        .bind(request.mode.as_str())
        .execute(&pool()?)
        .await
        .map_err(storage)?;
    }

    get(&remote, &branch, &uri_base)
        .await?
        .ok_or_else(|| SyncError::Storage("The follow vanished after storing".to_string()))
}

/// Stop every follow `user_id` scheduled.
///
/// Reached from `security::delete_sessions_for_user`, which runs whenever what
/// an account may do changes. A follow keeps acting as its account until then,
/// so this is where it stops.
pub async fn pause_for(user_id: &str) -> Result<u64, SyncError> {
    let result = sqlx::query(
        "UPDATE git_follows SET every_minutes = NULL, acting_user = NULL, next_due_at = NULL, \
         last_state = 'paused', last_outcome = $2, updated_at = NOW() WHERE acting_user = $1",
    )
    .bind(user_id)
    .bind("Paused: what the account that scheduled it may do changed. Schedule it again to resume.")
    .execute(&pool()?)
    .await
    .map_err(storage)?;
    Ok(result.rows_affected())
}

// ============================================================================
// Running
// ============================================================================

/// Run the follows that are due, every minute, until shutdown.
pub fn spawn_runner(shutdown: tokio::sync::oneshot::Receiver<()>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        // Not in the middle of startup.
        ticker.tick().await;

        let mut shutdown = shutdown;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = run_due().await {
                        warn!("Running due git follows failed: {}", e);
                    }
                }
                _ = &mut shutdown => return,
            }
        }
    });
}

/// Claim the follows that are due and run each once.
///
/// Claiming moves `next_due_at` on in the same statement, so another instance
/// asking at the same moment skips the row, and an engine that slept through
/// several intervals runs each follow once rather than once per interval
/// missed.
pub async fn run_due() -> Result<usize, SyncError> {
    let due = claim_due().await?;
    for follow in &due {
        let ran = run(follow, None).await;
        record(follow, &ran).await?;
    }
    Ok(due.len())
}

/// The next run is stepped from when this one was due, not from the tick that
/// claimed it, so the interval holds against a tick a few milliseconds early;
/// one missed by more than an interval starts again from now.
pub async fn claim_due() -> Result<Vec<Follow>, SyncError> {
    let rows = sqlx::query(
        "UPDATE git_follows f \
         SET next_due_at = CASE \
             WHEN f.next_due_at + make_interval(mins => f.every_minutes) > NOW() \
             THEN f.next_due_at + make_interval(mins => f.every_minutes) \
             ELSE NOW() + make_interval(mins => f.every_minutes) END \
         FROM ( \
             SELECT remote, branch, uri_base FROM git_follows \
             WHERE every_minutes IS NOT NULL AND next_due_at <= NOW() \
             ORDER BY next_due_at LIMIT $1 FOR UPDATE SKIP LOCKED \
         ) due \
         WHERE f.remote = due.remote AND f.branch = due.branch AND f.uri_base = due.uri_base \
         RETURNING f.remote, f.branch, f.uri_base, f.scripts, f.every_minutes, f.mode, \
                   f.acting_user, f.created_by, f.next_due_at, f.last_run_at, f.last_state, f.last_outcome",
    )
    .bind(BATCH)
    .fetch_all(&pool()?)
    .await
    .map_err(storage)?;
    Ok(rows.iter().map(follow_from).collect())
}

/// Record how a run went.
///
/// A follow that stops going through says so in the logs of the scripts it is
/// about, once, when its state changes rather than every interval: a pull
/// refused over a change made here is otherwise visible only to whoever thinks
/// to ask, and nobody waiting for updates that stopped arriving does.
pub async fn record(follow: &Follow, ran: &Ran) -> Result<(), SyncError> {
    sqlx::query(
        "UPDATE git_follows SET last_run_at = NOW(), last_state = $4, last_outcome = $5 \
         WHERE remote = $1 AND branch = $2 AND uri_base = $3",
    )
    .bind(&follow.remote)
    .bind(&follow.branch)
    .bind(&follow.uri_base)
    .bind(ran.state.as_str())
    .bind(&ran.outcome)
    .execute(&pool()?)
    .await
    .map_err(storage)?;

    let before = follow.last_state;
    if before == Some(ran.state) || (before.is_none() && ran.state == State::Ok) {
        return Ok(());
    }
    let (level, line) = if ran.state == State::Ok {
        (
            "INFO",
            format!("Following {} again: {}", follow.remote, ran.outcome),
        )
    } else {
        (
            "WARN",
            format!(
                "Following {} has stopped ({}): {}",
                follow.remote,
                ran.state.as_str(),
                ran.outcome
            ),
        )
    };
    let about = if ran.about.is_empty() {
        followed_scripts(follow)
            .await
            .into_iter()
            .map(|row| row.script_uri)
            .collect()
    } else {
        ran.about.clone()
    };
    for script_uri in about {
        crate::repository::insert_log_message_async(&script_uri, &line, level).await;
    }
    Ok(())
}

/// Run one follow once, as its account, and say how it went.
///
/// `client` is for the suite; a scheduled run builds its own with the
/// account's stored credential.
pub async fn run(follow: &Follow, client: Option<crate::git_github::GitHubClient>) -> Ran {
    let Some(user_id) = follow.acting_user.as_deref() else {
        return Ran::new(State::Paused, "Not scheduled");
    };
    let acting = match acting_context(user_id).await {
        Ok(acting) => acting,
        Err(reason) => {
            // It would fail the same way every time, so it stops here rather
            // than filling the log once an interval.
            if let Err(e) = pause_for(user_id).await {
                warn!(user = %user_id, error = %e, "Could not pause a follow that cannot run");
            }
            return Ran::new(State::Paused, format!("Paused: {}", reason));
        }
    };
    let client = match client {
        Some(client) => client,
        None => match crate::git_sync::client_for(&acting).await {
            Ok(client) => client,
            Err(e) => return Ran::new(State::Failing, e.to_string()),
        },
    };

    match follow.mode {
        Mode::Check => match check(follow, client).await {
            (true, outcome) => Ran::new(State::Ok, outcome),
            (false, outcome) => Ran::new(State::Failing, outcome),
        },
        Mode::Stage => {
            let pinned = pin_followed(follow, &acting).await;
            Ran::from_result(pull(follow, client, &acting).await.map(|report| {
                format!(
                    "{}{}",
                    describe(&report),
                    if pinned > 0 {
                        format!(
                            "; pinned {} to keep serving, deploy_script serves the pull",
                            pinned
                        )
                    } else {
                        String::new()
                    }
                )
            }))
        }
        Mode::Apply => Ran::from_result(
            pull(follow, client, &acting)
                .await
                .map(|report| describe(&report)),
        ),
    }
}

/// The context a scheduled run acts with, worked out now.
///
/// The account as stored at this moment, capped at editor whatever it holds —
/// a follow writes scripts the account owns and never acts on anybody else's —
/// and then narrowed to reading and writing scripts and their files. Built as a session with
/// no step-up, so an engine that gates writing behind one refuses a follow
/// rather than letting it write without.
pub async fn acting_context(user_id: &str) -> Result<UserContext, String> {
    use crate::user_repository::UserRole;

    let user = crate::user_repository::get_user_async(user_id)
        .await
        .map_err(|_| "the account that scheduled it no longer exists".to_string())?;
    if !user
        .roles
        .iter()
        .any(|role| matches!(role, UserRole::Editor | UserRole::Administrator))
    {
        return Err("the account that scheduled it can no longer write scripts".to_string());
    }

    let mut context = UserContext::for_session(SessionRoles {
        user_id: Some(user_id),
        is_admin: false,
        is_editor: true,
        elevation: None,
        via_token: false,
    });
    context.acting.kind = ActingKind::Delegated;
    let context = context.attenuated([
        Capability::ReadScripts,
        Capability::WriteScripts,
        Capability::ReadAssets,
        Capability::WriteAssets,
    ]);
    if !context.has_capability(&Capability::WriteScripts) {
        return Err(
            "this engine requires stepping up in person to write scripts, which a scheduled \
             run cannot do"
                .to_string(),
        );
    }
    Ok(context)
}

async fn pull(
    follow: &Follow,
    client: crate::git_github::GitHubClient,
    acting: &UserContext,
) -> Result<PullReport, SyncError> {
    crate::git_sync::pull_through(
        client,
        acting,
        PullRequest {
            repo: follow.remote.clone(),
            branch: Some(follow.branch.clone()),
            prefix: Some(follow.uri_base.clone()).filter(|prefix| !prefix.is_empty()),
            force: false,
            // What the follow names is what the pull reads, from the same row.
            scripts: None,
        },
    )
    .await
}

fn describe(report: &PullReport) -> String {
    let short = &report.commit[..7.min(report.commit.len())];
    if report.up_to_date {
        return format!("Up to date at {}", short);
    }
    let changed: Vec<&str> = report
        .scripts
        .iter()
        .filter(|script| script.changed)
        .map(|script| script.script_uri.as_str())
        .collect();
    if changed.is_empty() {
        format!("Nothing changed at {}", short)
    } else {
        format!("Pulled {}: {}", short, changed.join(", "))
    }
}

/// The scripts this follow has written here.
async fn followed_scripts(follow: &Follow) -> Vec<crate::git_sync::SyncedScript> {
    crate::git_sync::last_synced(&follow.remote, &follow.branch)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.uri_base.as_deref() == Some(follow.uri_base.as_str()))
        .filter(|row| match (&follow.scripts, &row.repo_dir) {
            (None, _) => true,
            (Some(scripts), Some(dir)) => scripts.contains(dir),
            (Some(_), None) => false,
        })
        .collect()
}

/// Whether the repository has moved past what this engine holds.
async fn check(follow: &Follow, client: crate::git_github::GitHubClient) -> (bool, String) {
    let repo = match crate::git_github::RepoRef::parse(&follow.remote) {
        Ok(repo) => repo,
        Err(e) => return (false, e.to_string()),
    };
    let branch = follow.branch.clone();
    let resolved =
        match tokio::task::spawn_blocking(move || client.resolve_branch(&repo, &branch)).await {
            Ok(Ok(resolved)) => resolved,
            Ok(Err(e)) => return (false, e.to_string()),
            Err(e) => return (false, format!("Check did not finish: {}", e)),
        };
    let short = &resolved.commit[..7.min(resolved.commit.len())];

    let held = followed_scripts(follow).await;
    let missing = follow
        .scripts
        .as_ref()
        .is_some_and(|scripts| scripts.len() > held.len());
    let behind = missing || held.is_empty() || held.iter().any(|row| row.commit != resolved.commit);
    if behind {
        (true, format!("Behind: {} is at {}", follow.remote, short))
    } else {
        (true, format!("Up to date at {}", short))
    }
}

/// Pin each followed script that is not pinned to the revision it serves now,
/// so a pull lands in head without going live. Returns how many it pinned.
async fn pin_followed(follow: &Follow, acting: &UserContext) -> usize {
    let mut pinned = 0;
    for row in followed_scripts(follow).await {
        let uri = row.script_uri;
        if crate::deployments::pinned(&uri).is_some()
            || !crate::engine_api::can_write_script(acting, &uri)
        {
            continue;
        }
        let Ok(Some(head)) = crate::revisions::head(&uri).await else {
            continue;
        };
        match crate::deployments::deploy(&uri, head, acting.user_id.as_deref()).await {
            Ok(_) => pinned += 1,
            Err(e) => warn!(script = %uri, error = ?e, "Could not pin a staged script"),
        }
    }
    pinned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_round_trips_through_its_name() {
        for mode in [Mode::Check, Mode::Stage, Mode::Apply] {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(Mode::parse("merge"), None);
    }
}
