//! Native engine management API for scripts and assets.
//!
//! These REST routes and MCP tools were previously implemented by the
//! bootstrapped `core.js` and `cli.js` scripts. They are engine
//! functionality, so they live in Rust: the HTTP contract and MCP tool
//! names/shapes are kept identical to the script implementations they
//! replace. Authorization mirrors the checks that `secure_globals`
//! applies to the equivalent `scriptStorage`/`assetStorage` functions.

use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::auth::AuthUser;
use crate::repository;
use crate::security::{
    Capability, SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity, UserContext,
};

/// Maximum asset size accepted by the write paths (same limit as the sandbox).
const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;

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

/// Re-initialize a script in the background after an upsert: clear its
/// GraphQL/MCP registrations, run init(), and rebuild the GraphQL schema.
/// Mirrors the behavior of the sandboxed `scriptStorage.upsertScript`.
fn spawn_script_init(script_uri: String) {
    tokio::task::spawn(async move {
        crate::graphql::clear_script_graphql_registrations(&script_uri);
        crate::mcp::clear_script_mcp_registrations(&script_uri);

        let initializer = crate::script_init::ScriptInitializer::with_configured_timeout();
        match initializer.initialize_script(&script_uri, false).await {
            Ok(result) => {
                if result.success {
                    if let Err(e) = crate::graphql::rebuild_schema() {
                        warn!(
                            "Failed to rebuild GraphQL schema after script '{}' initialization: {:?}",
                            script_uri, e
                        );
                    }
                } else if let Some(err) = result.error {
                    warn!("Script '{}' init failed after upsert: {}", script_uri, err);
                }
            }
            Err(e) => {
                warn!(
                    "Failed to initialize script '{}' after upsert: {}",
                    script_uri, e
                );
            }
        }
    });
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

/// Create or update a script with the same authorization semantics as
/// `scriptStorage.upsertScript`: WriteScripts capability required, and
/// existing scripts can only be modified by an admin or an owner.
/// Broadcasts the update and re-initializes the script on success.
pub fn upsert_script_authorized(
    user: &UserContext,
    uri: &str,
    content: &str,
    via: Option<&str>,
) -> Result<UpsertAction, String> {
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

    Ok(action)
}

/// Delete a script with the same semantics as `scriptStorage.deleteScript`:
/// DeleteScripts capability required. Returns false when the capability is
/// missing or the script does not exist. Broadcasts the removal on success.
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
    if user.require_capability(&Capability::ReadScripts).is_err() {
        return None;
    }
    repository::fetch_script(uri)
}

/// List script metadata; ReadScripts capability required (empty otherwise).
pub fn list_scripts_authorized(user: &UserContext) -> Vec<repository::ScriptMetadata> {
    if user.require_capability(&Capability::ReadScripts).is_err() {
        return Vec::new();
    }
    repository::get_all_script_metadata().unwrap_or_default()
}

/// Fetch logs for a script as JSON objects; ViewLogs capability required
/// (empty otherwise) — same as `console.listLogsForUri`.
pub fn logs_authorized(user: &UserContext, uri: &str) -> Vec<Value> {
    if user.require_capability(&Capability::ViewLogs).is_err() {
        return Vec::new();
    }
    repository::fetch_log_messages(uri)
        .iter()
        .map(|entry| {
            let timestamp_ms = entry
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f64;
            json!({
                "message": entry.message,
                "level": entry.level,
                "timestamp": timestamp_ms
            })
        })
        .collect()
}

/// Init status for one script; ReadScripts capability required.
pub fn init_status_authorized(user: &UserContext, uri: &str) -> Option<Value> {
    if user.require_capability(&Capability::ReadScripts).is_err() {
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

/// List a script's owners. Anyone may view owners (for transparency), same
/// as `scriptStorage.getScriptOwners`.
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
pub enum SecretAccessError {
    AccessDenied,
    Validation(String),
    Storage(String),
}

/// Cross-script secret management (the `secretStorage.*ForUri` functions):
/// admins and owners of the target script may manage its secret keys. Secret
/// values are write-only through this surface — there is deliberately no
/// read-value operation.
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

/// Store a secret for a script. Same validation as `setSecretForUri`.
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
/// Unlike the deprecated `userStorage.listUsers()`, an unauthorized caller is
/// denied rather than handed an empty array — "denied" and "no users" must not
/// look alike.
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
/// capability: capability holders, script owners, and admins all qualify —
/// same rule as the `assetStorage.*ForUri` functions.
fn can_access_assets(user: &UserContext, script_uri: &str, capability: &Capability) -> bool {
    user.has_capability(capability)
        || user.has_capability(&Capability::DeleteScripts)
        || user_owns_script(user, script_uri)
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

/// Fetch an asset's content as base64 (same transfer encoding the sandbox
/// used, which the /assets and MCP contracts expose).
pub fn fetch_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
) -> Result<String, AssetFetchError> {
    if !can_access_assets(user, script_uri, &Capability::ReadAssets) {
        return Err(AssetFetchError::AccessDenied);
    }
    match repository::fetch_asset(script_uri, asset_uri) {
        Some(asset) => Ok(base64::engine::general_purpose::STANDARD.encode(asset.content)),
        None => Err(AssetFetchError::NotFound),
    }
}

pub enum AssetWriteError {
    AccessDenied,
    Validation(String),
    Storage(String),
}

/// Create or update an asset from base64 content.
pub fn upsert_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    mimetype: &str,
    content_b64: &str,
) -> Result<(), AssetWriteError> {
    let content = base64::engine::general_purpose::STANDARD
        .decode(content_b64)
        .map_err(|e| {
            AssetWriteError::Validation(format!("Error decoding base64 content: {}", e))
        })?;

    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied);
    }

    if asset_uri.is_empty() || asset_uri.len() > 255 {
        return Err(AssetWriteError::Validation(
            "Invalid asset URI: must be 1-255 characters".to_string(),
        ));
    }
    if asset_uri.contains("..") || asset_uri.contains('\\') {
        return Err(AssetWriteError::Validation(
            "Invalid asset URI: path traversal not allowed".to_string(),
        ));
    }
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
        .map_err(|e| AssetWriteError::Storage(format!("Error upserting asset: {}", e)))
}

/// Delete an asset.
pub fn delete_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
) -> Result<bool, AssetFetchError> {
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

    Ok(repository::delete_asset(script_uri, asset_uri))
}

// ============================================================================
// OpenAPI spec generation
// ============================================================================

/// Generate the full OpenAPI spec: the Rust (utoipa) spec merged with
/// script-registered routes, asset routes, and SSE stream routes. Returns
/// the same `{"error": ...}` JSON strings as the former JS implementation
/// when a step fails, so callers can pass the result through unchanged.
pub fn generate_merged_openapi_spec() -> String {
    let rust_spec_str = crate::get_rust_openapi_spec();
    let mut rust_spec: Value = match serde_json::from_str(&rust_spec_str) {
        Ok(spec) => spec,
        Err(e) => {
            return format!(
                "{{\"error\": \"Failed to parse Rust OpenAPI spec: {}\"}}",
                e
            );
        }
    };

    let metadata_list = match repository::get_all_script_metadata() {
        Ok(list) => list,
        Err(e) => {
            return format!(
                "{{\"error\": \"Failed to fetch JavaScript routes: {}\"}}",
                e
            );
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
        Err(e) => format!(
            "{{\"error\": \"Failed to serialize merged OpenAPI spec: {}\"}}",
            e
        ),
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
        upsert_script_authorized(&user, &uri, &content, None).map(|_| (uri, content.len()))
    })
    .await;

    match result {
        Ok(Ok((uri, content_length))) => json_response(
            StatusCode::OK,
            json!({
                "success": true,
                "message": "Script upserted successfully",
                "uri": uri,
                "contentLength": content_length,
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

    let result = crate::script_test::TestRunner::with_configured_timeouts()
        .run(crate::script_test::TestRunRequest {
            script_uri: uri,
            user_context: user,
            filter,
            rollback,
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

/// Get logs for a script.
#[utoipa::path(
    get,
    path = "/engine/script_logs",
    tags = ["Logging"],
    params(("uri" = String, Query, description = "Script URI")),
    responses(
        (status = 200, description = "Log entries for the script"),
        (status = 400, description = "Missing required parameter"),
    )
)]
pub async fn script_logs_route(
    auth_user: Option<Extension<AuthUser>>,
    Query(query): Query<ScriptParams>,
) -> Response {
    let user = user_context_from(auth_user.as_deref());
    let Some(uri) = query.uri else {
        return missing_param_response("uri");
    };

    let uri_for_task = uri.clone();
    let logs = tokio::task::spawn_blocking(move || logs_authorized(&user, &uri_for_task))
        .await
        .unwrap_or_default();

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

#[derive(Deserialize, Default)]
pub struct AssetQuery {
    script: Option<String>,
    asset: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct AssetBody {
    asset: Option<String>,
    mimetype: Option<String>,
    content: Option<String>,
}

fn error_response(status: StatusCode, message: String) -> Response {
    json_response(status, json!({ "error": message }))
}

/// List assets for a script, or fetch one asset when `asset` is given.
#[utoipa::path(
    get,
    path = "/engine/assets",
    tags = ["Assets"],
    params(
        ("script" = String, Query, description = "URI of the script whose assets to manage"),
        ("asset" = Option<String>, Query, description = "Asset URI to fetch; omit to list all"),
    ),
    responses(
        (status = 200, description = "Asset list or asset content (base64)"),
        (status = 400, description = "Missing required parameter"),
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
            let (user, script_cl, asset_cl) = (user, script.clone(), asset.clone());
            let result = tokio::task::spawn_blocking(move || {
                fetch_asset_authorized(&user, &script_cl, &asset_cl)
            })
            .await
            .unwrap_or(Err(AssetFetchError::NotFound));

            match result {
                Ok(content) => json_response(
                    StatusCode::OK,
                    json!({ "script": script, "asset": asset, "content": content }),
                ),
                Err(AssetFetchError::AccessDenied) => {
                    error_response(StatusCode::NOT_FOUND, "Error: Access denied".to_string())
                }
                Err(AssetFetchError::NotFound) => error_response(
                    StatusCode::NOT_FOUND,
                    format!("Asset '{}' not found", asset),
                ),
            }
        }
        None => {
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
        Ok(()) => json_response(
            StatusCode::CREATED,
            json!({
                "message": format!("Asset '{}' upserted successfully", asset),
                "script": script,
                "asset": asset,
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
            .unwrap_or(Ok(false));

    match result {
        Err(AssetFetchError::AccessDenied) => {
            error_response(StatusCode::FORBIDDEN, "Error: Access denied".to_string())
        }
        Ok(false) | Err(AssetFetchError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            format!("Asset '{}' not found", asset),
        ),
        Ok(true) => json_response(
            StatusCode::OK,
            json!({
                "message": format!("Asset '{}' deleted successfully", asset),
                "script": script,
                "asset": asset,
            }),
        ),
    }
}

/// List all scripts with metadata (same shape as `scriptStorage.listScripts`).
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
    let (db_healthy, pool_stats) = if let Some(db) = crate::database::get_global_database() {
        let connected = db.health_check().await.is_ok();
        let pool = db.pool();
        let size = pool.size() as usize;
        let idle = pool.num_idle();
        (
            connected,
            json!({
                "available": true,
                "connected": connected,
                "active_connections": size.saturating_sub(idle),
                "idle_connections": idle,
                "max_connections": pool.options().get_max_connections(),
            }),
        )
    } else {
        (
            false,
            json!({
                "available": false,
                "connected": false,
                "message": "Database not initialized (memory mode)"
            }),
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
        .unwrap_or_else(|e| format!("{{\"error\": \"join error: {}\"}}", e));

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
            "Read log messages for a specific script (useful for debugging)",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "uri": { "type": "string", "description": "Script URI to retrieve logs for" }
                    },
                    "required": ["uri"]
                })
            },
            tool_read_logs,
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
            "Fetch the base64-encoded content of a specific asset owned by a script. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the asset" },
                        "asset": { "type": "string", "description": "URI/path of the asset to fetch (e.g., '/images/logo.png')" }
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
                        }
                    },
                    "required": ["uri"]
                })
            },
            tool_run_tests,
        ),
    ]
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

    let (timeout_ms, run_timeout_ms) = crate::script_test::configured_test_timeouts();
    let modules = crate::module_loader::discover_test_modules(uri);
    let result = crate::js_engine::execute_test_run(
        &crate::js_engine::TestRunParams {
            script_uri: uri.to_string(),
            user_context: user.clone(),
            timeout_ms,
            run_timeout_ms,
            filter,
            rollback,
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
        Ok(action) => {
            let action = match action {
                UpsertAction::Inserted => "created",
                UpsertAction::Updated => "updated",
            };
            json!({
                "success": true,
                "action": action,
                "uri": uri,
                "size": content.len(),
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
    let Some(uri) = arg_str(args, "uri") else {
        return missing_arg("uri");
    };
    let logs = logs_authorized(user, uri);
    json!({
        "uri": uri,
        "logs": logs,
        "count": logs.len(),
        "timestamp": iso_timestamp(),
    })
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
    match fetch_asset_authorized(user, script, asset) {
        Ok(content) => json!({
            "script": script,
            "asset": asset,
            "content": content,
            "timestamp": iso_timestamp(),
        }),
        Err(AssetFetchError::AccessDenied) => json!({ "error": "Error: Access denied" }),
        Err(AssetFetchError::NotFound) => {
            json!({ "error": format!("Asset not found: {}", asset) })
        }
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
        Ok(()) => json!({
            "success": true,
            "message": format!("Asset '{}' upserted successfully", asset),
            "script": script,
            "asset": asset,
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

fn tool_delete_asset(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(asset) = arg_str(args, "asset") else {
        return missing_arg("asset");
    };
    match delete_asset_authorized(user, script, asset) {
        Ok(true) => json!({
            "success": true,
            "message": format!("Asset '{}' deleted successfully", asset),
            "script": script,
            "asset": asset,
            "timestamp": iso_timestamp(),
        }),
        Ok(false) => json!({ "error": format!("Asset '{}' not found", asset) }),
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
