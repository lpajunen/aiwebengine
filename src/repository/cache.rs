//! The in-memory script metadata cache, its invalidation, and the cluster notifications
//! that keep every instance's copy current.

use super::*;
use crate::error::{AppError, AppResult};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use tracing::{debug, error, warn};

pub(super) static DYNAMIC_SCRIPTS: OnceLock<Mutex<HashMap<String, ScriptMetadata>>> =
    OnceLock::new();

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

/// Drop the caches that depend on one of a script's files: the compiled
/// bytecode (keyed by root URI), the prepared program bundle, and the cached
/// source of the file that changed. The script's *other* module sources stay
/// cached, so the rebuild re-reads one file rather than all of them.
///
/// The root module goes through here too, because it is one of the files now.
/// It needs more than the others: the in-memory metadata cache holds the root
/// source, and it is what every execution path reads — a write that
/// invalidated only the module caches would leave requests running the
/// previous entrypoint until the entry aged out, which it never does. That is
/// the work [`Repository::upsert_script`] does after its own write, and it is
/// the same work here because it is the same write.
///
/// `content` is what was just stored, when the caller has it. `None` means a
/// removal, or a write whose bytes were not kept: the cached source is then
/// dropped rather than corrected, and the next read loads it from the
/// database.
pub(super) fn invalidate_script_asset_caches(
    script_uri: &str,
    asset_path: &str,
    content: Option<&[u8]>,
) {
    crate::bytecode::invalidate(script_uri);
    crate::module_loader::invalidate_asset(script_uri, asset_path);

    if !crate::module_loader::is_root_module_name(asset_path) {
        return;
    }

    // A pinned script serves a revision, so a write to its files is not a
    // change to what is running. Refreshing the cache or dropping the prepared
    // program here would swap the deployment for head — which is the one thing
    // pinning exists to prevent.
    if crate::deployments::pinned(script_uri).is_some() {
        return;
    }

    // Counted like the write it is. [`get_script_metadata`] fills the cache
    // across an await: it reads the database, then takes the lock and
    // installs what it read. A write landing in that gap is overwritten by
    // the older content the reader already had in hand, and the cache — which
    // is what `fetch_script` answers from — stays permanently behind the
    // database. A root written as an ordinary file of the tree reaches the
    // cache through here, so the write is counted here.
    note_script_write();

    match content.map(|bytes| std::str::from_utf8(bytes)) {
        // Refresh in place rather than evicting: eviction would also drop the
        // script's route registrations, 404ing every one of its routes until
        // the re-init that follows this write completes.
        Some(Ok(text)) => refresh_cached_script_source(script_uri, text),
        _ => {
            if let Ok(mut guard) = safe_lock_scripts() {
                guard.remove(script_uri);
            }
        }
    }
    crate::route_index::invalidate();
    crate::module_loader::invalidate_program(script_uri);
}

/// How many times a script's stored source has changed under this process.
///
/// Read by [`note_script_write`] and by the cache fill in `get_script_metadata`,
/// which is a read-modify-write across an `await`: it looks in the cache, reads
/// the database when it misses, and only then takes the lock again to store
/// what it read. A write landing in that gap updates the database and the cache
/// and is then overwritten by the older content the reader already had in hand
/// — leaving the cache, which is what `fetch_script` answers from, permanently
/// behind the database. A first write is where it bites, because
/// [`refresh_cached_script_source`] does nothing when the script is not cached
/// yet, so the stale fill has no fresher entry to lose to.
///
/// Counting writes rather than versioning entries keeps this to two atomic
/// loads: the reader takes the count before its database read and again before
/// caching, and declines to cache when it moved. Coarse on purpose — a write to
/// any script makes every concurrent fill skip — which costs one repeated
/// database read of a script nobody wrote, and script writes happen at the rate
/// people deploy.
pub(super) static SCRIPT_WRITES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The write count a cache fill should compare against.
pub(super) fn script_write_count() -> u64 {
    SCRIPT_WRITES.load(std::sync::atomic::Ordering::SeqCst)
}

/// Record that a script's stored source changed, so a cache fill that read the
/// database before this point does not install what it read.
pub(super) fn note_script_write() {
    SCRIPT_WRITES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Point the metadata cache at `content` without disturbing the script's route
/// registrations, so requests keep routing while the re-init that follows an
/// upsert runs. Does nothing when the script is not cached — the next
/// `get_script_metadata` loads it from the database.
pub(super) fn refresh_cached_script_source(uri: &str, content: &str) {
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
    let content = read_served_source(uri).await;
    // Counted like a local write: this changes what the cache holds too, and a
    // fill still in flight would otherwise put back what it read before.
    note_script_write();
    match content {
        Some(content) => refresh_cached_script_source(uri, &content),
        None => {
            if let Ok(mut guard) = safe_lock_scripts() {
                guard.remove(uri);
            }
        }
    }
}

/// The root source `uri` serves: its pinned revision's, or the stored one.
///
/// The cache this feeds is what every execution path reads the root from, so
/// putting head's source there for a pinned script would run that script's
/// modules under a root from a version they were never written for.
pub async fn read_served_source(uri: &str) -> Option<String> {
    if let Some(revision) = crate::deployments::pinned(uri) {
        match crate::revisions::root_content(uri, revision).await {
            Ok(Some(content)) => return Some(content),
            Ok(None) => {
                warn!(
                    "Script '{}' is pinned to revision {}, which has no stored source; \
                     falling back to what is stored",
                    uri, revision
                );
            }
            Err(e) => {
                warn!(
                    "Could not read revision {} of '{}': {}; falling back to what is stored",
                    revision, uri, e
                );
            }
        }
    }

    get_repository().get_script(uri).await.ok().flatten()
}

/// Tell the rest of the cluster that `uri` changed.
///
/// The write paths do this as part of storing. A deployment changes what a
/// script serves without changing a row any of them touch, so it says so
/// itself — on the same channel, because the receiving side does the same
/// thing either way: re-read what the script serves, and initialise it.
pub async fn notify_script_changed(uri: &str) {
    let Some(repo) = GLOBAL_REPOSITORY.get() else {
        return;
    };
    if let Err(e) = send_script_notification(&repo.pool, uri, "upserted", &repo.server_id).await {
        warn!("Failed to announce the change to '{}': {}", uri, e);
    }
}

/// Put what `uri` serves into the in-memory source cache.
///
/// Called when a pin changes and at startup, where the cache is otherwise
/// filled from the `scripts` table — which is head, not necessarily what the
/// script serves.
pub async fn refresh_served_source(uri: &str) {
    if let Some(content) = read_served_source(uri).await {
        refresh_cached_script_source(uri, &content);
    }
}

pub(super) async fn send_script_notification(
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
