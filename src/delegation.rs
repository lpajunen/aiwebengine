//! What a person has authorised a script to do as them while they are away.
//!
//! Background work that acts as somebody is a grant, and until this the engine
//! had no model for one. A scheduled handler runs as
//! `UserContext::admin("scheduler")`, so `fetch` resolving `{{secret:...}}`
//! looks for the person's key under a user literally named `scheduler`, misses,
//! and falls back to the script-wide secret; `personalStorage` reads
//! `context.request.auth.userId` and throws without one. So anything needing a
//! person's credential or a person's storage could only run inside that
//! person's request — an agent could work only while its owner's tab was open.
//!
//! The fix is not to widen the background context. It is to record what was
//! consented to, and to hold the background work to exactly that.
//!
//! # The four things a grant has to answer
//!
//! *What* — [`Scope`], a fixed vocabulary. Each value names something the
//! engine can actually gate on, because a scope nothing checks is a promise to
//! the person that nothing keeps.
//!
//! *For which script* — the grant is per `(user, script)`. Authorising one
//! solution to act for you says nothing about another.
//!
//! *For how long* — [`Grant::expires_at`], and there is no "forever" to pick.
//! A delegation that outlives the tab that created it by an unbounded amount is
//! the thing nobody consented to.
//!
//! *How they revoke it* — `/auth/account`, beside their sessions, because a
//! background job acting as you is the same question as a session acting as
//! you. Revoking cancels the work that was queued under it.
//!
//! # Why it is re-read rather than captured
//!
//! A task records *who* it runs as and nothing else. Every capability it gets
//! is worked out when it runs: the grant is looked up again, its expiry
//! checked again, and the account read again. This is the argument
//! `auth::refresh_tokens` already makes for minting a fresh session on every
//! refresh — a revocation that happened in between has to take effect rather
//! than being copied forward. A task queued this morning and run this evening
//! must not be the one place a withdrawn grant still holds.
//!
//! And the tier is capped at [`crate::security::UserContext::authenticated`]
//! however much the person holds. Editing a solution is not something
//! background work needs, so an administrator's delegated task runs with what
//! an ordinary request has and nothing more. Narrowing is the safe direction
//! and it is one sentence to state.

use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use tracing::{debug, warn};

use crate::security::UserContext;

/// How long a grant lasts when the person does not choose.
pub const DEFAULT_DURATION_DAYS: i64 = 30;

/// The longest one may last, whatever is asked for.
pub const MAX_DURATION_DAYS: i64 = 90;

/// What a person can authorise.
///
/// Deliberately short. Each value gates something real, and the list is the
/// list of things the engine can currently do as somebody who is not here —
/// adding a name without adding the check that enforces it would be a promise
/// the engine does not keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Read and write this person's `personalStorage` for this script.
    PersonalStorage,
    /// Resolve this person's secrets in `fetch` — their API key rather than
    /// the script's.
    Secrets,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::PersonalStorage => "personal_storage",
            Scope::Secrets => "secrets",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "personal_storage" => Some(Scope::PersonalStorage),
            "secrets" => Some(Scope::Secrets),
            _ => None,
        }
    }

    /// What the consent page says this lets the script do. Written for the
    /// person deciding, not for the developer asking.
    pub fn describe(self) -> &'static str {
        match self {
            Scope::PersonalStorage => "Read and change the data this app keeps for you",
            Scope::Secrets => "Use the API keys you have given this app",
        }
    }

    pub fn all() -> [Scope; 2] {
        [Scope::PersonalStorage, Scope::Secrets]
    }
}

/// A grant as stored.
#[derive(Debug, Clone)]
pub struct Grant {
    pub user_id: String,
    pub script_uri: String,
    pub scopes: Vec<Scope>,
    pub expires_at: DateTime<Utc>,
    pub granted_at: DateTime<Utc>,
}

impl Grant {
    pub fn allows(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.expires_at > now
    }

    fn from_row(row: &sqlx::postgres::PgRow) -> Self {
        let scopes: Vec<String> = row.get("scopes");
        Self {
            user_id: row.get("user_id"),
            script_uri: row.get("script_uri"),
            scopes: scopes.iter().filter_map(|s| Scope::parse(s)).collect(),
            expires_at: row.get("expires_at"),
            granted_at: row.get("granted_at"),
        }
    }
}

/// Clamp a requested duration to something the person can be held to.
pub fn bounded_duration(days: Option<i64>) -> Duration {
    Duration::days(
        days.unwrap_or(DEFAULT_DURATION_DAYS)
            .clamp(1, MAX_DURATION_DAYS),
    )
}

/// Record consent, replacing whatever this person had granted this script.
///
/// Replacing rather than merging: the consent page shows what is being asked
/// for and the person agrees to that, so the grant is what they just saw. A
/// union with an older grant would mean agreeing to something not on the page.
pub async fn grant(
    user_id: &str,
    script_uri: &str,
    scopes: &[Scope],
    duration: Duration,
) -> Result<Grant, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Err(sqlx::Error::PoolClosed);
    };

    let mut names: Vec<String> = scopes.iter().map(|s| s.as_str().to_string()).collect();
    names.sort();
    names.dedup();
    let expires_at = Utc::now() + duration;

    let row = sqlx::query(
        r#"
        INSERT INTO script_delegations (user_id, script_uri, scopes, expires_at, granted_at)
        VALUES ($1, $2, $3, $4, NOW())
        ON CONFLICT (user_id, script_uri)
        DO UPDATE SET scopes = EXCLUDED.scopes,
                      expires_at = EXCLUDED.expires_at,
                      granted_at = NOW()
        RETURNING user_id, script_uri, scopes, expires_at, granted_at
        "#,
    )
    .bind(user_id)
    .bind(script_uri)
    .bind(&names)
    .bind(expires_at)
    .fetch_one(db.pool())
    .await?;

    debug!(user = %user_id, script = %script_uri, scopes = ?names, "Delegation granted");
    Ok(Grant::from_row(&row))
}

/// The grant this person gave this script, if any. Expiry is not filtered here
/// — a caller deciding whether to run checks [`Grant::is_live`], and the
/// account page wants to show one that has lapsed.
pub async fn get(user_id: &str, script_uri: &str) -> Result<Option<Grant>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(None);
    };

    let row = sqlx::query(
        r#"
        SELECT user_id, script_uri, scopes, expires_at, granted_at
        FROM script_delegations
        WHERE user_id = $1 AND script_uri = $2
        "#,
    )
    .bind(user_id)
    .bind(script_uri)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.as_ref().map(Grant::from_row))
}

/// Everything this person has authorised, for the account page.
pub async fn list_for_user(user_id: &str) -> Result<Vec<Grant>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(Vec::new());
    };

    let rows = sqlx::query(
        r#"
        SELECT user_id, script_uri, scopes, expires_at, granted_at
        FROM script_delegations
        WHERE user_id = $1
        ORDER BY granted_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db.pool())
    .await?;

    Ok(rows.iter().map(Grant::from_row).collect())
}

/// Withdraw one grant, and cancel the work queued under it.
///
/// Both, in that order, because a grant that is gone while its tasks still run
/// is a revocation that revoked nothing the person could see. Cancelling
/// afterwards is safe even if it fails: the run-time check reads the grant
/// again and refuses.
pub async fn revoke(user_id: &str, script_uri: &str) -> Result<bool, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(false);
    };

    let result =
        sqlx::query("DELETE FROM script_delegations WHERE user_id = $1 AND script_uri = $2")
            .bind(user_id)
            .bind(script_uri)
            .execute(db.pool())
            .await?;

    let removed = result.rows_affected() > 0;
    if removed {
        let cancelled = crate::tasks::cancel_delegated(user_id, Some(script_uri)).await;
        debug!(
            user = %user_id,
            script = %script_uri,
            cancelled = cancelled.unwrap_or(0),
            "Delegation revoked"
        );
    }

    Ok(removed)
}

/// Withdraw every grant this person has made, and cancel everything queued
/// under them.
///
/// Reached from `security::delete_sessions_for_user`, which is what runs when
/// an account's roles change, its realm narrows, or it is deleted. A session
/// list cannot show background work, so "sign out everywhere" has to include
/// the work that would go on acting as you afterwards.
pub async fn revoke_all(user_id: &str) -> Result<u64, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(0);
    };

    let result = sqlx::query("DELETE FROM script_delegations WHERE user_id = $1")
        .bind(user_id)
        .execute(db.pool())
        .await?;

    let _ = crate::tasks::cancel_delegated(user_id, None).await;
    Ok(result.rows_affected())
}

/// Where to send somebody to authorise a script.
///
/// Composed here so the script, the account page and the consent handler all
/// name the same URL rather than three spellings of it.
pub fn consent_url(script_uri: &str) -> String {
    format!("/auth/delegate?script={}", urlencoding::encode(script_uri))
}

/// Why a delegated run was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing was ever granted, or it has been withdrawn.
    NoGrant,
    /// It was granted and has since lapsed.
    Expired,
    /// The account is gone.
    NoSuchUser,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoGrant => write!(
                f,
                "the person who queued this has not authorised this script to act for them"
            ),
            Refusal::Expired => write!(f, "the authorisation to act for this person has expired"),
            Refusal::NoSuchUser => write!(f, "the account this was queued for no longer exists"),
        }
    }
}

/// Who a delegated task runs as, worked out at the moment it runs.
#[derive(Debug, Clone)]
pub struct Delegated {
    pub user_context: UserContext,
    pub grant: Grant,
    pub email: Option<String>,
    pub name: Option<String>,
}

impl Delegated {
    pub fn user_id(&self) -> &str {
        // Built by `resolve`, which always sets it.
        self.user_context.user_id.as_deref().unwrap_or_default()
    }
}

/// Work out what a task acting as `user_id` may do, right now.
///
/// Nothing here is read from the task. The grant is looked up again, its
/// expiry checked again, and the account read again, so a grant withdrawn
/// between queueing and running takes effect rather than being carried forward
/// in the row.
///
/// The tier is capped at `authenticated` however much the person holds: a
/// background job has no business authoring a solution, and a delegated task
/// belonging to an administrator should not be the way to get one that does.
pub async fn resolve(user_id: &str, script_uri: &str) -> Result<Delegated, Refusal> {
    let grant = match get(user_id, script_uri).await {
        Ok(Some(grant)) => grant,
        Ok(None) => return Err(Refusal::NoGrant),
        Err(e) => {
            // A lookup that failed is not a grant. Refusing is the only safe
            // reading, and the task retries.
            warn!(user = %user_id, script = %script_uri, error = %e, "Could not read a delegation");
            return Err(Refusal::NoGrant);
        }
    };

    if !grant.is_live(Utc::now()) {
        return Err(Refusal::Expired);
    }

    let user = match crate::user_repository::get_user_async(user_id).await {
        Ok(user) => user,
        Err(_) => return Err(Refusal::NoSuchUser),
    };

    Ok(Delegated {
        user_context: UserContext::authenticated(user_id.to_string()),
        grant,
        email: user.email.clone(),
        name: user.name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scope_round_trips_through_its_name() {
        for scope in Scope::all() {
            assert_eq!(Scope::parse(scope.as_str()), Some(scope));
        }
    }

    /// A name nothing gates on would be a promise to the person that nothing
    /// keeps, so an unknown one is dropped rather than stored.
    #[test]
    fn an_unknown_scope_is_not_a_scope() {
        assert_eq!(Scope::parse("administer_everything"), None);
        assert_eq!(Scope::parse(""), None);
    }

    #[test]
    fn a_duration_is_clamped_to_something_a_person_can_be_held_to() {
        assert_eq!(
            bounded_duration(None),
            Duration::days(DEFAULT_DURATION_DAYS)
        );
        assert_eq!(bounded_duration(Some(0)), Duration::days(1));
        assert_eq!(bounded_duration(Some(-5)), Duration::days(1));
        assert_eq!(
            bounded_duration(Some(9999)),
            Duration::days(MAX_DURATION_DAYS)
        );
        assert_eq!(bounded_duration(Some(7)), Duration::days(7));
    }

    fn a_grant(scopes: Vec<Scope>, expires_at: DateTime<Utc>) -> Grant {
        Grant {
            user_id: "u1".to_string(),
            script_uri: "test://script".to_string(),
            scopes,
            expires_at,
            granted_at: Utc::now(),
        }
    }

    #[test]
    fn a_grant_allows_only_what_it_names() {
        let grant = a_grant(vec![Scope::PersonalStorage], Utc::now() + Duration::days(1));
        assert!(grant.allows(Scope::PersonalStorage));
        assert!(
            !grant.allows(Scope::Secrets),
            "a scope that was not consented to is not granted by proximity"
        );
    }

    /// There is no "forever", so a grant is always eventually not live.
    #[test]
    fn a_lapsed_grant_is_not_live() {
        let grant = a_grant(Scope::all().to_vec(), Utc::now() - Duration::minutes(1));
        assert!(!grant.is_live(Utc::now()));

        let live = a_grant(Scope::all().to_vec(), Utc::now() + Duration::minutes(1));
        assert!(live.is_live(Utc::now()));
    }

    /// The tier a delegated task gets is capped regardless of the person's
    /// roles, so this is the set it must never exceed.
    #[test]
    fn the_delegated_tier_is_no_more_than_an_ordinary_request_holds() {
        let delegated = UserContext::authenticated("u1".to_string());
        let editor = UserContext::editor("u1".to_string());

        assert!(
            !delegated
                .capabilities
                .contains(&crate::security::Capability::WriteScripts),
            "background work must not be able to author a solution"
        );
        assert!(
            !delegated
                .capabilities
                .contains(&crate::security::Capability::AdministerEngine),
            "background work must not be able to administer the engine"
        );
        assert!(
            delegated.capabilities.len() < editor.capabilities.len(),
            "the delegated tier should be narrower than the authoring one"
        );
    }
}
