//! Script and personal secrets, encrypted at rest.

use super::*;
use crate::error::{AppError, AppResult};
use sqlx::Row;
use std::sync::{Arc, OnceLock};
use tracing::{debug, error};

/// Database-backed set script secret item
pub(super) async fn db_set_script_secret(
    mut executor: crate::database::TransactionExecutor<'_>,
    script_uri: &str,
    key: &str,
    value: &str,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Encrypt the value if secret encryption is configured
    let stored_value: String = if let Some(enc) = GLOBAL_SECRET_ENCRYPTION.get() {
        let encrypted = enc.encrypt_field(value).map_err(|e| AppError::Internal {
            message: format!("Failed to encrypt secret value: {}", e),
        })?;
        serde_json::to_string(&encrypted).map_err(|e| AppError::Internal {
            message: format!("Failed to serialize encrypted secret: {}", e),
        })?
    } else {
        value.to_string()
    };
    let value: &str = &stored_value;

    // Try to update existing item
    let update_result = match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                UPDATE script_secrets
                SET value = $1, updated_at = $2
                WHERE script_uri = $3 AND key = $4
                "#,
            )
            .bind(value)
            .bind(now)
            .bind(script_uri)
            .bind(key)
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(
                r#"
                UPDATE script_secrets
                SET value = $1, updated_at = $2
                WHERE script_uri = $3 AND key = $4
                "#,
            )
            .bind(value)
            .bind(now)
            .bind(script_uri)
            .bind(key)
            .execute(pool)
            .await
        }
    }
    .map_err(|e| {
        error!("Database error updating script secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if update_result.rows_affected() > 0 {
        debug!("Updated script secret in database: {}:{}", script_uri, key);
        return Ok(());
    }

    // Item doesn't exist, create new one
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                INSERT INTO script_secrets (script_uri, key, value, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $4)
                "#,
            )
            .bind(script_uri)
            .bind(key)
            .bind(value)
            .bind(now)
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(
                r#"
                INSERT INTO script_secrets (script_uri, key, value, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $4)
                "#,
            )
            .bind(script_uri)
            .bind(key)
            .bind(value)
            .bind(now)
            .execute(pool)
            .await
        }
    }
    .map_err(|e| {
        error!("Database error creating script secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Created new script secret in database: {}:{}",
        script_uri, key
    );
    Ok(())
}

/// Database-backed get script secret
pub(super) async fn db_get_script_secret<'e, E>(
    executor: E,
    script_uri: &str,
    key: &str,
) -> AppResult<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT value FROM script_secrets WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        error!("Database error getting script secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = row {
        let raw: String = row.try_get("value").map_err(|e| {
            error!("Database error getting value: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        // Decrypt if encryption is configured and the value is in encrypted form
        let value = if let Some(enc) = GLOBAL_SECRET_ENCRYPTION.get() {
            match serde_json::from_str::<crate::security::encryption::EncryptedData>(&raw) {
                Ok(encrypted) => match enc.decrypt_field(&encrypted) {
                    Ok(plain) => plain,
                    Err(e) => {
                        error!(
                            "Failed to decrypt script secret {}:{}: {}",
                            script_uri, key, e
                        );
                        raw
                    }
                },
                Err(_) => raw, // stored before encryption was enabled — return as-is
            }
        } else {
            raw
        };
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

/// Database-backed remove script secret
pub(super) async fn db_remove_script_secret<'e, E>(
    executor: E,
    script_uri: &str,
    key: &str,
) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM script_secrets WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error removing script secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!(
            "Removed script secret from database: {}:{}",
            script_uri, key
        );
    } else {
        debug!(
            "Script secret not found in database for removal: {}:{}",
            script_uri, key
        );
    }

    Ok(existed)
}

/// Database-backed clear all script secrets for a script
pub(super) async fn db_clear_script_secrets<'e, E>(executor: E, script_uri: &str) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        r#"
        DELETE FROM script_secrets WHERE script_uri = $1
        "#,
    )
    .bind(script_uri)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error clearing script secrets: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Cleared all script secrets from database for script: {}",
        script_uri
    );
    Ok(())
}

pub(super) async fn db_list_script_secrets<'e, E>(
    executor: E,
    script_uri: &str,
) -> AppResult<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query_scalar::<_, String>(
        r#"
        SELECT key FROM script_secrets WHERE script_uri = $1 ORDER BY key ASC
        "#,
    )
    .bind(script_uri)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error listing script secret keys: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Listed {} script secret keys from database for script: {}",
        rows.len(),
        script_uri
    );
    Ok(rows)
}

/// Database-backed set user secret item
pub(super) async fn db_set_user_secret(
    mut executor: crate::database::TransactionExecutor<'_>,
    script_uri: &str,
    user_id: &str,
    key: &str,
    value: &str,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Encrypt the value if secret encryption is configured
    let stored_value: String = if let Some(enc) = GLOBAL_SECRET_ENCRYPTION.get() {
        let encrypted = enc.encrypt_field(value).map_err(|e| AppError::Internal {
            message: format!("Failed to encrypt user secret value: {}", e),
        })?;
        serde_json::to_string(&encrypted).map_err(|e| AppError::Internal {
            message: format!("Failed to serialize encrypted user secret: {}", e),
        })?
    } else {
        value.to_string()
    };
    let value: &str = &stored_value;

    // Try to update existing item
    let update_result = match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                UPDATE user_secrets
                SET value = $1, updated_at = $2
                WHERE script_uri = $3 AND user_id = $4 AND key = $5
                "#,
            )
            .bind(value)
            .bind(now)
            .bind(script_uri)
            .bind(user_id)
            .bind(key)
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(
                r#"
                UPDATE user_secrets
                SET value = $1, updated_at = $2
                WHERE script_uri = $3 AND user_id = $4 AND key = $5
                "#,
            )
            .bind(value)
            .bind(now)
            .bind(script_uri)
            .bind(user_id)
            .bind(key)
            .execute(pool)
            .await
        }
    }
    .map_err(|e| {
        error!("Database error updating user secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if update_result.rows_affected() > 0 {
        debug!(
            "Updated user secret in database: {}:{}:{}",
            script_uri, user_id, key
        );
        return Ok(());
    }

    // Item doesn't exist, create new one
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                INSERT INTO user_secrets (script_uri, user_id, key, value, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $5)
                "#,
            )
            .bind(script_uri)
            .bind(user_id)
            .bind(key)
            .bind(value)
            .bind(now)
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(
                r#"
                INSERT INTO user_secrets (script_uri, user_id, key, value, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $5)
                "#,
            )
            .bind(script_uri)
            .bind(user_id)
            .bind(key)
            .bind(value)
            .bind(now)
            .execute(pool)
            .await
        }
    }
    .map_err(|e| {
        error!("Database error inserting user secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Inserted user secret to database: {}:{}:{}",
        script_uri, user_id, key
    );
    Ok(())
}

/// Database-backed get user secret
pub(super) async fn db_get_user_secret<'e, E>(
    executor: E,
    script_uri: &str,
    user_id: &str,
    key: &str,
) -> AppResult<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT value FROM user_secrets WHERE script_uri = $1 AND user_id = $2 AND key = $3
        "#,
    )
    .bind(script_uri)
    .bind(user_id)
    .bind(key)
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        error!("Database error getting user secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = row {
        let raw: String = row.try_get("value").map_err(|e| {
            error!("Database error getting value: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        // Decrypt if encryption is configured and the value is in encrypted form
        let value = if let Some(enc) = GLOBAL_SECRET_ENCRYPTION.get() {
            match serde_json::from_str::<crate::security::encryption::EncryptedData>(&raw) {
                Ok(encrypted) => match enc.decrypt_field(&encrypted) {
                    Ok(plain) => plain,
                    Err(e) => {
                        error!(
                            "Failed to decrypt user secret {}:{}:{}: {}",
                            script_uri, user_id, key, e
                        );
                        raw
                    }
                },
                Err(_) => raw, // stored before encryption was enabled — return as-is
            }
        } else {
            raw
        };
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

/// Database-backed remove user secret
pub(super) async fn db_remove_user_secret<'e, E>(
    executor: E,
    script_uri: &str,
    user_id: &str,
    key: &str,
) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM user_secrets WHERE script_uri = $1 AND user_id = $2 AND key = $3
        "#,
    )
    .bind(script_uri)
    .bind(user_id)
    .bind(key)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error removing user secret: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!(
            "Removed user secret from database: {}:{}:{}",
            script_uri, user_id, key
        );
    } else {
        debug!(
            "User secret not found in database for removal: {}:{}:{}",
            script_uri, user_id, key
        );
    }

    Ok(existed)
}

/// Database-backed clear all user secrets for a script and user
pub(super) async fn db_clear_user_secrets<'e, E>(
    executor: E,
    script_uri: &str,
    user_id: &str,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        r#"
        DELETE FROM user_secrets WHERE script_uri = $1 AND user_id = $2
        "#,
    )
    .bind(script_uri)
    .bind(user_id)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error clearing user secrets: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Cleared all user secrets from database for script {} and user {}",
        script_uri, user_id
    );
    Ok(())
}

/// Set a script secret (key-value pair for a specific script)
pub fn set_script_secret_item(script_uri: &str, key: &str, value: &str) -> AppResult<()> {
    if script_uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Script URI cannot be empty".to_string()).into());
    }

    if key.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Key cannot be empty".to_string()).into());
    }

    if value.len() > MAX_STORAGE_VALUE_BYTES {
        return Err(RepositoryError::InvalidData("Value too large (>1MB)".to_string()).into());
    }

    let repo = get_repository();
    run_bounded(async { repo.set_script_secret(script_uri, key, value).await })
}

/// Get a script secret
pub fn get_script_secret_item(script_uri: &str, key: &str) -> Option<String> {
    let repo = get_repository();
    let result = run_bounded(async { repo.get_script_secret(script_uri, key).await });

    match result {
        Ok(value) => value,
        Err(e) => {
            error!("Failed to get script secret {}:{}: {}", script_uri, key, e);
            None
        }
    }
}

/// Resolve a secret by checking user_secrets first (if user_id is given), then script_secrets.
/// Never reads environment variables or config files — only the database is consulted.
/// Returns `None` if the repository is not initialized (e.g. in tests without a DB).
pub fn resolve_secret_db(script_uri: &str, key: &str, user_id: Option<&str>) -> Option<String> {
    get_repository_opt()?;
    if let Some(uid) = user_id
        && let Some(value) = get_user_secret_item(script_uri, uid, key)
    {
        return Some(value);
    }
    crate::repository::get_script_secret_item(script_uri, key)
}

/// Remove a script secret
pub fn remove_script_secret_item(script_uri: &str, key: &str) -> bool {
    let repo = get_repository();
    let result = run_bounded(async { repo.remove_script_secret(script_uri, key).await });

    match result {
        Ok(existed) => existed,
        Err(e) => {
            error!(
                "Failed to remove script secret {}:{}: {}",
                script_uri, key, e
            );
            false
        }
    }
}

/// Clear all script secrets for a specific script
pub fn clear_script_secrets(script_uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.clear_script_secrets(script_uri).await })
}

/// List all secret keys for a specific script
pub fn list_script_secrets(script_uri: &str) -> AppResult<Vec<String>> {
    let repo = get_repository();
    run_bounded(async { repo.list_script_secrets(script_uri).await })
}

/// Set a user secret (key-value pair for a specific script and user)
pub fn set_user_secret_item(
    script_uri: &str,
    user_id: &str,
    key: &str,
    value: &str,
) -> AppResult<()> {
    if script_uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Script URI cannot be empty".to_string()).into());
    }

    if user_id.trim().is_empty() {
        return Err(RepositoryError::InvalidData("User ID cannot be empty".to_string()).into());
    }

    if key.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Key cannot be empty".to_string()).into());
    }

    if value.len() > MAX_STORAGE_VALUE_BYTES {
        return Err(RepositoryError::InvalidData("Value too large (>1MB)".to_string()).into());
    }

    let repo = get_repository();
    run_bounded(async { repo.set_user_secret(script_uri, user_id, key, value).await })
}

/// Get a user secret
pub fn get_user_secret_item(script_uri: &str, user_id: &str, key: &str) -> Option<String> {
    let repo = get_repository();
    let result = run_bounded(async { repo.get_user_secret(script_uri, user_id, key).await });

    match result {
        Ok(value) => value,
        Err(e) => {
            error!(
                "Failed to get user secret {}:{}:{}: {}",
                script_uri, user_id, key, e
            );
            None
        }
    }
}

/// Remove a user secret
pub fn remove_user_secret_item(script_uri: &str, user_id: &str, key: &str) -> bool {
    let repo = get_repository();
    let result = run_bounded(async { repo.remove_user_secret(script_uri, user_id, key).await });

    match result {
        Ok(existed) => existed,
        Err(e) => {
            error!(
                "Failed to remove user secret {}:{}:{}: {}",
                script_uri, user_id, key, e
            );
            false
        }
    }
}

/// Clear all user secrets for a specific script and user
pub fn clear_user_secrets(script_uri: &str, user_id: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.clear_user_secrets(script_uri, user_id).await })
}

/// Global secret encryption instance (optional — if not set, secrets are stored plaintext)
pub(super) static GLOBAL_SECRET_ENCRYPTION: OnceLock<
    Arc<crate::security::encryption::DataEncryption>,
> = OnceLock::new();

/// Initialize the global secret encryption used for at-rest encryption of secret values.
/// Should be called once at startup if a `secret_encryption_key` is configured.
/// Returns `true` if initialized successfully, `false` if already initialized.
pub fn initialize_secret_encryption(enc: Arc<crate::security::encryption::DataEncryption>) -> bool {
    GLOBAL_SECRET_ENCRYPTION.set(enc).is_ok()
}

/// The at-rest encryption this engine was configured with, if any.
///
/// Exposed so that a caller storing something more sensitive than a script's
/// own secret can refuse when there is no key, rather than quietly writing it
/// in the clear the way the secret tables do.
pub fn secret_encryption() -> Option<Arc<crate::security::encryption::DataEncryption>> {
    GLOBAL_SECRET_ENCRYPTION.get().cloned()
}
