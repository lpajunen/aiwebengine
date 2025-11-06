use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

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

/// Route registration: (path, method) -> handler_name
pub type RouteRegistrations = HashMap<(String, String), String>;

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
    pub code: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub initialized: bool,
    pub init_error: Option<String>,
    pub last_init_time: Option<SystemTime>,
    /// Cached route registrations from init() function
    pub registrations: RouteRegistrations,
}

impl ScriptMetadata {
    /// Create a new script metadata instance
    pub fn new(uri: String, code: String) -> Self {
        let now = SystemTime::now();
        Self {
            uri,
            code,
            created_at: now,
            updated_at: now,
            initialized: false,
            init_error: None,
            last_init_time: None,
            registrations: HashMap::new(),
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

    /// Update script code
    pub fn update_code(&mut self, new_code: String) {
        self.code = new_code;
        self.updated_at = SystemTime::now();
        // Reset initialization status when code changes
        self.initialized = false;
        self.init_error = None;
        // Clear cached registrations when code changes
        self.registrations.clear();
    }
}

/// Asset representation
#[derive(Debug, Clone)]
pub struct Asset {
    pub public_path: String,
    pub mimetype: String,
    pub content: Vec<u8>,
}

static DYNAMIC_SCRIPTS: OnceLock<Mutex<HashMap<String, ScriptMetadata>>> = OnceLock::new();
static DYNAMIC_LOGS: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
static DYNAMIC_ASSETS: OnceLock<Mutex<HashMap<String, Asset>>> = OnceLock::new();
static DYNAMIC_SCRIPT_STORAGE: OnceLock<Mutex<HashMap<String, HashMap<String, String>>>> =
    OnceLock::new();

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

type ScriptStorageGuard<'a> = std::sync::MutexGuard<'a, HashMap<String, HashMap<String, String>>>;

fn safe_lock_script_storage() -> Result<ScriptStorageGuard<'static>, RepositoryError> {
    let store = DYNAMIC_SCRIPT_STORAGE.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Script storage mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!(
                    "Failed to recover from poisoned script storage mutex: {}",
                    e
                );
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
async fn db_upsert_script(pool: &PgPool, uri: &str, code: &str) -> Result<(), RepositoryError> {
    let now = chrono::Utc::now();

    // Try to update existing script
    let update_result = sqlx::query(
        r#"
        UPDATE scripts
        SET code = $1, updated_at = $2
        WHERE uri = $3
        "#,
    )
    .bind(code)
    .bind(now)
    .bind(uri)
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
        INSERT INTO scripts (uri, code, created_at, updated_at)
        VALUES ($1, $2, $3, $3)
        "#,
    )
    .bind(uri)
    .bind(code)
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
        SELECT code FROM scripts WHERE uri = $1
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
        let code: String = row.try_get("code").map_err(|e| {
            error!("Database error getting code: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        Ok(Some(code))
    } else {
        Ok(None)
    }
}

/// Database-backed list all scripts
async fn db_list_scripts(pool: &PgPool) -> Result<HashMap<String, String>, RepositoryError> {
    let rows = sqlx::query(
        r#"
        SELECT uri, code FROM scripts ORDER BY uri
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
        let code: String = row.try_get("code").map_err(|e| {
            error!("Database error getting code: {}", e);
            RepositoryError::InvalidData(format!("Database error: {}", e))
        })?;
        scripts.insert(uri, code);
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

/// Database-backed set script storage item
async fn db_set_script_storage_item(
    pool: &PgPool,
    script_uri: &str,
    key: &str,
    value: &str,
) -> Result<(), RepositoryError> {
    let now = chrono::Utc::now();

    // Try to update existing item
    let update_result = sqlx::query(
        r#"
        UPDATE script_storage
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
        error!("Database error updating script storage: {}", e);
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    if update_result.rows_affected() > 0 {
        debug!(
            "Updated script storage item in database: {}:{}",
            script_uri, key
        );
        return Ok(());
    }

    // Item doesn't exist, create new one
    sqlx::query(
        r#"
        INSERT INTO script_storage (script_uri, key, value, created_at, updated_at)
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
        error!("Database error creating script storage item: {}", e);
        RepositoryError::InvalidData(format!("Database error: {}", e))
    })?;

    debug!(
        "Created new script storage item in database: {}:{}",
        script_uri, key
    );
    Ok(())
}

/// Database-backed get script storage item
async fn db_get_script_storage_item(
    pool: &PgPool,
    script_uri: &str,
    key: &str,
) -> Result<Option<String>, RepositoryError> {
    let row = sqlx::query(
        r#"
        SELECT value FROM script_storage WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        error!("Database error getting script storage item: {}", e);
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

/// Database-backed remove script storage item
async fn db_remove_script_storage_item(
    pool: &PgPool,
    script_uri: &str,
    key: &str,
) -> Result<bool, RepositoryError> {
    let result = sqlx::query(
        r#"
        DELETE FROM script_storage WHERE script_uri = $1 AND key = $2
        "#,
    )
    .bind(script_uri)
    .bind(key)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error removing script storage item: {}", e);
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

/// Database-backed clear all script storage for a script
async fn db_clear_script_storage(pool: &PgPool, script_uri: &str) -> Result<(), RepositoryError> {
    sqlx::query(
        r#"
        DELETE FROM script_storage WHERE script_uri = $1
        "#,
    )
    .bind(script_uri)
    .execute(pool)
    .await
    .map_err(|e| {
        error!("Database error clearing script storage: {}", e);
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
) -> Result<Vec<String>, RepositoryError> {
    let rows = sqlx::query(
        r#"
        SELECT message FROM logs
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
        .map(|row| row.try_get("message"))
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| {
            error!("Database error getting message: {}", e);
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
        let asset_mgmt = include_str!("../scripts/feature_scripts/asset_mgmt.js");
        let editor = include_str!("../scripts/feature_scripts/editor.js");
        let manager = include_str!("../scripts/feature_scripts/manager.js");
        let insufficient_permissions =
            include_str!("../scripts/feature_scripts/insufficient_permissions.js");

        m.insert("https://example.com/core".to_string(), core.to_string());
        m.insert(
            "https://example.com/asset_mgmt".to_string(),
            asset_mgmt.to_string(),
        );
        m.insert("https://example.com/editor".to_string(), editor.to_string());
        m.insert(
            "https://example.com/manager".to_string(),
            manager.to_string(),
        );
        m.insert(
            "https://example.com/insufficient_permissions".to_string(),
            insufficient_permissions.to_string(),
        );

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
                m.insert(k.to_string(), metadata.code.to_string());
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
        Ok(guard) => guard.get(uri).map(|metadata| metadata.code.clone()),
        Err(e) => {
            error!("Failed to access dynamic scripts for URI {}: {}", uri, e);
            None
        }
    }
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
pub fn fetch_log_messages(script_uri: &str) -> Vec<String> {
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
        Ok(guard) => guard.get(script_uri).cloned().unwrap_or_default(),
        Err(e) => {
            error!("Failed to fetch log messages for {}: {}", script_uri, e);
            vec![format!("Error: Could not retrieve logs - {}", e)]
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
    Ok(guard.values().cloned().collect())
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
        existing.update_code(content.to_string());
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
                "https://example.com/asset_mgmt",
                include_str!("../scripts/feature_scripts/asset_mgmt.js"),
            ),
            (
                "https://example.com/editor",
                include_str!("../scripts/feature_scripts/editor.js"),
            ),
            (
                "https://example.com/manager",
                include_str!("../scripts/feature_scripts/manager.js"),
            ),
            (
                "https://example.com/insufficient_permissions",
                include_str!("../scripts/feature_scripts/insufficient_permissions.js"),
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

    // Logo asset
    let logo_content = include_bytes!("../assets/logo.svg").to_vec();
    let logo = Asset {
        public_path: "/logo.svg".to_string(),
        mimetype: "image/svg+xml".to_string(),
        content: logo_content,
    };
    m.insert("/logo.svg".to_string(), logo);

    // Editor assets
    // Note: editor.html is NOT registered as a public asset
    // It's served exclusively through the /editor route in editor.js
    // This simplifies the API surface and provides a single entry point

    let editor_css_content = include_bytes!("../assets/editor.css").to_vec();
    let editor_css = Asset {
        public_path: "/editor.css".to_string(),
        mimetype: "text/css".to_string(),
        content: editor_css_content,
    };
    m.insert("/editor.css".to_string(), editor_css);

    let engine_css_content = include_bytes!("../assets/engine.css").to_vec();
    let engine_css = Asset {
        public_path: "/engine.css".to_string(),
        mimetype: "text/css".to_string(),
        content: engine_css_content,
    };
    m.insert("/engine.css".to_string(), engine_css);

    let editor_js_content = include_bytes!("../assets/editor.js").to_vec();
    let editor_js = Asset {
        public_path: "/editor.js".to_string(),
        mimetype: "application/javascript".to_string(),
        content: editor_js_content,
    };
    m.insert("/editor.js".to_string(), editor_js);

    let favicon_content = include_bytes!("../assets/favicon.ico").to_vec();
    let favicon = Asset {
        public_path: "/favicon.ico".to_string(),
        mimetype: "image/x-icon".to_string(),
        content: favicon_content,
    };
    m.insert("/favicon.ico".to_string(), favicon);

    m
}

/// Fetch assets with error handling (static + dynamic)
pub fn fetch_assets() -> HashMap<String, Asset> {
    let mut m = get_static_assets();

    // Merge in any dynamically upserted assets
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

    m
}

/// Fetch single asset with error handling (dynamic first, then static)
pub fn fetch_asset(public_path: &str) -> Option<Asset> {
    // Check dynamic assets first
    if let Ok(guard) = safe_lock_assets()
        && let Some(asset) = guard.get(public_path)
    {
        return Some(asset.clone());
    }

    // Check static assets
    get_static_assets().get(public_path).cloned()
}

/// Upsert asset with validation and error handling
pub fn upsert_asset(asset: Asset) -> Result<(), RepositoryError> {
    if asset.public_path.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "Public path cannot be empty".to_string(),
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

    let mut guard = safe_lock_assets()?;

    let public_path = asset.public_path.clone();
    guard.insert(public_path.clone(), asset);
    debug!("Upserted asset: {}", public_path);
    Ok(())
}

/// Delete asset with error handling  
pub fn delete_asset(public_path: &str) -> bool {
    match safe_lock_assets() {
        Ok(mut guard) => {
            let existed = guard.remove(public_path).is_some();
            if existed {
                debug!("Deleted asset: {}", public_path);
            } else {
                debug!("Attempted to delete non-existent asset: {}", public_path);
            }
            existed
        }
        Err(e) => {
            error!("Failed to delete asset {}: {}", public_path, e);
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

    // Count script storage entries
    match safe_lock_script_storage() {
        Ok(guard) => {
            let total_entries: usize = guard.values().map(|script_map| script_map.len()).sum();
            stats.insert("script_storage_entries".to_string(), total_entries);
        }
        Err(_) => {
            stats.insert("script_storage_entries".to_string(), 0);
        }
    }

    stats
}

/// Set a script storage item (key-value pair for a specific script)
pub fn set_script_storage_item(
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
                db_set_script_storage_item(db.pool(), script_uri, key, value).await
            })
        });

        match result {
            Ok(()) => {
                // Also update in-memory cache for consistency
                let _ = set_script_storage_item_in_memory(script_uri, key, value);
                debug!(
                    "Set script storage item to database: {}:{} = {} bytes",
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
    set_script_storage_item_in_memory(script_uri, key, value)
}

/// Get a script storage item
pub fn get_script_storage_item(script_uri: &str, key: &str) -> Option<String> {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_get_script_storage_item(db.pool(), script_uri, key).await })
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
    match safe_lock_script_storage() {
        Ok(guard) => guard
            .get(script_uri)
            .and_then(|script_map| script_map.get(key))
            .cloned(),
        Err(e) => {
            error!(
                "Failed to access script storage for get {}:{}: {}",
                script_uri, key, e
            );
            None
        }
    }
}

/// Remove a script storage item
pub fn remove_script_storage_item(script_uri: &str, key: &str) -> bool {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_remove_script_storage_item(db.pool(), script_uri, key).await })
        });

        match result {
            Ok(existed) => {
                // Also remove from in-memory cache for consistency
                let _ = safe_lock_script_storage()
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
    match safe_lock_script_storage() {
        Ok(mut guard) => {
            let existed = if let Some(script_map) = guard.get_mut(script_uri) {
                script_map.remove(key).is_some()
            } else {
                false
            };

            if existed {
                debug!(
                    "Removed script storage item from memory: {}:{}",
                    script_uri, key
                );
            } else {
                debug!(
                    "Script storage item not found in memory for removal: {}:{}",
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

/// Clear all script storage items for a specific script
pub fn clear_script_storage(script_uri: &str) -> Result<(), RepositoryError> {
    // Try database first
    if let Some(db) = get_db_pool() {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { db_clear_script_storage(db.pool(), script_uri).await })
        });

        match result {
            Ok(()) => {
                // Also clear from in-memory cache for consistency
                let _ = safe_lock_script_storage()
                    .map(|mut guard| {
                        guard.remove(script_uri);
                    })
                    .map_err(|e| warn!("Failed to clear from memory cache: {}", e));

                debug!(
                    "Cleared all script storage items from database for script: {}",
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
    match safe_lock_script_storage() {
        Ok(mut guard) => {
            guard.remove(script_uri);
            debug!(
                "Cleared all script storage items from memory for script: {}",
                script_uri
            );
            Ok(())
        }
        Err(e) => {
            error!("Failed to clear script storage for {}: {}", script_uri, e);
            Err(e)
        }
    }
}

/// In-memory implementation of set script storage item
fn set_script_storage_item_in_memory(
    script_uri: &str,
    key: &str,
    value: &str,
) -> Result<(), RepositoryError> {
    let mut guard = safe_lock_script_storage()?;

    let script_map = guard
        .entry(script_uri.to_string())
        .or_insert_with(HashMap::new);
    script_map.insert(key.to_string(), value.to_string());

    debug!(
        "Set script storage item in memory: {}:{} = {} bytes",
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
    fn test_script_storage_operations() {
        let script_uri = "test://storage-script";
        let key = "test_key";
        let value = "test_value";

        // Test set item
        assert!(set_script_storage_item(script_uri, key, value).is_ok());

        // Test get item
        let retrieved = get_script_storage_item(script_uri, key);
        assert_eq!(retrieved, Some(value.to_string()));

        // Test remove item
        assert!(remove_script_storage_item(script_uri, key));

        // Verify item is gone
        let retrieved_after_remove = get_script_storage_item(script_uri, key);
        assert_eq!(retrieved_after_remove, None);

        // Test clear storage
        assert!(set_script_storage_item(script_uri, "key1", "value1").is_ok());
        assert!(set_script_storage_item(script_uri, "key2", "value2").is_ok());

        assert!(clear_script_storage(script_uri).is_ok());

        // Verify both items are gone
        assert_eq!(get_script_storage_item(script_uri, "key1"), None);
        assert_eq!(get_script_storage_item(script_uri, "key2"), None);
    }

    #[test]
    fn test_script_storage_validation() {
        // Test empty script URI
        assert!(set_script_storage_item("", "key", "value").is_err());

        // Test empty key
        assert!(set_script_storage_item("test://script", "", "value").is_err());

        // Test oversized value (simulate by creating a large string)
        let large_value = "x".repeat(1_000_001); // Just over 1MB
        assert!(set_script_storage_item("test://script", "key", &large_value).is_err());
    }
}
