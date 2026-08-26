//! Native engine management API for scripts and assets.
//!
//! These REST routes and MCP tools are engine functionality, so they live in
//! Rust. They are the only way to administer scripts, assets, users, secrets
//! and logs: the JavaScript sandbox exposes no engine-management API, and every
//! call here is authorized against the calling user's capabilities, ownership
//! of the target script, and role.

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::auth::AuthUser;
use crate::error::AppResult;
use crate::repository;
use crate::revisions;
use crate::security::{
    Capability, SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity, UserContext,
};

/// Maximum asset size accepted by the write paths (same limit as the sandbox).
const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;

/// Maximum number of files one batch write may carry.
const MAX_BATCH_FILES: usize = 256;

/// Maximum total decoded size of one batch write. The per-file ceiling still
/// applies to each file, so a batch cannot smuggle in an oversized asset.
const MAX_BATCH_BYTES: usize = MAX_ASSET_BYTES;

/// Request body ceiling for the batch route: the decoded ceiling, plus the
/// third base64 adds, plus room for the JSON envelope around it.
///
/// The management router's own limit is `security.max_request_body_bytes`,
/// which defaults to 1MB — far below what a batch of source files needs — so
/// the batch route overrides it (see `lib.rs`).
pub const MAX_BATCH_BODY_BYTES: usize = MAX_BATCH_BYTES * 4 / 3 + 1024 * 1024;

/// Maximum number of edits one patch may carry.
const MAX_ASSET_EDITS: usize = 128;

/// How many matching lines a `grep=` read reports before it stops looking.
const MAX_GREP_MATCHES: usize = 200;

/// How much of a matching line a `grep=` read echoes back.
const MAX_GREP_LINE_CHARS: usize = 512;

/// Longest `grep=` pattern accepted, so a read cannot hand the regex engine an
/// arbitrarily large program to compile.
const MAX_GREP_PATTERN_CHARS: usize = 512;

fn iso_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn user_context_from(auth_user: Option<&AuthUser>) -> UserContext {
    match auth_user {
        Some(user) if user.is_admin => UserContext::admin(user.user_id.clone()),
        Some(user) => UserContext::authenticated(user.user_id.clone()),
        None => UserContext::anonymous(),
    }
}

fn auditor() -> SecurityAuditor {
    let pool = crate::database::get_global_database().map(|db| db.pool().clone());
    SecurityAuditor::new(pool)
}

fn user_owns_script(user: &UserContext, script_uri: &str) -> bool {
    match &user.user_id {
        Some(user_id) => repository::user_owns_script(script_uri, user_id).unwrap_or(false),
        None => false,
    }
}

/// Capability-only admin check: does this caller hold `DeleteScripts`?
///
/// **This does not mean the caller is an authenticated administrator.**
/// Development mode grants `DeleteScripts` to *anonymous* callers
/// (`UserContext::anonymous_capabilities`), so this returns true with no
/// session at all. It is the right check for script and asset operations,
/// where that elevation is the point of development mode.
///
/// For anything that exposes user data, engine topology, or role changes, use
/// [`is_user_admin`], which additionally requires an authenticated session.
/// Whether a caller may reach the engine's administration surface at all.
///
/// Capabilities alone do not answer this. An anonymous caller holds
/// `ReadScripts` and `ReadAssets` on purpose: a script serving a public
/// request runs with the requesting user's context, and reads its own modules
/// and assets through the sandbox with those capabilities, so a public page
/// would stop working without them. Who may read a script's tree *through*
/// `/engine/*` is a different question, and this is where the two part —
/// otherwise an unauthenticated caller can list, download, and now search
/// every script's files, and a script that carries an embedded credential
/// carries it in public.
///
/// Development mode stays open: it grants anonymous callers elevated
/// capabilities by design, so a local instance can be driven without a login.
fn may_administer(user: &UserContext) -> bool {
    user.is_authenticated || crate::security::is_development_mode()
}

fn has_admin_capability(user: &UserContext) -> bool {
    user.has_capability(&Capability::DeleteScripts)
}

fn is_admin_or_owner(user: &UserContext, script_uri: &str) -> bool {
    has_admin_capability(user) || user_owns_script(user, script_uri)
}

/// Path prefixes owned by the engine. Scripts may not register HTTP, stream,
/// or asset routes at or under these prefixes; every other path is open to
/// any script. `/` and `/favicon.ico` are intentionally not reserved — the
/// engine serves defaults for them only when no script claims them.
pub const RESERVED_ROUTE_PREFIXES: &[&str] = &[
    "/health",
    "/graphql",
    "/mcp",
    "/auth",
    "/.well-known",
    "/engine",
];

/// The engine-owned SSE stream carrying script change notifications.
///
/// Lives under the reserved `/engine` prefix so that a script cannot register
/// a stream on this path: [`crate::stream_registry::StreamRegistry`] replaces
/// an existing registration that has no active connections, so an unreserved
/// path would let a script take ownership of the engine's stream.
pub const ENGINE_SCRIPT_UPDATES_STREAM: &str = "/engine/script_updates";

/// Hosts allowed to serve the management APIs, normalized to Host-header form.
/// Empty (or unset) means every host serves them, which is what a single-host
/// deployment wants. Set once at startup from `server.management_hosts`.
static MANAGEMENT_HOSTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Record which hosts may serve the management APIs. Called once at startup;
/// later calls are ignored so the boundary cannot be widened at runtime.
pub fn init_management_hosts(hosts: Vec<String>) {
    let _ = MANAGEMENT_HOSTS.set(hosts);
}

/// Whether `host` may serve the management APIs, given the configured list.
///
/// An empty list allows every host. Otherwise the match is exact against the
/// normalized entries, and a request without a Host header is refused — HTTP
/// requires one, so its absence should not open the boundary.
fn host_is_allowed(allowed: &[String], host: Option<&str>) -> bool {
    if allowed.is_empty() {
        return true;
    }
    match host {
        Some(host) => allowed.contains(&host.trim().to_lowercase()),
        None => false,
    }
}

/// Whether a request arriving on `host` may reach the management APIs.
pub fn is_management_host(host: Option<&str>) -> bool {
    match MANAGEMENT_HOSTS.get() {
        Some(allowed) => host_is_allowed(allowed, host),
        // Not configured yet (tests constructing routers directly, or startup
        // ordering) — behave as an unrestricted single-host deployment.
        None => true,
    }
}

/// Returns the reserved prefix that `path` falls under, if any.
pub fn reserved_route_prefix(path: &str) -> Option<&'static str> {
    RESERVED_ROUTE_PREFIXES.iter().copied().find(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Broadcast a script update to the `/engine/script_updates` stream, matching
/// the message format core.js used. Extra `details` entries become message
/// metadata used for connection filtering.
pub fn broadcast_script_update(uri: &str, action: &str, details: &[(&str, Value)]) {
    let mut message = json!({
        "type": "script_update",
        "uri": uri,
        "action": action,
        "timestamp": iso_timestamp(),
    });
    if let Some(obj) = message.as_object_mut() {
        for (key, value) in details {
            obj.insert((*key).to_string(), value.clone());
        }
    }

    match crate::stream_registry::GLOBAL_STREAM_REGISTRY
        .broadcast_to_stream(ENGINE_SCRIPT_UPDATES_STREAM, &message.to_string())
    {
        Ok(_) => debug!("Broadcasted script update: {} {}", action, uri),
        Err(e) => warn!("Failed to broadcast script update for {}: {}", uri, e),
    }
}

/// Register engine-provided streams. Called once at startup.
///
/// [`ENGINE_SCRIPT_UPDATES_STREAM`] carries the script change notifications
/// broadcast by [`broadcast_script_update`]. There is no customization
/// function, so a connection's filter criteria come from its query parameters —
/// a client connecting without any receives all messages, exactly as before.
pub fn register_engine_streams() {
    if let Err(e) = crate::stream_registry::GLOBAL_STREAM_REGISTRY.register_stream(
        ENGINE_SCRIPT_UPDATES_STREAM,
        "engine://native",
        None,
    ) {
        warn!(
            "Failed to register {} stream: {}",
            ENGINE_SCRIPT_UPDATES_STREAM, e
        );
    }
}

/// Re-initialize a script after an upsert: clear its GraphQL/MCP
/// registrations, run init(), and rebuild the GraphQL schema.
///
/// Every local deploy path funnels through here, so what a script upsert does
/// to a script's registrations and what a batch asset write does to them
/// cannot drift apart.
async fn reinitialize_script(script_uri: &str) -> crate::script_init::InitResult {
    crate::graphql::clear_script_graphql_registrations(script_uri);
    crate::mcp::clear_script_mcp_registrations(script_uri);

    let initializer = crate::script_init::ScriptInitializer::with_configured_timeout();
    match initializer.initialize_script(script_uri, false).await {
        Ok(result) => {
            if result.success {
                if let Err(e) = crate::graphql::rebuild_schema() {
                    warn!(
                        "Failed to rebuild GraphQL schema after script '{}' initialization: {:?}",
                        script_uri, e
                    );
                }
            } else if let Some(err) = &result.error {
                warn!("Script '{}' init failed after upsert: {}", script_uri, err);
            }
            result
        }
        Err(e) => {
            warn!(
                "Failed to initialize script '{}' after upsert: {}",
                script_uri, e
            );
            crate::script_init::InitResult::failed(script_uri.to_string(), e.to_string(), 0)
        }
    }
}

/// [`reinitialize_script`] in the background, for the callers that answer
/// before init() finishes.
fn spawn_script_init(script_uri: String) {
    tokio::task::spawn(async move {
        reinitialize_script(&script_uri).await;
    });
}

/// An init() run reported back to whoever triggered it.
/// Resolve a caller's `revision` argument to a view of a script's files.
///
/// Accepts a number, `head`, `last-good`, or a label. Absent means the
/// deployed state — so every existing caller keeps checking and testing what
/// is serving, which is what they have always meant.
///
/// `last-good` is the one worth spelling out: it names the newest revision
/// whose write left `init()` succeeding, which is the target an operator means
/// by "back to when it worked" and a number they would otherwise have to find
/// by reading the history themselves.
async fn resolve_view(
    script_uri: &str,
    revision: Option<&str>,
) -> Result<crate::source_view::SourceView, String> {
    use crate::source_view::SourceView;

    let Some(spec) = revision.map(str::trim).filter(|spec| !spec.is_empty()) else {
        return Ok(SourceView::Live);
    };

    let resolved = match spec {
        "head" => revisions::head(script_uri)
            .await
            .map_err(|e| format!("Failed to read revision history: {}", e))?
            .ok_or_else(|| format!("Script '{}' has no revisions yet", script_uri))?,
        "last-good" => revisions::last_good(script_uri)
            .await
            .map_err(|e| format!("Failed to read revision history: {}", e))?
            .ok_or_else(|| {
                format!(
                    "Script '{}' has no revision whose init() succeeded",
                    script_uri
                )
            })?,
        _ => match spec.parse::<i32>() {
            Ok(number) => {
                // Named explicitly, so a number that is not there is an error
                // rather than a silent fall back to what is deployed.
                if revisions::get(script_uri, number)
                    .await
                    .map_err(|e| format!("Failed to read revision history: {}", e))?
                    .is_none()
                {
                    return Err(format!(
                        "Script '{}' has no revision {}",
                        script_uri, number
                    ));
                }
                number
            }
            Err(_) => revisions::by_label(script_uri, spec)
                .await
                .map_err(|e| format!("Failed to resolve revision label: {}", e))?
                .ok_or_else(|| {
                    format!(
                        "Script '{}' has no revision labelled '{}'",
                        script_uri, spec
                    )
                })?,
        },
    };

    Ok(SourceView::Revision(resolved))
}

fn init_result_json(result: &crate::script_init::InitResult) -> Value {
    json!({
        "ran": true,
        "success": result.success,
        "durationMs": result.duration_ms,
        "error": result.error,
    })
}

// ============================================================================
// Authorized core operations (shared by the REST routes and MCP tools)
// ============================================================================

/// Outcome of an authorized script upsert.
pub enum UpsertAction {
    Inserted,
    Updated,
}

impl UpsertAction {
    fn as_str(&self) -> &'static str {
        match self {
            UpsertAction::Inserted => "inserted",
            UpsertAction::Updated => "updated",
        }
    }
}

/// Create or update a script: WriteScripts capability required, and existing
/// scripts can only be modified by an admin or an owner.
/// Broadcasts the update and re-initializes the script on success.
pub fn upsert_script_authorized(
    user: &UserContext,
    uri: &str,
    content: &str,
    via: Option<&str>,
) -> Result<(UpsertAction, Option<i32>), String> {
    if let Err(e) = user.require_capability(&Capability::WriteScripts) {
        return Err(format!("Error: {}", e));
    }
    if uri.is_empty() || content.is_empty() {
        return Err("Error: Script name and content cannot be empty".to_string());
    }

    let exists = repository::fetch_script(uri).is_some();
    if exists {
        let is_admin = user.has_capability(&Capability::DeleteScripts);
        if !is_admin && !user_owns_script(user, uri) {
            warn!(
                user_id = ?user.user_id,
                script_name = %uri,
                "Permission denied: user is neither admin nor owner"
            );
            return Err(format!(
                "Error: Permission denied. You must be an administrator or owner to modify script '{}'",
                uri
            ));
        }
    }

    if let Err(e) = repository::upsert_script_with_owner(uri, content, user.user_id.as_deref()) {
        return Err(format!("Error storing script: {}", e));
    }

    // Recorded before init is spawned, so the revision exists by the time the
    // init that follows looks for one to attach its outcome to.
    let revision =
        revisions::record_blocking(uri, revisions::Origin::Script, user.user_id.as_deref());

    spawn_script_init(uri.to_string());

    let action = if exists {
        UpsertAction::Updated
    } else {
        UpsertAction::Inserted
    };
    let mut details = vec![
        ("contentLength", json!(content.len())),
        ("previousExists", json!(exists)),
    ];
    if let Some(via) = via {
        details.push(("via", json!(via)));
    }
    broadcast_script_update(uri, action.as_str(), &details);

    Ok((action, revision))
}

/// Delete a script: DeleteScripts capability required. Returns false when the
/// capability is missing or the script does not exist. Broadcasts the removal
/// on success.
pub fn delete_script_authorized(user: &UserContext, uri: &str, via: Option<&str>) -> bool {
    if let Err(e) = user.require_capability(&Capability::DeleteScripts) {
        let auditor = auditor();
        let user_id = user.user_id.clone();
        tokio::task::spawn(async move {
            let _ = auditor
                .log_authz_failure(
                    user_id,
                    "script".to_string(),
                    "delete".to_string(),
                    "DeleteScripts".to_string(),
                )
                .await;
        });
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            error = %e,
            "deleteScript capability check failed"
        );
        return false;
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let uri_owned = uri.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::High,
                    user_id,
                )
                .with_resource("script".to_string())
                .with_action("delete".to_string())
                .with_detail("script_name", &uri_owned),
            )
            .await;
    });

    let deleted = repository::delete_script(uri);
    if deleted {
        let details: Vec<(&str, Value)> = via.map(|v| ("via", json!(v))).into_iter().collect();
        broadcast_script_update(uri, "removed", &details);
    }
    deleted
}

/// Fetch script content; ReadScripts capability required (None otherwise).
pub fn get_script_authorized(user: &UserContext, uri: &str) -> Option<String> {
    if !may_administer(user) || user.require_capability(&Capability::ReadScripts).is_err() {
        return None;
    }
    repository::fetch_script(uri)
}

/// List script metadata; an authenticated caller with the ReadScripts
/// capability (empty otherwise).
pub fn list_scripts_authorized(user: &UserContext) -> Vec<repository::ScriptMetadata> {
    if !may_administer(user) || user.require_capability(&Capability::ReadScripts).is_err() {
        return Vec::new();
    }
    repository::get_all_script_metadata().unwrap_or_default()
}

/// One log entry as JSON. `scriptUri` is what lets an all-scripts listing
/// attribute each line to the script that logged it, and `requestId`/`kind`/
/// `route` are what let a caller pull one invocation's lines out of it.
///
/// `seq` is the entry's position in the engine's write order: pass the last one
/// seen back as `after_seq` to read only what has been written since.
pub fn log_entry_json(entry: &repository::LogEntry) -> Value {
    let timestamp_ms = entry
        .timestamp
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64;
    json!({
        "scriptUri": entry.script_uri,
        "message": entry.message,
        "level": entry.level,
        "timestamp": timestamp_ms,
        "seq": entry.seq,
        "requestId": entry.context.request_id,
        "kind": entry.context.kind,
        "route": entry.context.route,
    })
}

/// Run a filtered log query, newest first; ViewLogs capability required.
///
/// Denial is an error, not an empty result: over HTTP a caller has to be able
/// to tell "you may not read these" from "there is nothing to read". The
/// sandbox convention of answering `[]` belongs to the JS globals, where the
/// script has no status code to receive.
pub fn query_logs_authorized(
    user: &UserContext,
    query: &repository::LogQuery,
) -> AppResult<Vec<Value>> {
    Ok(query_log_entries_authorized(user, query)?
        .iter()
        .map(log_entry_json)
        .collect())
}

/// As [`query_logs_authorized`], but answering with the entries themselves.
///
/// The live tail needs each entry's `seq` to advance its cursor, and reading it
/// back out of the JSON would be a worse kind of coupling. Both functions go
/// through this one so the capability check exists once.
pub fn query_log_entries_authorized(
    user: &UserContext,
    query: &repository::LogQuery,
) -> AppResult<Vec<repository::LogEntry>> {
    user.require_capability(&Capability::ViewLogs)?;
    repository::query_log_messages(query)
}

/// Number of entries per script that a prune keeps; mirrors the repository's
/// prune statement so callers can report what happened.
const PRUNE_KEEPS_PER_SCRIPT: u32 = 20;

/// Delete logs; DeleteLogs capability required.
///
/// With a `uri` this clears that script's logs outright. Without one it prunes
/// every script back to its newest entries.
pub fn delete_logs_authorized(user: &UserContext, uri: Option<&str>) -> AppResult<Value> {
    user.require_capability(&Capability::DeleteLogs)?;
    match uri {
        Some(uri) => {
            repository::clear_log_messages(uri)?;
            Ok(json!({
                "uri": uri,
                "cleared": true,
                "timestamp": iso_timestamp(),
            }))
        }
        None => {
            repository::prune_log_messages()?;
            Ok(json!({
                "pruned": true,
                "keptPerScript": PRUNE_KEEPS_PER_SCRIPT,
                "timestamp": iso_timestamp(),
            }))
        }
    }
}

/// Init status for one script; an authenticated caller with the ReadScripts
/// capability.
pub fn init_status_authorized(user: &UserContext, uri: &str) -> Option<Value> {
    if !may_administer(user) || user.require_capability(&Capability::ReadScripts).is_err() {
        return None;
    }
    let metadata = repository::get_script_metadata(uri).ok()?;
    Some(script_init_status_json(&metadata))
}

fn script_init_status_json(metadata: &repository::ScriptMetadata) -> Value {
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as f64)
    };
    json!({
        "scriptName": metadata.uri,
        "initialized": metadata.initialized,
        "initError": metadata.init_error,
        "lastInitTime": metadata.last_init_time.and_then(millis),
        "createdAt": millis(metadata.created_at),
        "updatedAt": millis(metadata.updated_at),
    })
}

/// Why an owner change was rejected.
pub enum OwnerChangeError {
    AccessDenied,
    LastOwner,
    Storage(String),
}

/// List a script's owners. Anyone may view owners, for transparency.
pub fn owners_authorized(uri: &str) -> Result<Vec<String>, String> {
    repository::get_script_owners(uri).map_err(|e| format!("{}", e))
}

/// Add an owner to a script; admin or current owner only.
pub fn add_owner_authorized(
    user: &UserContext,
    uri: &str,
    owner: &str,
) -> Result<(), OwnerChangeError> {
    if !is_admin_or_owner(user, uri) {
        return Err(OwnerChangeError::AccessDenied);
    }
    repository::add_script_owner(uri, owner)
        .map_err(|e| OwnerChangeError::Storage(format!("{}", e)))
}

/// Remove an owner from a script; admin or current owner only. Non-admins
/// cannot remove the last owner. Returns whether the owner existed.
pub fn remove_owner_authorized(
    user: &UserContext,
    uri: &str,
    owner: &str,
) -> Result<bool, OwnerChangeError> {
    if !is_admin_or_owner(user, uri) {
        return Err(OwnerChangeError::AccessDenied);
    }
    if !has_admin_capability(user) {
        match repository::count_script_owners(uri) {
            Ok(count) if count <= 1 => return Err(OwnerChangeError::LastOwner),
            Err(e) => return Err(OwnerChangeError::Storage(format!("{}", e))),
            _ => {}
        }
    }
    repository::remove_script_owner(uri, owner)
        .map_err(|e| OwnerChangeError::Storage(format!("{}", e)))
}

/// Why a secret operation was rejected.
#[derive(Debug)]
pub enum SecretAccessError {
    AccessDenied,
    Validation(String),
    Storage(String),
}

/// Cross-script secret management: admins and owners of the target script may
/// manage its secret keys. Secret values are write-only through this surface —
/// there is deliberately no read-value operation.
fn can_manage_secrets(user: &UserContext, script_uri: &str) -> bool {
    is_admin_or_owner(user, script_uri)
}

/// List secret keys (not values) stored for a script.
pub fn list_secrets_authorized(
    user: &UserContext,
    script_uri: &str,
) -> Result<Vec<String>, SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    Ok(repository::list_script_secrets(script_uri).unwrap_or_default())
}

/// Store a secret for a script.
pub fn set_secret_authorized(
    user: &UserContext,
    script_uri: &str,
    key: &str,
    value: &str,
) -> Result<(), SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    if key.trim().is_empty() {
        return Err(SecretAccessError::Validation(
            "Key cannot be empty".to_string(),
        ));
    }
    if value.len() > 1_000_000 {
        return Err(SecretAccessError::Validation(
            "Value too large (>1MB)".to_string(),
        ));
    }
    repository::set_script_secret_item(script_uri, key, value)
        .map_err(|e| SecretAccessError::Storage(format!("{}", e)))
}

/// Remove one secret from a script. Returns whether the key existed.
pub fn remove_secret_authorized(
    user: &UserContext,
    script_uri: &str,
    key: &str,
) -> Result<bool, SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    Ok(repository::remove_script_secret_item(script_uri, key))
}

/// Remove all secrets stored for a script.
pub fn clear_secrets_authorized(
    user: &UserContext,
    script_uri: &str,
) -> Result<(), SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    repository::clear_script_secrets(script_uri)
        .map_err(|e| SecretAccessError::Storage(format!("{}", e)))
}

// ----------------------------------------------------------------------------
// User administration
// ----------------------------------------------------------------------------

/// Why a user-administration operation was rejected.
#[derive(Debug)]
pub enum UserAdminError {
    AccessDenied,
    Validation(String),
    UserNotFound(String),
    LastAdministrator,
    Storage(String),
}

/// User administration is restricted to session-verified administrators.
///
/// Deliberately stricter than [`has_admin_capability`], which accepts any holder of
/// `DeleteScripts` — a capability development mode also grants to *anonymous*
/// callers. Requiring authentication as well means only a `UserContext::admin`
/// passes, and that is built solely from a session whose `is_admin` flag is
/// set (`lib.rs`), for both the HTTP and MCP entry points. So development mode
/// cannot hand an unauthenticated caller the user directory or the ability to
/// grant itself a role.
fn is_user_admin(user: &UserContext) -> bool {
    user.is_authenticated && user.has_capability(&Capability::DeleteScripts)
}

/// Record an authorization failure against the user-administration surface.
fn audit_user_admin_denied(user: &UserContext, action: &str) {
    let auditor = auditor();
    let user_id = user.user_id.clone();
    let action = action.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_authz_failure(
                user_id,
                "user".to_string(),
                action,
                "Administrator".to_string(),
            )
            .await;
    });
}

/// Record a completed role change. Role changes are privilege escalations or
/// revocations, so they are logged at high severity like script deletion.
fn audit_role_change(actor: &UserContext, target_user_id: &str, role: &str, action: &str) {
    let auditor = auditor();
    let actor_id = actor.user_id.clone();
    let (target, role, action) = (
        target_user_id.to_string(),
        role.to_string(),
        action.to_string(),
    );
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::High,
                    actor_id,
                )
                .with_resource("user".to_string())
                .with_action(action)
                .with_detail("target_user", &target)
                .with_detail("role", &role),
            )
            .await;
    });
}

/// Parse a role name accepted by the role endpoints.
fn parse_user_role(role: &str) -> Result<crate::user_repository::UserRole, UserAdminError> {
    use crate::user_repository::UserRole;
    match role {
        "Authenticated" => Ok(UserRole::Authenticated),
        "Editor" => Ok(UserRole::Editor),
        "Administrator" => Ok(UserRole::Administrator),
        other => Err(UserAdminError::Validation(format!(
            "Invalid role: {}. Must be Editor, Administrator, or Authenticated",
            other
        ))),
    }
}

/// Look up a user, distinguishing "no such user" from a storage failure so the
/// caller can answer 404 rather than 500.
fn lookup_user(user_id: &str) -> Result<crate::user_repository::User, UserAdminError> {
    crate::user_repository::get_user(user_id).map_err(|e| match e {
        // `db_get_user` reports a missing row as a validation error on `user_id`.
        crate::error::AppError::Validation { ref field, .. } if field == "user_id" => {
            UserAdminError::UserNotFound(user_id.to_string())
        }
        other => UserAdminError::Storage(format!("{}", other)),
    })
}

fn role_names(user: &crate::user_repository::User) -> Vec<String> {
    user.roles.iter().map(|r| format!("{:?}", r)).collect()
}

fn user_to_json(user: &crate::user_repository::User) -> Value {
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64
    };
    json!({
        "id": user.id,
        "email": user.email,
        "name": user.name,
        "roles": role_names(user),
        "providers": user.providers
            .iter()
            .map(|p| p.provider_name.clone())
            .collect::<Vec<_>>(),
        "createdAt": millis(user.created_at),
        "updatedAt": millis(user.updated_at),
    })
}

/// List every user in the directory; administrators only.
///
/// An unauthorized caller is denied rather than handed an empty array —
/// "denied" and "no users" must not look alike.
pub fn list_users_authorized(user: &UserContext) -> Result<Vec<Value>, UserAdminError> {
    if !is_user_admin(user) {
        audit_user_admin_denied(user, "list");
        return Err(UserAdminError::AccessDenied);
    }
    crate::user_repository::list_users()
        .map(|users| users.iter().map(user_to_json).collect())
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))
}

/// Grant a role to a user; administrators only. Returns the user's resulting
/// role set. Granting a role the user already holds is a no-op, not an error.
pub fn add_user_role_authorized(
    actor: &UserContext,
    user_id: &str,
    role: &str,
) -> Result<Vec<String>, UserAdminError> {
    if !is_user_admin(actor) {
        audit_user_admin_denied(actor, "add_role");
        return Err(UserAdminError::AccessDenied);
    }
    let parsed = parse_user_role(role)?;
    // Establish the user exists before mutating, so a typo'd id reads as 404.
    lookup_user(user_id)?;

    crate::user_repository::add_user_role(user_id, parsed)
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))?;
    audit_role_change(actor, user_id, role, "add_role");

    Ok(role_names(&lookup_user(user_id)?))
}

/// Revoke a role from a user; administrators only. Returns the user's
/// resulting role set.
///
/// Two roles cannot be revoked: `Authenticated` (every user has it by
/// definition) and the last remaining `Administrator` — locking the last
/// administrator out would leave the instance with no way to appoint another,
/// the same reasoning behind the last-owner guard on scripts.
pub fn remove_user_role_authorized(
    actor: &UserContext,
    user_id: &str,
    role: &str,
) -> Result<Vec<String>, UserAdminError> {
    use crate::user_repository::UserRole;

    if !is_user_admin(actor) {
        audit_user_admin_denied(actor, "remove_role");
        return Err(UserAdminError::AccessDenied);
    }
    let parsed = parse_user_role(role)?;
    if matches!(parsed, UserRole::Authenticated) {
        return Err(UserAdminError::Validation(
            "Cannot remove the Authenticated role".to_string(),
        ));
    }
    let target = lookup_user(user_id)?;

    if matches!(parsed, UserRole::Administrator)
        && target.has_role(&UserRole::Administrator)
        && count_administrators()? <= 1
    {
        return Err(UserAdminError::LastAdministrator);
    }

    crate::user_repository::remove_user_role(user_id, &parsed)
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))?;
    audit_role_change(actor, user_id, role, "remove_role");

    Ok(role_names(&lookup_user(user_id)?))
}

fn count_administrators() -> Result<usize, UserAdminError> {
    use crate::user_repository::UserRole;
    crate::user_repository::list_users()
        .map(|users| {
            users
                .iter()
                .filter(|u| u.has_role(&UserRole::Administrator))
                .count()
        })
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))
}

/// Whether the user may access assets of `script_uri` given the per-operation
/// capability: capability holders, script owners, and admins all qualify.
fn can_access_assets(user: &UserContext, script_uri: &str, capability: &Capability) -> bool {
    may_administer(user)
        && (user.has_capability(capability)
            || user.has_capability(&Capability::DeleteScripts)
            || user_owns_script(user, script_uri))
}

/// List asset metadata for a script (empty when access is denied).
pub fn list_assets_authorized(user: &UserContext, script_uri: &str) -> Vec<Value> {
    if !can_access_assets(user, script_uri, &Capability::ReadAssets) {
        return Vec::new();
    }
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64
    };
    repository::fetch_assets(script_uri)
        .values()
        .map(|asset| {
            json!({
                "uri": asset.uri,
                "name": asset.name,
                "size": asset.content.len(),
                "mimetype": asset.mimetype,
                "createdAt": millis(asset.created_at),
                "updatedAt": millis(asset.updated_at),
            })
        })
        .collect()
}

pub enum AssetFetchError {
    AccessDenied,
    NotFound,
}

/// An inclusive, 1-based range of lines, as `lines=120-180` spells it. An
/// absent `end` means "to the end of the file".
#[derive(Clone, Copy)]
pub struct LineRange {
    pub start: usize,
    pub end: Option<usize>,
}

impl LineRange {
    /// Parse the `lines=` parameter: `120-180`, `120-` (to the end of the
    /// file), or `120` (that line alone).
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (start_raw, end_raw) = match raw.split_once('-') {
            Some((start, end)) => (start.trim(), end.trim()),
            None => (raw, raw),
        };
        let number = |value: &str| -> Result<usize, String> {
            value
                .parse::<usize>()
                .ok()
                .filter(|line| *line > 0)
                .ok_or_else(|| {
                    format!(
                        "Invalid lines range '{}': expected 'start-end', 'start-', or 'start', \
                         counting from 1",
                        raw
                    )
                })
        };
        let start = number(start_raw)?;
        let end = if end_raw.is_empty() {
            None
        } else {
            Some(number(end_raw)?)
        };
        if let Some(end) = end
            && end < start
        {
            return Err(format!(
                "Invalid lines range '{}': end is before start",
                raw
            ));
        }
        Ok(LineRange { start, end })
    }
}

/// How a read of one asset is scoped. The two filters compose: a `grep` inside
/// a `lines` range searches only that range.
#[derive(Default)]
pub struct AssetReadOptions {
    pub lines: Option<LineRange>,
    pub grep: Option<String>,
}

impl AssetReadOptions {
    /// Whether the caller asked for a view of part of the file rather than the
    /// whole of it.
    fn is_scoped(&self) -> bool {
        self.lines.is_some() || self.grep.is_some()
    }
}

/// One line a `grep=` read matched.
pub struct GrepMatch {
    pub line: usize,
    pub text: String,
    /// Whether `text` is only the beginning of a longer line.
    pub truncated: bool,
}

impl GrepMatch {
    fn to_json(&self) -> Value {
        json!({
            "line": self.line,
            "text": self.text,
            "truncated": self.truncated,
        })
    }
}

/// What a read returned: the whole file, a range of it, or the places a
/// pattern matched.
pub enum AssetView {
    Full {
        content_base64: String,
    },
    Range {
        content: String,
        start_line: usize,
        end_line: usize,
    },
    Matches {
        matches: Vec<GrepMatch>,
        truncated: bool,
    },
}

/// One asset as read, whole or scoped.
pub struct AssetRead {
    pub view: AssetView,
    /// Digest of the whole stored asset, whichever part of it was returned —
    /// this is what a following patch sends as `base_sha256`.
    pub sha256: String,
    /// Size of the whole stored asset, in bytes.
    pub bytes: usize,
    /// Line count of the whole asset; absent from a whole-file read, which
    /// does not require the content to be text at all.
    pub total_lines: Option<usize>,
}

impl AssetRead {
    /// The response fields for this read, to be merged with the script and
    /// asset the caller named.
    pub fn to_json(&self) -> Value {
        let mut body = match &self.view {
            AssetView::Full { content_base64 } => json!({ "content": content_base64 }),
            AssetView::Range {
                content,
                start_line,
                end_line,
            } => json!({
                "encoding": "utf8",
                "content": content,
                "start_line": start_line,
                "end_line": end_line,
            }),
            AssetView::Matches { matches, truncated } => json!({
                "encoding": "utf8",
                "matches": matches.iter().map(GrepMatch::to_json).collect::<Vec<Value>>(),
                "match_count": matches.len(),
                "truncated": truncated,
            }),
        };
        if let Some(object) = body.as_object_mut() {
            object.insert("sha256".to_string(), json!(self.sha256));
            object.insert("bytes".to_string(), json!(self.bytes));
            if let Some(total_lines) = self.total_lines {
                object.insert("total_lines".to_string(), json!(total_lines));
            }
        }
        body
    }
}

pub enum AssetReadError {
    AccessDenied,
    NotFound,
    /// The read asked for a text view of something that is not text, or asked
    /// for it in a way that does not parse.
    Validation(String),
}

/// Compile a `grep=` pattern, with the bounds a request-time regex needs.
fn compile_grep(pattern: &str) -> Result<regex::Regex, AssetReadError> {
    if pattern.is_empty() {
        return Err(AssetReadError::Validation(
            "Empty grep pattern: there is nothing to search for".to_string(),
        ));
    }
    if pattern.chars().count() > MAX_GREP_PATTERN_CHARS {
        return Err(AssetReadError::Validation(format!(
            "grep pattern too long (max {} characters)",
            MAX_GREP_PATTERN_CHARS
        )));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| AssetReadError::Validation(format!("Invalid grep pattern: {}", e)))
}

/// Cut a line down to what a match listing echoes back, on a character
/// boundary. Reports whether anything was cut.
fn truncate_chars(line: &str, max: usize) -> (String, bool) {
    match line.char_indices().nth(max) {
        Some((index, _)) => (line[..index].to_string(), true),
        None => (line.to_string(), false),
    }
}

/// Read one asset, whole or scoped to a line range and/or a pattern.
///
/// The unscoped read is the original contract — the whole file, base64 — and
/// stays that way for callers already parsing it. `lines` and `grep` are for a
/// caller that wants to look at part of a file, or to find out where to look,
/// without pulling the whole thing across; both require the asset to be UTF-8
/// text, since neither means anything otherwise.
///
/// Every read reports the digest of the whole asset, not of the part
/// returned, because that digest is what a following patch has to send to
/// prove it edited the version it read.
pub fn read_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    options: &AssetReadOptions,
) -> Result<AssetRead, AssetReadError> {
    if !can_access_assets(user, script_uri, &Capability::ReadAssets) {
        return Err(AssetReadError::AccessDenied);
    }
    let Some(asset) = repository::fetch_asset(script_uri, asset_uri) else {
        return Err(AssetReadError::NotFound);
    };

    let sha256 = sha256_hex(&asset.content);
    let bytes = asset.content.len();

    if !options.is_scoped() {
        return Ok(AssetRead {
            view: AssetView::Full {
                content_base64: base64::engine::general_purpose::STANDARD.encode(&asset.content),
            },
            sha256,
            bytes,
            total_lines: None,
        });
    }

    let text = std::str::from_utf8(&asset.content).map_err(|_| {
        AssetReadError::Validation(format!(
            "Asset '{}' is not UTF-8 text, so it has no lines to read: fetch it without \
             'lines' or 'grep' to get its bytes",
            asset_uri
        ))
    })?;

    let lines: Vec<&str> = text.lines().collect();
    let range = options.lines.unwrap_or(LineRange {
        start: 1,
        end: None,
    });
    // A `start` past the end of the file asks for a part that is not there —
    // most often a range computed against a version that has since shrunk —
    // and an empty 200 would leave the caller to work that out for itself. An
    // `end` past the end is a different request: "through line 1000" of a
    // 400-line file plainly means the rest of it, so it clamps.
    if options.lines.is_some() && range.start > lines.len() {
        return Err(AssetReadError::Validation(format!(
            "Asset '{}' has {} lines, so there is no line {} to read",
            asset_uri,
            lines.len(),
            range.start
        )));
    }
    let start = range.start;
    let end = range.end.unwrap_or(lines.len()).min(lines.len());
    // Only an empty file reaches this, and only through the default range: an
    // explicit one was bounded above.
    let selected: &[&str] = if start > end {
        &[]
    } else {
        &lines[start - 1..end]
    };

    let view = match &options.grep {
        None => AssetView::Range {
            content: selected.join("\n"),
            start_line: start,
            end_line: end,
        },
        Some(pattern) => {
            let regex = compile_grep(pattern)?;
            let mut matches = Vec::new();
            let mut truncated = false;
            for (offset, line) in selected.iter().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                if matches.len() >= MAX_GREP_MATCHES {
                    truncated = true;
                    break;
                }
                let (text, line_truncated) = truncate_chars(line, MAX_GREP_LINE_CHARS);
                matches.push(GrepMatch {
                    line: start + offset,
                    text,
                    truncated: line_truncated,
                });
            }
            AssetView::Matches { matches, truncated }
        }
    };

    Ok(AssetRead {
        view,
        sha256,
        bytes,
        total_lines: Some(lines.len()),
    })
}

pub enum AssetWriteError {
    AccessDenied,
    Validation(String),
    Storage(String),
}

/// The shape checks an asset path has to pass before it can be stored.
fn validate_asset_uri(asset_uri: &str) -> Result<(), AssetWriteError> {
    if asset_uri.is_empty() || asset_uri.len() > 255 {
        return Err(AssetWriteError::Validation(format!(
            "Invalid asset URI '{}': must be 1-255 characters",
            asset_uri
        )));
    }
    if asset_uri.contains("..") || asset_uri.contains('\\') {
        return Err(AssetWriteError::Validation(format!(
            "Invalid asset URI '{}': path traversal not allowed",
            asset_uri
        )));
    }
    Ok(())
}

/// Lowercase hex SHA-256 of an asset's decoded bytes — the digest the batch
/// write echoes back so a caller can verify what was stored without reading it
/// again.
fn sha256_hex(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// MIME type inferred from an asset's extension, for batch callers that push a
/// directory of source files and have nothing to say about each one's type.
///
/// Deliberately narrow: it covers what scripts are actually made of, and
/// anything else falls back to a type that will be served as a download rather
/// than guessed at.
fn mimetype_for(asset_uri: &str) -> &'static str {
    let extension = asset_uri
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "js" | "mjs" | "cjs" | "jsx" => "text/javascript",
        "ts" | "tsx" | "mts" | "cts" => "text/typescript",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Create or update an asset from base64 content.
pub fn upsert_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    mimetype: &str,
    content_b64: &str,
) -> Result<Option<i32>, AssetWriteError> {
    let content = base64::engine::general_purpose::STANDARD
        .decode(content_b64)
        .map_err(|e| {
            AssetWriteError::Validation(format!("Error decoding base64 content: {}", e))
        })?;

    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied);
    }

    validate_asset_uri(asset_uri)?;
    if content.len() > MAX_ASSET_BYTES {
        return Err(AssetWriteError::Validation(
            "Asset too large (max 10MB)".to_string(),
        ));
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let asset_uri_owned = asset_uri.to_string();
    let content_len = content.len();
    let mimetype_owned = mimetype.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Medium,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("upsert_for_uri".to_string())
                .with_detail("uri", &asset_uri_owned)
                .with_detail("script_uri", &script_uri_owned)
                .with_detail("content_size", content_len.to_string())
                .with_detail("mimetype", &mimetype_owned),
            )
            .await;
    });

    let now = std::time::SystemTime::now();
    let asset = repository::Asset {
        uri: asset_uri.to_string(),
        name: Some(asset_uri.to_string()),
        mimetype: mimetype.to_string(),
        content,
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    };
    repository::upsert_asset(asset)
        .map_err(|e| AssetWriteError::Storage(format!("Error upserting asset: {}", e)))?;

    Ok(revisions::record_blocking(
        script_uri,
        revisions::Origin::Post,
        user.user_id.as_deref(),
    ))
}

/// One file of a batch asset write.
pub struct AssetWrite {
    /// Path of the asset within the script, e.g. `/lib/util.ts`.
    pub name: String,
    /// MIME type; inferred from the extension when the caller omits it.
    pub mimetype: Option<String>,
    /// Base64-encoded content, the same transfer encoding the single-asset
    /// write and the MCP tools use.
    pub content_base64: String,
    /// Digest the caller believes the decoded bytes have, lowercase hex. When
    /// present it is checked before anything is written.
    pub expected_sha256: Option<String>,
}

/// What a batch write did with one file.
pub struct AssetWriteOutcome {
    pub name: String,
    pub sha256: String,
    pub bytes: usize,
    /// `created`, `updated`, or `unchanged`.
    pub status: &'static str,
}

impl AssetWriteOutcome {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "sha256": self.sha256,
            "bytes": self.bytes,
            "status": self.status,
        })
    }
}

/// What a batch write did overall.
pub struct BatchWriteOutcome {
    pub results: Vec<AssetWriteOutcome>,
    /// The revision this write produced, or `None` when it changed nothing.
    /// The number a caller reverts to when the change turns out to be wrong.
    pub revision: Option<i32>,
    /// How many files reached the database. Zero when every file already held
    /// the content it was sent with, which is also when re-initializing the
    /// script afterwards would be pure cost.
    pub written: usize,
}

/// Write several of a script's assets as one unit.
///
/// Every file is decoded and checked before any of them is stored, so a batch
/// with one bad entry writes nothing: the caller's tree never lands in the
/// engine half-applied. Files whose stored content already matches are
/// reported as `unchanged` and skipped, because rewriting one would invalidate
/// the script's prepared program for no change.
///
/// The authorization is the single-write rule applied once, not per file:
/// WriteAssets capability, ownership of the script, or admin.
pub fn upsert_assets_authorized(
    user: &UserContext,
    script_uri: &str,
    files: &[AssetWrite],
) -> Result<BatchWriteOutcome, AssetWriteError> {
    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied);
    }
    if files.is_empty() {
        return Err(AssetWriteError::Validation(
            "No files to write: 'files' must contain at least one entry".to_string(),
        ));
    }
    if files.len() > MAX_BATCH_FILES {
        return Err(AssetWriteError::Validation(format!(
            "Too many files in one batch: {} (max {})",
            files.len(),
            MAX_BATCH_FILES
        )));
    }

    let mut prepared: Vec<(String, String, Vec<u8>, String)> = Vec::with_capacity(files.len());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut total_bytes: usize = 0;

    for file in files {
        validate_asset_uri(&file.name)?;
        if !seen.insert(file.name.as_str()) {
            return Err(AssetWriteError::Validation(format!(
                "Duplicate file '{}' in batch",
                file.name
            )));
        }

        let content = base64::engine::general_purpose::STANDARD
            .decode(&file.content_base64)
            .map_err(|e| {
                AssetWriteError::Validation(format!(
                    "Error decoding base64 content for '{}': {}",
                    file.name, e
                ))
            })?;

        if content.len() > MAX_ASSET_BYTES {
            return Err(AssetWriteError::Validation(format!(
                "Asset '{}' too large (max 10MB)",
                file.name
            )));
        }
        total_bytes = total_bytes.saturating_add(content.len());
        if total_bytes > MAX_BATCH_BYTES {
            return Err(AssetWriteError::Validation(format!(
                "Batch too large: over {} bytes of content in one request",
                MAX_BATCH_BYTES
            )));
        }

        let digest = sha256_hex(&content);
        if let Some(expected) = &file.expected_sha256
            && !expected.eq_ignore_ascii_case(&digest)
        {
            return Err(AssetWriteError::Validation(format!(
                "Content of '{}' does not match the sha256 supplied for it (expected {}, got {})",
                file.name, expected, digest
            )));
        }

        let mimetype = match &file.mimetype {
            Some(mimetype) if !mimetype.trim().is_empty() => mimetype.clone(),
            _ => mimetype_for(&file.name).to_string(),
        };

        prepared.push((file.name.clone(), mimetype, content, digest));
    }

    // One read of what is stored, rather than one per file, to classify each
    // write and to drop the files that would rewrite identical bytes.
    let existing = repository::fetch_assets(script_uri);

    let now = std::time::SystemTime::now();
    let mut results = Vec::with_capacity(prepared.len());
    let mut to_write = Vec::new();

    for (name, mimetype, content, digest) in prepared {
        let current = existing.get(&name);
        let status = match current {
            Some(asset) if asset.content == content && asset.mimetype == mimetype => "unchanged",
            Some(_) => "updated",
            None => "created",
        };
        results.push(AssetWriteOutcome {
            name: name.clone(),
            sha256: digest,
            bytes: content.len(),
            status,
        });
        if status != "unchanged" {
            to_write.push(repository::Asset {
                uri: name.clone(),
                name: Some(name),
                mimetype,
                content,
                created_at: current.map(|asset| asset.created_at).unwrap_or(now),
                updated_at: now,
                script_uri: script_uri.to_string(),
            });
        }
    }

    let written = to_write.len();
    if written > 0 {
        repository::upsert_assets(script_uri, to_write)
            .map_err(|e| AssetWriteError::Storage(format!("Error upserting assets: {}", e)))?;
    }

    // One audit event for the batch, not one per file: the batch is the act.
    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let names = results
        .iter()
        .map(|result| result.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let file_count = results.len();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Medium,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("batch_upsert_for_uri".to_string())
                .with_detail("script_uri", &script_uri_owned)
                .with_detail("file_count", file_count.to_string())
                .with_detail("written", written.to_string())
                .with_detail("content_size", total_bytes.to_string())
                .with_detail("uris", &names),
            )
            .await;
    });

    // A batch that wrote nothing left the script exactly as the previous
    // revision already describes, so there is no new state to record.
    let revision = (written > 0)
        .then(|| {
            revisions::record_blocking(
                script_uri,
                revisions::Origin::Batch,
                user.user_id.as_deref(),
            )
        })
        .flatten();

    Ok(BatchWriteOutcome {
        results,
        revision,
        written,
    })
}

/// One string replacement of a patch.
pub struct AssetEdit {
    /// Text to find. It must be present, and unique unless `replace_all`.
    pub old_string: String,
    /// Text to put in its place.
    pub new_string: String,
    /// Replace every occurrence rather than requiring exactly one.
    pub replace_all: bool,
}

/// What a patch did to an asset.
pub struct AssetPatchOutcome {
    /// Digest of the content as it now stands, which the next patch can send
    /// back as `base_sha256`.
    pub sha256: String,
    /// The revision this patch produced, or `None` when the edits cancelled
    /// each other out and nothing was stored.
    pub revision: Option<i32>,
    pub bytes: usize,
    /// How many occurrences the edits replaced in total.
    pub replacements: usize,
    /// `updated`, or `unchanged` when the edits cancelled each other out.
    pub status: &'static str,
}

impl AssetPatchOutcome {
    fn to_json(&self) -> Value {
        json!({
            "sha256": self.sha256,
            "bytes": self.bytes,
            "replacements": self.replacements,
            "status": self.status,
            "revision": self.revision,
        })
    }
}

pub enum AssetPatchError {
    AccessDenied,
    NotFound,
    Validation(String),
    /// The stored asset is not the one the caller edited: `base_sha256` names
    /// content it no longer has.
    Conflict {
        expected: String,
        actual: String,
    },
    Storage(String),
}

/// Edit an asset in place, by replacing strings within it.
///
/// This exists so a caller that wants to change three lines of a file does not
/// have to send the file back. That makes it the one asset write whose request
/// does not carry the content being stored — so it has to be careful about
/// two things the full writes get for free.
///
/// The first is *what* is being edited: `base_sha256`, when given, is checked
/// against the stored bytes before anything is applied, so a patch computed
/// against a version someone has since replaced is refused rather than merged
/// blind.
///
/// The second is *where*: an `old_string` that appears more than once is
/// refused unless the caller said `replace_all`, because an edit meant for one
/// of three identical lines cannot be aimed by content alone. Every edit is
/// checked and applied in memory before the asset is written, so a patch whose
/// third edit does not match leaves the stored file exactly as it was.
pub fn patch_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    edits: &[AssetEdit],
    base_sha256: Option<&str>,
) -> Result<AssetPatchOutcome, AssetPatchError> {
    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetPatchError::AccessDenied);
    }
    if edits.is_empty() {
        return Err(AssetPatchError::Validation(
            "No edits to apply: 'edits' must contain at least one entry".to_string(),
        ));
    }
    if edits.len() > MAX_ASSET_EDITS {
        return Err(AssetPatchError::Validation(format!(
            "Too many edits in one patch: {} (max {})",
            edits.len(),
            MAX_ASSET_EDITS
        )));
    }

    let Some(asset) = repository::fetch_asset(script_uri, asset_uri) else {
        return Err(AssetPatchError::NotFound);
    };

    let original_digest = sha256_hex(&asset.content);
    if let Some(expected) = base_sha256
        && !expected.eq_ignore_ascii_case(&original_digest)
    {
        return Err(AssetPatchError::Conflict {
            expected: expected.to_string(),
            actual: original_digest,
        });
    }

    let mut text = String::from_utf8(asset.content.clone()).map_err(|_| {
        AssetPatchError::Validation(format!(
            "Asset '{}' is not UTF-8 text, so it cannot be edited as strings: write it whole \
             with POST /engine/assets or /engine/assets/batch",
            asset_uri
        ))
    })?;

    let mut replacements = 0usize;
    for (index, edit) in edits.iter().enumerate() {
        if edit.old_string.is_empty() {
            return Err(AssetPatchError::Validation(format!(
                "edits[{}]: old_string must not be empty",
                index
            )));
        }
        if edit.old_string == edit.new_string {
            return Err(AssetPatchError::Validation(format!(
                "edits[{}]: old_string and new_string are identical, so the edit would do nothing",
                index
            )));
        }

        let occurrences = text.matches(edit.old_string.as_str()).count();
        match (occurrences, edit.replace_all) {
            (0, _) => {
                return Err(AssetPatchError::Validation(format!(
                    "edits[{}]: old_string was not found in '{}'{}",
                    index,
                    asset_uri,
                    if index > 0 {
                        " as the earlier edits left it"
                    } else {
                        ""
                    }
                )));
            }
            (count, false) if count > 1 => {
                return Err(AssetPatchError::Validation(format!(
                    "edits[{}]: old_string appears {} times in '{}'; include enough surrounding \
                     text to make it unique, or pass replace_all",
                    index, count, asset_uri
                )));
            }
            (count, true) => {
                text = text.replace(edit.old_string.as_str(), &edit.new_string);
                replacements += count;
            }
            (_, false) => {
                text = text.replacen(edit.old_string.as_str(), &edit.new_string, 1);
                replacements += 1;
            }
        }
    }

    if text.len() > MAX_ASSET_BYTES {
        return Err(AssetPatchError::Validation(
            "Asset too large after the edits (max 10MB)".to_string(),
        ));
    }

    let content = text.into_bytes();
    let unchanged = content == asset.content;
    let digest = if unchanged {
        original_digest
    } else {
        sha256_hex(&content)
    };
    let bytes = content.len();

    if !unchanged {
        let now = std::time::SystemTime::now();
        // Through the batch write, so a patch reaches storage the same way a
        // deploy does: one transaction, one notification to the cluster.
        repository::upsert_assets(
            script_uri,
            vec![repository::Asset {
                uri: asset.uri.clone(),
                name: asset.name.clone(),
                mimetype: asset.mimetype.clone(),
                content,
                created_at: asset.created_at,
                updated_at: now,
                script_uri: script_uri.to_string(),
            }],
        )
        .map_err(|e| AssetPatchError::Storage(format!("Error patching asset: {}", e)))?;
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let asset_uri_owned = asset_uri.to_string();
    let edit_count = edits.len();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Medium,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("patch_for_uri".to_string())
                .with_detail("uri", &asset_uri_owned)
                .with_detail("script_uri", &script_uri_owned)
                .with_detail("edits", edit_count.to_string())
                .with_detail("replacements", replacements.to_string())
                .with_detail("content_size", bytes.to_string()),
            )
            .await;
    });

    let revision = (!unchanged)
        .then(|| {
            revisions::record_blocking(
                script_uri,
                revisions::Origin::Patch,
                user.user_id.as_deref(),
            )
        })
        .flatten();

    Ok(AssetPatchOutcome {
        sha256: digest,
        revision,
        bytes,
        replacements,
        status: if unchanged { "unchanged" } else { "updated" },
    })
}

/// Delete an asset.
pub fn delete_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
) -> Result<(bool, Option<i32>), AssetFetchError> {
    if !can_access_assets(user, script_uri, &Capability::DeleteAssets) {
        let auditor = auditor();
        let user_id = user.user_id.clone();
        tokio::task::spawn(async move {
            let _ = auditor
                .log_authz_failure(
                    user_id,
                    "asset".to_string(),
                    "delete_for_uri".to_string(),
                    "DeleteAssets".to_string(),
                )
                .await;
        });
        return Err(AssetFetchError::AccessDenied);
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let asset_uri_owned = asset_uri.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::High,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("delete_for_uri".to_string())
                .with_detail("uri", &asset_uri_owned)
                .with_detail("script_uri", &script_uri_owned),
            )
            .await;
    });

    let deleted = repository::delete_asset(script_uri, asset_uri);
    // Removing a file is a change like any other, and the one most worth being
    // able to undo: nothing else in the engine still holds the content.
    let revision = deleted
        .then(|| {
            revisions::record_blocking(
                script_uri,
                revisions::Origin::Delete,
                user.user_id.as_deref(),
            )
        })
        .flatten();

    Ok((deleted, revision))
}

// ============================================================================
// OpenAPI spec generation
// ============================================================================

/// Every registration in the engine as an introspection entry: script HTTP
/// routes, then SSE streams as `STREAM` rows, then asset routes as `ASSET`
/// rows. ReadScripts capability required (empty otherwise).
///
/// Backs `GET /engine/routes`. Host bindings are not applied here — this is the
/// whole engine's view; callers that care filter by `script_uri` (see
/// [`crate::route_index::script_serves_host`]).
pub fn routes_introspection_authorized(user: &UserContext) -> AppResult<Vec<Value>> {
    if !may_administer(user) {
        return Err(crate::error::AppError::AuthorizationFailed {
            message: "Engine route introspection is not open to anonymous callers".to_string(),
        });
    }
    user.require_capability(&Capability::ReadScripts)?;

    let metadata_list = repository::get_all_script_metadata()?;

    let mut all_routes = Vec::new();
    for metadata in metadata_list {
        if metadata.initialized && !metadata.registrations.is_empty() {
            for ((path, method), route_meta) in metadata.registrations {
                all_routes.push(json!({
                    "path": path,
                    "method": method,
                    "handler": route_meta.handler_name,
                    "script_uri": metadata.uri,
                    "summary": route_meta.summary,
                    "description": route_meta.description,
                    "tags": route_meta.tags,
                }));
            }
        }
    }

    for (path, script_uri, metadata) in
        crate::stream_registry::GLOBAL_STREAM_REGISTRY.get_all_registrations()
    {
        let handler = crate::stream_registry::GLOBAL_STREAM_REGISTRY
            .get_stream_info(&path)
            .and_then(|(_, customization_function)| customization_function);
        let tags = if metadata.tags.is_empty() {
            vec!["Streams".to_string()]
        } else {
            metadata.tags
        };
        all_routes.push(json!({
            "path": path,
            "method": "STREAM",
            "handler": handler,
            "script_uri": script_uri,
            "summary": metadata.summary,
            "description": metadata.description,
            "tags": tags,
        }));
    }

    for (path, registration) in crate::asset_registry::get_global_registry().get_all_registrations()
    {
        let tags = if registration.metadata.tags.is_empty() {
            vec!["Assets".to_string()]
        } else {
            registration.metadata.tags.clone()
        };
        all_routes.push(json!({
            "path": path,
            "method": "ASSET",
            "handler": registration.asset_name,
            "script_uri": registration.script_uri,
            "summary": registration.metadata.summary,
            "description": registration.metadata.description,
            "tags": tags,
        }));
    }

    Ok(all_routes)
}

/// Keep only entries whose owning script publishes on `host`.
///
/// Registrations are published per host, so an unfiltered listing shows routes
/// that are not live on the host the caller is looking at. Each distinct script
/// is checked once.
async fn filter_routes_by_host(routes: Vec<Value>, host: &str) -> Vec<Value> {
    let mut verdicts: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut filtered = Vec::with_capacity(routes.len());
    for route in routes {
        let script_uri = route
            .get("script_uri")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let serves = match verdicts.get(&script_uri) {
            Some(serves) => *serves,
            None => {
                let serves = crate::route_index::script_serves_host(&script_uri, host).await;
                verdicts.insert(script_uri, serves);
                serves
            }
        };
        if serves {
            filtered.push(route);
        }
    }
    filtered
}

/// Generate the full OpenAPI spec: the Rust (utoipa) spec merged with
/// script-registered routes, asset routes, and SSE stream routes. Returns
/// the same `{"error": ...}` JSON strings as the former JS implementation
/// when a step fails, so callers can pass the result through unchanged.
pub fn generate_merged_openapi_spec() -> String {
    let rust_spec_str = crate::get_rust_openapi_spec();
    let mut rust_spec: Value = match serde_json::from_str(&rust_spec_str) {
        Ok(spec) => spec,
        Err(e) => {
            return json!({ "error": format!("Failed to parse Rust OpenAPI spec: {}", e) })
                .to_string();
        }
    };

    let metadata_list = match repository::get_all_script_metadata() {
        Ok(list) => list,
        Err(e) => {
            return json!({ "error": format!("Failed to fetch JavaScript routes: {}", e) })
                .to_string();
        }
    };

    let mut js_paths = serde_json::Map::new();

    // Script-registered HTTP routes
    for metadata in metadata_list {
        if metadata.initialized && !metadata.registrations.is_empty() {
            for ((path, method), route_meta) in metadata.registrations {
                let path_item = js_paths.entry(path.clone()).or_insert_with(|| json!({}));
                let Some(path_obj) = path_item.as_object_mut() else {
                    continue;
                };

                let mut operation = serde_json::Map::new();
                operation.insert(
                    "summary".to_string(),
                    json!(
                        route_meta
                            .summary
                            .unwrap_or_else(|| format!("{} {}", method, path))
                    ),
                );
                if let Some(desc) = route_meta.description {
                    operation.insert("description".to_string(), json!(desc));
                }
                if !route_meta.tags.is_empty() {
                    operation.insert("tags".to_string(), json!(route_meta.tags));
                } else {
                    operation.insert("tags".to_string(), json!(["API"]));
                }
                if let Some(params) = &route_meta.parameters {
                    operation.insert("parameters".to_string(), params.clone());
                }
                if let Some(body) = &route_meta.request_body {
                    operation.insert("requestBody".to_string(), body.clone());
                }
                operation.insert(
                    "responses".to_string(),
                    json!({ "200": { "description": "Success" } }),
                );
                operation.insert("x-handler".to_string(), json!(route_meta.handler_name));
                operation.insert("x-script-uri".to_string(), json!(metadata.uri));
                operation.insert("x-source".to_string(), json!("javascript"));

                path_obj.insert(method.to_lowercase(), json!(operation));
            }
        }
    }

    // Asset routes from the asset registry
    let asset_registrations = crate::asset_registry::get_global_registry().get_all_registrations();
    for (path, registration) in asset_registrations {
        let extension = path.rsplit('.').next().unwrap_or("");
        let mime_type = match extension {
            "css" => "text/css",
            "js" => "application/javascript",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "ico" => "image/x-icon",
            "html" => "text/html",
            "json" => "application/json",
            "xml" => "application/xml",
            "pdf" => "application/pdf",
            "woff" | "woff2" => "font/woff2",
            "ttf" => "font/ttf",
            _ => "application/octet-stream",
        };

        let mut asset_operation = serde_json::Map::new();
        let asset_summary = registration
            .metadata
            .summary
            .clone()
            .unwrap_or_else(|| format!("Static asset: {}", registration.asset_name));
        asset_operation.insert("summary".to_string(), json!(asset_summary));
        let asset_description = registration
            .metadata
            .description
            .clone()
            .unwrap_or_else(|| {
                format!(
                    "Serves static asset '{}' registered by script '{}'",
                    registration.asset_name, registration.script_uri
                )
            });
        asset_operation.insert("description".to_string(), json!(asset_description));
        let asset_tags = if registration.metadata.tags.is_empty() {
            vec!["Assets".to_string()]
        } else {
            registration.metadata.tags.clone()
        };
        asset_operation.insert("tags".to_string(), json!(asset_tags));
        asset_operation.insert(
            "responses".to_string(),
            json!({
                "200": {
                    "description": "Asset content",
                    "content": {
                        mime_type: {
                            "schema": { "type": "string", "format": "binary" }
                        }
                    }
                },
                "404": { "description": "Asset not found" }
            }),
        );
        asset_operation.insert("x-asset-name".to_string(), json!(registration.asset_name));
        asset_operation.insert("x-script-uri".to_string(), json!(registration.script_uri));
        asset_operation.insert("x-source".to_string(), json!("asset-registry"));

        let path_entry = js_paths.entry(path).or_insert_with(|| json!({}));
        if let Some(path_obj) = path_entry.as_object_mut() {
            path_obj.insert("get".to_string(), json!(asset_operation));
        }
    }

    // SSE stream routes from the stream registry
    for (path, script_uri, metadata) in
        crate::stream_registry::GLOBAL_STREAM_REGISTRY.get_all_registrations()
    {
        let stream_tags = if metadata.tags.is_empty() {
            vec!["Streams".to_string()]
        } else {
            metadata.tags
        };

        let mut stream_operation = serde_json::Map::new();
        let stream_summary = metadata
            .summary
            .unwrap_or_else(|| format!("SSE stream: {}", path));
        stream_operation.insert("summary".to_string(), json!(stream_summary));
        let stream_description = metadata.description.unwrap_or_else(|| {
            format!(
                "Server-Sent Events stream registered by script '{}'",
                script_uri
            )
        });
        stream_operation.insert("description".to_string(), json!(stream_description));
        stream_operation.insert("tags".to_string(), json!(stream_tags));
        stream_operation.insert(
            "responses".to_string(),
            json!({
                "200": {
                    "description": "SSE event stream",
                    "content": {
                        "text/event-stream": { "schema": { "type": "string" } }
                    }
                }
            }),
        );
        stream_operation.insert("x-script-uri".to_string(), json!(script_uri));
        stream_operation.insert("x-source".to_string(), json!("stream-registry"));

        let path_entry = js_paths.entry(path).or_insert_with(|| json!({}));
        if let Some(path_obj) = path_entry.as_object_mut() {
            path_obj.insert("get".to_string(), json!(stream_operation));
        }
    }

    // Merge collected paths into the Rust spec
    if let Some(rust_paths) = rust_spec["paths"].as_object_mut() {
        for (path, operations) in js_paths {
            if let Some(existing) = rust_paths.get_mut(&path) {
                if let (Some(existing_obj), Some(new_ops)) =
                    (existing.as_object_mut(), operations.as_object())
                {
                    for (method, operation) in new_ops {
                        existing_obj.insert(method.clone(), operation.clone());
                    }
                }
            } else {
                rust_paths.insert(path, operations);
            }
        }
    }

    match serde_json::to_string_pretty(&rust_spec) {
        Ok(json) => json,
        Err(e) => json!({ "error": format!("Failed to serialize merged OpenAPI spec: {}", e) })
            .to_string(),
    }
}

// ============================================================================
// REST routes (contracts identical to the core.js/cli.js handlers)
// ============================================================================

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn missing_param_response(name: &str) -> Response {
    json_response(
        StatusCode::BAD_REQUEST,
        json!({
            "error": format!("Missing required parameter: {}", name),
            "timestamp": iso_timestamp(),
        }),
    )
}

#[derive(Deserialize, Default)]
pub struct ScriptParams {
    uri: Option<String>,
    content: Option<String>,
}

/// Create or update a script.
#[utoipa::path(
    post,
    path = "/engine/upsert_script",
    tags = ["Scripts"],
    request_body(content_type = "application/x-www-form-urlencoded",
        description = "Form fields: uri (required), content (required)"),
    responses(
        (status = 200, description = "Script upserted successfully"),
        (status = 400, description = "Missing required parameter"),
        (status = 500, description = "Failed to upsert script"),
    )
)]
pub async fn upsert_script_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form: ScriptParams = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(uri) = form.uri.or(query.uri) else {
        return missing_param_response("uri");
    };
    let Some(content) = form.content.or(query.content) else {
        return missing_param_response("content");
    };

    let result = tokio::task::spawn_blocking(move || {
        upsert_script_authorized(&user, &uri, &content, None)
            .map(|(_, revision)| (uri, content.len(), revision))
    })
    .await;

    match result {
        Ok(Ok((uri, content_length, revision))) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "message": "Script upserted successfully",
                "uri": uri,
                "contentLength": content_length,
                "revision": revision,
                "timestamp": iso_timestamp(),
            }),
        ),
        Ok(Err(details)) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "error": "Failed to upsert script",
                "details": details,
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "error": "Failed to upsert script",
                "details": format!("join error: {}", e),
                "timestamp": iso_timestamp(),
            }),
        ),
    }
}

/// Delete a script.
#[utoipa::path(
    post,
    path = "/engine/delete_script",
    tags = ["Scripts"],
    request_body(content_type = "application/x-www-form-urlencoded",
        description = "Form fields: uri (required)"),
    responses(
        (status = 200, description = "Script deleted successfully"),
        (status = 400, description = "Missing required parameter"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn delete_script_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form: ScriptParams = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(uri) = form.uri.or(query.uri) else {
        return missing_param_response("uri");
    };

    let uri_for_task = uri.clone();
    let deleted =
        tokio::task::spawn_blocking(move || delete_script_authorized(&user, &uri_for_task, None))
            .await
            .unwrap_or(false);

    if deleted {
        json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "message": "Script deleted successfully",
                "uri": uri,
                "timestamp": iso_timestamp(),
            }),
        )
    } else {
        json_response(
            StatusCode::NOT_FOUND,
            json!({
                "error": "Script not found",
                "message": "No script with the specified URI was found",
                "uri": uri,
                "timestamp": iso_timestamp(),
            }),
        )
    }
}

/// Read a script's content.
#[utoipa::path(
    get,
    path = "/engine/read_script",
    tags = ["Scripts"],
    params(("uri" = String, Query, description = "Script URI")),
    responses(
        (status = 200, description = "Script content", content_type = "application/javascript"),
        (status = 400, description = "Missing required parameter"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn read_script_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };

    let uri_for_task = uri.clone();
    let content = tokio::task::spawn_blocking(move || get_script_authorized(&user, &uri_for_task))
        .await
        .unwrap_or(None);

    match content {
        Some(content) => (
            StatusCode::OK,
            [("content-type", "application/javascript")],
            content,
        )
            .into_response(),
        None => json_response(
            StatusCode::NOT_FOUND,
            json!({
                "error": "Script not found",
                "message": "No script with the specified URI was found",
                "uri": uri,
                "timestamp": iso_timestamp(),
            }),
        ),
    }
}

/// Why a test run was refused before it started.
pub enum TestRunRefusal {
    NotFound,
    AccessDenied,
}

/// Whether `user` may run `uri`'s tests.
///
/// A run executes the script's own code with the caller's capabilities, so the
/// bar is the one for changing the script: an administrator, or an owner who
/// may write scripts. Anything looser would let someone with read access run
/// arbitrary code as themselves.
pub fn authorize_test_run(user: &UserContext, uri: &str) -> Result<(), TestRunRefusal> {
    if repository::fetch_script(uri).is_none() {
        return Err(TestRunRefusal::NotFound);
    }
    if user.require_capability(&Capability::WriteScripts).is_err() {
        return Err(TestRunRefusal::AccessDenied);
    }
    let is_admin = user.has_capability(&Capability::DeleteScripts);
    if !is_admin && !user_owns_script(user, uri) {
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            "Permission denied: only an administrator or owner may run a script's tests"
        );
        return Err(TestRunRefusal::AccessDenied);
    }
    Ok(())
}

#[derive(Deserialize, Default)]
pub struct TestRunParams {
    uri: Option<String>,
    filter: Option<String>,
    rollback: Option<bool>,
    /// Which version to run: a revision number, `head`, `last-good`, or a
    /// label. Absent runs what is deployed.
    revision: Option<String>,
}

/// Run a script's test modules and report the verdicts.
#[utoipa::path(
    post,
    path = "/engine/run_tests",
    tags = ["Scripts"],
    params(
        ("uri" = String, Query, description = "URI of the script whose tests to run"),
        ("filter" = Option<String>, Query, description = "Run only cases whose name contains this text"),
        ("rollback" = Option<bool>, Query, description = "Roll back database writes the tests make (default true)"),
        ("revision" = Option<String>, Query, description = "Which version to run: a revision number, `head`, `last-good`, or a label. Omit for what is deployed."),
    ),
    responses(
        (status = 200, description = "Test report; `success` is false when any case failed"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Not an administrator or owner of the script"),
        (status = 404, description = "Script not found"),
        (status = 500, description = "The run could not produce verdicts"),
    )
)]
pub async fn run_tests_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<TestRunParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form: TestRunParams = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(uri) = form.uri.or(query.uri) else {
        return missing_param_response("uri");
    };
    let filter = form.filter.or(query.filter);
    // Isolation is the default: a test that writes should not leave rows behind
    // unless the caller says so.
    let rollback = form.rollback.or(query.rollback).unwrap_or(true);
    let revision = form.revision.or(query.revision);

    let user_for_auth = user.clone();
    let uri_for_auth = uri.clone();
    let authorized =
        tokio::task::spawn_blocking(move || authorize_test_run(&user_for_auth, &uri_for_auth))
            .await;

    match authorized {
        Ok(Ok(())) => {}
        Ok(Err(TestRunRefusal::NotFound)) => {
            return json_response(
                StatusCode::NOT_FOUND,
                json!({
                    "error": "Script not found",
                    "uri": uri,
                    "timestamp": iso_timestamp(),
                }),
            );
        }
        Ok(Err(TestRunRefusal::AccessDenied)) => {
            return error_response(
                StatusCode::FORBIDDEN,
                format!(
                    "Error: Permission denied. You must be an administrator or owner to run tests for script '{}'",
                    uri
                ),
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to authorize test run: join error: {}", e),
            );
        }
    }

    let view = match resolve_view(&uri, revision.as_deref()).await {
        Ok(view) => view,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let result = crate::script_test::TestRunner::with_configured_timeouts()
        .run(crate::script_test::TestRunRequest {
            script_uri: uri,
            user_context: user,
            filter,
            rollback,
            view,
        })
        .await;

    let status = if result.error().is_some() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        // A failing test is a report, not a failed request.
        StatusCode::OK
    };

    let mut report = result.to_json();
    if let Some(object) = report.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
        if result.is_empty() && result.error().is_none() {
            // Distinguish "nothing to run" from "everything passed": both
            // report zero failures, and only one of them is good news.
            object.insert(
                "message".to_string(),
                json!(
                    "No test modules found. Tests are assets named '*.test.ts' (or .js/.jsx/.tsx)."
                ),
            );
        }
    }

    json_response(status, report)
}

/// Why a check was refused before it started.
pub enum CheckRefusal {
    NotFound,
    AccessDenied,
}

/// Whether `user` may check `uri`.
///
/// A check executes the script's own code with the caller's capabilities — the
/// same thing a test run does — so it takes the same bar: an administrator, or
/// an owner who may write scripts. When the caller supplies candidate content
/// there is no deployed script to own, and writing that content is the only
/// thing the check is a preview of, so `WriteScripts` alone is the bar there.
pub fn authorize_check(
    user: &UserContext,
    uri: &str,
    has_candidate: bool,
) -> Result<(), CheckRefusal> {
    let deployed = repository::fetch_script(uri).is_some();
    if !deployed && !has_candidate {
        return Err(CheckRefusal::NotFound);
    }
    if user.require_capability(&Capability::WriteScripts).is_err() {
        return Err(CheckRefusal::AccessDenied);
    }
    if deployed {
        let is_admin = user.has_capability(&Capability::DeleteScripts);
        if !is_admin && !user_owns_script(user, uri) {
            warn!(
                user_id = ?user.user_id,
                script_name = %uri,
                "Permission denied: only an administrator or owner may check a script"
            );
            return Err(CheckRefusal::AccessDenied);
        }
    }
    Ok(())
}

#[derive(Deserialize, Default)]
pub struct CheckParams {
    uri: Option<String>,
    rollback: Option<bool>,
    timeout_ms: Option<u64>,
    /// Which version to check: a revision number, `head`, `last-good`, or a
    /// label. Absent checks what is deployed.
    revision: Option<String>,
}

/// A JSON check request body.
#[derive(Deserialize, Default)]
struct CheckBody {
    uri: Option<String>,
    content: Option<String>,
    rollback: Option<bool>,
    timeout_ms: Option<u64>,
    revision: Option<String>,
}

/// Check what a script would do if it were deployed.
#[utoipa::path(
    post,
    path = "/engine/check",
    tags = ["Scripts"],
    params(
        ("uri" = String, Query, description = "URI of the script to check"),
        ("rollback" = Option<bool>, Query, description = "Roll back database writes init() makes (default true)"),
        ("timeout_ms" = Option<u64>, Query, description = "Ceiling for the init() run. Defaults to several times the deploy budget so a slow init() is measured rather than interrupted; raise it for one slower still."),
        ("revision" = Option<String>, Query, description = "Which version to check: a revision number, `head`, `last-good`, or a label. Omit for what is deployed."),
    ),
    request_body(
        description = "Optional candidate source to check instead of what is deployed. Send it as \
                       `application/json` (`{uri, content, rollback}`) or as a raw body under any \
                       other content type.",
        content_type = "application/json",
    ),
    responses(
        (status = 200, description = "Check report; `ok` is false when any diagnostic is an error"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Not an administrator or owner of the script"),
        (status = 404, description = "Script not found and no candidate content supplied"),
    )
)]
pub async fn check_route(
    auth_user: Option<Extension<AuthUser>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CheckParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    // A script is source text, so the body cannot be sniffed for structure the
    // way a form can — `{}` is a valid program. The content type is the only
    // honest signal, and it lets the common case stay a plain
    // `--data-binary @script.ts`.
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));

    let parsed: CheckBody = if body.is_empty() {
        CheckBody::default()
    } else if is_json {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid JSON body: {}", e),
                );
            }
        }
    } else {
        match String::from_utf8(body.to_vec()) {
            Ok(content) => CheckBody {
                content: Some(content),
                ..CheckBody::default()
            },
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Request body is not valid UTF-8 source text".to_string(),
                );
            }
        }
    };

    let Some(uri) = parsed.uri.or(query.uri) else {
        return missing_param_response("uri");
    };
    // Isolation is the default, as it is for a test run: a check should not
    // leave rows behind.
    let rollback = parsed.rollback.or(query.rollback).unwrap_or(true);
    let timeout_ms = parsed.timeout_ms.or(query.timeout_ms);
    let content = parsed.content;
    let revision = parsed.revision.or(query.revision);

    let user_for_auth = user.clone();
    let uri_for_auth = uri.clone();
    let has_candidate = content.is_some();
    let authorized = tokio::task::spawn_blocking(move || {
        authorize_check(&user_for_auth, &uri_for_auth, has_candidate)
    })
    .await;

    match authorized {
        Ok(Ok(())) => {}
        Ok(Err(CheckRefusal::NotFound)) => {
            return json_response(
                StatusCode::NOT_FOUND,
                json!({
                    "error": "Script not found",
                    "uri": uri,
                    "message": "Pass candidate source in the request body to check a script that is not deployed yet",
                    "timestamp": iso_timestamp(),
                }),
            );
        }
        Ok(Err(CheckRefusal::AccessDenied)) => {
            return error_response(
                StatusCode::FORBIDDEN,
                format!(
                    "Error: Permission denied. You must be an administrator or owner to check script '{}'",
                    uri
                ),
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to authorize check: join error: {}", e),
            );
        }
    }

    let view = match resolve_view(&uri, revision.as_deref()).await {
        Ok(view) => view,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };

    let report = crate::script_check::ScriptChecker::with_configured_timeout()
        .run(crate::script_check::CheckRequest {
            script_uri: uri,
            content,
            rollback,
            timeout_ms,
            view,
        })
        .await;

    let mut body = report.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }

    // A diagnostic is a report, not a failed request — the same way a failing
    // test is. Callers read `ok`.
    json_response(StatusCode::OK, body)
}

#[derive(Deserialize, Default)]
pub struct EvalParams {
    uri: Option<String>,
    rollback: Option<bool>,
    timeout_ms: Option<u64>,
}

/// A JSON evaluation request body.
#[derive(Deserialize, Default)]
struct EvalBody {
    uri: Option<String>,
    source: Option<String>,
    rollback: Option<bool>,
    timeout_ms: Option<u64>,
}

/// Whether `user` may evaluate a snippet against `uri`.
///
/// The same bar as a test run, because it is the same act: caller-authored
/// JavaScript executed in the script's sandbox with the caller's own
/// capabilities. Anything looser would let someone with read access run
/// arbitrary code as themselves.
pub fn authorize_eval(user: &UserContext, uri: &str) -> Result<(), CheckRefusal> {
    if repository::fetch_script(uri).is_none() {
        return Err(CheckRefusal::NotFound);
    }
    if user.require_capability(&Capability::WriteScripts).is_err() {
        return Err(CheckRefusal::AccessDenied);
    }
    let is_admin = user.has_capability(&Capability::DeleteScripts);
    if !is_admin && !user_owns_script(user, uri) {
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            "Permission denied: only an administrator or owner may evaluate against a script"
        );
        return Err(CheckRefusal::AccessDenied);
    }
    Ok(())
}

/// Evaluate a snippet against a script's sandbox.
#[utoipa::path(
    post,
    path = "/engine/eval",
    tags = ["Scripts"],
    params(
        ("uri" = String, Query, description = "URI of the script whose sandbox to evaluate in"),
        ("rollback" = Option<bool>, Query, description = "Roll back the database writes the snippet makes (default true)"),
        ("timeout_ms" = Option<u64>, Query, description = "Budget for the evaluation, clamped to the engine's execution timeout"),
    ),
    request_body(
        description = "The snippet. Send it as `application/json` (`{uri, source, rollback, timeoutMs}`) \
                       or as a raw body under any other content type.",
        content_type = "application/json",
    ),
    responses(
        (status = 200, description = "Evaluation report; `ok` is false when the snippet threw"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Not an administrator or owner of the script"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn eval_route(
    auth_user: Option<Extension<AuthUser>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<EvalParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    // Same rule as `/engine/check`: a snippet is source text, so only the
    // content type can say whether the body is a request envelope or the code
    // itself. Raw is the common case — `--data 'someHelper(1)'`.
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));

    let parsed: EvalBody = if body.is_empty() {
        EvalBody::default()
    } else if is_json {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid JSON body: {}", e),
                );
            }
        }
    } else {
        match String::from_utf8(body.to_vec()) {
            Ok(source) => EvalBody {
                source: Some(source),
                ..EvalBody::default()
            },
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Request body is not valid UTF-8 source text".to_string(),
                );
            }
        }
    };

    let Some(uri) = parsed.uri.or(query.uri) else {
        return missing_param_response("uri");
    };
    let Some(source) = parsed.source.filter(|source| !source.trim().is_empty()) else {
        return missing_param_response("source");
    };
    let rollback = parsed.rollback.or(query.rollback).unwrap_or(true);
    let timeout_ms = parsed.timeout_ms.or(query.timeout_ms);

    let user_for_auth = user.clone();
    let uri_for_auth = uri.clone();
    let authorized =
        tokio::task::spawn_blocking(move || authorize_eval(&user_for_auth, &uri_for_auth)).await;

    match authorized {
        Ok(Ok(())) => {}
        Ok(Err(CheckRefusal::NotFound)) => {
            return json_response(
                StatusCode::NOT_FOUND,
                json!({
                    "error": "Script not found",
                    "uri": uri,
                    "timestamp": iso_timestamp(),
                }),
            );
        }
        Ok(Err(CheckRefusal::AccessDenied)) => {
            return error_response(
                StatusCode::FORBIDDEN,
                format!(
                    "Error: Permission denied. You must be an administrator or owner to evaluate against script '{}'",
                    uri
                ),
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to authorize evaluation: join error: {}", e),
            );
        }
    }

    let report = crate::script_eval::ScriptEvaluator::run(crate::script_eval::EvalRequest {
        script_uri: uri,
        source,
        user_context: user,
        timeout_ms,
        rollback,
    })
    .await;

    let mut body = report.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }

    // A snippet that threw is a report, not a failed request — the caller asked
    // what the code does, and "it throws" is the answer.
    json_response(StatusCode::OK, body)
}

#[derive(Deserialize, Default)]
pub struct RoutesParams {
    host: Option<String>,
}

/// List every registration in the engine: script routes, SSE streams
/// (`STREAM`) and asset routes (`ASSET`).
///
/// A flat list, without the transform a client would need to rebuild it from
/// `/engine/openapi.json`. Unfiltered by default, since the management host
/// need not be a host scripts publish on; pass `host` to see only what is live
/// on one host.
#[utoipa::path(
    get,
    path = "/engine/routes",
    tags = ["Scripts"],
    params(("host" = Option<String>, Query, description = "Only registrations published on this host; omit for every host")),
    responses(
        (status = 200, description = "Route, stream and asset registrations"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn routes_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(params): Query<RoutesParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    let result = tokio::task::spawn_blocking(move || routes_introspection_authorized(&user)).await;

    let routes = match result {
        Ok(Ok(routes)) => routes,
        Ok(Err(e)) => {
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return error_response(status, format!("Failed to list routes: {}", e));
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list routes: {}", e),
            );
        }
    };

    let routes = match params.host.as_deref() {
        Some(host) => {
            filter_routes_by_host(routes, &crate::hosts::canonical_host(Some(host))).await
        }
        None => routes,
    };

    json_response(
        StatusCode::OK,
        json!({
            "host": params.host,
            "routes": routes,
            "count": routes.len(),
            "timestamp": iso_timestamp(),
        }),
    )
}

#[derive(Deserialize, Default)]
pub struct LogParams {
    uri: Option<String>,
    level: Option<String>,
    /// Milliseconds since the Unix epoch, or an RFC 3339 timestamp.
    since: Option<String>,
    /// Only entries written after this `seq`, for reading forward from where a
    /// previous response ended.
    after_seq: Option<i64>,
    /// Only entries whose message contains this substring.
    contains: Option<String>,
    /// Only the entries one invocation emitted, by `x-request-id` or the id a
    /// non-HTTP invocation was given.
    request_id: Option<String>,
    /// Only entries from invocations of this kind, e.g. `scheduled`.
    kind: Option<String>,
    /// Only entries logged while serving this registered route pattern.
    route: Option<String>,
    limit: Option<i64>,
}

/// Parse a `since` bound given either as epoch milliseconds or RFC 3339.
fn parse_since(raw: &str) -> Option<std::time::SystemTime> {
    if let Ok(millis) = raw.parse::<i64>() {
        let millis = u64::try_from(millis).ok()?;
        return Some(std::time::UNIX_EPOCH + std::time::Duration::from_millis(millis));
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| std::time::SystemTime::from(dt.with_timezone(&chrono::Utc)))
}

/// Get logs for one script (`uri` given) or across every script.
///
/// Entries come back oldest-first for a single script and newest-first for the
/// all-scripts view. `level`, `since` and `limit` filter in SQL; `limit` keeps
/// the newest matching entries.
#[utoipa::path(
    get,
    path = "/engine/script_logs",
    tags = ["Logging"],
    params(
        ("uri" = Option<String>, Query, description = "Script URI; omit for logs across all scripts"),
        ("level" = Option<String>, Query, description = "Only entries at this level, e.g. ERROR"),
        ("since" = Option<String>, Query, description = "Only entries at or after this time (epoch millis or RFC 3339)"),
        ("after_seq" = Option<i64>, Query, description = "Only entries written after this seq; read forward from where a previous response ended"),
        ("contains" = Option<String>, Query, description = "Only entries whose message contains this substring"),
        ("request_id" = Option<String>, Query, description = "Only the entries one invocation emitted, by x-request-id or a non-HTTP invocation's id"),
        ("kind" = Option<String>, Query, description = "Only entries from invocations of this kind, e.g. httpRoute, scheduled, streamCustomization"),
        ("route" = Option<String>, Query, description = "Only entries logged while serving this registered route pattern, e.g. /things/:id"),
        ("limit" = Option<i64>, Query, description = "Keep at most this many of the newest matching entries"),
    ),
    responses(
        (status = 200, description = "Log entries for one script or all scripts"),
        (status = 400, description = "Invalid query parameter"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn script_logs_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(params): Query<LogParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    let since = match params.since.as_deref() {
        Some(raw) => match parse_since(raw) {
            Some(since) => Some(since),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid 'since' value: {}", raw),
                );
            }
        },
        None => None,
    };
    if params.limit.is_some_and(|limit| limit <= 0) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Parameter 'limit' must be greater than zero".to_string(),
        );
    }

    let uri = params.uri.clone();
    let single_script = uri.is_some();
    let query = repository::LogQuery {
        script_uri: params.uri,
        level: params.level,
        since,
        after_seq: params.after_seq,
        contains: params.contains,
        request_id: params.request_id,
        kind: params.kind,
        route: params.route,
        limit: params.limit,
    };

    let result = tokio::task::spawn_blocking(move || {
        query_logs_authorized(&user, &query).map(|mut logs| {
            // A single script reads oldest-first, the order its own log view
            // has always used; the limit still selected the newest entries.
            if single_script {
                logs.reverse();
            }
            logs
        })
    })
    .await;

    let logs = match result {
        Ok(Ok(logs)) => logs,
        Ok(Err(e)) => {
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return error_response(status, format!("Failed to fetch logs: {}", e));
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to fetch logs: {}", e),
            );
        }
    };

    json_response(
        StatusCode::OK,
        json!({
            "uri": uri,
            "logs": logs,
            "count": logs.len(),
            "timestamp": iso_timestamp(),
        }),
    )
}

/// How often a live tail looks for entries written since its cursor.
///
/// Short enough that a person driving a client sees their own actions land,
/// long enough that an idle tail is a negligible query.
const LOG_TAIL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Entries one poll may carry. A burst larger than this is delivered over the
/// following polls rather than in one unbounded write, and nothing is dropped:
/// the cursor only advances past what was actually sent.
const LOG_TAIL_BATCH_LIMIT: i64 = 500;

/// Consecutive failed polls tolerated before the tail gives up and says so.
/// One failure is a blip worth riding out; a run of them is a tail that has
/// stopped being a tail, and a client that is told can reconnect from its last
/// seq instead of silently missing everything.
const LOG_TAIL_MAX_CONSECUTIVE_ERRORS: u32 = 3;

/// Most entries a tail will replay before going live.
const LOG_TAIL_MAX_BACKLOG: i64 = 1000;

/// Filters and starting point for a live tail. The filters are the ones
/// [`LogParams`] carries; what differs is where the stream starts.
#[derive(Deserialize, Default)]
pub struct LogTailParams {
    uri: Option<String>,
    level: Option<String>,
    contains: Option<String>,
    request_id: Option<String>,
    kind: Option<String>,
    route: Option<String>,
    /// Start after this `seq`, replaying everything written since. This is how
    /// a dropped tail resumes without a gap: the client reconnects with the
    /// last seq it saw.
    after_seq: Option<i64>,
    /// Start at this time (epoch millis or RFC 3339) instead of at the end of
    /// the log. Ignored when `after_seq` says where to start.
    since: Option<String>,
    /// Replay this many of the newest matching entries before going live.
    /// Ignored when `after_seq` or `since` says where to start.
    backlog: Option<i64>,
}

impl LogTailParams {
    /// The filters, with the starting point left to the caller.
    fn to_query(&self) -> repository::LogQuery {
        repository::LogQuery {
            script_uri: self.uri.clone(),
            level: self.level.clone(),
            since: None,
            after_seq: None,
            contains: self.contains.clone(),
            request_id: self.request_id.clone(),
            kind: self.kind.clone(),
            route: self.route.clone(),
            limit: None,
        }
    }
}

/// One SSE event carrying a log entry.
fn log_tail_event(entry: &repository::LogEntry) -> axum::response::sse::Event {
    axum::response::sse::Event::default()
        .event("log")
        .data(log_entry_json(entry).to_string())
}

/// Run one filtered log query off the async runtime.
///
/// The repository's query is blocking, and a tail runs one every poll for as
/// long as the client stays connected — running them inline would park a
/// runtime worker for the lifetime of every open tail.
async fn query_logs_off_runtime(
    user: UserContext,
    query: repository::LogQuery,
) -> AppResult<Vec<repository::LogEntry>> {
    match tokio::task::spawn_blocking(move || query_log_entries_authorized(&user, &query)).await {
        Ok(result) => result,
        Err(e) => Err(crate::error::AppError::internal(format!(
            "Log query task failed: {}",
            e
        ))),
    }
}

/// Follow a script's log as it is written, as Server-Sent Events.
///
/// Answers the question a one-shot listing cannot: what is this script doing
/// *now*. The tick, lease and stream paths are the hardest to debug precisely
/// because their output interleaves with every other invocation's, so the same
/// filters the listing takes apply here — narrowing a tail to one route, one
/// invocation kind or one request id is what makes watching a live session
/// legible.
///
/// Entries are delivered oldest-first as `log` events whose data is the same
/// JSON the listing returns. Each carries a `seq`; reconnecting with
/// `after_seq` set to the last one seen resumes without a gap.
///
/// Polls the database rather than being pushed to from the write path: every
/// instance in a cluster writes to the same table, so a tail sees the whole
/// cluster's output without a message bus, and it shows what was actually
/// committed rather than lines a rolled-back transaction never kept.
#[utoipa::path(
    get,
    path = "/engine/script_logs/stream",
    tags = ["Logging"],
    params(
        ("uri" = Option<String>, Query, description = "Script URI; omit to tail every script"),
        ("level" = Option<String>, Query, description = "Only entries at this level, e.g. ERROR"),
        ("contains" = Option<String>, Query, description = "Only entries whose message contains this substring"),
        ("request_id" = Option<String>, Query, description = "Only the entries one invocation emits"),
        ("kind" = Option<String>, Query, description = "Only entries from invocations of this kind, e.g. httpRoute, scheduled"),
        ("route" = Option<String>, Query, description = "Only entries logged while serving this registered route pattern"),
        ("after_seq" = Option<i64>, Query, description = "Resume after this seq, replaying everything written since"),
        ("since" = Option<String>, Query, description = "Start at this time (epoch millis or RFC 3339) instead of at the end of the log"),
        ("backlog" = Option<i64>, Query, description = "Replay this many of the newest matching entries before going live"),
    ),
    responses(
        (status = 200, description = "Event stream of log entries"),
        (status = 400, description = "Invalid query parameter"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn script_logs_stream_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(params): Query<LogTailParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    let since = match params.since.as_deref() {
        Some(raw) => match parse_since(raw) {
            Some(since) => Some(since),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid 'since' value: {}", raw),
                );
            }
        },
        None => None,
    };
    if params.backlog.is_some_and(|backlog| backlog < 0) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Parameter 'backlog' must not be negative".to_string(),
        );
    }
    if params.after_seq.is_some_and(|seq| seq < 0) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Parameter 'after_seq' must not be negative".to_string(),
        );
    }

    // The opening query doubles as the authorization check: a caller without
    // ViewLogs is refused with a status code here, rather than being handed an
    // event stream that would never carry anything.
    let opening = {
        let mut query = params.to_query();
        match (params.after_seq, since) {
            // Resuming: everything written since the cursor, oldest first.
            (Some(after_seq), _) => {
                query.after_seq = Some(after_seq);
                query.limit = Some(LOG_TAIL_BATCH_LIMIT);
            }
            // Starting from a time. The cursor makes the batch the *oldest*
            // entries at or after it, which is what lets the poll loop carry
            // the rest forward; taking the newest instead would silently drop
            // everything between the requested time and the last page.
            (None, Some(since)) => {
                query.since = Some(since);
                query.after_seq = Some(0);
                query.limit = Some(LOG_TAIL_BATCH_LIMIT);
            }
            // Starting at the end: the requested backlog, or nothing at all.
            // Even a backlog of zero reads one entry, which is what tells the
            // tail the seq to start after; that entry is not sent on.
            (None, None) => {
                query.limit = Some(params.backlog.unwrap_or(0).clamp(1, LOG_TAIL_MAX_BACKLOG))
            }
        }
        query
    };
    let replay_opening =
        params.after_seq.is_some() || since.is_some() || params.backlog.unwrap_or(0) > 0;

    let mut opening_entries = match query_logs_off_runtime(user.clone(), opening).await {
        Ok(entries) => entries,
        Err(e) => {
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            return error_response(status, format!("Failed to fetch logs: {}", e));
        }
    };
    // Queries answer newest-first; a tail reads in the order things happened.
    opening_entries.reverse();

    // Where the live tail picks up: after the last entry the opening query
    // named. With nothing to replay, finding that entry was all the opening
    // query was for, so it is not sent on.
    let opening_cursor = opening_entries.last().map(|entry| entry.seq);
    if !replay_opening {
        opening_entries.clear();
    }

    let mut cursor = match opening_cursor {
        Some(cursor) => cursor,
        // The opening query matched nothing, so it named no entry to start
        // after. Starting at zero would make the first poll replay the log from
        // the beginning, so the tail starts at the end of it instead — a filter
        // that has not matched yet waits for a line that does. The fallback
        // reads the newest entry written by anyone, since the filters have
        // already been shown to match nothing that exists.
        None => {
            let newest = repository::LogQuery {
                limit: Some(1),
                ..Default::default()
            };
            match query_logs_off_runtime(user.clone(), newest).await {
                Ok(entries) => entries.first().map(|entry| entry.seq).unwrap_or_default(),
                Err(e) => {
                    let status = StatusCode::from_u16(e.status_code())
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    return error_response(status, format!("Failed to fetch logs: {}", e));
                }
            }
        }
    };

    let filters = params.to_query();
    let stream = async_stream::stream! {
        // Tells the client the tail is live and where it starts, so it can
        // resume from here even if nothing is ever logged.
        yield Ok::<_, std::convert::Infallible>(
            axum::response::sse::Event::default()
                .event("open")
                .data(json!({ "seq": cursor, "timestamp": iso_timestamp() }).to_string()),
        );

        for entry in &opening_entries {
            yield Ok(log_tail_event(entry));
        }

        let mut consecutive_errors = 0u32;
        loop {
            tokio::time::sleep(LOG_TAIL_POLL_INTERVAL).await;

            let mut query = filters.clone();
            query.after_seq = Some(cursor);
            query.limit = Some(LOG_TAIL_BATCH_LIMIT);

            let mut entries = match query_logs_off_runtime(user.clone(), query).await {
                Ok(entries) => {
                    consecutive_errors = 0;
                    entries
                }
                Err(e) => {
                    consecutive_errors += 1;
                    warn!("Log tail query failed ({}): {}", consecutive_errors, e);
                    if consecutive_errors >= LOG_TAIL_MAX_CONSECUTIVE_ERRORS {
                        yield Ok(axum::response::sse::Event::default().event("error").data(
                            json!({
                                "error": format!("Log tail stopped: {}", e),
                                "seq": cursor,
                            })
                            .to_string(),
                        ));
                        break;
                    }
                    continue;
                }
            };

            entries.reverse();
            for entry in &entries {
                // Advance only past what has been sent, so a batch cut short by
                // the limit resumes at the right place on the next poll.
                cursor = entry.seq;
                yield Ok(log_tail_event(entry));
            }
        }
    };

    axum::response::Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

/// Delete logs for one script, or prune every script back to its newest
/// entries when `uri` is omitted.
#[utoipa::path(
    delete,
    path = "/engine/script_logs",
    tags = ["Logging"],
    params(("uri" = Option<String>, Query, description = "Script URI to clear; omit to prune every script")),
    responses(
        (status = 200, description = "Logs cleared or pruned"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn script_logs_delete_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(params): Query<ScriptParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let uri = params.uri.clone();

    let result =
        tokio::task::spawn_blocking(move || delete_logs_authorized(&user, uri.as_deref())).await;

    match result {
        Ok(Ok(body)) => json_response(StatusCode::OK, body),
        Ok(Err(e)) => {
            let status =
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error_response(status, format!("Failed to delete logs: {}", e))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to delete logs: {}", e),
        ),
    }
}

#[derive(Deserialize, Default)]
pub struct AssetQuery {
    script: Option<String>,
    asset: Option<String>,
    /// Inclusive 1-based line range to read, e.g. `120-180`, `120-`, or `120`.
    lines: Option<String>,
    /// Regular expression: answers with the lines that match it instead of
    /// with the file.
    grep: Option<String>,
}

/// One edit of a patch request.
#[derive(Deserialize, Default)]
pub struct AssetEditBody {
    old_string: Option<String>,
    new_string: Option<String>,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize, Default)]
pub struct AssetPatchBody {
    script: Option<String>,
    asset: Option<String>,
    edits: Option<Vec<AssetEditBody>>,
    /// Digest the caller believes the asset currently has; the patch is
    /// refused if it has moved on.
    #[serde(alias = "sha256")]
    base_sha256: Option<String>,
    reinit: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct AssetBody {
    asset: Option<String>,
    mimetype: Option<String>,
    content: Option<String>,
}

/// One file of a batch write request. `name`/`content_base64` are the batch
/// spelling; `asset`/`content` are accepted too, so a caller already written
/// against the single-asset route does not have to rename its fields.
#[derive(Deserialize, Default)]
pub struct AssetBatchFile {
    #[serde(alias = "asset")]
    name: Option<String>,
    mimetype: Option<String>,
    #[serde(alias = "content")]
    content_base64: Option<String>,
    sha256: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct AssetBatchBody {
    script: Option<String>,
    files: Option<Vec<AssetBatchFile>>,
    reinit: Option<String>,
}

/// Whether a batch write runs the script's init() when it is done.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReinitMode {
    /// Run init() once, after the whole batch has landed, and report it.
    After,
    /// Leave init() alone — for a caller pushing one part of a larger change
    /// that is not coherent yet.
    Never,
}

impl ReinitMode {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("after") => Ok(ReinitMode::After),
            Some("never") => Ok(ReinitMode::Never),
            Some(other) => Err(format!(
                "Invalid reinit mode '{}': expected 'after' or 'never'",
                other
            )),
        }
    }
}

fn error_response(status: StatusCode, message: String) -> Response {
    json_response(status, json!({ "error": message }))
}

/// The scoping a read asked for, or why it does not parse.
fn read_options(lines: Option<&str>, grep: Option<String>) -> Result<AssetReadOptions, String> {
    let lines = match lines {
        Some(raw) => Some(LineRange::parse(raw)?),
        None => None,
    };
    Ok(AssetReadOptions { lines, grep })
}

/// List assets for a script, or read one asset when `asset` is given.
///
/// A read is whole-file base64 by default. `lines` and `grep` narrow it to
/// part of a text asset, so a caller looking for one function in a large
/// module does not have to transfer the module to find it.
#[utoipa::path(
    get,
    path = "/engine/assets",
    tags = ["Assets"],
    params(
        ("script" = String, Query, description = "URI of the script whose assets to manage"),
        ("asset" = Option<String>, Query, description = "Asset URI to fetch; omit to list all"),
        ("lines" = Option<String>, Query, description = "Inclusive 1-based line range of a text asset, e.g. '120-180', '120-', or '120'"),
        ("grep" = Option<String>, Query, description = "Regular expression; answers with matching line numbers instead of the file"),
    ),
    responses(
        (status = 200, description = "Asset list, asset content (base64), a line range, or matching lines"),
        (status = 400, description = "Missing required parameter, an unreadable or out-of-range 'lines', an invalid 'grep', or a text view of a non-text asset"),
        (status = 404, description = "Asset not found or access denied"),
    )
)]
pub async fn assets_get_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: script".to_string(),
        );
    };

    match query.asset {
        Some(asset) => {
            let options = match read_options(query.lines.as_deref(), query.grep) {
                Ok(options) => options,
                Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
            };
            let (user, script_cl, asset_cl) = (user, script.clone(), asset.clone());
            let result = tokio::task::spawn_blocking(move || {
                read_asset_authorized(&user, &script_cl, &asset_cl, &options)
            })
            .await
            .unwrap_or(Err(AssetReadError::NotFound));

            match result {
                Ok(read) => {
                    let mut body = read.to_json();
                    if let Some(object) = body.as_object_mut() {
                        object.insert("script".to_string(), json!(script));
                        object.insert("asset".to_string(), json!(asset));
                    }
                    json_response(StatusCode::OK, body)
                }
                Err(AssetReadError::AccessDenied) => {
                    error_response(StatusCode::NOT_FOUND, "Error: Access denied".to_string())
                }
                Err(AssetReadError::NotFound) => error_response(
                    StatusCode::NOT_FOUND,
                    format!("Asset '{}' not found", asset),
                ),
                Err(AssetReadError::Validation(message)) => {
                    error_response(StatusCode::BAD_REQUEST, message)
                }
            }
        }
        None => {
            if query.lines.is_some() || query.grep.is_some() {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "'lines' and 'grep' read one asset: name it with the 'asset' parameter"
                        .to_string(),
                );
            }
            let script_cl = script.clone();
            let assets =
                tokio::task::spawn_blocking(move || list_assets_authorized(&user, &script_cl))
                    .await
                    .unwrap_or_default();
            json_response(
                StatusCode::OK,
                json!({ "script": script, "assets": assets }),
            )
        }
    }
}

/// Create or update an asset for a script.
#[utoipa::path(
    post,
    path = "/engine/assets",
    tags = ["Assets"],
    params(("script" = String, Query, description = "URI of the script that will own this asset")),
    request_body(content_type = "application/json",
        description = "JSON fields: asset (required), mimetype (required), content (required, base64, max 10MB)"),
    responses(
        (status = 201, description = "Asset created or updated"),
        (status = 400, description = "Missing or invalid parameters"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn assets_post_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: script".to_string(),
        );
    };
    let body: AssetBody = serde_json::from_slice(&body).unwrap_or_default();
    let Some(asset) = body.asset.or(query.asset) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: asset".to_string(),
        );
    };
    let (Some(mimetype), Some(content)) = (body.mimetype, body.content) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required fields: mimetype, content".to_string(),
        );
    };

    let (script_cl, asset_cl) = (script.clone(), asset.clone());
    let result = tokio::task::spawn_blocking(move || {
        upsert_asset_authorized(&user, &script_cl, &asset_cl, &mimetype, &content)
    })
    .await
    .unwrap_or(Err(AssetWriteError::Storage("join error".to_string())));

    match result {
        Ok(revision) => json_response(
            StatusCode::CREATED,
            json!({
                "message": format!("Asset '{}' upserted successfully", asset),
                "script": script,
                "asset": asset,
                "revision": revision,
            }),
        ),
        Err(AssetWriteError::AccessDenied) => {
            error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string())
        }
        Err(AssetWriteError::Validation(msg)) => error_response(StatusCode::BAD_REQUEST, msg),
        Err(AssetWriteError::Storage(msg)) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, msg)
        }
    }
}

/// Write several of a script's assets in one request.
///
/// A script's modules are one unit of change, and writing them one request at
/// a time makes the engine act on each partial state: every single-asset write
/// invalidates the script's prepared program and notifies the rest of the
/// cluster, so every other instance reinitializes the script once per file,
/// each time from a tree that is still being uploaded. One batch is one
/// transaction, one notification, and one init().
#[utoipa::path(
    post,
    path = "/engine/assets/batch",
    tags = ["Assets"],
    params(("script" = String, Query, description = "URI of the script that will own these assets")),
    request_body(content_type = "application/json",
        description = "JSON fields: script (required here or as a query parameter), \
                       files (required, array of { name, content_base64, mimetype?, sha256? }), \
                       reinit ('after' by default, or 'never')"),
    responses(
        (status = 200, description = "Per-file result and the init() run that followed"),
        (status = 400, description = "Missing or invalid parameters; nothing was written"),
        (status = 403, description = "Access denied"),
    )
)]
pub async fn assets_batch_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    let parsed: AssetBatchBody = match serde_json::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(e) => {
            return error_response(StatusCode::BAD_REQUEST, format!("Invalid JSON body: {}", e));
        }
    };

    let Some(script) = parsed.script.or(query.script) else {
        return missing_param_response("script");
    };
    let reinit = match ReinitMode::parse(parsed.reinit.as_deref()) {
        Ok(reinit) => reinit,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let Some(files) = parsed.files else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required field: files".to_string(),
        );
    };

    let mut writes = Vec::with_capacity(files.len());
    for (index, file) in files.into_iter().enumerate() {
        let Some(name) = file.name else {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("files[{}]: missing required field: name", index),
            );
        };
        let Some(content_base64) = file.content_base64 else {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!(
                    "files[{}] ('{}'): missing required field: content_base64",
                    index, name
                ),
            );
        };
        writes.push(AssetWrite {
            name,
            mimetype: file.mimetype,
            content_base64,
            expected_sha256: file.sha256,
        });
    }

    let script_cl = script.clone();
    let result =
        tokio::task::spawn_blocking(move || upsert_assets_authorized(&user, &script_cl, &writes))
            .await
            .unwrap_or_else(|e| Err(AssetWriteError::Storage(format!("join error: {}", e))));

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(AssetWriteError::AccessDenied) => {
            return error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string());
        }
        Err(AssetWriteError::Validation(msg)) => {
            return error_response(StatusCode::BAD_REQUEST, msg);
        }
        Err(AssetWriteError::Storage(msg)) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, msg);
        }
    };

    // A batch that changed nothing has nothing to re-register, and running
    // init() anyway would drop and rebuild registrations that are already
    // correct.
    let init = match (reinit, outcome.written) {
        (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
        (ReinitMode::After, 0) => json!({ "ran": false, "reason": "no files changed" }),
        (ReinitMode::After, _) => init_result_json(&reinitialize_script(&script).await),
    };

    json_response(
        StatusCode::OK,
        json!({
            "script": script,
            "results": outcome.results.iter().map(AssetWriteOutcome::to_json).collect::<Vec<Value>>(),
            "written": outcome.written,
            "revision": outcome.revision,
            "init": init,
            "timestamp": iso_timestamp(),
        }),
    )
}

/// Edit one of a script's assets in place.
///
/// The point of it is what is *not* in the request: a caller changing three
/// lines of a module sends those three lines, not the module. That keeps a
/// small change small — and, with `base_sha256`, makes it a change to a known
/// version rather than to whatever happens to be stored.
#[utoipa::path(
    patch,
    path = "/engine/assets",
    tags = ["Assets"],
    params(
        ("script" = Option<String>, Query, description = "URI of the script that owns the asset (may also be given in the body)"),
        ("asset" = Option<String>, Query, description = "Asset URI to edit (may also be given in the body)"),
    ),
    request_body(content_type = "application/json",
        description = "JSON fields: edits (required, array of { old_string, new_string, replace_all? }), \
                       base_sha256 (optional precondition on the stored content), \
                       reinit ('after' by default, or 'never')"),
    responses(
        (status = 200, description = "The content that resulted: sha256, bytes, replacements, and the init() run that followed"),
        (status = 400, description = "Missing or invalid parameters, or an edit that does not match; nothing was written"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Asset not found"),
        (status = 409, description = "The asset no longer has the content named by base_sha256; nothing was written"),
    )
)]
pub async fn assets_patch_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());

    let parsed: AssetPatchBody = match serde_json::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(e) => {
            return error_response(StatusCode::BAD_REQUEST, format!("Invalid JSON body: {}", e));
        }
    };

    let Some(script) = parsed.script.or(query.script) else {
        return missing_param_response("script");
    };
    let Some(asset) = parsed.asset.or(query.asset) else {
        return missing_param_response("asset");
    };
    let reinit = match ReinitMode::parse(parsed.reinit.as_deref()) {
        Ok(reinit) => reinit,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let Some(edits) = parsed.edits else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required field: edits".to_string(),
        );
    };

    let mut prepared = Vec::with_capacity(edits.len());
    for (index, edit) in edits.into_iter().enumerate() {
        let Some(old_string) = edit.old_string else {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("edits[{}]: missing required field: old_string", index),
            );
        };
        // Required rather than defaulted to empty, so a misspelled field name
        // cannot quietly turn a replacement into a deletion.
        let Some(new_string) = edit.new_string else {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!(
                    "edits[{}]: missing required field: new_string (pass \"\" to delete the text)",
                    index
                ),
            );
        };
        prepared.push(AssetEdit {
            old_string,
            new_string,
            replace_all: edit.replace_all,
        });
    }

    let (script_cl, asset_cl) = (script.clone(), asset.clone());
    let base_sha256 = parsed.base_sha256.clone();
    let result = tokio::task::spawn_blocking(move || {
        patch_asset_authorized(
            &user,
            &script_cl,
            &asset_cl,
            &prepared,
            base_sha256.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|e| Err(AssetPatchError::Storage(format!("join error: {}", e))));

    let outcome = match result {
        Ok(outcome) => outcome,
        Err(AssetPatchError::AccessDenied) => {
            return error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string());
        }
        Err(AssetPatchError::NotFound) => {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("Asset '{}' not found", asset),
            );
        }
        Err(AssetPatchError::Validation(message)) => {
            return error_response(StatusCode::BAD_REQUEST, message);
        }
        Err(AssetPatchError::Conflict { expected, actual }) => {
            // The current digest comes back with the refusal, so a caller can
            // re-read, rebase its edits, and retry without a second round trip
            // to find out what it is now working against.
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "error": format!(
                        "Asset '{}' has changed since it was read (expected {}, stored {})",
                        asset, expected, actual
                    ),
                    "script": script,
                    "asset": asset,
                    "expected_sha256": expected,
                    "sha256": actual,
                    "timestamp": iso_timestamp(),
                }),
            );
        }
        Err(AssetPatchError::Storage(message)) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, message);
        }
    };

    // Edits that cancelled out changed no file, so there is nothing for
    // init() to pick up and re-registering would only churn what is already
    // right.
    let init = match (reinit, outcome.status) {
        (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
        (_, "unchanged") => json!({ "ran": false, "reason": "no change" }),
        (ReinitMode::After, _) => init_result_json(&reinitialize_script(&script).await),
    };

    let mut body = outcome.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("script".to_string(), json!(script));
        object.insert("asset".to_string(), json!(asset));
        object.insert("init".to_string(), init);
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }
    json_response(StatusCode::OK, body)
}

/// Delete an asset from a script.
#[utoipa::path(
    delete,
    path = "/engine/assets",
    tags = ["Assets"],
    params(
        ("script" = String, Query, description = "URI of the script that owns the asset"),
        ("asset" = String, Query, description = "Asset URI to delete"),
    ),
    responses(
        (status = 200, description = "Asset deleted"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Access denied"),
        (status = 404, description = "Asset not found"),
    )
)]
pub async fn assets_delete_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<AssetQuery>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: script".to_string(),
        );
    };
    let Some(asset) = query.asset else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Missing required parameter: asset".to_string(),
        );
    };

    let (script_cl, asset_cl) = (script.clone(), asset.clone());
    let result =
        tokio::task::spawn_blocking(move || delete_asset_authorized(&user, &script_cl, &asset_cl))
            .await
            .unwrap_or(Ok((false, None)));

    match result {
        Err(AssetFetchError::AccessDenied) => {
            error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string())
        }
        Ok((false, _)) | Err(AssetFetchError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            format!("Asset '{}' not found", asset),
        ),
        Ok((true, revision)) => json_response(
            StatusCode::OK,
            json!({
                "message": format!("Asset '{}' deleted successfully", asset),
                "script": script,
                "asset": asset,
                "revision": revision,
            }),
        ),
    }
}

#[derive(Deserialize, Default)]
pub struct RevisionListParams {
    script: Option<String>,
    /// Restrict the history to one file's changes.
    asset: Option<String>,
    /// Include the file manifest of each revision listed.
    files: Option<bool>,
    limit: Option<i64>,
}

fn revision_to_json(revision: &revisions::Revision) -> Value {
    json!({
        "revision": revision.revision,
        "parent": revision.parent,
        "origin": revision.origin,
        "label": revision.label,
        "at": revision.created_at.to_rfc3339(),
        "by": revision.created_by,
        "files": revision.file_count,
        "bytes": revision.total_bytes,
        // Absent rather than false when init() has not reported: a revision
        // whose outcome is unknown is not one that failed, and a caller
        // looking for somewhere safe to land must be able to tell them apart.
        "initOk": revision.init_ok,
        "initError": revision.init_error,
    })
}

fn revision_file_to_json(file: &revisions::RevisionFile) -> Value {
    json!({
        "uri": file.uri,
        "name": file.name,
        "mimetype": file.mimetype,
        "sha256": file.sha256,
        "bytes": file.bytes,
    })
}

/// What a script's files have been.
///
/// Every write records a revision, so this is the history a caller editing
/// through `/engine/assets` has instead of a checkout: what changed, when, at
/// whose hand, and — uniquely — whether the script still initialised
/// afterwards. `lastGood` names the newest revision that did, which is the
/// answer to "where do I put this back to".
#[utoipa::path(
    get,
    path = "/engine/revisions",
    tags = ["Scripts"],
    params(
        ("script" = String, Query, description = "URI of the script whose history to read"),
        ("asset" = Option<String>, Query, description = "Only the revisions in which this file changed"),
        ("files" = Option<bool>, Query, description = "Include each revision's file manifest"),
        ("limit" = Option<i64>, Query, description = "Keep at most this many of the newest revisions (default 50)"),
    ),
    responses(
        (status = 200, description = "Revision history, newest first"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Not an administrator or owner of the script"),
    )
)]
pub async fn revisions_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<RevisionListParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return missing_param_response("script");
    };

    let user_for_auth = user.clone();
    let script_for_auth = script.clone();
    let allowed = tokio::task::spawn_blocking(move || {
        can_access_assets(&user_for_auth, &script_for_auth, &Capability::ReadAssets)
    })
    .await
    .unwrap_or(false);

    if !allowed {
        return error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string());
    }

    let limit = query.limit.unwrap_or(50);

    // One file's history is a different question from the script's, and
    // answering it by listing every revision and letting the caller diff the
    // manifests would make them read the whole history to find the two
    // revisions where their file moved.
    if let Some(asset) = query.asset {
        return match revisions::file_history(&script, &asset, limit).await {
            Ok(history) => json_response(
                StatusCode::OK,
                json!({
                    "script": script,
                    "asset": asset,
                    "history": history
                        .iter()
                        .map(|(revision, file)| {
                            let mut entry = revision_file_to_json(file);
                            if let Some(object) = entry.as_object_mut() {
                                object.insert("revision".to_string(), json!(revision));
                            }
                            entry
                        })
                        .collect::<Vec<Value>>(),
                    "timestamp": iso_timestamp(),
                }),
            ),
            Err(e) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read file history: {}", e),
            ),
        };
    }

    let listed = match revisions::list(&script, limit).await {
        Ok(listed) => listed,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read revisions: {}", e),
            );
        }
    };

    let mut entries = Vec::with_capacity(listed.len());
    for revision in &listed {
        let mut entry = revision_to_json(revision);
        if query.files.unwrap_or(false)
            && let Ok(Some(files)) = revisions::files(&script, revision.revision).await
            && let Some(object) = entry.as_object_mut()
        {
            object.insert(
                "manifest".to_string(),
                json!(
                    files
                        .iter()
                        .map(revision_file_to_json)
                        .collect::<Vec<Value>>()
                ),
            );
        }
        entries.push(entry);
    }

    json_response(
        StatusCode::OK,
        json!({
            "script": script,
            "revisions": entries,
            "head": listed.first().map(|revision| revision.revision),
            "lastGood": revisions::last_good(&script).await.ok().flatten(),
            "timestamp": iso_timestamp(),
        }),
    )
}

/// List all scripts with metadata.
#[utoipa::path(
    get,
    path = "/engine/scripts",
    tags = ["Scripts"],
    responses(
        (status = 200, description = "Script metadata list"),
    )
)]
pub async fn list_scripts_route(auth_user: Option<Extension<AuthUser>>) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let scripts = tokio::task::spawn_blocking(move || {
        list_scripts_authorized(&user)
            .iter()
            .map(|meta| {
                let millis = |t: std::time::SystemTime| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_millis() as f64)
                };
                json!({
                    "uri": meta.uri,
                    "name": meta.name,
                    "size": meta.content.len(),
                    "updatedAt": millis(meta.updated_at),
                    "createdAt": millis(meta.created_at),
                    "initialized": meta.initialized,
                    "initError": meta.init_error.as_deref(),
                })
            })
            .collect::<Vec<Value>>()
    })
    .await
    .unwrap_or_default();

    json_response(
        StatusCode::OK,
        json!({
            "scripts": scripts,
            "count": scripts.len(),
            "timestamp": iso_timestamp(),
        }),
    )
}

/// Init status for one script (`uri` given) or all scripts.
#[utoipa::path(
    get,
    path = "/engine/script_init_status",
    tags = ["Scripts"],
    params(("uri" = Option<String>, Query, description = "Script URI; omit for all scripts")),
    responses(
        (status = 200, description = "Init status for one or all scripts"),
    )
)]
pub async fn script_init_status_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    match query.uri {
        Some(uri) => {
            let uri_cl = uri.clone();
            let status =
                tokio::task::spawn_blocking(move || init_status_authorized(&user, &uri_cl))
                    .await
                    .unwrap_or(None);
            json_response(
                StatusCode::OK,
                json!({ "uri": uri, "status": status, "timestamp": iso_timestamp() }),
            )
        }
        None => {
            let statuses = tokio::task::spawn_blocking(move || {
                list_scripts_authorized(&user)
                    .iter()
                    .map(script_init_status_json)
                    .collect::<Vec<Value>>()
            })
            .await
            .unwrap_or_default();
            json_response(
                StatusCode::OK,
                json!({
                    "statuses": statuses,
                    "count": statuses.len(),
                    "timestamp": iso_timestamp(),
                }),
            )
        }
    }
}

#[derive(Deserialize, Default)]
pub struct OwnerParams {
    uri: Option<String>,
    owner: Option<String>,
}

/// List a script's owners.
#[utoipa::path(
    get,
    path = "/engine/script_owners",
    tags = ["Scripts"],
    params(("uri" = String, Query, description = "Script URI")),
    responses(
        (status = 200, description = "Owner list"),
        (status = 400, description = "Missing required parameter"),
    )
)]
pub async fn script_owners_get_route(Query(query): Query<OwnerParams>) -> Response {
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };

    let uri_cl = uri.clone();
    let result = tokio::task::spawn_blocking(move || owners_authorized(&uri_cl))
        .await
        .unwrap_or_else(|e| Err(format!("join error: {}", e)));

    match result {
        Ok(owners) => json_response(
            StatusCode::OK,
            json!({
                "uri": uri,
                "owners": owners,
                "count": owners.len(),
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(details) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": details, "uri": uri, "timestamp": iso_timestamp() }),
        ),
    }
}

fn owner_change_error_response(uri: &str, owner: &str, error: OwnerChangeError) -> Response {
    let (status, message) = match error {
        OwnerChangeError::AccessDenied => (
            StatusCode::FORBIDDEN,
            "Permission denied. You must be an administrator or owner".to_string(),
        ),
        OwnerChangeError::LastOwner => (
            StatusCode::CONFLICT,
            "Cannot remove the last owner. Transfer ownership to another user first, or contact an administrator.".to_string(),
        ),
        OwnerChangeError::Storage(details) => (StatusCode::INTERNAL_SERVER_ERROR, details),
    };
    json_response(
        status,
        json!({ "error": message, "uri": uri, "owner": owner, "timestamp": iso_timestamp() }),
    )
}

/// Add an owner to a script (admin or current owner only).
#[utoipa::path(
    post,
    path = "/engine/script_owners",
    tags = ["Scripts"],
    request_body(content_type = "application/x-www-form-urlencoded",
        description = "Form fields: uri (required), owner (required)"),
    responses(
        (status = 200, description = "Owner added"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Permission denied"),
    )
)]
pub async fn script_owners_post_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<OwnerParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form: OwnerParams = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(uri) = form.uri.or(query.uri) else {
        return missing_param_response("uri");
    };
    let Some(owner) = form.owner.or(query.owner) else {
        return missing_param_response("owner");
    };

    let (uri_cl, owner_cl) = (uri.clone(), owner.clone());
    let result =
        tokio::task::spawn_blocking(move || add_owner_authorized(&user, &uri_cl, &owner_cl))
            .await
            .unwrap_or_else(|e| Err(OwnerChangeError::Storage(format!("join error: {}", e))));

    match result {
        Ok(()) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "uri": uri,
                "owner": owner,
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(error) => owner_change_error_response(&uri, &owner, error),
    }
}

/// Remove an owner from a script (admin or current owner only; non-admins
/// cannot remove the last owner).
#[utoipa::path(
    delete,
    path = "/engine/script_owners",
    tags = ["Scripts"],
    params(
        ("uri" = String, Query, description = "Script URI"),
        ("owner" = String, Query, description = "Owner user id to remove"),
    ),
    responses(
        (status = 200, description = "Owner removed"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Owner not found"),
        (status = 409, description = "Cannot remove the last owner"),
    )
)]
pub async fn script_owners_delete_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<OwnerParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };
    let Some(owner) = query.owner else {
        return missing_param_response("owner");
    };

    let (uri_cl, owner_cl) = (uri.clone(), owner.clone());
    let result =
        tokio::task::spawn_blocking(move || remove_owner_authorized(&user, &uri_cl, &owner_cl))
            .await
            .unwrap_or_else(|e| Err(OwnerChangeError::Storage(format!("join error: {}", e))));

    match result {
        Ok(true) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "uri": uri,
                "owner": owner,
                "timestamp": iso_timestamp(),
            }),
        ),
        Ok(false) => json_response(
            StatusCode::NOT_FOUND,
            json!({
                "error": format!("Owner '{}' was not found for script '{}'", owner, uri),
                "uri": uri,
                "owner": owner,
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(error) => owner_change_error_response(&uri, &owner, error),
    }
}

#[derive(Deserialize, Default)]
pub struct SecretQuery {
    script: Option<String>,
    key: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct SecretBody {
    key: Option<String>,
    value: Option<String>,
}

fn secret_error_response(script: &str, error: SecretAccessError) -> Response {
    let (status, message) = match error {
        SecretAccessError::AccessDenied => (
            StatusCode::FORBIDDEN,
            "Permission denied. You must be an administrator or owner of the script".to_string(),
        ),
        SecretAccessError::Validation(details) => (StatusCode::BAD_REQUEST, details),
        SecretAccessError::Storage(details) => (StatusCode::INTERNAL_SERVER_ERROR, details),
    };
    json_response(
        status,
        json!({ "error": message, "script": script, "timestamp": iso_timestamp() }),
    )
}

/// List the secret keys stored for a script (values are never returned).
#[utoipa::path(
    get,
    path = "/engine/secrets",
    tags = ["Secrets"],
    params(("script" = String, Query, description = "URI of the script whose secrets to manage")),
    responses(
        (status = 200, description = "Secret key list (no values)"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Permission denied"),
    )
)]
pub async fn secrets_get_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<SecretQuery>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return missing_param_response("script");
    };

    let script_cl = script.clone();
    let result = tokio::task::spawn_blocking(move || list_secrets_authorized(&user, &script_cl))
        .await
        .unwrap_or_else(|e| Err(SecretAccessError::Storage(format!("join error: {}", e))));

    match result {
        Ok(keys) => json_response(
            StatusCode::OK,
            json!({
                "script": script,
                "keys": keys,
                "count": keys.len(),
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(error) => secret_error_response(&script, error),
    }
}

/// Store a secret for a script.
#[utoipa::path(
    post,
    path = "/engine/secrets",
    tags = ["Secrets"],
    params(("script" = String, Query, description = "URI of the script whose secrets to manage")),
    request_body(content_type = "application/json",
        description = "JSON fields: key (required), value (required, max 1MB)"),
    responses(
        (status = 200, description = "Secret stored"),
        (status = 400, description = "Missing or invalid parameters"),
        (status = 403, description = "Permission denied"),
    )
)]
pub async fn secrets_post_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<SecretQuery>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return missing_param_response("script");
    };
    let body: SecretBody = serde_json::from_slice(&body).unwrap_or_default();
    let Some(key) = body.key else {
        return missing_param_response("key");
    };
    let Some(value) = body.value else {
        return missing_param_response("value");
    };

    let (script_cl, key_cl) = (script.clone(), key.clone());
    let result = tokio::task::spawn_blocking(move || {
        set_secret_authorized(&user, &script_cl, &key_cl, &value)
    })
    .await
    .unwrap_or_else(|e| Err(SecretAccessError::Storage(format!("join error: {}", e))));

    match result {
        Ok(()) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "script": script,
                "key": key,
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(error) => secret_error_response(&script, error),
    }
}

/// Remove one secret (`key` given) or all secrets from a script.
#[utoipa::path(
    delete,
    path = "/engine/secrets",
    tags = ["Secrets"],
    params(
        ("script" = String, Query, description = "URI of the script whose secrets to manage"),
        ("key" = Option<String>, Query, description = "Secret key to remove; omit to clear all"),
    ),
    responses(
        (status = 200, description = "Secret(s) removed"),
        (status = 400, description = "Missing required parameter"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "Secret key not found"),
    )
)]
pub async fn secrets_delete_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<SecretQuery>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(script) = query.script else {
        return missing_param_response("script");
    };

    match query.key {
        Some(key) => {
            let (script_cl, key_cl) = (script.clone(), key.clone());
            let result = tokio::task::spawn_blocking(move || {
                remove_secret_authorized(&user, &script_cl, &key_cl)
            })
            .await
            .unwrap_or_else(|e| Err(SecretAccessError::Storage(format!("join error: {}", e))));

            match result {
                Ok(true) => json_response(
                    StatusCode::OK,
                    json!({
                        "success": true,
                        "script": script,
                        "key": key,
                        "timestamp": iso_timestamp(),
                    }),
                ),
                Ok(false) => json_response(
                    StatusCode::NOT_FOUND,
                    json!({
                        "error": format!("Secret '{}' not found for script '{}'", key, script),
                        "script": script,
                        "key": key,
                        "timestamp": iso_timestamp(),
                    }),
                ),
                Err(error) => secret_error_response(&script, error),
            }
        }
        None => {
            let script_cl = script.clone();
            let result =
                tokio::task::spawn_blocking(move || clear_secrets_authorized(&user, &script_cl))
                    .await
                    .unwrap_or_else(|e| {
                        Err(SecretAccessError::Storage(format!("join error: {}", e)))
                    });

            match result {
                Ok(()) => json_response(
                    StatusCode::OK,
                    json!({
                        "success": true,
                        "cleared": true,
                        "script": script,
                        "timestamp": iso_timestamp(),
                    }),
                ),
                Err(error) => secret_error_response(&script, error),
            }
        }
    }
}

#[derive(Deserialize, Default)]
pub struct UserRoleParams {
    #[serde(alias = "userId")]
    user_id: Option<String>,
    role: Option<String>,
}

/// Role changes carry two short scalar fields, so accept either a JSON body or
/// a form-encoded one rather than making callers guess.
fn parse_user_role_body(body: &[u8]) -> UserRoleParams {
    serde_json::from_slice(body)
        .ok()
        .or_else(|| serde_urlencoded::from_bytes(body).ok())
        .unwrap_or_default()
}

fn user_admin_error_response(user_id: Option<&str>, error: UserAdminError) -> Response {
    let (status, message) = match error {
        UserAdminError::AccessDenied => (
            StatusCode::FORBIDDEN,
            "Permission denied. Administrator privileges are required".to_string(),
        ),
        UserAdminError::Validation(details) => (StatusCode::BAD_REQUEST, details),
        UserAdminError::UserNotFound(id) => {
            (StatusCode::NOT_FOUND, format!("User not found: {}", id))
        }
        UserAdminError::LastAdministrator => (
            StatusCode::CONFLICT,
            "Cannot remove the last administrator. Grant the Administrator role to another user first.".to_string(),
        ),
        UserAdminError::Storage(details) => (StatusCode::INTERNAL_SERVER_ERROR, details),
    };
    let mut body = json!({ "error": message, "timestamp": iso_timestamp() });
    if let (Some(obj), Some(id)) = (body.as_object_mut(), user_id) {
        obj.insert("userId".to_string(), json!(id));
    }
    json_response(status, body)
}

// ---------------------------------------------------------------------------
// Script host bindings
// ---------------------------------------------------------------------------

/// Failure modes of the script host binding APIs.
#[derive(Debug)]
pub enum ScriptHostError {
    AccessDenied,
    ScriptNotFound(String),
    Validation(String),
    Storage(String),
}

fn script_host_error_response(uri: Option<&str>, error: ScriptHostError) -> Response {
    let (status, message) = match error {
        ScriptHostError::AccessDenied => (
            StatusCode::FORBIDDEN,
            "Permission denied. Administrator privileges are required to change where a script is published".to_string(),
        ),
        ScriptHostError::ScriptNotFound(uri) => {
            (StatusCode::NOT_FOUND, format!("Script not found: {}", uri))
        }
        ScriptHostError::Validation(details) => (StatusCode::BAD_REQUEST, details),
        ScriptHostError::Storage(details) => (StatusCode::INTERNAL_SERVER_ERROR, details),
    };
    let mut body = json!({ "error": message, "timestamp": iso_timestamp() });
    if let (Some(obj), Some(uri)) = (body.as_object_mut(), uri) {
        obj.insert("uri".to_string(), json!(uri));
    }
    json_response(status, body)
}

/// Check each requested host against the ones this engine serves.
///
/// A binding to a host the engine does not serve would silently take the
/// script's registrations offline, so it is rejected with the served hosts
/// listed rather than stored and left to puzzle over later.
fn validate_hosts(requested: &[String]) -> Result<Vec<String>, ScriptHostError> {
    let served = crate::hosts::all_hosts();
    let mut hosts = Vec::new();

    for entry in requested {
        let host = entry.trim().to_lowercase();
        if host.is_empty() {
            continue;
        }
        if host == crate::hosts::ALL_HOSTS {
            // Stored as-is so the binding keeps following the configured set
            // as hosts are added or removed.
            return Ok(vec![crate::hosts::ALL_HOSTS.to_string()]);
        }
        if !served.contains(&host) {
            return Err(ScriptHostError::Validation(format!(
                "Unknown host '{}'. This engine serves: {}. Use '{}' to publish on all of them.",
                entry,
                if served.is_empty() {
                    "(none configured)".to_string()
                } else {
                    served.join(", ")
                },
                crate::hosts::ALL_HOSTS
            )));
        }
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }

    Ok(hosts)
}

/// Read a script's host bindings, resolved to the hosts it actually serves.
/// Administrators only, matching the write path.
pub fn get_script_hosts_authorized(
    user: &UserContext,
    uri: &str,
) -> Result<(Vec<String>, Vec<String>), ScriptHostError> {
    if !is_user_admin(user) {
        audit_user_admin_denied(user, "get_script_hosts");
        return Err(ScriptHostError::AccessDenied);
    }
    if repository::fetch_script(uri).is_none() {
        return Err(ScriptHostError::ScriptNotFound(uri.to_string()));
    }

    let stored = repository::get_script_hosts(uri)
        .map_err(|e| ScriptHostError::Storage(format!("Failed to read script hosts: {}", e)))?;
    let effective = crate::hosts::effective_hosts(&stored);
    Ok((stored, effective))
}

/// Replace a script's host bindings. Administrators only.
///
/// Where a script's routes, assets, streams, GraphQL operations and MCP tools
/// are published decides which origins can reach them, so this is an
/// administrator's call rather than a script owner's — an owner could
/// otherwise move their own script onto the management host.
pub fn set_script_hosts_authorized(
    user: &UserContext,
    uri: &str,
    requested: &[String],
) -> Result<(Vec<String>, Vec<String>), ScriptHostError> {
    if !is_user_admin(user) {
        audit_user_admin_denied(user, "set_script_hosts");
        return Err(ScriptHostError::AccessDenied);
    }
    if repository::fetch_script(uri).is_none() {
        return Err(ScriptHostError::ScriptNotFound(uri.to_string()));
    }

    let hosts = validate_hosts(requested)?;
    repository::set_script_hosts(uri, &hosts)
        .map_err(|e| ScriptHostError::Storage(format!("Failed to store script hosts: {}", e)))?;

    let effective = crate::hosts::effective_hosts(&hosts);
    info!(
        "Script {} host binding set to {:?} (publishing on {:?})",
        uri, hosts, effective
    );
    Ok((hosts, effective))
}

#[derive(Debug, Default, Deserialize)]
pub struct ScriptHostParams {
    uri: Option<String>,
    /// Comma-separated host list; `*` publishes on every configured host and
    /// an empty value returns the script to the default host.
    hosts: Option<String>,
}

/// Split the `hosts` parameter, which is comma-separated in both the query
/// string and the form body.
fn parse_host_list(hosts: &str) -> Vec<String> {
    hosts
        .split(',')
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
        .collect()
}

fn script_hosts_response(uri: &str, stored: Vec<String>, effective: Vec<String>) -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "uri": uri,
            // What is stored: empty for "default host", ["*"] for all
            "hosts": stored,
            // What that resolves to right now
            "publishedOn": effective,
            "servedHosts": crate::hosts::all_hosts(),
            "defaultHost": crate::hosts::default_host(),
            "timestamp": iso_timestamp(),
        }),
    )
}

/// Read where a script is published. Administrators only.
#[utoipa::path(
    get,
    path = "/engine/script_hosts",
    tags = ["Scripts"],
    params(
        ("uri" = String, Query, description = "URI of the script to inspect"),
    ),
    responses(
        (status = 200, description = "The script's stored host binding and the hosts it resolves to"),
        (status = 403, description = "Administrator privileges required"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn script_hosts_get_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptHostParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };

    let uri_cl = uri.clone();
    let result = tokio::task::spawn_blocking(move || get_script_hosts_authorized(&user, &uri_cl))
        .await
        .unwrap_or_else(|e| Err(ScriptHostError::Storage(format!("join error: {}", e))));

    match result {
        Ok((stored, effective)) => script_hosts_response(&uri, stored, effective),
        Err(error) => script_host_error_response(Some(&uri), error),
    }
}

/// Set where a script is published. Administrators only.
#[utoipa::path(
    post,
    path = "/engine/script_hosts",
    tags = ["Scripts"],
    params(
        ("uri" = String, Query, description = "URI of the script to modify"),
        ("hosts" = String, Query, description = "Comma-separated hosts, '*' for every configured host, or empty for the default host"),
    ),
    responses(
        (status = 200, description = "Binding replaced; returns the resulting hosts"),
        (status = 400, description = "A host this engine does not serve"),
        (status = 403, description = "Administrator privileges required"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn script_hosts_post_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptHostParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form: ScriptHostParams = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(uri) = form.uri.or(query.uri) else {
        return missing_param_response("uri");
    };
    // An explicitly empty value is meaningful: it clears the binding.
    let hosts = parse_host_list(&form.hosts.or(query.hosts).unwrap_or_default());

    let uri_cl = uri.clone();
    let result =
        tokio::task::spawn_blocking(move || set_script_hosts_authorized(&user, &uri_cl, &hosts))
            .await
            .unwrap_or_else(|e| Err(ScriptHostError::Storage(format!("join error: {}", e))));

    match result {
        Ok((stored, effective)) => script_hosts_response(&uri, stored, effective),
        Err(error) => script_host_error_response(Some(&uri), error),
    }
}

/// Clear a script's host binding, returning it to the default host.
/// Administrators only.
#[utoipa::path(
    delete,
    path = "/engine/script_hosts",
    tags = ["Scripts"],
    params(
        ("uri" = String, Query, description = "URI of the script to reset"),
    ),
    responses(
        (status = 200, description = "Binding cleared; the script publishes on the default host"),
        (status = 403, description = "Administrator privileges required"),
        (status = 404, description = "Script not found"),
    )
)]
pub async fn script_hosts_delete_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptHostParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };

    let uri_cl = uri.clone();
    let result =
        tokio::task::spawn_blocking(move || set_script_hosts_authorized(&user, &uri_cl, &[]))
            .await
            .unwrap_or_else(|e| Err(ScriptHostError::Storage(format!("join error: {}", e))));

    match result {
        Ok((stored, effective)) => script_hosts_response(&uri, stored, effective),
        Err(error) => script_host_error_response(Some(&uri), error),
    }
}

/// List all users. Administrators only.
#[utoipa::path(
    get,
    path = "/engine/users",
    tags = ["Users"],
    responses(
        (status = 200, description = "User list with roles and linked providers"),
        (status = 403, description = "Permission denied"),
    )
)]
pub async fn users_get_route(auth_user: Option<Extension<AuthUser>>) -> Response {
    let user = user_context_from(auth_user.as_deref());

    let result = tokio::task::spawn_blocking(move || list_users_authorized(&user))
        .await
        .unwrap_or_else(|e| Err(UserAdminError::Storage(format!("join error: {}", e))));

    match result {
        Ok(users) => json_response(
            StatusCode::OK,
            json!({
                "users": users,
                "count": users.len(),
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(error) => user_admin_error_response(None, error),
    }
}

/// Grant a role to a user. Administrators only.
#[utoipa::path(
    post,
    path = "/engine/user_roles",
    tags = ["Users"],
    request_body(content_type = "application/json",
        description = "Fields (JSON or form-encoded): user_id (required), role (required: Editor, Administrator, or Authenticated)"),
    responses(
        (status = 200, description = "Role granted; returns the resulting role set"),
        (status = 400, description = "Missing parameter or unknown role"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "User not found"),
    )
)]
pub async fn user_roles_post_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<UserRoleParams>,
    body: axum::body::Bytes,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let form = parse_user_role_body(&body);
    let Some(user_id) = form.user_id.or(query.user_id) else {
        return missing_param_response("user_id");
    };
    let Some(role) = form.role.or(query.role) else {
        return missing_param_response("role");
    };

    let (user_id_cl, role_cl) = (user_id.clone(), role.clone());
    let result =
        tokio::task::spawn_blocking(move || add_user_role_authorized(&user, &user_id_cl, &role_cl))
            .await
            .unwrap_or_else(|e| Err(UserAdminError::Storage(format!("join error: {}", e))));

    match result {
        Ok(roles) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "userId": user_id,
                "role": role,
                "roles": roles,
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(error) => user_admin_error_response(Some(&user_id), error),
    }
}

/// Revoke a role from a user. Administrators only.
#[utoipa::path(
    delete,
    path = "/engine/user_roles",
    tags = ["Users"],
    params(
        ("user_id" = String, Query, description = "Id of the user to modify"),
        ("role" = String, Query, description = "Role to revoke: Editor or Administrator"),
    ),
    responses(
        (status = 200, description = "Role revoked; returns the resulting role set"),
        (status = 400, description = "Missing parameter, unknown role, or the Authenticated role"),
        (status = 403, description = "Permission denied"),
        (status = 404, description = "User not found"),
        (status = 409, description = "Cannot remove the last administrator"),
    )
)]
pub async fn user_roles_delete_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<UserRoleParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(user_id) = query.user_id else {
        return missing_param_response("user_id");
    };
    let Some(role) = query.role else {
        return missing_param_response("role");
    };

    let (user_id_cl, role_cl) = (user_id.clone(), role.clone());
    let result = tokio::task::spawn_blocking(move || {
        remove_user_role_authorized(&user, &user_id_cl, &role_cl)
    })
    .await
    .unwrap_or_else(|e| Err(UserAdminError::Storage(format!("join error: {}", e))));

    match result {
        Ok(roles) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "userId": user_id,
                "role": role,
                "roles": roles,
                "timestamp": iso_timestamp(),
            }),
        ),
        Err(error) => user_admin_error_response(Some(&user_id), error),
    }
}

/// Installation confirmation page, shown after a fresh install (the root
/// path redirects here until further routes are registered).
const INSTALLED_PAGE_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>aiwebengine Installed</title>
  <style>
    body {
      margin: 0;
      padding: 0;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      display: flex;
      justify-content: center;
      align-items: center;
      min-height: 100vh;
      background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    }
    .container {
      text-align: center;
      background: white;
      padding: 3rem 4rem;
      border-radius: 1rem;
      box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
    }
    h1 {
      color: #333;
      margin: 0 0 1rem 0;
      font-size: 2.5rem;
    }
    p {
      color: #666;
      font-size: 1.2rem;
      margin: 0;
    }
    .emoji {
      font-size: 4rem;
      margin-bottom: 1rem;
    }
  </style>
</head>
<body>
  <div class="container">
    <div class="emoji">🎉</div>
    <h1>Thanks for installing aiwebengine!</h1>
    <p>Your server is up and running.</p>
  </div>
</body>
</html>"#;

/// Installation confirmation page.
#[utoipa::path(
    get,
    path = "/engine/installed",
    tags = ["Engine"],
    responses(
        (status = 200, description = "Shows a confirmation page for successful installation",
            content_type = "text/html"),
    )
)]
pub async fn installed_page_route() -> Response {
    (
        StatusCode::OK,
        [("content-type", "text/html")],
        INSTALLED_PAGE_HTML,
    )
        .into_response()
}

/// How many rows of lock detail one health check reports.
///
/// A wedged table produces one blocked waiter per stalled request, and they all
/// name the same holder. A handful is enough to identify it; the rest is
/// repetition an operator has to scroll past.
const LOCK_DIAGNOSTIC_LIMIT: i64 = 10;

/// Statements currently waiting on a lock, and the sessions holding them.
///
/// The question this answers used to be unanswerable from inside the engine:
/// a wedged table shows up as requests that never return, and telling that
/// apart from a slow script meant reaching for `psql`. A blocked statement
/// names its own query, and `pg_blocking_pids` names whoever is in front of it,
/// which together identify a lock wedge without leaving the health check.
///
/// Reported alongside the oldest transaction on the server, because the holder
/// of a lock nobody can break is usually a transaction that stopped making
/// progress rather than one doing something slow.
pub async fn lock_diagnostics(pool: &sqlx::PgPool) -> Value {
    use sqlx::Row;

    let waiting = sqlx::query(
        r#"
        SELECT
            a.pid,
            a.state,
            a.wait_event_type,
            EXTRACT(EPOCH FROM (now() - a.xact_start))::float8 AS xact_age_seconds,
            left(a.query, 200) AS query,
            pg_blocking_pids(a.pid) AS blocked_by
        FROM pg_stat_activity a
        WHERE a.datname = current_database()
          AND cardinality(pg_blocking_pids(a.pid)) > 0
        ORDER BY a.xact_start
        LIMIT $1
        "#,
    )
    .bind(LOCK_DIAGNOSTIC_LIMIT)
    .fetch_all(pool)
    .await;

    let waiting = match waiting {
        Ok(rows) => rows
            .into_iter()
            .map(|row| {
                json!({
                    "pid": row.try_get::<i32, _>("pid").unwrap_or_default(),
                    "state": row.try_get::<Option<String>, _>("state").unwrap_or_default(),
                    "wait_event_type": row
                        .try_get::<Option<String>, _>("wait_event_type")
                        .unwrap_or_default(),
                    "transaction_age_seconds": row
                        .try_get::<Option<f64>, _>("xact_age_seconds")
                        .unwrap_or_default(),
                    "query": row.try_get::<Option<String>, _>("query").unwrap_or_default(),
                    "blocked_by": row
                        .try_get::<Vec<i32>, _>("blocked_by")
                        .unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            // A diagnostic that cannot run must not decide whether the engine
            // is healthy; the database ping above already answered that.
            return json!({ "available": false, "message": e.to_string() });
        }
    };

    let oldest = sqlx::query(
        r#"
        SELECT
            pid,
            state,
            EXTRACT(EPOCH FROM (now() - xact_start))::float8 AS xact_age_seconds,
            left(query, 200) AS query
        FROM pg_stat_activity
        WHERE datname = current_database() AND xact_start IS NOT NULL
        ORDER BY xact_start
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|row| {
        json!({
            "pid": row.try_get::<i32, _>("pid").unwrap_or_default(),
            "state": row.try_get::<Option<String>, _>("state").unwrap_or_default(),
            "age_seconds": row
                .try_get::<Option<f64>, _>("xact_age_seconds")
                .unwrap_or_default(),
            "query": row.try_get::<Option<String>, _>("query").unwrap_or_default(),
        })
    });

    json!({
        "available": true,
        "blocked_statements": waiting.len(),
        "waiting": waiting,
        "oldest_transaction": oldest,
    })
}

/// Detailed cluster diagnostics. Administrators only.
///
/// Unlike the unauthenticated `/health` liveness probe, this reports internal
/// topology — connection-pool metrics, notification-listener state, and
/// per-script scheduler job counts — so it lives under the authorized
/// `/engine` prefix rather than being world-readable.
///
/// Like `/health`, it verifies the database with a real `SELECT 1` ping and
/// returns 503 when that fails.
#[utoipa::path(
    get,
    path = "/engine/health/cluster",
    tags = ["Health"],
    responses(
        (status = 200, description = "Detailed cluster health information", body = crate::openapi_schemas::ClusterHealthResponse),
        (status = 403, description = "Permission denied"),
        (status = 503, description = "Cluster is unhealthy (database unreachable)", body = crate::openapi_schemas::ClusterHealthResponse),
    )
)]
pub async fn cluster_health_route(auth_user: Option<Extension<AuthUser>>) -> Response {
    let user = user_context_from(auth_user.as_deref());
    // Deliberately the stricter `is_user_admin` check, not
    // `has_admin_capability`: the latter passes on capability alone, which
    // development mode grants to anonymous callers. Topology diagnostics
    // require a real admin session.
    if !is_user_admin(&user) {
        return error_response(
            StatusCode::FORBIDDEN,
            "Permission denied. You must be an administrator".to_string(),
        );
    }

    let server_id = crate::notifications::get_server_id().unwrap_or_else(|| "unknown".to_string());

    // Verify the database with a real query and report pool stats alongside it.
    let (db_healthy, pool_stats, locks) = if let Some(db) = crate::database::get_global_database() {
        let connected = db.health_check().await.is_ok();
        let pool = db.pool();
        let size = pool.size() as usize;
        let idle = pool.num_idle();
        let locks = if connected {
            lock_diagnostics(pool).await
        } else {
            json!({ "available": false, "message": "Database unreachable" })
        };
        (
            connected,
            json!({
                "available": true,
                "connected": connected,
                "active_connections": size.saturating_sub(idle),
                "idle_connections": idle,
                "max_connections": pool.options().get_max_connections(),
            }),
            locks,
        )
    } else {
        (
            false,
            json!({
                "available": false,
                "connected": false,
                "message": "Database not initialized (memory mode)"
            }),
            json!({ "available": false, "message": "Database not initialized" }),
        )
    };

    // Get notification listener status
    let listener_status = if crate::notifications::get_global_listener().is_some() {
        json!({
            "active": true,
            "server_id": server_id.clone(),
        })
    } else {
        json!({
            "active": false,
            "message": "Notification listener not initialized"
        })
    };

    // Get scheduler job counts per script
    let scheduler = crate::scheduler::get_scheduler();
    let job_counts = scheduler.get_job_counts();
    let total_jobs: usize = job_counts.values().sum();

    // Reported rather than folded into `status`: a lost thread does not make
    // the engine unreachable, and a probe that starts failing would take the
    // instance out of rotation for a condition that often clears itself. It is
    // what to alert on, not what to fail on.
    let census = crate::worker_census::snapshot();

    let status_code = if db_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    json_response(
        status_code,
        json!({
            "status": if db_healthy { "healthy" } else { "unhealthy" },
            "instance_id": server_id,
            "timestamp": iso_timestamp(),
            "version": {
                "cargo": env!("CARGO_PKG_VERSION"),
                "git_commit": option_env!("VERGEN_GIT_SHA").unwrap_or(""),
                "git_commit_timestamp": option_env!("VERGEN_GIT_COMMIT_TIMESTAMP").unwrap_or(""),
                "build_timestamp": option_env!("VERGEN_BUILD_TIMESTAMP").unwrap_or("")
            },
            "database": pool_stats,
            "locks": locks,
            "workers": {
                "abandoned": census.abandoned,
                "recovered": census.recovered,
                "in_flight": census.in_flight,
            },
            "notification_listener": listener_status,
            "scheduler": {
                "total_jobs": total_jobs,
                "jobs_by_script": job_counts,
            }
        }),
    )
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Deserialize, Default)]
pub struct UnauthorizedQuery {
    attempted: Option<String>,
}

/// Insufficient permissions page, shown when an authenticated user lacks the
/// role required for the page they attempted to access (formerly auth.js).
#[utoipa::path(
    get,
    path = "/auth/unauthorized",
    tags = ["Authentication"],
    params(("attempted" = Option<String>, Query, description = "Path the user attempted to access")),
    responses(
        (status = 403, description = "Insufficient permissions page", content_type = "text/html"),
    )
)]
pub async fn unauthorized_page_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<UnauthorizedQuery>,
) -> Response {
    let auth_user = auth_user.as_deref();
    let attempted = query.attempted.as_deref();

    let user_info_block = match auth_user {
        Some(user) => {
            let user_name = user
                .name
                .as_deref()
                .or(user.email.as_deref())
                .unwrap_or("User");
            let email_suffix = user
                .email
                .as_deref()
                .map(|email| format!(" ({})", html_escape(email)))
                .unwrap_or_default();
            format!(
                r#"
            <div class="user-info">
                <strong>Signed in as:</strong> {}{}
            </div>
            "#,
                html_escape(user_name),
                email_suffix
            )
        }
        None => String::new(),
    };

    let attempted_path_block = match attempted {
        Some(path) => format!(
            r#"
            <div class="attempted-path">
                <strong>Attempted to access:</strong> {}
            </div>
            "#,
            html_escape(path)
        ),
        None => String::new(),
    };

    let action_link = match auth_user {
        Some(_) => r#"<a href="/auth/logout">Sign Out</a>"#.to_string(),
        None => {
            let redirect_suffix = attempted
                .map(|path| format!("?redirect={}", urlencoding::encode(path)))
                .unwrap_or_default();
            format!(r#"<a href="/auth/login{}">Sign In</a>"#, redirect_suffix)
        }
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Insufficient Permissions - aiwebengine</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style>
        /* Self-contained: this page is shown in error situations and must
           not depend on any other resource being served correctly. */
        :root {{
            --primary-color: #007acc;
            --bg-primary: #ffffff;
            --bg-secondary: #f8f9fa;
            --text-color: #212529;
            --text-muted: #6c757d;
            --border-color: #dee2e6;
            --border-radius: 6px;
            --border-radius-lg: 8px;
            --shadow: 0 1px 3px rgba(0, 0, 0, 0.1), 0 1px 2px rgba(0, 0, 0, 0.06);
            --shadow-lg: 0 10px 15px rgba(0, 0, 0, 0.1), 0 4px 6px rgba(0, 0, 0, 0.05);
            --transition: all 0.2s ease;
            --info-bg: #e8f4fd;
            --info-border: #b6def7;
            --info-color: #0c5464;
            --error-bg: #f8d7da;
            --error-color: #dc3545;
        }}

        body {{
            margin: 0;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            font-size: 14px;
            line-height: 1.5;
        }}

        body {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 2rem 0;
        }}

        .permissions-container {{
            max-width: 600px;
            margin: 0 auto;
            background: rgba(255, 255, 255, 0.95);
            backdrop-filter: blur(10px);
            border-radius: var(--border-radius-lg);
            box-shadow: var(--shadow-lg);
            overflow: hidden;
        }}

        .permissions-content {{
            padding: 3rem 2rem;
            text-align: center;
        }}

        .permissions-icon {{
            width: 80px;
            height: 80px;
            margin: 0 auto 1.5rem;
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            border-radius: 50%;
            display: flex;
            align-items: center;
            justify-content: center;
            font-size: 40px;
            color: white;
        }}

        .permissions-content h1 {{
            color: var(--text-color);
            margin-bottom: 1rem;
            font-size: 2rem;
        }}

        .permissions-subtitle {{
            color: var(--text-muted);
            margin-bottom: 2rem;
            font-size: 1.1rem;
            line-height: 1.6;
        }}

        .info-box {{
            background: var(--bg-secondary);
            border-left: 4px solid var(--primary-color);
            border-radius: var(--border-radius);
            padding: 1.5rem;
            margin-bottom: 2rem;
            text-align: left;
        }}

        .info-box p {{
            color: var(--text-muted);
            line-height: 1.6;
            margin-bottom: 0.75rem;
        }}

        .info-box p:last-child {{
            margin-bottom: 0;
        }}

        .info-box strong {{
            color: var(--text-color);
        }}

        .user-info {{
            background: var(--info-bg);
            border: 1px solid var(--info-border);
            border-radius: var(--border-radius);
            padding: 1rem;
            margin-bottom: 1.5rem;
            font-size: 0.9rem;
            color: var(--info-color);
        }}

        .user-info strong {{
            color: var(--text-color);
        }}

        .attempted-path {{
            background: var(--error-bg);
            border-left: 4px solid var(--error-color);
            border-radius: var(--border-radius);
            padding: 1rem;
            margin-bottom: 1.5rem;
            text-align: left;
            font-size: 0.9rem;
            color: var(--error-color);
            word-break: break-all;
        }}

        .permissions-actions {{
            display: flex;
            gap: 1rem;
            justify-content: center;
            flex-wrap: wrap;
            margin-bottom: 2rem;
        }}

        .permissions-actions a {{
            padding: 0.75rem 1.5rem;
            border-radius: var(--border-radius);
            text-decoration: none;
            font-weight: 600;
            font-size: 1rem;
            transition: var(--transition);
            display: inline-block;
            text-align: center;
        }}

        .permissions-actions a:first-child {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
        }}

        .permissions-actions a:first-child:hover {{
            transform: translateY(-2px);
            box-shadow: var(--shadow);
        }}

        .permissions-actions a:last-child {{
            background: var(--bg-secondary);
            color: var(--text-muted);
            border: 1px solid var(--border-color);
        }}

        .permissions-actions a:last-child:hover {{
            background: var(--bg-primary);
        }}

        .contact-info {{
            margin-top: 2rem;
            padding-top: 1.5rem;
            border-top: 1px solid var(--border-color);
            font-size: 0.9rem;
            color: var(--text-muted);
        }}

        @media (max-width: 768px) {{
            .permissions-content {{
                padding: 2rem 1rem;
            }}

            .permissions-content h1 {{
                font-size: 1.75rem;
            }}

            .permissions-actions {{
                flex-direction: column;
            }}

            .permissions-actions a {{
                width: 100%;
            }}
        }}
    </style>
</head>
<body>
    <div class="permissions-container">
        <div class="permissions-content">
            <div class="permissions-icon">
                🔒
            </div>

            <h1>Insufficient Permissions</h1>

            <p class="permissions-subtitle">
                You don't have the required permissions to access this resource.
            </p>
            {user_info_block}{attempted_path_block}
            <div class="info-box">
                <p><strong>Why am I seeing this?</strong></p>
                <p>This page or feature requires <strong>Editor</strong> or <strong>Administrator</strong> privileges. Your current account does not have these permissions.</p>
                <p><strong>What can I do?</strong></p>
                <p>• Contact your system administrator to request the appropriate role</p>
                <p>• Verify you're signed in with the correct account</p>
                <p>• Return to the home page to access features available to you</p>
            </div>

            <div class="permissions-actions">
                <a href="/">Go to Home</a>
                {action_link}
            </div>

            <div class="contact-info">
                If you believe this is an error, please contact your system administrator.
            </div>
        </div>
    </div>
</body>
</html>"#
    );

    (
        StatusCode::FORBIDDEN,
        [("content-type", "text/html; charset=UTF-8")],
        html,
    )
        .into_response()
}

/// Site favicon, served from the engine's bootstrapped assets.
#[utoipa::path(
    get,
    path = "/favicon.ico",
    tags = ["Assets"],
    responses(
        (status = 200, description = "Favicon", content_type = "image/x-icon"),
        (status = 404, description = "Favicon not found"),
    )
)]
pub async fn favicon_route() -> Response {
    match repository::fetch_asset_async("https://example.com/core", "favicon.ico").await {
        Some(asset) => (
            StatusCode::OK,
            [
                ("content-type", asset.mimetype),
                ("cache-control", "public, max-age=3600".to_string()),
            ],
            asset.content,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "Favicon not found").into_response(),
    }
}

/// OpenAPI specification for all registered routes.
#[utoipa::path(
    get,
    path = "/engine/openapi.json",
    tags = ["Engine"],
    responses(
        (status = 200, description = "OpenAPI 3.0 specification for all registered routes"),
        (status = 403, description = "Insufficient permissions"),
    )
)]
pub async fn openapi_route(auth_user: Option<Extension<AuthUser>>) -> Response {
    let user = user_context_from(auth_user.as_deref());
    if user.require_capability(&Capability::ReadScripts).is_err() {
        return json_response(
            StatusCode::FORBIDDEN,
            json!({ "error": "Insufficient permissions" }),
        );
    }

    let spec = tokio::task::spawn_blocking(generate_merged_openapi_spec)
        .await
        .unwrap_or_else(|e| json!({ "error": format!("join error: {}", e) }).to_string());

    (StatusCode::OK, [("content-type", "application/json")], spec).into_response()
}

// ============================================================================
// Native MCP tools (names and result shapes identical to the script tools)
// ============================================================================

/// Descriptor of a native MCP tool for tools/list.
pub struct NativeToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

type NativeToolHandler = fn(&Value, &UserContext) -> Value;
type NativeToolEntry = (&'static str, &'static str, fn() -> Value, NativeToolHandler);

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn missing_arg(name: &str) -> Value {
    json!({ "error": format!("Missing required parameter: {}", name) })
}

fn native_tools() -> &'static [NativeToolEntry] {
    &[
        (
            "read_file",
            "Fetch the contents of a remote file (script) by URI",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI (e.g., 'https://example.com/myscript')" }
                    },
                    "required": ["uri"]
                })
            },
            tool_read_file,
        ),
        (
            "write_file",
            "Create or update a file (script) on the server",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI" },
                        "content": { "type": "string", "description": "File content (JavaScript code)" }
                    },
                    "required": ["uri", "content"]
                })
            },
            tool_write_file,
        ),
        (
            "create_file",
            "Create a new file (script) on the server. Fails if file already exists.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI" },
                        "content": { "type": "string", "description": "File content (JavaScript code)", "default": "" }
                    },
                    "required": ["uri"]
                })
            },
            tool_create_file,
        ),
        (
            "list_files",
            "List all files (scripts) in the system, optionally filtered by pattern",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Optional regex pattern to filter files by URI" }
                    }
                })
            },
            tool_list_files,
        ),
        (
            "delete_file",
            "Remove a file (script) from the server",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI to delete" }
                    },
                    "required": ["uri"]
                })
            },
            tool_delete_file,
        ),
        (
            "search_files",
            "Perform text search across all files (grep-like functionality)",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text or regex pattern to search for" },
                        "caseInsensitive": { "type": "boolean", "description": "Whether search should be case-insensitive", "default": true }
                    },
                    "required": ["query"]
                })
            },
            tool_search_files,
        ),
        (
            "read_logs",
            "Read log messages (useful for debugging). Returns logs for one script when uri is given, otherwise across all scripts. Each entry carries the invocation that emitted it (requestId, kind, route), so the lines from one request, scheduler tick or stream connection can be read on their own.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Optional script URI to retrieve logs for; omit for all scripts" },
                        "level": { "type": "string", "description": "Only entries at this level, e.g. ERROR" },
                        "since": { "type": "string", "description": "Only entries at or after this time (epoch millis or RFC 3339)" },
                        "after_seq": { "type": "integer", "description": "Only entries written after this seq; pass the highest seq from a previous read to see only what is new" },
                        "contains": { "type": "string", "description": "Only entries whose message contains this substring" },
                        "request_id": { "type": "string", "description": "Only the entries one invocation emitted, by x-request-id or a non-HTTP invocation's id" },
                        "kind": { "type": "string", "description": "Only entries from invocations of this kind: httpRoute, graphqlQuery, graphqlMutation, graphqlSubscription, scheduled, streamCustomization, mcpTool, mcpPrompt, init, eval, test" },
                        "route": { "type": "string", "description": "Only entries logged while serving this registered route pattern, e.g. /things/:id" },
                        "limit": { "type": "integer", "description": "Keep at most this many of the newest matching entries" }
                    }
                })
            },
            tool_read_logs,
        ),
        (
            "prune_logs",
            "Delete log messages. Clears one script's logs when uri is given, otherwise prunes every script back to its 20 newest entries.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Optional script URI whose logs to clear; omit to prune every script" }
                    }
                })
            },
            tool_prune_logs,
        ),
        (
            "list_routes",
            "List every registration in the engine: script HTTP routes, SSE streams (method STREAM) and asset routes (method ASSET).",
            || {
                json!({
                    "type": "object",
                    "properties": {}
                })
            },
            tool_list_routes,
        ),
        (
            "read_init_status",
            "Read init() status for scripts (useful for debugging). Returns status for one script when uri is given, otherwise for all scripts.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Optional script URI to retrieve init status for; omit to list all scripts" }
                    }
                })
            },
            tool_read_init_status,
        ),
        (
            "list_assets",
            "List all assets owned by a script. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose assets to list (e.g., 'https://example.com/myscript')" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_assets,
        ),
        (
            "read_asset",
            "Fetch an asset owned by a script: the whole file base64-encoded, or, with 'lines' or 'grep', part of a text asset. Every reply carries the sha256 of the whole asset, which edit_asset takes as base_sha256. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset to fetch (e.g., '/images/logo.png')" },
                        "lines": { "type": "string", "description": "Inclusive 1-based line range of a text asset, e.g. '120-180', '120-' to the end, or '120' alone. Answers with text rather than base64." },
                        "grep": { "type": "string", "description": "Regular expression; answers with the matching line numbers and their text instead of the file. Searches within 'lines' when both are given." }
                    },
                    "required": ["script", "asset"]
                })
            },
            tool_read_asset,
        ),
        (
            "write_asset",
            "Create or update an asset for a script. Requires the user to own the script, have WriteAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that will own the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset (e.g., '/images/logo.png')" },
                        "mimetype": { "type": "string", "description": "MIME type of the asset (e.g., 'image/png', 'text/css')" },
                        "content": { "type": "string", "description": "Base64-encoded content of the asset (max 10MB)" }
                    },
                    "required": ["script", "asset", "mimetype", "content"]
                })
            },
            tool_write_asset,
        ),
        (
            "write_assets",
            "Create or update several of a script's assets in one atomic write, then run the script's init() once. Nothing is written if any file is rejected. Requires the user to own the script, have WriteAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that will own these assets" },
                        "files": {
                            "type": "array",
                            "description": "Files to write (max 256, 10MB of content in total)",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "URI/path of the asset (e.g., '/lib/util.ts')" },
                                    "content_base64": { "type": "string", "description": "Base64-encoded content of the asset (max 10MB)" },
                                    "mimetype": { "type": "string", "description": "MIME type; inferred from the file extension when omitted" },
                                    "sha256": { "type": "string", "description": "Expected SHA-256 of the decoded content, lowercase hex. The batch is rejected if it does not match." }
                                },
                                "required": ["name", "content_base64"]
                            }
                        },
                        "reinit": { "type": "string", "enum": ["after", "never"], "description": "Run the script's init() once after the batch lands (default 'after'), or leave it alone" }
                    },
                    "required": ["script", "files"]
                })
            },
            tool_write_assets,
        ),
        (
            "edit_asset",
            "Edit one of a script's assets in place by replacing strings in it, without resending the file, then run the script's init() once. Each old_string must be present and unique unless replace_all is set; nothing is written unless every edit applies. Requires the user to own the script, have WriteAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset to edit (e.g., '/lib/util.ts')" },
                        "edits": {
                            "type": "array",
                            "description": "Edits to apply in order (max 128). Each is checked and applied in memory before anything is stored.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old_string": { "type": "string", "description": "Text to find. Must appear exactly once unless replace_all is set." },
                                    "new_string": { "type": "string", "description": "Text to put in its place; empty string deletes." },
                                    "replace_all": { "type": "boolean", "description": "Replace every occurrence rather than requiring exactly one (default false)" }
                                },
                                "required": ["old_string", "new_string"]
                            }
                        },
                        "base_sha256": { "type": "string", "description": "SHA-256 the asset is expected to have right now, as read_asset reported it. The patch is refused if the stored content has moved on." },
                        "reinit": { "type": "string", "enum": ["after", "never"], "description": "Run the script's init() once the edits land (default 'after'), or leave it alone" }
                    },
                    "required": ["script", "asset", "edits"]
                })
            },
            tool_edit_asset,
        ),
        (
            "delete_asset",
            "Delete an asset from a script. Requires the user to own the script, have DeleteAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset to delete (e.g., '/images/logo.png')" }
                    },
                    "required": ["script", "asset"]
                })
            },
            tool_delete_asset,
        ),
        (
            "list_script_owners",
            "List the owners of a script",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI" }
                    },
                    "required": ["uri"]
                })
            },
            tool_list_script_owners,
        ),
        (
            "add_script_owner",
            "Add an owner to a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI" },
                        "owner": { "type": "string", "description": "User id to add as owner" }
                    },
                    "required": ["uri", "owner"]
                })
            },
            tool_add_script_owner,
        ),
        (
            "remove_script_owner",
            "Remove an owner from a script. Requires the user to own the script or be an administrator; non-admins cannot remove the last owner.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI" },
                        "owner": { "type": "string", "description": "Owner user id to remove" }
                    },
                    "required": ["uri", "owner"]
                })
            },
            tool_remove_script_owner,
        ),
        (
            "list_secrets",
            "List the secret keys stored for a script (values are never returned). Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_secrets,
        ),
        (
            "write_secret",
            "Store a secret for a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" },
                        "key": { "type": "string", "description": "Secret key" },
                        "value": { "type": "string", "description": "Secret value (max 1MB)" }
                    },
                    "required": ["script", "key", "value"]
                })
            },
            tool_write_secret,
        ),
        (
            "delete_secret",
            "Remove one secret from a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" },
                        "key": { "type": "string", "description": "Secret key to remove" }
                    },
                    "required": ["script", "key"]
                })
            },
            tool_delete_secret,
        ),
        (
            "clear_secrets",
            "Remove all secrets stored for a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" }
                    },
                    "required": ["script"]
                })
            },
            tool_clear_secrets,
        ),
        (
            "list_users",
            "List all users with their roles and linked identity providers. Administrator privileges required.",
            || {
                json!({
                    "type": "object",
                    "properties": {}
                })
            },
            tool_list_users,
        ),
        (
            "add_user_role",
            "Grant a role to a user. Administrator privileges required.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "user_id": { "type": "string", "description": "Id of the user to modify" },
                        "role": {
                            "type": "string",
                            "description": "Role to grant",
                            "enum": ["Editor", "Administrator", "Authenticated"]
                        }
                    },
                    "required": ["user_id", "role"]
                })
            },
            tool_add_user_role,
        ),
        (
            "remove_user_role",
            "Revoke a role from a user. Administrator privileges required. The Authenticated role and the last remaining Administrator cannot be removed.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "user_id": { "type": "string", "description": "Id of the user to modify" },
                        "role": {
                            "type": "string",
                            "description": "Role to revoke",
                            "enum": ["Editor", "Administrator"]
                        }
                    },
                    "required": ["user_id", "role"]
                })
            },
            tool_remove_user_role,
        ),
        (
            "get_script_hosts",
            "Read which hostnames a script's registrations are published on. Administrator privileges required.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "URI of the script to inspect" }
                    },
                    "required": ["uri"]
                })
            },
            tool_get_script_hosts,
        ),
        (
            "set_script_hosts",
            "Set which hostnames a script's registrations are published on. Administrator privileges required. Pass '*' to publish on every configured host, or an empty list to return the script to the default host.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "URI of the script to modify" },
                        "hosts": {
                            "type": "array",
                            "description": "Hostnames to publish on. ['*'] means every configured host; [] returns the script to the default host.",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["uri", "hosts"]
                })
            },
            tool_set_script_hosts,
        ),
        (
            "run_tests",
            "Run a script's test modules and report a verdict per case. Tests are the script's own assets named '*.test.ts' (or .js/.jsx/.tsx). Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "URI of the script whose tests to run" },
                        "filter": { "type": "string", "description": "Run only cases whose name contains this text" },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes the tests make (default true)",
                            "default": true
                        },
                        "revision": {
                            "type": "string",
                            "description": "Which version to use: a revision number, 'head', 'last-good', or a label. Omit for what is deployed."
                        }
                    },
                    "required": ["uri"]
                })
            },
            tool_run_tests,
        ),
        (
            "check_script",
            "Check what a script would do if it were deployed, without deploying it: resolve its \
            asset-backed imports the way the engine does, run its init() with every registration \
            withheld and database writes rolled back, and report diagnostics as {file, line, \
            severity, code, message}. Catches what a local tsc cannot — import cycles the \
            bundler rejects, registrations whose handler name is not defined as a global, an \
            init() close to its deploy budget, and paths another script already claims. Pass \
            'content' to check code before writing it. Requires the user to own the script or be \
            an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "URI of the script to check" },
                        "content": {
                            "type": "string",
                            "description": "Candidate source to check instead of what is deployed. Use this to check code before writing it."
                        },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes init() makes (default true)",
                            "default": true
                        },
                        "timeoutMs": {
                            "type": "integer",
                            "description": "Ceiling for the init() run. Defaults to several times the deploy budget so a slow init() is measured rather than interrupted; raise it for one slower still."
                        },
                        "revision": {
                            "type": "string",
                            "description": "Which version to check: a revision number, 'head', 'last-good', or a label. Omit for what is deployed. Combined with 'content', the candidate root is checked against that revision's modules."
                        }
                    },
                    "required": ["uri"]
                })
            },
            tool_check_script,
        ),
        (
            "eval_script",
            "Evaluate a JavaScript snippet against a deployed script's sandbox and return its \
            value plus everything it logged. The script's own program is loaded first, so the \
            snippet can call its functions and use the bindings its entrypoint imported. It can \
            also import any module the entrypoint reaches, directly or through another module - \
            `import { x } from \"./server/m.ts\"; x()` - or reach one with require(path). Use \
            this to inspect data or try an expression without authoring, deploying and deleting \
            a throwaway test. Database writes roll back by default; registrations do nothing. \
            Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "URI of the script whose sandbox to evaluate in" },
                        "source": {
                            "type": "string",
                            "description": "The snippet. Its last expression is the returned value; scripts run synchronously, so do not use async/await."
                        },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes the snippet makes (default true)",
                            "default": true
                        },
                        "timeoutMs": {
                            "type": "integer",
                            "description": "Budget for the evaluation, clamped to the engine's execution timeout"
                        }
                    },
                    "required": ["uri", "source"]
                })
            },
            tool_eval_script,
        ),
    ]
}

/// Ceiling for a native tool that does nothing but read or write the
/// repository.
///
/// Generous, because it is a backstop rather than a budget: these tools are
/// database round trips that finish in milliseconds, and the only way to reach
/// this is a connection that will never answer.
const NATIVE_TOOL_CEILING_MS: u64 = 30_000;

/// The longest a native tool can legitimately run, for the MCP dispatcher's
/// backstop.
///
/// Returns `None` for a name that is not a native tool, so the dispatcher falls
/// back to the JavaScript execution budget that bounds a script-registered one.
/// Each tool that enforces its own ceiling reports that ceiling here, so the
/// backstop never cuts short a call the tool would have completed — it only
/// fires once a tool is past every limit it sets for itself, which means it is
/// blocked somewhere no interrupt can reach.
pub fn native_tool_ceiling_ms(tool_name: &str) -> Option<u64> {
    match tool_name {
        "run_tests" => Some(crate::script_test::configured_test_timeouts().1),
        "check_script" => Some(crate::script_check::MAX_CHECK_TIMEOUT_MS),
        "eval_script" => Some(crate::script_eval::default_eval_timeout_ms()),
        name if is_native_mcp_tool(name) => Some(NATIVE_TOOL_CEILING_MS),
        _ => None,
    }
}

/// Whether `name` is one of the engine's own MCP tools.
///
/// Native tools take precedence over script-registered ones at dispatch
/// ([`crate::mcp::execute_mcp_tool`]), so anything deciding whether a call is
/// allowed has to ask this before consulting the script registry — otherwise a
/// script registering a colliding name would answer for the native tool.
pub fn is_native_mcp_tool(name: &str) -> bool {
    native_tools().iter().any(|(tool, _, _, _)| *tool == name)
}

/// Descriptors of all native MCP tools, for tools/list.
pub fn native_mcp_tool_descriptors() -> Vec<NativeToolDescriptor> {
    native_tools()
        .iter()
        .map(|(name, description, schema, _)| NativeToolDescriptor {
            name,
            description,
            input_schema: schema(),
        })
        .collect()
}

/// Execute a native MCP tool. Returns None when no native tool has this name
/// (the caller then falls back to script-registered tools).
pub fn execute_native_mcp_tool(
    tool_name: &str,
    arguments: &Value,
    user_context: &UserContext,
) -> Option<Value> {
    let handler = native_tools()
        .iter()
        .find(|(name, _, _, _)| *name == tool_name)
        .map(|(_, _, _, handler)| *handler)?;
    Some(handler(arguments, user_context))
}

/// Evaluate a snippet and return the same report the REST endpoint serves.
///
/// Runs on the blocking pool, like every native tool — which is also what the
/// isolating transaction needs, being thread-local.
fn tool_eval_script(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let Some(source) = arg_str(args, "source").filter(|source| !source.trim().is_empty()) else {
        return missing_arg("source");
    };

    match authorize_eval(user, uri) {
        Ok(()) => {}
        Err(CheckRefusal::NotFound) => {
            return json!({ "error": format!("Script not found: {}", uri) });
        }
        Err(CheckRefusal::AccessDenied) => {
            return json!({
                "error": format!(
                    "Permission denied. You must be an administrator or owner to evaluate against script '{}'",
                    uri
                )
            });
        }
    }

    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let timeout_ms = args.get("timeoutMs").and_then(Value::as_u64);

    let report = crate::script_eval::eval_blocking(crate::script_eval::EvalRequest {
        script_uri: uri.to_string(),
        source: source.to_string(),
        user_context: user.clone(),
        timeout_ms,
        rollback,
    });

    let mut body = report.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }
    body
}

/// Check a script and return the same report the REST endpoint serves.
///
/// Runs on the blocking pool, like every native tool: a check evaluates the
/// script's program and calls its `init()` under the deploy budget, and the
/// transaction that isolates it is bound to the thread that opens it.
fn tool_check_script(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let content = arg_str(args, "content").map(str::to_string);

    match authorize_check(user, uri, content.is_some()) {
        Ok(()) => {}
        Err(CheckRefusal::NotFound) => {
            return json!({
                "error": format!("Script not found: {}", uri),
                "message": "Pass 'content' to check a script that is not deployed yet",
            });
        }
        Err(CheckRefusal::AccessDenied) => {
            return json!({
                "error": format!(
                    "Permission denied. You must be an administrator or owner to check script '{}'",
                    uri
                )
            });
        }
    }

    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let view = match crate::database::run_blocking(resolve_view(uri, arg_str(args, "revision"))) {
        Ok(view) => view,
        Err(message) => return json!({ "error": message }),
    };

    // Through the async runner rather than straight to `check_blocking`, so a
    // call over MCP gets the same answer one over HTTP does when `init()` will
    // not stop: the registrations collected before it stalled, rather than only
    // the dispatcher's report that nothing came back. That is the half of the
    // answer worth having — it says how far `init()` got.
    //
    // Bridging back to async from this blocking thread costs a second one while
    // the check runs, and the dispatcher's own backstop bounds how long that
    // can last.
    let report = crate::database::run_blocking(
        crate::script_check::ScriptChecker::with_configured_timeout().run(
            crate::script_check::CheckRequest {
                script_uri: uri.to_string(),
                content,
                rollback,
                timeout_ms: args.get("timeoutMs").and_then(Value::as_u64),
                view,
            },
        ),
    );

    let mut body = report.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }
    body
}

/// Run a script's tests and return the same report the REST endpoint serves.
///
/// This runs on the blocking pool: the MCP dispatcher moves tool execution
/// there, because a run is JavaScript executed to completion under a budget
/// measured in seconds. That is also why the whole-run ceiling is enforced
/// inside the run loop rather than by an outer timeout — there is no async
/// backstop on this path, and the in-loop ceiling is the one that can still
/// report the modules that finished.
fn tool_run_tests(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };

    match authorize_test_run(user, uri) {
        Ok(()) => {}
        Err(TestRunRefusal::NotFound) => {
            return json!({ "error": format!("Script not found: {}", uri) });
        }
        Err(TestRunRefusal::AccessDenied) => {
            return json!({
                "error": format!(
                    "Permission denied. You must be an administrator or owner to run tests for script '{}'",
                    uri
                )
            });
        }
    }

    let filter = arg_str(args, "filter").map(str::to_string);
    // Isolation is the default here as it is over HTTP: a test that writes
    // should not leave rows behind unless the caller says so.
    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let view = match crate::database::run_blocking(resolve_view(uri, arg_str(args, "revision"))) {
        Ok(view) => view,
        Err(message) => return json!({ "error": message }),
    };

    let (timeout_ms, run_timeout_ms) = crate::script_test::configured_test_timeouts();
    let modules = crate::module_loader::discover_test_modules_in(uri, &view);
    let result = crate::js_engine::execute_test_run(
        &crate::js_engine::TestRunParams {
            script_uri: uri.to_string(),
            user_context: user.clone(),
            timeout_ms,
            run_timeout_ms,
            filter,
            rollback,
            view,
        },
        &modules,
    );

    let mut report = result.to_json();
    if let Some(object) = report.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
        if result.is_empty() && result.error().is_none() {
            object.insert(
                "message".to_string(),
                json!(
                    "No test modules found. Tests are assets named '*.test.ts' (or .js/.jsx/.tsx)."
                ),
            );
        }
    }
    report
}

fn tool_read_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    match get_script_authorized(user, uri) {
        Some(content) => json!({
            "uri": uri,
            "content": content,
            "size": content.len(),
            "timestamp": iso_timestamp(),
        }),
        None => json!({ "error": format!("File not found: {}", uri) }),
    }
}

fn tool_write_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let Some(content) = arg_str(args, "content") else {
        return missing_arg("content");
    };
    match upsert_script_authorized(user, uri, content, Some("mcp")) {
        Ok((action, revision)) => {
            let action = match action {
                UpsertAction::Inserted => "created",
                UpsertAction::Updated => "updated",
            };
            json!({
                "success": true,
                "action": action,
                "uri": uri,
                "size": content.len(),
                "revision": revision,
                "timestamp": iso_timestamp(),
            })
        }
        Err(e) => json!({ "error": format!("Failed to write file: {}", e) }),
    }
}

fn tool_create_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let content = arg_str(args, "content").unwrap_or("");

    if get_script_authorized(user, uri).is_some() {
        return json!({ "error": format!("File already exists: {}", uri) });
    }
    match upsert_script_authorized(user, uri, content, Some("mcp")) {
        Ok(_) => json!({
            "success": true,
            "uri": uri,
            "size": content.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => json!({ "error": format!("Failed to create file: {}", e) }),
    }
}

fn tool_list_files(args: &Value, user: &UserContext) -> Value {
    let pattern = arg_str(args, "pattern");
    let regex = match pattern {
        Some(p) => match regex::RegexBuilder::new(p).case_insensitive(true).build() {
            Ok(r) => Some(r),
            Err(e) => return json!({ "error": format!("Failed to list files: {}", e) }),
        },
        None => None,
    };

    let files: Vec<Value> = list_scripts_authorized(user)
        .iter()
        .filter(|meta| regex.as_ref().is_none_or(|r| r.is_match(&meta.uri)))
        .map(|meta| {
            json!({
                "uri": meta.uri,
                "size": meta.content.len(),
                "type": "script",
            })
        })
        .collect();

    json!({
        "files": files,
        "count": files.len(),
        "pattern": pattern,
        "timestamp": iso_timestamp(),
    })
}

fn tool_delete_file(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    if delete_script_authorized(user, uri, Some("mcp")) {
        json!({
            "success": true,
            "uri": uri,
            "timestamp": iso_timestamp(),
        })
    } else {
        json!({ "error": format!("File not found: {}", uri) })
    }
}

fn tool_search_files(args: &Value, user: &UserContext) -> Value {
    let Some(query) = arg_str(args, "query") else {
        return missing_arg("query");
    };
    let case_insensitive = args
        .get("caseInsensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let regex = match regex::RegexBuilder::new(query)
        .case_insensitive(case_insensitive)
        .build()
    {
        Ok(r) => r,
        Err(e) => return json!({ "error": format!("Failed to search files: {}", e) }),
    };

    let mut results: Vec<Value> = Vec::new();
    for meta in list_scripts_authorized(user) {
        let matches: Vec<Value> = meta
            .content
            .lines()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
            .take(50)
            .map(|(i, line)| {
                json!({
                    "line": i + 1,
                    "content": line.trim(),
                    "preview": line.chars().take(200).collect::<String>(),
                })
            })
            .collect();

        if !matches.is_empty() {
            results.push(json!({
                "uri": meta.uri,
                "matchCount": matches.len(),
                "matches": matches,
            }));
        }
    }

    json!({
        "query": query,
        "caseInsensitive": case_insensitive,
        "filesMatched": results.len(),
        "results": results,
        "timestamp": iso_timestamp(),
    })
}

fn tool_read_logs(args: &Value, user: &UserContext) -> Value {
    let uri = arg_str(args, "uri");
    let since = match arg_str(args, "since") {
        Some(raw) => match parse_since(raw) {
            Some(since) => Some(since),
            None => return json!({ "error": format!("Invalid 'since' value: {}", raw) }),
        },
        None => None,
    };
    let limit = args.get("limit").and_then(Value::as_i64);
    if limit.is_some_and(|limit| limit <= 0) {
        return json!({ "error": "Parameter 'limit' must be greater than zero" });
    }

    let query = repository::LogQuery {
        script_uri: uri.map(str::to_string),
        level: arg_str(args, "level").map(str::to_string),
        since,
        after_seq: args.get("after_seq").and_then(Value::as_i64),
        contains: arg_str(args, "contains").map(str::to_string),
        request_id: arg_str(args, "request_id").map(str::to_string),
        kind: arg_str(args, "kind").map(str::to_string),
        route: arg_str(args, "route").map(str::to_string),
        limit,
    };

    match query_logs_authorized(user, &query) {
        Ok(mut logs) => {
            // Oldest-first for a single script, as its own log view reads.
            if uri.is_some() {
                logs.reverse();
            }
            json!({
                "uri": uri,
                "logs": logs,
                "count": logs.len(),
                "timestamp": iso_timestamp(),
            })
        }
        Err(e) => json!({ "error": format!("Failed to fetch logs: {}", e) }),
    }
}

fn tool_prune_logs(args: &Value, user: &UserContext) -> Value {
    match delete_logs_authorized(user, arg_str(args, "uri")) {
        Ok(body) => body,
        Err(e) => json!({ "error": format!("Failed to delete logs: {}", e) }),
    }
}

fn tool_list_routes(_args: &Value, user: &UserContext) -> Value {
    match routes_introspection_authorized(user) {
        Ok(routes) => json!({
            "routes": routes,
            "count": routes.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => json!({ "error": format!("Failed to list routes: {}", e) }),
    }
}

fn tool_read_init_status(args: &Value, user: &UserContext) -> Value {
    match arg_str(args, "uri") {
        Some(uri) => json!({
            "uri": uri,
            "status": init_status_authorized(user, uri),
            "timestamp": iso_timestamp(),
        }),
        None => {
            let statuses: Vec<Value> = list_scripts_authorized(user)
                .iter()
                .map(script_init_status_json)
                .collect();
            json!({
                "statuses": statuses,
                "count": statuses.len(),
                "timestamp": iso_timestamp(),
            })
        }
    }
}

fn tool_list_assets(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let assets = list_assets_authorized(user, script);
    json!({
        "script": script,
        "assets": assets,
        "count": assets.len(),
        "timestamp": iso_timestamp(),
    })
}

fn tool_read_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    let options = match read_options(
        arg_str(args, "lines"),
        arg_str(args, "grep").map(str::to_string),
    ) {
        Ok(options) => options,
        Err(message) => return json!({ "error": message }),
    };
    match read_asset_authorized(user, script, asset, &options) {
        Ok(read) => {
            let mut body = read.to_json();
            if let Some(object) = body.as_object_mut() {
                object.insert("script".to_string(), json!(script));
                object.insert("asset".to_string(), json!(asset));
                object.insert("timestamp".to_string(), json!(iso_timestamp()));
            }
            body
        }
        Err(AssetReadError::AccessDenied) => json!({ "error": "Error: Access denied" }),
        Err(AssetReadError::NotFound) => {
            json!({ "error": format!("Asset not found: {}", asset) })
        }
        Err(AssetReadError::Validation(message)) => json!({ "error": message }),
    }
}

fn tool_write_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    let Some(mimetype) = arg_str(args, "mimetype") else {
        return missing_arg("mimetype");
    };
    let Some(content) = arg_str(args, "content") else {
        return missing_arg("content");
    };

    match upsert_asset_authorized(user, script, asset, mimetype, content) {
        Ok(revision) => json!({
            "success": true,
            "message": format!("Asset '{}' upserted successfully", asset),
            "script": script,
            "asset": asset,
            "revision": revision,
            "timestamp": iso_timestamp(),
        }),
        Err(AssetWriteError::AccessDenied) => {
            json!({ "error": "Failed to write asset: Access denied" })
        }
        Err(AssetWriteError::Validation(msg)) | Err(AssetWriteError::Storage(msg)) => {
            json!({ "error": format!("Failed to write asset: {}", msg) })
        }
    }
}

/// Write several of a script's assets as one unit — the MCP face of
/// [`assets_batch_route`], down to the shape of its answer.
fn tool_write_assets(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(files) = args.get("files").and_then(Value::as_array) else {
        return missing_arg("files");
    };
    let reinit = match ReinitMode::parse(arg_str(args, "reinit")) {
        Ok(reinit) => reinit,
        Err(message) => return json!({ "error": message }),
    };

    let mut writes = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let Some(name) = arg_str(file, "name").or_else(|| arg_str(file, "asset")) else {
            return json!({
                "error": format!("files[{}]: missing required field: name", index)
            });
        };
        let Some(content_base64) =
            arg_str(file, "content_base64").or_else(|| arg_str(file, "content"))
        else {
            return json!({
                "error": format!(
                    "files[{}] ('{}'): missing required field: content_base64",
                    index, name
                )
            });
        };
        writes.push(AssetWrite {
            name: name.to_string(),
            mimetype: arg_str(file, "mimetype").map(str::to_string),
            content_base64: content_base64.to_string(),
            expected_sha256: arg_str(file, "sha256").map(str::to_string),
        });
    }

    match upsert_assets_authorized(user, script, &writes) {
        Ok(outcome) => {
            // Bridging back to async, as `check_script` does, so the caller is
            // told what init() did rather than that it was started.
            let init = match (reinit, outcome.written) {
                (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
                (ReinitMode::After, 0) => json!({ "ran": false, "reason": "no files changed" }),
                (ReinitMode::After, _) => {
                    init_result_json(&crate::database::run_blocking(reinitialize_script(script)))
                }
            };
            json!({
                "success": true,
                "script": script,
                "results": outcome.results.iter().map(AssetWriteOutcome::to_json).collect::<Vec<Value>>(),
                "written": outcome.written,
                "init": init,
                "timestamp": iso_timestamp(),
            })
        }
        Err(AssetWriteError::AccessDenied) => {
            json!({ "error": "Failed to write assets: Access denied" })
        }
        Err(AssetWriteError::Validation(msg)) | Err(AssetWriteError::Storage(msg)) => {
            json!({ "error": format!("Failed to write assets: {}", msg) })
        }
    }
}

fn tool_edit_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    let Some(edits) = args.get("edits").and_then(Value::as_array) else {
        return missing_arg("edits");
    };
    let reinit = match ReinitMode::parse(arg_str(args, "reinit")) {
        Ok(reinit) => reinit,
        Err(message) => return json!({ "error": message }),
    };

    let mut prepared = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let Some(old_string) = arg_str(edit, "old_string") else {
            return json!({
                "error": format!("edits[{}]: missing required field: old_string", index)
            });
        };
        let Some(new_string) = arg_str(edit, "new_string") else {
            return json!({
                "error": format!(
                    "edits[{}]: missing required field: new_string (pass \"\" to delete the text)",
                    index
                )
            });
        };
        prepared.push(AssetEdit {
            old_string: old_string.to_string(),
            new_string: new_string.to_string(),
            replace_all: edit
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }

    match patch_asset_authorized(
        user,
        script,
        asset,
        &prepared,
        arg_str(args, "base_sha256").or_else(|| arg_str(args, "sha256")),
    ) {
        Ok(outcome) => {
            // Bridging back to async, as write_assets does, so the caller is
            // told what init() did rather than that it was started.
            let init = match (reinit, outcome.status) {
                (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
                (_, "unchanged") => json!({ "ran": false, "reason": "no change" }),
                (ReinitMode::After, _) => {
                    init_result_json(&crate::database::run_blocking(reinitialize_script(script)))
                }
            };
            let mut body = outcome.to_json();
            if let Some(object) = body.as_object_mut() {
                object.insert("success".to_string(), json!(true));
                object.insert("script".to_string(), json!(script));
                object.insert("asset".to_string(), json!(asset));
                object.insert("init".to_string(), init);
                object.insert("timestamp".to_string(), json!(iso_timestamp()));
            }
            body
        }
        Err(AssetPatchError::AccessDenied) => {
            json!({ "error": "Failed to edit asset: Access denied" })
        }
        Err(AssetPatchError::NotFound) => {
            json!({ "error": format!("Asset not found: {}", asset) })
        }
        Err(AssetPatchError::Conflict { expected, actual }) => json!({
            "error": format!(
                "Asset '{}' has changed since it was read (expected {}, stored {})",
                asset, expected, actual
            ),
            "script": script,
            "asset": asset,
            "expected_sha256": expected,
            "sha256": actual,
        }),
        Err(AssetPatchError::Validation(message)) | Err(AssetPatchError::Storage(message)) => {
            json!({ "error": format!("Failed to edit asset: {}", message) })
        }
    }
}

fn tool_delete_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    match delete_asset_authorized(user, script, asset) {
        Ok((true, revision)) => json!({
            "success": true,
            "message": format!("Asset '{}' deleted successfully", asset),
            "script": script,
            "asset": asset,
            "revision": revision,
            "timestamp": iso_timestamp(),
        }),
        Ok((false, _)) => json!({ "error": format!("Asset '{}' not found", asset) }),
        Err(_) => json!({ "error": "Failed to delete asset: Access denied" }),
    }
}

fn owner_change_error_json(error: OwnerChangeError) -> Value {
    match error {
        OwnerChangeError::AccessDenied => {
            json!({ "error": "Permission denied. You must be an administrator or owner" })
        }
        OwnerChangeError::LastOwner => json!({
            "error": "Cannot remove the last owner. Transfer ownership to another user first, or contact an administrator."
        }),
        OwnerChangeError::Storage(details) => json!({ "error": details }),
    }
}

fn secret_error_json(error: SecretAccessError) -> Value {
    match error {
        SecretAccessError::AccessDenied => json!({
            "error": "Permission denied. You must be an administrator or owner of the script"
        }),
        SecretAccessError::Validation(details) | SecretAccessError::Storage(details) => {
            json!({ "error": details })
        }
    }
}

fn tool_list_script_owners(args: &Value, _user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    match owners_authorized(uri) {
        Ok(owners) => json!({
            "uri": uri,
            "owners": owners,
            "count": owners.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(details) => json!({ "error": details }),
    }
}

fn tool_add_script_owner(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let Some(owner) = arg_str(args, "owner") else {
        return missing_arg("owner");
    };
    match add_owner_authorized(user, uri, owner) {
        Ok(()) => json!({
            "success": true,
            "uri": uri,
            "owner": owner,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => owner_change_error_json(error),
    }
}

fn tool_remove_script_owner(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let Some(owner) = arg_str(args, "owner") else {
        return missing_arg("owner");
    };
    match remove_owner_authorized(user, uri, owner) {
        Ok(true) => json!({
            "success": true,
            "uri": uri,
            "owner": owner,
            "timestamp": iso_timestamp(),
        }),
        Ok(false) => {
            json!({ "error": format!("Owner '{}' was not found for script '{}'", owner, uri) })
        }
        Err(error) => owner_change_error_json(error),
    }
}

fn tool_list_secrets(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    match list_secrets_authorized(user, script) {
        Ok(keys) => json!({
            "script": script,
            "keys": keys,
            "count": keys.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(error) => secret_error_json(error),
    }
}

fn tool_write_secret(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(key) = arg_str(args, "key") else {
        return missing_arg("key");
    };
    let Some(value) = arg_str(args, "value") else {
        return missing_arg("value");
    };
    match set_secret_authorized(user, script, key, value) {
        Ok(()) => json!({
            "success": true,
            "script": script,
            "key": key,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => secret_error_json(error),
    }
}

fn tool_delete_secret(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(key) = arg_str(args, "key") else {
        return missing_arg("key");
    };
    match remove_secret_authorized(user, script, key) {
        Ok(true) => json!({
            "success": true,
            "script": script,
            "key": key,
            "timestamp": iso_timestamp(),
        }),
        Ok(false) => {
            json!({ "error": format!("Secret '{}' not found for script '{}'", key, script) })
        }
        Err(error) => secret_error_json(error),
    }
}

fn tool_clear_secrets(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    match clear_secrets_authorized(user, script) {
        Ok(()) => json!({
            "success": true,
            "cleared": true,
            "script": script,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => secret_error_json(error),
    }
}

fn user_admin_error_json(error: UserAdminError) -> Value {
    match error {
        UserAdminError::AccessDenied => {
            json!({ "error": "Permission denied. Administrator privileges are required" })
        }
        UserAdminError::UserNotFound(id) => json!({ "error": format!("User not found: {}", id) }),
        UserAdminError::LastAdministrator => json!({
            "error": "Cannot remove the last administrator. Grant the Administrator role to another user first."
        }),
        UserAdminError::Validation(details) | UserAdminError::Storage(details) => {
            json!({ "error": details })
        }
    }
}

fn tool_list_users(_args: &Value, user: &UserContext) -> Value {
    match list_users_authorized(user) {
        Ok(users) => json!({
            "users": users,
            "count": users.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(error) => user_admin_error_json(error),
    }
}

fn script_host_error_json(error: ScriptHostError) -> Value {
    let message = match error {
        ScriptHostError::AccessDenied => {
            "Permission denied. Administrator privileges are required to change where a script is published".to_string()
        }
        ScriptHostError::ScriptNotFound(uri) => format!("Script not found: {}", uri),
        ScriptHostError::Validation(details) => details,
        ScriptHostError::Storage(details) => details,
    };
    json!({ "error": message, "timestamp": iso_timestamp() })
}

fn script_hosts_json(uri: &str, stored: Vec<String>, effective: Vec<String>) -> Value {
    json!({
        "uri": uri,
        "hosts": stored,
        "publishedOn": effective,
        "servedHosts": crate::hosts::all_hosts(),
        "defaultHost": crate::hosts::default_host(),
        "timestamp": iso_timestamp(),
    })
}

fn tool_get_script_hosts(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    match get_script_hosts_authorized(user, uri) {
        Ok((stored, effective)) => script_hosts_json(uri, stored, effective),
        Err(error) => script_host_error_json(error),
    }
}

fn tool_set_script_hosts(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    // An explicit empty array is meaningful: it clears the binding.
    let Some(hosts) = args.get("hosts").and_then(|value| value.as_array()) else {
        return missing_arg("hosts");
    };
    let hosts: Vec<String> = hosts
        .iter()
        .filter_map(|host| host.as_str().map(str::to_string))
        .collect();

    match set_script_hosts_authorized(user, uri, &hosts) {
        Ok((stored, effective)) => {
            let mut body = script_hosts_json(uri, stored, effective);
            if let Some(obj) = body.as_object_mut() {
                obj.insert("success".to_string(), json!(true));
            }
            body
        }
        Err(error) => script_host_error_json(error),
    }
}

fn tool_add_user_role(args: &Value, user: &UserContext) -> Value {
    let Some(user_id) = arg_str(args, "user_id") else {
        return missing_arg("user_id");
    };
    let Some(role) = arg_str(args, "role") else {
        return missing_arg("role");
    };
    match add_user_role_authorized(user, user_id, role) {
        Ok(roles) => json!({
            "success": true,
            "userId": user_id,
            "role": role,
            "roles": roles,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => user_admin_error_json(error),
    }
}

fn tool_remove_user_role(args: &Value, user: &UserContext) -> Value {
    let Some(user_id) = arg_str(args, "user_id") else {
        return missing_arg("user_id");
    };
    let Some(role) = arg_str(args, "role") else {
        return missing_arg("role");
    };
    match remove_user_role_authorized(user, user_id, role) {
        Ok(roles) => json!({
            "success": true,
            "userId": user_id,
            "role": role,
            "roles": roles,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => user_admin_error_json(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{host_is_allowed, reserved_route_prefix};

    #[test]
    fn reserved_prefixes_match_exact_and_subpaths() {
        assert_eq!(reserved_route_prefix("/engine"), Some("/engine"));
        assert_eq!(reserved_route_prefix("/engine/scripts"), Some("/engine"));
        assert_eq!(reserved_route_prefix("/auth/login"), Some("/auth"));
        assert_eq!(
            reserved_route_prefix("/.well-known/oauth-authorization-server"),
            Some("/.well-known")
        );
        assert_eq!(reserved_route_prefix("/health"), Some("/health"));
        assert_eq!(reserved_route_prefix("/graphql/sse"), Some("/graphql"));
        assert_eq!(reserved_route_prefix("/mcp"), Some("/mcp"));
        assert_eq!(
            reserved_route_prefix("/engine/health/cluster"),
            Some("/engine")
        );
        // OAuth2 lives entirely under /auth, so it needs no prefix of its own.
        assert_eq!(reserved_route_prefix("/auth/oauth2/token"), Some("/auth"));
    }

    #[test]
    fn non_reserved_paths_are_allowed() {
        assert_eq!(reserved_route_prefix("/"), None);
        assert_eq!(reserved_route_prefix("/favicon.ico"), None);
        assert_eq!(reserved_route_prefix("/engineering"), None);
        assert_eq!(reserved_route_prefix("/healthcheck"), None);
        assert_eq!(reserved_route_prefix("/authors"), None);
        assert_eq!(reserved_route_prefix("/my/app"), None);
        // The top-level OAuth2 endpoints were withdrawn once clients migrated
        // to /auth/oauth2/*, so scripts may claim these names themselves.
        assert_eq!(reserved_route_prefix("/authorize"), None);
        assert_eq!(reserved_route_prefix("/token"), None);
        assert_eq!(reserved_route_prefix("/oauth2/token"), None);
        assert_eq!(reserved_route_prefix("/oauth2"), None);
    }

    #[test]
    fn empty_management_host_list_allows_every_host() {
        // A single-host deployment leaves the setting unset and must keep
        // serving the management APIs wherever it is reached.
        assert!(host_is_allowed(&[], Some("softagen.com")));
        assert!(host_is_allowed(&[], None));
    }

    #[test]
    fn configured_management_hosts_match_exactly_and_case_insensitively() {
        let allowed = vec!["manage.softagen.com".to_string()];

        assert!(host_is_allowed(&allowed, Some("manage.softagen.com")));
        assert!(host_is_allowed(&allowed, Some("MANAGE.Softagen.com")));
        assert!(host_is_allowed(&allowed, Some("  manage.softagen.com  ")));

        assert!(!host_is_allowed(&allowed, Some("softagen.com")));
        assert!(!host_is_allowed(&allowed, Some("world.softagen.com")));
        // Not a suffix or prefix match: neither a parent domain nor an
        // attacker-controlled name that merely ends with the allowed host.
        assert!(!host_is_allowed(&allowed, Some("evil-manage.softagen.com")));
        assert!(!host_is_allowed(
            &allowed,
            Some("manage.softagen.com.evil.test")
        ));
    }

    #[test]
    fn missing_host_header_is_refused_when_restricted() {
        let allowed = vec!["manage.softagen.com".to_string()];
        assert!(!host_is_allowed(&allowed, None));
    }

    #[test]
    fn management_host_list_may_name_several_hosts() {
        let allowed = vec![
            "manage.softagen.com".to_string(),
            "localhost:3000".to_string(),
        ];
        assert!(host_is_allowed(&allowed, Some("manage.softagen.com")));
        assert!(host_is_allowed(&allowed, Some("localhost:3000")));
        assert!(!host_is_allowed(&allowed, Some("localhost")));
    }
}
