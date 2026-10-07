//! The files of a script's tree, keyed `(script_uri, path)`.

use super::*;
use crate::error::{AppError, AppResult};
use sqlx::Row;
use std::collections::HashMap;
use tracing::{debug, error, warn};

/// Asset representation
/// Assets are stored by URI and can be registered to public HTTP paths at runtime
#[derive(Debug, Clone)]
pub struct Asset {
    pub uri: String,
    pub name: Option<String>,
    pub mimetype: String,
    pub content: Vec<u8>,
    pub created_at: std::time::SystemTime,
    pub updated_at: std::time::SystemTime,
    pub script_uri: String,
}

/// Database-backed upsert asset
pub(super) async fn db_upsert_asset(
    mut executor: crate::database::TransactionExecutor<'_>,
    asset: &Asset,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Update the row this script owns. Matching on the path alone would let a
    // write take over an asset belonging to another script — the UPDATE would
    // find that script's row and overwrite it — so both halves of the key are
    // required here, as they are in every other asset query.
    let update_result = match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                UPDATE assets
                SET mimetype = $1, content = $2, updated_at = $3
                WHERE script_uri = $4 AND uri = $5
                "#,
            )
            .bind(&asset.mimetype)
            .bind(&asset.content)
            .bind(now)
            .bind(&asset.script_uri)
            .bind(&asset.uri)
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(
                r#"
                UPDATE assets
                SET mimetype = $1, content = $2, updated_at = $3
                WHERE script_uri = $4 AND uri = $5
                "#,
            )
            .bind(&asset.mimetype)
            .bind(&asset.content)
            .bind(now)
            .bind(&asset.script_uri)
            .bind(&asset.uri)
            .execute(pool)
            .await
        }
    }
    .map_err(|e| {
        error!("Database error updating asset: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if update_result.rows_affected() > 0 {
        debug!("Updated existing asset in database: {}", asset.uri);
        return Ok(());
    }

    // Asset doesn't exist, create new one
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
                r#"
                INSERT INTO assets (uri, mimetype, content, name, script_uri, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $6)
                "#,
            )
            .bind(&asset.uri)
            .bind(&asset.mimetype)
            .bind(&asset.content)
            .bind(&asset.name)
            .bind(&asset.script_uri)
            .bind(now)
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(
                r#"
                INSERT INTO assets (uri, mimetype, content, name, script_uri, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $6)
                "#,
            )
            .bind(&asset.uri)
            .bind(&asset.mimetype)
            .bind(&asset.content)
            .bind(&asset.name)
            .bind(&asset.script_uri)
            .bind(now)
            .execute(pool)
            .await
        }
    }
    .map_err(|e| {
        error!("Database error creating asset: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!("Created new asset in database: {}", asset.uri);
    Ok(())
}

/// Database-backed get asset by URI
pub(super) async fn db_get_asset<'e, E>(
    executor: E,
    script_uri: &str,
    uri: &str,
) -> AppResult<Option<Asset>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT uri, mimetype, content, name, script_uri, created_at, updated_at FROM assets WHERE script_uri = $1 AND uri = $2
        "#,
    )
    .bind(script_uri)
    .bind(uri)
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        error!("Database error getting asset: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = row {
        let uri: String = row.try_get("uri").map_err(|e| {
            error!("Database error getting uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let mimetype: String = row.try_get("mimetype").map_err(|e| {
            error!("Database error getting mimetype: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let content: Vec<u8> = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at").map_err(|e| {
            error!("Database error getting created_at: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at").map_err(|e| {
            error!("Database error getting updated_at: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let name: Option<String> = row.try_get("name").ok();
        let script_uri: String = row.try_get("script_uri").map_err(|e| {
            error!("Database error getting script_uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        Ok(Some(Asset {
            uri,
            name,
            mimetype,
            content,
            created_at: created_at.into(),
            updated_at: updated_at.into(),
            script_uri,
        }))
    } else {
        Ok(None)
    }
}

/// Database-backed list all assets for a script
pub(super) async fn db_list_assets<'e, E>(
    executor: E,
    script_uri: &str,
) -> AppResult<HashMap<String, Asset>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT uri, mimetype, content, name, script_uri, created_at, updated_at FROM assets WHERE script_uri = $1 ORDER BY uri
        "#,
    )
    .bind(script_uri)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error listing assets: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut assets = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("uri").map_err(|e| {
            error!("Database error getting uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let mimetype: String = row.try_get("mimetype").map_err(|e| {
            error!("Database error getting mimetype: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let content: Vec<u8> = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at").map_err(|e| {
            error!("Database error getting created_at: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at").map_err(|e| {
            error!("Database error getting updated_at: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let name: Option<String> = row.try_get("name").ok();
        let script_uri: String = row.try_get("script_uri").map_err(|e| {
            error!("Database error getting script_uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        assets.insert(
            uri.clone(),
            Asset {
                uri,
                name,
                mimetype,
                content,
                created_at: created_at.into(),
                updated_at: updated_at.into(),
                script_uri,
            },
        );
    }

    Ok(assets)
}

/// Database-backed delete asset
pub(super) async fn db_delete_asset<'e, E>(
    executor: E,
    script_uri: &str,
    uri: &str,
) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM assets WHERE script_uri = $1 AND uri = $2
        "#,
    )
    .bind(script_uri)
    .bind(uri)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error deleting asset: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!("Deleted asset from database: {}", uri);
    } else {
        debug!("Asset not found in database for deletion: {}", uri);
    }

    Ok(existed)
}

/// Helper function to get static assets embedded at compile time
pub(super) fn get_static_assets() -> HashMap<String, Asset> {
    let mut m = HashMap::new();
    let now = std::time::SystemTime::now();

    // Logo asset
    let logo_content = include_bytes!("../../assets/logo.svg").to_vec();
    let logo = Asset {
        uri: "logo.svg".to_string(),
        name: Some("Logo".to_string()),
        mimetype: "image/svg+xml".to_string(),
        content: logo_content,
        created_at: now,
        updated_at: now,
        script_uri: "https://example.com/core".to_string(),
    };
    m.insert("logo.svg".to_string(), logo);

    let favicon_content = include_bytes!("../../assets/favicon.ico").to_vec();
    let favicon = Asset {
        uri: "favicon.ico".to_string(),
        name: Some("Favicon".to_string()),
        mimetype: "image/x-icon".to_string(),
        content: favicon_content,
        created_at: now,
        updated_at: now,
        script_uri: "https://example.com/core".to_string(),
    };
    m.insert("favicon.ico".to_string(), favicon);

    let aiwebengine_dts_content = include_bytes!("../../assets/aiwebengine.d.ts").to_vec();
    let aiwebengine_dts = Asset {
        uri: "aiwebengine.d.ts".to_string(),
        name: Some("TypeScript Type Definitions".to_string()),
        mimetype: "text/plain".to_string(),
        content: aiwebengine_dts_content,
        created_at: now,
        updated_at: now,
        script_uri: "https://example.com/core".to_string(),
    };
    m.insert("aiwebengine.d.ts".to_string(), aiwebengine_dts);

    let script_primer = Asset {
        uri: "script-primer.md".to_string(),
        name: Some("Script primer".to_string()),
        mimetype: "text/markdown".to_string(),
        content: include_bytes!("../../assets/script-primer.md").to_vec(),
        created_at: now,
        updated_at: now,
        script_uri: "https://example.com/core".to_string(),
    };
    m.insert("script-primer.md".to_string(), script_primer);

    m
}

/// Fetch assets with error handling (static + dynamic)
pub fn fetch_assets(script_uri: &str) -> HashMap<String, Asset> {
    let repo = get_repository();

    let result = run_bounded(async { repo.list_assets(script_uri).await });

    match result {
        Ok(assets) => {
            debug!(
                "Loaded {} assets from repository for script {}",
                assets.len(),
                script_uri
            );
            assets
        }
        Err(e) => {
            error!("Failed to fetch assets: {}", e);
            HashMap::new()
        }
    }
}

/// Fetch single asset by URI with error handling (dynamic first, then static)
pub fn fetch_asset(script_uri: &str, uri: &str) -> Option<Asset> {
    run_blocking(fetch_asset_async(script_uri, uri))
}

/// Async variant of [`fetch_asset`] for callers already in async context
pub async fn fetch_asset_async(script_uri: &str, uri: &str) -> Option<Asset> {
    let repo = get_repository();

    // Try repository first (DB or Memory)
    let result = repo.get_asset(script_uri, uri).await;

    match result {
        Ok(Some(asset)) => {
            debug!("Loaded asset from repository: {}", uri);
            return Some(asset);
        }
        Ok(None) => {
            // Not in repository, check static assets if script_uri is core
        }
        Err(e) => {
            warn!("Repository asset fetch failed for {}: {}", uri, e);
            // Fall through to static assets
        }
    }

    // Check static assets if script_uri is core
    if script_uri == "https://example.com/core"
        && let Some(asset) = get_static_assets().get(uri)
    {
        return Some(asset.clone());
    }

    None
}

/// Upsert asset with validation and error handling
pub fn upsert_asset(asset: Asset) -> AppResult<()> {
    run_blocking(upsert_asset_async(asset))
}

/// The storage-level checks every asset write passes, whether it arrives
/// alone or as part of a batch.
pub(super) fn validate_asset(asset: &Asset) -> AppResult<()> {
    if asset.uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Asset URI cannot be empty".to_string()).into());
    }

    // The entrypoint is source rather than payload, so it meets the ceiling
    // source has always met. The two used to apply to two different stores
    // and could not disagree; now that the root is a file of the same tree,
    // taking the file ceiling for it would have raised the limit on a
    // script's source from 1MB to 10MB by way of a storage change nobody
    // meant as a policy change.
    let (ceiling, label) = if crate::module_loader::is_root_module_name(&asset.uri) {
        (MAX_SCRIPT_CONTENT_BYTES, "Script content too large (>1MB)")
    } else {
        (MAX_ASSET_CONTENT_BYTES, "Asset content too large (>10MB)")
    };
    if asset.content.len() > ceiling {
        return Err(RepositoryError::InvalidData(label.to_string()).into());
    }

    if asset.mimetype.trim().is_empty() {
        return Err(RepositoryError::InvalidData("MIME type cannot be empty".to_string()).into());
    }

    Ok(())
}

/// Async variant of [`upsert_asset`] for callers already in async context
pub async fn upsert_asset_async(asset: Asset) -> AppResult<()> {
    validate_asset(&asset)?;

    let repo = get_repository();
    repo.upsert_asset(asset).await
}

/// Upsert several of one script's assets as a single unit.
pub fn upsert_assets(script_uri: &str, assets: Vec<Asset>) -> AppResult<()> {
    run_blocking(upsert_assets_async(script_uri, assets))
}

/// Async variant of [`upsert_assets`] for callers already in async context.
///
/// Every asset is validated before any of them is written, so a rejected entry
/// costs nothing: the transaction underneath never opens.
pub async fn upsert_assets_async(script_uri: &str, assets: Vec<Asset>) -> AppResult<()> {
    for asset in &assets {
        validate_asset(asset)?;
        if asset.script_uri != script_uri {
            return Err(RepositoryError::InvalidData(format!(
                "Asset '{}' belongs to script '{}', not '{}'",
                asset.uri, asset.script_uri, script_uri
            ))
            .into());
        }
    }

    let repo = get_repository();
    repo.upsert_assets(script_uri, assets).await
}

/// Delete asset with error handling  
/// Write some of a script's assets and remove others in one transaction.
///
/// Returns how many of the named removals actually removed something. A sync
/// naming a file the script no longer has is not an error; it is a sync that
/// has already happened.
pub fn sync_assets(script_uri: &str, assets: Vec<Asset>, delete: Vec<String>) -> AppResult<usize> {
    run_blocking(sync_assets_async(script_uri, assets, delete))
}

/// Async variant of [`sync_assets`].
pub async fn sync_assets_async(
    script_uri: &str,
    assets: Vec<Asset>,
    delete: Vec<String>,
) -> AppResult<usize> {
    for asset in &assets {
        validate_asset(asset)?;
        if asset.script_uri != script_uri {
            return Err(RepositoryError::InvalidData(format!(
                "Asset '{}' belongs to script '{}', not '{}'",
                asset.uri, asset.script_uri, script_uri
            ))
            .into());
        }
    }

    get_repository()
        .sync_assets(script_uri, assets, delete)
        .await
}

pub fn delete_asset(script_uri: &str, uri: &str) -> bool {
    let repo = get_repository();
    let result = run_bounded(async { repo.delete_asset(script_uri, uri).await });

    match result {
        Ok(existed) => existed,
        Err(e) => {
            error!("Failed to delete asset {}: {}", uri, e);
            false
        }
    }
}
