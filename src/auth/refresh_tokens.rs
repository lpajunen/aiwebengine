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
//! and nothing distinguishes "the client retried" from "someone else has a
//! copy" — so the whole family is revoked and the client has to go back through
//! an authorization.

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
/// to and it has not been spent already.
///
/// The read and the write are one transaction with the row locked, so two
/// requests racing with the same token cannot both come away with a grant —
/// the second sees `consumed_at` set and is treated as a replay.
pub async fn redeem(
    pool: &PgPool,
    presented: &str,
    client_id: &str,
) -> Result<RefreshGrant, RefreshTokenError> {
    let mut tx = pool.begin().await?;

    let row = sqlx::query(
        "SELECT family_id, user_id, client_id, audience, scope, expires_at, consumed_at \
         FROM oauth_refresh_tokens WHERE token_hash = $1 FOR UPDATE",
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

    // A spent token coming back is the one thing that cannot be explained
    // innocently: the successor was handed out, so whoever is presenting this
    // is either replaying or is not the client that received the successor.
    if consumed_at.is_some() {
        let _ = tx.rollback().await;
        let revoked = revoke_family(pool, &family_id).await.unwrap_or(0);
        tracing::warn!(
            "Refresh token replayed; revoked {} token(s) in family {}",
            revoked,
            family_id
        );
        return Err(RefreshTokenError::Reused);
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

    sqlx::query("UPDATE oauth_refresh_tokens SET consumed_at = $1 WHERE token_hash = $2")
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

    #[test]
    fn only_the_hash_is_ever_stored() {
        let token = generate_token();
        let hash = hash_token(&token);

        assert_ne!(hash, token);
        assert_eq!(hash.len(), 64, "SHA-256, hex");
        assert_eq!(hash, hash_token(&token), "and it has to be stable");
    }
}
