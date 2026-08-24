use crate::error::{AppError, AppResult};
use crate::scheduler;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool, Row};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

// TODO: Transaction Integration
// All database operations in this file currently use &PgPool directly.
// To support transactions, these should be refactored to accept a generic Executor:
//
// Example pattern:
//   async fn db_get_script<'e, E>(executor: E, uri: &str) -> AppResult<Option<String>>
//   where
//       E: sqlx::Executor<'e, Database = sqlx::Postgres>,
//   {
//       sqlx::query("SELECT content FROM scripts WHERE uri = $1")
//           .bind(uri)
//           .fetch_optional(executor)
//           .await?
//   }
//
// This would allow operations to work with both:
// - Direct pool access: db_get_script(&pool, uri)
// - Within transaction: db_get_script(&mut tx, uri)
//
// The synchronous wrappers (run_blocking) would check thread-local transaction
// state and pass the appropriate executor.

/// Built-in scripts that were bootstrapped by earlier versions of the engine
/// and have since been replaced by native Rust functionality. They are
/// removed from the database on startup so stale copies stop executing.
const RETIRED_BOOTSTRAP_SCRIPTS: &[&str] = &[
    "https://example.com/core",
    "https://example.com/cli",
    "https://example.com/auth",
];

/// Helper to run async code in a blocking context, handling different runtime scenarios
fn run_blocking<F, R>(future: F) -> R
where
    F: std::future::Future<Output = R>,
{
    crate::database::run_blocking(future)
}

/// The same, bounded by whatever remains of the caller's execution budget.
///
/// What every wrapper a script can reach goes through. A blocked database call
/// is invisible to the interrupt handler that enforces the budget between
/// bytecode operations, so without this the budget simply does not apply to the
/// one kind of work most likely to exceed it.
fn run_bounded<F, T>(future: F) -> AppResult<T>
where
    F: std::future::Future<Output = AppResult<T>>,
{
    crate::database::run_bounded(future)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "requestBody")]
    pub request_body: Option<serde_json::Value>,
}

impl RouteMetadata {
    pub fn simple(handler_name: String) -> Self {
        Self {
            handler_name,
            summary: None,
            description: None,
            tags: Vec::new(),
            parameters: None,
            request_body: None,
        }
    }
}

/// Route registration: (path, method) -> RouteMetadata
pub type RouteRegistrations = HashMap<(String, String), RouteMetadata>;

/// Log entry with timestamp information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// Script the message was logged by. Callers that fetch logs across every
    /// script rely on this to attribute each entry.
    pub script_uri: String,
    pub message: String,
    pub level: String,
    pub timestamp: SystemTime,
    /// Monotonic write order. Breaks `timestamp` ties, and doubles as the
    /// cursor a caller pages or tails from. Zero for entries that were never
    /// read back from the database (error placeholders).
    pub seq: i64,
    /// Which invocation emitted this line, when one did. See [`LogContext`].
    pub context: LogContext,
}

impl LogEntry {
    pub fn new(script_uri: String, message: String, level: String, timestamp: SystemTime) -> Self {
        Self {
            script_uri,
            message,
            level,
            timestamp,
            seq: 0,
            context: LogContext::default(),
        }
    }

    /// Attach the invocation this entry was emitted by.
    pub fn with_context(mut self, seq: i64, context: LogContext) -> Self {
        self.seq = seq;
        self.context = context;
        self
    }
}

/// Identifies the invocation a log line was emitted by.
///
/// A script's output is otherwise one undifferentiated stream: the lines from a
/// route call, a scheduler tick and a stream connection interleave and cannot be
/// separated again. Carrying this down to each write is what lets a caller ask
/// for "the lines this one request produced" or "everything this route logged".
///
/// Every field is optional because engine-internal writes — startup, transpiler
/// diagnostics — have no invocation to name.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogContext {
    /// Groups the lines one invocation emitted. For HTTP this is the request's
    /// `x-request-id`, so a client holding the response header can ask for
    /// exactly the lines its own call produced; other invocation kinds generate
    /// one per run.
    pub request_id: Option<String>,
    /// What sort of invocation this was: `httpRoute`, `scheduled`,
    /// `streamCustomization`, … — the names
    /// `js_engine::HandlerInvocationKind` uses.
    pub kind: Option<String>,
    /// The registered route pattern (`/things/:id`), not the concrete path, so
    /// that filtering by it aggregates every call to the same handler. For
    /// invocations that are not HTTP routes this names the job, stream or tool.
    pub route: Option<String>,
}

impl LogContext {
    /// True when this context names nothing, i.e. there is no invocation to
    /// attribute the line to.
    pub fn is_empty(&self) -> bool {
        self.request_id.is_none() && self.kind.is_none() && self.route.is_none()
    }
}

/// Filters for a log query. Every field is optional; `None` means "no filter".
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Restrict to one script; `None` spans every script.
    pub script_uri: Option<String>,
    /// Restrict to one log level, e.g. `ERROR`. Matched case-insensitively.
    pub level: Option<String>,
    /// Keep only entries logged at or after this instant.
    pub since: Option<SystemTime>,
    /// Keep only entries written after this [`LogEntry::seq`]. Used to page or
    /// tail forward without re-reading what the caller already has, and unlike
    /// `since` it cannot repeat or skip entries that share a timestamp. Set
    /// with `limit`, it takes the *oldest* entries past the cursor, so reading
    /// forward one page at a time cannot skip what falls between pages.
    pub after_seq: Option<i64>,
    /// Keep only entries whose message contains this substring, matched
    /// case-insensitively.
    pub contains: Option<String>,
    /// Keep only the entries one invocation emitted. See
    /// [`LogContext::request_id`].
    pub request_id: Option<String>,
    /// Keep only entries from invocations of this kind, e.g. `scheduled`.
    /// Matched case-insensitively.
    pub kind: Option<String>,
    /// Keep only entries logged while serving this registered route pattern.
    pub route: Option<String>,
    /// Keep at most this many of the *newest* matching entries.
    pub limit: Option<i64>,
}

impl LogQuery {
    /// All logs for one script, unfiltered.
    pub fn for_uri(script_uri: &str) -> Self {
        Self {
            script_uri: Some(script_uri.to_string()),
            ..Self::default()
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
    pub owners: Vec<String>,
    /// Hosts this script's registrations are published on, as stored in
    /// `script_hosts`. Empty means the default host; a `*` entry means every
    /// configured host. Resolve with [`crate::hosts::effective_hosts`] rather
    /// than reading it directly.
    pub hosts: Vec<String>,
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
            owners: Vec::new(),
            hosts: Vec::new(),
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

    /// Update script content, keeping the route registrations installed by the
    /// last successful `init()`.
    ///
    /// Registrations live only in this in-memory metadata, and routing skips
    /// any script whose registrations are empty (see `route_index::build_index`).
    /// Clearing them here made every route of a script 404 from the moment its
    /// source was upserted until the re-init that follows finished — seconds for
    /// a small script, indefinitely for one whose init() times out. Keeping the
    /// previous table means a deploy serves the *new* source through the *old*
    /// route map for that window, and `update_script_init_status` swaps in the
    /// new map atomically when init() succeeds.
    pub fn update_content(&mut self, new_content: String) {
        self.content = new_content;
        self.updated_at = SystemTime::now();
        // The pending init() has not run against this source yet; `init_error`
        // describes the previous one, so drop it. `initialized` and
        // `registrations` stay as-is so routing keeps working meanwhile.
        self.init_error = None;
    }
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
    pub script_uri: String,
}

// ============================================================================
// Script Database Schema Introspection Types
// ============================================================================

/// Metadata about a script-owned table
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableInfo {
    pub logical_name: String,
    pub physical_name: String,
    pub created_at: DateTime<Utc>,
}

/// Schema information for a table
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableSchema {
    pub table_name: String,
    pub columns: Vec<ColumnInfo>,
}

/// One column a script wants a table to have.
#[derive(Debug, Clone)]
pub struct EnsuredColumn {
    pub name: String,
    pub column_type: crate::db_schema_utils::ColumnType,
    pub nullable: bool,
    pub default_value: Option<String>,
}

/// The shape a script wants a table to be in, whatever shape it is in now.
#[derive(Debug, Clone, Default)]
pub struct TableSpec {
    pub columns: Vec<EnsuredColumn>,
    /// Column groups that must each carry a unique index — what `upsert` needs
    /// before it can use them as a conflict target.
    pub unique_indexes: Vec<Vec<String>>,
}

/// What converging a table to a [`TableSpec`] actually changed.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsuredTable {
    /// Whether this call is the one that created the table.
    pub created: bool,
    /// Columns this call added. A column already present is not listed.
    pub columns_added: Vec<String>,
    /// Index column groups this call ensured. Postgres does not report whether
    /// `CREATE UNIQUE INDEX IF NOT EXISTS` created anything, so these are
    /// "present now", not "added now".
    pub unique_indexes_ensured: Vec<Vec<String>>,
}

/// Information about a table column
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String, // "INTEGER", "TEXT", "BOOLEAN", "TIMESTAMPTZ"
    pub nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default)]
    pub is_primary_key: bool,
}

/// Foreign key relationship information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ForeignKeyInfo {
    pub column_name: String,
    pub referenced_table_logical: String,
    pub referenced_table_physical: String,
    pub referenced_column: String,
}

static DYNAMIC_SCRIPTS: OnceLock<Mutex<HashMap<String, ScriptMetadata>>> = OnceLock::new();

/// Safe mutex access with recovery from poisoned state
pub fn safe_lock_scripts()
-> AppResult<std::sync::MutexGuard<'static, HashMap<String, ScriptMetadata>>> {
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

/// Get database pool if available
pub fn get_db_pool() -> Option<std::sync::Arc<crate::database::Database>> {
    if let Some(db) = crate::database::get_global_database() {
        return Some(db);
    }

    // Fallback: try to get pool from GLOBAL_REPOSITORY
    if let Some(repo) = GLOBAL_REPOSITORY.get() {
        return Some(std::sync::Arc::new(crate::database::Database::from_pool(
            repo.pool.clone(),
        )));
    }

    None
}

/// Drop the caches that depend on one of a script's assets: the compiled
/// bytecode (keyed by root URI), the prepared program bundle, and the cached
/// source of the asset that changed. The script's *other* module sources stay
/// cached, so the rebuild re-reads one asset rather than all of them.
fn invalidate_script_asset_caches(script_uri: &str, asset_path: &str) {
    crate::bytecode::invalidate(script_uri);
    crate::module_loader::invalidate_asset(script_uri, asset_path);
}

/// Point the metadata cache at `content` without disturbing the script's route
/// registrations, so requests keep routing while the re-init that follows an
/// upsert runs. Does nothing when the script is not cached — the next
/// `get_script_metadata` loads it from the database.
fn refresh_cached_script_source(uri: &str, content: &str) {
    if let Ok(mut guard) = safe_lock_scripts()
        && let Some(metadata) = guard.get_mut(uri)
    {
        metadata.update_content(content.to_string());
    }
}

/// Re-read a script's host bindings into its cached metadata.
///
/// The cache is what the route index and the host filters are built from, so
/// an instance that learns of a binding change from another one has to refresh
/// this or keep publishing the script where it used to be.
pub async fn refresh_cached_script_hosts_from_db(uri: &str) {
    let repo = get_repository();
    match repo.get_script_hosts(uri).await {
        Ok(script_hosts) => {
            if let Ok(mut guard) = safe_lock_scripts()
                && let Some(metadata) = guard.get_mut(uri)
            {
                metadata.hosts = script_hosts;
            }
        }
        Err(e) => {
            // Evict rather than keep a binding we can no longer confirm.
            warn!(
                "Could not refresh host bindings for {} ({}); dropping its cached metadata",
                uri, e
            );
            if let Ok(mut guard) = safe_lock_scripts() {
                guard.remove(uri);
            }
        }
    }
}

/// [`refresh_cached_script_source`] for callers that do not hold the new source
/// — a script upserted on another cluster instance, where the update arrives as
/// a notification and only the database has the new content.
///
/// Falls back to evicting the entry if the new source cannot be read: serving a
/// stale *source* is a correctness bug, while losing the registrations only
/// costs the routes until the pending init() restores them.
pub async fn refresh_cached_script_source_from_db(uri: &str) {
    let repo = get_repository();
    match repo.get_script(uri).await {
        Ok(Some(content)) => refresh_cached_script_source(uri, &content),
        Ok(None) | Err(_) => {
            if let Ok(mut guard) = safe_lock_scripts() {
                guard.remove(uri);
            }
        }
    }
}

async fn send_script_notification(
    pool: &PgPool,
    uri: &str,
    action: &str,
    server_id: &str,
) -> AppResult<()> {
    let channel = match action {
        "upserted" => "script_upserted",
        "deleted" => "script_deleted",
        _ => return Ok(()), // Unknown action, skip notification
    };

    // Create notification payload
    let payload = serde_json::json!({
        "uri": uri,
        "action": action,
        "timestamp": chrono::Utc::now().timestamp(),
        "server_id": server_id,
    });

    let payload_str = payload.to_string();

    // Send notification using pg_notify
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(channel)
        .bind(&payload_str)
        .execute(pool)
        .await
        .map_err(|e| {
            error!("Failed to send {} notification for {}: {}", action, uri, e);
            AppError::Database {
                message: format!("Failed to send notification: {}", e),
                source: None,
            }
        })?;

    debug!("Sent {} notification for script: {}", action, uri);
    Ok(())
}

/// Database-backed upsert script
async fn db_upsert_script(
    mut executor: crate::database::TransactionExecutor<'_>,
    uri: &str,
    content: &str,
) -> AppResult<()> {
    debug!(
        "db_upsert_script called: uri={}, content_len={}",
        uri,
        content.len()
    );
    let now = chrono::Utc::now();

    // Extract name from URI (last segment after /)
    let name = uri.rsplit('/').next().unwrap_or(uri);

    // Try to update existing script
    debug!("Attempting to UPDATE script: uri={}", uri);
    let update_result = match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(
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
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(
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
        }
    }
    .map_err(|e| {
        error!("Database error updating script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let rows_affected = update_result.rows_affected();
    debug!(
        "UPDATE result: uri={}, rows_affected={}",
        uri, rows_affected
    );

    if rows_affected > 0 {
        debug!(
            "✓ Successfully updated existing script in database: {}",
            uri
        );
        return Ok(());
    }

    // Script doesn't exist, create new one
    debug!(
        "Script not found for update, attempting INSERT: uri={}",
        uri
    );
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
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
            .execute(&mut ***tx)
            .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
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
        }
    }
    .map_err(|e| {
        error!("Database error creating script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!("✓ Successfully created new script in database: {}", uri);
    Ok(())
}

/// Database-backed get script
async fn db_get_script<'e, E>(executor: E, uri: &str) -> AppResult<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT content FROM scripts WHERE uri = $1
        "#,
    )
    .bind(uri)
    .fetch_optional(executor)
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
async fn db_list_scripts<'e, E>(executor: E) -> AppResult<HashMap<String, String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT uri, content FROM scripts ORDER BY uri
        "#,
    )
    .fetch_all(executor)
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
async fn db_delete_script<'e, E>(executor: E, uri: &str) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM scripts WHERE uri = $1
        "#,
    )
    .bind(uri)
    .execute(executor)
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

/// Database-backed add script owner
async fn db_add_script_owner<'e, E>(executor: E, uri: &str, user_id: &str) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    // Insert ownership record (ignore if already exists)
    sqlx::query(
        r#"
        INSERT INTO script_owners (script_uri, user_id, created_at)
        VALUES ($1, $2, NOW())
        ON CONFLICT (script_uri, user_id) DO NOTHING
        "#,
    )
    .bind(uri)
    .bind(user_id)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error adding script owner: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!("Added owner {} to script {}", user_id, uri);
    Ok(())
}

/// Database-backed remove script owner
async fn db_remove_script_owner<'e, E>(executor: E, uri: &str, user_id: &str) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM script_owners WHERE script_uri = $1 AND user_id = $2
        "#,
    )
    .bind(uri)
    .bind(user_id)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error removing script owner: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!("Removed owner {} from script {}", user_id, uri);
    } else {
        debug!("Owner {} was not found for script {}", user_id, uri);
    }

    Ok(existed)
}

/// Database-backed get script owners
async fn db_get_script_owners<'e, E>(executor: E, uri: &str) -> AppResult<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT user_id
        FROM script_owners
        WHERE script_uri = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(uri)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting script owners: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let owners = rows
        .into_iter()
        .map(|row| {
            row.try_get("user_id").map_err(|e| {
                error!("Database error parsing user_id: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })
        })
        .collect::<Result<Vec<String>, AppError>>()?;

    Ok(owners)
}

/// Database-backed check if user owns script
async fn db_user_owns_script<'e, E>(executor: E, uri: &str, user_id: &str) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM script_owners
            WHERE script_uri = $1 AND user_id = $2
        ) as owns
        "#,
    )
    .bind(uri)
    .bind(user_id)
    .fetch_one(executor)
    .await
    .map_err(|e| {
        error!("Database error checking script ownership: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let owns: bool = row.try_get("owns").map_err(|e| {
        error!("Database error parsing ownership check: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    Ok(owns)
}

/// Database-backed count script owners
async fn db_count_script_owners<'e, E>(executor: E, uri: &str) -> AppResult<i64>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) as count
        FROM script_owners
        WHERE script_uri = $1
        "#,
    )
    .bind(uri)
    .fetch_one(executor)
    .await
    .map_err(|e| {
        error!("Database error counting script owners: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let count: i64 = row.try_get("count").map_err(|e| {
        error!("Database error parsing owner count: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    Ok(count)
}

/// Database-backed get all script owners (returns HashMap of uri -> owners)
async fn db_get_all_script_owners<'e, E>(executor: E) -> AppResult<HashMap<String, Vec<String>>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT script_uri, user_id
        FROM script_owners
        ORDER BY script_uri, created_at ASC
        "#,
    )
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting all script owners: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut owners_map: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("script_uri").map_err(|e| {
            error!("Database error parsing script_uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let user_id: String = row.try_get("user_id").map_err(|e| {
            error!("Database error parsing user_id: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

        owners_map.entry(uri).or_default().push(user_id);
    }

    Ok(owners_map)
}

/// Database-backed get the hosts a script is bound to
async fn db_get_script_hosts<'e, E>(executor: E, uri: &str) -> AppResult<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT host
        FROM script_hosts
        WHERE script_uri = $1
        ORDER BY host ASC
        "#,
    )
    .bind(uri)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting script hosts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    rows.into_iter()
        .map(|row| {
            row.try_get("host").map_err(|e| {
                error!("Database error parsing host: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })
        })
        .collect()
}

/// Database-backed get all script host bindings (uri -> hosts)
async fn db_get_all_script_hosts<'e, E>(executor: E) -> AppResult<HashMap<String, Vec<String>>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT script_uri, host
        FROM script_hosts
        ORDER BY script_uri, host ASC
        "#,
    )
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting all script hosts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut hosts_map: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("script_uri").map_err(|e| {
            error!("Database error parsing script_uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let host: String = row.try_get("host").map_err(|e| {
            error!("Database error parsing host: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        hosts_map.entry(uri).or_default().push(host);
    }

    Ok(hosts_map)
}

/// Database-backed replace of a script's host bindings.
///
/// Replaces rather than merges: the caller states the complete set, so an
/// empty list clears the bindings and returns the script to the default host.
async fn db_set_script_hosts(
    mut executor: crate::database::TransactionExecutor<'_>,
    uri: &str,
    hosts: &[String],
) -> AppResult<()> {
    let delete = sqlx::query("DELETE FROM script_hosts WHERE script_uri = $1").bind(uri);
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            delete.execute(&mut ***tx).await
        }
        crate::database::TransactionExecutor::Pool(pool) => delete.execute(pool).await,
    }
    .map_err(|e| {
        error!("Database error clearing script hosts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    for host in hosts {
        let insert = sqlx::query(
            r#"
            INSERT INTO script_hosts (script_uri, host, created_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (script_uri, host) DO NOTHING
            "#,
        )
        .bind(uri)
        .bind(host);

        match executor {
            crate::database::TransactionExecutor::Transaction(ref mut tx) => {
                insert.execute(&mut ***tx).await
            }
            crate::database::TransactionExecutor::Pool(pool) => insert.execute(pool).await,
        }
        .map_err(|e| {
            error!("Database error inserting script host: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
    }

    Ok(())
}

/// Database-backed set shared storage item
async fn db_set_script_properties_item(
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
async fn db_get_script_properties_item<'e, E>(
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
async fn db_remove_script_properties_item<'e, E>(
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
async fn db_clear_script_properties<'e, E>(executor: E, script_uri: &str) -> AppResult<()>
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

/// Database-backed set personal storage item
async fn db_set_user_properties_item(
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
async fn db_get_user_properties_item<'e, E>(
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
async fn db_remove_user_properties_item<'e, E>(
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
async fn db_clear_user_properties<'e, E>(
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

/// Database-backed set script secret item
async fn db_set_script_secret(
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
async fn db_get_script_secret<'e, E>(
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
async fn db_remove_script_secret<'e, E>(executor: E, script_uri: &str, key: &str) -> AppResult<bool>
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
async fn db_clear_script_secrets<'e, E>(executor: E, script_uri: &str) -> AppResult<()>
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

/// Database-backed list script secret keys for a script
async fn db_list_script_properties_keys<'e, E>(
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
        error!("Database error listing shared storage keys: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Listed {} shared storage keys from database for script: {}",
        rows.len(),
        script_uri
    );
    Ok(rows)
}

async fn db_list_user_properties_keys<'e, E>(
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

async fn db_list_script_secrets<'e, E>(executor: E, script_uri: &str) -> AppResult<Vec<String>>
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
async fn db_set_user_secret(
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
async fn db_get_user_secret<'e, E>(
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
async fn db_remove_user_secret<'e, E>(
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
async fn db_clear_user_secrets<'e, E>(executor: E, script_uri: &str, user_id: &str) -> AppResult<()>
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

/// Database-backed insert log message
async fn db_insert_log_message<'e, E>(
    executor: E,
    script_uri: &str,
    message: &str,
    log_level: &str,
    context: &LogContext,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        // `clock_timestamp()`, not `NOW()`: `NOW()` is the *transaction's*
        // start time, so every line a script wrote inside one transaction —
        // which is how `/engine/eval` and the test runner execute — would carry
        // the same timestamp and lose its order.
        r#"
        INSERT INTO logs (script_uri, message, log_level, created_at, request_id, kind, route)
        VALUES ($1, $2, $3, clock_timestamp(), $4, $5, $6)
        "#,
    )
    .bind(script_uri)
    .bind(message)
    .bind(log_level)
    .bind(context.request_id.as_deref())
    .bind(context.kind.as_deref())
    .bind(context.route.as_deref())
    .execute(executor)
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

/// Database-backed log query. Filters are applied in SQL so callers never pull
/// the whole table just to throw most of it away, and `limit` keeps the
/// *newest* matching entries — rows come back newest-first.
///
/// With `after_seq` the limit works the other way round, keeping the *oldest*
/// entries past the cursor: a caller reading forward wants the next page after
/// what it has, and keeping the newest would silently skip everything in
/// between. Rows still come back newest-first either way.
async fn db_query_log_messages<'e, E>(executor: E, query: &LogQuery) -> AppResult<Vec<LogEntry>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    // Levels are stored upper-case; normalise so `?level=error` matches.
    let level = query.level.as_ref().map(|l| l.to_uppercase());
    let since = query.since.map(DateTime::<Utc>::from);
    // `contains` is a literal substring, not a pattern: escape what LIKE would
    // otherwise read as wildcards so searching for `a_b` or `100%` works.
    let contains = query.contains.as_ref().map(|needle| {
        format!(
            "%{}%",
            needle
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });

    // A NULL bind disables the corresponding filter, and `LIMIT NULL` means
    // "no limit" in Postgres, so one statement covers every combination.
    let rows = sqlx::query(
        // `seq` breaks `created_at` ties, so a listing has one stable order
        // rather than an arbitrary one per query — which is what makes paging
        // and tailing by `seq` reliable.
        r#"
        WITH matching AS (
            SELECT script_uri, message, log_level, created_at, seq, request_id, kind, route
            FROM logs
            WHERE ($1::text IS NULL OR script_uri = $1)
              AND ($2::text IS NULL OR log_level = $2)
              AND ($3::timestamptz IS NULL OR created_at >= $3)
              AND ($4::text IS NULL OR message ILIKE $4)
              AND ($5::text IS NULL OR request_id = $5)
              AND ($6::text IS NULL OR lower(kind) = lower($6))
              AND ($7::text IS NULL OR route = $7)
              AND ($8::bigint IS NULL OR seq > $8)
            -- Without a cursor the constant leaves the ordering to the keys
            -- that follow, so the limit keeps the newest entries; with one it
            -- orders oldest-first, so the limit keeps the next page instead.
            ORDER BY
              CASE WHEN $8::bigint IS NULL THEN 0 ELSE seq END ASC,
              created_at DESC,
              seq DESC
            LIMIT $9::bigint
        )
        SELECT * FROM matching ORDER BY created_at DESC, seq DESC
        "#,
    )
    .bind(query.script_uri.as_deref())
    .bind(level.as_deref())
    .bind(since)
    .bind(contains.as_deref())
    .bind(query.request_id.as_deref())
    .bind(query.kind.as_deref())
    .bind(query.route.as_deref())
    .bind(query.after_seq)
    .bind(query.limit)
    .fetch_all(executor)
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
            let script_uri: String = row.try_get("script_uri")?;
            let message: String = row.try_get("message")?;
            let log_level: String = row.try_get("log_level")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            let seq: i64 = row.try_get("seq")?;
            let context = LogContext {
                request_id: row.try_get("request_id")?,
                kind: row.try_get("kind")?,
                route: row.try_get("route")?,
            };
            // Convert chrono DateTime to SystemTime
            let system_time = SystemTime::from(created_at);
            Ok(LogEntry::new(script_uri, message, log_level, system_time)
                .with_context(seq, context))
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

/// Database-backed fetch log messages for a script, oldest first.
async fn db_fetch_log_messages<'e, E>(executor: E, script_uri: &str) -> AppResult<Vec<LogEntry>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let mut messages = db_query_log_messages(executor, &LogQuery::for_uri(script_uri)).await?;
    messages.reverse();
    Ok(messages)
}

/// Database-backed fetch all log messages, newest first.
async fn db_fetch_all_log_messages<'e, E>(executor: E) -> AppResult<Vec<LogEntry>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    db_query_log_messages(executor, &LogQuery::default()).await
}

/// Database-backed clear log messages for a script
async fn db_clear_log_messages<'e, E>(executor: E, script_uri: &str) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        r#"
        DELETE FROM logs WHERE script_uri = $1
        "#,
    )
    .bind(script_uri)
    .execute(executor)
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
async fn db_prune_log_messages<'e, E>(executor: E) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
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
    .execute(executor)
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
async fn db_upsert_asset(
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
async fn db_get_asset<'e, E>(executor: E, script_uri: &str, uri: &str) -> AppResult<Option<Asset>>
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
async fn db_list_assets<'e, E>(executor: E, script_uri: &str) -> AppResult<HashMap<String, Asset>>
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
async fn db_delete_asset<'e, E>(executor: E, script_uri: &str, uri: &str) -> AppResult<bool>
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

// ============================================================================
// Script Database Schema Management Functions
// ============================================================================

use crate::db_schema_utils::{
    ColumnType, MAX_COLUMNS_PER_TABLE, MAX_TABLES_PER_SCRIPT, generate_physical_table_name,
    quote_identifier, validate_default_value, validate_identifier,
};

/// Names each savepoint a [`ScopedConn`] brackets an operation with.
static SCHEMA_SAVEPOINT_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The connection a script's database operation runs on, and the scope that
/// undoes it if it fails.
///
/// Whatever opened it, the rule is the same: an operation that fails inside
/// the caller's transaction must not take the transaction with it. Postgres
/// aborts a transaction on any error, so without a savepoint one bad statement
/// discards every write made before it — a scheduled tick's unrelated work
/// included — and the script's `try`/`catch` catches an error it can no longer
/// do anything about.
///
/// Schema work must never run on a second connection while the caller holds a
/// transaction. `CREATE INDEX` takes SHARE and `ALTER TABLE` takes ACCESS
/// EXCLUSIVE, and both conflict with the ROW EXCLUSIVE the caller's own writes
/// already hold on the table. Postgres cannot see that as a deadlock — the
/// holder is not waiting on the database, it is waiting on the engine to finish
/// the request — so its detector never fires, the statement blocks until the
/// connection dies, and every later writer queues behind the pending strong
/// lock. A script calling `ensureSchema()` inside `transaction()` was enough to
/// wedge a table for every instance in the cluster.
///
/// Joining the caller's transaction makes that conflict impossible: a
/// connection never blocks on locks it already holds.
///
/// Inside a transaction the operation is bracketed by a savepoint, so a failure
/// — `table already exists`, an invalid column type — leaves the caller's
/// transaction usable instead of aborting it. That is what a script wrapping an
/// ensure-schema step in `try`/`catch` expects, and what running on a separate
/// connection used to give it for free.
///
/// Outside one, the operation gets a transaction of its own. These are
/// multi-statement units — `CREATE TABLE` plus the `script_tables` row that
/// records it — and running them in autocommit leaves a physical table with no
/// metadata behind whenever the second statement fails.
///
/// Every path must end at [`ScopedConn::finish`], which releases the savepoint
/// or commits, and undoes either one if the operation failed.
enum ScopedConn<'a> {
    /// Bracketing the caller's transaction, which this must leave open.
    Savepoint {
        tx: &'a mut sqlx::Transaction<'static, sqlx::Postgres>,
        savepoint: String,
    },
    /// A transaction of this operation's own, to be committed or rolled back.
    Owned(sqlx::Transaction<'a, sqlx::Postgres>),
    /// A pooled connection in autocommit, for an operation that is one
    /// statement and has no caller's transaction to protect.
    Pooled(sqlx::pool::PoolConnection<sqlx::Postgres>),
}

impl<'a> ScopedConn<'a> {
    /// Opens the savepoint every scope shares when a transaction is active.
    async fn savepoint_in(
        tx: &'a mut sqlx::Transaction<'static, sqlx::Postgres>,
    ) -> AppResult<Self> {
        let savepoint = format!(
            "aiwe_scope_{}",
            SCHEMA_SAVEPOINT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        sqlx::query(sqlx::AssertSqlSafe(format!("SAVEPOINT {}", savepoint)))
            .execute(&mut **tx)
            .await
            .map_err(|e| schema_transaction_error("opening a savepoint", e))?;
        Ok(ScopedConn::Savepoint { tx, savepoint })
    }

    /// A connection for a schema operation.
    ///
    /// Outside a transaction the operation gets one of its own, because these
    /// are multi-statement units — `CREATE TABLE` plus the `script_tables` row
    /// that records it — and running them in autocommit leaves a physical
    /// table with no metadata behind whenever the second statement fails.
    async fn for_schema(pool: &'a PgPool) -> AppResult<Self> {
        match crate::database::get_current_executor(pool) {
            crate::database::TransactionExecutor::Transaction(tx) => Self::savepoint_in(tx).await,
            crate::database::TransactionExecutor::Pool(pool) => {
                let tx = pool
                    .begin()
                    .await
                    .map_err(|e| schema_transaction_error("opening a schema transaction", e))?;
                Ok(ScopedConn::Owned(tx))
            }
        }
    }

    /// A connection for a schema operation on one named table, serialised
    /// against every other engine instance doing the same.
    ///
    /// The existence checks these operations start with are worthless
    /// concurrently: two handlers calling `ensureSchema()` on a cold cache both
    /// read "no such table" and both go on to create it. The loser gets
    /// Postgres's own `relation already exists` rather than the engine's
    /// answer, and between the two statements each of these operations makes
    /// there is room for worse — a physical table with no `script_tables` row
    /// to find it by.
    ///
    /// The advisory lock is keyed on the script and table, so two scripts, or
    /// one script's two tables, never wait on each other. It is held for the
    /// transaction rather than the savepoint, which means until the caller's
    /// transaction ends — the same scope the schema change itself commits in,
    /// and the only scope at which "did this table exist" stays true.
    async fn for_schema_of(
        pool: &'a PgPool,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Self> {
        let mut scope = Self::for_schema(pool).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1), hashtext($2))")
            .bind(script_uri)
            .bind(logical_table_name)
            .execute(scope.conn())
            .await
            .map_err(|e| schema_transaction_error("taking the schema lock", e))?;
        Ok(scope)
    }

    /// A connection for a single data statement — a row read or write.
    ///
    /// Inside a transaction it is bracketed like any other operation, which is
    /// what makes a failed write survivable: a duplicate key, a value the
    /// column will not take, a timestamp Postgres cannot parse. The script
    /// catches the error and the transaction it is standing in remains usable.
    /// The bracket costs two round trips, which is the price of not losing the
    /// rest of a transaction to one failed statement.
    ///
    /// Outside a transaction there is nothing to protect: a lone statement in
    /// autocommit is already its own unit, and wrapping it would only add
    /// round trips.
    async fn for_statement(pool: &'a PgPool) -> AppResult<Self> {
        match crate::database::get_current_executor(pool) {
            crate::database::TransactionExecutor::Transaction(tx) => Self::savepoint_in(tx).await,
            crate::database::TransactionExecutor::Pool(pool) => {
                let conn = pool.acquire().await.map_err(|e| AppError::Database {
                    message: format!("Failed to acquire connection: {}", e),
                    source: None,
                })?;
                Ok(ScopedConn::Pooled(conn))
            }
        }
    }

    fn conn(&mut self) -> &mut PgConnection {
        match self {
            ScopedConn::Savepoint { tx, .. } => tx,
            ScopedConn::Owned(tx) => tx,
            ScopedConn::Pooled(conn) => conn,
        }
    }

    /// Closes the bracket around `outcome` and returns it unchanged.
    ///
    /// The operation's own error is the one worth reporting, so undoing a
    /// failed operation is best effort — but recovering the caller's
    /// transaction is not optional, which is why the rollback runs before the
    /// error is handed back.
    async fn finish<T>(self, outcome: AppResult<T>) -> AppResult<T> {
        match self {
            ScopedConn::Savepoint { tx, savepoint } => {
                let verb = if outcome.is_ok() {
                    "RELEASE SAVEPOINT"
                } else {
                    "ROLLBACK TO SAVEPOINT"
                };
                let closed = sqlx::query(sqlx::AssertSqlSafe(format!("{} {}", verb, savepoint)))
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| schema_transaction_error("closing a savepoint", e));
                match outcome {
                    Ok(value) => closed.map(|_| value),
                    Err(operation_error) => Err(operation_error),
                }
            }
            ScopedConn::Pooled(_) => outcome,
            ScopedConn::Owned(tx) => match outcome {
                Ok(value) => {
                    tx.commit().await.map_err(|e| {
                        schema_transaction_error("committing a schema transaction", e)
                    })?;
                    Ok(value)
                }
                Err(operation_error) => {
                    if let Err(e) = tx.rollback().await {
                        error!("Database error rolling back a schema transaction: {}", e);
                    }
                    Err(operation_error)
                }
            },
        }
    }
}

fn schema_transaction_error(what: &str, e: sqlx::Error) -> AppError {
    error!("Database error {}: {}", what, e);
    AppError::Database {
        message: format!("Database error: {}", e),
        source: None,
    }
}

/// Database-backed create script-owned table
async fn db_create_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<String> {
    // Validate the logical table name
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;

    // Check table limit for this script
    let table_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM script_tables WHERE script_uri = $1")
            .bind(script_uri)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error counting tables for script: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;

    if table_count >= MAX_TABLES_PER_SCRIPT as i64 {
        return Err(AppError::Validation {
            field: "table_name".to_string(),
            reason: format!(
                "Script has reached maximum table limit of {}",
                MAX_TABLES_PER_SCRIPT
            ),
        });
    }

    // Generate physical table name
    let physical_table_name = generate_physical_table_name(script_uri, logical_table_name);

    // Check if table already exists for this script
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2)",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error checking table existence: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if exists {
        return Err(AppError::Validation {
            field: "table_name".to_string(),
            reason: format!(
                "Table '{}' already exists for this script",
                logical_table_name
            ),
        });
    }

    // Create the physical table with id column
    let create_table_sql = format!(
        "CREATE TABLE {} (id SERIAL PRIMARY KEY)",
        quote_identifier(&physical_table_name)
    );

    sqlx::query(sqlx::AssertSqlSafe(create_table_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error creating table: {}", e);
            AppError::Database {
                message: format!("Failed to create table: {}", e),
                source: None,
            }
        })?;

    // Record the table in script_tables metadata
    let schema_json = serde_json::json!({
        "columns": [
            {"name": "id", "type": "SERIAL", "nullable": false, "primary_key": true}
        ]
    });

    sqlx::query(
        r#"
        INSERT INTO script_tables (script_uri, logical_table_name, physical_table_name, schema_json)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .bind(&physical_table_name)
    .bind(schema_json)
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error recording table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Created script table: {} -> {}",
        logical_table_name, physical_table_name
    );

    Ok(physical_table_name)
}

/// Brings a script-owned table to the shape `spec` describes.
///
/// Every solution ends up writing this by hand — create the table, add the
/// columns, add the indexes, catch and ignore the "already exists" from each —
/// and every hand-written version has the same two faults. It runs on the
/// request path, where its `ALTER TABLE`s meet whatever else is holding the
/// table; and it treats an error as success because the common error means
/// "already done", which hides the ones that mean something else.
///
/// Here it is one call: one advisory lock, one transaction, and a check before
/// each step instead of an exception after it. Doing nothing is the normal
/// outcome and costs one query.
async fn db_ensure_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    spec: &TableSpec,
) -> AppResult<EnsuredTable> {
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;

    let mut outcome = EnsuredTable::default();

    let existing: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error reading table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut present: Vec<String> = match &existing {
        Some(schema_json) => schema_json
            .get("columns")
            .and_then(|columns| columns.as_array())
            .map(|columns| {
                columns
                    .iter()
                    .filter_map(|column| column.get("name")?.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        None => {
            db_create_script_table(&mut *conn, script_uri, logical_table_name).await?;
            outcome.created = true;
            vec!["id".to_string()]
        }
    };

    for column in &spec.columns {
        if present.iter().any(|name| name == &column.name) {
            continue;
        }
        db_add_column_to_script_table(
            &mut *conn,
            script_uri,
            logical_table_name,
            &column.name,
            column.column_type.clone(),
            column.nullable,
            column.default_value.as_deref(),
        )
        .await?;
        present.push(column.name.clone());
        outcome.columns_added.push(column.name.clone());
    }

    for columns in &spec.unique_indexes {
        db_add_unique_index(&mut *conn, script_uri, logical_table_name, columns).await?;
        outcome.unique_indexes_ensured.push(columns.clone());
    }

    Ok(outcome)
}

/// Database-backed add column to script-owned table
async fn db_add_column_to_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    column_type: ColumnType,
    nullable: bool,
    default_value: Option<&str>,
) -> AppResult<()> {
    // Validate identifiers
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(column_name).map_err(|e| AppError::Validation {
        field: "column_name".to_string(),
        reason: e.to_string(),
    })?;

    // Get the physical table name
    let row: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT physical_table_name, schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let (physical_table_name, mut schema_json) = row.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Check column limit
    let column_count = schema_json
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|c| c.len())
        .unwrap_or(0);

    if column_count >= MAX_COLUMNS_PER_TABLE {
        return Err(AppError::Validation {
            field: "column_name".to_string(),
            reason: format!(
                "Table has reached maximum column limit of {}",
                MAX_COLUMNS_PER_TABLE
            ),
        });
    }

    // Check if column already exists
    if let Some(columns) = schema_json.get("columns").and_then(|c| c.as_array())
        && columns
            .iter()
            .any(|col| col.get("name").and_then(|n| n.as_str()) == Some(column_name))
    {
        return Err(AppError::Validation {
            field: "column_name".to_string(),
            reason: format!(
                "Column '{}' already exists in table '{}'",
                column_name, logical_table_name
            ),
        });
    }

    // Build ALTER TABLE statement
    let mut alter_sql = format!(
        "ALTER TABLE {} ADD COLUMN {} {}",
        quote_identifier(&physical_table_name),
        quote_identifier(column_name),
        column_type.to_sql()
    );

    if !nullable {
        alter_sql.push_str(" NOT NULL");
    }

    if let Some(default) = default_value {
        let validated_default =
            validate_default_value(&column_type, default).map_err(|e| AppError::Validation {
                field: "default_value".to_string(),
                reason: e.to_string(),
            })?;
        alter_sql.push_str(&format!(" DEFAULT {}", validated_default));
    }

    // Execute the ALTER TABLE
    sqlx::query(sqlx::AssertSqlSafe(alter_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error adding column: {}", e);
            AppError::Database {
                message: format!("Failed to add column: {}", e),
                source: None,
            }
        })?;

    // Update schema_json metadata
    if let Some(columns) = schema_json
        .get_mut("columns")
        .and_then(|c| c.as_array_mut())
    {
        columns.push(serde_json::json!({
            "name": column_name,
            "type": column_type.to_sql(),
            "nullable": nullable,
            "default": default_value,
        }));
    }

    sqlx::query("UPDATE script_tables SET schema_json = $1, updated_at = NOW() WHERE script_uri = $2 AND logical_table_name = $3")
        .bind(schema_json)
        .bind(script_uri)
        .bind(logical_table_name)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error updating schema metadata: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    debug!(
        "Added column {} to table {}: {} {}",
        column_name,
        logical_table_name,
        column_type.to_sql(),
        if nullable { "NULL" } else { "NOT NULL" }
    );

    Ok(())
}

/// Database-backed add reference column (creates INTEGER column with FK constraint)
async fn db_add_reference_column(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    referenced_logical_table_name: &str,
    nullable: bool,
) -> AppResult<()> {
    // Validate identifiers
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(column_name).map_err(|e| AppError::Validation {
        field: "column_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(referenced_logical_table_name).map_err(|e| AppError::Validation {
        field: "referenced_table_name".to_string(),
        reason: e.to_string(),
    })?;

    // First, add the integer column
    db_add_column_to_script_table(
        &mut *conn,
        script_uri,
        logical_table_name,
        column_name,
        ColumnType::Integer,
        nullable,
        None,
    )
    .await?;

    // Get physical table names for FK constraint
    let source_table: String = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching source table: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?
    .ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    let referenced_table: String = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(referenced_logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching referenced table: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?
    .ok_or_else(|| AppError::Validation {
        field: "referenced_table_name".to_string(),
        reason: format!(
            "Referenced table '{}' not found for this script",
            referenced_logical_table_name
        ),
    })?;

    // Create the foreign key constraint
    let constraint_name = format!("fk_{}_{}", logical_table_name.replace("_", ""), column_name);
    let alter_sql = format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} (id)",
        quote_identifier(&source_table),
        quote_identifier(&constraint_name),
        quote_identifier(column_name),
        quote_identifier(&referenced_table)
    );

    sqlx::query(sqlx::AssertSqlSafe(alter_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error creating foreign key: {}", e);
            AppError::Database {
                message: format!("Failed to create foreign key: {}", e),
                source: None,
            }
        })?;

    debug!(
        "Created reference column: {}.{} -> {}.id (nullable: {})",
        logical_table_name, column_name, referenced_logical_table_name, nullable
    );

    Ok(())
}

/// Database-backed drop column from script-owned table
async fn db_drop_column(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
) -> AppResult<bool> {
    // Validate identifiers
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(column_name).map_err(|e| AppError::Validation {
        field: "column_name".to_string(),
        reason: e.to_string(),
    })?;

    // Get the physical table name and schema
    let row: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT physical_table_name, schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let (physical_table_name, mut schema_json) = row.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Check if column exists in schema
    let column_exists = schema_json
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|columns| {
            columns
                .iter()
                .any(|col| col.get("name").and_then(|n| n.as_str()) == Some(column_name))
        })
        .unwrap_or(false);

    if !column_exists {
        return Ok(false);
    }

    // Don't allow dropping the id column
    if column_name == "id" {
        return Err(AppError::Validation {
            field: "column_name".to_string(),
            reason: "Cannot drop the 'id' column".to_string(),
        });
    }

    // Drop the column
    let drop_sql = format!(
        "ALTER TABLE {} DROP COLUMN {}",
        quote_identifier(&physical_table_name),
        quote_identifier(column_name)
    );

    sqlx::query(sqlx::AssertSqlSafe(drop_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error dropping column: {}", e);
            AppError::Database {
                message: format!("Failed to drop column: {}", e),
                source: None,
            }
        })?;

    // Update schema_json metadata
    if let Some(columns) = schema_json
        .get_mut("columns")
        .and_then(|c| c.as_array_mut())
    {
        columns.retain(|col| col.get("name").and_then(|n| n.as_str()) != Some(column_name));
    }

    sqlx::query(
        "UPDATE script_tables SET schema_json = $1, updated_at = NOW() WHERE script_uri = $2 AND logical_table_name = $3",
    )
    .bind(schema_json)
    .bind(script_uri)
    .bind(logical_table_name)
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error updating schema metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Dropped column {} from table {}",
        column_name, logical_table_name
    );

    Ok(true)
}

/// Database-backed drop script-owned table
async fn db_drop_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<bool> {
    // Validate identifier
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;

    // Get the physical table name
    let physical_table_name: Option<String> = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(physical_name) = physical_table_name {
        // Drop the physical table
        let drop_sql = format!(
            "DROP TABLE IF EXISTS {} CASCADE",
            quote_identifier(&physical_name)
        );

        sqlx::query(sqlx::AssertSqlSafe(drop_sql.as_str()))
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error dropping table: {}", e);
                AppError::Database {
                    message: format!("Failed to drop table: {}", e),
                    source: None,
                }
            })?;

        // Remove from script_tables metadata (will cascade due to FK)
        sqlx::query("DELETE FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2")
            .bind(script_uri)
            .bind(logical_table_name)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error removing table metadata: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;

        debug!(
            "Dropped script table: {} ({})",
            logical_table_name, physical_name
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Database-backed drop all tables for a script
async fn db_drop_all_script_tables(conn: &mut PgConnection, script_uri: &str) -> AppResult<usize> {
    // Get all tables for this script
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT physical_table_name FROM script_tables WHERE script_uri = $1")
            .bind(script_uri)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error fetching script tables: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;

    let count = tables.len();

    // Drop each table
    for physical_name in tables {
        let drop_sql = format!(
            "DROP TABLE IF EXISTS {} CASCADE",
            quote_identifier(&physical_name)
        );

        sqlx::query(sqlx::AssertSqlSafe(drop_sql.as_str()))
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error dropping table {}: {}", physical_name, e);
                AppError::Database {
                    message: format!("Failed to drop table: {}", e),
                    source: None,
                }
            })?;
    }

    // Delete metadata entries (script_uri FK will auto-delete on script deletion)
    sqlx::query("DELETE FROM script_tables WHERE script_uri = $1")
        .bind(script_uri)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error removing table metadata: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    if count > 0 {
        debug!("Dropped {} tables for script {}", count, script_uri);
    }

    Ok(count)
}

// ============================================================================
// Script Database Schema Introspection Functions
// ============================================================================

/// List all tables owned by a script
async fn db_list_script_tables(
    conn: &mut PgConnection,
    script_uri: &str,
) -> AppResult<Vec<TableInfo>> {
    let rows = sqlx::query!(
        r#"
        SELECT logical_table_name, physical_table_name, created_at
        FROM script_tables
        WHERE script_uri = $1
        ORDER BY logical_table_name
        "#,
        script_uri
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error listing script tables: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    Ok(rows
        .into_iter()
        .map(|row| TableInfo {
            logical_name: row.logical_table_name,
            physical_name: row.physical_table_name,
            created_at: row.created_at,
        })
        .collect())
}

/// Get detailed schema information for a specific table
async fn db_get_table_schema(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<TableSchema> {
    // Fetch schema_json from script_tables
    let schema_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table schema: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let schema_json = schema_json.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Parse columns from schema_json
    let columns_array = schema_json
        .get("columns")
        .and_then(|c| c.as_array())
        .ok_or_else(|| AppError::Validation {
            field: "schema_json".to_string(),
            reason: "Invalid schema format: missing columns array".to_string(),
        })?;

    let mut columns = Vec::new();
    for col in columns_array {
        let name = col
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| AppError::Validation {
                field: "column".to_string(),
                reason: "Column missing name".to_string(),
            })?
            .to_string();

        let data_type = col
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| AppError::Validation {
                field: "column".to_string(),
                reason: format!("Column '{}' missing type", name),
            })?
            .to_string();

        let nullable = col
            .get("nullable")
            .and_then(|n| n.as_bool())
            .unwrap_or(true);

        let default_value = col.get("default").map(|d| {
            if let Some(s) = d.as_str() {
                s.to_string()
            } else {
                d.to_string()
            }
        });

        let is_primary_key = col
            .get("primary_key")
            .and_then(|p| p.as_bool())
            .unwrap_or(false);

        columns.push(ColumnInfo {
            name,
            data_type,
            nullable,
            default_value,
            is_primary_key,
        });
    }

    Ok(TableSchema {
        table_name: logical_table_name.to_string(),
        columns,
    })
}

/// Get foreign key relationships for a table
async fn db_get_foreign_keys(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<Vec<ForeignKeyInfo>> {
    // Get the physical table name first
    let physical_table_name: Option<String> = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let physical_table_name = physical_table_name.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Query PostgreSQL information_schema for foreign keys
    let rows = sqlx::query!(
        r#"
        SELECT
            kcu.column_name,
            ccu.table_name AS referenced_table_physical,
            ccu.column_name AS referenced_column
        FROM information_schema.table_constraints AS tc
        JOIN information_schema.key_column_usage AS kcu
            ON tc.constraint_name = kcu.constraint_name
            AND tc.table_schema = kcu.table_schema
        JOIN information_schema.constraint_column_usage AS ccu
            ON ccu.constraint_name = tc.constraint_name
            AND ccu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY'
            AND tc.table_name = $1
        "#,
        &physical_table_name
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching foreign keys: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut foreign_keys = Vec::new();

    for row in rows {
        // Look up the logical table name for the referenced table
        let referenced_table_logical: Option<String> = sqlx::query_scalar(
            "SELECT logical_table_name FROM script_tables WHERE script_uri = $1 AND physical_table_name = $2",
        )
        .bind(script_uri)
        .bind(&row.referenced_table_physical)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error fetching referenced table name: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

        // Only include FK if it references another table owned by the same script
        if let Some(referenced_logical) = referenced_table_logical
            && let Some(column_name) = row.column_name
        {
            foreign_keys.push(ForeignKeyInfo {
                column_name,
                referenced_table_logical: referenced_logical,
                referenced_table_physical: row.referenced_table_physical.unwrap_or_default(),
                referenced_column: row.referenced_column.unwrap_or_else(|| "id".to_string()),
            });
        }
    }

    Ok(foreign_keys)
}

// ============================================================================
// Script Database Data Access Functions
// ============================================================================

/// Supported comparison operators for query filters.
enum FilterOp {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
    Ne,
}

/// A single resolved filter condition (column, operator, value).
struct FilterCondition {
    column: String,
    op: FilterOp,
    value: serde_json::Value,
}

/// Parse a filter map into a flat list of `FilterCondition`s.
///
/// Supports two forms:
/// - Equality:  `{ "col": value }`
/// - Operators: `{ "col": { "$gt": v, "$lt": w, ... } }`
///
/// Allowed operators: `$gt`, `$gte`, `$lt`, `$lte`, `$ne`.
fn parse_filter_conditions(
    filters: &HashMap<String, serde_json::Value>,
) -> AppResult<Vec<FilterCondition>> {
    let mut conditions = Vec::new();
    for (column, value) in filters {
        validate_identifier(column).map_err(|e| AppError::Validation {
            field: "filter_column".to_string(),
            reason: e.to_string(),
        })?;
        match value {
            serde_json::Value::Object(ops) => {
                for (op_key, op_val) in ops {
                    let op = match op_key.as_str() {
                        "$gt" => FilterOp::Gt,
                        "$gte" => FilterOp::Gte,
                        "$lt" => FilterOp::Lt,
                        "$lte" => FilterOp::Lte,
                        "$ne" => FilterOp::Ne,
                        other => {
                            return Err(AppError::Validation {
                                field: "filter_operator".to_string(),
                                reason: format!("Unknown filter operator: {}", other),
                            });
                        }
                    };
                    conditions.push(FilterCondition {
                        column: column.clone(),
                        op,
                        value: op_val.clone(),
                    });
                }
            }
            _ => {
                conditions.push(FilterCondition {
                    column: column.clone(),
                    op: FilterOp::Eq,
                    value: value.clone(),
                });
            }
        }
    }
    Ok(conditions)
}

/// The Postgres type a script's value is bound as, and cast to in the SQL.
///
/// Picking this from the shape of the JSON that carried the value — an `i64`
/// for `2`, an `f64` for `1.57` — is what let one SQL string arrive with
/// different parameter types on different calls. sqlx caches a prepared
/// statement under that string alone: `get_or_prepare` returns the cached
/// entry before it looks at the argument types, so the types inferred by the
/// first call are the types every later call binds against, for as long as
/// that pooled connection lives. Bind ships the encoded bytes unchecked, so a
/// float sent to a parameter prepared as `int8` is not rejected — it is
/// reinterpreted, bit for bit, and `1.57` arrives as 4609081767789723156.
///
/// Deciding the type from the column instead makes it a function of the
/// column names, which are already in the SQL text — the thing the cache is
/// keyed on. The same statement can then only ever be bound the same way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BindType {
    Int4,
    Int8,
    Float8,
    Text,
    Bool,
    Timestamptz,
}

impl BindType {
    /// The type the placeholder is cast to, as in `$1::int4`.
    ///
    /// Pinning it in the SQL does two things. It stops Postgres inferring the
    /// parameter's type from the surrounding statement — inference is what
    /// quietly rounded `1.57` to `2` through a `float8 → int4` assignment
    /// cast — and it puts the type into the cache key, so a column whose
    /// declared type is unknown and whose bind type had to be guessed from
    /// the value still cannot collide with a differently typed guess.
    fn cast(self) -> &'static str {
        match self {
            BindType::Int4 => "int4",
            BindType::Int8 => "int8",
            BindType::Float8 => "float8",
            BindType::Text => "text",
            BindType::Bool => "bool",
            BindType::Timestamptz => "timestamptz",
        }
    }

    /// How the type is named back to the script, in the words it declared it with.
    fn describe(self) -> &'static str {
        match self {
            BindType::Int4 => "INTEGER",
            BindType::Int8 => "BIGINT",
            BindType::Float8 => "FLOAT",
            BindType::Text => "TEXT",
            BindType::Bool => "BOOLEAN",
            BindType::Timestamptz => "TIMESTAMP",
        }
    }

    /// Resolve a column type recorded in `script_tables.schema_json`.
    ///
    /// Returns `None` for anything unrecognised, which leaves the value's own
    /// shape to decide — see [`BindType::infer`].
    fn from_declared(declared: &str) -> Option<Self> {
        match declared.to_uppercase().as_str() {
            "INTEGER" | "INT" | "INT4" | "SERIAL" => Some(BindType::Int4),
            "BIGINT" | "INT8" | "BIGSERIAL" => Some(BindType::Int8),
            "DOUBLE PRECISION" | "FLOAT8" | "FLOAT" | "REAL" | "DOUBLE" => Some(BindType::Float8),
            "TEXT" | "STRING" | "VARCHAR" => Some(BindType::Text),
            "BOOLEAN" | "BOOL" => Some(BindType::Bool),
            "TIMESTAMPTZ" | "TIMESTAMP" => Some(BindType::Timestamptz),
            _ => None,
        }
    }

    /// The type to bind a value as when the column's own type is unknown.
    ///
    /// Tables predating the schema metadata — and lease tables, which record
    /// no columns — have nothing to look the column up in. The value's shape
    /// is all that is left, which is the old behaviour; what keeps it safe is
    /// that the guess is pinned by [`BindType::cast`], so two different
    /// guesses land on two different cached statements.
    fn infer(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Number(n) if n.as_i64().is_some() => BindType::Int8,
            serde_json::Value::Number(_) => BindType::Float8,
            serde_json::Value::Bool(_) => BindType::Bool,
            _ => BindType::Text,
        }
    }
}

/// The columns of a script-owned table, with the physical name to address it by.
///
/// Loaded in the one query that used to fetch the physical name alone, so
/// typing a statement's parameters costs no extra round trip.
struct TableColumns {
    physical_name: String,
    /// Column name to declared type. Empty for a table that records no schema.
    declared: HashMap<String, BindType>,
}

impl TableColumns {
    async fn load(
        conn: &mut PgConnection,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Self> {
        let row: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
            "SELECT physical_table_name, schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
        )
        .bind(script_uri)
        .bind(logical_table_name)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error fetching table metadata: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

        let (physical_name, schema_json) = row.ok_or_else(|| AppError::Validation {
            field: "table_name".to_string(),
            reason: format!("Table '{}' not found for this script", logical_table_name),
        })?;

        let mut declared = HashMap::new();
        if let Some(columns) = schema_json
            .as_ref()
            .and_then(|s| s.get("columns"))
            .and_then(|c| c.as_array())
        {
            for column in columns {
                if let Some(name) = column.get("name").and_then(|n| n.as_str())
                    && let Some(bind_type) = column
                        .get("type")
                        .and_then(|t| t.as_str())
                        .and_then(BindType::from_declared)
                {
                    declared.insert(name.to_string(), bind_type);
                }
            }
        }

        Ok(Self {
            physical_name,
            declared,
        })
    }

    /// The type `column` must be bound as, falling back to the value's shape.
    fn bind_type(&self, column: &str, value: &serde_json::Value) -> BindType {
        self.declared
            .get(column)
            .copied()
            .unwrap_or_else(|| BindType::infer(value))
    }
}

/// A value to bind, with the type resolved from the column it is going into.
struct BoundValue<'a> {
    column: &'a str,
    value: &'a serde_json::Value,
    bind_type: BindType,
}

impl<'a> BoundValue<'a> {
    fn new(columns: &TableColumns, column: &'a str, value: &'a serde_json::Value) -> Self {
        Self {
            column,
            value,
            bind_type: columns.bind_type(column, value),
        }
    }

    /// The placeholder for this value, cast to its resolved type.
    fn placeholder(&self, position: usize) -> String {
        format!("${}::{}", position, self.bind_type.cast())
    }
}

/// Resolve a script's `{column: value}` map into a deterministic binding order.
///
/// Sorted rather than left in hash order: the column list is part of the SQL
/// text, and the statement cache is keyed on that text, so an unstable order
/// would scatter one logical statement across as many cached statements as the
/// map has orderings.
fn ordered_bindings<'a>(
    columns: &TableColumns,
    data: &'a HashMap<String, serde_json::Value>,
) -> AppResult<Vec<BoundValue<'a>>> {
    let mut names: Vec<&'a String> = data.keys().collect();
    names.sort();

    let mut bound = Vec::with_capacity(names.len());
    for name in names {
        validate_identifier(name).map_err(|e| AppError::Validation {
            field: "column_name".to_string(),
            reason: e.to_string(),
        })?;
        bound.push(BoundValue::new(columns, name, &data[name]));
    }
    Ok(bound)
}

/// Reject a value the column's declared type cannot hold.
fn value_rejected(column: &str, bind_type: BindType, got: &str) -> AppError {
    AppError::Validation {
        field: column.to_string(),
        reason: format!(
            "Column '{}' is {}; got {}",
            column,
            bind_type.describe(),
            got
        ),
    }
}

/// A JSON number as a whole number, or an error naming what was wrong with it.
///
/// A script computing `1.57` for an integer column has a bug, and rounding it
/// to `2` on the script's behalf hides that bug behind a value it never asked
/// to store. `2.0` is a different matter: JavaScript has one numeric type, so
/// a whole number arrives as a float whenever it has been through arithmetic,
/// and refusing it would refuse ordinary integer work.
fn as_whole_number(column: &str, bind_type: BindType, n: &serde_json::Number) -> AppResult<i64> {
    if let Some(i) = n.as_i64() {
        return Ok(i);
    }
    match n.as_f64() {
        Some(f) if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 => Ok(f as i64),
        Some(f) if f.fract() != 0.0 => Err(value_rejected(
            column,
            bind_type,
            &format!(
                "{} — a whole number is required, or a FLOAT column to keep the fraction",
                f
            ),
        )),
        _ => Err(value_rejected(
            column,
            bind_type,
            &format!("{} — out of range", n),
        )),
    }
}

/// Bind one resolved value to a sqlx query as the type its column declared.
///
/// Every mismatch is reported here, as a validation error naming the column,
/// rather than being sent to Postgres to fail against. That matters inside a
/// transaction: a statement that never runs cannot abort one.
fn bind_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    bound: &BoundValue<'q>,
) -> AppResult<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>> {
    use serde_json::Value;

    let column = bound.column;
    let bind_type = bound.bind_type;

    match (bind_type, bound.value) {
        (BindType::Int4, Value::Null) => Ok(query.bind(Option::<i32>::None)),
        (BindType::Int4, Value::Number(n)) => {
            let whole = as_whole_number(column, bind_type, n)?;
            let narrowed = i32::try_from(whole).map_err(|_| {
                value_rejected(column, bind_type, &format!("{} — out of range", whole))
            })?;
            Ok(query.bind(narrowed))
        }

        (BindType::Int8, Value::Null) => Ok(query.bind(Option::<i64>::None)),
        (BindType::Int8, Value::Number(n)) => {
            Ok(query.bind(as_whole_number(column, bind_type, n)?))
        }

        (BindType::Float8, Value::Null) => Ok(query.bind(Option::<f64>::None)),
        (BindType::Float8, Value::Number(n)) => {
            // A `serde_json` number is finite by construction, so there is no
            // NaN or infinity to screen out here.
            let f = n.as_f64().ok_or_else(|| {
                value_rejected(column, bind_type, &format!("{} — out of range", n))
            })?;
            Ok(query.bind(f))
        }

        (BindType::Text, Value::Null) => Ok(query.bind(Option::<String>::None)),
        (BindType::Text, Value::String(s)) => Ok(query.bind(s.as_str())),

        (BindType::Bool, Value::Null) => Ok(query.bind(Option::<bool>::None)),
        (BindType::Bool, Value::Bool(b)) => Ok(query.bind(*b)),

        // Postgres parses the timestamp out of the cast placeholder, which
        // accepts the ISO 8601 strings scripts get from `toISOString()`.
        (BindType::Timestamptz, Value::Null) => Ok(query.bind(Option::<String>::None)),
        (BindType::Timestamptz, Value::String(s)) => Ok(query.bind(s.as_str())),

        (_, Value::Array(_)) | (_, Value::Object(_)) => Err(value_rejected(
            column,
            bind_type,
            "an array or object — only scalar values can be stored",
        )),
        (_, Value::Number(n)) => Err(value_rejected(column, bind_type, &format!("{}", n))),
        (_, Value::String(_)) => Err(value_rejected(column, bind_type, "a string")),
        (_, Value::Bool(b)) => Err(value_rejected(column, bind_type, &format!("{}", b))),
    }
}

/// Convert a sqlx `PgRow` to a `serde_json::Value::Object`.
fn row_to_json(row: &sqlx::postgres::PgRow) -> serde_json::Value {
    use sqlx::Column;
    let mut obj = serde_json::Map::new();
    for (idx, column) in row.columns().iter().enumerate() {
        let column_name = column.name().to_string();
        let value = if let Ok(v) = row.try_get::<i64, _>(idx) {
            serde_json::Value::Number(v.into())
        } else if let Ok(v) = row.try_get::<i32, _>(idx) {
            serde_json::Value::Number(v.into())
        } else if let Ok(v) = row.try_get::<String, _>(idx) {
            serde_json::Value::String(v)
        } else if let Ok(v) = row.try_get::<f64, _>(idx) {
            // Ahead of the null fallback rather than after it: a `float8`
            // column matches none of the arms above, so without this one every
            // float a script stored would read back as null.
            serde_json::Number::from_f64(v)
                .map_or(serde_json::Value::Null, serde_json::Value::Number)
        } else if let Ok(v) = row.try_get::<bool, _>(idx) {
            serde_json::Value::Bool(v)
        } else if let Ok(v) = row.try_get::<DateTime<Utc>, _>(idx) {
            serde_json::Value::String(v.to_rfc3339())
        } else {
            serde_json::Value::Null
        };
        obj.insert(column_name, value);
    }
    serde_json::Value::Object(obj)
}

/// Look up the physical table name for a script-owned logical table.
async fn get_physical_table_name(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<String> {
    let physical_table_name: Option<String> = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    physical_table_name.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })
}

/// Query rows from a script-owned table.
///
/// `filters` supports equality (`{"col": value}`) and comparison operators
/// (`{"col": {"$gt": v}}`). Supported operators: `$gt`, `$gte`, `$lt`,
/// `$lte`, `$ne`.
///
/// `order_by` must be a valid column identifier; `order_dir` is `"asc"` or
/// `"desc"` (defaults to `"asc"`).
async fn db_query_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    filters: Option<&HashMap<String, serde_json::Value>>,
    limit: Option<i64>,
    order_by: Option<&str>,
    order_dir: Option<&str>,
) -> AppResult<Vec<serde_json::Value>> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    // Parse filter conditions (supports equality and range operators)
    let conditions = if let Some(f) = filters {
        parse_filter_conditions(f)?
    } else {
        Vec::new()
    };

    let bound: Vec<BoundValue<'_>> = conditions
        .iter()
        .map(|c| BoundValue::new(&columns, &c.column, &c.value))
        .collect();

    // Build WHERE clause
    let mut sql = format!("SELECT * FROM {}", quote_identifier(&columns.physical_name));
    let mut param_count = 0usize;

    if !conditions.is_empty() {
        let clauses: Vec<String> = conditions
            .iter()
            .zip(&bound)
            .map(|(c, b)| {
                param_count += 1;
                let op_str = match c.op {
                    FilterOp::Eq => "=",
                    FilterOp::Gt => ">",
                    FilterOp::Gte => ">=",
                    FilterOp::Lt => "<",
                    FilterOp::Lte => "<=",
                    FilterOp::Ne => "!=",
                };
                format!(
                    "{} {} {}",
                    quote_identifier(&c.column),
                    op_str,
                    b.placeholder(param_count)
                )
            })
            .collect();
        sql.push_str(&format!(" WHERE {}", clauses.join(" AND ")));
    }

    // ORDER BY
    if let Some(order_col) = order_by {
        validate_identifier(order_col).map_err(|e| AppError::Validation {
            field: "order_by".to_string(),
            reason: e.to_string(),
        })?;
        let dir = match order_dir.unwrap_or("asc").to_lowercase().as_str() {
            "desc" => "DESC",
            _ => "ASC",
        };
        sql.push_str(&format!(
            " ORDER BY {} {}",
            quote_identifier(order_col),
            dir
        ));
    }

    // LIMIT (default 100, max 1000)
    let limit_val = limit.unwrap_or(100).min(1000);
    param_count += 1;
    sql.push_str(&format!(" LIMIT ${}::int8", param_count));

    // Bind parameters
    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }
    sql_query = sql_query.bind(limit_val);

    let rows = sql_query.fetch_all(&mut *conn).await.map_err(|e| {
        error!("Database error querying table: {}", e);
        AppError::Database {
            message: format!("Query error: {}", e),
            source: None,
        }
    })?;

    Ok(rows.iter().map(row_to_json).collect())
}

/// Insert a row into a script-owned table
async fn db_insert_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if data.is_empty() {
        return Err(AppError::Validation {
            field: "data".to_string(),
            reason: "No data provided for insert".to_string(),
        });
    }

    let bound = ordered_bindings(&columns, data)?;

    let mut column_list = Vec::new();
    let mut placeholders = Vec::new();
    for (position, value) in bound.iter().enumerate() {
        column_list.push(quote_identifier(value.column));
        placeholders.push(value.placeholder(position + 1));
    }

    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING *",
        quote_identifier(&columns.physical_name),
        column_list.join(", "),
        placeholders.join(", ")
    );

    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }

    let row = sql_query.fetch_one(&mut *conn).await.map_err(|e| {
        error!("Database error inserting row: {}", e);
        AppError::Database {
            message: format!("Insert error: {}", e),
            source: None,
        }
    })?;

    Ok(row_to_json(&row))
}

/// Update a row in a script-owned table
async fn db_update_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    id: i32,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if data.is_empty() {
        return Err(AppError::Validation {
            field: "data".to_string(),
            reason: "No data provided for update".to_string(),
        });
    }

    let bound = ordered_bindings(&columns, data)?;

    let mut param_count = 0usize;
    let set_clauses: Vec<String> = bound
        .iter()
        .map(|value| {
            param_count += 1;
            format!(
                "{} = {}",
                quote_identifier(value.column),
                value.placeholder(param_count)
            )
        })
        .collect();

    param_count += 1;
    let sql = format!(
        "UPDATE {} SET {} WHERE id = ${}::int4 RETURNING *",
        quote_identifier(&columns.physical_name),
        set_clauses.join(", "),
        param_count
    );

    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }
    sql_query = sql_query.bind(id);

    let row = sql_query.fetch_one(&mut *conn).await.map_err(|e| {
        error!("Database error updating row: {}", e);
        AppError::Database {
            message: format!("Update error: {}", e),
            source: None,
        }
    })?;

    Ok(row_to_json(&row))
}

/// Delete a row from a script-owned table by ID
async fn db_delete_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    id: i32,
) -> AppResult<bool> {
    let physical_table_name = get_physical_table_name(conn, script_uri, logical_table_name).await?;

    let sql = format!(
        "DELETE FROM {} WHERE id = $1::int4",
        quote_identifier(&physical_table_name)
    );

    let result = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error deleting row: {}", e);
            AppError::Database {
                message: format!("Delete error: {}", e),
                source: None,
            }
        })?;

    Ok(result.rows_affected() > 0)
}

/// Upsert a row into a script-owned table using INSERT … ON CONFLICT DO UPDATE.
///
/// `key_columns` names the columns that form the conflict target; a unique
/// index on those columns must exist (create one with `db_add_unique_index`).
/// `data` must contain values for all columns, including the key columns.
async fn db_upsert_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    key_columns: &[String],
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if data.is_empty() {
        return Err(AppError::Validation {
            field: "data".to_string(),
            reason: "No data provided for upsert".to_string(),
        });
    }
    if key_columns.is_empty() {
        return Err(AppError::Validation {
            field: "key_columns".to_string(),
            reason: "At least one key column must be specified".to_string(),
        });
    }

    // Validate key columns
    for kc in key_columns {
        validate_identifier(kc).map_err(|e| AppError::Validation {
            field: "key_column".to_string(),
            reason: e.to_string(),
        })?;
    }

    // Build column/placeholder lists (deterministic order via sorted keys)
    let bound = ordered_bindings(&columns, data)?;

    let mut col_list = Vec::new();
    let mut placeholder_list = Vec::new();
    for (position, value) in bound.iter().enumerate() {
        col_list.push(quote_identifier(value.column));
        placeholder_list.push(value.placeholder(position + 1));
    }

    // SET clause: update all non-key columns
    let key_set: std::collections::HashSet<&str> = key_columns.iter().map(|s| s.as_str()).collect();
    let set_clauses: Vec<String> = bound
        .iter()
        .filter(|value| !key_set.contains(value.column))
        .map(|value| {
            format!(
                "{} = EXCLUDED.{}",
                quote_identifier(value.column),
                quote_identifier(value.column)
            )
        })
        .collect();

    let conflict_target = key_columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = if set_clauses.is_empty() {
        // All columns are key columns — DO NOTHING is the right action
        format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
            quote_identifier(&columns.physical_name),
            col_list.join(", "),
            placeholder_list.join(", "),
            conflict_target,
        )
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
            quote_identifier(&columns.physical_name),
            col_list.join(", "),
            placeholder_list.join(", "),
            conflict_target,
            set_clauses.join(", "),
        )
    };

    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }

    let row = sql_query.fetch_one(&mut *conn).await.map_err(|e| {
        error!("Database error upserting row: {}", e);
        AppError::Database {
            message: format!("Upsert error: {}", e),
            source: None,
        }
    })?;

    Ok(row_to_json(&row))
}

/// Delete rows from a script-owned table matching the given filter conditions.
///
/// Supports the same filter syntax as `db_query_table` (equality and range
/// operators). Returns the number of rows deleted.
async fn db_delete_where(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    filters: &HashMap<String, serde_json::Value>,
) -> AppResult<u64> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if filters.is_empty() {
        return Err(AppError::Validation {
            field: "filters".to_string(),
            reason: "deleteWhere requires at least one filter to prevent accidental full-table delete. Use dropTable to remove all rows.".to_string(),
        });
    }

    let conditions = parse_filter_conditions(filters)?;

    let bound: Vec<BoundValue<'_>> = conditions
        .iter()
        .map(|c| BoundValue::new(&columns, &c.column, &c.value))
        .collect();

    let mut param_count = 0usize;
    let clauses: Vec<String> = conditions
        .iter()
        .zip(&bound)
        .map(|(c, b)| {
            param_count += 1;
            let op_str = match c.op {
                FilterOp::Eq => "=",
                FilterOp::Gt => ">",
                FilterOp::Gte => ">=",
                FilterOp::Lt => "<",
                FilterOp::Lte => "<=",
                FilterOp::Ne => "!=",
            };
            format!(
                "{} {} {}",
                quote_identifier(&c.column),
                op_str,
                b.placeholder(param_count)
            )
        })
        .collect();

    let sql = format!(
        "DELETE FROM {} WHERE {}",
        quote_identifier(&columns.physical_name),
        clauses.join(" AND ")
    );

    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }

    let result = sql_query.execute(&mut *conn).await.map_err(|e| {
        error!("Database error in deleteWhere: {}", e);
        AppError::Database {
            message: format!("Delete error: {}", e),
            source: None,
        }
    })?;

    Ok(result.rows_affected())
}

/// Atomically acquire or extend a distributed lease stored in a script-owned table.
///
/// The table must have been created with `db_create_lease_table` (or manually
/// given the schema `lease_id TEXT, owner TEXT, expires_at TIMESTAMPTZ` with a
/// UNIQUE constraint on `lease_id`).
///
/// Returns `{acquired: bool, owner: string, expires_at: string}`.
///
/// How it works (single-statement, race-free):
/// - If no row with `lease_id` exists → INSERT succeeds → we own the lease.
/// - If a row exists and is expired OR already owned by us → UPDATE succeeds → we own it.
/// - If a row exists, is not expired, and belongs to someone else → nothing changes → we do NOT own it.
async fn db_acquire_lease(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    lease_id: &str,
    owner: &str,
    ttl_ms: i64,
) -> AppResult<serde_json::Value> {
    let physical_table_name = get_physical_table_name(conn, script_uri, logical_table_name).await?;

    if ttl_ms <= 0 {
        return Err(AppError::Validation {
            field: "ttl_ms".to_string(),
            reason: "ttl_ms must be a positive integer".to_string(),
        });
    }

    // Single-statement atomic upsert: wins only if slot is free or ours.
    // bind order: $1=lease_id, $2=owner, $3=ttl_ms (bigint milliseconds)
    let sql = format!(
        r#"
        INSERT INTO {tbl} (lease_id, owner, expires_at)
        VALUES ($1, $2, NOW() + ($3::bigint * interval '1 millisecond'))
        ON CONFLICT (lease_id) DO UPDATE
            SET owner      = EXCLUDED.owner,
                expires_at = EXCLUDED.expires_at
        WHERE {tbl}.expires_at <= NOW()
           OR {tbl}.owner = EXCLUDED.owner
        RETURNING owner, expires_at
        "#,
        tbl = quote_identifier(&physical_table_name)
    );

    let upsert_row = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(lease_id)
        .bind(owner)
        .bind(ttl_ms)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error in acquireLease: {}", e);
            AppError::Database {
                message: format!("Lease error: {}", e),
                source: None,
            }
        })?;

    if let Some(row) = upsert_row {
        // Upsert succeeded — we hold the lease
        let row_owner: String = row.try_get("owner").unwrap_or_default();
        let expires_at: String = row
            .try_get::<DateTime<Utc>, _>("expires_at")
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        return Ok(serde_json::json!({
            "acquired": row_owner == owner,
            "owner": row_owner,
            "expires_at": expires_at,
        }));
    }

    // Upsert produced no row — someone else holds an active lease.
    // Do a plain SELECT to return current lease info (best-effort, non-critical).
    let select_sql = format!(
        "SELECT owner, expires_at FROM {} WHERE lease_id = $1",
        quote_identifier(&physical_table_name)
    );
    let current = sqlx::query(sqlx::AssertSqlSafe(select_sql.as_str()))
        .bind(lease_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error reading current lease: {}", e);
            AppError::Database {
                message: format!("Lease read error: {}", e),
                source: None,
            }
        })?;

    if let Some(row) = current {
        let row_owner: String = row.try_get("owner").unwrap_or_default();
        let expires_at: String = row
            .try_get::<DateTime<Utc>, _>("expires_at")
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        Ok(serde_json::json!({
            "acquired": false,
            "owner": row_owner,
            "expires_at": expires_at,
        }))
    } else {
        Ok(serde_json::json!({
            "acquired": false,
            "owner": serde_json::Value::Null,
            "expires_at": serde_json::Value::Null,
        }))
    }
}

/// Create a properly structured lease table owned by the given script.
///
/// The table schema is: `lease_id TEXT UNIQUE NOT NULL, owner TEXT NOT NULL,
/// expires_at TIMESTAMPTZ NOT NULL`. An explicit UNIQUE index on `lease_id`
/// is created so that `acquireLease` can use it as a conflict target.
///
/// Returns the physical table name on success.
async fn db_create_lease_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<String> {
    use crate::db_schema_utils::{
        MAX_TABLES_PER_SCRIPT, generate_physical_table_name, validate_identifier,
    };

    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;

    // Count existing tables for this script
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM script_tables WHERE script_uri = $1")
        .bind(script_uri)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        })?;

    if count >= MAX_TABLES_PER_SCRIPT as i64 {
        return Err(AppError::Validation {
            field: "table_name".to_string(),
            reason: format!("Maximum table limit of {} reached", MAX_TABLES_PER_SCRIPT),
        });
    }

    // Check for duplicates
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| AppError::Database {
        message: format!("Database error: {}", e),
        source: None,
    })?;

    if let Some(existing_physical) = existing {
        // Table already exists — idempotent
        return Ok(existing_physical);
    }

    let physical_name = generate_physical_table_name(script_uri, logical_table_name);

    // Create the table with the required lease schema
    let create_sql = format!(
        r#"CREATE TABLE {} (
            lease_id  TEXT        NOT NULL,
            owner     TEXT        NOT NULL,
            expires_at TIMESTAMPTZ NOT NULL,
            CONSTRAINT {}_pkey PRIMARY KEY (lease_id)
        )"#,
        quote_identifier(&physical_name),
        physical_name,
    );

    sqlx::query(sqlx::AssertSqlSafe(create_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| AppError::Database {
            message: format!("Error creating lease table: {}", e),
            source: None,
        })?;

    // Register in metadata
    sqlx::query(
        "INSERT INTO script_tables (script_uri, logical_table_name, physical_table_name, created_at)
         VALUES ($1, $2, $3, NOW())",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .bind(&physical_name)
    .execute(&mut *conn)
    .await
    .map_err(|e| AppError::Database {
        message: format!("Error registering lease table: {}", e),
        source: None,
    })?;

    debug!(
        "Created lease table '{}' (physical: '{}') for script '{}'",
        logical_table_name, physical_name, script_uri
    );

    Ok(physical_name)
}

/// Add a unique index on one or more columns of a script-owned table.
///
/// This is required before using those columns as a conflict target in
/// `db_upsert_row`. Index names are derived from the physical table name and
/// columns to avoid collisions.
async fn db_add_unique_index(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    columns: &[String],
) -> AppResult<()> {
    let physical_table_name = get_physical_table_name(conn, script_uri, logical_table_name).await?;

    if columns.is_empty() {
        return Err(AppError::Validation {
            field: "columns".to_string(),
            reason: "At least one column must be specified".to_string(),
        });
    }

    for col in columns {
        validate_identifier(col).map_err(|e| AppError::Validation {
            field: "column".to_string(),
            reason: e.to_string(),
        })?;
    }

    // Build a deterministic index name
    let cols_slug = columns.join("_");
    // Truncate to stay within PostgreSQL's 63-char identifier limit
    let index_name = format!(
        "{}_uniq_{}",
        &physical_table_name[..physical_table_name.len().min(40)],
        &cols_slug[..cols_slug.len().min(20)]
    );

    let col_list = columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "CREATE UNIQUE INDEX IF NOT EXISTS {} ON {} ({})",
        quote_identifier(&index_name),
        quote_identifier(&physical_table_name),
        col_list
    );

    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error adding unique index: {}", e);
            AppError::Database {
                message: format!("Index error: {}", e),
                source: None,
            }
        })?;

    debug!(
        "Created unique index '{}' on table '{}' for script '{}'",
        index_name, logical_table_name, script_uri
    );

    Ok(())
}

/// Fetch scripts from repository with proper error handling
pub fn fetch_scripts() -> HashMap<String, String> {
    let repo = get_repository();

    // Use run_blocking to call async repository method
    let result = run_bounded(async { repo.list_scripts().await });

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

/// Fetch a single script by URI with proper error handling.
///
/// Fast path: the in-memory metadata cache (`DYNAMIC_SCRIPTS`) already holds
/// each script's source `content`. It is populated when the route index is
/// (re)built via `get_all_script_metadata` and evicted on every upsert/delete —
/// locally and through the cross-instance notification handlers — so a present
/// entry is authoritative. Serving content from it avoids a Postgres round-trip
/// on the hot request path.
///
/// The cache is bypassed while a transaction is active so a script still reads
/// its own uncommitted writes (an in-flight upsert has already evicted the
/// entry, so a concurrent reader without the transaction correctly falls through
/// to committed state).
pub fn fetch_script(uri: &str) -> Option<String> {
    if !crate::database::get_current_transaction_active()
        && let Ok(guard) = safe_lock_scripts()
        && let Some(metadata) = guard.get(uri)
    {
        return Some(metadata.content.clone());
    }

    let repo = get_repository();

    let result = run_bounded(async { repo.get_script(uri).await });

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
    run_bounded(async { repo.get_script_metadata(uri).await })
}

/// Get metadata for all scripts
pub fn get_all_script_metadata() -> AppResult<Vec<ScriptMetadata>> {
    let repo = get_repository();
    run_bounded(async { repo.get_all_script_metadata().await })
}

pub fn mark_script_init_failed(uri: &str, error: String) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.update_script_init_status(uri, false, Some(error), None)
            .await
    })
}

pub fn mark_script_initialized(uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.update_script_init_status(uri, true, None, None).await })
}

pub fn mark_script_initialized_with_registrations(
    uri: &str,
    registrations: RouteRegistrations,
) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.update_script_init_status(uri, true, None, Some(registrations))
            .await
    })
}

/// Insert log message with error handling
pub fn insert_log_message(script_uri: &str, message: &str, log_level: &str) {
    insert_log_message_in_context(script_uri, message, log_level, &LogContext::default())
}

/// Insert a log message attributed to the invocation that emitted it.
///
/// Engine-internal writes keep using [`insert_log_message`]; a line written on
/// behalf of a script's `console` should come through here so it can be traced
/// back to the request, tick or connection it belongs to.
pub fn insert_log_message_in_context(
    script_uri: &str,
    message: &str,
    log_level: &str,
    context: &LogContext,
) {
    run_blocking(insert_log_message_async_in_context(
        script_uri, message, log_level, context,
    ))
}

/// Async variant of [`insert_log_message`] for callers already in async context
pub async fn insert_log_message_async(script_uri: &str, message: &str, log_level: &str) {
    insert_log_message_async_in_context(script_uri, message, log_level, &LogContext::default())
        .await
}

/// Async variant of [`insert_log_message_in_context`].
pub async fn insert_log_message_async_in_context(
    script_uri: &str,
    message: &str,
    log_level: &str,
    context: &LogContext,
) {
    let repo = get_repository();
    if let Err(e) = repo
        .insert_log(script_uri, message, log_level, context)
        .await
    {
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
    let result = run_bounded(async { repo.fetch_logs(script_uri).await });

    match result {
        Ok(messages) => messages,
        Err(e) => {
            error!("Failed to fetch log messages for {}: {}", script_uri, e);
            let now = SystemTime::now();
            vec![LogEntry::new(
                script_uri.to_string(),
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
    let result = run_bounded(async { repo.fetch_all_logs().await });

    match result {
        Ok(messages) => messages,
        Err(e) => {
            error!("Failed to fetch all log messages: {}", e);
            vec![LogEntry::new(
                String::new(),
                format!("Error: Could not retrieve logs - {}", e),
                "ERROR".to_string(),
                SystemTime::now(),
            )]
        }
    }
}

/// Fetch log messages matching `query`, newest first.
///
/// Unlike [`fetch_log_messages`] this surfaces database errors instead of
/// folding them into a synthetic entry, so HTTP callers can answer with a real
/// status code.
pub fn query_log_messages(query: &LogQuery) -> AppResult<Vec<LogEntry>> {
    let repo = get_repository();
    run_bounded(async { repo.query_logs(query).await })
}

/// Clear log messages for a script
pub fn clear_log_messages(script_uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.clear_logs(script_uri).await })
}

/// Keep only the latest `limit` log messages (default 20) for each script URI and remove older ones
pub fn prune_log_messages() -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.prune_logs().await })
}

/// Upsert script with error handling
pub fn upsert_script(uri: &str, content: &str) -> AppResult<()> {
    run_blocking(upsert_script_async(uri, content))
}

/// Async variant of [`upsert_script`] for callers already in async context
pub async fn upsert_script_async(uri: &str, content: &str) -> AppResult<()> {
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
    repo.upsert_script(uri, content).await
}

/// Upsert script and set owner if it's a new script
pub fn upsert_script_with_owner(
    uri: &str,
    content: &str,
    owner_user_id: Option<&str>,
) -> AppResult<()> {
    debug!(
        "upsert_script_with_owner called: uri={}, owner_user_id={:?}, content_len={}",
        uri,
        owner_user_id,
        content.len()
    );

    if uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("URI cannot be empty".to_string()).into());
    }

    if content.len() > 1_000_000 {
        // 1MB limit
        return Err(
            RepositoryError::InvalidData("Script content too large (>1MB)".to_string()).into(),
        );
    }

    // Check if script already exists
    let script_exists = fetch_script(uri).is_some();
    debug!(
        "Script existence check: uri={}, exists={}",
        uri, script_exists
    );

    // Upsert the script
    let repo = get_repository();
    run_bounded(async { repo.upsert_script(uri, content).await })?;

    // Assign ownership if needed:
    // - For NEW scripts: set the creator as owner
    // - For EXISTING scripts: set owner if they don't have any owners yet (backfill)
    if let Some(user_id) = owner_user_id {
        // Check if script has any owners
        let owner_count = run_bounded(async { repo.count_script_owners(uri).await }).unwrap_or(0);
        let has_owners = owner_count > 0;
        debug!(
            "Owner count check: uri={}, count={}, has_owners={}",
            uri, owner_count, has_owners
        );

        // If script has no owners, assign this user as the first owner
        if !has_owners {
            debug!("Attempting to add owner: uri={}, user_id={}", uri, user_id);
            run_bounded(async { repo.add_script_owner(uri, user_id).await })?;
            if script_exists {
                debug!(
                    "✓ Backfilled owner {} for existing script {} (had no owners)",
                    user_id, uri
                );
            } else {
                debug!("✓ Set initial owner {} for new script {}", user_id, uri);
            }
        } else {
            debug!(
                "Skipping ownership assignment - script already has {} owner(s): uri={}",
                owner_count, uri
            );
        }
    } else {
        debug!(
            "Skipping ownership assignment - no user_id provided: uri={}",
            uri
        );
    }

    Ok(())
}

/// Bootstrap hardcoded scripts into database on startup
pub fn bootstrap_scripts() -> AppResult<()> {
    run_blocking(bootstrap_scripts_async())
}

/// Async variant of [`bootstrap_scripts`] for callers already in async context
pub async fn bootstrap_scripts_async() -> AppResult<()> {
    if let Some(db) = get_db_pool() {
        let pool = db.pool();

        // All former built-in scripts are now native Rust functionality
        // (src/engine_api.rs); only test fixtures are bootstrapped here.
        let mut all_scripts: Vec<(&str, &str)> = Vec::new();
        let include_test_scripts =
            std::env::var("AIWEBENGINE_INCLUDE_TEST_SCRIPTS").is_ok() || cfg!(test);

        if include_test_scripts {
            all_scripts.push((
                "https://example.com/graphql_test",
                include_str!("../scripts/test_scripts/graphql_test.js"),
            ));
            all_scripts.push((
                "https://example.com/dispatcher_test",
                include_str!("../scripts/test_scripts/dispatcher_test.js"),
            ));
        }

        let result = async {
            for (uri, code) in all_scripts {
                let mut exists = false;
                // Check if script already exists
                if let Ok(Some(existing_content)) = db_get_script(pool, uri).await {
                    debug!("Script already exists in database: {}", uri);
                    exists = true;

                    // Update if content differs
                    if existing_content != code {
                        info!("Updating bootstrap script in database: {}", uri);
                        if let Err(e) = db_upsert_script(
                            crate::database::TransactionExecutor::Pool(pool),
                            uri,
                            code,
                        )
                        .await
                        {
                            error!("Failed to update bootstrap script {}: {}", uri, e);
                            return Err(e);
                        }
                    }
                }

                if !exists {
                    // Insert the script
                    if let Err(e) = db_upsert_script(
                        crate::database::TransactionExecutor::Pool(pool),
                        uri,
                        code,
                    )
                    .await
                    {
                        error!("Failed to bootstrap script {}: {}", uri, e);
                        return Err(e);
                    } else {
                        info!("Bootstrapped script into database: {}", uri);
                    }
                }
            }

            // Remove built-in scripts that have been replaced by native Rust
            // functionality so stale database copies stop executing.
            for uri in RETIRED_BOOTSTRAP_SCRIPTS {
                match db_delete_script(pool, uri).await {
                    Ok(true) => info!("Removed retired bootstrap script from database: {}", uri),
                    Ok(false) => {}
                    Err(e) => warn!("Failed to remove retired bootstrap script {}: {}", uri, e),
                }
            }

            Ok(())
        }
        .await;

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
    let repo = get_repository();

    let result = run_bounded(async { repo.delete_script(uri).await });

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

/// Add an owner to a script
pub fn add_script_owner(uri: &str, user_id: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.add_script_owner(uri, user_id).await })
}

/// Remove an owner from a script
pub fn remove_script_owner(uri: &str, user_id: &str) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.remove_script_owner(uri, user_id).await })
}

/// Get all owners of a script
pub fn get_script_owners(uri: &str) -> AppResult<Vec<String>> {
    let repo = get_repository();
    run_bounded(async { repo.get_script_owners(uri).await })
}

/// Get the hosts a script's registrations are published on.
///
/// Returns the stored bindings: empty means the default host, and a `*` entry
/// means every configured host. Resolve with [`crate::hosts::effective_hosts`].
pub fn get_script_hosts(uri: &str) -> AppResult<Vec<String>> {
    let repo = get_repository();
    run_bounded(async { repo.get_script_hosts(uri).await })
}

/// Replace the hosts a script's registrations are published on.
///
/// The list is the complete set, so passing an empty one returns the script to
/// the default host.
pub fn set_script_hosts(uri: &str, hosts: &[String]) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.set_script_hosts(uri, hosts).await })
}

/// Check if a user owns a script
pub fn user_owns_script(uri: &str, user_id: &str) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.user_owns_script(uri, user_id).await })
}

/// Count the number of owners for a script
pub fn count_script_owners(uri: &str) -> AppResult<i64> {
    let repo = get_repository();
    run_bounded(async { repo.count_script_owners(uri).await })
}

// ============================================================================
// Script Database Schema Public API
// ============================================================================

/// Create a new table for a script
pub fn create_script_table(script_uri: &str, logical_table_name: &str) -> AppResult<String> {
    let repo = get_repository();
    run_bounded(async {
        repo.create_script_table(script_uri, logical_table_name)
            .await
    })
}

/// Add a column to a script-owned table
pub fn add_column_to_script_table(
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    column_type: ColumnType,
    nullable: bool,
    default_value: Option<&str>,
) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.add_column_to_script_table(
            script_uri,
            logical_table_name,
            column_name,
            column_type,
            nullable,
            default_value,
        )
        .await
    })
}

/// Add a reference column (INTEGER with FK) to a script-owned table
pub fn add_reference_column(
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    referenced_logical_table_name: &str,
    nullable: bool,
) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.add_reference_column(
            script_uri,
            logical_table_name,
            column_name,
            referenced_logical_table_name,
            nullable,
        )
        .await
    })
}

/// Drop a column from a script-owned table
pub fn drop_column(
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async {
        repo.drop_column(script_uri, logical_table_name, column_name)
            .await
    })
}

/// Drop a script-owned table
pub fn drop_script_table(script_uri: &str, logical_table_name: &str) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.drop_script_table(script_uri, logical_table_name).await })
}

// ============================================================================
// Script Database Introspection and Data Operations (Synchronous Wrappers)
// ============================================================================

/// List all tables owned by a script
pub fn list_script_tables(script_uri: &str) -> AppResult<Vec<TableInfo>> {
    let repo = get_repository();
    run_bounded(async { repo.list_script_tables(script_uri).await })
}

/// Get detailed schema for a table
pub fn get_table_schema(script_uri: &str, logical_table_name: &str) -> AppResult<TableSchema> {
    let repo = get_repository();
    run_bounded(async { repo.get_table_schema(script_uri, logical_table_name).await })
}

/// Get foreign key relationships for a table
pub fn get_foreign_keys(
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<Vec<ForeignKeyInfo>> {
    let repo = get_repository();
    run_bounded(async { repo.get_foreign_keys(script_uri, logical_table_name).await })
}

/// Query rows from a script-owned table
pub fn query_table(
    script_uri: &str,
    logical_table_name: &str,
    filters: Option<&HashMap<String, serde_json::Value>>,
    limit: Option<i64>,
    order_by: Option<&str>,
    order_dir: Option<&str>,
) -> AppResult<Vec<serde_json::Value>> {
    let repo = get_repository();
    run_bounded(async {
        repo.query_table(
            script_uri,
            logical_table_name,
            filters,
            limit,
            order_by,
            order_dir,
        )
        .await
    })
}

/// Insert a row into a script-owned table
pub fn insert_row(
    script_uri: &str,
    logical_table_name: &str,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let repo = get_repository();
    run_bounded(async { repo.insert_row(script_uri, logical_table_name, data).await })
}

/// Update a row in a script-owned table
pub fn update_row(
    script_uri: &str,
    logical_table_name: &str,
    id: i32,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let repo = get_repository();
    run_bounded(async {
        repo.update_row(script_uri, logical_table_name, id, data)
            .await
    })
}

/// Delete a row from a script-owned table
pub fn delete_row(script_uri: &str, logical_table_name: &str, id: i32) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.delete_row(script_uri, logical_table_name, id).await })
}

/// Upsert a row into a script-owned table (INSERT … ON CONFLICT DO UPDATE)
pub fn upsert_row(
    script_uri: &str,
    logical_table_name: &str,
    key_columns: &[String],
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let repo = get_repository();
    run_bounded(async {
        repo.upsert_row(script_uri, logical_table_name, key_columns, data)
            .await
    })
}

/// Delete rows from a script-owned table matching the given filters
pub fn delete_where(
    script_uri: &str,
    logical_table_name: &str,
    filters: &HashMap<String, serde_json::Value>,
) -> AppResult<u64> {
    let repo = get_repository();
    run_bounded(async {
        repo.delete_where(script_uri, logical_table_name, filters)
            .await
    })
}

/// Atomically acquire or extend a distributed lease in a script-owned table
pub fn acquire_lease(
    script_uri: &str,
    logical_table_name: &str,
    lease_id: &str,
    owner: &str,
    ttl_ms: i64,
) -> AppResult<serde_json::Value> {
    let repo = get_repository();
    run_bounded(async {
        repo.acquire_lease(script_uri, logical_table_name, lease_id, owner, ttl_ms)
            .await
    })
}

/// Create a lease table with the required schema in a script-owned table
pub fn create_lease_table(script_uri: &str, logical_table_name: &str) -> AppResult<String> {
    let repo = get_repository();
    run_bounded(async {
        repo.create_lease_table(script_uri, logical_table_name)
            .await
    })
}

/// Bring a script-owned table to the shape `spec` describes
pub fn ensure_script_table(
    script_uri: &str,
    logical_table_name: &str,
    spec: &TableSpec,
) -> AppResult<EnsuredTable> {
    let repo = get_repository();
    run_bounded(async {
        repo.ensure_script_table(script_uri, logical_table_name, spec)
            .await
    })
}

/// Add a unique index on one or more columns of a script-owned table
pub fn add_unique_index(
    script_uri: &str,
    logical_table_name: &str,
    columns: &[String],
) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.add_unique_index(script_uri, logical_table_name, columns)
            .await
    })
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
        script_uri: "https://example.com/core".to_string(),
    };
    m.insert("logo.svg".to_string(), logo);

    let engine_css_content = include_bytes!("../assets/engine.css").to_vec();
    let engine_css = Asset {
        uri: "engine.css".to_string(),
        name: Some("Engine Styles".to_string()),
        mimetype: "text/css".to_string(),
        content: engine_css_content,
        created_at: now,
        updated_at: now,
        script_uri: "https://example.com/core".to_string(),
    };
    m.insert("engine.css".to_string(), engine_css);

    let favicon_content = include_bytes!("../assets/favicon.ico").to_vec();
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

    let aiwebengine_dts_content = include_bytes!("../assets/aiwebengine.d.ts").to_vec();
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

    m
}

/// Helper function to get static scripts embedded at compile time
fn get_static_scripts() -> HashMap<String, String> {
    let mut m = HashMap::new();

    // Include test scripts when appropriate
    let include_test_scripts =
        std::env::var("AIWEBENGINE_INCLUDE_TEST_SCRIPTS").is_ok() || cfg!(test);

    if include_test_scripts {
        let graphql_test = include_str!("../scripts/test_scripts/graphql_test.js");
        m.insert(
            "https://example.com/graphql_test".to_string(),
            graphql_test.to_string(),
        );
        let dispatcher_test = include_str!("../scripts/test_scripts/dispatcher_test.js");
        m.insert(
            "https://example.com/dispatcher_test".to_string(),
            dispatcher_test.to_string(),
        );
    }

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
fn validate_asset(asset: &Asset) -> AppResult<()> {
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
    let asset_count = if let Some(db) = get_db_pool() {
        run_blocking(async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assets")
                .fetch_one(db.pool())
                .await
                .unwrap_or(0) as usize
        })
    } else {
        0
    };
    stats.insert("assets".to_string(), asset_count);

    // Count total log entries
    let log_count = if let Some(db) = get_db_pool() {
        run_blocking(async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM logs")
                .fetch_one(db.pool())
                .await
                .unwrap_or(0) as usize
        })
    } else {
        0
    };
    stats.insert("log_entries".to_string(), log_count);

    // Count shared storage entries
    let script_properties_count = if let Some(db) = get_db_pool() {
        run_blocking(async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM script_properties")
                .fetch_one(db.pool())
                .await
                .unwrap_or(0) as usize
        })
    } else {
        0
    };
    stats.insert(
        "script_properties_entries".to_string(),
        script_properties_count,
    );

    stats
}

/// Set a shared storage item (key-value pair for a specific script)
pub fn set_script_properties_item(script_uri: &str, key: &str, value: &str) -> AppResult<()> {
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
    run_bounded(async { repo.set_script_properties(script_uri, key, value).await })
}

/// Get a shared storage item
pub fn get_script_properties_item(script_uri: &str, key: &str) -> Option<String> {
    let repo = get_repository();
    let result = run_bounded(async { repo.get_script_properties(script_uri, key).await });

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
pub fn remove_script_properties_item(script_uri: &str, key: &str) -> bool {
    let repo = get_repository();
    let result = run_bounded(async { repo.remove_script_properties(script_uri, key).await });

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
pub fn clear_script_properties(script_uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.clear_script_properties(script_uri).await })
}

/// List the keys a script has in shared storage, in ascending order.
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
                "Failed to list shared storage keys for {}: {}",
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

    if value.len() > 1_000_000 {
        // 1MB limit per value
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

/// Set a script secret (key-value pair for a specific script)
pub fn set_script_secret_item(script_uri: &str, key: &str, value: &str) -> AppResult<()> {
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

    if value.len() > 1_000_000 {
        // 1MB limit per value
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
    async fn get_asset(&self, script_uri: &str, uri: &str) -> AppResult<Option<Asset>>;
    async fn list_assets(&self, script_uri: &str) -> AppResult<HashMap<String, Asset>>;
    async fn upsert_asset(&self, asset: Asset) -> AppResult<()>;
    /// Write several of one script's assets as a unit: one transaction, one
    /// cache invalidation pass, one change notification.
    async fn upsert_assets(&self, script_uri: &str, assets: Vec<Asset>) -> AppResult<()>;
    async fn delete_asset(&self, script_uri: &str, uri: &str) -> AppResult<bool>;

    // Log operations
    async fn insert_log(
        &self,
        script_uri: &str,
        message: &str,
        level: &str,
        context: &LogContext,
    ) -> AppResult<()>;
    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>>;
    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>>;
    async fn query_logs(&self, query: &LogQuery) -> AppResult<Vec<LogEntry>>;
    async fn clear_logs(&self, script_uri: &str) -> AppResult<()>;
    async fn prune_logs(&self) -> AppResult<()>;

    // Shared storage operations
    async fn get_script_properties(&self, script_uri: &str, key: &str)
    -> AppResult<Option<String>>;
    async fn set_script_properties(
        &self,
        script_uri: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()>;
    async fn remove_script_properties(&self, script_uri: &str, key: &str) -> AppResult<bool>;
    async fn clear_script_properties(&self, script_uri: &str) -> AppResult<()>;
    async fn list_script_properties_keys(&self, script_uri: &str) -> AppResult<Vec<String>>;

    // Personal storage operations
    async fn get_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>>;
    async fn set_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()>;
    async fn remove_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool>;
    async fn clear_user_properties(&self, script_uri: &str, user_id: &str) -> AppResult<()>;
    async fn list_user_properties_keys(
        &self,
        script_uri: &str,
        user_id: &str,
    ) -> AppResult<Vec<String>>;

    // Script secrets operations
    async fn get_script_secret(&self, script_uri: &str, key: &str) -> AppResult<Option<String>>;
    async fn set_script_secret(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()>;
    async fn remove_script_secret(&self, script_uri: &str, key: &str) -> AppResult<bool>;
    async fn clear_script_secrets(&self, script_uri: &str) -> AppResult<()>;
    async fn list_script_secrets(&self, script_uri: &str) -> AppResult<Vec<String>>;

    // User secrets operations
    async fn get_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>>;
    async fn set_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()>;
    async fn remove_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool>;
    async fn clear_user_secrets(&self, script_uri: &str, user_id: &str) -> AppResult<()>;

    // Security operations

    // Ownership operations
    async fn add_script_owner(&self, uri: &str, user_id: &str) -> AppResult<()>;
    async fn remove_script_owner(&self, uri: &str, user_id: &str) -> AppResult<bool>;
    async fn get_script_owners(&self, uri: &str) -> AppResult<Vec<String>>;
    async fn user_owns_script(&self, uri: &str, user_id: &str) -> AppResult<bool>;
    async fn count_script_owners(&self, uri: &str) -> AppResult<i64>;

    // Host binding operations
    async fn get_script_hosts(&self, uri: &str) -> AppResult<Vec<String>>;
    async fn set_script_hosts(&self, uri: &str, hosts: &[String]) -> AppResult<()>;

    // Script database schema operations
    async fn create_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<String>;
    async fn add_column_to_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        column_type: ColumnType,
        nullable: bool,
        default_value: Option<&str>,
    ) -> AppResult<()>;
    async fn add_reference_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        referenced_logical_table_name: &str,
        nullable: bool,
    ) -> AppResult<()>;
    async fn drop_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
    ) -> AppResult<bool>;
    async fn drop_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<bool>;

    // Script database introspection operations
    async fn list_script_tables(&self, script_uri: &str) -> AppResult<Vec<TableInfo>>;
    async fn get_table_schema(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<TableSchema>;
    async fn get_foreign_keys(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Vec<ForeignKeyInfo>>;

    // Script database data operations
    async fn query_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: Option<&HashMap<String, serde_json::Value>>,
        limit: Option<i64>,
        order_by: Option<&str>,
        order_dir: Option<&str>,
    ) -> AppResult<Vec<serde_json::Value>>;
    async fn insert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value>;
    async fn update_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value>;
    async fn delete_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
    ) -> AppResult<bool>;
    async fn upsert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        key_columns: &[String],
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value>;
    async fn delete_where(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: &HashMap<String, serde_json::Value>,
    ) -> AppResult<u64>;
    async fn acquire_lease(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        lease_id: &str,
        owner: &str,
        ttl_ms: i64,
    ) -> AppResult<serde_json::Value>;
    async fn create_lease_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<String>;
    async fn add_unique_index(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        columns: &[String],
    ) -> AppResult<()>;
    async fn ensure_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        spec: &TableSpec,
    ) -> AppResult<EnsuredTable>;
}

/// PostgreSQL implementation of the Repository trait
pub struct PostgresRepository {
    pool: PgPool,
    server_id: String,
}

impl PostgresRepository {
    pub fn new(pool: PgPool, server_id: String) -> Self {
        Self { pool, server_id }
    }
}

#[async_trait]
impl Repository for PostgresRepository {
    async fn get_script(&self, uri: &str) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => db_get_script(pool, uri).await,
        }
    }

    async fn list_scripts(&self) -> AppResult<HashMap<String, String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_scripts(&mut **tx).await
            }
            crate::database::TransactionExecutor::Pool(pool) => db_list_scripts(pool).await,
        }
    }

    async fn upsert_script(&self, uri: &str, content: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_upsert_script(executor, uri, content).await?;

        // Send notification after successful upsert
        send_script_notification(&self.pool, uri, "upserted", &self.server_id).await?;

        // Refresh the cached source in place rather than evicting it: eviction
        // would also drop the script's route registrations, 404ing every one of
        // its routes until the re-init that follows this upsert completes.
        refresh_cached_script_source(uri, content);
        crate::route_index::invalidate();
        crate::bytecode::invalidate(uri);
        // Only the root source changed; the script's imported modules are still
        // current, so the rebuild reads them from cache instead of the database.
        crate::module_loader::invalidate_program(uri);
        Ok(())
    }

    async fn delete_script(&self, uri: &str) -> AppResult<bool> {
        // First, drop all script-owned tables. This joins the caller's
        // transaction when there is one: DROP TABLE takes ACCESS EXCLUSIVE, and
        // taking it on a second connection would block on locks the caller
        // already holds.
        if let Ok(mut schema) = ScopedConn::for_schema(&self.pool).await {
            let dropped = db_drop_all_script_tables(schema.conn(), uri).await;
            let _ = schema.finish(dropped).await;
        }

        // Delete the script (within transaction if active)
        let executor = crate::database::get_current_executor(&self.pool);
        let result = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_delete_script(&mut **tx, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => db_delete_script(pool, uri).await?,
        };

        if result {
            // Send notification after successful deletion
            send_script_notification(&self.pool, uri, "deleted", &self.server_id).await?;

            // Update in-memory cache
            if let Ok(mut guard) = safe_lock_scripts() {
                guard.remove(uri);
            }
            crate::route_index::invalidate();
            crate::bytecode::invalidate(uri);
            crate::module_loader::invalidate(uri);
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

        let executor = crate::database::get_current_executor(&self.pool);
        let owners = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_owners(&mut **tx, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_owners(pool, uri).await?
            }
        };

        let script_hosts = match crate::database::get_current_executor(&self.pool) {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_hosts(&mut **tx, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_hosts(pool, uri).await?
            }
        };

        let mut metadata = ScriptMetadata::new(uri.to_string(), content);
        metadata.owners = owners;
        metadata.hosts = script_hosts;

        // Cache it
        if let Ok(mut guard) = safe_lock_scripts() {
            guard.insert(uri.to_string(), metadata.clone());
        }

        Ok(metadata)
    }

    async fn get_all_script_metadata(&self) -> AppResult<Vec<ScriptMetadata>> {
        // Fetch scripts from database only (no static scripts for Postgres)
        let db_scripts = self.list_scripts().await?;

        // Fetch all owners in one query
        let executor = crate::database::get_current_executor(&self.pool);
        let all_owners = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_all_script_owners(&mut **tx).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_all_script_owners(pool).await?
            }
        };

        // Fetch all host bindings in one query, same as owners above
        let all_hosts = match crate::database::get_current_executor(&self.pool) {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_all_script_hosts(&mut **tx).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_all_script_hosts(pool).await?
            }
        };

        let mut metadata_list = Vec::new();

        // Scope for mutex lock
        {
            let mut guard = safe_lock_scripts()?;
            for (uri, content) in db_scripts {
                if let Some(cached) = guard.get(&uri) {
                    // Use cached version to preserve runtime state
                    metadata_list.push(cached.clone());
                } else {
                    // Create new metadata and cache it
                    let mut metadata = ScriptMetadata::new(uri.clone(), content);
                    // Set owners from bulk query
                    if let Some(owners) = all_owners.get(&uri) {
                        metadata.owners = owners.clone();
                    }
                    // Set host bindings from bulk query
                    if let Some(script_hosts) = all_hosts.get(&uri) {
                        metadata.hosts = script_hosts.clone();
                    }
                    guard.insert(uri.clone(), metadata.clone());
                    metadata_list.push(metadata);
                }
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
        let metadata = match guard.get_mut(uri) {
            Some(metadata) => metadata,
            None => {
                // If it's a static script, it might not be in dynamic scripts map yet
                let static_scripts = get_static_scripts();
                let Some(content) = static_scripts.get(uri) else {
                    return Err(RepositoryError::ScriptNotFound(uri.to_string()).into());
                };
                guard
                    .entry(uri.to_string())
                    .or_insert_with(|| ScriptMetadata::new(uri.to_string(), content.clone()))
            }
        };

        // Registrations from a *failed* init() are partial by definition — the
        // script stopped registering wherever it broke. They are worth
        // installing only when there is no working table to lose, which is the
        // case on a script's first init: a route table that is missing entries
        // still beats a script that answers nothing. An already-serving table
        // is kept instead, so a broken redeploy degrades to "running the old
        // routes" rather than to a partial set.
        if let Some(regs) = registrations
            && (initialized || metadata.registrations.is_empty())
        {
            metadata.registrations = regs;
        }
        // Routing reads `initialized` as "has a usable route table" (see
        // `route_index::build_index`), so it stays set whenever registrations
        // are installed; `init_error` is what records a failure.
        metadata.initialized = initialized || !metadata.registrations.is_empty();
        metadata.init_error = init_error;
        if initialized {
            metadata.last_init_time = Some(SystemTime::now());
        }
        drop(guard);
        crate::route_index::invalidate();
        Ok(())
    }

    async fn get_asset(&self, script_uri: &str, uri: &str) -> AppResult<Option<Asset>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_asset(&mut **tx, script_uri, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_asset(pool, script_uri, uri).await
            }
        }
    }

    async fn list_assets(&self, script_uri: &str) -> AppResult<HashMap<String, Asset>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_assets(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_assets(pool, script_uri).await
            }
        }
    }

    async fn upsert_asset(&self, asset: Asset) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_upsert_asset(executor, &asset).await?;

        // An imported asset is part of the owning script's prepared program, so
        // its change must invalidate the same caches a script edit does — both
        // locally and (via the refresh notification) on other cluster nodes.
        invalidate_script_asset_caches(&asset.script_uri, &asset.uri);
        send_script_notification(&self.pool, &asset.script_uri, "upserted", &self.server_id)
            .await?;
        Ok(())
    }

    /// The batch counterpart of [`Repository::upsert_asset`].
    ///
    /// Two things make this more than a loop over that method. The rows go in
    /// under one transaction, so a failure partway through leaves none of them
    /// behind. And the cache invalidation and the `script_upserted`
    /// notification happen once for the whole set: per-file notifications
    /// would make every other cluster node reinitialize the script once per
    /// file, each time from a set that is still being written.
    async fn upsert_assets(&self, script_uri: &str, assets: Vec<Asset>) -> AppResult<()> {
        if assets.is_empty() {
            return Ok(());
        }

        if crate::database::get_current_transaction_active() {
            // The caller already owns a transaction (a script writing inside
            // `transaction()`). Joining it is what every other repository
            // method does here, and committing our own would end theirs early.
            for asset in &assets {
                let executor = crate::database::get_current_executor(&self.pool);
                db_upsert_asset(executor, asset).await?;
            }
        } else {
            let mut tx = self.pool.begin().await.map_err(|e| {
                error!("Database error opening asset batch transaction: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
            for asset in &assets {
                db_upsert_asset(
                    crate::database::TransactionExecutor::Transaction(&mut tx),
                    asset,
                )
                .await?;
            }
            tx.commit().await.map_err(|e| {
                error!("Database error committing asset batch: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
        }

        for asset in &assets {
            invalidate_script_asset_caches(script_uri, &asset.uri);
        }
        send_script_notification(&self.pool, script_uri, "upserted", &self.server_id).await?;
        Ok(())
    }

    async fn delete_asset(&self, script_uri: &str, uri: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        let result = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_delete_asset(&mut **tx, script_uri, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_delete_asset(pool, script_uri, uri).await?
            }
        };

        if result {
            invalidate_script_asset_caches(script_uri, uri);
            send_script_notification(&self.pool, script_uri, "upserted", &self.server_id).await?;
        }
        Ok(result)
    }

    async fn insert_log(
        &self,
        script_uri: &str,
        message: &str,
        level: &str,
        context: &LogContext,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_insert_log_message(&mut **tx, script_uri, message, level, context).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_insert_log_message(pool, script_uri, message, level, context).await
            }
        }
    }

    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_fetch_log_messages(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_fetch_log_messages(pool, script_uri).await
            }
        }
    }

    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_fetch_all_log_messages(&mut **tx).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_fetch_all_log_messages(pool).await
            }
        }
    }

    async fn query_logs(&self, query: &LogQuery) -> AppResult<Vec<LogEntry>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_query_log_messages(&mut **tx, query).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_query_log_messages(pool, query).await
            }
        }
    }

    async fn clear_logs(&self, script_uri: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_log_messages(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_log_messages(pool, script_uri).await
            }
        }
    }

    async fn prune_logs(&self) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_prune_log_messages(&mut **tx).await
            }
            crate::database::TransactionExecutor::Pool(pool) => db_prune_log_messages(pool).await,
        }
    }

    async fn get_script_properties(
        &self,
        script_uri: &str,
        key: &str,
    ) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_properties_item(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_properties_item(pool, script_uri, key).await
            }
        }
    }

    async fn set_script_properties(
        &self,
        script_uri: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_script_properties_item(executor, script_uri, key, value).await
    }

    async fn remove_script_properties(&self, script_uri: &str, key: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_script_properties_item(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_script_properties_item(pool, script_uri, key).await
            }
        }
    }

    async fn clear_script_properties(&self, script_uri: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_script_properties(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_script_properties(pool, script_uri).await
            }
        }
    }

    async fn list_script_properties_keys(&self, script_uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_script_properties_keys(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_script_properties_keys(pool, script_uri).await
            }
        }
    }

    async fn get_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_user_properties_item(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_user_properties_item(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn set_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_user_properties_item(executor, script_uri, user_id, key, value).await
    }

    async fn remove_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_user_properties_item(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_user_properties_item(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn clear_user_properties(&self, script_uri: &str, user_id: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_user_properties(&mut **tx, script_uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_user_properties(pool, script_uri, user_id).await
            }
        }
    }

    async fn list_user_properties_keys(
        &self,
        script_uri: &str,
        user_id: &str,
    ) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_user_properties_keys(&mut **tx, script_uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_user_properties_keys(pool, script_uri, user_id).await
            }
        }
    }

    async fn get_script_secret(&self, script_uri: &str, key: &str) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_secret(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_secret(pool, script_uri, key).await
            }
        }
    }

    async fn set_script_secret(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_script_secret(executor, script_uri, key, value).await
    }

    async fn remove_script_secret(&self, script_uri: &str, key: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_script_secret(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_script_secret(pool, script_uri, key).await
            }
        }
    }

    async fn clear_script_secrets(&self, script_uri: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_script_secrets(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_script_secrets(pool, script_uri).await
            }
        }
    }

    async fn list_script_secrets(&self, script_uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_script_secrets(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_script_secrets(pool, script_uri).await
            }
        }
    }

    async fn get_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_user_secret(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_user_secret(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn set_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_user_secret(executor, script_uri, user_id, key, value).await
    }

    async fn remove_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_user_secret(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_user_secret(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn clear_user_secrets(&self, script_uri: &str, user_id: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_user_secrets(&mut **tx, script_uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_user_secrets(pool, script_uri, user_id).await
            }
        }
    }

    async fn add_script_owner(&self, uri: &str, user_id: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_add_script_owner(&mut **tx, uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_add_script_owner(pool, uri, user_id).await
            }
        }
    }

    async fn remove_script_owner(&self, uri: &str, user_id: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_script_owner(&mut **tx, uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_script_owner(pool, uri, user_id).await
            }
        }
    }

    async fn get_script_owners(&self, uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_owners(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_owners(pool, uri).await
            }
        }
    }

    async fn user_owns_script(&self, uri: &str, user_id: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_user_owns_script(&mut **tx, uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_user_owns_script(pool, uri, user_id).await
            }
        }
    }

    async fn count_script_owners(&self, uri: &str) -> AppResult<i64> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_count_script_owners(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_count_script_owners(pool, uri).await
            }
        }
    }

    async fn get_script_hosts(&self, uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_hosts(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_hosts(pool, uri).await
            }
        }
    }

    async fn set_script_hosts(&self, uri: &str, hosts: &[String]) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_script_hosts(executor, uri, hosts).await?;

        // The cached metadata carries the old bindings, and the route index was
        // built from them, so both have to go.
        if let Ok(mut guard) = safe_lock_scripts()
            && let Some(metadata) = guard.get_mut(uri)
        {
            metadata.hosts = hosts.to_vec();
        }
        crate::route_index::invalidate();
        // The per-host GraphQL schemas were built from the old bindings
        crate::graphql::invalidate_host_schemas();

        // Other instances cache the old bindings and would keep publishing the
        // script where it used to be. Reuse the upsert channel: their handler
        // refreshes the bindings along with the source.
        send_script_notification(&self.pool, uri, "upserted", &self.server_id).await?;

        Ok(())
    }

    async fn create_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<String> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let created = db_create_script_table(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(created).await
    }

    async fn add_column_to_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        column_type: ColumnType,
        nullable: bool,
        default_value: Option<&str>,
    ) -> AppResult<()> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let added = db_add_column_to_script_table(
            schema.conn(),
            script_uri,
            logical_table_name,
            column_name,
            column_type,
            nullable,
            default_value,
        )
        .await;
        schema.finish(added).await
    }

    async fn add_reference_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        referenced_logical_table_name: &str,
        nullable: bool,
    ) -> AppResult<()> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let added = db_add_reference_column(
            schema.conn(),
            script_uri,
            logical_table_name,
            column_name,
            referenced_logical_table_name,
            nullable,
        )
        .await;
        schema.finish(added).await
    }

    async fn drop_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
    ) -> AppResult<bool> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let dropped =
            db_drop_column(schema.conn(), script_uri, logical_table_name, column_name).await;
        schema.finish(dropped).await
    }

    async fn drop_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<bool> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let dropped = db_drop_script_table(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(dropped).await
    }

    async fn list_script_tables(&self, script_uri: &str) -> AppResult<Vec<TableInfo>> {
        let mut schema = ScopedConn::for_schema(&self.pool).await?;
        let listed = db_list_script_tables(schema.conn(), script_uri).await;
        schema.finish(listed).await
    }

    async fn get_table_schema(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<TableSchema> {
        let mut schema = ScopedConn::for_schema(&self.pool).await?;
        let fetched = db_get_table_schema(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(fetched).await
    }

    async fn get_foreign_keys(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Vec<ForeignKeyInfo>> {
        let mut schema = ScopedConn::for_schema(&self.pool).await?;
        let fetched = db_get_foreign_keys(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(fetched).await
    }

    async fn query_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: Option<&HashMap<String, serde_json::Value>>,
        limit: Option<i64>,
        order_by: Option<&str>,
        order_dir: Option<&str>,
    ) -> AppResult<Vec<serde_json::Value>> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let queried = db_query_table(
            scope.conn(),
            script_uri,
            logical_table_name,
            filters,
            limit,
            order_by,
            order_dir,
        )
        .await;
        scope.finish(queried).await
    }

    async fn insert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let inserted = db_insert_row(scope.conn(), script_uri, logical_table_name, data).await;
        scope.finish(inserted).await
    }

    async fn update_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let updated = db_update_row(scope.conn(), script_uri, logical_table_name, id, data).await;
        scope.finish(updated).await
    }

    async fn delete_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
    ) -> AppResult<bool> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let deleted = db_delete_row(scope.conn(), script_uri, logical_table_name, id).await;
        scope.finish(deleted).await
    }

    async fn upsert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        key_columns: &[String],
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let upserted = db_upsert_row(
            scope.conn(),
            script_uri,
            logical_table_name,
            key_columns,
            data,
        )
        .await;
        scope.finish(upserted).await
    }

    async fn delete_where(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: &HashMap<String, serde_json::Value>,
    ) -> AppResult<u64> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let deleted = db_delete_where(scope.conn(), script_uri, logical_table_name, filters).await;
        scope.finish(deleted).await
    }

    /// Leases run on their own pooled connection on purpose, unlike the row
    /// operations above: a lease taken inside a caller's transaction would be
    /// invisible to every other instance until that transaction committed, and
    /// would vanish on rollback — which defeats the point of a lease.
    ///
    /// The cost of that choice is the hazard [`ScopedConn`] exists to avoid: a
    /// caller whose own transaction has already written to the lease table
    /// blocks here on a row lock it holds itself, and nothing in Postgres can
    /// break the wait. Scripts must not write to a lease table directly.
    async fn acquire_lease(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        lease_id: &str,
        owner: &str,
        ttl_ms: i64,
    ) -> AppResult<serde_json::Value> {
        let mut conn = self.pool.acquire().await.map_err(|e| AppError::Database {
            message: format!("Failed to acquire connection: {}", e),
            source: None,
        })?;
        db_acquire_lease(
            &mut conn,
            script_uri,
            logical_table_name,
            lease_id,
            owner,
            ttl_ms,
        )
        .await
    }

    async fn create_lease_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<String> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let created = db_create_lease_table(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(created).await
    }

    async fn ensure_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        spec: &TableSpec,
    ) -> AppResult<EnsuredTable> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let ensured =
            db_ensure_script_table(schema.conn(), script_uri, logical_table_name, spec).await;
        schema.finish(ensured).await
    }

    async fn add_unique_index(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        columns: &[String],
    ) -> AppResult<()> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let added =
            db_add_unique_index(schema.conn(), script_uri, logical_table_name, columns).await;
        schema.finish(added).await
    }
}

/// Global secret encryption instance (optional — if not set, secrets are stored plaintext)
static GLOBAL_SECRET_ENCRYPTION: OnceLock<Arc<crate::security::encryption::DataEncryption>> =
    OnceLock::new();

/// Initialize the global secret encryption used for at-rest encryption of secret values.
/// Should be called once at startup if a `secret_encryption_key` is configured.
/// Returns `true` if initialized successfully, `false` if already initialized.
pub fn initialize_secret_encryption(enc: Arc<crate::security::encryption::DataEncryption>) -> bool {
    GLOBAL_SECRET_ENCRYPTION.set(enc).is_ok()
}

/// Global repository instance
static GLOBAL_REPOSITORY: OnceLock<PostgresRepository> = OnceLock::new();

/// Initialize the global repository
pub fn initialize_repository(repo: PostgresRepository) -> bool {
    GLOBAL_REPOSITORY.set(repo).is_ok()
}

/// Get the global repository
/// Returns the global repository if it has been initialized, or `None` otherwise.
/// Use this in contexts where the repository may not be available (e.g. tests without a DB).
pub fn get_repository_opt() -> Option<&'static PostgresRepository> {
    GLOBAL_REPOSITORY.get()
}

pub fn get_repository() -> &'static PostgresRepository {
    GLOBAL_REPOSITORY.get_or_init(|| {
        let db = crate::database::get_global_database()
            .expect("Database must be initialized before repository");

        // Fallback initialization with empty server_id (shouldn't happen in normal flow)
        warn!("Repository not initialized, using fallback with empty server_id");
        PostgresRepository::new(db.pool().clone(), String::new())
    })
}

#[cfg(test)]
pub static GLOBAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Once, OnceLock};

    static INIT: Once = Once::new();
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

    fn setup_db() {
        INIT.call_once(|| {
            // Skip if running in offline mode (CI/CD)
            if std::env::var("DATABASE_URL").is_err() {
                return;
            }

            let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine".to_string()
            });
            let pool = sqlx::PgPool::connect_lazy(&url).unwrap();
            let db = Arc::new(crate::database::Database::from_pool(pool.clone()));
            crate::database::initialize_global_database(db);

            // Generate and initialize server ID
            let server_id = crate::notifications::generate_server_id();
            crate::notifications::initialize_server_id(server_id.clone());

            // Initialize PostgresRepository with pool and server_id
            let repo = crate::repository::PostgresRepository::new(pool, server_id);
            crate::repository::initialize_repository(repo);
        });
    }

    fn get_runtime() -> &'static tokio::runtime::Runtime {
        RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
    }

    // Helper to check if we should skip database-dependent tests
    fn should_skip_db_tests() -> bool {
        std::env::var("DATABASE_URL").is_err()
    }

    fn initialized_metadata_with_route(uri: &str, content: &str) -> ScriptMetadata {
        let mut metadata = ScriptMetadata::new(uri.to_string(), content.to_string());
        let mut registrations = RouteRegistrations::new();
        registrations.insert(
            ("/keep-me".to_string(), "GET".to_string()),
            RouteMetadata::simple("keepMeHandler".to_string()),
        );
        metadata.mark_initialized_with_registrations(registrations);
        metadata
    }

    #[test]
    fn update_content_keeps_routes_serving_until_reinit() {
        let mut metadata = initialized_metadata_with_route("test://redeploy", "v1");
        metadata.init_error = Some("previous failure".to_string());

        metadata.update_content("v2".to_string());

        assert_eq!(metadata.content, "v2", "should serve the new source");
        assert!(
            metadata.initialized,
            "routes must keep serving while the re-init runs"
        );
        assert!(
            metadata
                .registrations
                .contains_key(&("/keep-me".to_string(), "GET".to_string())),
            "upserting source must not drop the route table"
        );
        assert!(
            metadata.init_error.is_none(),
            "the error describes the previous source"
        );
    }

    /// `update_script_init_status` only touches the in-memory metadata map, so a
    /// lazily-connected pool is enough — nothing in these tests reaches the
    /// database.
    fn metadata_only_repository() -> PostgresRepository {
        let pool = sqlx::PgPool::connect_lazy("postgresql://unused@localhost/unused")
            .expect("lazy pool should be constructible without connecting");
        PostgresRepository::new(pool, "test".to_string())
    }

    fn route_table(handler: &str) -> RouteRegistrations {
        let mut registrations = RouteRegistrations::new();
        registrations.insert(
            ("/probe".to_string(), "GET".to_string()),
            RouteMetadata::simple(handler.to_string()),
        );
        registrations
    }

    fn seed_metadata(uri: &str, metadata: ScriptMetadata) {
        safe_lock_scripts()
            .expect("scripts lock")
            .insert(uri.to_string(), metadata);
    }

    fn seeded_metadata(uri: &str) -> ScriptMetadata {
        safe_lock_scripts()
            .expect("scripts lock")
            .get(uri)
            .cloned()
            .expect("metadata should still be cached")
    }

    fn handler_at(metadata: &ScriptMetadata, path: &str, method: &str) -> Option<String> {
        metadata
            .registrations
            .get(&(path.to_string(), method.to_string()))
            .map(|route| route.handler_name.clone())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_init_keeps_the_route_table_that_is_already_serving() {
        let uri = "test://failed-init-keeps-table";
        let mut metadata = ScriptMetadata::new(uri.to_string(), "v1".to_string());
        metadata.mark_initialized_with_registrations(route_table("v1Handler"));
        seed_metadata(uri, metadata);

        metadata_only_repository()
            .update_script_init_status(
                uri,
                false,
                Some("init threw".to_string()),
                // Partial registrations from the failed attempt
                Some(route_table("v2Handler")),
            )
            .await
            .expect("status update should succeed");

        let metadata = seeded_metadata(uri);
        assert_eq!(
            handler_at(&metadata, "/probe", "GET").as_deref(),
            Some("v1Handler"),
            "a partial table from a failed init must not replace one that works"
        );
        assert!(metadata.initialized, "routes must keep serving");
        assert!(metadata.init_error.is_some(), "failure must be recorded");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_init_installs_partial_routes_when_nothing_is_serving() {
        let uri = "test://failed-init-installs-partial";
        seed_metadata(uri, ScriptMetadata::new(uri.to_string(), "v1".to_string()));

        metadata_only_repository()
            .update_script_init_status(
                uri,
                false,
                Some("init threw after registering".to_string()),
                Some(route_table("registeredBeforeFailing")),
            )
            .await
            .expect("status update should succeed");

        let metadata = seeded_metadata(uri);
        assert_eq!(
            handler_at(&metadata, "/probe", "GET").as_deref(),
            Some("registeredBeforeFailing"),
            "routes registered before the failure are better than no routes"
        );
        assert!(
            metadata.initialized,
            "routing skips scripts that are not marked initialized"
        );
        assert!(metadata.init_error.is_some(), "failure must be recorded");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn successful_init_replaces_the_previous_route_table() {
        let uri = "test://successful-init-replaces-table";
        let mut metadata = ScriptMetadata::new(uri.to_string(), "v1".to_string());
        metadata.mark_initialized_with_registrations(route_table("v1Handler"));
        seed_metadata(uri, metadata);

        metadata_only_repository()
            .update_script_init_status(uri, true, None, Some(route_table("v2Handler")))
            .await
            .expect("status update should succeed");

        let metadata = seeded_metadata(uri);
        assert_eq!(
            handler_at(&metadata, "/probe", "GET").as_deref(),
            Some("v2Handler"),
            "a successful init swaps in the new table"
        );
        assert!(metadata.initialized);
        assert!(metadata.init_error.is_none());
        assert!(metadata.last_init_time.is_some());
    }

    #[test]
    fn update_content_on_never_initialized_script_registers_nothing() {
        let mut metadata = ScriptMetadata::new("test://fresh".to_string(), "v1".to_string());

        metadata.update_content("v2".to_string());

        assert!(!metadata.initialized);
        assert!(metadata.registrations.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_operations() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
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
        if should_skip_db_tests() {
            return;
        }
        let _lock = GLOBAL_TEST_LOCK.lock().unwrap();
        // Test that bootstrap_scripts doesn't crash when database is available
        let result = bootstrap_scripts();
        assert!(
            result.is_ok(),
            "bootstrap_scripts should succeed even without database"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_properties_operations() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://storage-script";
        let key = "test_key";
        let value = "test_value";

        // Test set item
        assert!(set_script_properties_item(script_uri, key, value).is_ok());

        // Test get item
        let retrieved = get_script_properties_item(script_uri, key);
        assert_eq!(retrieved, Some(value.to_string()));

        // Test remove item
        assert!(remove_script_properties_item(script_uri, key));

        // Verify item is gone
        let retrieved_after_remove = get_script_properties_item(script_uri, key);
        assert_eq!(retrieved_after_remove, None);

        // Test clear storage
        assert!(set_script_properties_item(script_uri, "key1", "value1").is_ok());
        assert!(set_script_properties_item(script_uri, "key2", "value2").is_ok());

        assert!(clear_script_properties(script_uri).is_ok());

        // Verify both items are gone
        assert_eq!(get_script_properties_item(script_uri, "key1"), None);
        assert_eq!(get_script_properties_item(script_uri, "key2"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_properties_validation() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        // Test empty script URI
        assert!(set_script_properties_item("", "key", "value").is_err());

        // Test empty key
        assert!(set_script_properties_item("test://script", "", "value").is_err());

        // Test oversized value (simulate by creating a large string)
        let large_value = "x".repeat(1_000_001); // Just over 1MB
        assert!(set_script_properties_item("test://script", "key", &large_value).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_user_properties_operations() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://personal-storage-script";
        let user_id_1 = "user123";
        let user_id_2 = "user456";
        let key = "test_key";
        let value1 = "test_value_1";
        let value2 = "test_value_2";

        // Test set item for user 1
        assert!(set_user_properties_item(script_uri, user_id_1, key, value1).is_ok());

        // Test get item for user 1
        let retrieved1 = get_user_properties_item(script_uri, user_id_1, key);
        assert_eq!(retrieved1, Some(value1.to_string()));

        // Test set item for user 2 (same script, same key, different user)
        assert!(set_user_properties_item(script_uri, user_id_2, key, value2).is_ok());

        // Test get item for user 2
        let retrieved2 = get_user_properties_item(script_uri, user_id_2, key);
        assert_eq!(retrieved2, Some(value2.to_string()));

        // Verify user 1's data is still separate
        let still_user1 = get_user_properties_item(script_uri, user_id_1, key);
        assert_eq!(still_user1, Some(value1.to_string()));

        // Test remove item for user 1
        assert!(remove_user_properties_item(script_uri, user_id_1, key));

        // Verify item is gone for user 1
        let retrieved_after_remove = get_user_properties_item(script_uri, user_id_1, key);
        assert_eq!(retrieved_after_remove, None);

        // Verify user 2's data is still there
        let user2_still_there = get_user_properties_item(script_uri, user_id_2, key);
        assert_eq!(user2_still_there, Some(value2.to_string()));

        // Test clear storage for user 2
        assert!(set_user_properties_item(script_uri, user_id_2, "key1", "value1").is_ok());
        assert!(set_user_properties_item(script_uri, user_id_2, "key2", "value2").is_ok());

        assert!(clear_user_properties(script_uri, user_id_2).is_ok());

        // Verify all items are gone for user 2
        assert_eq!(get_user_properties_item(script_uri, user_id_2, key), None);
        assert_eq!(
            get_user_properties_item(script_uri, user_id_2, "key1"),
            None
        );
        assert_eq!(
            get_user_properties_item(script_uri, user_id_2, "key2"),
            None
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_user_properties_validation() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://script";
        let user_id = "user123";

        // Test empty script URI
        assert!(set_user_properties_item("", user_id, "key", "value").is_err());

        // Test empty user ID
        assert!(set_user_properties_item(script_uri, "", "key", "value").is_err());

        // Test empty key
        assert!(set_user_properties_item(script_uri, user_id, "", "value").is_err());

        // Test oversized value (simulate by creating a large string)
        let large_value = "x".repeat(1_000_001); // Just over 1MB
        assert!(set_user_properties_item(script_uri, user_id, "key", &large_value).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_user_properties_user_isolation() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://isolation-test";
        let user1 = "alice";
        let user2 = "bob";

        // Both users set the same key in the same script
        assert!(set_user_properties_item(script_uri, user1, "pref", "dark").is_ok());
        assert!(set_user_properties_item(script_uri, user2, "pref", "light").is_ok());

        // Each user should see only their own value
        assert_eq!(
            get_user_properties_item(script_uri, user1, "pref"),
            Some("dark".to_string())
        );
        assert_eq!(
            get_user_properties_item(script_uri, user2, "pref"),
            Some("light".to_string())
        );

        // Removing user1's data shouldn't affect user2
        assert!(remove_user_properties_item(script_uri, user1, "pref"));
        assert_eq!(get_user_properties_item(script_uri, user1, "pref"), None);
        assert_eq!(
            get_user_properties_item(script_uri, user2, "pref"),
            Some("light".to_string())
        );

        // Clearing user2's data shouldn't affect anything (since user1 already removed)
        assert!(clear_user_properties(script_uri, user2).is_ok());
        assert_eq!(get_user_properties_item(script_uri, user2, "pref"), None);
        assert_eq!(get_user_properties_item(script_uri, user1, "pref"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_secrets_operations() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://secrets-script";
        let key = "api_key";
        let value = "super_secret_value";

        // Test set item
        assert!(set_script_secret_item(script_uri, key, value).is_ok());

        // Test get item
        let retrieved = get_script_secret_item(script_uri, key);
        assert_eq!(retrieved, Some(value.to_string()));

        // Test overwrite
        let new_value = "updated_secret";
        assert!(set_script_secret_item(script_uri, key, new_value).is_ok());
        let retrieved_updated = get_script_secret_item(script_uri, key);
        assert_eq!(retrieved_updated, Some(new_value.to_string()));

        // Test remove item
        assert!(remove_script_secret_item(script_uri, key));

        // Verify item is gone
        let retrieved_after_remove = get_script_secret_item(script_uri, key);
        assert_eq!(retrieved_after_remove, None);

        // Test clear secrets
        assert!(set_script_secret_item(script_uri, "key1", "value1").is_ok());
        assert!(set_script_secret_item(script_uri, "key2", "value2").is_ok());

        assert!(clear_script_secrets(script_uri).is_ok());

        // Verify both items are gone
        assert_eq!(get_script_secret_item(script_uri, "key1"), None);
        assert_eq!(get_script_secret_item(script_uri, "key2"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_secrets_validation() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

        // Test empty script URI
        assert!(set_script_secret_item("", "key", "value").is_err());

        // Test empty key
        assert!(set_script_secret_item("test://script", "", "value").is_err());

        // Test oversized value
        let large_value = "x".repeat(1_000_001);
        assert!(set_script_secret_item("test://script", "key", &large_value).is_err());

        // Test secrets are scoped per script_uri
        let script_a = "test://secrets-scope-a";
        let script_b = "test://secrets-scope-b";
        assert!(set_script_secret_item(script_a, "scope_key", "value_a").is_ok());
        assert_eq!(
            get_script_secret_item(script_b, "scope_key"),
            None,
            "Secret from script_a must not be visible in script_b"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_user_secrets_operations() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://user-secrets-script";
        let user_id_1 = "user_secrets_123";
        let user_id_2 = "user_secrets_456";
        let key = "token";
        let value1 = "token_for_user1";
        let value2 = "token_for_user2";

        // Test set item for user 1
        assert!(set_user_secret_item(script_uri, user_id_1, key, value1).is_ok());

        // Test get item for user 1
        let retrieved1 = get_user_secret_item(script_uri, user_id_1, key);
        assert_eq!(retrieved1, Some(value1.to_string()));

        // Test set item for user 2 (same script, same key, different user)
        assert!(set_user_secret_item(script_uri, user_id_2, key, value2).is_ok());

        // Test get item for user 2
        let retrieved2 = get_user_secret_item(script_uri, user_id_2, key);
        assert_eq!(retrieved2, Some(value2.to_string()));

        // Verify user 1's data is still separate
        let still_user1 = get_user_secret_item(script_uri, user_id_1, key);
        assert_eq!(still_user1, Some(value1.to_string()));

        // Test overwrite for user 1
        let updated_value1 = "updated_token_for_user1";
        assert!(set_user_secret_item(script_uri, user_id_1, key, updated_value1).is_ok());
        assert_eq!(
            get_user_secret_item(script_uri, user_id_1, key),
            Some(updated_value1.to_string())
        );

        // Test remove item for user 1
        assert!(remove_user_secret_item(script_uri, user_id_1, key));

        // Verify item is gone for user 1
        let retrieved_after_remove = get_user_secret_item(script_uri, user_id_1, key);
        assert_eq!(retrieved_after_remove, None);

        // Verify user 2's data is still there
        let user2_still_there = get_user_secret_item(script_uri, user_id_2, key);
        assert_eq!(user2_still_there, Some(value2.to_string()));

        // Test clear for user 2
        assert!(set_user_secret_item(script_uri, user_id_2, "key1", "value1").is_ok());
        assert!(set_user_secret_item(script_uri, user_id_2, "key2", "value2").is_ok());

        assert!(clear_user_secrets(script_uri, user_id_2).is_ok());

        // Verify all items are gone for user 2
        assert_eq!(get_user_secret_item(script_uri, user_id_2, key), None);
        assert_eq!(get_user_secret_item(script_uri, user_id_2, "key1"), None);
        assert_eq!(get_user_secret_item(script_uri, user_id_2, "key2"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_user_secrets_validation() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://script";
        let user_id = "user_sec_123";

        // Test empty script URI
        assert!(set_user_secret_item("", user_id, "key", "value").is_err());

        // Test empty user ID
        assert!(set_user_secret_item(script_uri, "", "key", "value").is_err());

        // Test empty key
        assert!(set_user_secret_item(script_uri, user_id, "", "value").is_err());

        // Test oversized value
        let large_value = "x".repeat(1_000_001);
        assert!(set_user_secret_item(script_uri, user_id, "key", &large_value).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_user_secrets_user_isolation() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_uri = "test://user-secrets-isolation";
        let alice = "alice_secrets";
        let bob = "bob_secrets";

        // Both users set the same key in the same script
        assert!(set_user_secret_item(script_uri, alice, "api_token", "alice_token").is_ok());
        assert!(set_user_secret_item(script_uri, bob, "api_token", "bob_token").is_ok());

        // Each user should see only their own value
        assert_eq!(
            get_user_secret_item(script_uri, alice, "api_token"),
            Some("alice_token".to_string())
        );
        assert_eq!(
            get_user_secret_item(script_uri, bob, "api_token"),
            Some("bob_token".to_string())
        );

        // Removing alice's secret shouldn't affect bob
        assert!(remove_user_secret_item(script_uri, alice, "api_token"));
        assert_eq!(get_user_secret_item(script_uri, alice, "api_token"), None);
        assert_eq!(
            get_user_secret_item(script_uri, bob, "api_token"),
            Some("bob_token".to_string())
        );

        // Clearing bob's secrets
        assert!(clear_user_secrets(script_uri, bob).is_ok());
        assert_eq!(get_user_secret_item(script_uri, bob, "api_token"), None);
        assert_eq!(get_user_secret_item(script_uri, alice, "api_token"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_ownership_assignment() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        // Test that new scripts get assigned an owner
        let script_uri = "test://owned-script";
        let owner_user_id = "test-user-123";
        let script_code = "console.log('owned script');";

        // Create a new script with an owner
        let result = upsert_script_with_owner(script_uri, script_code, Some(owner_user_id));
        assert!(
            result.is_ok(),
            "Should successfully create script with owner"
        );

        // Verify the script was created
        let fetched = fetch_script(script_uri);
        assert_eq!(
            fetched,
            Some(script_code.to_string()),
            "Script should exist"
        );

        // Verify the owner was assigned
        let owners = get_script_owners(script_uri).expect("Should get owners");
        assert_eq!(owners.len(), 1, "Script should have exactly one owner");
        assert_eq!(
            owners[0], owner_user_id,
            "Owner should be the specified user"
        );

        // Verify user_owns_script returns true
        let owns = user_owns_script(script_uri, owner_user_id).expect("Should check ownership");
        assert!(owns, "User should own the script");

        // Verify count_script_owners returns 1
        let count = count_script_owners(script_uri).expect("Should count owners");
        assert_eq!(count, 1, "Should have exactly 1 owner");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_ownership_backfill() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        // Test that existing scripts without owners get backfilled
        let script_uri = "test://backfill-script";
        let script_code = "console.log('backfill test');";

        // Clean up any stale state from a prior run
        let _ = delete_script(script_uri);

        // First, create script without owner (simulate old script)
        let result = upsert_script(script_uri, script_code);
        assert!(result.is_ok(), "Should create script");

        // Verify it has no owners
        let owners_before = get_script_owners(script_uri).expect("Should get owners");
        assert_eq!(
            owners_before.len(),
            0,
            "Script should have no owners initially"
        );

        // Now update the script with a user (simulating editing in the editor)
        let editor_user_id = "editor-user-456";
        let updated_code = "console.log('backfill test - updated');";
        let result = upsert_script_with_owner(script_uri, updated_code, Some(editor_user_id));
        assert!(result.is_ok(), "Should update script with owner backfill");

        // Verify the owner was backfilled
        let owners_after = get_script_owners(script_uri).expect("Should get owners");
        assert_eq!(owners_after.len(), 1, "Script should now have one owner");
        assert_eq!(
            owners_after[0], editor_user_id,
            "Owner should be the editor"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_ownership_no_duplicate() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        // Test that scripts with existing owners don't get duplicates
        let script_uri = "test://no-duplicate-script";
        let owner_user_id = "original-owner";
        let script_code = "console.log('original');";

        // Create script with owner
        let result = upsert_script_with_owner(script_uri, script_code, Some(owner_user_id));
        assert!(result.is_ok(), "Should create script with owner");

        // Update script with same user
        let updated_code = "console.log('updated');";
        let result = upsert_script_with_owner(script_uri, updated_code, Some(owner_user_id));
        assert!(result.is_ok(), "Should update script");

        // Verify still only one owner
        let owners = get_script_owners(script_uri).expect("Should get owners");
        assert_eq!(owners.len(), 1, "Should still have exactly one owner");
        assert_eq!(owners[0], owner_user_id, "Owner should be unchanged");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_ownership_add_remove() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        // Test adding and removing owners
        let script_uri = "test://multi-owner-script";
        let owner1 = "owner-one";
        let owner2 = "owner-two";
        let script_code = "console.log('multi-owner');";

        // Clean up any stale state from a prior run
        let _ = delete_script(script_uri);

        // Create script with first owner
        upsert_script_with_owner(script_uri, script_code, Some(owner1))
            .expect("Should create script");

        // Add second owner
        let result = add_script_owner(script_uri, owner2);
        assert!(result.is_ok(), "Should add second owner");

        // Verify both owners exist
        let owners = get_script_owners(script_uri).expect("Should get owners");
        assert_eq!(owners.len(), 2, "Should have two owners");
        assert!(
            owners.contains(&owner1.to_string()),
            "Should contain owner1"
        );
        assert!(
            owners.contains(&owner2.to_string()),
            "Should contain owner2"
        );

        // Remove first owner
        let result = remove_script_owner(script_uri, owner1);
        assert!(result.is_ok(), "Should remove first owner");

        // Verify only second owner remains
        let owners_after = get_script_owners(script_uri).expect("Should get owners");
        assert_eq!(owners_after.len(), 1, "Should have one owner");
        assert_eq!(owners_after[0], owner2, "Only owner2 should remain");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_ownership_without_user() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        // Test that scripts created without a user_id don't crash
        let script_uri = "test://no-user-script";
        let script_code = "console.log('no user');";

        // Create script without owner (None user_id)
        let result = upsert_script_with_owner(script_uri, script_code, None);
        assert!(result.is_ok(), "Should create script even without user_id");

        // Verify script exists but has no owners
        let fetched = fetch_script(script_uri);
        assert_eq!(
            fetched,
            Some(script_code.to_string()),
            "Script should exist"
        );

        let owners = get_script_owners(script_uri).expect("Should get owners");
        assert_eq!(owners.len(), 0, "Script should have no owners");
    }
}
