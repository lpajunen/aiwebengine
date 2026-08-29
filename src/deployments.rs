//! What a script serves, when that is not simply its newest revision.
//!
//! Writing a script's files used to *be* deploying them. That is right for one
//! person editing their own script and wrong as soon as an agent is editing
//! modules other people are using: every experiment reaches production because
//! there is nowhere else for it to go.
//!
//! A pin separates the two. Writes still record revisions and still advance
//! head; they stop being deployments. What is served changes when somebody
//! says so.
//!
//! No pin means follow head — what every script does today — so nothing
//! changes for anyone who does not opt in.

use sqlx::Row;

use crate::error::{AppError, AppResult};
use crate::revisions;

/// A script's deployment, as stored.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub script_uri: String,
    pub revision: i32,
    pub deployed_at: chrono::DateTime<chrono::Utc>,
    pub deployed_by: Option<String>,
    pub init_ok: Option<bool>,
    pub init_error: Option<String>,
}

fn db_error(context: &str, e: sqlx::Error) -> AppError {
    tracing::error!("Database error {}: {}", context, e);
    AppError::Database {
        message: format!("Database error {}: {}", context, e),
        source: None,
    }
}

fn pool() -> AppResult<sqlx::PgPool> {
    crate::repository::get_db_pool()
        .map(|db| db.pool().clone())
        .ok_or_else(|| AppError::Database {
            message: "No database configured".to_string(),
            source: None,
        })
}

// ============================================================================
// What this instance is serving
// ============================================================================

/// The pin each script has here, as far as this instance knows.
///
/// Consulted on every build of a script's program, so it cannot be a query.
/// It is also the more accurate answer: an instance that has not yet heard
/// about a pin someone set elsewhere is still serving what it was, and its
/// behaviour should follow what it believes rather than what is stored.
static PINNED: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, i32>>> =
    std::sync::OnceLock::new();

fn pinned_map() -> &'static std::sync::RwLock<std::collections::HashMap<String, i32>> {
    PINNED.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// The revision `script_uri` is pinned to here, or `None` when it follows head.
pub fn pinned(script_uri: &str) -> Option<i32> {
    pinned_map()
        .read()
        .ok()
        .and_then(|map| map.get(script_uri).copied())
}

/// Whether any script is pinned at all.
///
/// The write path asks this before doing anything different, so an engine
/// where nobody pins pays one atomic read rather than a map lookup per write.
pub fn any_pinned() -> bool {
    pinned_map()
        .read()
        .map(|map| !map.is_empty())
        .unwrap_or(false)
}

fn remember(script_uri: &str, revision: i32) {
    if let Ok(mut map) = pinned_map().write() {
        map.insert(script_uri.to_string(), revision);
    }
}

fn forget(script_uri: &str) {
    if let Ok(mut map) = pinned_map().write() {
        map.remove(script_uri);
    }
}

/// The revision whose code is running here.
///
/// The pin when there is one, and otherwise the newest revision this instance
/// knows about — which is what an unpinned script runs.
///
/// This is the answer to "which version did that?", and it has two callers
/// that both need it to be right: the outcome of `init()`, which decides where
/// a rollback can land, and the attribution on every log line. Head is not the
/// answer. A pinned script's head is a revision nothing has executed, and
/// crediting it with an outcome it never earned turns `lastGood` into an
/// assurance the engine cannot back.
pub fn serving_revision(script_uri: &str) -> Option<i32> {
    pinned(script_uri).or_else(|| crate::revisions::current(script_uri))
}

/// The view a script's program is built from here.
///
/// `SourceView::Live` for an unpinned script, which is every script until
/// somebody pins one — so this is the identity function for existing
/// deployments and stays that way.
pub fn serving_view(script_uri: &str) -> crate::source_view::SourceView {
    match pinned(script_uri) {
        Some(revision) => crate::source_view::SourceView::Revision(revision),
        None => crate::source_view::SourceView::Live,
    }
}

// ============================================================================
// Reading
// ============================================================================

/// One script's deployment, or `None` when it follows head.
pub async fn get(script_uri: &str) -> AppResult<Option<Deployment>> {
    let pool = pool()?;
    let row = sqlx::query(
        "SELECT script_uri, revision, deployed_at, deployed_by, init_ok, init_error
         FROM script_deployments WHERE script_uri = $1",
    )
    .bind(script_uri)
    .fetch_optional(&pool)
    .await
    .map_err(|e| db_error("reading deployment", e))?;

    Ok(row.map(|row| Deployment {
        script_uri: row.get("script_uri"),
        revision: row.get("revision"),
        deployed_at: row.get("deployed_at"),
        deployed_by: row.get("deployed_by"),
        init_ok: row.get("init_ok"),
        init_error: row.get("init_error"),
    }))
}

/// Load every pin in one query.
///
/// Run at startup, before scripts are executed, so an instance coming up
/// serves what it is pinned to rather than briefly serving head.
pub async fn load_pins() {
    let Ok(pool) = pool() else {
        return;
    };
    match sqlx::query("SELECT script_uri, revision FROM script_deployments")
        .fetch_all(&pool)
        .await
    {
        Ok(rows) => {
            if let Ok(mut map) = pinned_map().write() {
                map.clear();
                for row in rows {
                    map.insert(row.get("script_uri"), row.get("revision"));
                }
            }
        }
        Err(e) => tracing::warn!("Could not load script deployments: {}", e),
    }
}

/// Re-read one script's pin, for an instance picking up someone else's change.
pub async fn refresh(script_uri: &str) {
    match get(script_uri).await {
        Ok(Some(deployment)) => remember(script_uri, deployment.revision),
        Ok(None) => forget(script_uri),
        Err(e) => tracing::debug!(
            script = script_uri,
            "Could not refresh the deployment: {}",
            e
        ),
    }
}

/// Forget a script, because it is gone.
pub fn forget_pin(script_uri: &str) {
    forget(script_uri);
}

// ============================================================================
// Writing
// ============================================================================

/// Why a deployment was refused.
#[derive(Debug)]
pub enum DeployRefusal {
    /// The script has no such revision.
    NoSuchRevision(String),
    Storage(String),
}

/// Pin `script_uri` to `revision`.
///
/// The revision has to exist; a pin naming one that does not would be a script
/// serving nothing. Callers that mean "whatever is newest" resolve that to a
/// number first, so the pin records what was deployed rather than a moving
/// target — the point of pinning is that it stops moving.
pub async fn deploy(
    script_uri: &str,
    revision: i32,
    deployed_by: Option<&str>,
) -> Result<Deployment, DeployRefusal> {
    let exists = revisions::get(script_uri, revision)
        .await
        .map_err(|e| DeployRefusal::Storage(format!("Failed to read revision: {}", e)))?;
    if exists.is_none() {
        return Err(DeployRefusal::NoSuchRevision(format!(
            "Script '{}' has no revision {}",
            script_uri, revision
        )));
    }

    let pool = pool().map_err(|e| DeployRefusal::Storage(e.to_string()))?;
    sqlx::query(
        "INSERT INTO script_deployments (script_uri, revision, deployed_by)
         VALUES ($1, $2, $3)
         ON CONFLICT (script_uri) DO UPDATE
         SET revision = EXCLUDED.revision,
             deployed_at = NOW(),
             deployed_by = EXCLUDED.deployed_by,
             init_ok = NULL,
             init_error = NULL",
    )
    .bind(script_uri)
    .bind(revision)
    .bind(deployed_by)
    .execute(&pool)
    .await
    .map_err(|e| DeployRefusal::Storage(format!("Failed to store deployment: {}", e)))?;

    remember(script_uri, revision);

    get(script_uri)
        .await
        .map_err(|e| DeployRefusal::Storage(e.to_string()))?
        .ok_or_else(|| DeployRefusal::Storage("Deployment vanished after storing".to_string()))
}

/// Stop pinning `script_uri`, so it follows head again.
///
/// Returns whether it was pinned. Unpinning is not a deployment of head — it
/// is a decision to stop deciding, and the caller re-initialises the script so
/// what runs matches what it now follows.
pub async fn unpin(script_uri: &str) -> AppResult<bool> {
    let pool = pool()?;
    let removed = sqlx::query("DELETE FROM script_deployments WHERE script_uri = $1")
        .bind(script_uri)
        .execute(&pool)
        .await
        .map_err(|e| db_error("removing deployment", e))?
        .rows_affected()
        > 0;

    forget(script_uri);
    Ok(removed)
}

/// Record how `init()` went for the revision this script is pinned to.
pub async fn record_init(script_uri: &str, ok: bool, error: Option<&str>) {
    let Ok(pool) = pool() else {
        return;
    };
    if let Err(e) = sqlx::query(
        "UPDATE script_deployments SET init_ok = $2, init_error = $3 WHERE script_uri = $1",
    )
    .bind(script_uri)
    .bind(ok)
    .bind(error)
    .execute(&pool)
    .await
    {
        tracing::warn!(
            script = script_uri,
            "Failed to record the deployment's init outcome: {}",
            e
        );
    }
}
