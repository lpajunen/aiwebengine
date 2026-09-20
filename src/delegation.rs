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

use crate::security::{Capability, UserContext};

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
///
/// # Two nouns and a verb
///
/// [`Scope::PersonalStorage`] and [`Scope::Secrets`] name *what* a delegation
/// reaches — which of the person's things are in scope at all. For a long
/// time they were the whole vocabulary, and nothing named *what it may do
/// with them*, so every grant was a grant to change as well as to read.
///
/// [`Scope::Write`] is the verb, and it could not exist until there was
/// somewhere to enforce it. A read-only scope needs a read-only context, and
/// a `UserContext` had no way to hold less than a tier until
/// [`crate::security::UserContext::attenuated`] — which is why this and
/// capability attenuation are one piece of work approached from opposite
/// ends.
///
/// The two kinds are enforced differently and deliberately so. A noun is
/// checked where the person's thing is reached
/// ([`crate::security::secure_globals::GlobalSecurityConfig::allows_delegated`]),
/// and answers as though that thing were not there — the refusal a script
/// already handles for "nobody is signed in", rather than a new failure mode.
/// The verb is a capability, so it is checked by the same gate that checks
/// every other caller, underneath the JavaScript, at every write there is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Reach this person's `personalStorage` for this script.
    ///
    /// Reaching is not changing: writing it also takes [`Scope::Write`].
    PersonalStorage,
    /// Resolve this person's secrets in `fetch` — their API key rather than
    /// the script's.
    Secrets,
    /// Change things, rather than only reading them.
    ///
    /// Without it a delegated run holds no write capability at all: not the
    /// script's tables, not either storage, not the queue, not the message
    /// dispatcher. That is the "plan approved in advance" this vocabulary
    /// could not express — a person authorising an app to go away and *work
    /// out* what to do, without authorising it to do the thing.
    Write,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::PersonalStorage => "personal_storage",
            Scope::Secrets => "secrets",
            Scope::Write => "write",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "personal_storage" => Some(Scope::PersonalStorage),
            "secrets" => Some(Scope::Secrets),
            "write" => Some(Scope::Write),
            _ => None,
        }
    }

    /// What the consent page says this lets the script do. Written for the
    /// person deciding, not for the developer asking.
    pub fn describe(self) -> &'static str {
        match self {
            Scope::PersonalStorage => "Read the data this app keeps for you",
            Scope::Secrets => "Use the API keys you have given this app",
            Scope::Write => "Change things, not just read them",
        }
    }

    /// What this scope adds to the delegated context.
    ///
    /// Empty for the nouns: they decide whether the person's own things are
    /// in scope, which is a question about *whose* data rather than about
    /// what may be done to it, and is answered at the two surfaces that reach
    /// it rather than by a capability.
    pub fn capabilities(self) -> &'static [Capability] {
        match self {
            Scope::PersonalStorage | Scope::Secrets => &[],
            Scope::Write => &[
                Capability::WriteScriptData,
                Capability::WriteStorage,
                // Queueing and dispatching are both "set something in motion
                // that outlives this run". Neither can escalate — a queued
                // personal task re-resolves this same grant, and a listener
                // runs under the sending context — so what they are refused
                // for is the plainer reason: a person who ticked nothing but
                // "read" would not expect the app to have started anything.
                //
                // The cost is real and worth naming: work longer than one
                // budget is a chain of tasks, so a read-only delegation
                // cannot be a long one. If that turns out to matter the
                // answer is another scope, not a hole in this one.
                Capability::EnqueueTasks,
                Capability::SendMessages,
            ],
        }
    }

    pub fn all() -> [Scope; 3] {
        [Scope::PersonalStorage, Scope::Secrets, Scope::Write]
    }
}

/// What every live delegation holds, whatever was ticked.
///
/// Reading is the floor rather than a grant, because a delegation that could
/// not read would have nothing to act on and there would be no point
/// consenting to it. So this is `authenticated` minus the writes, and the
/// scopes add back from there.
///
/// Three of these are worth the sentence:
///
/// `UseNetwork` is here because a delegated run that cannot call out is a
/// delegated run that cannot do the thing people delegate — check a feed,
/// ask a model. Making it a scope would be a separate decision with its own
/// checkbox, and a defensible one; it is not this change.
///
/// `ReadSecrets` gates secrets in general, where [`Scope::Secrets`] gates
/// *this person's* secrets specifically. Without it here, a grant that did
/// not mention secrets could not use even the script's own key, which is a
/// narrowing nobody asked for.
///
/// `ManageStreams` is how a background run tells the person what it did. A
/// read-only delegation that could not report back would be mute, and the
/// message is not a change to anything.
fn base_capabilities() -> Vec<Capability> {
    vec![
        Capability::ReadScripts,
        Capability::ReadAssets,
        Capability::ViewLogs,
        Capability::ManageStreams,
        Capability::ReadScriptData,
        Capability::ReadStorage,
        Capability::ReadSecrets,
        Capability::UseNetwork,
    ]
}

/// The context a run under `scopes` acts with.
///
/// Two narrowings, in order. The tier is capped at
/// [`UserContext::authenticated`] however much the person holds, because
/// background work has no business authoring a solution and a delegated task
/// must not be the way to get a context that can. Then the capabilities are
/// attenuated to what was actually consented to, which is what makes a
/// read-only grant read-only rather than merely described as one.
///
/// Attenuating rather than intersecting by hand matters: the cap already
/// happened, so this can only take more away.
pub fn context_for(user_id: &str, scopes: &[Scope]) -> UserContext {
    let mut keep = base_capabilities();
    for scope in scopes {
        keep.extend_from_slice(scope.capabilities());
    }
    UserContext::authenticated(user_id.to_string()).attenuated(keep)
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
        // The senders that could set this work going go with it. A binding
        // that outlived its grant would be a row saying somebody may trigger
        // a delegation that no longer exists — harmless today, since the
        // enqueue checks the grant, and exactly the kind of leftover that
        // silently comes back to life when the person authorises the script
        // again for some unrelated reason.
        let unlinked = sqlx::query(
            "DELETE FROM script_channel_identities WHERE user_id = $1 AND script_uri = $2",
        )
        .bind(user_id)
        .bind(script_uri)
        .execute(db.pool())
        .await
        .map(|done| done.rows_affected())
        .unwrap_or_default();
        debug!(
            user = %user_id,
            script = %script_uri,
            cancelled = cancelled.unwrap_or(0),
            unlinked = unlinked,
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

    // Every sender that could set any of it going, for the same reason: this
    // runs when an account's roles change, its realm narrows, or it is
    // deleted, and "everything that can act as you is gone" has to include
    // the ways to start it.
    let _ = sqlx::query("DELETE FROM script_channel_identities WHERE user_id = $1")
        .bind(user_id)
        .execute(db.pool())
        .await;

    let _ = crate::tasks::cancel_delegated(user_id, None).await;
    Ok(result.rows_affected())
}

// ============================================================================
// Which sender may set a person's delegated work going
// ============================================================================

/// The longest a channel slug or a sender's identity may be.
///
/// Both arrive from a script, which got them from a request body, so both are
/// attacker-shaped. Neither is ever interpreted — they are compared — so the
/// cap is about what is worth storing rather than about safety.
const MAX_CHANNEL_CHARS: usize = 64;
const MAX_IDENTITY_CHARS: usize = 256;

/// A sender that may trigger one person's delegated work on one script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelIdentity {
    pub user_id: String,
    pub script_uri: String,
    pub channel: String,
    pub identity: String,
    pub granted_at: DateTime<Utc>,
}

impl ChannelIdentity {
    fn from_row(row: &sqlx::postgres::PgRow) -> Self {
        Self {
            user_id: row.get("user_id"),
            script_uri: row.get("script_uri"),
            channel: row.get("channel"),
            identity: row.get("identity"),
            granted_at: row.get("granted_at"),
        }
    }
}

/// Why a binding could not be recorded or used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRefusal {
    /// The channel slug or the identity is empty, or longer than the cap.
    Unusable(&'static str),
    /// Somebody else has already bound this sender on this script.
    ///
    /// Refused rather than replaced: moving a binding from one account to
    /// another is a takeover, and the engine has no way to tell which of two
    /// claimants really owns a Telegram id. The cost is that a squatter can
    /// stop the rightful owner from binding — which buys the squatter
    /// nothing, since triggers then run as *them*, spending their budget and
    /// reading their storage — so it is a nuisance to be unpicked by an
    /// administrator rather than a way in.
    TakenByAnother,
}

impl std::fmt::Display for ChannelRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelRefusal::Unusable(why) => write!(f, "{}", why),
            ChannelRefusal::TakenByAnother => write!(
                f,
                "another account has already linked that sender to this script"
            ),
        }
    }
}

/// Normalise and bound the pair a caller gave.
///
/// The channel is lower-cased, because it is a slug the solution chooses and
/// `Telegram` and `telegram` are the same place. The identity is not, because
/// it belongs to the far end: a Slack user id is case-sensitive, and folding
/// it would make two senders look like one.
pub fn normalize_channel(
    channel: &str,
    identity: &str,
) -> Result<(String, String), ChannelRefusal> {
    let channel = channel.trim().to_lowercase();
    let identity = identity.trim().to_string();

    if channel.is_empty() {
        return Err(ChannelRefusal::Unusable("the channel cannot be empty"));
    }
    if identity.is_empty() {
        return Err(ChannelRefusal::Unusable("the sender cannot be empty"));
    }
    if channel.chars().count() > MAX_CHANNEL_CHARS {
        return Err(ChannelRefusal::Unusable("that channel name is too long"));
    }
    if identity.chars().count() > MAX_IDENTITY_CHARS {
        return Err(ChannelRefusal::Unusable("that sender id is too long"));
    }

    Ok((channel, identity))
}

/// Record that this sender may trigger this person's delegated work.
///
/// Answers `true` when something was written and `false` when this same
/// person had already bound it, so a person re-consenting is not an error.
pub async fn bind_channel(
    user_id: &str,
    script_uri: &str,
    channel: &str,
    identity: &str,
) -> Result<bool, ChannelRefusal> {
    let (channel, identity) = normalize_channel(channel, identity)?;

    let Some(db) = crate::database::get_global_database() else {
        return Err(ChannelRefusal::Unusable("the engine has no database"));
    };

    // `DO NOTHING` rather than `DO UPDATE`, so a row belonging to somebody
    // else survives and is reported below. An upsert here would be the
    // takeover this is meant to refuse.
    let inserted = sqlx::query(
        r#"
        INSERT INTO script_channel_identities (user_id, script_uri, channel, identity)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (script_uri, channel, identity) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(script_uri)
    .bind(&channel)
    .bind(&identity)
    .execute(db.pool())
    .await
    .map_err(|e| {
        warn!(script = %script_uri, error = %e, "Could not record a channel identity");
        ChannelRefusal::Unusable("the link could not be recorded")
    })?;

    if inserted.rows_affected() > 0 {
        debug!(user = %user_id, script = %script_uri, channel = %channel, "Channel identity linked");
        return Ok(true);
    }

    // Nothing was written, so the row exists. Whose is it?
    match resolve_channel(script_uri, &channel, &identity).await {
        Ok(Some(owner)) if owner == user_id => Ok(false),
        _ => Err(ChannelRefusal::TakenByAnother),
    }
}

/// Who this sender is, on this script.
///
/// The whole of what a script may ask. It cannot ask for a person by id,
/// which is the point: a script that trusted the wrong field in a request
/// body can then claim to be the wrong *sender*, and not to be a different
/// person — an unbound sender resolves to nobody and nothing runs.
pub async fn resolve_channel(
    script_uri: &str,
    channel: &str,
    identity: &str,
) -> Result<Option<String>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(None);
    };

    let row = sqlx::query(
        r#"
        SELECT user_id
        FROM script_channel_identities
        WHERE script_uri = $1 AND channel = $2 AND identity = $3
        "#,
    )
    .bind(script_uri)
    .bind(channel)
    .bind(identity)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|row| row.get("user_id")))
}

/// Every sender this person has linked, for the account page.
pub async fn list_channels_for_user(user_id: &str) -> Result<Vec<ChannelIdentity>, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(Vec::new());
    };

    let rows = sqlx::query(
        r#"
        SELECT user_id, script_uri, channel, identity, granted_at
        FROM script_channel_identities
        WHERE user_id = $1
        ORDER BY granted_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db.pool())
    .await?;

    Ok(rows.iter().map(ChannelIdentity::from_row).collect())
}

/// Unlink one sender. Scoped to `user_id` in the statement rather than by a
/// check before it, so another account's binding is not something this can
/// delete however it is called.
pub async fn unbind_channel(
    user_id: &str,
    script_uri: &str,
    channel: &str,
    identity: &str,
) -> Result<bool, sqlx::Error> {
    let Some(db) = crate::database::get_global_database() else {
        return Ok(false);
    };

    let result = sqlx::query(
        r#"
        DELETE FROM script_channel_identities
        WHERE user_id = $1 AND script_uri = $2 AND channel = $3 AND identity = $4
        "#,
    )
    .bind(user_id)
    .bind(script_uri)
    .bind(channel)
    .bind(identity)
    .execute(db.pool())
    .await?;

    Ok(result.rows_affected() > 0)
}

/// How long a link invitation lasts.
///
/// Short, because it is delivered into a chat the person is looking at and
/// used within the minute. Long enough to survive being asked to sign in
/// first, which is the slowest legitimate path through it.
pub const LINK_TOKEN_MINUTES: i64 = 15;

/// Marks a link invitation apart at a glance, as `rt_` marks a refresh token.
const LINK_TOKEN_PREFIX: &str = "lnk_";

fn generate_link_token() -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let bytes: [u8; 32] = rand::random();
    format!("{}{}", LINK_TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(bytes))
}

/// Only the hash is stored. Engine-generated entropy rather than a password,
/// so no salt and no work factor — the `oauth_refresh_tokens` reasoning.
fn hash_link_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Invite somebody to link this sender, and hand back the one URL that does
/// it.
///
/// This is the load-bearing part of the whole scheme, and the reason the
/// consent page cannot simply take `?channel=&identity=`. Such a URL is one
/// anybody can construct for anybody, which would make linking first come
/// first served on a guessable string — and the harm there is not squatting
/// but **interception**: bind somebody else's Telegram id to your own account
/// before they do, and every message they send that bot is processed as your
/// turn, with their text landing in your storage.
///
/// A script mints this in response to a message it actually received and
/// replies into that chat. So reaching the consent page for a sender means
/// being able to read that sender's messages, which is the only evidence of
/// ownership the engine can have and the one a query parameter cannot carry.
///
/// Minting replaces whatever was outstanding for the same sender. That bounds
/// the table at one live row per sender, and makes asking for a new link
/// invalidate the old one — which is the behaviour somebody re-requesting a
/// link expects anyway.
pub async fn invite_link(
    script_uri: &str,
    channel: &str,
    identity: &str,
) -> Result<String, ChannelRefusal> {
    let (channel, identity) = normalize_channel(channel, identity)?;

    let Some(db) = crate::database::get_global_database() else {
        return Err(ChannelRefusal::Unusable("the engine has no database"));
    };

    let token = generate_link_token();
    let expires_at = Utc::now() + Duration::minutes(LINK_TOKEN_MINUTES);

    let mut tx = db.pool().begin().await.map_err(|e| {
        warn!(script = %script_uri, error = %e, "Could not begin a link invitation");
        ChannelRefusal::Unusable("the link could not be created")
    })?;

    // Expired rows go with it. An unredeemed token is litter rather than a
    // liability, but litter nobody collects is still a growing table, and
    // this is the moment somebody is already paying for a write.
    let _ = sqlx::query("DELETE FROM script_channel_link_tokens WHERE expires_at <= NOW()")
        .execute(&mut *tx)
        .await;

    sqlx::query(
        r#"
        DELETE FROM script_channel_link_tokens
        WHERE script_uri = $1 AND channel = $2 AND identity = $3
        "#,
    )
    .bind(script_uri)
    .bind(&channel)
    .bind(&identity)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        warn!(script = %script_uri, error = %e, "Could not clear an earlier link invitation");
        ChannelRefusal::Unusable("the link could not be created")
    })?;

    sqlx::query(
        r#"
        INSERT INTO script_channel_link_tokens
            (token_hash, script_uri, channel, identity, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(hash_link_token(&token))
    .bind(script_uri)
    .bind(&channel)
    .bind(&identity)
    .bind(expires_at)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        warn!(script = %script_uri, error = %e, "Could not record a link invitation");
        ChannelRefusal::Unusable("the link could not be created")
    })?;

    tx.commit().await.map_err(|e| {
        warn!(script = %script_uri, error = %e, "Could not commit a link invitation");
        ChannelRefusal::Unusable("the link could not be created")
    })?;

    Ok(link_url(&token))
}

/// What an invitation names, without spending it.
///
/// The consent page reads it to show the person which sender they are about
/// to link. Reading must not spend: a page that consumed the token on display
/// would break the sign-in redirect, the back button and the reload, all of
/// which happen before anybody has agreed to anything.
pub async fn peek_invite(token: &str) -> Option<(String, String, String)> {
    let db = crate::database::get_global_database()?;

    let row = sqlx::query(
        r#"
        SELECT script_uri, channel, identity
        FROM script_channel_link_tokens
        WHERE token_hash = $1 AND expires_at > NOW()
        "#,
    )
    .bind(hash_link_token(token))
    .fetch_optional(db.pool())
    .await
    .ok()
    .flatten()?;

    Some((
        row.get("script_uri"),
        row.get("channel"),
        row.get("identity"),
    ))
}

/// Spend an invitation, answering what it named.
///
/// Single use, and deleted in the statement that reads it, so two browsers
/// racing on the same link cannot both bind — the second finds nothing.
pub async fn spend_invite(token: &str) -> Option<(String, String, String)> {
    let db = crate::database::get_global_database()?;

    let row = sqlx::query(
        r#"
        DELETE FROM script_channel_link_tokens
        WHERE token_hash = $1 AND expires_at > NOW()
        RETURNING script_uri, channel, identity
        "#,
    )
    .bind(hash_link_token(token))
    .fetch_optional(db.pool())
    .await
    .ok()
    .flatten()?;

    Some((
        row.get("script_uri"),
        row.get("channel"),
        row.get("identity"),
    ))
}

/// Where an invitation sends somebody.
///
/// The token is the whole of it: it names the script and the sender, so
/// there is nothing else to put in the URL and nothing in it a person could
/// usefully change.
pub fn link_url(token: &str) -> String {
    format!("/auth/delegate?link={}", urlencoding::encode(token))
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
/// Then [`context_for`] narrows that cap to what the grant actually says, so
/// a grant without [`Scope::Write`] holds no write capability at all.
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
        user_context: context_for(user_id, &grant.scopes),
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

    /// The verb, and the whole reason it exists: a grant that did not say
    /// "change things" produces a context that holds no write at all.
    #[test]
    fn a_grant_without_the_verb_can_read_and_cannot_write() {
        let context = context_for("u1", &[Scope::PersonalStorage, Scope::Secrets]);

        for readable in [
            Capability::ReadScriptData,
            Capability::ReadStorage,
            Capability::ReadSecrets,
            Capability::ReadScripts,
            Capability::ReadAssets,
            // The two that are here so a read-only run is neither blind nor
            // mute: it can still call out, and still tell the person what it
            // found.
            Capability::UseNetwork,
            Capability::ManageStreams,
        ] {
            assert!(
                context.has_capability(&readable),
                "a delegation has to be able to {:?}",
                readable
            );
        }

        for denied in [
            Capability::WriteScriptData,
            Capability::WriteStorage,
            Capability::EnqueueTasks,
            Capability::SendMessages,
        ] {
            assert!(
                !context.has_capability(&denied),
                "a grant that did not say 'write' must not hold {:?}",
                denied
            );
        }
    }

    /// And with the verb, it holds what a delegation held before the verb
    /// existed — with one deliberate exception. This is the assertion the
    /// migration rests on: adding `write` to a stored grant restores it to
    /// what its owner agreed to rather than to something new.
    ///
    /// The exception is `WriteSecrets`, and it changes no behaviour. The
    /// surface already refused credential management in a delegated
    /// execution outright, whatever was granted — the consent page offers to
    /// let an app *use* your keys, and rotating or deleting one while you are
    /// away is not using it. So the old context held a capability that
    /// nothing would honour, and now it does not hold it: the rule is stated
    /// in one more place rather than in a different way.
    #[test]
    fn a_grant_with_the_verb_holds_what_a_delegation_always_held() {
        let context = context_for("u1", &Scope::all());
        let before = UserContext::authenticated("u1".to_string());

        for capability in &before.capabilities {
            if *capability == Capability::WriteSecrets {
                assert!(
                    !context.has_capability(capability),
                    "the one capability a delegation never honoured should not be held"
                );
                continue;
            }
            assert!(
                context.has_capability(capability),
                "granting every scope must not be narrower than the old fixed tier: {:?}",
                capability
            );
        }
    }

    /// The cap comes first and attenuation second, so no combination of
    /// scopes reaches past what an ordinary request holds. A scope whose
    /// capability list grew to include something authoring would be
    /// intersected away rather than granted.
    #[test]
    fn no_combination_of_scopes_exceeds_an_ordinary_request() {
        let context = context_for("u1", &Scope::all());
        let ordinary = UserContext::authenticated("u1".to_string());

        for capability in &context.capabilities {
            assert!(
                ordinary.has_capability(capability),
                "a delegation reached past an ordinary request: {:?}",
                capability
            );
        }
        assert!(context.attenuated);
    }

    /// Ticking nothing is still a grant — the narrowest useful one, which is
    /// "act as me, read what you need, touch nothing of mine".
    #[test]
    fn a_grant_with_no_scopes_can_still_read() {
        let context = context_for("u1", &[]);

        assert!(context.has_capability(&Capability::ReadScriptData));
        assert!(!context.has_capability(&Capability::WriteScriptData));
        assert_eq!(context.user_id.as_deref(), Some("u1"));
    }

    #[test]
    fn a_channel_is_folded_and_a_sender_is_not() {
        let (channel, identity) = normalize_channel("  Telegram ", " U123aBc ").expect("usable");
        // The channel is a slug the solution chose, so `Telegram` and
        // `telegram` are the same place.
        assert_eq!(channel, "telegram");
        // The identity belongs to the far end. A Slack id is case-sensitive,
        // and folding it would make two senders look like one.
        assert_eq!(identity, "U123aBc");
    }

    #[test]
    fn an_empty_or_oversized_sender_is_refused() {
        assert!(matches!(
            normalize_channel("", "1"),
            Err(ChannelRefusal::Unusable(_))
        ));
        assert!(matches!(
            normalize_channel("telegram", "   "),
            Err(ChannelRefusal::Unusable(_))
        ));
        assert!(matches!(
            normalize_channel(&"c".repeat(MAX_CHANNEL_CHARS + 1), "1"),
            Err(ChannelRefusal::Unusable(_))
        ));
        assert!(matches!(
            normalize_channel("telegram", &"9".repeat(MAX_IDENTITY_CHARS + 1)),
            Err(ChannelRefusal::Unusable(_))
        ));
    }

    /// The URL carries the token and nothing else. Naming the sender in it
    /// is precisely what would make linking somebody else's chat possible,
    /// so this asserts the absence rather than the encoding.
    #[test]
    fn a_link_url_carries_only_its_token() {
        let url = link_url("lnk_abc-123");
        assert_eq!(url, "/auth/delegate?link=lnk_abc-123");
    }

    /// And a token that needs escaping is escaped, since it reaches the
    /// page as a query parameter like any other.
    #[test]
    fn a_link_url_encodes_its_token() {
        assert!(link_url("a+b/c=").contains("link=a%2Bb%2Fc%3D"));
    }

    /// Two invitations are two secrets. Minting is what replaces one, not
    /// the token happening to repeat.
    #[test]
    fn each_invitation_is_its_own_secret() {
        let first = generate_link_token();
        let second = generate_link_token();
        assert_ne!(first, second);
        assert!(first.starts_with(LINK_TOKEN_PREFIX));
        // Stored hashed, so a copy of the table is not a set of usable links.
        assert_ne!(hash_link_token(&first), first);
        assert_eq!(hash_link_token(&first), hash_link_token(&first));
        assert_ne!(hash_link_token(&first), hash_link_token(&second));
    }

    /// Changing a credential is refused outright in a delegated execution,
    /// so no scope may hand it over by listing the capability.
    #[test]
    fn no_scope_grants_writing_a_credential() {
        for scope in Scope::all() {
            assert!(
                !scope.capabilities().contains(&Capability::WriteSecrets),
                "{:?} must not offer what the surface refuses outright",
                scope
            );
        }
        assert!(!context_for("u1", &Scope::all()).has_capability(&Capability::WriteSecrets));
    }
}
