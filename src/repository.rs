use crate::error::{AppError, AppResult};
use crate::scheduler;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

/// Built-in scripts that must remain privileged by default
pub const PRIVILEGED_BOOTSTRAP_SCRIPTS: &[&str] = &[
    "https://example.com/core",
    "https://example.com/cli",
    "https://example.com/editor",
    "https://example.com/admin",
    "https://example.com/auth",
];

/// Helper to run async code in a blocking context, handling different runtime scenarios
fn run_blocking<F, R>(future: F) -> R
where
    F: std::future::Future<Output = R>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // We are in a runtime. Use block_in_place to avoid blocking the reactor.
            // Note: This requires a multi-threaded runtime. If called from a single-threaded
            // runtime (like default #[tokio::test]), this will panic.
            tokio::task::block_in_place(move || handle.block_on(future))
        }
        Err(_) => {
            // No runtime available. Create a temporary single-threaded runtime.
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create temporary runtime")
                .block_on(future)
        }
    }
}

fn is_bootstrap_script(uri: &str) -> bool {
    PRIVILEGED_BOOTSTRAP_SCRIPTS
        .iter()
        .any(|bootstrap_uri| bootstrap_uri == &uri)
}

fn default_privileged_for(uri: &str) -> bool {
    is_bootstrap_script(uri)
}

/// Defines the types of repository errors that can occur
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("Mutex lock failed: {0}")]
    LockError(String),
    #[error("Script not found: {0}")]
    ScriptNotFound(String),
    #[error("Asset not found: {0}")]
    AssetNotFound(String),
    #[error("Invalid data format: {0}")]
    InvalidData(String),
}

/// OpenAPI metadata for a registered route
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteMetadata {
    pub handler_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl RouteMetadata {
    pub fn simple(handler_name: String) -> Self {
        Self {
            handler_name,
            summary: None,
            description: None,
            tags: Vec::new(),
        }
    }
}

/// Route registration: (path, method) -> RouteMetadata
pub type RouteRegistrations = HashMap<(String, String), RouteMetadata>;

/// Log entry with timestamp information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub message: String,
    pub level: String,
    pub timestamp: SystemTime,
}

impl LogEntry {
    pub fn new(message: String, level: String, timestamp: SystemTime) -> Self {
        Self {
            message,
            level,
            timestamp,
        }
    }
}

/// Script metadata for tracking initialization status and registrations
#[derive(Debug, Clone)]
pub struct ScriptMetadata {
    pub uri: String,
    pub name: Option<String>,
    pub content: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub initialized: bool,
    pub init_error: Option<String>,
    pub last_init_time: Option<SystemTime>,
    /// Cached route registrations from init() function
    pub registrations: RouteRegistrations,
    pub privileged: bool,
}

impl ScriptMetadata {
    /// Create a new script metadata instance
    pub fn new(uri: String, content: String) -> Self {
        let now = SystemTime::now();
        // Extract name from URI (last segment after /)
        let name = uri.rsplit('/').next().map(String::from);
        Self {
            uri,
            name,
            content,
            created_at: now,
            updated_at: now,
            initialized: false,
            init_error: None,
            last_init_time: None,
            registrations: HashMap::new(),
            privileged: false,
        }
    }

    /// Mark script as initialized successfully
    pub fn mark_initialized(&mut self) {
        self.initialized = true;
        self.init_error = None;
        self.last_init_time = Some(SystemTime::now());
    }

    /// Mark script as initialized successfully with registrations
    pub fn mark_initialized_with_registrations(&mut self, registrations: RouteRegistrations) {
        self.initialized = true;
        self.init_error = None;
        self.last_init_time = Some(SystemTime::now());
        self.registrations = registrations;
    }

    /// Mark script initialization as failed
    pub fn mark_init_failed(&mut self, error: String) {
        self.initialized = false;
        self.init_error = Some(error);
        self.last_init_time = Some(SystemTime::now());
    }

    /// Update script content
    pub fn update_content(&mut self, new_content: String) {
        self.content = new_content;
        self.updated_at = SystemTime::now();
        // Reset initialization status when content changes
        self.initialized = false;
        self.init_error = None;
        // Clear cached registrations when content changes
        self.registrations.clear();
    }
}

/// Script security metadata exposed to admin tooling
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScriptSecurityProfile {
    pub uri: String,
    pub privileged: bool,
    pub default_privileged: bool,
}

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
}

static DYNAMIC_SCRIPTS: OnceLock<Mutex<HashMap<String, ScriptMetadata>>> = OnceLock::new();
static DYNAMIC_LOGS: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
static DYNAMIC_ASSETS: OnceLock<Mutex<HashMap<String, Asset>>> = OnceLock::new();
static DYNAMIC_SHARED_STORAGE: OnceLock<Mutex<HashMap<String, HashMap<String, String>>>> =
    OnceLock::new();
static SCRIPT_PRIVILEGE_OVERRIDES: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

/// Safe mutex access with recovery from poisoned state
fn safe_lock_scripts() -> AppResult<std::sync::MutexGuard<'static, HashMap<String, ScriptMetadata>>>
{
    let store = DYNAMIC_SCRIPTS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Scripts mutex was poisoned, recovering with new data");
            // In a poisoned state, we can still access the data but should log this
            // In production, you might want to restart the component or use more sophisticated recovery
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned mutex: {}", e);
                AppError::Internal {
                    message: format!("Unrecoverable mutex poisoning: {}", e),
                }
            })
        }
    }
}

fn safe_lock_logs() -> AppResult<std::sync::MutexGuard<'static, HashMap<String, Vec<String>>>> {
    let store = DYNAMIC_LOGS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Logs mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned logs mutex: {}", e);
                AppError::Internal {
                    message: format!("Unrecoverable mutex poisoning: {}", e),
                }
            })
        }
    }
}

fn safe_lock_assets() -> AppResult<std::sync::MutexGuard<'static, HashMap<String, Asset>>> {
    let store = DYNAMIC_ASSETS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Assets mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned assets mutex: {}", e);
                AppError::Internal {
                    message: format!("Unrecoverable mutex poisoning: {}", e),
                }
            })
        }
    }
}

type SharedStorageGuard<'a> = std::sync::MutexGuard<'a, HashMap<String, HashMap<String, String>>>;

fn safe_lock_shared_storage() -> AppResult<SharedStorageGuard<'static>> {
    let store = DYNAMIC_SHARED_STORAGE.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Shared storage mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!(
                    "Failed to recover from poisoned shared storage mutex: {}",
                    e
                );
                AppError::Internal {
                    message: format!("Unrecoverable mutex poisoning: {}", e),
                }
            })
        }
    }
}

fn safe_lock_privilege_overrides()
-> AppResult<std::sync::MutexGuard<'static, HashMap<String, bool>>> {
    let store = SCRIPT_PRIVILEGE_OVERRIDES.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Privilege override mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned privilege mutex: {}", e);
                AppError::Internal {
                    message: format!("Unrecoverable mutex poisoning: {}", e),
                }
            })
        }
    }
}

/// Get database pool if available
fn get_db_pool() -> Option<std::sync::Arc<crate::database::Database>> {
    crate::database::get_global_database()
}

/// Database-backed upsert script
async fn db_upsert_script(pool: &PgPool, uri: &str, content: &str) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Extract name from URI (last segment after /)
    let name = uri.rsplit('/').next().unwrap_or(uri);

    // Try to update existing script
    let update_result = sqlx::query(
        r#"
        UPDATE scripts
        SET content = $1, updated_at = $2, name = COALESCE(name, $4)
        WHERE uri = $3
        "#,
    )
    .bind(content)
    .bind(now)
    .bind(uri)
    .bind(name)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error updating script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if update_result.rows_affected() > 0 {
        debug!("Updated existing script in database: {}", uri);
        return Ok(());
    }

    // Script doesn't exist, create new one
    sqlx::query(
        r#"
        INSERT INTO scripts (uri, content, name, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $4)
        "#,
    )
    .bind(uri)
    .bind(content)
    .bind(name)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error creating script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!("Created new script in database: {}", uri);
    Ok(())
}

/// Database-backed get script
async fn db_get_script(pool: &PgPool, uri: &str) -> AppResult<Option<String>> {
    let row = sqlx::query(
        r#"
        SELECT content FROM scripts WHERE uri = $1
        "#,
    )
    .bind(uri)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error getting script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = row {
        let content: String = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        Ok(Some(content))
    } else {
        Ok(None)
    }
}

/// Database-backed list all scripts
async fn db_list_scripts(pool: &PgPool) -> AppResult<HashMap<String, String>> {
    let rows = sqlx::query(
        r#"
        SELECT uri, content FROM scripts ORDER BY uri
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| {
        error!("Database error listing scripts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut scripts = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("uri").map_err(|e| {
            error!("Database error getting uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let content: String = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        scripts.insert(uri, content);
    }

    Ok(scripts)
}

/// Database-backed delete script
async fn db_delete_script(pool: &PgPool, uri: &str) -> AppResult<bool> {
    let result = sqlx::query(
        r#"
        DELETE FROM scripts WHERE uri = $1
        "#,
    )
    .bind(uri)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error deleting script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!("Deleted script from database: {}", uri);
    } else {
        debug!("Script not found in database for deletion: {}", uri);
    }

    Ok(existed)
}

/// Database-backed getter for script privilege flag
async fn db_get_script_privileged(pool: &PgPool, uri: &str) -> AppResult<Option<bool>> {
    let row = sqlx::query(
        r#"
        SELECT privileged FROM scripts WHERE uri = $1
        "#,
    )
    .bind(uri)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error getting script privilege: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(row) = row {
        let privileged: bool = row.try_get("privileged").map_err(|e| {
            error!("Database error parsing privilege flag: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        Ok(Some(privileged))
    } else {
        Ok(None)
    }
}

/// Database-backed setter for script privilege flag
async fn db_set_script_privileged(pool: &PgPool, uri: &str, privileged: bool) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE scripts SET privileged = $1, updated_at = $2 WHERE uri = $3
        "#,
    )
    .bind(privileged)
    .bind(chrono::Utc::now())
    .bind(uri)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error updating script privilege: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    Ok(())
}

/// Database-backed set shared storage item
async fn db_set_shared_storage_item(
    pool: &PgPool,
    script_uri: &str,
    key: &str,
    value: &str,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Try to update existing item
    let update_result = sqlx::query(
        r#"
        UPDATE shared_storage
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
    .map_err(|e| {
        error!("Database error updating shared storage: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if update_result.rows_affected() > 0 {
        debug!(
            "Updated shared storage item in database: {}:{}",
            script_uri, key
        );
        return Ok(());
    }

    // Item doesn't exist, create new one
    sqlx::query(
        r#"
        INSERT INTO shared_storage (script_uri, key, value, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $4)
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .bind(value)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error creating shared storage item: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Created new shared storage item in database: {}:{}",
        script_uri, key
    );
    Ok(())
}

/// Database-backed get shared storage item
async fn db_get_shared_storage_item(
    pool: &PgPool,
    script_uri: &str,
    key: &str,
) -> AppResult<Option<String>> {
    let row = sqlx::query(
        r#"
        SELECT value FROM shared_storage WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error getting shared storage item: {}", e);
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

/// Database-backed remove shared storage item
async fn db_remove_shared_storage_item(
    pool: &PgPool,
    script_uri: &str,
    key: &str,
) -> AppResult<bool> {
    let result = sqlx::query(
        r#"
        DELETE FROM shared_storage WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error removing shared storage item: {}", e);
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

/// Database-backed clear all shared storage for a script
async fn db_clear_shared_storage(pool: &PgPool, script_uri: &str) -> AppResult<()> {
    sqlx::query(
        r#"
        DELETE FROM shared_storage WHERE script_uri = $1
        "#,
    )
    .bind(script_uri)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error clearing shared storage: {}", e);
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

/// Database-backed insert log message
async fn db_insert_log_message(
    pool: &PgPool,
    script_uri: &str,
    message: &str,
    log_level: &str,
) -> AppResult<()> {
    sqlx::query(
        r#"
        INSERT INTO logs (script_uri, message, log_level, created_at)
        VALUES ($1, $2, $3, NOW())
        "#,
    )
    .bind(script_uri)
    .bind(message)
    .bind(log_level)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error inserting log message: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Inserted log message to database for script: {}",
        script_uri
    );
    Ok(())
}

/// Database-backed fetch log messages for a script
async fn db_fetch_log_messages(pool: &PgPool, script_uri: &str) -> AppResult<Vec<LogEntry>> {
    let rows = sqlx::query(
        r#"
        SELECT message, log_level, created_at FROM logs
        WHERE script_uri = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(script_uri)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        error!("Database error fetching log messages: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let messages = rows
        .into_iter()
        .map(|row| {
            let message: String = row.try_get("message")?;
            let log_level: String = row.try_get("log_level")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            // Convert chrono DateTime to SystemTime
            let system_time = SystemTime::from(created_at);
            Ok(LogEntry::new(message, log_level, system_time))
        })
        .collect::<Result<Vec<LogEntry>, sqlx::Error>>()
        .map_err(|e| {
            error!("Database error getting message/level/timestamp: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    Ok(messages)
}

/// Database-backed fetch all log messages
async fn db_fetch_all_log_messages(pool: &PgPool) -> AppResult<Vec<LogEntry>> {
    let rows = sqlx::query(
        r#"
        SELECT message, log_level, created_at FROM logs
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| {
        error!("Database error fetching all log messages: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let messages = rows
        .into_iter()
        .map(|row| {
            let message: String = row.try_get("message")?;
            let log_level: String = row.try_get("log_level")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            // Convert chrono DateTime to SystemTime
            let system_time = SystemTime::from(created_at);
            Ok(LogEntry::new(message, log_level, system_time))
        })
        .collect::<Result<Vec<LogEntry>, sqlx::Error>>()
        .map_err(|e| {
            error!("Database error getting message/level/timestamp: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    Ok(messages)
}

/// Database-backed clear log messages for a script
async fn db_clear_log_messages(pool: &PgPool, script_uri: &str) -> AppResult<()> {
    sqlx::query(
        r#"
        DELETE FROM logs WHERE script_uri = $1
        "#,
    )
    .bind(script_uri)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error clearing log messages: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Cleared log messages from database for script: {}",
        script_uri
    );
    Ok(())
}

/// Database-backed prune log messages (keep only latest 20 per script)
async fn db_prune_log_messages(pool: &PgPool) -> AppResult<()> {
    // For each script_uri, keep only the 20 most recent messages
    sqlx::query(
        r#"
        DELETE FROM logs
        WHERE id IN (
            SELECT id FROM (
                SELECT id,
                       ROW_NUMBER() OVER (PARTITION BY script_uri ORDER BY created_at DESC) as rn
                FROM logs
            ) ranked
            WHERE rn > 20
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error pruning log messages: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!("Pruned log messages in database, keeping 20 entries per script");
    Ok(())
}

/// Database-backed upsert asset
async fn db_upsert_asset(pool: &PgPool, asset: &Asset) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Try to update existing asset
    let update_result = sqlx::query(
        r#"
        UPDATE assets
        SET mimetype = $1, content = $2, updated_at = $3
        WHERE uri = $4
        "#,
    )
    .bind(&asset.mimetype)
    .bind(&asset.content)
    .bind(now)
    .bind(&asset.uri)
    .execute(pool)
    .await
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
    sqlx::query(
        r#"
        INSERT INTO assets (uri, mimetype, content, name, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $5)
        "#,
    )
    .bind(&asset.uri)
    .bind(&asset.mimetype)
    .bind(&asset.content)
    .bind(&asset.name)
    .bind(now)
    .execute(pool)
    .await
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
async fn db_get_asset(pool: &PgPool, uri: &str) -> AppResult<Option<Asset>> {
    let row = sqlx::query(
        r#"
        SELECT uri, mimetype, content, name, created_at, updated_at FROM assets WHERE uri = $1
        "#,
    )
    .bind(uri)
    .fetch_optional(pool)
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
        Ok(Some(Asset {
            uri,
            name,
            mimetype,
            content,
            created_at: created_at.into(),
            updated_at: updated_at.into(),
        }))
    } else {
        Ok(None)
    }
}

/// Database-backed list all assets
async fn db_list_assets(pool: &PgPool) -> AppResult<HashMap<String, Asset>> {
    let rows = sqlx::query(
        r#"
        SELECT uri, mimetype, content, name, created_at, updated_at FROM assets ORDER BY uri
        "#,
    )
    .fetch_all(pool)
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
        assets.insert(
            uri.clone(),
            Asset {
                uri,
                name,
                mimetype,
                content,
                created_at: created_at.into(),
                updated_at: updated_at.into(),
            },
        );
    }

    Ok(assets)
}

/// Database-backed delete asset
async fn db_delete_asset(pool: &PgPool, uri: &str) -> AppResult<bool> {
    let result = sqlx::query(
        r#"
        DELETE FROM assets WHERE uri = $1
        "#,
    )
    .bind(uri)
    .execute(pool)
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

/// Fetch scripts from repository with proper error handling
pub fn fetch_scripts() -> HashMap<String, String> {
    let repo = get_repository();

    // Use run_blocking to call async repository method
    let result = run_blocking(async { repo.list_scripts().await });

    match result {
        Ok(scripts) => {
            debug!("Loaded {} scripts from repository", scripts.len());
            scripts
        }
        Err(e) => {
            error!("Failed to fetch scripts: {}", e);
            HashMap::new()
        }
    }
}

/// Fetch a single script by URI with proper error handling
pub fn fetch_script(uri: &str) -> Option<String> {
    let repo = get_repository();

    let result = run_blocking(async { repo.get_script(uri).await });

    match result {
        Ok(Some(script)) => {
            debug!("Loaded script from repository: {}", uri);
            Some(script)
        }
        Ok(None) => None,
        Err(e) => {
            warn!("Failed to fetch script {}: {}", uri, e);
            None
        }
    }
}

/// Get metadata for a script
pub fn get_script_metadata(uri: &str) -> AppResult<ScriptMetadata> {
    let repo = get_repository();
    run_blocking(async { repo.get_script_metadata(uri).await })
}

/// Get metadata for all scripts
pub fn get_all_script_metadata() -> AppResult<Vec<ScriptMetadata>> {
    let repo = get_repository();
    run_blocking(async { repo.get_all_script_metadata().await })
}

pub fn mark_script_init_failed(uri: &str, error: String) -> AppResult<()> {
    let repo = get_repository();
    run_blocking(async {
        repo.update_script_init_status(uri, false, Some(error), None)
            .await
    })
}

pub fn mark_script_initialized(uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_blocking(async { repo.update_script_init_status(uri, true, None, None).await })
}

pub fn mark_script_initialized_with_registrations(
    uri: &str,
    registrations: RouteRegistrations,
) -> AppResult<()> {
    let repo = get_repository();
    run_blocking(async {
        repo.update_script_init_status(uri, true, None, Some(registrations))
            .await
    })
}

/// Determine current privilege state for a script
pub fn get_script_security_profile(uri: &str) -> AppResult<ScriptSecurityProfile> {
    let default_privileged = default_privileged_for(uri);
    let repo = get_repository();

    // Try repository first
    let result = run_blocking(async { repo.get_script_privileged(uri).await });

    if let Ok(Some(privileged)) = result {
        return Ok(ScriptSecurityProfile {
            uri: uri.to_string(),
            privileged,
            default_privileged,
        });
    }

    // If repository didn't have it (or failed), check overrides explicitly
    // This handles the case where we are in Postgres mode but the script is static/local
    // and has a local override.
    if let Ok(guard) = safe_lock_privilege_overrides()
        && let Some(value) = guard.get(uri)
    {
        return Ok(ScriptSecurityProfile {
            uri: uri.to_string(),
            privileged: *value,
            default_privileged,
        });
    }

    Ok(ScriptSecurityProfile {
        uri: uri.to_string(),
        privileged: default_privileged,
        default_privileged,
    })
}

/// Helper used by security enforcement
pub fn is_script_privileged(uri: &str) -> AppResult<bool> {
    Ok(get_script_security_profile(uri)?.privileged)
}

/// Update the privilege flag for a script
pub fn set_script_privileged(uri: &str, privileged: bool) -> AppResult<()> {
    // Ensure script exists before toggling
    if fetch_script(uri).is_none() {
        return Err(RepositoryError::ScriptNotFound(uri.to_string()).into());
    }

    let repo = get_repository();

    // 1. Try to update repository (DB or Memory)
    let result = run_blocking(async { repo.set_script_privileged(uri, privileged).await });

    if let Err(e) = result {
        warn!("Repository set privileged failed for {}: {}", uri, e);
    }

    // 2. Also update in-memory structures to ensure static scripts or fallbacks work

    // Update dynamic metadata cache if present
    if let Ok(mut guard) = safe_lock_scripts()
        && let Some(metadata) = guard.get_mut(uri)
    {
        metadata.privileged = privileged;
        debug!(script = %uri, privileged, "Updated in-memory script privilege");
        return Ok(());
    }

    // Persist override for static scripts or when metadata not present
    let mut guard = safe_lock_privilege_overrides()?;
    guard.insert(uri.to_string(), privileged);
    debug!(script = %uri, privileged, "Stored privilege override in memory");

    Ok(())
}

/// Insert log message with error handling
pub fn insert_log_message(script_uri: &str, message: &str, log_level: &str) {
    let repo = get_repository();
    let result = run_blocking(async { repo.insert_log(script_uri, message, log_level).await });

    if let Err(e) = result {
        error!(
            "Failed to insert log message for {}: {}. Message: {}",
            script_uri, e, message
        );
        // Log to system instead as fallback
        error!("FALLBACK LOG [{}]: {}", script_uri, message);
    }
}

/// Fetch log messages with error handling
pub fn fetch_log_messages(script_uri: &str) -> Vec<LogEntry> {
    let repo = get_repository();
    let result = run_blocking(async { repo.fetch_logs(script_uri).await });

    match result {
        Ok(messages) => messages,
        Err(e) => {
            error!("Failed to fetch log messages for {}: {}", script_uri, e);
            let now = SystemTime::now();
            vec![LogEntry::new(
                format!("Error: Could not retrieve logs - {}", e),
                "ERROR".to_string(),
                now,
            )]
        }
    }
}

/// Fetch ALL log messages from all script URIs
pub fn fetch_all_log_messages() -> Vec<LogEntry> {
    // Try database first if configured
    let repo = get_repository();
    let result = run_blocking(async { repo.fetch_all_logs().await });

    match result {
        Ok(messages) => messages,
        Err(e) => {
            error!("Failed to fetch all log messages: {}", e);
            vec![LogEntry::new(
                format!("Error: Could not retrieve logs - {}", e),
                "ERROR".to_string(),
                SystemTime::now(),
            )]
        }
    }
}

/// Clear log messages for a script
pub fn clear_log_messages(script_uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_blocking(async { repo.clear_logs(script_uri).await })
}

/// Keep only the latest `limit` log messages (default 20) for each script URI and remove older ones
pub fn prune_log_messages() -> AppResult<()> {
    let repo = get_repository();
    run_blocking(async { repo.prune_logs().await })
}

/// Upsert script with error handling
pub fn upsert_script(uri: &str, content: &str) -> AppResult<()> {
    if uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("URI cannot be empty".to_string()).into());
    }

    if content.len() > 1_000_000 {
        // 1MB limit
        return Err(
            RepositoryError::InvalidData("Script content too large (>1MB)".to_string()).into(),
        );
    }

    let repo = get_repository();

    run_blocking(async { repo.upsert_script(uri, content).await })
}

/// In-memory implementation of upsert script (existing logic)
fn upsert_script_in_memory(uri: &str, content: &str) -> AppResult<()> {
    let mut guard = safe_lock_scripts()?;

    // Check if script already exists
    if let Some(existing) = guard.get_mut(uri) {
        // Update existing script
        existing.update_content(content.to_string());
        debug!(
            "Updated script in memory: {} ({} bytes)",
            uri,
            content.len()
        );
    } else {
        // Insert new script
        let metadata = ScriptMetadata::new(uri.to_string(), content.to_string());
        guard.insert(uri.to_string(), metadata);
        debug!(
            "Created new script in memory: {} ({} bytes)",
            uri,
            content.len()
        );
    }

    Ok(())
}

/// In-memory implementation of upsert asset
fn upsert_asset_in_memory(asset: &Asset) -> AppResult<()> {
    let mut guard = safe_lock_assets()?;
    guard.insert(asset.uri.clone(), asset.clone());
    debug!("Upserted asset in memory: {}", asset.uri);
    Ok(())
}

/// Bootstrap hardcoded scripts into database on startup
pub fn bootstrap_scripts() -> AppResult<()> {
    if let Some(db) = get_db_pool() {
        let pool = db.pool();

        // Define the hardcoded scripts
        let hardcoded_scripts = vec![
            (
                "https://example.com/core",
                include_str!("../scripts/feature_scripts/core.js"),
            ),
            (
                "https://example.com/cli",
                include_str!("../scripts/feature_scripts/cli.js"),
            ),
            (
                "https://example.com/editor",
                include_str!("../scripts/feature_scripts/editor.js"),
            ),
            (
                "https://example.com/admin",
                include_str!("../scripts/feature_scripts/admin.js"),
            ),
            (
                "https://example.com/auth",
                include_str!("../scripts/feature_scripts/auth.js"),
            ),
        ];

        // Include test scripts when appropriate
        let mut all_scripts = hardcoded_scripts;
        let include_test_scripts =
            std::env::var("AIWEBENGINE_INCLUDE_TEST_SCRIPTS").is_ok() || cfg!(test);

        if include_test_scripts {
            all_scripts.push((
                "https://example.com/graphql_test",
                include_str!("../scripts/test_scripts/graphql_test.js"),
            ));
        }

        let result = run_blocking(async {
            for (uri, code) in all_scripts {
                // Check if script already exists
                if let Ok(Some(_)) = db_get_script(pool, uri).await {
                    debug!("Script already exists in database: {}", uri);
                    continue;
                }

                // Insert the script
                if let Err(e) = db_upsert_script(pool, uri, code).await {
                    error!("Failed to bootstrap script {}: {}", uri, e);
                    return Err(e);
                } else {
                    info!("Bootstrapped script into database: {}", uri);
                    if let Err(e) = db_set_script_privileged(pool, uri, true).await {
                        warn!(
                            "Failed to flag bootstrapped script {} as privileged: {}",
                            uri, e
                        );
                    }
                }
            }
            Ok(())
        });

        match result {
            Ok(()) => {
                info!("Successfully bootstrapped scripts into database");
                Ok(())
            }
            Err(e) => {
                error!("Failed to bootstrap scripts: {}", e);
                Err(e)
            }
        }
    } else {
        debug!("Database not configured, skipping script bootstrap");
        Ok(())
    }
}

/// Bootstrap hardcoded assets into database on startup
pub fn bootstrap_assets() -> AppResult<()> {
    run_blocking(async { bootstrap_assets_async().await })
}

/// Async version of bootstrap_assets
pub async fn bootstrap_assets_async() -> AppResult<()> {
    let repo = get_repository();
    let assets = get_static_assets();
    info!("Bootstrapping {} assets...", assets.len());

    for (uri, asset) in assets {
        debug!("Bootstrapping asset: {}", uri);
        if let Err(e) = repo.upsert_asset(asset).await {
            error!("Failed to bootstrap asset {}: {}", uri, e);
            return Err(e);
        }
    }

    Ok(())
}

/// Delete script with error handling
pub fn delete_script(uri: &str) -> bool {
    let repo = get_repository();

    let result = run_blocking(async { repo.delete_script(uri).await });

    match result {
        Ok(existed) => {
            if existed {
                scheduler::clear_script_jobs(uri);
                debug!("Deleted script from repository: {}", uri);
            } else {
                debug!("Script not found in repository for deletion: {}", uri);
            }
            existed
        }
        Err(e) => {
            error!("Failed to delete script {}: {}", uri, e);
            false
        }
    }
}

/// Helper function to get static assets embedded at compile time
fn get_static_assets() -> HashMap<String, Asset> {
    let mut m = HashMap::new();
    let now = std::time::SystemTime::now();

    // Logo asset
    let logo_content = include_bytes!("../assets/logo.svg").to_vec();
    let logo = Asset {
        uri: "logo.svg".to_string(),
        name: Some("Logo".to_string()),
        mimetype: "image/svg+xml".to_string(),
        content: logo_content,
        created_at: now,
        updated_at: now,
    };
    m.insert("logo.svg".to_string(), logo);

    // Editor assets
    // Note: editor.html is NOT registered as a public asset
    // It's served exclusively through the /editor route in editor.js
    // This simplifies the API surface and provides a single entry point

    let editor_css_content = include_bytes!("../assets/editor.css").to_vec();
    let editor_css = Asset {
        uri: "editor.css".to_string(),
        name: Some("Editor Styles".to_string()),
        mimetype: "text/css".to_string(),
        content: editor_css_content,
        created_at: now,
        updated_at: now,
    };
    m.insert("editor.css".to_string(), editor_css);

    let engine_css_content = include_bytes!("../assets/engine.css").to_vec();
    let engine_css = Asset {
        uri: "engine.css".to_string(),
        name: Some("Engine Styles".to_string()),
        mimetype: "text/css".to_string(),
        content: engine_css_content,
        created_at: now,
        updated_at: now,
    };
    m.insert("engine.css".to_string(), engine_css);

    let editor_js_content = include_bytes!("../assets/editor.js").to_vec();
    let editor_js = Asset {
        uri: "editor.js".to_string(),
        name: Some("Editor Script".to_string()),
        mimetype: "application/javascript".to_string(),
        content: editor_js_content,
        created_at: now,
        updated_at: now,
    };
    m.insert("editor.js".to_string(), editor_js);

    let favicon_content = include_bytes!("../assets/favicon.ico").to_vec();
    let favicon = Asset {
        uri: "favicon.ico".to_string(),
        name: Some("Favicon".to_string()),
        mimetype: "image/x-icon".to_string(),
        content: favicon_content,
        created_at: now,
        updated_at: now,
    };
    m.insert("favicon.ico".to_string(), favicon);

    m
}

/// Helper function to get static scripts embedded at compile time
fn get_static_scripts() -> HashMap<String, String> {
    let mut m = HashMap::new();

    let core = include_str!("../scripts/feature_scripts/core.js");
    let cli = include_str!("../scripts/feature_scripts/cli.js");
    let editor = include_str!("../scripts/feature_scripts/editor.js");
    let admin = include_str!("../scripts/feature_scripts/admin.js");
    let auth = include_str!("../scripts/feature_scripts/auth.js");

    m.insert("https://example.com/core".to_string(), core.to_string());
    m.insert("https://example.com/cli".to_string(), cli.to_string());
    m.insert("https://example.com/editor".to_string(), editor.to_string());
    m.insert("https://example.com/admin".to_string(), admin.to_string());
    m.insert("https://example.com/auth".to_string(), auth.to_string());

    // Include test scripts when appropriate
    let include_test_scripts =
        std::env::var("AIWEBENGINE_INCLUDE_TEST_SCRIPTS").is_ok() || cfg!(test);

    if include_test_scripts {
        let graphql_test = include_str!("../scripts/test_scripts/graphql_test.js");
        m.insert(
            "https://example.com/graphql_test".to_string(),
            graphql_test.to_string(),
        );
    }

    m
}

/// Fetch assets with error handling (static + dynamic)
pub fn fetch_assets() -> HashMap<String, Asset> {
    let repo = get_repository();

    let result = run_blocking(async { repo.list_assets().await });

    match result {
        Ok(assets) => {
            debug!("Loaded {} assets from repository", assets.len());
            assets
        }
        Err(e) => {
            error!("Failed to fetch assets: {}", e);
            HashMap::new()
        }
    }
}

/// Fetch single asset by URI with error handling (dynamic first, then static)
pub fn fetch_asset(uri: &str) -> Option<Asset> {
    let repo = get_repository();

    // Try repository first (DB or Memory)
    let result = run_blocking(async { repo.get_asset(uri).await });

    match result {
        Ok(Some(asset)) => {
            debug!("Loaded asset from repository: {}", uri);
            return Some(asset);
        }
        Ok(None) => {
            // Not in repository, check static assets
        }
        Err(e) => {
            warn!("Repository asset fetch failed for {}: {}", uri, e);
            // Fall through to static assets
        }
    }

    // Check static assets
    if let Some(asset) = get_static_assets().get(uri) {
        return Some(asset.clone());
    }

    None
}

/// Upsert asset with validation and error handling
pub fn upsert_asset(asset: Asset) -> AppResult<()> {
    if asset.uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Asset URI cannot be empty".to_string()).into());
    }

    if asset.content.len() > 10_000_000 {
        // 10MB limit for assets
        return Err(
            RepositoryError::InvalidData("Asset content too large (>10MB)".to_string()).into(),
        );
    }

    if asset.mimetype.trim().is_empty() {
        return Err(RepositoryError::InvalidData("MIME type cannot be empty".to_string()).into());
    }

    let repo = get_repository();
    run_blocking(async { repo.upsert_asset(asset).await })
}

/// Delete asset with error handling  
pub fn delete_asset(uri: &str) -> bool {
    let repo = get_repository();
    let result = run_blocking(async { repo.delete_asset(uri).await });

    match result {
        Ok(existed) => existed,
        Err(e) => {
            error!("Failed to delete asset {}: {}", uri, e);
            false
        }
    }
}

/// Get repository statistics for monitoring
pub fn get_repository_stats() -> HashMap<String, usize> {
    let mut stats = HashMap::new();

    // Count scripts
    match safe_lock_scripts() {
        Ok(guard) => {
            stats.insert("dynamic_scripts".to_string(), guard.len());
        }
        Err(_) => {
            stats.insert("dynamic_scripts".to_string(), 0);
        }
    }

    // Count assets
    match safe_lock_assets() {
        Ok(guard) => {
            stats.insert("assets".to_string(), guard.len());
        }
        Err(_) => {
            stats.insert("assets".to_string(), 0);
        }
    }

    // Count total log entries
    match safe_lock_logs() {
        Ok(guard) => {
            let total_logs: usize = guard.values().map(|v: &Vec<String>| v.len()).sum();
            stats.insert("log_entries".to_string(), total_logs);
        }
        Err(_) => {
            stats.insert("log_entries".to_string(), 0);
        }
    }

    // Count shared storage entries
    match safe_lock_shared_storage() {
        Ok(guard) => {
            let total_entries: usize = guard.values().map(|script_map| script_map.len()).sum();
            stats.insert("shared_storage_entries".to_string(), total_entries);
        }
        Err(_) => {
            stats.insert("shared_storage_entries".to_string(), 0);
        }
    }

    stats
}

/// Set a shared storage item (key-value pair for a specific script)
pub fn set_shared_storage_item(script_uri: &str, key: &str, value: &str) -> AppResult<()> {
    if script_uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Script URI cannot be empty".to_string()).into());
    }

    if key.trim().is_empty() {
        return Err(RepositoryError::InvalidData("Key cannot be empty".to_string()).into());
    }

    if value.len() > 1_000_000 {
        // 1MB limit per value
        return Err(RepositoryError::InvalidData("Value too large (>1MB)".to_string()).into());
    }

    let repo = get_repository();
    run_blocking(async { repo.set_shared_storage(script_uri, key, value).await })
}

/// Get a shared storage item
pub fn get_shared_storage_item(script_uri: &str, key: &str) -> Option<String> {
    let repo = get_repository();
    let result = run_blocking(async { repo.get_shared_storage(script_uri, key).await });

    match result {
        Ok(value) => value,
        Err(e) => {
            error!(
                "Failed to get shared storage item {}:{}: {}",
                script_uri, key, e
            );
            None
        }
    }
}

/// Remove a shared storage item
pub fn remove_shared_storage_item(script_uri: &str, key: &str) -> bool {
    let repo = get_repository();
    let result = run_blocking(async { repo.remove_shared_storage(script_uri, key).await });

    match result {
        Ok(existed) => existed,
        Err(e) => {
            error!(
                "Failed to remove shared storage item {}:{}: {}",
                script_uri, key, e
            );
            false
        }
    }
}

/// Clear all shared storage items for a specific script
pub fn clear_shared_storage(script_uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_blocking(async { repo.clear_shared_storage(script_uri).await })
}

use async_trait::async_trait;

/// Abstract repository interface
#[async_trait]
pub trait Repository: Send + Sync {
    // Script operations
    async fn get_script(&self, uri: &str) -> AppResult<Option<String>>;
    async fn list_scripts(&self) -> AppResult<HashMap<String, String>>;
    async fn upsert_script(&self, uri: &str, content: &str) -> AppResult<()>;
    async fn delete_script(&self, uri: &str) -> AppResult<bool>;
    async fn get_script_metadata(&self, uri: &str) -> AppResult<ScriptMetadata>;
    async fn get_all_script_metadata(&self) -> AppResult<Vec<ScriptMetadata>>;
    async fn update_script_init_status(
        &self,
        uri: &str,
        initialized: bool,
        init_error: Option<String>,
        registrations: Option<RouteRegistrations>,
    ) -> AppResult<()>;

    // Asset operations
    async fn get_asset(&self, uri: &str) -> AppResult<Option<Asset>>;
    async fn list_assets(&self) -> AppResult<HashMap<String, Asset>>;
    async fn upsert_asset(&self, asset: Asset) -> AppResult<()>;
    async fn delete_asset(&self, uri: &str) -> AppResult<bool>;

    // Log operations
    async fn insert_log(&self, script_uri: &str, message: &str, level: &str) -> AppResult<()>;
    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>>;
    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>>;
    async fn clear_logs(&self, script_uri: &str) -> AppResult<()>;
    async fn prune_logs(&self) -> AppResult<()>;

    // Shared storage operations
    async fn get_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<Option<String>>;
    async fn set_shared_storage(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()>;
    async fn remove_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<bool>;
    async fn clear_shared_storage(&self, script_uri: &str) -> AppResult<()>;

    // Security operations
    async fn get_script_privileged(&self, uri: &str) -> AppResult<Option<bool>>;
    async fn set_script_privileged(&self, uri: &str, privileged: bool) -> AppResult<()>;
}

/// PostgreSQL implementation of the Repository trait
pub struct PostgresRepository {
    pool: PgPool,
}

impl PostgresRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl Repository for PostgresRepository {
    async fn get_script(&self, uri: &str) -> AppResult<Option<String>> {
        db_get_script(&self.pool, uri).await
    }

    async fn list_scripts(&self) -> AppResult<HashMap<String, String>> {
        db_list_scripts(&self.pool).await
    }

    async fn upsert_script(&self, uri: &str, content: &str) -> AppResult<()> {
        db_upsert_script(&self.pool, uri, content).await?;
        // Invalidate cache
        if let Ok(mut guard) = safe_lock_scripts() {
            guard.remove(uri);
        }
        Ok(())
    }

    async fn delete_script(&self, uri: &str) -> AppResult<bool> {
        let result = db_delete_script(&self.pool, uri).await?;
        if result && let Ok(mut guard) = safe_lock_scripts() {
            guard.remove(uri);
        }
        Ok(result)
    }

    async fn get_script_metadata(&self, uri: &str) -> AppResult<ScriptMetadata> {
        // Check cache first
        if let Ok(guard) = safe_lock_scripts()
            && let Some(metadata) = guard.get(uri)
        {
            return Ok(metadata.clone());
        }

        // Fetch from DB
        let content = self
            .get_script(uri)
            .await?
            .ok_or_else(|| RepositoryError::ScriptNotFound(uri.to_string()))?;

        let privileged = self
            .get_script_privileged(uri)
            .await?
            .unwrap_or(default_privileged_for(uri));

        let mut metadata = ScriptMetadata::new(uri.to_string(), content);
        metadata.privileged = privileged;

        // Cache it
        if let Ok(mut guard) = safe_lock_scripts() {
            guard.insert(uri.to_string(), metadata.clone());
        }

        Ok(metadata)
    }

    async fn get_all_script_metadata(&self) -> AppResult<Vec<ScriptMetadata>> {
        let mut all_scripts = get_static_scripts();

        // Fetch scripts from database and merge (DB overrides static)
        let db_scripts = self.list_scripts().await?;
        for (uri, content) in db_scripts {
            all_scripts.insert(uri, content);
        }

        let mut metadata_list = Vec::new();

        // Scope for mutex lock
        {
            let mut guard = safe_lock_scripts()?;
            for (uri, content) in all_scripts {
                if let Some(cached) = guard.get(&uri) {
                    // Use cached version to preserve runtime state
                    metadata_list.push(cached.clone());
                } else {
                    // Create new metadata and cache it
                    let metadata = ScriptMetadata::new(uri.clone(), content);
                    guard.insert(uri.clone(), metadata.clone());
                    metadata_list.push(metadata);
                }
            }
        }

        // Apply privilege overrides
        if let Ok(overrides) = safe_lock_privilege_overrides() {
            for metadata in &mut metadata_list {
                if let Some(privileged) = overrides.get(&metadata.uri) {
                    metadata.privileged = *privileged;
                }
            }
        }

        // Apply default privileges
        for metadata in &mut metadata_list {
            let profile = get_script_security_profile(&metadata.uri)?;
            if profile.privileged != metadata.privileged {
                metadata.privileged = profile.privileged;
            }
        }

        Ok(metadata_list)
    }

    async fn update_script_init_status(
        &self,
        uri: &str,
        initialized: bool,
        init_error: Option<String>,
        registrations: Option<RouteRegistrations>,
    ) -> AppResult<()> {
        let mut guard = safe_lock_scripts()?;
        if let Some(metadata) = guard.get_mut(uri) {
            metadata.initialized = initialized;
            metadata.init_error = init_error;
            if let Some(regs) = registrations {
                metadata.registrations = regs;
            }
            if initialized {
                metadata.last_init_time = Some(SystemTime::now());
            }
        } else {
            // If it's a static script, it might not be in dynamic scripts map yet
            if let Some(content) = get_static_scripts().get(uri) {
                let mut metadata = ScriptMetadata::new(uri.to_string(), content.clone());
                metadata.initialized = initialized;
                metadata.init_error = init_error;
                if let Some(regs) = registrations {
                    metadata.registrations = regs;
                }
                if initialized {
                    metadata.last_init_time = Some(SystemTime::now());
                }
                guard.insert(uri.to_string(), metadata);
            } else {
                return Err(RepositoryError::ScriptNotFound(uri.to_string()).into());
            }
        }
        Ok(())
    }

    async fn get_asset(&self, uri: &str) -> AppResult<Option<Asset>> {
        db_get_asset(&self.pool, uri).await
    }

    async fn list_assets(&self) -> AppResult<HashMap<String, Asset>> {
        db_list_assets(&self.pool).await
    }

    async fn upsert_asset(&self, asset: Asset) -> AppResult<()> {
        db_upsert_asset(&self.pool, &asset).await
    }

    async fn delete_asset(&self, uri: &str) -> AppResult<bool> {
        db_delete_asset(&self.pool, uri).await
    }

    async fn insert_log(&self, script_uri: &str, message: &str, level: &str) -> AppResult<()> {
        db_insert_log_message(&self.pool, script_uri, message, level).await
    }

    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>> {
        db_fetch_log_messages(&self.pool, script_uri).await
    }

    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>> {
        db_fetch_all_log_messages(&self.pool).await
    }

    async fn clear_logs(&self, script_uri: &str) -> AppResult<()> {
        db_clear_log_messages(&self.pool, script_uri).await
    }

    async fn prune_logs(&self) -> AppResult<()> {
        db_prune_log_messages(&self.pool).await
    }

    async fn get_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<Option<String>> {
        db_get_shared_storage_item(&self.pool, script_uri, key).await
    }

    async fn set_shared_storage(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()> {
        db_set_shared_storage_item(&self.pool, script_uri, key, value).await
    }

    async fn remove_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<bool> {
        db_remove_shared_storage_item(&self.pool, script_uri, key).await
    }

    async fn clear_shared_storage(&self, script_uri: &str) -> AppResult<()> {
        db_clear_shared_storage(&self.pool, script_uri).await
    }

    async fn get_script_privileged(&self, uri: &str) -> AppResult<Option<bool>> {
        db_get_script_privileged(&self.pool, uri).await
    }

    async fn set_script_privileged(&self, uri: &str, privileged: bool) -> AppResult<()> {
        db_set_script_privileged(&self.pool, uri, privileged).await
    }
}

/// In-memory implementation of the Repository trait
pub struct MemoryRepository;

impl MemoryRepository {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MemoryRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repository for MemoryRepository {
    async fn get_script(&self, uri: &str) -> AppResult<Option<String>> {
        // Check static scripts first
        if let Some(script) = get_static_scripts().get(uri) {
            return Ok(Some(script.clone()));
        }

        // Then check dynamic scripts
        let guard = safe_lock_scripts()?;
        Ok(guard.get(uri).map(|m| m.content.clone()))
    }

    async fn list_scripts(&self) -> AppResult<HashMap<String, String>> {
        // Start with static scripts
        let mut scripts = get_static_scripts();

        // Merge dynamic scripts
        let guard = safe_lock_scripts()?;
        for (uri, metadata) in guard.iter() {
            scripts.insert(uri.clone(), metadata.content.clone());
        }

        Ok(scripts)
    }

    async fn upsert_script(&self, uri: &str, content: &str) -> AppResult<()> {
        upsert_script_in_memory(uri, content)
    }

    async fn delete_script(&self, uri: &str) -> AppResult<bool> {
        let mut guard = safe_lock_scripts()?;
        Ok(guard.remove(uri).is_some())
    }

    async fn get_script_metadata(&self, uri: &str) -> AppResult<ScriptMetadata> {
        let guard = safe_lock_scripts()?;
        guard
            .get(uri)
            .cloned()
            .ok_or_else(|| RepositoryError::ScriptNotFound(uri.to_string()).into())
    }

    async fn get_all_script_metadata(&self) -> AppResult<Vec<ScriptMetadata>> {
        let mut metadata_list = Vec::new();

        // Get static scripts
        for (uri, content) in get_static_scripts() {
            metadata_list.push(ScriptMetadata::new(uri, content));
        }

        // Merge dynamic scripts (overwriting static if same URI)
        {
            let guard = safe_lock_scripts()?;
            for (uri, metadata) in guard.iter() {
                if let Some(existing) = metadata_list.iter_mut().find(|m| m.uri == *uri) {
                    *existing = metadata.clone();
                } else {
                    metadata_list.push(metadata.clone());
                }
            }
        }

        // Apply privilege overrides
        if let Ok(overrides) = safe_lock_privilege_overrides() {
            for metadata in &mut metadata_list {
                if let Some(privileged) = overrides.get(&metadata.uri) {
                    metadata.privileged = *privileged;
                }
            }
        }

        // Apply default privileges
        for metadata in &mut metadata_list {
            let profile = get_script_security_profile(&metadata.uri)?;
            if profile.privileged != metadata.privileged {
                metadata.privileged = profile.privileged;
            }
        }

        Ok(metadata_list)
    }

    async fn update_script_init_status(
        &self,
        uri: &str,
        initialized: bool,
        init_error: Option<String>,
        registrations: Option<RouteRegistrations>,
    ) -> AppResult<()> {
        let mut guard = safe_lock_scripts()?;
        if let Some(metadata) = guard.get_mut(uri) {
            metadata.initialized = initialized;
            metadata.init_error = init_error;
            if let Some(regs) = registrations {
                metadata.registrations = regs;
            }
            if initialized {
                metadata.last_init_time = Some(SystemTime::now());
            }
        } else {
            // If it's a static script, it might not be in dynamic scripts map yet
            if let Some(content) = get_static_scripts().get(uri) {
                let mut metadata = ScriptMetadata::new(uri.to_string(), content.clone());
                metadata.initialized = initialized;
                metadata.init_error = init_error;
                if let Some(regs) = registrations {
                    metadata.registrations = regs;
                }
                if initialized {
                    metadata.last_init_time = Some(SystemTime::now());
                }
                guard.insert(uri.to_string(), metadata);
            } else {
                return Err(RepositoryError::ScriptNotFound(uri.to_string()).into());
            }
        }
        Ok(())
    }

    async fn get_asset(&self, uri: &str) -> AppResult<Option<Asset>> {
        // Check static assets first
        if let Some(asset) = get_static_assets().get(uri) {
            return Ok(Some(asset.clone()));
        }

        // Then check dynamic assets
        let guard = safe_lock_assets()?;
        Ok(guard.get(uri).cloned())
    }

    async fn list_assets(&self) -> AppResult<HashMap<String, Asset>> {
        let mut assets = get_static_assets();

        let guard = safe_lock_assets()?;
        for (uri, asset) in guard.iter() {
            assets.insert(uri.clone(), asset.clone());
        }

        Ok(assets)
    }

    async fn upsert_asset(&self, asset: Asset) -> AppResult<()> {
        upsert_asset_in_memory(&asset)
    }

    async fn delete_asset(&self, uri: &str) -> AppResult<bool> {
        let mut guard = safe_lock_assets()?;
        Ok(guard.remove(uri).is_some())
    }

    async fn insert_log(&self, script_uri: &str, message: &str, _level: &str) -> AppResult<()> {
        let mut guard = safe_lock_logs()?;
        guard
            .entry(script_uri.to_string())
            .or_insert_with(Vec::new)
            .push(message.to_string());
        Ok(())
    }

    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>> {
        let guard = safe_lock_logs()?;
        let now = SystemTime::now();
        Ok(guard
            .get(script_uri)
            .map(|messages| {
                messages
                    .iter()
                    .map(|msg| LogEntry::new(msg.clone(), "INFO".to_string(), now))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>> {
        let guard = safe_lock_logs()?;
        let mut all_logs = Vec::new();
        let now = SystemTime::now();
        for logs in guard.values() {
            for message in logs {
                all_logs.push(LogEntry::new(message.clone(), "INFO".to_string(), now));
            }
        }
        Ok(all_logs)
    }

    async fn clear_logs(&self, script_uri: &str) -> AppResult<()> {
        let mut guard = safe_lock_logs()?;
        guard.remove(script_uri);
        Ok(())
    }

    async fn prune_logs(&self) -> AppResult<()> {
        const LIMIT: usize = 20;
        let mut guard = safe_lock_logs()?;

        for logs in guard.values_mut() {
            if logs.len() > LIMIT {
                let remove = logs.len() - LIMIT;
                logs.drain(0..remove);
            }
        }
        Ok(())
    }

    async fn get_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<Option<String>> {
        let guard = safe_lock_shared_storage()?;
        Ok(guard.get(script_uri).and_then(|map| map.get(key)).cloned())
    }

    async fn set_shared_storage(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()> {
        let mut guard = safe_lock_shared_storage()?;
        guard
            .entry(script_uri.to_string())
            .or_insert_with(HashMap::new)
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn remove_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<bool> {
        let mut guard = safe_lock_shared_storage()?;
        if let Some(map) = guard.get_mut(script_uri) {
            Ok(map.remove(key).is_some())
        } else {
            Ok(false)
        }
    }

    async fn clear_shared_storage(&self, script_uri: &str) -> AppResult<()> {
        let mut guard = safe_lock_shared_storage()?;
        guard.remove(script_uri);
        Ok(())
    }

    async fn get_script_privileged(&self, uri: &str) -> AppResult<Option<bool>> {
        // Check overrides
        if let Ok(guard) = safe_lock_privilege_overrides()
            && let Some(value) = guard.get(uri)
        {
            return Ok(Some(*value));
        }

        // Check metadata
        if let Ok(guard) = safe_lock_scripts()
            && let Some(metadata) = guard.get(uri)
        {
            return Ok(Some(metadata.privileged));
        }

        Ok(None)
    }

    async fn set_script_privileged(&self, uri: &str, privileged: bool) -> AppResult<()> {
        // Update metadata if exists
        if let Ok(mut guard) = safe_lock_scripts()
            && let Some(metadata) = guard.get_mut(uri)
        {
            metadata.privileged = privileged;
        }

        // Update override
        let mut guard = safe_lock_privilege_overrides()?;
        guard.insert(uri.to_string(), privileged);
        Ok(())
    }
}

/// Unified repository that delegates to either Postgres or Memory implementation
pub enum UnifiedRepository {
    Postgres(PostgresRepository),
    Memory(MemoryRepository),
}

impl UnifiedRepository {
    pub fn new_postgres(pool: PgPool) -> Self {
        Self::Postgres(PostgresRepository::new(pool))
    }

    pub fn new_memory() -> Self {
        Self::Memory(MemoryRepository::new())
    }
}

#[async_trait]
impl Repository for UnifiedRepository {
    async fn get_script(&self, uri: &str) -> AppResult<Option<String>> {
        match self {
            Self::Postgres(repo) => repo.get_script(uri).await,
            Self::Memory(repo) => repo.get_script(uri).await,
        }
    }

    async fn list_scripts(&self) -> AppResult<HashMap<String, String>> {
        match self {
            Self::Postgres(repo) => repo.list_scripts().await,
            Self::Memory(repo) => repo.list_scripts().await,
        }
    }

    async fn upsert_script(&self, uri: &str, content: &str) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.upsert_script(uri, content).await,
            Self::Memory(repo) => repo.upsert_script(uri, content).await,
        }
    }

    async fn delete_script(&self, uri: &str) -> AppResult<bool> {
        match self {
            Self::Postgres(repo) => repo.delete_script(uri).await,
            Self::Memory(repo) => repo.delete_script(uri).await,
        }
    }

    async fn get_script_metadata(&self, uri: &str) -> AppResult<ScriptMetadata> {
        match self {
            Self::Postgres(repo) => repo.get_script_metadata(uri).await,
            Self::Memory(repo) => repo.get_script_metadata(uri).await,
        }
    }

    async fn get_all_script_metadata(&self) -> AppResult<Vec<ScriptMetadata>> {
        match self {
            Self::Postgres(repo) => repo.get_all_script_metadata().await,
            Self::Memory(repo) => repo.get_all_script_metadata().await,
        }
    }

    async fn update_script_init_status(
        &self,
        uri: &str,
        initialized: bool,
        init_error: Option<String>,
        registrations: Option<RouteRegistrations>,
    ) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => {
                repo.update_script_init_status(uri, initialized, init_error, registrations)
                    .await
            }
            Self::Memory(repo) => {
                repo.update_script_init_status(uri, initialized, init_error, registrations)
                    .await
            }
        }
    }

    async fn get_asset(&self, uri: &str) -> AppResult<Option<Asset>> {
        match self {
            Self::Postgres(repo) => repo.get_asset(uri).await,
            Self::Memory(repo) => repo.get_asset(uri).await,
        }
    }

    async fn list_assets(&self) -> AppResult<HashMap<String, Asset>> {
        match self {
            Self::Postgres(repo) => repo.list_assets().await,
            Self::Memory(repo) => repo.list_assets().await,
        }
    }

    async fn upsert_asset(&self, asset: Asset) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.upsert_asset(asset).await,
            Self::Memory(repo) => repo.upsert_asset(asset).await,
        }
    }

    async fn delete_asset(&self, uri: &str) -> AppResult<bool> {
        match self {
            Self::Postgres(repo) => repo.delete_asset(uri).await,
            Self::Memory(repo) => repo.delete_asset(uri).await,
        }
    }

    async fn insert_log(&self, script_uri: &str, message: &str, level: &str) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.insert_log(script_uri, message, level).await,
            Self::Memory(repo) => repo.insert_log(script_uri, message, level).await,
        }
    }

    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>> {
        match self {
            Self::Postgres(repo) => repo.fetch_logs(script_uri).await,
            Self::Memory(repo) => repo.fetch_logs(script_uri).await,
        }
    }

    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>> {
        match self {
            Self::Postgres(repo) => repo.fetch_all_logs().await,
            Self::Memory(repo) => repo.fetch_all_logs().await,
        }
    }

    async fn clear_logs(&self, script_uri: &str) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.clear_logs(script_uri).await,
            Self::Memory(repo) => repo.clear_logs(script_uri).await,
        }
    }

    async fn prune_logs(&self) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.prune_logs().await,
            Self::Memory(repo) => repo.prune_logs().await,
        }
    }

    async fn get_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<Option<String>> {
        match self {
            Self::Postgres(repo) => repo.get_shared_storage(script_uri, key).await,
            Self::Memory(repo) => repo.get_shared_storage(script_uri, key).await,
        }
    }

    async fn set_shared_storage(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.set_shared_storage(script_uri, key, value).await,
            Self::Memory(repo) => repo.set_shared_storage(script_uri, key, value).await,
        }
    }

    async fn remove_shared_storage(&self, script_uri: &str, key: &str) -> AppResult<bool> {
        match self {
            Self::Postgres(repo) => repo.remove_shared_storage(script_uri, key).await,
            Self::Memory(repo) => repo.remove_shared_storage(script_uri, key).await,
        }
    }

    async fn clear_shared_storage(&self, script_uri: &str) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.clear_shared_storage(script_uri).await,
            Self::Memory(repo) => repo.clear_shared_storage(script_uri).await,
        }
    }

    async fn get_script_privileged(&self, uri: &str) -> AppResult<Option<bool>> {
        match self {
            Self::Postgres(repo) => repo.get_script_privileged(uri).await,
            Self::Memory(repo) => repo.get_script_privileged(uri).await,
        }
    }

    async fn set_script_privileged(&self, uri: &str, privileged: bool) -> AppResult<()> {
        match self {
            Self::Postgres(repo) => repo.set_script_privileged(uri, privileged).await,
            Self::Memory(repo) => repo.set_script_privileged(uri, privileged).await,
        }
    }
}

/// Global repository instance
static GLOBAL_REPOSITORY: OnceLock<UnifiedRepository> = OnceLock::new();

/// Initialize the global repository
pub fn initialize_repository(repo: UnifiedRepository) -> bool {
    GLOBAL_REPOSITORY.set(repo).is_ok()
}

/// Get the global repository
pub fn get_repository() -> &'static UnifiedRepository {
    GLOBAL_REPOSITORY.get_or_init(UnifiedRepository::new_memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_operations() {
        let uri = "test://example";
        let content = "console.log('test')";

        // Test upsert
        assert!(upsert_script(uri, content).is_ok());

        // Test fetch
        let fetched = fetch_script(uri);
        assert_eq!(fetched, Some(content.to_string()));

        // Test delete
        assert!(delete_script(uri));
        assert!(!delete_script(uri)); // Should return false for non-existent
    }

    #[test]
    fn test_bootstrap_scripts() {
        // Test that bootstrap_scripts doesn't crash when no database is configured
        let result = bootstrap_scripts();
        assert!(
            result.is_ok(),
            "bootstrap_scripts should succeed even without database"
        );
    }

    #[test]
    fn test_bootstrap_assets() {
        // Test that bootstrap_assets doesn't crash when no database is configured
        let result = bootstrap_assets();
        assert!(
            result.is_ok(),
            "bootstrap_assets should succeed even without database"
        );
    }

    #[test]
    fn test_shared_storage_operations() {
        let script_uri = "test://storage-script";
        let key = "test_key";
        let value = "test_value";

        // Test set item
        assert!(set_shared_storage_item(script_uri, key, value).is_ok());

        // Test get item
        let retrieved = get_shared_storage_item(script_uri, key);
        assert_eq!(retrieved, Some(value.to_string()));

        // Test remove item
        assert!(remove_shared_storage_item(script_uri, key));

        // Verify item is gone
        let retrieved_after_remove = get_shared_storage_item(script_uri, key);
        assert_eq!(retrieved_after_remove, None);

        // Test clear storage
        assert!(set_shared_storage_item(script_uri, "key1", "value1").is_ok());
        assert!(set_shared_storage_item(script_uri, "key2", "value2").is_ok());

        assert!(clear_shared_storage(script_uri).is_ok());

        // Verify both items are gone
        assert_eq!(get_shared_storage_item(script_uri, "key1"), None);
        assert_eq!(get_shared_storage_item(script_uri, "key2"), None);
    }

    #[test]
    fn test_shared_storage_validation() {
        // Test empty script URI
        assert!(set_shared_storage_item("", "key", "value").is_err());

        // Test empty key
        assert!(set_shared_storage_item("test://script", "", "value").is_err());

        // Test oversized value (simulate by creating a large string)
        let large_value = "x".repeat(1_000_001); // Just over 1MB
        assert!(set_shared_storage_item("test://script", "key", &large_value).is_err());
    }

    #[test]
    fn test_script_privileged_flag_defaults() {
        let bootstrap_uri = "https://example.com/core";
        let bootstrap_profile =
            get_script_security_profile(bootstrap_uri).expect("bootstrap profile available");
        assert!(
            bootstrap_profile.privileged,
            "Bootstrap scripts should default to privileged"
        );

        let custom_uri = "test://privileged-script";
        let code = "function handler() { return { status: 200 }; }";
        upsert_script(custom_uri, code).expect("Should upsert script");

        let initial_profile =
            get_script_security_profile(custom_uri).expect("custom profile available");
        assert!(
            !initial_profile.privileged,
            "Custom scripts start restricted"
        );

        set_script_privileged(custom_uri, true).expect("Should toggle privileged flag");

        let updated_profile =
            get_script_security_profile(custom_uri).expect("updated profile available");
        assert!(updated_profile.privileged, "Flag update must persist");
    }
}
