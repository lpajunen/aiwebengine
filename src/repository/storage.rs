//! `scriptStorage` and `personalStorage`: key-value properties per script and per person.

use super::*;
use crate::error::{AppError, AppResult};
use sqlx::Row;
use tracing::{debug, error};

/// Database-backed set script storage item
pub(super) async fn db_set_script_properties_item(
    mut executor: crate::database::TransactionExecutor<'_>,
    script_uri: &str,
    key: &str,
    value: &str,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Try to update existing item
    let update_result = match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                UPDATE script_properties
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
                UPDATE script_properties
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
        error!("Database error updating script storage: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if update_result.rows_affected() > 0 {
        debug!(
            "Updated script storage item in database: {}:{}",
            script_uri, key
        );
        return Ok(());
    }

    // Item doesn't exist, create new one
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                INSERT INTO script_properties (script_uri, key, value, created_at, updated_at)
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
                INSERT INTO script_properties (script_uri, key, value, created_at, updated_at)
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
        error!("Database error creating script storage item: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Created new script storage item in database: {}:{}",
        script_uri, key
    );
    Ok(())
}

/// Database-backed get script storage item
pub(super) async fn db_get_script_properties_item<'e, E>(
    executor: E,
    script_uri: &str,
    key: &str,
) -> AppResult<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT value FROM script_properties WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        error!("Database error getting script storage item: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = row {
        let value: String = row.try_get("value").map_err(|e| {
            error!("Database error getting value: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

/// Database-backed remove script storage item
pub(super) async fn db_remove_script_properties_item<'e, E>(
    executor: E,
    script_uri: &str,
    key: &str,
) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM script_properties WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error removing script storage item: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!(
            "Removed script storage item from database: {}:{}",
            script_uri, key
        );
    } else {
        debug!(
            "Script storage item not found in database for removal: {}:{}",
            script_uri, key
        );
    }

    Ok(existed)
}

/// Database-backed clear all script storage for a script
pub(super) async fn db_clear_script_properties<'e, E>(
    executor: E,
    script_uri: &str,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        r#"
        DELETE FROM script_properties WHERE script_uri = $1
        "#,
    )
    .bind(script_uri)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error clearing script storage: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Cleared all script storage items from database for script: {}",
        script_uri
    );
    Ok(())
}

/// Database-backed set personal storage item
pub(super) async fn db_set_user_properties_item(
    mut executor: crate::database::TransactionExecutor<'_>,
    script_uri: &str,
    user_id: &str,
    key: &str,
    value: &str,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Try to update existing item
    let update_result = match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                UPDATE user_properties
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
                UPDATE user_properties
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
        error!("Database error updating personal storage: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if update_result.rows_affected() > 0 {
        debug!(
            "Updated personal storage item in database: {}:{}:{}",
            script_uri, user_id, key
        );
        return Ok(());
    }

    // Item doesn't exist, create new one
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                INSERT INTO user_properties (script_uri, user_id, key, value, created_at, updated_at)
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
                INSERT INTO user_properties (script_uri, user_id, key, value, created_at, updated_at)
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
        error!("Database error inserting personal storage item: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Inserted personal storage item to database: {}:{}:{}",
        script_uri, user_id, key
    );
    Ok(())
}

/// Database-backed get personal storage item
pub(super) async fn db_get_user_properties_item<'e, E>(
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
        SELECT value FROM user_properties WHERE script_uri = $1 AND user_id = $2 AND key = $3
        "#,
    )
    .bind(script_uri)
    .bind(user_id)
    .bind(key)
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        error!("Database error getting personal storage item: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = row {
        let value: String = row.try_get("value").map_err(|e| {
            error!("Database error getting value: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        Ok(Some(value))
    } else {
        Ok(None)
    }
}

/// Database-backed remove personal storage item
pub(super) async fn db_remove_user_properties_item<'e, E>(
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
        DELETE FROM user_properties WHERE script_uri = $1 AND user_id = $2 AND key = $3
        "#,
    )
    .bind(script_uri)
    .bind(user_id)
    .bind(key)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error removing personal storage item: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!(
            "Removed personal storage item from database: {}:{}:{}",
            script_uri, user_id, key
        );
    } else {
        debug!(
            "Personal storage item not found in database for removal: {}:{}:{}",
            script_uri, user_id, key
        );
    }

    Ok(existed)
}

/// Database-backed clear all personal storage for a script and user
pub(super) async fn db_clear_user_properties<'e, E>(
    executor: E,
    script_uri: &str,
    user_id: &str,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        r#"
        DELETE FROM user_properties WHERE script_uri = $1 AND user_id = $2
        "#,
    )
    .bind(script_uri)
    .bind(user_id)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error clearing personal storage: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Cleared all personal storage items from database for script {} and user {}",
        script_uri, user_id
    );
    Ok(())
}

/// Database-backed list script secret keys for a script
pub(super) async fn db_list_script_properties_keys<'e, E>(
    executor: E,
    script_uri: &str,
) -> AppResult<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query_scalar::<_, String>(
        r#"
        SELECT key FROM script_properties WHERE script_uri = $1 ORDER BY key ASC
        "#,
    )
    .bind(script_uri)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error listing script storage keys: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Listed {} script storage keys from database for script: {}",
        rows.len(),
        script_uri
    );
    Ok(rows)
}

pub(super) async fn db_list_user_properties_keys<'e, E>(
    executor: E,
    script_uri: &str,
    user_id: &str,
) -> AppResult<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query_scalar::<_, String>(
        r#"
        SELECT key FROM user_properties WHERE script_uri = $1 AND user_id = $2 ORDER BY key ASC
        "#,
    )
    .bind(script_uri)
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error listing personal storage keys: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Listed {} personal storage keys from database for script {} user {}",
        rows.len(),
        script_uri,
        user_id
    );
    Ok(rows)
}

/// Set a script storage item (key-value pair for a specific script)
pub fn set_script_properties_item(script_uri: &str, key: &str, value: &str) -> AppResult<()> {
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
    run_bounded(async { repo.set_script_properties(script_uri, key, value).await })
}

/// Get a script storage item
pub fn get_script_properties_item(script_uri: &str, key: &str) -> Option<String> {
    let repo = get_repository();
    let result = run_bounded(async { repo.get_script_properties(script_uri, key).await });

    match result {
        Ok(value) => value,
        Err(e) => {
            error!(
                "Failed to get script storage item {}:{}: {}",
                script_uri, key, e
            );
            None
        }
    }
}

/// Remove a script storage item
pub fn remove_script_properties_item(script_uri: &str, key: &str) -> bool {
    let repo = get_repository();
    let result = run_bounded(async { repo.remove_script_properties(script_uri, key).await });

    match result {
        Ok(existed) => existed,
        Err(e) => {
            error!(
                "Failed to remove script storage item {}:{}: {}",
                script_uri, key, e
            );
            false
        }
    }
}

/// Clear all script storage items for a specific script
pub fn clear_script_properties(script_uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.clear_script_properties(script_uri).await })
}

/// List the keys a script has in script storage, in ascending order.
///
/// What `length` and `key(i)` are built on: the Web Storage interface indexes
/// its keys, and a stable order is what makes indexing mean anything.
pub fn list_script_properties_keys(script_uri: &str) -> Vec<String> {
    let repo = get_repository();
    let result = run_bounded(async { repo.list_script_properties_keys(script_uri).await });

    match result {
        Ok(keys) => keys,
        Err(e) => {
            error!(
                "Failed to list script storage keys for {}: {}",
                script_uri, e
            );
            Vec::new()
        }
    }
}

/// Set a personal storage item (key-value pair for a specific script and user)
pub fn set_user_properties_item(
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
    run_bounded(async {
        repo.set_user_properties(script_uri, user_id, key, value)
            .await
    })
}

/// Get a personal storage item
pub fn get_user_properties_item(script_uri: &str, user_id: &str, key: &str) -> Option<String> {
    let repo = get_repository();
    let result = run_bounded(async { repo.get_user_properties(script_uri, user_id, key).await });

    match result {
        Ok(value) => value,
        Err(e) => {
            error!(
                "Failed to get personal storage item {}:{}:{}: {}",
                script_uri, user_id, key, e
            );
            None
        }
    }
}

/// Remove a personal storage item
pub fn remove_user_properties_item(script_uri: &str, user_id: &str, key: &str) -> bool {
    let repo = get_repository();
    let result = run_bounded(async { repo.remove_user_properties(script_uri, user_id, key).await });

    match result {
        Ok(existed) => existed,
        Err(e) => {
            error!(
                "Failed to remove personal storage item {}:{}:{}: {}",
                script_uri, user_id, key, e
            );
            false
        }
    }
}

/// Clear all personal storage items for a specific script and user
pub fn clear_user_properties(script_uri: &str, user_id: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.clear_user_properties(script_uri, user_id).await })
}

/// List the keys a user has in a script's personal storage, in ascending order.
pub fn list_user_properties_keys(script_uri: &str, user_id: &str) -> Vec<String> {
    let repo = get_repository();
    let result = run_bounded(async { repo.list_user_properties_keys(script_uri, user_id).await });

    match result {
        Ok(keys) => keys,
        Err(e) => {
            error!(
                "Failed to list personal storage keys for {} user {}: {}",
                script_uri, user_id, e
            );
            Vec::new()
        }
    }
}
