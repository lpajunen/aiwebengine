//! Where a person's git token lives, and why it lives there.
//!
//! `user_secrets` looks like the obvious home and is the wrong one. It is keyed
//! `(script_uri, user_id, key)` and it *is* the JavaScript `secretStorage`
//! ([`crate::security::secure_globals`]) — a token stored there is readable by
//! the script it is keyed to. It is also script-scoped, and a personal access
//! token is not a script's credential; it is the person's, and it is
//! engine-wide.
//!
//! So this is a table of its own, unreachable from the sandbox, read in exactly
//! one place: the code that talks to the git host. No endpoint returns a token,
//! and the listing answers with metadata alone — the same line the engine
//! already draws for administration, which lives on `/engine/*` and not in
//! JavaScript.
//!
//! Encryption is the engine's existing at-rest encryption
//! ([`crate::security::encryption`], AES-GCM under
//! `security.secret_encryption_key`), stored as the serialized `EncryptedData`
//! every other encrypted column holds. An engine with no key configured refuses
//! to store a token rather than writing one in the clear: a secret nobody
//! declared a key for is one the operator did not agree to keep.

use chrono::{DateTime, Utc};
use sqlx::Row;
use thiserror::Error;

use crate::error::{AppError, AppResult};

/// What a listing may say about a stored credential.
///
/// Everything here is deliberately not the token: which host it authenticates
/// against, which account it belongs to, and when it was last useful. Enough to
/// tell two credentials apart and to decide whether one is still wanted.
#[derive(Debug, Clone)]
pub struct CredentialSummary {
    pub remote_host: String,
    /// The account the host reported when the token was checked, when it
    /// reported one.
    pub account: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("{0}")]
    NotConfigured(String),

    #[error("{0}")]
    Storage(String),
}

fn pool() -> AppResult<sqlx::PgPool> {
    crate::repository::get_db_pool()
        .map(|db| db.pool().clone())
        .ok_or_else(|| AppError::Database {
            message: "No database configured".to_string(),
            source: None,
        })
}

fn storage(context: &str, e: sqlx::Error) -> CredentialError {
    tracing::error!("Database error {}: {}", context, e);
    CredentialError::Storage(format!("Database error {}", context))
}

/// Encrypt a token for storage.
///
/// The absence of a key is refused rather than tolerated. Elsewhere in the
/// engine an unset `secret_encryption_key` means a secret is stored in the
/// clear, which is a defensible default for values a script put there itself;
/// it is not defensible for a credential that reaches outside the engine as a
/// named person.
fn seal(token: &str) -> Result<String, CredentialError> {
    let encryption = crate::repository::secret_encryption().ok_or_else(|| {
        CredentialError::NotConfigured(
            "This engine has no security.secret_encryption_key, so it will not store a git \
             token. Configure one and restart."
                .to_string(),
        )
    })?;

    let encrypted = encryption
        .encrypt_field(token)
        .map_err(|e| CredentialError::Storage(format!("Could not encrypt the token: {}", e)))?;

    serde_json::to_string(&encrypted)
        .map_err(|e| CredentialError::Storage(format!("Could not store the token: {}", e)))
}

fn unseal(stored: &str) -> Option<String> {
    let encryption = crate::repository::secret_encryption()?;
    let encrypted =
        serde_json::from_str::<crate::security::encryption::EncryptedData>(stored).ok()?;
    encryption.decrypt_field(&encrypted).ok()
}

/// Store or replace `user`'s credential for `remote_host`.
pub async fn store(
    user_id: &str,
    remote_host: &str,
    token: &str,
    account: Option<&str>,
) -> Result<(), CredentialError> {
    let sealed = seal(token)?;
    let pool = pool().map_err(|e| CredentialError::Storage(e.to_string()))?;

    sqlx::query(
        r#"
        INSERT INTO user_git_credentials
            (user_id, remote_host, token, account, created_at, updated_at)
        VALUES ($1, $2, $3, $4, NOW(), NOW())
        ON CONFLICT (user_id, remote_host) DO UPDATE SET
            token = EXCLUDED.token,
            account = EXCLUDED.account,
            updated_at = NOW(),
            -- Deliberately not reset: replacing a token does not make the
            -- credential newly unused, and "when did this last work" is the
            -- question the column exists to answer.
            last_used_at = user_git_credentials.last_used_at
        "#,
    )
    .bind(user_id)
    .bind(remote_host)
    .bind(sealed)
    .bind(account)
    .execute(&pool)
    .await
    .map_err(|e| storage("storing a git credential", e))?;

    Ok(())
}

/// The token `user` holds for `remote_host`, decrypted.
///
/// The only function that yields a token, and its one caller is the code that
/// puts it in an `Authorization` header. Nothing routes its result back into a
/// response.
pub async fn token_for(user_id: &str, remote_host: &str) -> Option<String> {
    let pool = pool().ok()?;
    let row = sqlx::query(
        "SELECT token FROM user_git_credentials WHERE user_id = $1 AND remote_host = $2",
    )
    .bind(user_id)
    .bind(remote_host)
    .fetch_optional(&pool)
    .await
    .ok()
    .flatten()?;

    unseal(&row.get::<String, _>(0))
}

/// Note that a credential was just used, for the listing.
///
/// Failure is swallowed: this is bookkeeping about a pull that already
/// succeeded, and failing the pull because a timestamp would not write would be
/// the wrong trade.
pub async fn mark_used(user_id: &str, remote_host: &str) {
    let Ok(pool) = pool() else {
        return;
    };
    let _ = sqlx::query(
        "UPDATE user_git_credentials SET last_used_at = NOW() \
         WHERE user_id = $1 AND remote_host = $2",
    )
    .bind(user_id)
    .bind(remote_host)
    .execute(&pool)
    .await;
}

/// What `user` has stored, without any of it.
pub async fn list(user_id: &str) -> Result<Vec<CredentialSummary>, CredentialError> {
    let pool = pool().map_err(|e| CredentialError::Storage(e.to_string()))?;
    let rows = sqlx::query(
        "SELECT remote_host, account, created_at, updated_at, last_used_at \
         FROM user_git_credentials WHERE user_id = $1 ORDER BY remote_host",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .map_err(|e| storage("listing git credentials", e))?;

    Ok(rows
        .into_iter()
        .map(|row| CredentialSummary {
            remote_host: row.get::<String, _>(0),
            account: row.get::<Option<String>, _>(1),
            created_at: row.get::<DateTime<Utc>, _>(2),
            updated_at: row.get::<DateTime<Utc>, _>(3),
            last_used_at: row.get::<Option<DateTime<Utc>>, _>(4),
        })
        .collect())
}

/// Remove `user`'s credential for `remote_host`. Returns whether there was one.
pub async fn forget(user_id: &str, remote_host: &str) -> Result<bool, CredentialError> {
    let pool = pool().map_err(|e| CredentialError::Storage(e.to_string()))?;
    let result =
        sqlx::query("DELETE FROM user_git_credentials WHERE user_id = $1 AND remote_host = $2")
            .bind(user_id)
            .bind(remote_host)
            .execute(&pool)
            .await
            .map_err(|e| storage("removing a git credential", e))?;

    Ok(result.rows_affected() > 0)
}

/// Drop every credential a user holds.
///
/// Called when the account goes, because a token outliving the person it
/// belonged to is a credential nobody is watching.
pub async fn forget_all(user_id: &str) -> Result<u64, CredentialError> {
    let pool = pool().map_err(|e| CredentialError::Storage(e.to_string()))?;
    let result = sqlx::query("DELETE FROM user_git_credentials WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .map_err(|e| storage("removing a user's git credentials", e))?;
    Ok(result.rows_affected())
}
