//! Refresh tokens for this engine's OAuth2 authorization server.
//!
//! A refresh token used to be the session token itself — the token endpoint
//! returned the same string in `access_token` and `refresh_token`. That made
//! rotation impossible and made a leaked refresh token a leaked access token,
//! carrying the same audience and the same roles for as long as the session
//! lived.
//!
//! What lives here is a separate credential. It authenticates nothing on its
//! own: it is presented only at the token endpoint, only by the client it was
//! issued to, and only to mint a *fresh* session — which is also why refreshing
//! re-reads roles and realm from the repository rather than copying them from
//! whatever the previous session was carrying.
//!
//! Single use. Redeeming a token spends it and issues its successor in the same
//! family. Presenting one that was already spent is the signature of a replay,
//! and the whole family is revoked — the client has to go back through an
//! authorization.
//!
//! With one exception, and it is the one that made this unusable for an agent
//! nobody is watching. "The client retried" and "someone else has a copy" are
//! genuinely indistinguishable *in general*, but not in the first seconds after
//! a redemption: a client that crashed between the server spending its token
//! and the client storing the successor has no way back, and two jobs sharing
//! one stored token race every time they start together. Both present the same
//! token moments after it was spent, and both used to kill the family — so the
//! cost of the rule fell entirely on unattended clients, which are the ones
//! that cannot answer an authorization prompt.
//!
//! So a spent token is forgiven for [`REPLAY_GRACE_SECS`] seconds, and only
//! while the chain has not moved on: if any *later* token in the family has
//! itself been spent, the presenter is behind a chain somebody else is
//! advancing, which is the theft signal rather than a retry, and the family
//! goes. The window is anchored to the first redemption and never slid forward
//! by a replay, so repeating one cannot extend it.

use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row};

/// What a refresh token, once redeemed, says a new session should carry.
#[derive(Debug, Clone)]
pub struct RefreshGrant {
    /// The rotation chain the redeemed token belonged to; the successor is
    /// issued into the same one.
    pub family_id: String,
    pub user_id: String,
    pub client_id: String,
    /// The audience the original authorization was for. A refresh cannot
    /// widen it — it is copied, never re-derived from the request.
    pub audience: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RefreshTokenError {
    #[error("Unknown refresh token")]
    Unknown,

    #[error("Refresh token has expired")]
    Expired,

    /// The token was already spent. Either the client replayed it or someone
    /// else holds a copy; the family is revoked either way.
    #[error("Refresh token has already been used")]
    Reused,

    #[error("Refresh token was issued to another client")]
    WrongClient,

    #[error("Refresh token storage error: {0}")]
    Storage(String),
}

impl From<sqlx::Error> for RefreshTokenError {
    fn from(error: sqlx::Error) -> Self {
        RefreshTokenError::Storage(error.to_string())
    }
}

/// Marks a refresh token apart from an access token at a glance, the way
/// `code_` marks an authorization code.
const TOKEN_PREFIX: &str = "rt_";

/// How long a just-spent token goes on being accepted as a retry.
///
/// Short enough that a stolen token is nearly always past it — an attacker has
/// to replay within seconds *and* before the legitimate client advances the
/// chain — and long enough to cover a lost response, a process restart, or two
/// jobs that started together. Not configurable: an operator tuning this is
/// trading away the replay detection this table exists for, and the failure it
/// addresses does not vary between deployments.
const REPLAY_GRACE_SECS: i64 = 30;

fn generate_token() -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let bytes: [u8; 32] = rand::random();
    format!("{}{}", TOKEN_PREFIX, URL_SAFE_NO_PAD.encode(bytes))
}

/// Only the hash is stored, so a copy of the table is not a set of usable
/// credentials. No salt and no work factor on purpose: this is a 256-bit random
/// string, not a password, so there is nothing to guess and nothing to
/// pre-compute.
fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Issue a refresh token, returning the one and only copy of it.
///
/// `family_id` is `None` for a token minted from an authorization code, which
/// starts a new chain, and `Some` for the successor of a redeemed one.
#[allow(clippy::too_many_arguments)]
pub async fn issue(
    pool: &PgPool,
    user_id: &str,
    client_id: &str,
    audience: Option<&str>,
    scope: Option<&str>,
    family_id: Option<&str>,
    lifetime: Duration,
) -> Result<String, RefreshTokenError> {
    let token = generate_token();
    let family_id = family_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("rtf_{}", uuid::Uuid::new_v4()));

    sqlx::query(
        "INSERT INTO oauth_refresh_tokens \
         (token_hash, family_id, user_id, client_id, audience, scope, issued_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(hash_token(&token))
    .bind(&family_id)
    .bind(user_id)
    .bind(client_id)
    .bind(audience)
    .bind(scope)
    .bind(Utc::now())
    .bind(Utc::now() + lifetime)
    .execute(pool)
    .await?;

    Ok(token)
}

/// Spend a refresh token, if the client presenting it is the one it was issued
/// to and it has not been spent already — or was spent so recently, and with
/// the chain so unmoved, that a retry is the better explanation.
///
/// The read and the write are one transaction with the row locked, so two
/// requests racing with the same token are serialised rather than both reading
/// it unspent. The second one then meets the grace window described on the
/// module, which is what lets two jobs that started together both come away
/// with a working token instead of revoking each other.
pub async fn redeem(
    pool: &PgPool,
    presented: &str,
    client_id: &str,
) -> Result<RefreshGrant, RefreshTokenError> {
    let mut tx = pool.begin().await?;

    let row = sqlx::query(
        "SELECT family_id, user_id, client_id, audience, scope, issued_at, expires_at, \
         consumed_at FROM oauth_refresh_tokens WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(hash_token(presented))
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        let _ = tx.rollback().await;
        return Err(RefreshTokenError::Unknown);
    };

    let family_id: String = row.try_get("family_id")?;
    let consumed_at: Option<DateTime<Utc>> = row.try_get("consumed_at")?;
    let issued_at: DateTime<Utc> = row.try_get("issued_at")?;

    // A spent token coming back is either a client retrying something it could
    // not confirm, or somebody else holding a copy. Two things separate them,
    // and both have to hold before the retry reading is taken.
    if let Some(consumed_at) = consumed_at {
        // How long ago it was spent — measured from the first redemption,
        // which is why the write below never overwrites `consumed_at`.
        let within_window = Utc::now() - consumed_at <= Duration::seconds(REPLAY_GRACE_SECS);

        // And whether the chain has moved past it. A later token in this
        // family that has itself been spent means somebody is advancing the
        // rotation while this presenter is still holding an earlier link:
        // that is not a retry however recent it is.
        let superseded: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM oauth_refresh_tokens \
             WHERE family_id = $1 AND consumed_at IS NOT NULL AND issued_at > $2)",
        )
        .bind(&family_id)
        .bind(issued_at)
        .fetch_one(&mut *tx)
        .await?;

        if !within_window || superseded {
            let _ = tx.rollback().await;
            let revoked = revoke_family(pool, &family_id).await.unwrap_or(0);
            tracing::warn!(
                "Refresh token replayed ({}); revoked {} token(s) in family {}",
                if superseded {
                    "the chain had already moved on"
                } else {
                    "outside the retry window"
                },
                revoked,
                family_id
            );
            return Err(RefreshTokenError::Reused);
        }

        tracing::info!(
            "Refresh token presented again {}s after it was spent and the chain had not moved; \
             treating it as a retry in family {}",
            (Utc::now() - consumed_at).num_seconds(),
            family_id
        );
    }

    let issued_to: String = row.try_get("client_id")?;
    if issued_to != client_id {
        let _ = tx.rollback().await;
        return Err(RefreshTokenError::WrongClient);
    }

    let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
    if expires_at <= Utc::now() {
        let _ = tx.rollback().await;
        return Err(RefreshTokenError::Expired);
    }

    // `COALESCE` rather than an assignment: a forgiven retry must not slide the
    // window forward, or replaying a token every twenty seconds would keep it
    // alive indefinitely. The anchor stays at the first redemption.
    sqlx::query(
        "UPDATE oauth_refresh_tokens SET consumed_at = COALESCE(consumed_at, $1) \
         WHERE token_hash = $2",
    )
    .bind(Utc::now())
    .bind(hash_token(presented))
    .execute(&mut *tx)
    .await?;

    let grant = RefreshGrant {
        family_id,
        user_id: row.try_get("user_id")?,
        client_id: issued_to,
        audience: row.try_get("audience")?,
        scope: row.try_get("scope")?,
    };

    tx.commit().await?;
    Ok(grant)
}

/// Drop a whole rotation chain. Called when a spent token is presented again.
pub async fn revoke_family(pool: &PgPool, family_id: &str) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM oauth_refresh_tokens WHERE family_id = $1")
        .bind(family_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

/// Drop the refresh tokens a user holds for one audience.
///
/// What makes revoking a single API session mean anything. A session carrying
/// an audience was minted at the token endpoint, and the client that got it
/// usually holds a refresh token too — so ending the session alone buys the
/// length of one access token, after which the client mints another and the
/// person who pressed the button is still signed in where they wanted not to
/// be.
///
/// Scoped to the audience rather than the user, because the two are different
/// statements: ending one API session should not take down a client acting for
/// a different resource.
pub async fn revoke_for_user_audience(
    pool: &PgPool,
    user_id: &str,
    audience: &str,
) -> Result<u64, sqlx::Error> {
    let result =
        sqlx::query("DELETE FROM oauth_refresh_tokens WHERE user_id = $1 AND audience = $2")
            .bind(user_id)
            .bind(audience)
            .execute(pool)
            .await?;

    Ok(result.rows_affected())
}

/// Drop every refresh token belonging to a user.
///
/// Ending someone's sessions would otherwise revoke nothing: a client holding a
/// refresh token would mint a new session on the next call, and the roles or
/// realm that were just taken away would come straight back. So this runs
/// beside [`crate::security::delete_sessions_for_user`], not instead of it.
pub async fn revoke_for_user(pool: &PgPool, user_id: &str) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM oauth_refresh_tokens WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_random_and_marked() {
        let first = generate_token();
        let second = generate_token();

        assert!(first.starts_with(TOKEN_PREFIX));
        assert_ne!(first, second, "two tokens must never be the same string");
        assert!(
            first.len() > TOKEN_PREFIX.len() + 32,
            "and there must be enough of it to be unguessable"
        );
    }

    /// These exercise the real table, because the whole of the retry rule is
    /// in the SQL: the `COALESCE` that anchors the window and the `EXISTS`
    /// that asks whether the chain has moved on.
    fn test_pool() -> PgPool {
        crate::test_db::pool()
    }

    /// A fresh chain, and the handle to advance it with.
    async fn a_chain(pool: &PgPool) -> (String, String) {
        let user = format!("u_{}", uuid::Uuid::new_v4());
        let client = format!("c_{}", uuid::Uuid::new_v4());
        let token = issue(
            pool,
            &user,
            &client,
            Some("https://example.test/mcp"),
            None,
            None,
            Duration::days(30),
        )
        .await
        .expect("a chain has to start somewhere");
        (token, client)
    }

    /// Backdate a token's redemption so a test does not have to wait out the
    /// window in real time.
    async fn spent_seconds_ago(pool: &PgPool, token: &str, seconds: i64) {
        sqlx::query("UPDATE oauth_refresh_tokens SET consumed_at = $1 WHERE token_hash = $2")
            .bind(Utc::now() - Duration::seconds(seconds))
            .bind(hash_token(token))
            .execute(pool)
            .await
            .expect("backdating is a plain update");
    }

    #[tokio::test]
    async fn a_retry_moments_after_a_redemption_is_forgiven() {
        let pool = test_pool();
        let (first, client) = a_chain(&pool).await;

        let grant = redeem(&pool, &first, &client)
            .await
            .expect("the first redemption is ordinary");
        let successor = issue(
            &pool,
            &grant.user_id,
            &client,
            grant.audience.as_deref(),
            None,
            Some(&grant.family_id),
            Duration::days(30),
        )
        .await
        .expect("which hands out a successor the client never receives");

        // The client crashed before storing `successor` and comes back with
        // the only token it has. This used to revoke the family.
        let retried = redeem(&pool, &first, &client)
            .await
            .expect("a retry inside the window is not a replay");
        assert_eq!(
            retried.family_id, grant.family_id,
            "and it stays in the same chain"
        );
        assert_eq!(retried.user_id, grant.user_id);
        assert_eq!(
            retried.audience, grant.audience,
            "a refresh copies the audience rather than re-deriving it"
        );

        // The family survived, so the orphaned successor is still live.
        assert!(
            redeem(&pool, &successor, &client).await.is_ok(),
            "forgiving the retry must not have revoked anything"
        );
    }

    #[tokio::test]
    async fn a_replay_after_the_window_still_kills_the_family() {
        let pool = test_pool();
        let (first, client) = a_chain(&pool).await;

        let grant = redeem(&pool, &first, &client).await.expect("ordinary");
        let successor = issue(
            &pool,
            &grant.user_id,
            &client,
            None,
            None,
            Some(&grant.family_id),
            Duration::days(30),
        )
        .await
        .expect("successor issued");

        spent_seconds_ago(&pool, &first, REPLAY_GRACE_SECS + 5).await;

        assert!(
            matches!(
                redeem(&pool, &first, &client).await,
                Err(RefreshTokenError::Reused)
            ),
            "past the window there is no innocent reading left"
        );
        assert!(
            matches!(
                redeem(&pool, &successor, &client).await,
                Err(RefreshTokenError::Unknown)
            ),
            "and the whole family goes with it"
        );
    }

    #[tokio::test]
    async fn a_replay_is_refused_once_the_chain_has_moved_on() {
        let pool = test_pool();
        let (first, client) = a_chain(&pool).await;

        let grant = redeem(&pool, &first, &client).await.expect("ordinary");
        let second = issue(
            &pool,
            &grant.user_id,
            &client,
            None,
            None,
            Some(&grant.family_id),
            Duration::days(30),
        )
        .await
        .expect("successor issued");

        // The legitimate client has the successor and uses it. Everything here
        // is well inside the window, so recency alone would forgive the replay.
        redeem(&pool, &second, &client)
            .await
            .expect("the real client advances the chain");

        assert!(
            matches!(
                redeem(&pool, &first, &client).await,
                Err(RefreshTokenError::Reused)
            ),
            "somebody holding an earlier link while the chain advances is the theft signal"
        );
    }

    #[tokio::test]
    async fn replaying_cannot_slide_the_window_forward() {
        let pool = test_pool();
        let (first, client) = a_chain(&pool).await;

        redeem(&pool, &first, &client).await.expect("ordinary");

        // Put the redemption near the far edge of the window, then retry.
        let anchor = Utc::now() - Duration::seconds(REPLAY_GRACE_SECS - 2);
        sqlx::query("UPDATE oauth_refresh_tokens SET consumed_at = $1 WHERE token_hash = $2")
            .bind(anchor)
            .bind(hash_token(&first))
            .execute(&pool)
            .await
            .expect("backdating is a plain update");

        redeem(&pool, &first, &client)
            .await
            .expect("still inside the window");

        // The retry must not have re-stamped `consumed_at`. Asserting on the
        // stored timestamp rather than on a later refusal is deliberate: a
        // test that backdates the row again after the retry overwrites the one
        // difference it is trying to observe, and passes whether the write is
        // an assignment or a `COALESCE`.
        let stored: DateTime<Utc> = sqlx::query_scalar(
            "SELECT consumed_at FROM oauth_refresh_tokens WHERE token_hash = $1",
        )
        .bind(hash_token(&first))
        .fetch_one(&pool)
        .await
        .expect("the row is still there");

        assert!(
            (stored - anchor).num_milliseconds().abs() < 500,
            "the anchor is the first redemption, not the most recent retry — \
             stored {stored}, expected about {anchor}"
        );

        // Which is what makes the window actually close: one more second of
        // real time past the original redemption and the retry reading is gone.
        spent_seconds_ago(&pool, &first, REPLAY_GRACE_SECS + 1).await;
        assert!(
            matches!(
                redeem(&pool, &first, &client).await,
                Err(RefreshTokenError::Reused)
            ),
            "past the window a replay is a replay again"
        );
    }

    #[tokio::test]
    async fn two_jobs_starting_together_both_come_away_with_a_token() {
        let pool = test_pool();
        let (shared, client) = a_chain(&pool).await;

        // Serialised by the row lock rather than genuinely simultaneous, which
        // is the state the second one actually meets: consumed a moment ago,
        // nothing later spent.
        let first = redeem(&pool, &shared, &client).await;
        let second = redeem(&pool, &shared, &client).await;

        assert!(first.is_ok(), "the job that won the lock refreshes");
        assert!(
            second.is_ok(),
            "and the one that lost it retries rather than revoking them both"
        );
    }

    #[tokio::test]
    async fn another_client_presenting_the_token_is_still_refused() {
        let pool = test_pool();
        let (token, client) = a_chain(&pool).await;

        assert!(
            matches!(
                redeem(&pool, &token, "some-other-client").await,
                Err(RefreshTokenError::WrongClient)
            ),
            "the grace window is about when, not about who"
        );
        assert!(
            redeem(&pool, &token, &client).await.is_ok(),
            "and a wrong-client refusal spends nothing"
        );
    }

    #[test]
    fn only_the_hash_is_ever_stored() {
        let token = generate_token();
        let hash = hash_token(&token);

        assert_ne!(hash, token);
        assert_eq!(hash.len(), 64, "SHA-256, hex");
        assert_eq!(hash, hash_token(&token), "and it has to be stable");
    }
}
