use anyhow::Result;
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
fn safe_lock_scripts()
-> Result<std::sync::MutexGuard<'static, HashMap<String, ScriptMetadata>>, RepositoryError> {
    let store = DYNAMIC_SCRIPTS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Scripts mutex was poisoned, recovering with new data");
            // In a poisoned state, we can still access the data but should log this
            // In production, you might want to restart the component or use more sophisticated recovery
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned mutex: {}", e);
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

fn safe_lock_logs()
-> Result<std::sync::MutexGuard<'static, HashMap<String, Vec<String>>>, RepositoryError> {
    let store = DYNAMIC_LOGS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Logs mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned logs mutex: {}", e);
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

fn safe_lock_assets()
-> Result<std::sync::MutexGuard<'static, HashMap<String, Asset>>, RepositoryError> {
    let store = DYNAMIC_ASSETS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Assets mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned assets mutex: {}", e);
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

type SharedStorageGuard<'a> = std::sync::MutexGuard<'a, HashMap<String, HashMap<String, String>>>;

fn safe_lock_shared_storage() -> Result<SharedStorageGuard<'static>, RepositoryError> {
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
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

fn safe_lock_privilege_overrides()
-> Result<std::sync::MutexGuard<'static, HashMap<String, bool>>, RepositoryError> {
    let store = SCRIPT_PRIVILEGE_OVERRIDES.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Privilege override mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned privilege mutex: {}", e);
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

/// Get database pool if available
fn get_db_pool() -> Option<std::sync::Arc<crate::database::Database>> {
    crate::database::get_global_database()
}

/// Database-backed upsert script
async fn db_upsert_script(pool: &PgPool, uri: &str, content: &str) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    debug!("Created new script in database: {}", uri);
    Ok(())
}

/// Database-backed get script
async fn db_get_script(pool: &PgPool, uri: &str) -> Result<Option<String>, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    if let Some(row) = row {
        let content: String = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        Ok(Some(content))
    } else {
        Ok(None)
    }
}

/// Database-backed list all scripts
async fn db_list_scripts(pool: &PgPool) -> Result<HashMap<String, String>, RepositoryError> {
    let rows = sqlx::query(
        r#"
        SELECT uri, content FROM scripts ORDER BY uri
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| {
        error!("Database error listing scripts: {}", e);
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    let mut scripts = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("uri").map_err(|e| {
            error!("Database error getting uri: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let content: String = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        scripts.insert(uri, content);
    }

    Ok(scripts)
}

/// Database-backed delete script
async fn db_delete_script(pool: &PgPool, uri: &str) -> Result<bool, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
async fn db_get_script_privileged(
    pool: &PgPool,
    uri: &str,
) -> Result<Option<bool>, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    if let Some(row) = row {
        let privileged: bool = row.try_get("privileged").map_err(|e| {
            error!("Database error parsing privilege flag: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        Ok(Some(privileged))
    } else {
        Ok(None)
    }
}

/// Database-backed setter for script privilege flag
async fn db_set_script_privileged(
    pool: &PgPool,
    uri: &str,
    privileged: bool,
) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    Ok(())
}

/// Database-backed set shared storage item
async fn db_set_shared_storage_item(
    pool: &PgPool,
    script_uri: &str,
    key: &str,
    value: &str,
) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
) -> Result<Option<String>, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    if let Some(row) = row {
        let value: String = row.try_get("value").map_err(|e| {
            error!("Database error getting value: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
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
) -> Result<bool, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
async fn db_clear_shared_storage(pool: &PgPool, script_uri: &str) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    debug!(
        "Inserted log message to database for script: {}",
        script_uri
    );
    Ok(())
}

/// Database-backed fetch log messages for a script
async fn db_fetch_log_messages(
    pool: &PgPool,
    script_uri: &str,
) -> Result<Vec<LogEntry>, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;

    Ok(messages)
}

/// Database-backed fetch all log messages
async fn db_fetch_all_log_messages(pool: &PgPool) -> Result<Vec<LogEntry>, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;

    Ok(messages)
}

/// Database-backed clear log messages for a script
async fn db_clear_log_messages(pool: &PgPool, script_uri: &str) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    debug!(
        "Cleared log messages from database for script: {}",
        script_uri
    );
    Ok(())
}

/// Database-backed prune log messages (keep only latest 20 per script)
async fn db_prune_log_messages(pool: &PgPool) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    debug!("Pruned log messages in database, keeping 20 entries per script");
    Ok(())
}

/// Database-backed upsert asset
async fn db_upsert_asset(pool: &PgPool, asset: &Asset) -> Result<(), RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    debug!("Created new asset in database: {}", asset.uri);
    Ok(())
}

/// Database-backed get asset by URI
async fn db_get_asset(pool: &PgPool, uri: &str) -> Result<Option<Asset>, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    if let Some(row) = row {
        let uri: String = row.try_get("uri").map_err(|e| {
            error!("Database error getting uri: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let mimetype: String = row.try_get("mimetype").map_err(|e| {
            error!("Database error getting mimetype: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let content: Vec<u8> = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at").map_err(|e| {
            error!("Database error getting created_at: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at").map_err(|e| {
            error!("Database error getting updated_at: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
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
async fn db_list_assets(pool: &PgPool) -> Result<HashMap<String, Asset>, RepositoryError> {
    let rows = sqlx::query(
        r#"
        SELECT uri, mimetype, content, name, created_at, updated_at FROM assets ORDER BY uri
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| {
        error!("Database error listing assets: {}", e);
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    let mut assets = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("uri").map_err(|e| {
            error!("Database error getting uri: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let mimetype: String = row.try_get("mimetype").map_err(|e| {
            error!("Database error getting mimetype: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let content: Vec<u8> = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at").map_err(|e| {
            error!("Database error getting created_at: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        let updated_at: chrono::DateTime<chrono::Utc> = row.try_get("updated_at").map_err(|e| {
            error!("Database error getting updated_at: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
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
async fn db_delete_asset(pool: &PgPool, uri: &str) -> Result<bool, RepositoryError> {
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
        RepositoryError::InvalidData(format!("Database error: {}", e))
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
    let mut m = HashMap::new();

    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { db_list_scripts(db.pool()).await })
        });

        match result {
            Ok(db_scripts) => {
                m.extend(db_scripts);
                debug!("Loaded {} scripts from database", m.len());
            }
            Err(e) => {
                warn!(
                    "Database script fetch failed, falling back to static scripts: {}",
                    e
                );
                // Fall through to static scripts
            }
        }
    }

    // Always include core functionality scripts (fallback or when no database)
    if m.is_empty() {
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
    }

    // Safely merge in any dynamically upserted scripts
    match safe_lock_scripts() {
        Ok(guard) => {
            for (k, metadata) in guard.iter() {
                m.insert(k.to_string(), metadata.content.to_string());
            }
        }
        Err(e) => {
            error!(
                "Failed to access dynamic scripts: {}. Continuing with static scripts only.",
                e
            );
            // Continue with just the static scripts rather than crashing
        }
    }

    m
}

/// Fetch a single script by URI with proper error handling
pub fn fetch_script(uri: &str) -> Option<String> {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_get_script(db.pool(), uri).await })
        });

        match result {
            Ok(Some(script)) => {
                debug!("Loaded script from database: {}", uri);
                return Some(script);
            }
            Ok(None) => {
                // Script not in database, continue to check static/dynamic
            }
            Err(e) => {
                warn!("Database script fetch failed for {}: {}", uri, e);
                // Fall through to static/dynamic scripts
            }
        }
    }

    // First check static scripts
    let static_scripts = fetch_scripts();
    if let Some(script) = static_scripts.get(uri) {
        return Some(script.clone());
    }

    // Then check dynamic scripts
    match safe_lock_scripts() {
        Ok(guard) => guard.get(uri).map(|metadata| metadata.content.clone()),
        Err(e) => {
            error!("Failed to access dynamic scripts for URI {}: {}", uri, e);
            None
        }
    }
}

/// Determine current privilege state for a script
pub fn get_script_security_profile(uri: &str) -> Result<ScriptSecurityProfile, RepositoryError> {
    let default_privileged = default_privileged_for(uri);

    // Prefer database when configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_get_script_privileged(db.pool(), uri).await })
        });

        match result {
            Ok(Some(value)) => {
                return Ok(ScriptSecurityProfile {
                    uri: uri.to_string(),
                    privileged: value,
                    default_privileged,
                });
            }
            Ok(None) => {
                // Not persisted, fall through to in-memory/static defaults
            }
            Err(e) => {
                warn!(
                    "Database privilege lookup failed for {}: {}. Falling back to cache",
                    uri, e
                );
            }
        }
    }

    // Dynamic scripts stored in memory
    if let Ok(guard) = safe_lock_scripts()
        && let Some(metadata) = guard.get(uri)
    {
        return Ok(ScriptSecurityProfile {
            uri: uri.to_string(),
            privileged: metadata.privileged,
            default_privileged,
        });
    }

    // Local overrides for static scripts when no database is configured
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
pub fn is_script_privileged(uri: &str) -> Result<bool, RepositoryError> {
    Ok(get_script_security_profile(uri)?.privileged)
}

/// Update the privilege flag for a script
pub fn set_script_privileged(uri: &str, privileged: bool) -> Result<(), RepositoryError> {
    // Ensure script exists before toggling
    if fetch_script(uri).is_none() {
        return Err(RepositoryError::ScriptNotFound(uri.to_string()));
    }

    // Update database when available
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_set_script_privileged(db.pool(), uri, privileged).await })
        });

        if let Err(e) = result {
            warn!(
                "Failed to persist privileged flag for {} in database: {}. Falling back to in-memory override",
                uri, e
            );
        }
    }

    // Update dynamic metadata cache if present
    if let Ok(mut guard) = safe_lock_scripts()
        && let Some(metadata) = guard.get_mut(uri)
    {
        metadata.privileged = privileged;
        debug!(
            script = %uri,
            privileged,
            "Updated in-memory script privilege"
        );
        return Ok(());
    }

    // Persist override for static scripts or when metadata not present
    let mut guard = safe_lock_privilege_overrides()?;
    guard.insert(uri.to_string(), privileged);
    debug!(
        script = %uri,
        privileged,
        "Stored privilege override in memory"
    );
    Ok(())
}

/// Insert log message with error handling
pub fn insert_log_message(script_uri: &str, message: &str, log_level: &str) {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                db_insert_log_message(db.pool(), script_uri, message, log_level).await
            })
        });

        match result {
            Ok(()) => {
                debug!(
                    "Inserted log message to database for script: {}",
                    script_uri
                );
                return;
            }
            Err(e) => {
                warn!(
                    "Database log insert failed, falling back to in-memory: {}",
                    e
                );
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    match safe_lock_logs() {
        Ok(mut guard) => {
            guard
                .entry(script_uri.to_string())
                .or_insert_with(Vec::new)
                .push(message.to_string());
            debug!("Logged message for script {}: {}", script_uri, message);
        }
        Err(e) => {
            error!(
                "Failed to insert log message for {}: {}. Message: {}",
                script_uri, e, message
            );
            // Log to system instead as fallback
            error!("FALLBACK LOG [{}]: {}", script_uri, message);
        }
    }
}

/// Fetch log messages with error handling
pub fn fetch_log_messages(script_uri: &str) -> Vec<LogEntry> {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_fetch_log_messages(db.pool(), script_uri).await })
        });

        match result {
            Ok(messages) => {
                debug!(
                    "Fetched {} log messages from database for script: {}",
                    messages.len(),
                    script_uri
                );
                return messages;
            }
            Err(e) => {
                warn!(
                    "Database log fetch failed, falling back to in-memory: {}",
                    e
                );
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    match safe_lock_logs() {
        Ok(guard) => {
            let now = SystemTime::now();
            guard
                .get(script_uri)
                .map(|messages| {
                    messages
                        .iter()
                        .map(|msg| LogEntry::new(msg.clone(), "INFO".to_string(), now))
                        .collect()
                })
                .unwrap_or_default()
        }
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
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_fetch_all_log_messages(db.pool()).await })
        });

        match result {
            Ok(messages) => {
                debug!("Fetched {} log messages from database", messages.len());
                return messages;
            }
            Err(e) => {
                warn!(
                    "Database all logs fetch failed, falling back to in-memory: {}",
                    e
                );
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    match safe_lock_logs() {
        Ok(guard) => {
            let mut all_logs = Vec::new();
            let now = SystemTime::now();
            for logs in guard.values() {
                for message in logs {
                    all_logs.push(LogEntry::new(message.clone(), "INFO".to_string(), now));
                }
            }
            all_logs
        }
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
pub fn clear_log_messages(script_uri: &str) -> Result<(), RepositoryError> {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_clear_log_messages(db.pool(), script_uri).await })
        });

        match result {
            Ok(()) => {
                debug!(
                    "Cleared log messages from database for script: {}",
                    script_uri
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    "Database log clear failed, falling back to in-memory: {}",
                    e
                );
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    let mut guard = safe_lock_logs()?;
    guard.remove(script_uri);
    debug!("Cleared log messages for script: {}", script_uri);
    Ok(())
}

/// Keep only the latest `limit` log messages (default 20) for each script URI and remove older ones
pub fn prune_log_messages() -> Result<(), RepositoryError> {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_prune_log_messages(db.pool()).await })
        });

        match result {
            Ok(()) => {
                debug!("Pruned log messages in database");
                return Ok(());
            }
            Err(e) => {
                warn!(
                    "Database log prune failed, falling back to in-memory: {}",
                    e
                );
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    const LIMIT: usize = 20;
    let mut guard = safe_lock_logs()?;

    for logs in guard.values_mut() {
        if logs.len() > LIMIT {
            let remove = logs.len() - LIMIT;
            // remove older entries from the front
            logs.drain(0..remove);
        }
    }
    debug!("Pruned log messages, keeping {} entries per script", LIMIT);
    Ok(())
}

/// Get script metadata for a specific URI
pub fn get_script_metadata(uri: &str) -> Result<ScriptMetadata, RepositoryError> {
    let guard = safe_lock_scripts()?;
    guard
        .get(uri)
        .cloned()
        .ok_or_else(|| RepositoryError::ScriptNotFound(uri.to_string()))
}

/// Get all script metadata
pub fn get_all_script_metadata() -> Result<Vec<ScriptMetadata>, RepositoryError> {
    let guard = safe_lock_scripts()?;
    let mut metadata_list: Vec<ScriptMetadata> = guard.values().cloned().collect();
    drop(guard); // Release lock before calling get_script_security_profile

    // Update privileged status for each script
    for metadata in &mut metadata_list {
        if let Ok(profile) = get_script_security_profile(&metadata.uri) {
            metadata.privileged = profile.privileged;
        }
    }

    Ok(metadata_list)
}

/// Mark a script as initialized successfully
pub fn mark_script_initialized(uri: &str) -> Result<(), RepositoryError> {
    let mut guard = safe_lock_scripts()?;

    if let Some(metadata) = guard.get_mut(uri) {
        metadata.mark_initialized();
        debug!("Marked script as initialized: {}", uri);
        Ok(())
    } else {
        Err(RepositoryError::ScriptNotFound(uri.to_string()))
    }
}

/// Mark a script as initialized with registrations from init()
pub fn mark_script_initialized_with_registrations(
    uri: &str,
    registrations: RouteRegistrations,
) -> Result<(), RepositoryError> {
    let mut guard = safe_lock_scripts()?;

    if let Some(metadata) = guard.get_mut(uri) {
        metadata.mark_initialized_with_registrations(registrations);
        debug!("Marked script as initialized with registrations: {}", uri);
        Ok(())
    } else {
        Err(RepositoryError::ScriptNotFound(uri.to_string()))
    }
}

/// Mark a script initialization as failed
pub fn mark_script_init_failed(uri: &str, error: String) -> Result<(), RepositoryError> {
    let mut guard = safe_lock_scripts()?;

    if let Some(metadata) = guard.get_mut(uri) {
        metadata.mark_init_failed(error.clone());
        debug!("Marked script init as failed: {} - {}", uri, error);
        Ok(())
    } else {
        Err(RepositoryError::ScriptNotFound(uri.to_string()))
    }
}

/// Upsert script with error handling
pub fn upsert_script(uri: &str, content: &str) -> Result<(), RepositoryError> {
    if uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "URI cannot be empty".to_string(),
        ));
    }

    if content.len() > 1_000_000 {
        // 1MB limit
        return Err(RepositoryError::InvalidData(
            "Script content too large (>1MB)".to_string(),
        ));
    }

    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_upsert_script(db.pool(), uri, content).await })
        });

        match result {
            Ok(()) => {
                // Also update in-memory cache for consistency
                let _ = upsert_script_in_memory(uri, content);
                debug!(
                    "Upserted script to database: {} ({} bytes)",
                    uri,
                    content.len()
                );
                return Ok(());
            }
            Err(e) => {
                warn!("Database upsert failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    upsert_script_in_memory(uri, content)
}

/// In-memory implementation of upsert script (existing logic)
fn upsert_script_in_memory(uri: &str, content: &str) -> Result<(), RepositoryError> {
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

/// Bootstrap hardcoded scripts into database on startup
pub fn bootstrap_scripts() -> Result<(), RepositoryError> {
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

        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
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
            })
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
pub fn bootstrap_assets() -> Result<(), RepositoryError> {
    if let Some(db) = get_db_pool() {
        let pool = db.pool();

        // Get all static assets
        let static_assets = get_static_assets();

        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                for (asset_name, asset) in static_assets {
                    // Check if asset already exists
                    if let Ok(Some(_)) = db_get_asset(pool, &asset_name).await {
                        debug!("Asset already exists in database: {}", asset_name);
                        continue;
                    }

                    // Insert the asset
                    if let Err(e) = db_upsert_asset(pool, &asset).await {
                        error!("Failed to bootstrap asset {}: {}", asset_name, e);
                        return Err(e);
                    } else {
                        info!("Bootstrapped asset into database: {}", asset_name);
                    }
                }
                Ok(())
            })
        });

        match result {
            Ok(()) => {
                info!("Successfully bootstrapped assets into database");
                Ok(())
            }
            Err(e) => {
                error!("Failed to bootstrap assets: {}", e);
                Err(e)
            }
        }
    } else {
        debug!("Database not configured, skipping asset bootstrap");
        Ok(())
    }
}

/// Delete script with error handling
pub fn delete_script(uri: &str) -> bool {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_delete_script(db.pool(), uri).await })
        });

        match result {
            Ok(existed) => {
                // Also remove from in-memory cache for consistency
                let _ = safe_lock_scripts()
                    .map(|mut guard| guard.remove(uri))
                    .map_err(|e| warn!("Failed to remove script from memory cache: {}", e));

                if existed {
                    debug!("Deleted script from database: {}", uri);
                } else {
                    debug!("Script not found in database for deletion: {}", uri);
                }
                return existed;
            }
            Err(e) => {
                warn!("Database delete failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    match safe_lock_scripts() {
        Ok(mut guard) => {
            let existed = guard.remove(uri).is_some();
            if existed {
                debug!("Deleted script from memory: {}", uri);
            } else {
                debug!(
                    "Attempted to delete non-existent script from memory: {}",
                    uri
                );
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

/// Fetch assets with error handling (static + dynamic)
pub fn fetch_assets() -> HashMap<String, Asset> {
    let mut m = HashMap::new();

    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { db_list_assets(db.pool()).await })
        });

        match result {
            Ok(db_assets) => {
                m.extend(db_assets);
                debug!("Loaded {} assets from database", m.len());
            }
            Err(e) => {
                warn!(
                    "Database asset fetch failed, falling back to static assets: {}",
                    e
                );
                // Fall through to static assets
            }
        }
    }

    // Always include static assets (fallback or when no database)
    if m.is_empty() {
        m = get_static_assets();

        // Merge in any dynamically upserted assets when using in-memory mode
        match safe_lock_assets() {
            Ok(guard) => {
                for (k, v) in guard.iter() {
                    m.insert(k.clone(), v.clone());
                }
            }
            Err(e) => {
                error!("Failed to fetch dynamic assets: {}", e);
            }
        }
    }

    m
}

/// Fetch single asset by URI with error handling (dynamic first, then static)
pub fn fetch_asset(uri: &str) -> Option<Asset> {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { db_get_asset(db.pool(), uri).await })
        });

        match result {
            Ok(Some(asset)) => {
                debug!("Loaded asset from database: {}", uri);
                return Some(asset);
            }
            Ok(None) => {
                // Asset not in database, continue to check static/dynamic
            }
            Err(e) => {
                warn!("Database asset fetch failed for {}: {}", uri, e);
                // Fall through to static/dynamic assets
            }
        }
    }

    // Check static assets first
    if let Some(asset) = get_static_assets().get(uri) {
        return Some(asset.clone());
    }

    // Then check dynamic assets
    if let Ok(guard) = safe_lock_assets()
        && let Some(asset) = guard.get(uri)
    {
        return Some(asset.clone());
    }

    None
}

/// Upsert asset with validation and error handling
pub fn upsert_asset(asset: Asset) -> Result<(), RepositoryError> {
    if asset.uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "Asset URI cannot be empty".to_string(),
        ));
    }

    if asset.content.len() > 10_000_000 {
        // 10MB limit for assets
        return Err(RepositoryError::InvalidData(
            "Asset content too large (>10MB)".to_string(),
        ));
    }

    if asset.mimetype.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "MIME type cannot be empty".to_string(),
        ));
    }

    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_upsert_asset(db.pool(), &asset).await })
        });

        match result {
            Ok(()) => {
                // Also update in-memory cache for consistency
                let _ = upsert_asset_in_memory(&asset);
                debug!(
                    "Upserted asset to database: {} ({} bytes)",
                    asset.uri,
                    asset.content.len()
                );
                return Ok(());
            }
            Err(e) => {
                warn!("Database upsert failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    upsert_asset_in_memory(&asset)
}

/// In-memory implementation of upsert asset (existing logic)
fn upsert_asset_in_memory(asset: &Asset) -> Result<(), RepositoryError> {
    let mut guard = safe_lock_assets()?;

    let uri = asset.uri.clone();
    guard.insert(uri.clone(), asset.clone());
    debug!("Upserted asset in memory: {}", uri);
    Ok(())
}

/// Delete asset with error handling  
pub fn delete_asset(uri: &str) -> bool {
    // Try database first if configured
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_delete_asset(db.pool(), uri).await })
        });

        match result {
            Ok(existed) => {
                // Also remove from in-memory cache for consistency
                let _ = safe_lock_assets()
                    .map(|mut guard| guard.remove(uri))
                    .map_err(|e| warn!("Failed to remove asset from memory cache: {}", e));

                if existed {
                    debug!("Deleted asset from database: {}", uri);
                } else {
                    debug!("Asset not found in database for deletion: {}", uri);
                }
                return existed;
            }
            Err(e) => {
                warn!("Database delete failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    match safe_lock_assets() {
        Ok(mut guard) => {
            let existed = guard.remove(uri).is_some();
            if existed {
                debug!("Deleted asset from memory: {}", uri);
            } else {
                debug!(
                    "Attempted to delete non-existent asset from memory: {}",
                    uri
                );
            }
            existed
        }
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
pub fn set_shared_storage_item(
    script_uri: &str,
    key: &str,
    value: &str,
) -> Result<(), RepositoryError> {
    if script_uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "Script URI cannot be empty".to_string(),
        ));
    }

    if key.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "Key cannot be empty".to_string(),
        ));
    }

    if value.len() > 1_000_000 {
        // 1MB limit per value
        return Err(RepositoryError::InvalidData(
            "Value too large (>1MB)".to_string(),
        ));
    }

    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                db_set_shared_storage_item(db.pool(), script_uri, key, value).await
            })
        });

        match result {
            Ok(()) => {
                // Also update in-memory cache for consistency
                let _ = set_shared_storage_item_in_memory(script_uri, key, value);
                debug!(
                    "Set shared storage item to database: {}:{} = {} bytes",
                    script_uri,
                    key,
                    value.len()
                );
                return Ok(());
            }
            Err(e) => {
                warn!("Database set failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    set_shared_storage_item_in_memory(script_uri, key, value)
}

/// Get a shared storage item
pub fn get_shared_storage_item(script_uri: &str, key: &str) -> Option<String> {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_get_shared_storage_item(db.pool(), script_uri, key).await })
        });

        match result {
            Ok(Some(value)) => {
                debug!(
                    "Got script storage item from database: {}:{}",
                    script_uri, key
                );
                return Some(value);
            }
            Ok(None) => {
                // Item not in database, continue to check in-memory
            }
            Err(e) => {
                warn!("Database get failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Check in-memory storage
    match safe_lock_shared_storage() {
        Ok(guard) => guard
            .get(script_uri)
            .and_then(|script_map| script_map.get(key))
            .cloned(),
        Err(e) => {
            error!(
                "Failed to access shared storage for get {}:{}: {}",
                script_uri, key, e
            );
            None
        }
    }
}

/// Remove a shared storage item
pub fn remove_shared_storage_item(script_uri: &str, key: &str) -> bool {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_remove_shared_storage_item(db.pool(), script_uri, key).await })
        });

        match result {
            Ok(existed) => {
                // Also remove from in-memory cache for consistency
                let _ = safe_lock_shared_storage()
                    .map(|mut guard| {
                        if let Some(script_map) = guard.get_mut(script_uri) {
                            script_map.remove(key);
                        }
                    })
                    .map_err(|e| warn!("Failed to remove from memory cache: {}", e));

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
                return existed;
            }
            Err(e) => {
                warn!("Database remove failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    match safe_lock_shared_storage() {
        Ok(mut guard) => {
            let existed = if let Some(script_map) = guard.get_mut(script_uri) {
                script_map.remove(key).is_some()
            } else {
                false
            };

            if existed {
                debug!(
                    "Removed shared storage item from memory: {}:{}",
                    script_uri, key
                );
            } else {
                debug!(
                    "Shared storage item not found in memory for removal: {}:{}",
                    script_uri, key
                );
            }
            existed
        }
        Err(e) => {
            error!(
                "Failed to remove script storage item {}:{}: {}",
                script_uri, key, e
            );
            false
        }
    }
}

/// Clear all shared storage items for a specific script
pub fn clear_shared_storage(script_uri: &str) -> Result<(), RepositoryError> {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_clear_shared_storage(db.pool(), script_uri).await })
        });

        match result {
            Ok(()) => {
                // Also clear from in-memory cache for consistency
                let _ = safe_lock_shared_storage()
                    .map(|mut guard| {
                        guard.remove(script_uri);
                    })
                    .map_err(|e| warn!("Failed to clear from memory cache: {}", e));

                debug!(
                    "Cleared all shared storage items from database for script: {}",
                    script_uri
                );
                return Ok(());
            }
            Err(e) => {
                warn!("Database clear failed, falling back to in-memory: {}", e);
                // Fall through to in-memory implementation
            }
        }
    }

    // Fall back to in-memory implementation
    match safe_lock_shared_storage() {
        Ok(mut guard) => {
            guard.remove(script_uri);
            debug!(
                "Cleared all shared storage items from memory for script: {}",
                script_uri
            );
            Ok(())
        }
        Err(e) => {
            error!("Failed to clear shared storage for {}: {}", script_uri, e);
            Err(e)
        }
    }
}

/// In-memory implementation of set script storage item
fn set_shared_storage_item_in_memory(
    script_uri: &str,
    key: &str,
    value: &str,
) -> Result<(), RepositoryError> {
    let mut guard = safe_lock_shared_storage()?;

    let script_map = guard
        .entry(script_uri.to_string())
        .or_insert_with(HashMap::new);
    script_map.insert(key.to_string(), value.to_string());

    debug!(
        "Set shared storage item in memory: {}:{} = {} bytes",
        script_uri,
        key,
        value.len()
    );
    Ok(())
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
