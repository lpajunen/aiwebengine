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
pub const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;

/// Maximum number of files one batch write may carry.
pub const MAX_BATCH_FILES: usize = 256;

/// Maximum total decoded size of one batch write. The per-file ceiling still
/// applies to each file, so a batch cannot smuggle in an oversized asset.
pub const MAX_BATCH_BYTES: usize = MAX_ASSET_BYTES;

/// Request body ceiling for the batch route: the decoded ceiling, plus the
/// third base64 adds, plus room for the JSON envelope around it.
///
/// The management router's own limit is `security.max_request_body_bytes`,
/// which defaults to 1MB — far below what a batch of source files needs — so
/// the batch route overrides it (see `lib.rs`).
pub const MAX_BATCH_BODY_BYTES: usize = MAX_BATCH_BYTES * 4 / 3 + 1024 * 1024;

/// Request body ceiling for the single-asset routes, on the same reasoning as
/// [`MAX_BATCH_BODY_BYTES`] and for a failure the batch route was spared by
/// having one.
///
/// An asset's content travels base64-encoded, so a body limit bounds three
/// quarters of an asset. Inheriting the router's `max_request_body_bytes` made
/// the largest asset writable here three quarters of that — 786 KB against the
/// 1 MB default, 7.86 MB against the 10 MB a deployment ships — while
/// [`MAX_ASSET_BYTES`], the sandbox and the published document all named
/// 10,000,000 bytes. The endpoint whose job is writing an asset was the one
/// path that could not write the largest one, and it answered with a 413 about
/// a body rather than anything about the limit it was failing.
pub const MAX_ASSET_BODY_BYTES: usize = MAX_ASSET_BYTES * 4 / 3 + 64 * 1024;

/// Maximum number of edits one patch may carry.
pub const MAX_PATCH_EDITS: usize = 128;

/// How many matching lines a `grep=` read reports before it stops looking.
pub const MAX_GREP_MATCHES: usize = 200;

/// How much of a matching line a `grep=` read echoes back.
pub const MAX_GREP_LINE_CHARS: usize = 512;

/// Longest `grep=` pattern accepted, so a read cannot hand the regex engine an
/// arbitrarily large program to compile.
pub const MAX_GREP_PATTERN_CHARS: usize = 512;

fn iso_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn user_context_from(auth_user: Option<&AuthUser>) -> UserContext {
    UserContext::for_session(auth_user.map(AuthUser::roles).unwrap_or_default())
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

/// Capability-only admin check: does this caller hold `AdministerEngine`?
///
/// **This does not mean the caller is an authenticated administrator.**
/// Capability alone. No tier grants `AdministerEngine` without a session any
/// more, so in practice this and [`is_user_admin`] agree — the two are kept
/// apart because they answer different questions, and the stricter one is what
/// anything exposing user data, engine topology or role changes should ask.
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
fn may_administer(user: &UserContext) -> bool {
    user.is_authenticated
}

fn has_admin_capability(user: &UserContext) -> bool {
    user.has_capability(&Capability::AdministerEngine)
}

fn is_admin_or_owner(user: &UserContext, script_uri: &str) -> bool {
    has_admin_capability(user) || user_owns_script(user, script_uri)
}

/// Path prefixes owned by the engine. Scripts may not register HTTP, stream,
/// or asset routes at or under these prefixes; every other path is open to
/// any script. `/` and `/favicon.ico` are intentionally not reserved — the
/// engine serves defaults for them only when no script claims them.
pub const RESERVED_ROUTE_PREFIXES: &[&str] =
    &["/health", "/mcp", "/auth", "/.well-known", "/engine"];

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

/// Re-initialize a script after an upsert: clear its MCP registrations and
/// run init().
///
/// Every local deploy path funnels through here, so what a script upsert does
/// to a script's registrations and what a batch asset write does to them
/// cannot drift apart.
async fn reinitialize_script(script_uri: &str) -> crate::script_init::InitResult {
    crate::mcp::clear_script_mcp_registrations(script_uri);

    let initializer = crate::script_init::ScriptInitializer::with_configured_timeout();
    match initializer.initialize_script(script_uri, false).await {
        Ok(result) => {
            if !result.success
                && let Some(err) = &result.error
            {
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
/// Resolve a caller's `revision` argument to a revision number.
///
/// Accepts a number, `head`, `last-good`, or a label. A revision named and not
/// found is an error rather than a fall back to what is deployed: a caller who
/// asked for 41 and silently got head would be told their check passed against
/// code they never named.
///
/// `last-good` is the one worth spelling out: it names the newest revision
/// whose write left `init()` succeeding, which is the target an operator means
/// by "back to when it worked" and a number they would otherwise have to find
/// by reading the history themselves.
async fn resolve_revision(script_uri: &str, spec: &str) -> Result<i32, String> {
    match spec {
        "head" => revisions::head(script_uri)
            .await
            .map_err(|e| format!("Failed to read revision history: {}", e))?
            .ok_or_else(|| format!("Script '{}' has no revisions yet", script_uri)),
        "last-good" => revisions::last_good(script_uri)
            .await
            .map_err(|e| format!("Failed to read revision history: {}", e))?
            .ok_or_else(|| {
                format!(
                    "Script '{}' has no revision whose init() succeeded",
                    script_uri
                )
            }),
        _ => match spec.parse::<i32>() {
            Ok(number) => {
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
                Ok(number)
            }
            Err(_) => revisions::by_label(script_uri, spec)
                .await
                .map_err(|e| format!("Failed to resolve revision label: {}", e))?
                .ok_or_else(|| {
                    format!(
                        "Script '{}' has no revision labelled '{}'",
                        script_uri, spec
                    )
                }),
        },
    }
}

/// Resolve a caller's `revision` argument to a view of a script's files.
///
/// Absent means the deployed state — so every existing caller keeps checking
/// and testing what is serving, which is what they have always meant.
async fn resolve_view(
    script_uri: &str,
    revision: Option<&str>,
) -> Result<crate::source_view::SourceView, String> {
    use crate::source_view::SourceView;

    let Some(spec) = revision.map(str::trim).filter(|spec| !spec.is_empty()) else {
        return Ok(SourceView::Live);
    };

    Ok(SourceView::Revision(
        resolve_revision(script_uri, spec).await?,
    ))
}

/// The `check_script` report for what a write just stored, unless the caller
/// opted out with `check: false`.
///
/// A write answers what `init()` did; it cannot say that a handler it
/// registered does not exist, that a registration was refused, or what the
/// script now serves. The check says all three, and a caller that writes and
/// then asks would be making two round trips for one verdict. It runs against
/// head, which is what was just written, so it is also the verdict for a
/// pinned script whose `init()` was left alone.
fn check_after_write(script: &str, user: &UserContext, args: &Value) -> Option<Value> {
    if args.get("check").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    Some(tool_check_script(&json!({ "script": script }), user))
}

/// Re-initialise a script after a write, unless a write is not what it serves.
///
/// A pinned script runs a revision, so writing its files changes nothing about
/// the program that is running. Re-initialising anyway would clear and rebuild
/// the registrations of a working deployment on the strength of an edit it is
/// not serving — the disturbance pinning exists to prevent.
pub async fn reinitialize_after_write(script_uri: &str) -> Value {
    if let Some(revision) = crate::deployments::pinned(script_uri) {
        return json!({
            "ran": false,
            "reason": "pinned",
            "servingRevision": revision,
        });
    }
    init_result_json(&reinitialize_script(script_uri).await)
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
/// Whether `user` may write `uri`, and whether it already exists.
///
/// Extracted so that every path writing a script's root — a caller's own write,
/// and a sync replaying somebody's repository into it — asks the same question.
/// The rule it encodes is the engine's rule everywhere: `WriteScripts` to write
/// at all, and ownership or `AdministerEngine` to write over something that is
/// already there.
fn authorize_script_write(user: &UserContext, uri: &str) -> Result<bool, String> {
    if let Err(e) = user.require_capability(&Capability::WriteScripts) {
        return Err(format!("Error: {}", e));
    }
    if uri.is_empty() {
        return Err("Error: Script name cannot be empty".to_string());
    }

    let exists = repository::fetch_script(uri).is_some();
    // A name is checked where a script comes into being. Every script that
    // already exists keeps the identifier it has, whatever its shape.
    if !exists {
        crate::slug::validate(uri).map_err(|e| format!("Error: {}", e))?;
    }
    if exists {
        let is_admin = user.has_capability(&Capability::AdministerEngine);
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
    Ok(exists)
}

/// Whether `user` could write `uri` right now.
///
/// A pull checks every script it is about to write before writing any of them.
/// A repository is one change, and half-applying it because the fourth script
/// happens to belong to somebody else leaves the engine holding a state the
/// repository never described.
pub fn can_write_script(user: &UserContext, uri: &str) -> bool {
    authorize_script_write(user, uri).is_ok()
}

pub fn upsert_script_authorized(
    user: &UserContext,
    uri: &str,
    content: &str,
    via: Option<&str>,
) -> Result<(UpsertAction, Option<i32>), String> {
    upsert_root_authorized(user, uri, None, content, via)
}

/// [`upsert_script_authorized`] into the entrypoint file `root` names.
///
/// `None` writes whichever entrypoint the tree already has, which is what a
/// caller sending only source means. `Some("main.ts")` writes that file and
/// removes any other entrypoint, which is what a caller writing a file by
/// name means — and the only way the language of an entrypoint can change,
/// since the name is what says it.
pub fn upsert_root_authorized(
    user: &UserContext,
    uri: &str,
    root: Option<&str>,
    content: &str,
    via: Option<&str>,
) -> Result<(UpsertAction, Option<i32>), String> {
    if content.is_empty() {
        return Err("Error: Script name and content cannot be empty".to_string());
    }
    let exists = authorize_script_write(user, uri)?;

    if let Err(e) = repository::upsert_root_with_owner(uri, root, content, user.user_id.as_deref())
    {
        return Err(format!("Error storing script: {}", e));
    }

    // Recorded before init is spawned, so the revision exists by the time the
    // init that follows looks for one to attach its outcome to.
    let revision =
        revisions::record_blocking(uri, revisions::Origin::Script, user.user_id.as_deref());

    // A pinned script serves a revision; writing its root advances head and
    // leaves the running program alone, so there is nothing to initialise.
    if crate::deployments::pinned(uri).is_none() {
        spawn_script_init(uri.to_string());
    }

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

/// Edit a script's entrypoint in place, by replacing strings within it.
///
/// [`patch_file_authorized`] aimed at whichever file of the tree is the root,
/// which is what `/engine/edit_file` means by "the script": the caller
/// names a script and not a path, so the path is resolved here.
///
/// It deliberately does not run `init()`. A whole write re-initialises on its
/// way out; a patch leaves that to the caller's `reinit`, so a change spanning
/// the entrypoint and three modules initialises once at the end rather than
/// once per file.
pub fn patch_script_authorized(
    user: &UserContext,
    uri: &str,
    edits: &[StringEdit],
    base_sha256: Option<&str>,
    via: Option<&str>,
) -> Result<PatchOutcome, PatchError> {
    if uri.is_empty() {
        return Err(PatchError::Validation(
            "Script name cannot be empty".to_string(),
        ));
    }
    let path =
        crate::module_loader::root_module_path_in(uri, &crate::source_view::SourceView::Live)
            .map_err(|_| PatchError::NotFound)?;
    patch_file_authorized(user, uri, &path, edits, base_sha256, via)
}

/// Edit one file of a script's tree in place.
///
/// This was two functions with ninety near-identical lines each, because a
/// patch of the root wrote a column and a patch of a module wrote a row. What
/// actually differs is policy, and it is gathered here rather than spread
/// across two copies of the same arithmetic:
///
/// - **Who may.** The entrypoint takes `WriteScripts` and ownership, every
///   other file takes `WriteAssets` and ownership. The same line the whole
///   write and the delete draw.
/// - **How big.** Source meets the 1MB script ceiling; an asset meets the
///   10MB one.
/// - **Whether it may be emptied.** Edits that leave the entrypoint empty are
///   refused — a script with no program is not a state to store, and deleting
///   it is what `delete_script` is. Any other file may legitimately become
///   empty.
pub fn patch_file_authorized(
    user: &UserContext,
    script_uri: &str,
    path: &str,
    edits: &[StringEdit],
    base_sha256: Option<&str>,
    via: Option<&str>,
) -> Result<PatchOutcome, PatchError> {
    let is_root = crate::module_loader::is_root_module_name(path);

    // `authorize_script_write` answers "may this caller write here", and
    // reports whether anything is there. A patch of the entrypoint needs
    // both: the same rule a whole write applies, and a file to apply the
    // edits to.
    if is_root {
        let exists = authorize_script_write(user, script_uri).map_err(|message| {
            // Its refusals are phrased for callers that render them as they
            // stand; a patch's are wrapped, so the prefix would be said twice.
            PatchError::AccessDenied(
                message
                    .strip_prefix("Error: ")
                    .unwrap_or(&message)
                    .to_string(),
            )
        })?;
        if !exists {
            return Err(PatchError::NotFound);
        }
    } else if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(PatchError::AccessDenied("Access denied".to_string()));
    }

    // The stored file rather than the served one. Basing the edits on a pin
    // and writing the result to head is how a patch after a batch reverted
    // the batch, and `base_sha256` could not catch it: the digest was taken
    // from the same pinned content the edits were applied to, so the
    // precondition agreed with itself while disagreeing with what was stored.
    let Some(file) = repository::fetch_asset(script_uri, path) else {
        return Err(PatchError::NotFound);
    };

    let original_digest = sha256_hex(&file.content);
    if let Some(expected) = base_sha256
        && !expected.eq_ignore_ascii_case(&original_digest)
    {
        return Err(PatchError::Conflict {
            expected: expected.to_string(),
            actual: original_digest,
        });
    }

    let mut text = String::from_utf8(file.content.clone()).map_err(|_| {
        PatchError::Validation(format!(
            "'{}' is not UTF-8 text, so it cannot be edited as strings: write it whole \
             with /engine/write_file or /engine/write_files",
            path
        ))
    })?;

    let replacements =
        apply_string_edits(&mut text, edits, path).map_err(PatchError::Validation)?;

    // The bounds a whole write gets from its own request body, asked here of
    // the content the edits produced — since the point of a patch is that its
    // body is not the content, so neither bound can be read off it.
    if is_root && text.is_empty() {
        return Err(PatchError::Validation(format!(
            "The edits would leave script '{}' empty; delete it instead if that is what was meant",
            script_uri
        )));
    }
    let ceiling = if is_root {
        repository::MAX_SCRIPT_CONTENT_BYTES
    } else {
        MAX_ASSET_BYTES
    };
    if text.len() > ceiling {
        return Err(PatchError::Validation(format!(
            "'{}' too large after the edits: {} bytes (max {})",
            path,
            text.len(),
            ceiling
        )));
    }

    let content = text.into_bytes();
    let unchanged = content == file.content;
    let digest = if unchanged {
        original_digest
    } else {
        sha256_hex(&content)
    };
    let bytes = content.len();

    if !unchanged {
        // Through the batch write, so a patch reaches storage the same way a
        // deploy does: one transaction, one notification to the cluster.
        repository::upsert_assets(
            script_uri,
            vec![repository::Asset {
                uri: file.uri.clone(),
                name: file.name.clone(),
                mimetype: file.mimetype.clone(),
                content,
                created_at: file.created_at,
                updated_at: std::time::SystemTime::now(),
                script_uri: script_uri.to_string(),
            }],
        )
        .map_err(|e| PatchError::Storage(format!("Error patching '{}': {}", path, e)))?;
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let path_owned = path.to_string();
    let edit_count = edits.len();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Medium,
                    user_id,
                )
                .with_resource("file".to_string())
                .with_action("patch_for_uri".to_string())
                .with_detail("uri", &path_owned)
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

    // Watchers of `/engine/script_updates` see a patch of the entrypoint the
    // way they see a whole write: the file they are tracking changed, and how
    // much of it travelled to say so is not something they should have to
    // know. A patch of any other file is not a change to the script's program
    // and is not broadcast, which is what it did before this.
    if is_root && !unchanged {
        let mut details = vec![
            ("contentLength", json!(bytes)),
            ("previousExists", json!(true)),
            ("replacements", json!(replacements)),
        ];
        if let Some(via) = via {
            details.push(("via", json!(via)));
        }
        broadcast_script_update(script_uri, UpsertAction::Updated.as_str(), &details);
    }

    Ok(PatchOutcome {
        sha256: digest,
        revision,
        bytes,
        replacements,
        status: if unchanged { "unchanged" } else { "updated" },
    })
}

/// Delete a script: `DeleteScripts` capability required, and the script must be
/// one the caller owns unless they hold `AdministerEngine`. Returns false when
/// the capability is missing, the caller is neither admin nor owner, or the
/// script does not exist. Broadcasts the removal on success.
/// Give a script a new name.
///
/// Takes what writing the script takes — `WriteScripts`, and ownership or
/// administration — and not `DeleteScripts`, because nothing is lost: the
/// files, history, secrets, tables, queue and settings are the same script's
/// under another name. The new name has to be a slug even when the old one was
/// not, which is the point of renaming.
pub fn rename_script_authorized(user: &UserContext, uri: &str, to: &str) -> Result<(), String> {
    let exists = authorize_script_write(user, uri)?;
    if !exists {
        return Err(format!("Script not found: {}", uri));
    }
    crate::slug::validate(to).map_err(|e| format!("Error: {}", e))?;
    if uri == to {
        return Err("Error: the script already has that name".to_string());
    }
    match repository::rename_script(uri, to) {
        Ok(()) => {}
        Err(repository::RenameError::NotFound) => {
            return Err(format!("Script not found: {}", uri));
        }
        Err(repository::RenameError::Taken) => {
            return Err(format!("Script already exists: {}", to));
        }
        Err(repository::RenameError::Storage(e)) => {
            return Err(format!("Failed to rename: {}", e));
        }
    }
    info!(user_id = ?user.user_id, from = %uri, to = %to, "Script renamed");
    broadcast_script_update(uri, "deleted", &[("renamedTo", json!(to))]);
    broadcast_script_update(to, "renamed", &[("renamedFrom", json!(uri))]);
    spawn_script_init(to.to_string());
    Ok(())
}

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

    // The capability says the caller deletes scripts; ownership says *which*.
    // Editors hold `DeleteScripts` for their own work, so without this an
    // editor could delete every script in the engine.
    if !is_admin_or_owner(user, uri) {
        let auditor = auditor();
        let user_id = user.user_id.clone();
        tokio::task::spawn(async move {
            let _ = auditor
                .log_authz_failure(
                    user_id,
                    "script".to_string(),
                    "delete".to_string(),
                    "AdministerEngine".to_string(),
                )
                .await;
        });
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            "Permission denied: user is neither admin nor owner"
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

/// Read a script's entrypoint, whole or scoped to a line range and/or a
/// pattern.
///
/// [`read_file_authorized`] aimed at whichever file of the tree is the root,
/// for `/engine/read_file`, whose caller names a script rather than a
/// path.
///
/// Every read reports the digest of the whole file, not of the part returned,
/// because that digest is what a following patch has to send to prove it
/// edited the version it read.
pub fn read_script_authorized(
    user: &UserContext,
    uri: &str,
    options: &FileReadOptions,
) -> Result<FileRead, FileReadError> {
    let path =
        crate::module_loader::root_module_path_in(uri, &crate::source_view::SourceView::Live)
            .map_err(|_| FileReadError::NotFound)?;
    read_file_authorized(user, uri, &path, options)
}

/// Whether `user` may read one file of `script_uri`'s tree.
///
/// The two halves of a script used to be read under two different rules, and
/// merging the storage must not quietly pick one of them. The entrypoint is
/// the script's source and is read under `ReadScripts`, which asks nothing
/// about ownership — a solution's code has always been readable by any reader.
/// Every other file is an asset and takes `ReadAssets` *plus* ownership.
/// Collapsing to the first rule would publish every script's private files;
/// collapsing to the second would make source unreadable to anyone but its
/// owner.
fn can_read_file(user: &UserContext, script_uri: &str, path: &str) -> bool {
    if crate::module_loader::is_root_module_name(path) {
        may_administer(user) && user.require_capability(&Capability::ReadScripts).is_ok()
    } else {
        can_access_assets(user, script_uri, &Capability::ReadAssets)
    }
}

/// Read one file of a script's tree — its entrypoint or any other.
///
/// Head, not what the script serves: this read is the first half of an edit,
/// and its digest is what the following patch sends back as `base_sha256`. A
/// pinned script serves an older revision, and reading that one would have the
/// caller editing a version its write cannot land on. `/engine/get_deployment` is
/// where a caller asks what is being served.
pub fn read_file_authorized(
    user: &UserContext,
    script_uri: &str,
    path: &str,
    options: &FileReadOptions,
) -> Result<FileRead, FileReadError> {
    if !can_read_file(user, script_uri, path) {
        return Err(FileReadError::AccessDenied);
    }
    let Some(file) = repository::fetch_asset(script_uri, path) else {
        return Err(FileReadError::NotFound);
    };

    let sha256 = sha256_hex(&file.content);
    let bytes = file.content.len();

    if !options.is_scoped() {
        // Text when the bytes are text, base64 otherwise — the rule
        // `resources/read` already applies, and the rule a *scoped* read of
        // any file has always applied. The unscoped read was the one place
        // the two halves disagreed: a root came back as text because it is a
        // program, and every other file came back as base64 because an asset
        // is anything at all. That made a module answer in base64 whole and
        // in text by the line.
        return Ok(FileRead {
            view: match String::from_utf8(file.content.clone()) {
                Ok(content) => FileView::Whole { content },
                Err(_) => FileView::Full {
                    content_base64: base64::engine::general_purpose::STANDARD.encode(&file.content),
                },
            },
            sha256,
            bytes,
            total_lines: None,
        });
    }

    let (view, total_lines) = scoped_view(&file.content, options, path, "File")?;
    Ok(FileRead {
        view,
        sha256,
        bytes,
        total_lines: Some(total_lines),
    })
}

/// List script metadata; an authenticated caller with the ReadScripts
/// capability (empty otherwise).
pub fn list_scripts_authorized(user: &UserContext) -> Vec<repository::ScriptMetadata> {
    if !may_administer(user) || user.require_capability(&Capability::ReadScripts).is_err() {
        return Vec::new();
    }
    repository::get_all_script_metadata().unwrap_or_default()
}

/// Which of a script's files a search reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// Root sources and assets alike — what "search my code" means, since a
    /// solution's code is mostly its modules.
    All,
    /// Root sources only: what this search did before it could read assets.
    Scripts,
    /// Assets only.
    Assets,
}

impl SearchScope {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("all") => Ok(SearchScope::All),
            Some("scripts") => Ok(SearchScope::Scripts),
            Some("assets") => Ok(SearchScope::Assets),
            Some(other) => Err(format!(
                "Invalid scope '{}': expected 'all', 'scripts' or 'assets'",
                other
            )),
        }
    }

    fn reads_scripts(self) -> bool {
        self != SearchScope::Assets
    }

    fn reads_assets(self) -> bool {
        self != SearchScope::Scripts
    }
}

/// How a search across a deployment's files is narrowed.
pub struct SearchOptions {
    pub case_insensitive: bool,
    pub scope: SearchScope,
    /// One script's files rather than every script's.
    pub script: Option<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            // The default this search has always had. A caller looking for a
            // symbol usually does not know how it is capitalised.
            case_insensitive: true,
            scope: SearchScope::All,
            script: None,
        }
    }
}

/// How many files one search reports before it stops looking.
pub const MAX_SEARCH_FILES: usize = 200;

/// How many matching lines a search reports per file.
pub const MAX_SEARCH_MATCHES_PER_FILE: usize = 50;

/// Search a deployment's files for a pattern.
///
/// The counterpart of `read_asset`'s `grep`, which searches one named file: this
/// is for the caller that does not yet know which file to name. It read only
/// root sources, which is the smaller half of a solution — the modules are
/// where the code is — so "which file mentions `movePlayer`" meant listing every
/// script's assets and fetching each one.
///
/// A binary asset is skipped rather than refused: a search across a tree that
/// happens to contain a PNG is a reasonable thing to ask for, and failing the
/// whole search because of it would not be.
pub fn search_files_authorized(
    user: &UserContext,
    pattern: &str,
    options: &SearchOptions,
) -> Result<Value, String> {
    if pattern.is_empty() {
        return Err("Empty search pattern: there is nothing to search for".to_string());
    }
    if pattern.chars().count() > MAX_GREP_PATTERN_CHARS {
        return Err(format!(
            "Search pattern too long (max {} characters)",
            MAX_GREP_PATTERN_CHARS
        ));
    }
    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(options.case_insensitive)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| format!("Invalid search pattern: {}", e))?;

    let matches_in = |text: &str| -> Vec<Value> {
        text.lines()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
            .take(MAX_SEARCH_MATCHES_PER_FILE)
            .map(|(index, line)| {
                json!({
                    "line": index + 1,
                    "content": line.trim(),
                    "preview": line.chars().take(200).collect::<String>(),
                })
            })
            .collect()
    };

    let mut results: Vec<Value> = Vec::new();
    let mut truncated = false;
    for meta in list_scripts_authorized(user) {
        if options
            .script
            .as_deref()
            .is_some_and(|script| script != meta.uri)
        {
            continue;
        }
        if truncated {
            break;
        }

        // Reading a script's assets through `/engine/*` is a permission of its
        // own, so a caller who may list the scripts is still asked for it
        // before their modules are searched. The entrypoint is not one of
        // those: it is the script's source, and reading it is what listing the
        // script already granted.
        let may_read_assets = options.scope.reads_assets()
            && can_access_assets(user, &meta.uri, &Capability::ReadAssets);

        // One pass over the tree. This used to be two — the root read off the
        // metadata, the modules read from the asset rows — which is why a
        // search over a merged tree reported the entrypoint twice.
        let mut files: Vec<(String, Vec<u8>)> = repository::fetch_assets(&meta.uri)
            .into_iter()
            .map(|(name, asset)| (name, asset.content))
            .collect();
        // A stable order, so the same search reports the same list twice.
        // `fetch_assets` hands back a map, and the root used to come first
        // because it came from somewhere else entirely.
        files.sort_by(|(left, _), (right, _)| {
            let rank = |name: &str| u8::from(!crate::module_loader::is_root_module_name(name));
            rank(left).cmp(&rank(right)).then_with(|| left.cmp(right))
        });

        for (name, content) in files {
            if results.len() >= MAX_SEARCH_FILES {
                truncated = true;
                break;
            }
            let allowed = if crate::module_loader::is_root_module_name(&name) {
                options.scope.reads_scripts()
            } else {
                may_read_assets
            };
            if !allowed {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&content) else {
                continue;
            };
            let matches = matches_in(text);
            if !matches.is_empty() {
                results.push(json!({
                    "uri": meta.uri,
                    "asset": name,
                    "matchCount": matches.len(),
                    "matches": matches,
                }));
            }
        }
    }

    Ok(json!({
        "query": pattern,
        "caseInsensitive": options.case_insensitive,
        "filesMatched": results.len(),
        "truncated": truncated,
        "results": results,
        "timestamp": iso_timestamp(),
    }))
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
        "revision": entry.context.revision,
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

/// Clear one script's logs; `DeleteLogs` and ownership of the script, or an
/// administrator.
///
/// Naming the script is required. This used to accept no `uri` as "prune every
/// script back to its newest entries", which is now what the background pruner
/// does on its own schedule — and which, reachable here, let anyone holding
/// the editor-tier `DeleteLogs` truncate the logs of every script in the
/// engine, on hosts they had nothing to do with. Acting on what you do not own
/// is what `AdministerEngine` marks.
pub fn delete_logs_authorized(user: &UserContext, uri: &str) -> AppResult<Value> {
    user.require_capability(&Capability::DeleteLogs)?;
    if !is_admin_or_owner(user, uri) {
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            "Permission denied: only an administrator or owner may clear a script's logs"
        );
        return Err(crate::error::AppError::AuthorizationFailed {
            message: format!(
                "You must be an administrator or owner to clear the logs of script '{}'",
                uri
            ),
        });
    }
    repository::clear_log_messages(uri)?;
    Ok(json!({
        "uri": uri,
        "cleared": true,
        "timestamp": iso_timestamp(),
    }))
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
/// Deliberately stricter than [`has_admin_capability`], which accepts any
/// holder of `AdministerEngine` however it was obtained. Requiring
/// authentication as well means only a `UserContext::admin` passes, and that is
/// built solely from a session whose `is_admin` flag is set (`lib.rs`), for both
/// the HTTP and MCP entry points — so no future widening of what an anonymous
/// caller holds can reach the user directory or role changes.
fn is_user_admin(user: &UserContext) -> bool {
    user.is_authenticated && user.has_capability(&Capability::AdministerEngine)
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
        "realm": user.realm,
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

/// Move a user into a realm; administrators only. Returns the realm as stored.
///
/// The realm is the host an account is a principal on, and `*` means every
/// host. No sign-in path produces `*` — an account anyone can create must not
/// reach every host by existing — so this is how an administrator who needs to
/// work across hosts gets there, and it is deliberately a separate act from
/// granting a role.
///
/// Takes effect immediately: sessions carry the realm they were minted with, so
/// the ones the account already holds are ended by the write.
pub fn set_user_realm_authorized(
    actor: &UserContext,
    user_id: &str,
    realm: &str,
) -> Result<String, UserAdminError> {
    if !is_user_admin(actor) {
        audit_user_admin_denied(actor, "set_realm");
        return Err(UserAdminError::AccessDenied);
    }
    // Establish the user exists before mutating, so a typo'd id reads as 404.
    lookup_user(user_id)?;

    crate::user_repository::set_user_realm(user_id, realm)
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))?;
    audit_role_change(actor, user_id, realm, "set_realm");

    Ok(lookup_user(user_id)?.realm)
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
        && (user.has_capability(&Capability::AdministerEngine)
            || (user.has_capability(capability) && user_owns_script(user, script_uri)))
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

/// How a read of one file is scoped — one of a script's assets, or its root
/// source. The two filters compose: a `grep` inside a `lines` range searches
/// only that range.
#[derive(Default)]
pub struct FileReadOptions {
    pub lines: Option<LineRange>,
    pub grep: Option<String>,
}

impl FileReadOptions {
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
pub enum FileView {
    Full {
        content_base64: String,
    },
    /// The whole file as text. What a read of a script's root source answers
    /// with: it is a program, so there is nothing base64 would be protecting.
    Whole {
        content: String,
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

/// One file as read, whole or scoped.
pub struct FileRead {
    pub view: FileView,
    /// Digest of the whole stored file, whichever part of it was returned —
    /// this is what a following patch sends as `base_sha256`.
    pub sha256: String,
    /// Size of the whole stored file, in bytes.
    pub bytes: usize,
    /// Line count of the whole file; absent from a whole-file read, which
    /// does not require the content to be text at all.
    pub total_lines: Option<usize>,
}

impl FileRead {
    /// The response fields for this read, to be merged with the file the
    /// caller named.
    pub fn to_json(&self) -> Value {
        let mut body = match &self.view {
            // Said rather than implied. One `read_file` answers with either
            // spelling depending on whether the bytes are text, so a caller
            // branches on a field that is present rather than on one that is
            // absent.
            FileView::Full { content_base64 } => {
                json!({ "encoding": "base64", "content": content_base64 })
            }
            FileView::Whole { content } => json!({ "encoding": "utf8", "content": content }),
            FileView::Range {
                content,
                start_line,
                end_line,
            } => json!({
                "encoding": "utf8",
                "content": content,
                "start_line": start_line,
                "end_line": end_line,
            }),
            FileView::Matches { matches, truncated } => json!({
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

pub enum FileReadError {
    AccessDenied,
    NotFound,
    /// The read asked for a text view of something that is not text, or asked
    /// for it in a way that does not parse.
    Validation(String),
}

/// Compile a `grep=` pattern, with the bounds a request-time regex needs.
fn compile_grep(pattern: &str) -> Result<regex::Regex, FileReadError> {
    if pattern.is_empty() {
        return Err(FileReadError::Validation(
            "Empty grep pattern: there is nothing to search for".to_string(),
        ));
    }
    if pattern.chars().count() > MAX_GREP_PATTERN_CHARS {
        return Err(FileReadError::Validation(format!(
            "grep pattern too long (max {} characters)",
            MAX_GREP_PATTERN_CHARS
        )));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| FileReadError::Validation(format!("Invalid grep pattern: {}", e)))
}

/// Cut a line down to what a match listing echoes back, on a character
/// boundary. Reports whether anything was cut.
fn truncate_chars(line: &str, max: usize) -> (String, bool) {
    match line.char_indices().nth(max) {
        Some((index, _)) => (line[..index].to_string(), true),
        None => (line.to_string(), false),
    }
}

/// The part of a file a scoped read asked for, and how many lines the whole of
/// it has.
///
/// Shared by every read that can be scoped, because slicing a file and
/// searching it are the same operations whether the file is one of a script's
/// assets or the script's own root source. `file` and `kind` are quoted back
/// in the refusals — the only part that differs between them.
fn scoped_view(
    content: &[u8],
    options: &FileReadOptions,
    file: &str,
    kind: &str,
) -> Result<(FileView, usize), FileReadError> {
    let text = std::str::from_utf8(content).map_err(|_| {
        FileReadError::Validation(format!(
            "{} '{}' is not UTF-8 text, so it has no lines to read: fetch it without \
             'lines' or 'grep' to get its bytes",
            kind, file
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
        return Err(FileReadError::Validation(format!(
            "{} '{}' has {} lines, so there is no line {} to read",
            kind,
            file,
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
        None => FileView::Range {
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
            FileView::Matches { matches, truncated }
        }
    };

    Ok((view, lines.len()))
}

#[derive(Debug)]
pub enum AssetWriteError {
    /// The write was to create a file and the file is already there. Its own
    /// variant rather than a validation message, because the caller's next
    /// move is different: this one is told what it asked to be told.
    Exists(String),
    /// Carries why, for the same reason [`PatchError::AccessDenied`] does: a
    /// write covering a script's root and its assets can be refused by either
    /// rule, and "you do not own this script" is not the message an asset
    /// write would have given.
    AccessDenied(String),
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
pub(crate) fn mimetype_for(asset_uri: &str) -> &'static str {
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
    write_asset_authorized(user, script_uri, asset_uri, mimetype, content_b64, false)
}

/// [`upsert_asset_authorized`], with the option of refusing to overwrite.
///
/// `create_file` could say "this is a new script, fail if it is not" and the
/// asset write could not, so creating a module and overwriting somebody's was
/// the same request. `if_absent` is that distinction: it is a precondition on
/// the write rather than a check before it, since a caller who reads first and
/// writes second has a window in which the answer changes.
pub fn write_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    mimetype: &str,
    content_b64: &str,
    if_absent: bool,
) -> Result<Option<i32>, AssetWriteError> {
    let content = base64::engine::general_purpose::STANDARD
        .decode(content_b64)
        .map_err(|e| {
            AssetWriteError::Validation(format!("Error decoding base64 content: {}", e))
        })?;
    write_file_bytes_authorized(user, script_uri, asset_uri, mimetype, content, if_absent)
}

/// Write one file of a script's tree, whole, given the bytes rather than a
/// spelling of them.
///
/// The transfer encoding is the request's business, and a module arrives as
/// text: `write_file` and `create_file` take `text`, and so does the batch.
///
/// The entrypoint differs from the rest in three ways, and all three are
/// here rather than in a second copy of this function:
///
/// - It takes `WriteScripts` and ownership rather than `WriteAssets`, the
///   line the patch and the delete also draw.
/// - Writing it can *create* the script, because a script is brought into
///   being by its program. Every file references the `scripts` row, so that
///   row has to exist first; a write of any other file to a script that is
///   not there is a write with nothing to belong to.
/// - It meets the 1MB source ceiling rather than the 10MB file one.
pub fn write_file_bytes_authorized(
    user: &UserContext,
    script_uri: &str,
    path: &str,
    mimetype: &str,
    content: Vec<u8>,
    if_absent: bool,
) -> Result<Option<i32>, AssetWriteError> {
    let is_root = crate::module_loader::is_root_module_name(path);

    validate_asset_uri(path)?;
    if if_absent && repository::fetch_asset(script_uri, path).is_some() {
        return Err(AssetWriteError::Exists(path.to_string()));
    }
    let ceiling = if is_root {
        repository::MAX_SCRIPT_CONTENT_BYTES
    } else {
        MAX_ASSET_BYTES
    };
    if content.len() > ceiling {
        return Err(AssetWriteError::Validation(format!(
            "'{}' too large: {} bytes (max {})",
            path,
            content.len(),
            ceiling
        )));
    }

    // Writing the entrypoint *is* writing the script, so it goes the way
    // writing a script goes: the same permission, the same ownership on
    // creation, the same revision, the same broadcast to `/engine/script_updates`
    // and the same `init()`. Delegating rather than repeating that is what
    // the merge is for — there is one way to replace a script's program,
    // whichever name the caller reached for.
    if is_root {
        let text = String::from_utf8(content).map_err(|_| {
            AssetWriteError::Validation(format!(
                "'{}' is a script's program, so it has to be text",
                path
            ))
        })?;
        return upsert_root_authorized(user, script_uri, Some(path), &text, Some("file"))
            .map(|(_, revision)| revision)
            .map_err(|message| {
                let message = message
                    .strip_prefix("Error: ")
                    .unwrap_or(&message)
                    .to_string();
                // Its two refusals are not the same answer: a caller who may
                // not write this script is forbidden, and one who sent no
                // content asked for something impossible.
                if message.starts_with("Script name and content cannot be empty") {
                    AssetWriteError::Validation(message)
                } else {
                    AssetWriteError::AccessDenied(message)
                }
            });
    }

    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied("Access denied".to_string()));
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let path_owned = path.to_string();
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
                .with_resource("file".to_string())
                .with_action("upsert_for_uri".to_string())
                .with_detail("uri", &path_owned)
                .with_detail("script_uri", &script_uri_owned)
                .with_detail("content_size", content_len.to_string())
                .with_detail("mimetype", &mimetype_owned),
            )
            .await;
    });

    let now = std::time::SystemTime::now();
    let asset = repository::Asset {
        uri: path.to_string(),
        name: Some(path.to_string()),
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
    /// The file's bytes, already decoded.
    ///
    /// How they arrived is the request's business and not this type's: a
    /// module comes as text and an image as base64, and by here both are
    /// bytes. [`asset_content_from_request`] is where that choice is made,
    /// once, for the endpoint and the tool alike.
    pub content: Vec<u8>,
    /// Digest the caller believes the bytes have, lowercase hex. When present
    /// it is checked before anything is written.
    pub expected_sha256: Option<String>,
}

/// The bytes of one file of a batch, from whichever field carried them.
///
/// A batch required base64 for every file, including the modules — which are
/// text, are written as text, and are checked as text: `/engine/check_script` takes
/// candidate modules as plain source and says why ("a module the bundler can
/// read has to be UTF-8 anyway"). So the request that *described* a change and
/// the request that *applied* it, which are meant to be the same shape,
/// disagreed about the one field that carries the code. Encoding it cost a
/// third of the bytes and a step no agent can do reliably in its head, which
/// is what drove callers back to writing one file per request — and back to
/// the partial deployments a batch exists to prevent.
///
/// Exactly one of the two fields, because a file whose two spellings disagree
/// has no right answer and guessing which was meant is worse than refusing.
fn asset_content_from_request(
    label: &str,
    base64_field: &str,
    text: Option<String>,
    encoded: Option<String>,
) -> Result<Vec<u8>, String> {
    match (text, encoded) {
        (Some(_), Some(_)) => Err(format!(
            "{}: give either 'text' or '{}', not both",
            label, base64_field
        )),
        (Some(text), None) => Ok(text.into_bytes()),
        (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .map_err(|e| format!("{}: error decoding base64 content: {}", label, e)),
        (None, None) => Err(format!(
            "{}: missing required field: text (or {} for a file that is not text)",
            label, base64_field
        )),
    }
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
    /// How many of the requested removals actually removed something. A sync
    /// naming a file the script no longer has is not an error; it is a sync
    /// that has already happened.
    pub deleted: usize,
}

/// What a sync wants from the asset write that a plain batch does not.
pub struct AssetSyncOptions<'a> {
    /// Asset paths that must not survive this write.
    ///
    /// A pull is a *sync* rather than an append: a module deleted upstream has
    /// to go here too, or the script keeps building against a file its source
    /// of truth no longer holds.
    pub delete: &'a [String],
    /// How the resulting revision describes where it came from.
    pub origin: revisions::Origin,
    /// Ceiling on this write's total content.
    ///
    /// A parameter because [`MAX_BATCH_BYTES`] bounds an HTTP *request body*,
    /// and a sync the engine started on its own has no request body to bound.
    pub max_total_bytes: usize,
    /// Ceiling on how many files this write may carry, for the same reason.
    pub max_files: usize,
    /// Whether this write records a revision of its own.
    ///
    /// False for a caller whose write is one part of a larger change. A pull
    /// writes a script's root and then its assets, and both belong to one
    /// revision: letting the asset write record its own would describe the
    /// change as two, and would miss a pull that only altered the root — a
    /// script consisting of nothing but `main.ts` writes no assets at all.
    pub record_revision: bool,
}

impl Default for AssetSyncOptions<'_> {
    /// What an HTTP batch write asks for: no removals, and the ceilings that
    /// bound a request.
    fn default() -> Self {
        Self {
            delete: &[],
            origin: revisions::Origin::Batch,
            max_total_bytes: MAX_BATCH_BYTES,
            max_files: MAX_BATCH_FILES,
            record_revision: true,
        }
    }
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
    upsert_assets_synced(user, script_uri, files, AssetSyncOptions::default())
}

/// [`upsert_assets_authorized`], for a caller replacing a script's tree rather
/// than adding to it.
///
/// One implementation rather than two, because the difference between a batch
/// write and a sync is three parameters and not a different set of rules. A
/// second write path would have to reimplement the ownership check, the digest
/// comparison that drops unchanged files, and the audit event — and one arm of
/// that would drift.
pub fn upsert_assets_synced(
    user: &UserContext,
    script_uri: &str,
    files: &[AssetWrite],
    options: AssetSyncOptions<'_>,
) -> Result<BatchWriteOutcome, AssetWriteError> {
    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied("Access denied".to_string()));
    }
    // A removal that takes the entrypoint with it takes what removing a script
    // takes, the same line [`delete_asset_authorized`] draws. A change that
    // also writes a root is a rename rather than a removal — `main.js` out,
    // `main.ts` in — and the caller writing the new one has already been
    // asked for script-write rights by the time this runs.
    if options.delete.iter().any(|path| {
        crate::module_loader::is_root_module_name(path)
            && !files
                .iter()
                .any(|file| crate::module_loader::is_root_module_name(&file.name))
    }) && !can_access_assets(user, script_uri, &Capability::DeleteScripts)
    {
        return Err(AssetWriteError::AccessDenied(
            "Removing a script's entrypoint takes the right to delete the script".to_string(),
        ));
    }
    if files.is_empty() && options.delete.is_empty() {
        return Err(AssetWriteError::Validation(
            "No files to write: 'files' must contain at least one entry".to_string(),
        ));
    }
    if files.len() > options.max_files {
        return Err(AssetWriteError::Validation(format!(
            "Too many files in one batch: {} (max {})",
            files.len(),
            options.max_files
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

        // Decoded at the request boundary, where the caller's spelling of it
        // is still visible; by here a file is bytes however it arrived.
        let content = file.content.clone();

        if content.len() > MAX_ASSET_BYTES {
            return Err(AssetWriteError::Validation(format!(
                "Asset '{}' too large (max 10MB)",
                file.name
            )));
        }
        total_bytes = total_bytes.saturating_add(content.len());
        if total_bytes > options.max_total_bytes {
            return Err(AssetWriteError::Validation(format!(
                "Batch too large: over {} bytes of content in one request",
                options.max_total_bytes
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

    // Writes and removals in one transaction, so the tree either becomes what
    // the caller described or stays exactly as it was. Doing the two in
    // sequence left a window where a process dying between them kept files the
    // caller had asked to drop.
    let written = to_write.len();
    let deleted = if written > 0 || !options.delete.is_empty() {
        repository::sync_assets(script_uri, to_write, options.delete.to_vec())
            .map_err(|e| AssetWriteError::Storage(format!("Error writing assets: {}", e)))?
    } else {
        0
    };

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

    // A write that changed nothing left the script exactly as the previous
    // revision already describes, so there is no new state to record.
    let revision = (options.record_revision && (written > 0 || deleted > 0))
        .then(|| revisions::record_blocking(script_uri, options.origin, user.user_id.as_deref()))
        .flatten();

    Ok(BatchWriteOutcome {
        results,
        revision,
        written,
        deleted,
    })
}

/// One change to a script's files, and whatever the change removes.
///
/// There is no separate root here, and that is the point. A script's
/// entrypoint is the file named `main.*` in its tree, so writing it is one
/// entry of `writes` like any other — which is what lets a change that
/// rewrites the entrypoint and two of its modules be one request, one
/// transaction and one revision, rather than a batch with a special case
/// bolted to the front of it.
pub struct ScriptFilesChange<'a> {
    pub writes: &'a [AssetWrite],
    /// Asset paths that must not survive this change.
    pub delete: &'a [String],
}

/// What a change to a script's files needs beyond the files themselves.
///
/// Deliberately not [`AssetSyncOptions`], which also carries the removals and
/// whether to record a revision: a change already names its own removals, and
/// recording the revision is what [`write_script_files_authorized`] is for, so
/// taking those here would be taking two answers to each question.
pub struct ScriptWriteOptions {
    /// How the resulting revision describes where the change came from.
    pub origin: revisions::Origin,
    /// Ceiling on this change's total asset content.
    pub max_total_bytes: usize,
    /// Ceiling on how many assets it may carry.
    pub max_files: usize,
}

impl Default for ScriptWriteOptions {
    /// What a change arriving over HTTP or MCP asks for: the ceilings that
    /// bound a request, and a history that calls it a batch.
    fn default() -> Self {
        Self {
            origin: revisions::Origin::Batch,
            max_total_bytes: MAX_BATCH_BYTES,
            max_files: MAX_BATCH_FILES,
        }
    }
}

/// What a change to a script's files did.
pub struct ScriptFilesOutcome {
    /// Whether the change stored a new entrypoint.
    ///
    /// Derived from the per-file results rather than tracked separately: the
    /// root is one of the files, so this is a question about the list.
    pub root_changed: bool,
    /// `inserted` when the change created the script, `updated` when it did
    /// not, and `None` when the change carried no root and so could not have
    /// created one.
    pub action: Option<UpsertAction>,
    pub assets: BatchWriteOutcome,
    /// The one revision describing everything this change did, or `None` when
    /// it changed nothing.
    pub revision: Option<i32>,
}

impl ScriptFilesOutcome {
    /// Whether anything reached storage — which is also whether re-initializing
    /// the script afterwards would be anything but cost.
    pub fn changed(&self) -> bool {
        self.assets.written > 0 || self.assets.deleted > 0
    }
}

/// Fold a caller's `content` argument into `writes` as the script's root file.
///
/// `content` is the older spelling of "and also the entrypoint", from when the
/// root was a column and a batch could not carry it as a file. It is kept
/// because it is what callers send, and it is sugar now rather than a second
/// write path: the text becomes one more entry of the batch, named after
/// whichever root file the script already has, or after the name a first write
/// would give it.
///
/// A caller who sends `content` *and* names a root file in `files` has
/// described the entrypoint twice, and the batch refuses the duplicate rather
/// than picking one.
fn with_root_content(writes: &mut Vec<AssetWrite>, script_uri: &str, content: Option<&str>) {
    let Some(content) = content else {
        return;
    };
    let name = crate::module_loader::root_module_path_in(
        script_uri,
        &crate::source_view::SourceView::Live,
    )
    .unwrap_or_else(|_| crate::module_loader::default_root_module_name(script_uri).to_string());
    writes.push(AssetWrite {
        name,
        mimetype: None,
        content: content.as_bytes().to_vec(),
        expected_sha256: None,
    });
}

/// Which of `writes` is the change's root module, if any.
///
/// The first by [`crate::module_loader::ROOT_MODULE_NAMES`] order, which is
/// the rule the tree itself is read by, so a change carrying both a `main.ts`
/// and a `main.js` resolves the same way the resulting tree would.
fn root_write(writes: &[AssetWrite]) -> Option<&AssetWrite> {
    crate::module_loader::ROOT_MODULE_NAMES
        .iter()
        .find_map(|name| writes.iter().find(|write| write.name == *name))
}

/// Write a script's files as one change: one transaction per store, one
/// revision, one `init()`.
///
/// A script's modules were already one unit of change; its root source was
/// not. A change that touches both had to be two writes — two revisions, two
/// cluster notifications, two `init()` runs, and a window in which the
/// deployment is half-changed — even though `/engine/check_script` would check
/// exactly such a change in one request. That asymmetry is what this removes:
/// the request that describes a change is now the request that applies it.
///
/// What remains asymmetric is the script *row*, and only the row. Every file
/// references it, so a change carrying the first file of a script that does
/// not exist yet has to create it first — and creating a script is a
/// different permission from writing to one that is already there, which is
/// why that step is authorized separately and why a change with no root
/// cannot bring a script into being.
///
/// `record_revision: false` on the file write is what keeps the change one
/// revision rather than two, and is why a caller cannot simply call the
/// pieces in sequence: the revision is recorded here, once, after everything
/// has landed. A change that stored nothing records none at all, since the
/// previous revision already describes exactly this content.
pub fn write_script_files_authorized(
    user: &UserContext,
    script_uri: &str,
    change: ScriptFilesChange<'_>,
    options: ScriptWriteOptions,
) -> Result<ScriptFilesOutcome, AssetWriteError> {
    // The script row has to exist before any of its files can, since every
    // file references it. `authorize_script_write` is what decides whether
    // this caller may create or replace this script at all — a stricter
    // question than "may they write its assets", and the one writing a root
    // has always been asked.
    let action = match root_write(change.writes) {
        None => None,
        Some(root) => {
            if root.content.is_empty() {
                return Err(AssetWriteError::Validation(format!(
                    "Script '{}' has no content: '{}' is empty",
                    script_uri, root.name
                )));
            }
            let existed = authorize_script_write(user, script_uri).map_err(|message| {
                let message = message
                    .strip_prefix("Error: ")
                    .unwrap_or(&message)
                    .to_string();
                AssetWriteError::AccessDenied(message)
            })?;
            repository::ensure_script(script_uri, user.user_id.as_deref())
                .map_err(|e| AssetWriteError::Storage(format!("Error storing script: {}", e)))?;
            Some(if existed {
                UpsertAction::Updated
            } else {
                UpsertAction::Inserted
            })
        }
    };

    // The file write refuses an empty batch — right for a caller who sent an
    // empty request, and there is nothing else for this to be now that a root
    // is one of the files.
    let assets = if change.writes.is_empty() && change.delete.is_empty() {
        BatchWriteOutcome {
            results: Vec::new(),
            revision: None,
            written: 0,
            deleted: 0,
        }
    } else {
        upsert_assets_synced(
            user,
            script_uri,
            change.writes,
            AssetSyncOptions {
                delete: change.delete,
                origin: options.origin,
                max_total_bytes: options.max_total_bytes,
                max_files: options.max_files,
                // One revision covers the whole change; it is recorded below,
                // once every file has landed.
                record_revision: false,
            },
        )?
    };

    let root_changed = assets.results.iter().any(|result| {
        crate::module_loader::is_root_module_name(&result.name) && result.status != "unchanged"
    });

    let mut outcome = ScriptFilesOutcome {
        root_changed,
        action,
        assets,
        revision: None,
    };
    if outcome.changed() {
        outcome.revision =
            revisions::record_blocking(script_uri, options.origin, user.user_id.as_deref());
    }
    Ok(outcome)
}

/// One string replacement of a patch — of one of a script's assets, or of its
/// root source. Editing either is the same act on the same kind of content, so
/// both take the same request shape and the same checks.
pub struct StringEdit {
    /// Text to find. It must be present, and unique unless `replace_all`.
    pub old_string: String,
    /// Text to put in its place.
    pub new_string: String,
    /// Replace every occurrence rather than requiring exactly one.
    pub replace_all: bool,
}

/// What a patch did to the file it edited.
pub struct PatchOutcome {
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

impl PatchOutcome {
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

pub enum PatchError {
    /// Carries why, because the two ways to be refused a write are not the
    /// same thing to be told: lacking the capability is a different problem
    /// from holding it and not owning this script.
    AccessDenied(String),
    NotFound,
    Validation(String),
    /// The stored file is not the one the caller edited: `base_sha256` names
    /// content it no longer has.
    Conflict {
        expected: String,
        actual: String,
    },
    Storage(String),
}

/// Apply a patch's edits to `text`, reporting how many occurrences they
/// replaced in total.
///
/// This is the half of a patch that is the same whether the file being edited
/// is one of a script's assets or its root source, so it lives in one place:
/// the arithmetic of finding and replacing, and the rule that keeps an edit
/// aimed by content alone from being a guess. An `old_string` that appears
/// more than once is refused unless the caller said `replace_all`, because an
/// edit meant for one of three identical lines cannot be aimed by content.
///
/// `file` is only ever quoted back in the refusals. It is what a caller needs
/// to know which of the files it sent is the one that did not match.
///
/// The edits are applied to a copy the caller owns, so a patch whose third
/// edit does not match has written nothing anywhere.
fn apply_string_edits(
    text: &mut String,
    edits: &[StringEdit],
    file: &str,
) -> Result<usize, String> {
    if edits.is_empty() {
        return Err("No edits to apply: 'edits' must contain at least one entry".to_string());
    }
    if edits.len() > MAX_PATCH_EDITS {
        return Err(format!(
            "Too many edits in one patch: {} (max {})",
            edits.len(),
            MAX_PATCH_EDITS
        ));
    }

    let mut replacements = 0usize;
    for (index, edit) in edits.iter().enumerate() {
        if edit.old_string.is_empty() {
            return Err(format!("edits[{}]: old_string must not be empty", index));
        }
        if edit.old_string == edit.new_string {
            return Err(format!(
                "edits[{}]: old_string and new_string are identical, so the edit would do nothing",
                index
            ));
        }

        let occurrences = text.matches(edit.old_string.as_str()).count();
        match (occurrences, edit.replace_all) {
            (0, _) => {
                return Err(format!(
                    "edits[{}]: old_string was not found in '{}'{}",
                    index,
                    file,
                    if index > 0 {
                        " as the earlier edits left it"
                    } else {
                        ""
                    }
                ));
            }
            (count, false) if count > 1 => {
                return Err(format!(
                    "edits[{}]: old_string appears {} times in '{}'; include enough surrounding \
                     text to make it unique, or pass replace_all",
                    index, count, file
                ));
            }
            (count, true) => {
                *text = text.replace(edit.old_string.as_str(), &edit.new_string);
                replacements += count;
            }
            (_, false) => {
                *text = text.replacen(edit.old_string.as_str(), &edit.new_string, 1);
                replacements += 1;
            }
        }
    }

    Ok(replacements)
}

/// Delete one file of a script.
pub fn delete_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
) -> Result<(bool, Option<i32>), AssetFetchError> {
    // Removing a script's entrypoint is removing its source, so it takes what
    // removing a script takes. The write side already draws this line —
    // `WriteAssets` is not a way to write a root — and leaving the delete side
    // at `DeleteAssets` would make the tree merge a way around it: what you
    // could not overwrite you could delete.
    let required = if crate::module_loader::is_root_module_name(asset_uri) {
        Capability::DeleteScripts
    } else {
        Capability::DeleteAssets
    };
    if !can_access_assets(user, script_uri, &required) {
        let auditor = auditor();
        let user_id = user.user_id.clone();
        tokio::task::spawn(async move {
            let _ = auditor
                .log_authz_failure(
                    user_id,
                    "asset".to_string(),
                    "delete_for_uri".to_string(),
                    format!("{:?}", required),
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
/// Backs `/engine/list_routes`. Host bindings are not applied here — this is the
/// whole engine's view; callers that care filter by `script_uri` (see
/// [`crate::route_index::script_serves_host`]).
/// What enforcing exposure-by-directory would change.
///
/// Reads the live registries, so it is a report about what this instance is
/// actually publishing rather than about what its scripts' source appears to
/// say. Administrator-only: the list names every file a deployment serves
/// from a directory that says it is private, which is a map of exactly the
/// mistakes worth exploiting.
pub fn exposure_report_authorized(
    user: &UserContext,
) -> AppResult<crate::exposure::ExposureReport> {
    if !may_administer(user) {
        return Err(crate::error::AppError::AuthorizationFailed {
            message: "The exposure report is not open to anonymous callers".to_string(),
        });
    }
    user.require_capability(&Capability::AdministerEngine)?;

    let metadata = repository::get_all_script_metadata()?;
    Ok(crate::exposure::report(&metadata))
}

/// The engine's own streams, which are not registrations of any script.
///
/// A script's streams are indexed with its routes; these have no script
/// behind them and no metadata to be indexed from, so they stay in
/// `stream_registry` — which is where they are registered — and are listed
/// from there.
fn engine_owned_streams() -> Vec<(String, String)> {
    crate::stream_registry::GLOBAL_STREAM_REGISTRY
        .get_all_registrations()
        .into_iter()
        .filter(|(_, script_uri, _)| script_uri.starts_with("engine://"))
        .map(|(path, script_uri, _)| (path, script_uri))
        .collect()
}

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
                // Handlers and file routes come from the same list now, which
                // is what this view always showed them as: the asset half
                // used to be assembled from a registry of its own here.
                let tags = if route_meta.tags.is_empty()
                    && route_meta.kind != repository::RouteKind::Handler
                {
                    vec![route_meta.default_tag().to_string()]
                } else {
                    route_meta.tags.clone()
                };
                all_routes.push(json!({
                    "path": path,
                    "method": method,
                    "handler": route_meta.target(),
                    "script_uri": metadata.uri,
                    "summary": route_meta.summary,
                    "description": route_meta.description,
                    "tags": tags,
                }));
            }
        }
    }

    for (path, script_uri) in engine_owned_streams() {
        all_routes.push(json!({
            "path": path,
            "method": repository::STREAM_METHOD,
            "handler": Value::Null,
            "script_uri": script_uri,
            "summary": Value::Null,
            "description": Value::Null,
            "tags": ["Streams"],
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
            return refuse(
                Refusal::Failed,
                format!("Failed to parse Rust OpenAPI spec: {}", e),
            )
            .to_string();
        }
    };

    let metadata_list = match repository::get_all_script_metadata() {
        Ok(list) => list,
        Err(e) => {
            return refuse(
                Refusal::Failed,
                format!("Failed to fetch JavaScript routes: {}", e),
            )
            .to_string();
        }
    };

    let mut js_paths = serde_json::Map::new();

    // File routes, taken from the same registrations as the handler routes
    // below and rendered separately because what they document is a file
    // rather than an operation. They used to come from a registry of their
    // own; the collect-first is only because the loop below consumes the
    // metadata list.
    let file_routes: Vec<(String, String, String, repository::RouteMetadata)> = metadata_list
        .iter()
        .filter(|metadata| metadata.initialized)
        .flat_map(|metadata| {
            metadata
                .registrations
                .iter()
                .filter(|(_, route_meta)| route_meta.kind == repository::RouteKind::File)
                .map(|((path, _), route_meta)| {
                    (
                        path.clone(),
                        metadata.uri.clone(),
                        route_meta.file.clone().unwrap_or_default(),
                        route_meta.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let stream_routes: Vec<(String, String, repository::RouteMetadata)> = metadata_list
        .iter()
        .filter(|metadata| metadata.initialized)
        .flat_map(|metadata| {
            metadata
                .registrations
                .iter()
                .filter(|(_, route_meta)| route_meta.kind == repository::RouteKind::Stream)
                .map(|((path, _), route_meta)| {
                    (path.clone(), metadata.uri.clone(), route_meta.clone())
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // Script-registered HTTP routes
    for metadata in metadata_list {
        if metadata.initialized && !metadata.registrations.is_empty() {
            for ((path, method), route_meta) in metadata.registrations {
                if route_meta.kind != repository::RouteKind::Handler {
                    continue;
                }
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

    // Asset routes
    for (path, script_uri, asset_name, registration_meta) in file_routes {
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
        let asset_summary = registration_meta
            .summary
            .clone()
            .unwrap_or_else(|| format!("Static asset: {}", asset_name));
        asset_operation.insert("summary".to_string(), json!(asset_summary));
        let asset_description = registration_meta.description.clone().unwrap_or_else(|| {
            format!(
                "Serves static asset '{}' registered by script '{}'",
                asset_name, script_uri
            )
        });
        asset_operation.insert("description".to_string(), json!(asset_description));
        let asset_tags = if registration_meta.tags.is_empty() {
            vec!["Assets".to_string()]
        } else {
            registration_meta.tags.clone()
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
        asset_operation.insert("x-asset-name".to_string(), json!(asset_name));
        asset_operation.insert("x-script-uri".to_string(), json!(script_uri));
        asset_operation.insert("x-source".to_string(), json!("file-route"));

        let path_entry = js_paths.entry(path).or_insert_with(|| json!({}));
        if let Some(path_obj) = path_entry.as_object_mut() {
            path_obj.insert("get".to_string(), json!(asset_operation));
        }
    }

    // SSE stream routes, the scripts' and the engine's own
    let stream_routes: Vec<(String, String, repository::RouteMetadata)> = stream_routes
        .into_iter()
        .chain(
            engine_owned_streams()
                .into_iter()
                .map(|(path, script_uri)| {
                    (path, script_uri, repository::RouteMetadata::stream(None))
                }),
        )
        .collect();
    for (path, script_uri, metadata) in stream_routes {
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
        // The engine's own operations, from the table `/mcp` reads.
        for (path, operations) in crate::engine_http::openapi_paths() {
            rust_paths.insert(path, operations);
        }
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

    // What a caller — or a script written against this engine — may spend, as
    // this deployment is configured. A limit nobody can find is one every
    // caller meets by surprise, and the numbers here are read from the code
    // that enforces them rather than retyped beside it.
    match serde_json::to_value(crate::limits::snapshot()) {
        Ok(limits) => {
            if let Some(spec) = rust_spec.as_object_mut() {
                spec.insert("x-aiwebengine-limits".to_string(), limits);
            }
        }
        Err(e) => warn!(
            "Failed to serialize engine limits for the OpenAPI document: {}",
            e
        ),
    }

    match serde_json::to_string_pretty(&rust_spec) {
        Ok(json) => json,
        Err(e) => refuse(
            Refusal::Failed,
            format!("Failed to serialize merged OpenAPI spec: {}", e),
        )
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

/// Tell a caller editing a pinned script that its write will not serve.
///
/// Reads and edits act on head, and a pinned script serves an older revision,
/// so the write lands somewhere the requests do not look. That is the whole
/// point of pinning and is invisible from an answer that only reports what was
/// written — so the answer says it, in the one case where it is true.
fn deployment_note(script_uri: &str) -> Option<Value> {
    let revision = crate::deployments::pinned(script_uri)?;
    Some(json!({
        "pinnedRevision": revision,
        "note": format!(
            "This script is pinned to revision {}, so it serves that revision and not what is \
             stored. Reads and edits act on the stored files; deploy_script \
             (`follow` to stop pinning) is what changes which revision serves.",
            revision
        ),
    }))
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
    let is_admin = user.has_capability(&Capability::AdministerEngine);
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
        let is_admin = user.has_capability(&Capability::AdministerEngine);
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

/// One file of a candidate change: source that has not been written anywhere.
#[derive(Deserialize)]
pub struct CandidateFile {
    /// Source text. Not base64, unlike the asset write paths: a module the
    /// bundler can read has to be UTF-8 text anyway, so encoding it would buy
    /// nothing and cost the caller a step.
    content: String,
    /// Inferred from the extension when omitted, as it is for a batch write.
    mimetype: Option<String>,
}

/// The files of a candidate change, by path. `null` means the change deletes
/// that file.
type CandidateFiles = std::collections::BTreeMap<String, Option<CandidateFile>>;

/// Build the view a candidate change describes, over `base`.
///
/// The point of the whole thing: a change that spans modules can be checked
/// while it is still a proposal. `content` alone only ever answered for the
/// root, so a change to a schema module and the three modules that read it had
/// to be written — all of it, to the deployment other people are using —
/// before anything could tell you whether it bundled.
///
/// A `null` entry is a deletion, which is as much a part of a change as a
/// rewrite: a check that quietly kept reading a module the change removes
/// would pass on a program that cannot be built once it lands.
fn candidate_overlay(
    files: CandidateFiles,
    base: crate::source_view::SourceView,
) -> Result<(crate::source_view::SourceView, usize), String> {
    use crate::source_view::{OverlayEntry, SourceFile};

    if files.len() > MAX_BATCH_FILES {
        return Err(format!(
            "Too many candidate files: {} (limit {})",
            files.len(),
            MAX_BATCH_FILES
        ));
    }

    let mut total = 0usize;
    let mut entries = std::collections::BTreeMap::new();
    for (path, file) in files {
        validate_asset_uri(&path).map_err(|e| match e {
            AssetWriteError::Validation(message) => message,
            _ => format!("Invalid candidate path '{}'", path),
        })?;

        let entry = match file {
            None => OverlayEntry::Deleted,
            Some(file) => {
                if file.content.len() > MAX_ASSET_BYTES {
                    return Err(format!("Candidate file '{}' is too large", path));
                }
                total = total.saturating_add(file.content.len());
                if total > MAX_BATCH_BYTES {
                    return Err(format!(
                        "Candidate files exceed the {}-byte ceiling",
                        MAX_BATCH_BYTES
                    ));
                }
                let mimetype = file
                    .mimetype
                    .unwrap_or_else(|| mimetype_for(&path).to_string());
                OverlayEntry::Written(SourceFile::text(file.content, mimetype))
            }
        };
        entries.insert(path, entry);
    }

    let count = entries.len();
    Ok((
        crate::source_view::SourceView::overlay_on(base, entries),
        count,
    ))
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
    let is_admin = user.has_capability(&Capability::AdministerEngine);
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
    script: Option<String>,
    level: Option<String>,
    contains: Option<String>,
    request_id: Option<String>,
    kind: Option<String>,
    route: Option<String>,
    revision: Option<i32>,
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
            script_uri: self.script.clone(),
            level: self.level.clone(),
            since: None,
            after_seq: None,
            contains: self.contains.clone(),
            request_id: self.request_id.clone(),
            kind: self.kind.clone(),
            revision: self.revision,
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
        ("script" = Option<String>, Query, description = "Script name; omit to tail every script"),
        ("level" = Option<String>, Query, description = "Only entries at this level, e.g. ERROR"),
        ("contains" = Option<String>, Query, description = "Only entries whose message contains this substring"),
        ("request_id" = Option<String>, Query, description = "Only the entries one invocation emits"),
        ("kind" = Option<String>, Query, description = "Only entries from invocations of this kind, e.g. httpRoute, scheduled"),
        ("route" = Option<String>, Query, description = "Only entries logged while serving this registered route pattern"),
        ("revision" = Option<i32>, Query, description = "Only entries written while this revision of the script was running"),
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

/// One edit of a patch request.
#[derive(Deserialize, Default)]
pub struct StringEditBody {
    old_string: Option<String>,
    new_string: Option<String>,
    #[serde(default)]
    replace_all: bool,
}

/// Turn a request's `edits` into the patch's, or name the entry that is not
/// one.
///
/// Both fields are required rather than defaulted to empty, so a misspelled
/// field name cannot quietly turn a replacement into a deletion — which is the
/// one mistake here that destroys content rather than being refused.
fn prepare_edits(edits: Vec<StringEditBody>) -> Result<Vec<StringEdit>, String> {
    edits
        .into_iter()
        .enumerate()
        .map(|(index, edit)| {
            let old_string = edit
                .old_string
                .ok_or_else(|| format!("edits[{}]: missing required field: old_string", index))?;
            let new_string = edit.new_string.ok_or_else(|| {
                format!(
                    "edits[{}]: missing required field: new_string (pass \"\" to delete the text)",
                    index
                )
            })?;
            Ok(StringEdit {
                old_string,
                new_string,
                replace_all: edit.replace_all,
            })
        })
        .collect()
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
fn read_options(lines: Option<&str>, grep: Option<String>) -> Result<FileReadOptions, String> {
    let lines = match lines {
        Some(raw) => Some(LineRange::parse(raw)?),
        None => None,
    };
    Ok(FileReadOptions { lines, grep })
}

/// The answer a change to a script's files gives, shared by the endpoint and
/// the tool so the two cannot drift.
fn batch_outcome_json(script: &str, outcome: &ScriptFilesOutcome, init: Value) -> Value {
    let mut body = json!({
        "script": script,
        "results": outcome.assets.results.iter().map(AssetWriteOutcome::to_json).collect::<Vec<Value>>(),
        "written": outcome.assets.written,
        "revision": outcome.revision,
        "init": init,
        "timestamp": iso_timestamp(),
    });
    if let Some(object) = body.as_object_mut() {
        // Only reported when the change carried them, so a caller written
        // against the asset-only batch sees exactly the body it always saw.
        if outcome.action.is_some() {
            object.insert(
                "root".to_string(),
                json!(if outcome.root_changed {
                    "updated"
                } else {
                    "unchanged"
                }),
            );
        }
        if outcome.assets.deleted > 0 {
            object.insert("deleted".to_string(), json!(outcome.assets.deleted));
        }
        if let Some(note) = deployment_note(script) {
            object.insert("deployment".to_string(), note);
        }
    }
    body
}

/// Apply a deployment on this instance and tell the rest of the cluster.
///
/// Everything after the pin is what makes it take effect *here*: the source
/// cache holds what a script serves, the prepared program is built from it,
/// and `init()` registers what that version registers. The notification hands
/// the same sequence to every other instance.
async fn activate_deployment(script: &str) -> Value {
    repository::refresh_served_source(script).await;
    crate::module_loader::invalidate(script);
    crate::bytecode::invalidate(script);
    crate::route_index::invalidate();

    let result = reinitialize_script(script).await;
    crate::deployments::record_init(script, result.success, result.error.as_deref()).await;

    // Same channel a write uses. The handler on the other side re-reads the
    // pin, loads the source it now names and re-initialises — which is the
    // whole of what deploying means there too.
    repository::notify_script_changed(script).await;

    init_result_json(&result)
}

fn deployment_to_json(deployment: &crate::deployments::Deployment) -> Value {
    json!({
        "revision": deployment.revision,
        "at": deployment.deployed_at.to_rfc3339(),
        "by": deployment.deployed_by,
        "initOk": deployment.init_ok,
        "initError": deployment.init_error,
    })
}

// ============================================================================
// Per-script execution limits
// ============================================================================

/// What one script may spend. Every field is optional; one left out follows
/// the engine's own setting rather than keeping an earlier override.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(example = json!({ "script": "myapp", "timeoutMs": 60000, "note": "calls a model API" }))]
pub struct SetScriptLimitsBody {
    pub script: Option<String>,
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: Option<u64>,
    #[serde(rename = "jobTimeoutMs")]
    pub job_timeout_ms: Option<u64>,
    #[serde(rename = "maxMemoryBytes")]
    pub max_memory_bytes: Option<u64>,
    pub note: Option<String>,
}

fn script_limits_to_json(limits: &crate::script_limits::ScriptLimits) -> Value {
    json!({
        "script": limits.script_uri,
        "timeoutMs": limits.overrides.timeout_ms,
        "jobTimeoutMs": limits.overrides.job_timeout_ms,
        "maxMemoryBytes": limits.overrides.max_memory_bytes,
        "note": limits.note,
        "setBy": limits.set_by,
        "updatedAt": limits.updated_at.to_rfc3339(),
    })
}

// ============================================================================
// Script tasks
// ============================================================================

fn file_diff_to_json(file: &revisions::FileDiff) -> Value {
    json!({
        "uri": file.uri,
        "status": file.status,
        "from": file.from_sha256,
        "to": file.to_sha256,
        "diff": file.diff,
        "note": file.note,
    })
}

/// Whether `user` may read a script's history.
///
/// A revision covers the script's root source as well as its files, so reading
/// one is reading both — hence both capabilities rather than the asset one
/// alone. Every built-in capability set grants them together, which is why
/// this changes nothing today; requiring both is what keeps that coincidence
/// from being the reason it is safe.
fn can_read_history(user: &UserContext, script_uri: &str) -> bool {
    can_access_assets(user, script_uri, &Capability::ReadAssets)
        && can_access_assets(user, script_uri, &Capability::ReadScripts)
}

// ============================================================================
// Git sync
// ============================================================================

/// What to pull, and from where.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(example = json!({ "repo": "octocat/hello-world" }))]
pub struct GitPullBody {
    /// The repository, as `owner/repo` or as any GitHub URL naming it —
    /// `octocat/hello-world`, `https://github.com/octocat/hello-world`, or the
    /// `.git` clone URL. Required.
    #[schema(example = "octocat/hello-world")]
    pub repo: Option<String>,

    /// Branch to read. Omit to use whichever branch the repository calls its
    /// default.
    #[schema(example = "main")]
    pub branch: Option<String>,

    /// URI prefix the pulled scripts land under, appended to this engine's own
    /// origin. Defaults to the repository name. Set it when two repositories
    /// would otherwise collide, or to place a solution somewhere specific.
    #[schema(example = "examples")]
    pub prefix: Option<String>,

    /// Download and re-apply even when the repository has not moved since the
    /// last pull. Off by default, because an unchanged repository is normally
    /// nothing to do.
    #[serde(default)]
    pub force: bool,
}

fn pull_report_json(report: &crate::git_sync::PullReport) -> Value {
    json!({
        "success": true,
        "repo": report.repo,
        "branch": report.branch,
        "commit": report.commit,
        "upToDate": report.up_to_date,
        "scripts": report.scripts.iter().map(|script| json!({
            "script": script.script_uri,
            "source": script.source,
            "action": script.action,
            "changed": script.changed,
            "written": script.written,
            "deleted": script.deleted,
            "unchanged": script.unchanged,
            "revision": script.revision,
            "init": script.init,
        })).collect::<Vec<Value>>(),
        "timestamp": iso_timestamp(),
    })
}

/// A personal access token to store for a git host.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(example = json!({ "token": "github_pat_11ABCDEFG..." }))]
pub struct GitCredentialBody {
    /// The token. A fine-grained token scoped to the repositories you want is
    /// enough; it needs no more than read access to their contents. Required.
    ///
    /// It is encrypted at rest and is never returned by this or any other
    /// endpoint.
    pub token: Option<String>,

    /// Git host the token authenticates against. Defaults to `github.com`,
    /// which is the only host supported.
    #[schema(example = "github.com")]
    pub host: Option<String>,
}

fn credential_json(summary: &crate::git_credentials::CredentialSummary) -> Value {
    json!({
        "host": summary.remote_host,
        "account": summary.account,
        "createdAt": summary.created_at.to_rfc3339(),
        "updatedAt": summary.updated_at.to_rfc3339(),
        "lastUsedAt": summary.last_used_at.map(|t| t.to_rfc3339()),
    })
}

#[derive(Deserialize, Default, utoipa::IntoParams)]
pub struct GitHostQuery {
    /// Git host to act on. Defaults to `github.com`.
    pub host: Option<String>,
}

fn tool_set_git_credential(args: &Value, user: &UserContext) -> Value {
    let Some(token) = arg_str(args, "token") else {
        return missing_arg("token");
    };
    if !user.has_capability(&Capability::WriteScripts) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    let Some(user_id) = user.user_id.clone() else {
        return refuse(
            Refusal::BadRequest,
            "A git credential belongs to an account, and this request has none",
        );
    };
    let host = arg_str(args, "host").unwrap_or(crate::git_github::HOST);
    if host != crate::git_github::HOST {
        return refuse(
            Refusal::BadRequest,
            format!("Only {} is supported", crate::git_github::HOST),
        );
    }
    if !crate::config::git_config().allows(host) {
        return refuse(
            Refusal::BadRequest,
            format!("This engine is not configured to read from {}", host),
        );
    }

    let account = match crate::git_github::GitHubClient::new()
        .map(|client| client.with_token(Some(token.to_string())))
        .and_then(|client| client.verify_token())
    {
        Ok(account) => account,
        Err(e) => return refuse(Refusal::Failed, e.to_string()),
    };

    match crate::database::run_blocking(crate::git_credentials::store(
        &user_id,
        host,
        token,
        Some(&account),
    )) {
        Ok(()) => json!({
            "success": true,
            "host": host,
            "account": account,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

fn tool_list_git_credentials(_args: &Value, user: &UserContext) -> Value {
    if !user.has_capability(&Capability::WriteScripts) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    let Some(user_id) = user.user_id.clone() else {
        return json!({ "credentials": [] });
    };
    match crate::database::run_blocking(crate::git_credentials::list(&user_id)) {
        Ok(credentials) => json!({
            "credentials": credentials.iter().map(credential_json).collect::<Vec<Value>>(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

fn tool_delete_git_credential(args: &Value, user: &UserContext) -> Value {
    if !user.has_capability(&Capability::WriteScripts) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    let Some(user_id) = user.user_id.clone() else {
        return refuse(
            Refusal::BadRequest,
            "A git credential belongs to an account, and this request has none",
        );
    };
    let host = arg_str(args, "host").unwrap_or(crate::git_github::HOST);
    match crate::database::run_blocking(crate::git_credentials::forget(&user_id, host)) {
        Ok(removed) => json!({
            "success": true,
            "host": host,
            "removed": removed,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

/// What to publish, and where.
#[derive(Deserialize, Default, utoipa::ToSchema)]
#[schema(example = json!({ "script": "https://example.com/shop.js" }))]
pub struct GitPushBody {
    /// URI of the script to publish. Required.
    #[schema(example = "https://example.com/shop.js")]
    pub script: Option<String>,

    /// Repository to publish to, as `owner/repo` or any GitHub URL naming it.
    /// Optional once the script has been pulled, since it already knows where
    /// it came from.
    #[schema(example = "octocat/hello-world")]
    pub repo: Option<String>,

    /// Branch to write. Defaults to the branch the script was pulled from, or
    /// the repository's default branch on a first publish.
    #[schema(example = "main")]
    pub branch: Option<String>,

    /// Commit message. Defaulted to one naming the script.
    #[schema(example = "Fix the cart total")]
    pub message: Option<String>,

    /// Publish even when this engine believes the repository has moved. GitHub
    /// still refuses a non-fast-forward update, so this cannot overwrite work
    /// it has not seen.
    #[serde(default)]
    pub force: bool,
}

fn push_report_json(report: &crate::git_sync::PushReport) -> Value {
    json!({
        "success": true,
        "script": report.script_uri,
        "repo": report.repo,
        "branch": report.branch,
        "commit": report.commit,
        "upToDate": report.up_to_date,
        "written": report.written,
        "removed": report.removed,
        "timestamp": iso_timestamp(),
    })
}

fn tool_push_to_git(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };

    let request = crate::git_sync::PushRequest {
        script_uri: script.to_string(),
        repo: arg_str(args, "repo").map(str::to_string),
        branch: arg_str(args, "branch").map(str::to_string),
        message: arg_str(args, "message").map(str::to_string),
        force: args.get("force").and_then(Value::as_bool).unwrap_or(false),
    };

    let user = user.clone();
    match crate::database::run_blocking(crate::git_sync::push(&user, request)) {
        Ok(report) => push_report_json(&report),
        Err(e) => refuse_sync(&e),
    }
}

#[derive(Deserialize, Default, utoipa::IntoParams)]
pub struct GitStatusQuery {
    /// URI of the script to report on.
    pub script: Option<String>,
}

fn status_json(status: &crate::git_sync::SyncStatus) -> Value {
    json!({
        "script": status.script_uri,
        "state": status.state.as_str(),
        "action": status.state.advice(),
        "repo": status.remote,
        "branch": status.branch,
        "commit": status.remote_commit,
        "commitAtSync": status.commit_at_sync,
        "revision": status.revision,
        "revisionAtSync": status.revision_at_sync,
        "pinnedRevision": status.pinned,
        "unreachable": status.unreachable,
        "timestamp": iso_timestamp(),
    })
}

fn tool_get_git_status(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_read_history(user, script) {
        return refuse(Refusal::Forbidden, "Access denied");
    }

    let user = user.clone();
    let script = script.to_string();
    match crate::database::run_blocking(
        async move { crate::git_sync::status(&user, &script).await },
    ) {
        Ok(status) => status_json(&status),
        Err(e) => refuse_sync(&e),
    }
}

fn binding_json(binding: &crate::git_sync::Binding) -> Value {
    json!({
        "script": binding.script_uri,
        "repo": binding.remote,
        "branch": binding.branch,
        "commitAtSync": binding.last_commit,
        "syncedAt": binding.synced_at.to_rfc3339(),
        "syncedBy": binding.synced_by,
    })
}

fn tool_list_git_bindings(_args: &Value, user: &UserContext) -> Value {
    match crate::database::run_blocking(crate::git_sync::bindings()) {
        Ok(bindings) => json!({
            "bindings": bindings
                .iter()
                .filter(|binding| can_read_history(user, &binding.script_uri))
                .map(binding_json)
                .collect::<Vec<Value>>(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

fn tool_clear_git_remote(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_write_script(user, script) {
        return refuse(Refusal::Forbidden, "Access denied");
    }
    match crate::database::run_blocking(crate::git_sync::unbind(script)) {
        Ok(removed) => json!({
            "success": true,
            "script": script,
            "removed": removed,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, e.to_string()),
    }
}

fn tool_pull_from_git(args: &Value, user: &UserContext) -> Value {
    let Some(repo) = arg_str(args, "repo") else {
        return missing_arg("repo");
    };

    let request = crate::git_sync::PullRequest {
        repo: repo.to_string(),
        branch: arg_str(args, "branch").map(str::to_string),
        prefix: arg_str(args, "prefix").map(str::to_string),
        force: args.get("force").and_then(Value::as_bool).unwrap_or(false),
    };

    let user = user.clone();
    match crate::database::run_blocking(crate::git_sync::pull(&user, request)) {
        Ok(report) => pull_report_json(&report),
        Err(e) => refuse_sync(&e),
    }
}

/// Whether `user` may change a script's history or restore from it.
///
/// A revert writes the root as readily as it writes a module, so it takes what
/// writing either takes.
fn can_write_history(user: &UserContext, script_uri: &str) -> bool {
    can_access_assets(user, script_uri, &Capability::WriteAssets)
        && can_access_assets(user, script_uri, &Capability::WriteScripts)
}

/// Why a revert was refused.
pub enum RevertRefusal {
    AccessDenied,
    NotFound(String),
    /// The target revision cannot be bundled, so restoring it would deploy a
    /// script that does not build.
    WillNotBuild(String),
    Storage(String),
}

/// What a revert did, or would do.
///
/// The counts are of *assets*. The script's entrypoint is restored alongside
/// them and reported separately, because it is not one of them — it lives in
/// the `scripts` row rather than the asset tree. That distinction is real and
/// it is also a trap: a revert that restored the entrypoint and nothing else
/// reported zero written, which reads as nothing having happened. Hence the
/// names below saying what they count, and `changed_anything` so a caller
/// never has to know the composition to answer the only question it has.
pub struct RevertOutcome {
    pub target: i32,
    pub assets_written: Vec<String>,
    pub assets_deleted: Vec<String>,
    pub entrypoint_changed: bool,
    /// The revision the revert recorded, or `None` for a dry run and for a
    /// revert that found nothing to change.
    pub revision: Option<i32>,
    pub dry_run: bool,
    /// How the script's tables now differ from what the target revision ran
    /// against.
    ///
    /// Advisory and never acted on. A revert restores code; the data that
    /// code's successors wrote is a separate question with a separate answer,
    /// and dropping a column to match old modules would destroy data in order
    /// to restore code.
    pub schema_warnings: Vec<String>,
}

impl RevertOutcome {
    /// Whether the revert moved anything at all.
    ///
    /// The file lists are the whole answer, because the entrypoint is one of
    /// the files they list. They used to be only part of it — a revert that
    /// restored nothing but the root reported two empty lists, and a caller
    /// reading them would have concluded nothing happened.
    pub fn changed_anything(&self) -> bool {
        !self.assets_written.is_empty() || !self.assets_deleted.is_empty()
    }

    fn to_json(&self) -> Value {
        json!({
            "revertedTo": self.target,
            "revision": self.revision,
            "dryRun": self.dry_run,
            "changed": {
                // First, because it is the question every caller has and the
                // only one that cannot be got wrong by reading the wrong count.
                "any": self.changed_anything(),
                "entrypoint": self.entrypoint_changed,
                "assetsWritten": self.assets_written.len(),
                "assetsDeleted": self.assets_deleted.len(),
            },
            "files": { "written": self.assets_written, "deleted": self.assets_deleted },
            "schema": {
                "matches": self.schema_warnings.is_empty(),
                "warnings": self.schema_warnings,
            },
        })
    }
}

/// Restore a script's files to what a revision held.
///
/// A forward write, never a rewrite of history: the restored content becomes a
/// new revision whose parent is the one it came from. That keeps the cluster
/// notification, the cache invalidation and the `init()` that follows exactly
/// as they are for any other write, instead of adding a second path into the
/// same caches with its own way of going wrong.
///
/// The whole thing is one transaction. A revert that wrote the modules but not
/// the root — or that restored files without removing the ones the target
/// never had — would leave a tree that is neither version, which is the state
/// reverting exists to escape.
pub async fn revert_authorized(
    user: &UserContext,
    script_uri: &str,
    spec: &str,
    dry_run: bool,
    force: bool,
) -> Result<RevertOutcome, RevertRefusal> {
    if !can_write_history(user, script_uri) {
        return Err(RevertRefusal::AccessDenied);
    }

    let target = resolve_revision(script_uri, spec)
        .await
        .map_err(RevertRefusal::NotFound)?;

    let plan = revisions::plan_revert(script_uri, target)
        .await
        .map_err(|e| RevertRefusal::Storage(format!("Failed to plan revert: {}", e)))?
        .ok_or_else(|| {
            RevertRefusal::NotFound(format!(
                "Revision {} of '{}' cannot be read",
                target, script_uri
            ))
        })?;

    // Refuse to deploy a revision that does not bundle. The engine can answer
    // this without running anything and without writing anything — which is
    // the point of being able to build from a view other than the deployed
    // one — so a revert onto a broken tree is a thing the caller finds out
    // before it happens rather than from the FATAL afterwards.
    if !force
        && let Some(root) = revisions::root_content(script_uri, target)
            .await
            .map_err(|e| RevertRefusal::Storage(format!("Failed to read revision: {}", e)))?
        && let Err(error) = crate::module_loader::prepare_executable_program_in(
            script_uri,
            &root,
            &crate::source_view::SourceView::Revision(target),
        )
    {
        return Err(RevertRefusal::WillNotBuild(format!(
            "Revision {} does not bundle: {}. Pass force to restore it anyway.",
            target, error
        )));
    }

    // Compared before anything is applied, so a dry run reports it too — which
    // is the point. Someone deciding whether to revert wants to know that the
    // target's modules never heard of the column added since, while they still
    // have the option not to.
    let schema_warnings = match (
        revisions::schema_at(script_uri, target).await,
        revisions::schema_now(script_uri).await,
    ) {
        (Ok(recorded), Ok(current)) => {
            revisions::compare_schema(target, recorded.as_ref(), &current)
        }
        _ => Vec::new(),
    };

    let outcome = RevertOutcome {
        target,
        assets_written: plan.writes.iter().map(|file| file.uri.clone()).collect(),
        assets_deleted: plan.deletes.clone(),
        entrypoint_changed: plan.root_changes(),
        revision: None,
        dry_run,
        schema_warnings,
    };

    if dry_run || plan.is_empty() {
        return Ok(outcome);
    }

    let writes = revisions::revert_content(script_uri, target, &plan)
        .await
        .map_err(|e| RevertRefusal::Storage(format!("Failed to read revision content: {}", e)))?;

    let script = script_uri.to_string();
    let deletes = plan.deletes.clone();
    let user_id = user.user_id.clone();
    let applied = tokio::task::spawn_blocking(move || {
        apply_revert(&script, target, writes, deletes, user_id.as_deref())
    })
    .await
    .map_err(|e| RevertRefusal::Storage(format!("join error: {}", e)))?
    .map_err(RevertRefusal::Storage)?;

    Ok(RevertOutcome {
        revision: applied,
        ..outcome
    })
}

/// The writing half of a revert, as one transaction on one thread.
///
/// Blocking because the repository's transaction is thread-local: the writes
/// and the revision that records them have to happen on the thread that opened
/// it, or they join no transaction at all.
fn apply_revert(
    script_uri: &str,
    target: i32,
    writes: Vec<(String, String, Vec<u8>)>,
    deletes: Vec<String>,
    user_id: Option<&str>,
) -> Result<Option<i32>, String> {
    let _guard = crate::database::Database::begin_transaction(None)
        .map_err(|e| format!("Failed to open revert transaction: {}", e))?;

    let result = (|| -> Result<Option<i32>, String> {
        // The entrypoint is one of the writes. It used to need a call of its
        // own here, before them, because restoring it was a different kind of
        // write to a different table.
        if !writes.is_empty() {
            let now = std::time::SystemTime::now();
            let assets = writes
                .into_iter()
                .map(|(uri, mimetype, content)| repository::Asset {
                    name: Some(uri.clone()),
                    uri,
                    mimetype,
                    content,
                    created_at: now,
                    updated_at: now,
                    script_uri: script_uri.to_string(),
                })
                .collect();
            repository::upsert_assets(script_uri, assets)
                .map_err(|e| format!("Failed to restore files: {}", e))?;
        }

        for uri in &deletes {
            repository::delete_asset(script_uri, uri);
        }

        Ok(revisions::record_blocking_with_parent(
            script_uri,
            revisions::Origin::Revert,
            user_id,
            Some(target),
        ))
    })();

    match result {
        Ok(revision) => {
            crate::database::Database::commit_transaction()
                .map_err(|e| format!("Failed to commit revert: {}", e))?;
            Ok(revision)
        }
        Err(e) => {
            let _ = crate::database::Database::rollback_transaction();
            Err(e)
        }
    }
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
/// Where a script's routes, assets, streams and MCP tools
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

    // Moving a script is where an operator learns that it would take a path
    // another script already serves there, rather than from a path that stops
    // answering after the move.
    if let Ok(metadata) = repository::get_all_script_metadata() {
        let conflicts = crate::route_index::conflicts_if_bound(&metadata, uri, &hosts);
        if !conflicts.is_empty() {
            let listed: Vec<String> = conflicts
                .iter()
                .take(10)
                .map(|c| {
                    format!(
                        "{} {} on {} (held by {})",
                        c.method,
                        c.path,
                        if c.host.is_empty() {
                            "this engine"
                        } else {
                            &c.host
                        },
                        c.held_by
                    )
                })
                .collect();
            return Err(ScriptHostError::Validation(format!(
                "Not bound: {} registration(s) of {} are already held on those hosts: {}{}",
                conflicts.len(),
                uri,
                listed.join("; "),
                if conflicts.len() > listed.len() {
                    "; …"
                } else {
                    ""
                },
            )));
        }
    }

    repository::set_script_hosts(uri, &hosts)
        .map_err(|e| ScriptHostError::Storage(format!("Failed to store script hosts: {}", e)))?;

    let effective = crate::hosts::effective_hosts(&hosts);
    info!(
        "Script {} host binding set to {:?} (publishing on {:?})",
        uri, hosts, effective
    );
    Ok((hosts, effective))
}

/// Installation confirmation page, shown after a fresh install (the root
/// path redirects here until further routes are registered).
fn installed_page_html(nonce: &str) -> String {
    crate::engine_page::document(
        "aiwebengine installed",
        nonce,
        crate::engine_page::Width::Narrow,
        r#"<h1>Thanks for installing aiwebengine!</h1>
        <p class="aw-identity aw-muted">Your server is up and running.</p>"#,
    )
}

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
    let nonce = crate::security::generate_nonce();
    crate::engine_page::response(StatusCode::OK, installed_page_html(&nonce), &nonce)
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
    // `has_admin_capability`: the latter passes on capability alone. Topology
    // diagnostics require a real admin session.
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
                .filter(|email| *email != user_name)
                .map(|email| format!(r#" <span class="aw-muted">({})</span>"#, html_escape(email)))
                .unwrap_or_default();
            format!(
                r#"
        <div class="aw-detail">
            <span class="aw-detail-label">Signed in as</span>
            {}{}
        </div>"#,
                html_escape(user_name),
                email_suffix
            )
        }
        None => String::new(),
    };

    let attempted_path_block = match attempted {
        Some(path) => format!(
            r#"
        <div class="aw-detail">
            <span class="aw-detail-label">Attempted to access</span>
            <code>{}</code>
        </div>"#,
            html_escape(path)
        ),
        None => String::new(),
    };

    let action_link = match auth_user {
        Some(_) => r#"<a class="aw-button" href="/auth/logout">Sign out</a>"#.to_string(),
        None => {
            let redirect_suffix = attempted
                .map(|path| format!("?redirect={}", urlencoding::encode(path)))
                .unwrap_or_default();
            format!(
                r#"<a class="aw-button" href="/auth/login{}">Sign in</a>"#,
                redirect_suffix
            )
        }
    };

    // Names the one <style> block the shell writes and nothing else, so
    // anything injected into this page stays inert. Fresh per response — a
    // nonce a caller can predict is not a nonce. The sheet is inlined rather
    // than linked because this page is shown when something has already gone
    // wrong, and must not depend on another resource being served.
    let nonce = crate::security::generate_nonce();

    let body = format!(
        r#"<h1>Insufficient permissions</h1>
        <p class="aw-identity aw-muted">You don't have the required permissions to access this
            resource.</p>{user_info_block}{attempted_path_block}
        <div class="aw-detail">
            <span class="aw-detail-label">Why am I seeing this?</span>
            <p class="aw-explain">This page or feature requires <strong>Editor</strong> or
                <strong>Administrator</strong> privileges. Your current account does not have
                these permissions.</p>
            <span class="aw-detail-label">What can I do?</span>
            <ul class="aw-list aw-explain">
                <li>Contact your system administrator to request the appropriate role</li>
                <li>Verify you're signed in with the correct account</li>
                <li>Return to the home page to access features available to you</li>
            </ul>
        </div>
        <div class="aw-actions">
            <a class="aw-button aw-button--secondary" href="/">Go to home</a>
            {action_link}
        </div>"#
    );

    crate::engine_page::response(
        StatusCode::FORBIDDEN,
        crate::engine_page::document(
            "Insufficient permissions",
            &nonce,
            crate::engine_page::Width::Narrow,
            &body,
        ),
        &nonce,
    )
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
            refuse(Refusal::Forbidden, "Insufficient permissions"),
        );
    }

    let spec = tokio::task::spawn_blocking(generate_merged_openapi_spec)
        .await
        .unwrap_or_else(|e| refuse(Refusal::BadRequest, format!("join error: {}", e)).to_string());

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

/// A tool call's `edits` argument, read the way the HTTP routes read theirs.
///
/// Going through [`StringEditBody`] rather than picking the fields out of the
/// JSON is what keeps an `edit_asset` call and a `/engine/edit_file` of the
/// same edits from disagreeing about which of them is malformed.
fn parse_tool_edits(edits: &[Value]) -> Result<Vec<StringEdit>, String> {
    let bodies: Vec<StringEditBody> = edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            serde_json::from_value(edit.clone()).map_err(|e| format!("edits[{}]: {}", index, e))
        })
        .collect::<Result<_, _>>()?;
    prepare_edits(bodies)
}

/// Why an operation did not do what it was asked.
///
/// The operation table answers every caller — an MCP client, a script calling
/// `engine.call`, an HTTP request — with the same JSON, so a refusal is a value
/// like any other result. What HTTP needs on top is a status line, and it used
/// to be recovered by reading the message: a text that happened to contain
/// `not found` became a 404. A refusal now says what kind it is where it is
/// made, in a `status` field the HTTP layer reads, and the message is only
/// ever something for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The arguments are missing or malformed, or ask for something that
    /// cannot be done as stated.
    BadRequest,
    /// The caller may not do this.
    Forbidden,
    /// What the arguments name does not exist.
    NotFound,
    /// What is stored disagrees: it already exists, or it has moved on since
    /// the caller read it.
    Conflict,
    /// The operation ran out of time.
    TimedOut,
    /// Something went wrong that was not the caller's doing.
    Failed,
}

impl Refusal {
    /// The kind a refusal's *text* implies, for the errors the layers below
    /// still report as strings (`authorize_script_write`, the revision
    /// resolvers). It is the one place the engine reads a message to decide a
    /// status, kept so that the typed refusals around it can say what they are
    /// and this can shrink as those layers do.
    pub fn from_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        let has = |needle: &str| lower.contains(needle);
        if has("access denied")
            || has("permission denied")
            || has("this takes an administrator")
            || has("administrator privileges")
            || has("insufficient permissions")
        {
            Refusal::Forbidden
        } else if has("already exists")
            || has("cannot remove the last")
            || has("has changed since")
            || has("are already held")
        {
            Refusal::Conflict
        } else if has("old_string")
            || has("edits[")
            || has("would leave")
            || has("missing required")
            || has("is required")
            || has("must be")
            || has("invalid")
            || has("is not")
            || has("not a ")
            || has("escapes")
            || has("does not match")
        {
            Refusal::BadRequest
        } else if has("not found") || has("no such") || has("no revision") {
            Refusal::NotFound
        } else if has("timed out") {
            Refusal::TimedOut
        } else if has("failed to") {
            Refusal::Failed
        } else {
            Refusal::BadRequest
        }
    }

    pub fn status(self) -> u16 {
        match self {
            Refusal::BadRequest => 400,
            Refusal::Forbidden => 403,
            Refusal::NotFound => 404,
            Refusal::Conflict => 409,
            Refusal::TimedOut => 504,
            Refusal::Failed => 500,
        }
    }
}

/// A refusal as an operation's result: `{ "error": message, "status": code }`.
pub fn refuse(kind: Refusal, message: impl Into<String>) -> Value {
    json!({ "error": message.into(), "status": kind.status() })
}

/// A refusal from an [`AppError`], whose own status says what kind it is.
pub fn refuse_app(error: &crate::error::AppError, message: impl Into<String>) -> Value {
    json!({ "error": message.into(), "status": error.status_code() })
}

/// A refusal whose kind is read from its text; see [`Refusal::from_message`].
fn refuse_text(message: impl Into<String>) -> Value {
    let message = message.into();
    refuse(Refusal::from_message(&message), message)
}

/// A refusal from a git operation, by what kind of failure it was.
fn refuse_sync(error: &crate::git_sync::SyncError) -> Value {
    use crate::git_sync::SyncError;
    let kind = match error {
        SyncError::AccessDenied(_) => Refusal::Forbidden,
        SyncError::Diverged(_) | SyncError::WouldOverwrite(_) => Refusal::Conflict,
        SyncError::Layout(_) | SyncError::TooLarge(_) => Refusal::BadRequest,
        SyncError::RateLimited(_) => Refusal::Forbidden,
        SyncError::Archive(_) | SyncError::Remote(_) | SyncError::Storage(_) => Refusal::Failed,
    };
    refuse(kind, error.to_string())
}

fn missing_arg(name: &str) -> Value {
    refuse(
        Refusal::BadRequest,
        format!("Missing required parameter: {}", name),
    )
}

fn native_tools() -> &'static [NativeToolEntry] {
    &[
        (
            "list_scripts",
            "List the scripts in this engine, optionally filtered by a pattern over their URIs. A script is a tree of files; list_files lists one script's files.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Optional regex pattern to filter scripts by URI" }
                    }
                })
            },
            tool_list_scripts,
        ),
        (
            "rename_script",
            "Rename a script; files, history, secrets, tables and settings go with it and routes are registered again. The new name is a slug: lower-case letters, digits, '-' and '_'. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Current name of the script" },
                        "to": { "type": "string", "description": "The new name, a slug" }
                    },
                    "required": ["script", "to"]
                })
            },
            tool_rename_script,
        ),
        (
            "delete_script",
            "Delete a script and everything that belongs to it: its files, its revisions, its tables and its queued work. To remove one file of a script, use delete_file.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to delete" }
                    },
                    "required": ["script"]
                })
            },
            tool_delete_script,
        ),
        (
            "search_files",
            "Search script sources and assets for a pattern, to find which file to read. read_file's 'grep' searches one file you can already name.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text or regex pattern to search for" },
                        "caseInsensitive": { "type": "boolean", "description": "Whether search should be case-insensitive", "default": true },
                        "scope": { "type": "string", "enum": ["all", "scripts", "assets"], "description": "Which files to read: 'all' (default), root sources only, or assets only" },
                        "script": { "type": "string", "description": "Search only this script's files" }
                    },
                    "required": ["query"]
                })
            },
            tool_search_files,
        ),
        (
            "read_logs",
            "Read log messages for one script ('script') or all. Each entry carries its invocation (requestId, kind, route), so one request's lines can be read alone.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI; omit for all" },
                        "level": { "type": "string", "description": "Only entries at this level, e.g. ERROR" },
                        "since": { "type": "string", "description": "Only entries at or after this time (epoch millis or RFC 3339)" },
                        "after_seq": { "type": "integer", "description": "Only entries after this seq; pass the highest seq of the last read for what is new" },
                        "contains": { "type": "string", "description": "Only entries whose message contains this substring" },
                        "request_id": { "type": "string", "description": "Only one invocation's entries, by x-request-id or invocation id" },
                        "kind": { "type": "string", "description": "httpRoute, scheduled, streamCustomization, mcpTool, mcpPrompt, init, eval or test" },
                        "route": { "type": "string", "description": "Only entries while serving this route pattern, e.g. /things/:id" },
                        "revision": { "type": "integer", "description": "Only entries written while this revision was running" },
                        "limit": { "type": "integer", "description": "At most this many of the newest matches" }
                    }
                })
            },
            tool_read_logs,
        ),
        (
            "read_audit",
            "Read the events one script recorded with audit.record, newest first. Its owner or an administrator only; nothing deletes them.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script whose events to read" },
                        "action": { "type": "string", "description": "Only events with this action" },
                        "before_id": { "type": "integer", "description": "Only events older than this id; pass the smallest id of the last read for the next page" },
                        "limit": { "type": "integer", "description": "At most this many (default 100, at most 1000)" }
                    },
                    "required": ["script"]
                })
            },
            tool_read_audit,
        ),
        (
            "clear_logs",
            "Delete one script's log messages. Retention across every script is applied by the engine's own pruner and is not something a caller triggers.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI whose logs to clear" }
                    },
                    "required": ["script"]
                })
            },
            tool_clear_logs,
        ),
        (
            "list_routes",
            "List every registration in the engine: script HTTP routes, SSE streams (method STREAM) and asset routes (method ASSET). Pass 'host' to see only what is live on that host.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "host": { "type": "string", "description": "Only registrations published on this host" }
                    }
                })
            },
            tool_list_routes,
        ),
        (
            "exposure_report",
            "List registrations the engine refused to publish. 'scripts': a file outside the directory that exposes it ('public/' is served, 'resources/' is an MCP resource, the rest is private). 'collisions': a host, path and method already held by an older script. 'unclassified': scripts whose init() has not run cleanly.",
            || {
                json!({
                    "type": "object",
                    "properties": {}
                })
            },
            tool_exposure_report,
        ),
        (
            "read_init_status",
            "Read init() status for scripts (useful for debugging). Returns status for one script when 'script' is given, otherwise for all scripts.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Optional script URI to retrieve init status for; omit to list all scripts" }
                    }
                })
            },
            tool_read_init_status,
        ),
        (
            "list_files",
            "List the files of a script: its entrypoint (main.ts, main.js, main.tsx or main.jsx) and every module, template and other file beside it. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose files to list (e.g., 'https://example.com/myscript')" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_files,
        ),
        (
            "read_file",
            "Read one file of a script, or part of it with 'lines' or 'grep'. Text is 'content', anything else 'content_base64'; the reply carries the file's sha256 (edit_file's base_sha256). Reads head, which for a pinned script is not what it serves; a 'deployment' block says so.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts', 'lib/util.ts' or 'images/logo.png'" },
                        "lines": { "type": "string", "description": "1-based inclusive line range, e.g. '120-180', '120-' or '120'" },
                        "grep": { "type": "string", "description": "Regular expression; answers with matching lines instead of the file" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_read_file,
        ),
        (
            "write_file",
            "Create or update one file. Writing the entrypoint (main.ts/.js/.tsx/.jsx) writes the script itself, creating it if needed, and takes WriteScripts and ownership instead of WriteAssets. Use write_files for several files.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts' or 'lib/util.ts'" },
                        "mimetype": { "type": "string", "description": "MIME type; inferred from the extension when omitted" },
                        "text": { "type": "string", "description": "The file as text — what a module is. Exactly one of 'text' and 'content' is required." },
                        "content": { "type": "string", "description": "The file as base64, for content that is not text (max 10MB)" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_write_file,
        ),
        (
            "write_files",
            "Write several of a script's files as one change, then run init() once. Modules go in 'text' as plain source ('content_base64' for non-text); 'remove' deletes paths. One revision, nothing written if any file is rejected. The entrypoint (main.ts/.js/.tsx/.jsx) is one of the files, or 'content'; writing it takes WriteScripts, the rest WriteAssets, both with ownership or administrator. The answer includes a 'check' report: read its diagnostics first.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose files these are" },
                        "files": {
                            "type": "array",
                            "description": "Files to write (max 256, 10MB of content in total)",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "URI/path of the asset (e.g., '/lib/util.ts')" },
                                    "text": { "type": "string", "description": "The file as text; exactly one of 'text' and 'content_base64'" },
                                    "content_base64": { "type": "string", "description": "The file as base64, for content that is not text (max 10MB)" },
                                    "mimetype": { "type": "string", "description": "MIME type; inferred from the file extension when omitted" },
                                    "sha256": { "type": "string", "description": "Expected SHA-256 (hex); the batch is rejected on a mismatch" }
                                },
                                "required": ["name"]
                            }
                        },
                        "content": { "type": "string", "description": "The script's root source as this change leaves it. Omit to leave it alone." },
                        "remove": {
                            "type": "array",
                            "description": "Asset paths this change removes. Naming a file the script does not have is not an error.",
                            "items": { "type": "string" }
                        },
                        "reinit": { "type": "string", "enum": ["after", "never"], "description": "Run the script's init() once after the batch lands (default 'after'), or leave it alone" },
                        "check": { "type": "boolean", "description": "Answer with the check_script report for what was written (default true)" }
                    },
                    "required": ["script"]
                })
            },
            tool_write_files,
        ),
        (
            "edit_file",
            "Edit one file in place by replacing strings, without resending it, then run init() once. Each old_string must be present and unique unless replace_all is set; nothing is written unless every edit applies. Editing the entrypoint takes WriteScripts and ownership; other files WriteAssets and ownership. Edits head; a pinned script keeps serving its pinned revision. The answer includes a 'check' report.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts' or 'lib/util.ts'" },
                        "edits": {
                            "type": "array",
                            "description": "Edits applied in order (max 128)",
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
                        "base_sha256": { "type": "string", "description": "SHA-256 from read_file; refused if the file has changed since" },
                        "reinit": { "type": "string", "enum": ["after", "never"], "description": "Run the script's init() once the edits land (default 'after'), or leave it alone" },
                        "check": { "type": "boolean", "description": "Answer with the check_script report for the script as edited (default true)" }
                    },
                    "required": ["script", "path", "edits"]
                })
            },
            tool_edit_file,
        ),
        (
            "create_file",
            "Create a new file; fails if the path exists, unlike write_file. Creating the entrypoint (main.ts/.js/.tsx/.jsx) creates the script.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that will own the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts' or 'lib/util.ts'" },
                        "mimetype": { "type": "string", "description": "MIME type; inferred from the file extension when omitted" },
                        "text": { "type": "string", "description": "The file as text — what a module is. Exactly one of 'text' and 'content' is required." },
                        "content": { "type": "string", "description": "The file as base64, for content that is not text (max 10MB)" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_create_file,
        ),
        (
            "delete_file",
            "Delete one file from a script. Takes DeleteAssets and ownership, or — for the entrypoint, whose removal leaves the script with no program — DeleteScripts. To remove the whole script, use delete_script.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'lib/util.ts' or 'images/logo.png'" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_delete_file,
        ),
        (
            "list_script_owners",
            "List the owners of a script",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI" }
                    },
                    "required": ["script"]
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
                        "script": { "type": "string", "description": "Script URI" },
                        "owner": { "type": "string", "description": "User id to add as owner" }
                    },
                    "required": ["script", "owner"]
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
                        "script": { "type": "string", "description": "Script URI" },
                        "owner": { "type": "string", "description": "Owner user id to remove" }
                    },
                    "required": ["script", "owner"]
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
            "set_user_realm",
            "Move a user into a realm: the host they authenticate on, or * for every host. Administrator privileges required. No sign-in path produces *, so this is how an account is given access across hosts. Takes effect on the user's next sign-in.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "user_id": { "type": "string", "description": "Id of the user to modify" },
                        "realm": {
                            "type": "string",
                            "description": "Host name the user authenticates on, or * for every host"
                        }
                    },
                    "required": ["user_id", "realm"]
                })
            },
            tool_set_user_realm,
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
                        "script": { "type": "string", "description": "URI of the script to inspect" }
                    },
                    "required": ["script"]
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
                        "script": { "type": "string", "description": "URI of the script to modify" },
                        "hosts": {
                            "type": "array",
                            "description": "Hostnames to publish on. ['*'] means every configured host; [] returns the script to the default host.",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["script", "hosts"]
                })
            },
            tool_set_script_hosts,
        ),
        (
            "run_tests",
            "Run a script's test modules ('*.test.ts' or .js/.jsx/.tsx assets) and report a verdict per case. Runs the stored files (head). Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose tests to run" },
                        "filter": { "type": "string", "description": "Run only cases whose name contains this text" },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes the tests make (default true)",
                            "default": true
                        },
                        "revision": {
                            "type": "string",
                            "description": "A revision number, 'head', 'last-good' or a label; omit for head"
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_run_tests,
        ),
        (
            "list_revisions",
            "A script's revision history: what changed, when, by whom, and whether init() succeeded. 'lastGood' is the newest revision whose init() succeeded. Pass 'asset' for one file's history.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose history to read" },
                        "asset": { "type": "string", "description": "Only the revisions in which this file changed" },
                        "limit": { "type": "integer", "description": "Keep at most this many of the newest revisions (default 50)" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_revisions,
        ),
        (
            "revert_script",
            "Restore a script's files to a revision, recorded as a new revision. Removes files that revision did not contain; refuses a target that does not bundle unless 'force'. 'dryRun' previews. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to restore" },
                        "revision": { "type": "string", "description": "Which version to restore: a revision number, 'head', 'last-good', or a label" },
                        "dryRun": {
                            "type": "boolean",
                            "description": "Report what would change without changing it",
                            "default": false
                        },
                        "force": {
                            "type": "boolean",
                            "description": "Restore even if the target revision does not bundle",
                            "default": false
                        },
                        "reinit": {
                            "type": "string",
                            "description": "'after' runs the script's init() once the files land (default); 'never' leaves it alone"
                        }
                    },
                    "required": ["script", "revision"]
                })
            },
            tool_revert_script,
        ),
        (
            "diff_revisions",
            "A unified diff per file between two revisions: see what you changed, or what a revert would undo. With neither 'from' nor 'to', the newest change.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to compare" },
                        "from": { "type": "string", "description": "The older side: a revision number, 'head', 'last-good' or a label" },
                        "to": { "type": "string", "description": "The newer side. Defaults to 'head'." },
                        "context": { "type": "integer", "description": "Lines of context around each hunk (default 3)" }
                    },
                    "required": ["script"]
                })
            },
            tool_diff_revisions,
        ),
        (
            "label_revision",
            "Name a revision so it can be restored by name and survives retention. Omit 'label' to clear one.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose revision to name" },
                        "revision": { "type": "string", "description": "Which revision: a number, 'head', 'last-good', or an existing label" },
                        "label": { "type": "string", "description": "The name. Omit to clear the revision's label." }
                    },
                    "required": ["script", "revision"]
                })
            },
            tool_label_revision,
        ),
        (
            "set_script_limits",
            "Give one script its own execution budget, to raise it for a slow model API or lower it to contain a runaway; effective without a restart. Administrator only, because it claims shared slots, threads and memory. Omit a field to follow the engine; omit all to remove the override.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script" },
                        "timeoutMs": { "type": "integer", "description": "Wall clock for one request-shaped invocation" },
                        "jobTimeoutMs": { "type": "integer", "description": "Wall clock for one scheduled job or queued task" },
                        "maxMemoryBytes": { "type": "integer", "description": "Heap ceiling for this script's runtime" },
                        "note": { "type": "string", "description": "Why, for whoever reads this next" }
                    },
                    "required": ["script"]
                })
            },
            tool_set_script_limits,
        ),
        (
            "get_script_limits",
            "What one script may spend, and what that resolves to once the engine's own settings are laid under it. Omit 'script' to list every override in the engine — which is the way to find out why one script behaves differently from the rest.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of one script; omit to list every override" }
                    }
                })
            },
            tool_get_script_limits,
        ),
        (
            "list_tasks",
            "Read a script's queued work: pending, running, and failed (with the error). A task that succeeded is deleted; its output is in the log under its invocation id. A pending task may be waiting behind another in the same 'lane'.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose queue to read" },
                        "limit": { "type": "integer", "description": "How many to return, newest first (default 50, max 500)" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_tasks,
        ),
        (
            "cancel_task",
            "Cancel one pending task (one already running is not stopped), or with finished=true discard the failed and cancelled ones.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose queue to act on" },
                        "task": { "type": "string", "description": "Id of the pending task to cancel" },
                        "finished": {
                            "type": "boolean",
                            "description": "Discard this script's failed and cancelled tasks instead of cancelling one.",
                            "default": false
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_cancel_task,
        ),
        (
            "deploy_script",
            "Choose which revision of a script is served. After the first deploy, writes advance head without changing what is served, until you deploy again. revision='head' takes the newest; follow=true stops pinning.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to deploy" },
                        "revision": { "type": "string", "description": "Which version to serve: a revision number, 'head', 'last-good', or a label" },
                        "follow": {
                            "type": "boolean",
                            "description": "Stop pinning; serve the newest revision. Ignores 'revision'.",
                            "default": false
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_deploy_script,
        ),
        (
            "set_git_credential",
            "Store your personal access token for a git host so pulls can read private repositories. Encrypted at rest, never returned, checked against the host first. Always your own credential.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "token": { "type": "string", "description": "Personal access token; read access to the repositories' contents is enough" },
                        "host": { "type": "string", "description": "Git host. Defaults to github.com, the only host supported.", "default": "github.com" }
                    },
                    "required": ["token"]
                })
            },
            tool_set_git_credential,
        ),
        (
            "list_git_credentials",
            "The git credentials you have stored: host, the account each belongs to, and when it \
            was added and last used. Never the token itself.",
            || json!({ "type": "object", "properties": {} }),
            tool_list_git_credentials,
        ),
        (
            "delete_git_credential",
            "Remove your stored credential for a git host. Pulls of private repositories stop \
            working; public ones carry on.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "host": { "type": "string", "description": "Git host to forget. Defaults to github.com.", "default": "github.com" }
                    }
                })
            },
            tool_delete_git_credential,
        ),
        (
            "list_git_bindings",
            "Which repository each script tracks, and where the two last agreed. Shows only \
            scripts you can read.",
            || json!({ "type": "object", "properties": {} }),
            tool_list_git_bindings,
        ),
        (
            "clear_git_remote",
            "Stop a script tracking a repository. The script and its files stay exactly as they \
            are; what goes is the record of where they came from, so later pulls no longer treat \
            it as that repository's to replace.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to unbind" }
                    },
                    "required": ["script"]
                })
            },
            tool_clear_git_remote,
        ),
        (
            "get_git_status",
            "Where a script stands against its repository: unbound, in_sync, behind, ahead, diverged or unreachable. Ask this before pushing rather than reading the refusal. Also reports the deployment pin.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to report on" }
                    },
                    "required": ["script"]
                })
            },
            tool_get_git_status,
        ),
        (
            "push_to_git",
            "Publish a script's files to a GitHub repository as one commit, laid out the way pull_from_git reads them back. Files the script does not own are left alone. Refuses when both sides changed since the last sync; the engine does not merge. Needs a stored credential with write access.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to publish" },
                        "repo": { "type": "string", "description": "'owner/repo' or a GitHub URL; optional once the script was pulled" },
                        "branch": { "type": "string", "description": "Branch to write. Defaults to the one it was pulled from, or the repository's default branch." },
                        "message": { "type": "string", "description": "Commit message. Defaults to one naming the script." },
                        "force": {
                            "type": "boolean",
                            "description": "Publish even if the repository seems to have moved; GitHub still refuses a non-fast-forward",
                            "default": false
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_push_to_git,
        ),
        (
            "pull_from_git",
            "Pull a public GitHub repository in as scripts: a directory holding main.ts (or .js/.tsx/.jsx) is one script, other files under it become assets at the same relative path, and files removed upstream are removed here.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "repo": { "type": "string", "description": "'owner/repo', or any GitHub URL naming it" },
                        "branch": { "type": "string", "description": "Branch to read. Defaults to the repository's default branch." },
                        "prefix": { "type": "string", "description": "URI prefix for the scripts; defaults to the repository name" },
                        "force": {
                            "type": "boolean",
                            "description": "Re-apply even if the repository has not moved",
                            "default": false
                        }
                    },
                    "required": ["repo"]
                })
            },
            tool_pull_from_git,
        ),
        (
            "get_deployment",
            "What a script serves, what its newest revision is, and how many revisions separate them. Use before deploying to see what taking head would mean.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to report on" }
                    },
                    "required": ["script"]
                })
            },
            tool_get_deployment,
        ),
        (
            "check_script",
            "Check a script's stored files (head) as if deployed, without deploying: bundle its imports, run init() with registrations withheld and database writes rolled back, and report diagnostics {file, line, severity, code, message}. Pass 'content' (or 'files') to check code before writing it. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to check" },
                        "content": {
                            "type": "string",
                            "description": "Candidate source to check instead of what is deployed"
                        },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes init() makes (default true)",
                            "default": true
                        },
                        "timeoutMs": {
                            "type": "integer",
                            "description": "Ceiling for the init() run; defaults to several times the deploy budget"
                        },
                        "revision": {
                            "type": "string",
                            "description": "A revision number, 'head', 'last-good' or a label; omit for head"
                        },
                        "files": {
                            "type": "object",
                            "description": "Candidate files laid over what is deployed: path -> {content, mimetype?}, or path -> null to check a removal",
                            "additionalProperties": true
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_check_script,
        ),
        (
            "eval_script",
            "Evaluate a JavaScript snippet in a deployed script's sandbox and return its value plus what it logged. The script's program is loaded first, so the snippet can call its functions and import its modules. Database writes roll back by default; registrations do nothing. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose sandbox to evaluate in" },
                        "source": {
                            "type": "string",
                            "description": "The snippet; its last expression is the value. Synchronous: no async/await."
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
                    "required": ["script", "source"]
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
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(source) = arg_str(args, "source").filter(|source| !source.trim().is_empty()) else {
        return missing_arg("source");
    };

    match authorize_eval(user, uri) {
        Ok(()) => {}
        Err(CheckRefusal::NotFound) => {
            return refuse(Refusal::NotFound, format!("Script not found: {}", uri));
        }
        Err(CheckRefusal::AccessDenied) => {
            return refuse(
                Refusal::Forbidden,
                format!(
                    "Permission denied. You must be an administrator or owner to evaluate against script '{}'",
                    uri
                ),
            );
        }
    }

    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let timeout_ms = args.get("timeoutMs").and_then(Value::as_u64);

    let report = crate::script_eval::eval_blocking(crate::script_eval::EvalRequest {
        timeout_ms,
        rollback,
        ..crate::script_eval::EvalRequest::new(uri.to_string(), source.to_string(), user.clone())
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
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let content = arg_str(args, "content").map(str::to_string);
    let files: CandidateFiles = match args.get("files") {
        None | Some(Value::Null) => CandidateFiles::default(),
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(files) => files,
            Err(e) => {
                return refuse(
                    Refusal::BadRequest,
                    format!(
                        "Invalid 'files': expected an object of path -> {{content, mimetype?}} or null: {}",
                        e
                    ),
                );
            }
        },
    };

    let has_candidate = content.is_some()
        || files.iter().any(|(path, candidate)| {
            candidate.is_some() && crate::module_loader::is_root_module_name(path)
        });

    match authorize_check(user, uri, has_candidate) {
        Ok(()) => {}
        Err(CheckRefusal::NotFound) => {
            return json!({
                "status": Refusal::NotFound.status(),
                "error": format!("Script not found: {}", uri),
                "message": "Pass 'content' to check a script that is not deployed yet",
            });
        }
        Err(CheckRefusal::AccessDenied) => {
            return refuse(
                Refusal::Forbidden,
                format!(
                    "Permission denied. You must be an administrator or owner to check script '{}'",
                    uri
                ),
            );
        }
    }

    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let base = match crate::database::run_blocking(resolve_view(uri, arg_str(args, "revision"))) {
        Ok(view) => view,
        Err(message) => return refuse_text(message),
    };

    let (view, candidate_files) = if files.is_empty() {
        (base, 0)
    } else {
        match candidate_overlay(files, base) {
            Ok(built) => built,
            Err(message) => return refuse_text(message),
        }
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
        object.insert("candidateFiles".to_string(), json!(candidate_files));
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
fn tool_list_revisions(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_read_history(user, script) {
        return refuse(
            Refusal::Forbidden,
            "Failed to read revisions: Access denied",
        );
    }
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);

    if let Some(asset) = arg_str(args, "asset") {
        return match crate::database::run_blocking(revisions::file_history(script, asset, limit)) {
            Ok(history) => json!({
                "success": true,
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
            Err(e) => refuse(
                Refusal::Failed,
                format!("Failed to read file history: {}", e),
            ),
        };
    }

    match crate::database::run_blocking(revisions::list(script, limit)) {
        Ok(listed) => json!({
            "success": true,
            "script": script,
            "revisions": listed.iter().map(revision_to_json).collect::<Vec<Value>>(),
            "head": listed.first().map(|revision| revision.revision),
            "lastGood": crate::database::run_blocking(revisions::last_good(script))
                .ok()
                .flatten(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, format!("Failed to read revisions: {}", e)),
    }
}

fn tool_diff_revisions(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_read_history(user, script) {
        return refuse(
            Refusal::Forbidden,
            "Failed to diff revisions: Access denied",
        );
    }

    let to = match crate::database::run_blocking(resolve_revision(
        script,
        arg_str(args, "to").unwrap_or("head"),
    )) {
        Ok(revision) => revision,
        Err(message) => return refuse_text(message),
    };

    let from = match arg_str(args, "from") {
        Some(spec) => match crate::database::run_blocking(resolve_revision(script, spec)) {
            Ok(revision) => revision,
            Err(message) => return refuse_text(message),
        },
        None => match crate::database::run_blocking(revisions::get(script, to)) {
            Ok(Some(revision)) => match revision.parent {
                Some(parent) => parent,
                None => {
                    return json!({
                        "success": true,
                        "script": script,
                        "to": to,
                        "files": [],
                        "message": "Revision 1 has nothing before it to compare against",
                    });
                }
            },
            Ok(None) => return refuse(Refusal::NotFound, format!("No revision {}", to)),
            Err(e) => return refuse(Refusal::Failed, format!("Failed to read revision: {}", e)),
        },
    };

    let context = args
        .get("context")
        .and_then(Value::as_u64)
        .unwrap_or(3)
        .min(50) as usize;

    match crate::database::run_blocking(revisions::diff(script, from, to, context)) {
        Ok(Some(diff)) => json!({
            "success": true,
            "script": script,
            "from": diff.from,
            "to": diff.to,
            "files": diff.files.iter().map(file_diff_to_json).collect::<Vec<Value>>(),
            "truncated": diff.truncated,
            "timestamp": iso_timestamp(),
        }),
        Ok(None) => refuse(Refusal::NotFound, format!("No revision {} or {}", from, to)),
        Err(e) => refuse(Refusal::Failed, format!("Failed to diff revisions: {}", e)),
    }
}

fn tool_set_script_limits(args: &Value, user: &UserContext) -> Value {
    if !is_user_admin(user) {
        return refuse(
            Refusal::Forbidden,
            "Failed to set limits: this takes an administrator. A script's limits are a claim on the engine's execution slots, threads and memory, which are shared with every other script — owning the script is not the same question.",
        );
    }

    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };

    let as_u64 = |name: &str| args.get(name).and_then(Value::as_u64);
    let overrides = crate::script_limits::Overrides {
        timeout_ms: as_u64("timeoutMs"),
        job_timeout_ms: as_u64("jobTimeoutMs"),
        max_memory_bytes: as_u64("maxMemoryBytes"),
    };

    if overrides.is_empty() {
        return match crate::database::run_blocking(crate::script_limits::clear(script)) {
            Ok(cleared) => json!({
                "success": true,
                "script": script,
                "cleared": cleared,
                "timestamp": iso_timestamp(),
            }),
            Err(e) => refuse(Refusal::Failed, format!("Failed to clear limits: {}", e)),
        };
    }

    match crate::database::run_blocking(crate::script_limits::set(
        script,
        overrides,
        args.get("note").and_then(Value::as_str),
        user.user_id.as_deref(),
    )) {
        Ok(limits) => json!({
            "success": true,
            "script": script,
            "limits": script_limits_to_json(&limits),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, format!("Failed to set limits: {}", e)),
    }
}

fn tool_get_script_limits(args: &Value, user: &UserContext) -> Value {
    if !is_user_admin(user) {
        return refuse(
            Refusal::Forbidden,
            "Failed to read limits: this takes an administrator",
        );
    }

    match arg_str(args, "script") {
        Some(script) => match crate::database::run_blocking(crate::script_limits::get(script)) {
            Ok(limits) => json!({
                "success": true,
                "script": script,
                "limits": limits.as_ref().map(script_limits_to_json),
                "effective": {
                    "timeoutMs": crate::script_limits::for_script(script).timeout_ms,
                    "jobTimeoutMs": crate::script_limits::job_timeout_ms_for(script),
                    "maxMemoryBytes": crate::script_limits::for_script(script).max_memory_mb * 1024 * 1024,
                },
                "timestamp": iso_timestamp(),
            }),
            Err(e) => refuse(Refusal::Failed, format!("Failed to read limits: {}", e)),
        },
        None => match crate::database::run_blocking(crate::script_limits::list()) {
            Ok(all) => json!({
                "success": true,
                "limits": all.iter().map(script_limits_to_json).collect::<Vec<_>>(),
                "timestamp": iso_timestamp(),
            }),
            Err(e) => refuse(Refusal::Failed, format!("Failed to list limits: {}", e)),
        },
    }
}

fn tool_list_tasks(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_read_history(user, script) {
        return refuse(Refusal::Forbidden, "Failed to read tasks: Access denied");
    }

    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
    match crate::database::run_blocking(crate::tasks::list(script, limit)) {
        Ok(tasks) => json!({
            "success": true,
            "script": script,
            "tasks": tasks.iter().map(crate::tasks::to_json).collect::<Vec<_>>(),
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, format!("Failed to read tasks: {}", e)),
    }
}

fn tool_cancel_task(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_write_history(user, script) {
        return refuse(Refusal::Forbidden, "Failed to cancel: Access denied");
    }

    if args
        .get("finished")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return match crate::database::run_blocking(crate::tasks::clear_finished(script)) {
            Ok(discarded) => json!({
                "success": true,
                "script": script,
                "discarded": discarded,
                "timestamp": iso_timestamp(),
            }),
            Err(e) => refuse(Refusal::Failed, format!("Failed to discard tasks: {}", e)),
        };
    }

    let Some(task) = arg_str(args, "task") else {
        return missing_arg("task");
    };
    let Ok(task_id) = uuid::Uuid::parse_str(task.trim()) else {
        return refuse(
            Refusal::BadRequest,
            "Failed to cancel: that is not a task id",
        );
    };

    // Scoped to the script the caller was authorized against, so an id alone
    // is not authority over another script's queue.
    match crate::database::run_blocking(crate::tasks::get(task_id)) {
        Ok(Some(found)) if found.script_uri == script => {}
        Ok(_) => {
            return refuse(
                Refusal::NotFound,
                "Failed to cancel: no such task for this script",
            );
        }
        Err(e) => return refuse(Refusal::Failed, format!("Failed to read the task: {}", e)),
    }

    match crate::database::run_blocking(crate::tasks::cancel(task_id)) {
        Ok(cancelled) => json!({
            "success": true,
            "script": script,
            "task": task_id.to_string(),
            "cancelled": cancelled,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(Refusal::Failed, format!("Failed to cancel the task: {}", e)),
    }
}

fn tool_deploy_script(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_write_history(user, script) {
        return refuse(Refusal::Forbidden, "Failed to deploy: Access denied");
    }

    if args.get("follow").and_then(Value::as_bool).unwrap_or(false) {
        let was_pinned = match crate::database::run_blocking(crate::deployments::unpin(script)) {
            Ok(was_pinned) => was_pinned,
            Err(e) => {
                return refuse(
                    Refusal::Failed,
                    format!("Failed to remove the deployment: {}", e),
                );
            }
        };
        let init = if was_pinned {
            crate::database::run_blocking(activate_deployment(script))
        } else {
            json!({ "ran": false, "reason": "the script was not pinned" })
        };
        return json!({
            "success": true,
            "script": script,
            "wasPinned": was_pinned,
            "following": "head",
            "init": init,
            "timestamp": iso_timestamp(),
        });
    }

    let Some(spec) = arg_str(args, "revision") else {
        return missing_arg("revision");
    };
    let revision = match crate::database::run_blocking(resolve_revision(script, spec)) {
        Ok(revision) => revision,
        Err(message) => return refuse_text(message),
    };

    let deployment = match crate::database::run_blocking(crate::deployments::deploy(
        script,
        revision,
        user.user_id.as_deref(),
    )) {
        Ok(deployment) => deployment,
        Err(crate::deployments::DeployRefusal::NoSuchRevision(message)) => {
            return refuse(Refusal::NotFound, format!("Failed to deploy: {}", message));
        }
        Err(crate::deployments::DeployRefusal::Storage(message)) => {
            return refuse(Refusal::Failed, format!("Failed to deploy: {}", message));
        }
    };

    let init = crate::database::run_blocking(activate_deployment(script));

    json!({
        "success": true,
        "script": script,
        "deployed": deployment_to_json(&deployment),
        "init": init,
        "timestamp": iso_timestamp(),
    })
}

fn tool_get_deployment(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if !can_read_history(user, script) {
        return refuse(
            Refusal::Forbidden,
            "Failed to read the deployment: Access denied",
        );
    }

    let deployment = crate::database::run_blocking(crate::deployments::get(script))
        .ok()
        .flatten();
    let head = crate::database::run_blocking(revisions::head(script))
        .ok()
        .flatten();

    json!({
        "success": true,
        "script": script,
        "pinned": deployment.is_some(),
        "serving": deployment.as_ref().map(|d| d.revision).or(head),
        "head": head,
        "behind": match (deployment.as_ref().map(|d| d.revision), head) {
            (Some(serving), Some(head)) => json!((head - serving).max(0)),
            _ => Value::Null,
        },
        "deployment": deployment.as_ref().map(deployment_to_json),
        "timestamp": iso_timestamp(),
    })
}

fn tool_label_revision(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(spec) = arg_str(args, "revision") else {
        return missing_arg("revision");
    };
    if !can_write_history(user, script) {
        return refuse(
            Refusal::Forbidden,
            "Failed to label revision: Access denied",
        );
    }

    let revision = match crate::database::run_blocking(resolve_revision(script, spec)) {
        Ok(revision) => revision,
        Err(message) => return refuse_text(message),
    };
    let label = arg_str(args, "label").filter(|label| !label.trim().is_empty());

    match crate::database::run_blocking(revisions::set_label(script, revision, label)) {
        Ok(true) => json!({
            "success": true,
            "script": script,
            "revision": revision,
            "label": label,
            "timestamp": iso_timestamp(),
        }),
        Ok(false) => refuse(Refusal::NotFound, format!("No revision {}", revision)),
        Err(e) => refuse(Refusal::Failed, format!("Failed to label revision: {}", e)),
    }
}

fn tool_revert_script(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(spec) = arg_str(args, "revision") else {
        return missing_arg("revision");
    };
    let dry_run = args.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
    let reinit = match ReinitMode::parse(arg_str(args, "reinit")) {
        Ok(reinit) => reinit,
        Err(message) => return refuse_text(message),
    };

    let outcome = match crate::database::run_blocking(revert_authorized(
        user, script, spec, dry_run, force,
    )) {
        Ok(outcome) => outcome,
        Err(RevertRefusal::AccessDenied) => {
            return refuse(Refusal::Forbidden, "Failed to revert: Access denied");
        }
        Err(RevertRefusal::NotFound(message)) => {
            return refuse(Refusal::NotFound, format!("Failed to revert: {}", message));
        }
        Err(RevertRefusal::WillNotBuild(message)) => {
            return refuse(
                Refusal::BadRequest,
                format!("Failed to revert: {}", message),
            );
        }
        Err(RevertRefusal::Storage(message)) => {
            return refuse(Refusal::Failed, format!("Failed to revert: {}", message));
        }
    };

    // Bridging back to async, as the asset tools do, so the caller is told what
    // init() did rather than that it was started.
    let init = match (reinit, outcome.revision) {
        (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
        (_, None) => json!({ "ran": false, "reason": "no change" }),
        (ReinitMode::After, Some(_)) => {
            crate::database::run_blocking(reinitialize_after_write(script))
        }
    };

    let mut body = outcome.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("success".to_string(), json!(true));
        object.insert("script".to_string(), json!(script));
        object.insert("init".to_string(), init);
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }
    body
}

fn tool_run_tests(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };

    match authorize_test_run(user, uri) {
        Ok(()) => {}
        Err(TestRunRefusal::NotFound) => {
            return refuse(Refusal::NotFound, format!("Script not found: {}", uri));
        }
        Err(TestRunRefusal::AccessDenied) => {
            return refuse(
                Refusal::Forbidden,
                format!(
                    "Permission denied. You must be an administrator or owner to run tests for script '{}'",
                    uri
                ),
            );
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
        Err(message) => return refuse_text(message),
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

/// The scripts in the engine. A script is a tree; `tool_list_files` lists one
/// tree's files.
fn tool_list_scripts(args: &Value, user: &UserContext) -> Value {
    let pattern = arg_str(args, "pattern");
    let regex = match pattern {
        Some(p) => match regex::RegexBuilder::new(p).case_insensitive(true).build() {
            Ok(r) => Some(r),
            Err(e) => {
                return refuse(
                    Refusal::BadRequest,
                    format!("Failed to list scripts: {}", e),
                );
            }
        },
        None => None,
    };

    let scripts: Vec<Value> = list_scripts_authorized(user)
        .iter()
        .filter(|meta| regex.as_ref().is_none_or(|r| r.is_match(&meta.uri)))
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
                "type": "script",
                "updatedAt": millis(meta.updated_at),
                "createdAt": millis(meta.created_at),
                "initialized": meta.initialized,
                "initError": meta.init_error.as_deref(),
            })
        })
        .collect();

    json!({
        "scripts": scripts,
        "count": scripts.len(),
        "pattern": pattern,
        "timestamp": iso_timestamp(),
    })
}

fn tool_rename_script(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(to) = arg_str(args, "to") else {
        return missing_arg("to");
    };
    match rename_script_authorized(user, uri, to) {
        Ok(()) => json!({
            "success": true,
            "uri": to,
            "renamedFrom": uri,
            "timestamp": iso_timestamp(),
        }),
        Err(message) => refuse_text(message),
    }
}

fn tool_delete_script(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    if delete_script_authorized(user, uri, Some("mcp")) {
        json!({
            "success": true,
            "uri": uri,
            "timestamp": iso_timestamp(),
        })
    } else {
        refuse(Refusal::NotFound, format!("Script not found: {}", uri))
    }
}

fn tool_search_files(args: &Value, user: &UserContext) -> Value {
    let Some(query) = arg_str(args, "query") else {
        return missing_arg("query");
    };
    let scope = match SearchScope::parse(arg_str(args, "scope")) {
        Ok(scope) => scope,
        Err(message) => return refuse_text(message),
    };
    let options = SearchOptions {
        case_insensitive: args
            .get("caseInsensitive")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        scope,
        script: arg_str(args, "script").map(str::to_string),
    };

    match search_files_authorized(user, query, &options) {
        Ok(body) => body,
        Err(message) => refuse(
            Refusal::from_message(&message),
            format!("Failed to search files: {}", message),
        ),
    }
}

fn tool_read_logs(args: &Value, user: &UserContext) -> Value {
    let uri = arg_str(args, "script");
    let since = match arg_str(args, "since") {
        Some(raw) => match parse_since(raw) {
            Some(since) => Some(since),
            None => {
                return refuse(
                    Refusal::BadRequest,
                    format!("Invalid 'since' value: {}", raw),
                );
            }
        },
        None => None,
    };
    let limit = args.get("limit").and_then(Value::as_i64);
    if limit.is_some_and(|limit| limit <= 0) {
        return refuse(
            Refusal::BadRequest,
            "Parameter 'limit' must be greater than zero",
        );
    }

    let query = repository::LogQuery {
        script_uri: uri.map(str::to_string),
        level: arg_str(args, "level").map(str::to_string),
        since,
        after_seq: args.get("after_seq").and_then(Value::as_i64),
        contains: arg_str(args, "contains").map(str::to_string),
        request_id: arg_str(args, "request_id").map(str::to_string),
        kind: arg_str(args, "kind").map(str::to_string),
        revision: args
            .get("revision")
            .and_then(Value::as_i64)
            .map(|revision| revision as i32),
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
        Err(e) => refuse(Refusal::Failed, format!("Failed to fetch logs: {}", e)),
    }
}

fn tool_read_audit(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100);
    if limit <= 0 {
        return refuse(
            Refusal::BadRequest,
            "Parameter 'limit' must be greater than zero",
        );
    }
    match read_audit_authorized(
        user,
        uri,
        arg_str(args, "action"),
        args.get("before_id").and_then(Value::as_i64),
        limit,
    ) {
        Ok(events) => json!({
            "script": uri,
            "events": events,
            "count": events.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(refusal) => refusal,
    }
}

/// A script's audit events: `ViewLogs` and ownership of the script, or an
/// administrator.
///
/// Stricter than the log, which `ViewLogs` alone reads: an audit trail names
/// people and addresses, and is read by whoever answers for the solution.
pub fn read_audit_authorized(
    user: &UserContext,
    uri: &str,
    action: Option<&str>,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<crate::script_audit::AuditEvent>, Value> {
    if !user.has_capability(&Capability::ViewLogs) || !is_admin_or_owner(user, uri) {
        return Err(refuse(
            Refusal::Forbidden,
            "Only the script's owner or an administrator may read its audit events",
        ));
    }
    crate::database::run_blocking(crate::script_audit::query(uri, action, before_id, limit))
        .map_err(|e| {
            refuse(
                Refusal::Failed,
                format!("Failed to read audit events: {}", e),
            )
        })
}

fn tool_clear_logs(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return refuse(
            Refusal::BadRequest,
            "uri is required: name the script whose logs to clear",
        );
    };
    match delete_logs_authorized(user, uri) {
        Ok(body) => body,
        Err(e) => refuse(Refusal::Failed, format!("Failed to delete logs: {}", e)),
    }
}

fn tool_exposure_report(_args: &Value, user: &UserContext) -> Value {
    match exposure_report_authorized(user) {
        Ok(report) => json!({
            "publicDir": crate::exposure::PUBLIC_DIR,
            "resourceDir": crate::exposure::RESOURCE_DIR,
            "enforced": true,
            "refused": report.refused,
            "unclassified": report.unclassified,
            "scripts": report.scripts,
            "collided": report.collisions.len(),
            "collisions": report.collisions,
            "timestamp": iso_timestamp(),
        }),
        Err(e) => refuse(
            Refusal::Failed,
            format!("Failed to build the exposure report: {}", e),
        ),
    }
}

fn tool_list_routes(args: &Value, user: &UserContext) -> Value {
    let host = arg_str(args, "host");
    match routes_introspection_authorized(user) {
        Ok(routes) => {
            let routes = match host {
                Some(host) => crate::database::run_blocking(filter_routes_by_host(
                    routes,
                    &crate::hosts::canonical_host(Some(host)),
                )),
                None => routes,
            };
            json!({
            "host": host,
            "routes": routes,
            "count": routes.len(),
            "timestamp": iso_timestamp(),
            })
        }
        Err(e) => refuse(Refusal::Failed, format!("Failed to list routes: {}", e)),
    }
}

fn tool_read_init_status(args: &Value, user: &UserContext) -> Value {
    match arg_str(args, "script") {
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

fn tool_list_files(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let files = list_assets_authorized(user, script);
    json!({
        "script": script,
        "files": files,
        "count": files.len(),
        "timestamp": iso_timestamp(),
    })
}

fn tool_read_file(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    let options = match read_options(
        arg_str(args, "lines"),
        arg_str(args, "grep").map(str::to_string),
    ) {
        Ok(options) => options,
        Err(message) => return refuse_text(message),
    };
    match read_file_authorized(user, script, path, &options) {
        Ok(read) => {
            let mut body = read.to_json();
            if let Some(object) = body.as_object_mut() {
                object.insert("script".to_string(), json!(script));
                object.insert("path".to_string(), json!(path));
                // What a pinned script serves is not what this read answered
                // with, and that is worth saying wherever it is true rather
                // than only for the entrypoint, since a module is pinned
                // with it.
                if let Some(note) = deployment_note(script) {
                    object.insert("deployment".to_string(), note);
                }
                object.insert("timestamp".to_string(), json!(iso_timestamp()));
            }
            body
        }
        Err(FileReadError::AccessDenied) => refuse(Refusal::Forbidden, "Error: Access denied"),
        Err(FileReadError::NotFound) => {
            refuse(Refusal::NotFound, format!("File not found: {}", path))
        }
        Err(FileReadError::Validation(message)) => refuse(Refusal::BadRequest, message),
    }
}

fn tool_write_file(args: &Value, user: &UserContext) -> Value {
    write_one_file(args, user, false)
}

/// `write_file` and `create_file`, which differ only in whether an existing
/// path is an error. That was the whole of `create_asset`'s body, repeated.
fn write_one_file(args: &Value, user: &UserContext, if_absent: bool) -> Value {
    let verb = if if_absent { "create" } else { "write" };
    let done = if if_absent { "created" } else { "written" };
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    // Inferred rather than required: a caller writing `lib/util.ts` should
    // not have to know what the engine calls a TypeScript file.
    let mimetype = arg_str(args, "mimetype")
        .map(str::to_string)
        .unwrap_or_else(|| mimetype_for(path).to_string());
    let content = match asset_content_from_request(
        &format!("file '{}'", path),
        "content",
        arg_str(args, "text").map(str::to_string),
        arg_str(args, "content").map(str::to_string),
    ) {
        Ok(content) => content,
        Err(message) => return refuse_text(message),
    };

    match write_file_bytes_authorized(user, script, path, &mimetype, content, if_absent) {
        Ok(revision) => json!({
            "success": true,
            "message": format!("File '{}' {} successfully", path, done),
            "script": script,
            "path": path,
            "revision": revision,
            "timestamp": iso_timestamp(),
        }),
        Err(AssetWriteError::Exists(path)) => {
            refuse(Refusal::Conflict, format!("File already exists: {}", path))
        }
        Err(AssetWriteError::AccessDenied(message)) => refuse(
            Refusal::Forbidden,
            format!("Failed to {} file: {}", verb, message),
        ),
        Err(AssetWriteError::Validation(msg)) => refuse(
            Refusal::BadRequest,
            format!("Failed to {} file: {}", verb, msg),
        ),
        Err(AssetWriteError::Storage(msg)) => {
            refuse(Refusal::Failed, format!("Failed to {} file: {}", verb, msg))
        }
    }
}

/// Write several of a script's assets as one unit — the MCP face of
/// [`assets_batch_route`], down to the shape of its answer.
fn tool_write_files(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let root = arg_str(args, "content");
    let remove: Vec<String> = match args.get("remove") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(paths)) => {
            let mut collected = Vec::with_capacity(paths.len());
            for (index, path) in paths.iter().enumerate() {
                let Some(path) = path.as_str() else {
                    return refuse(
                        Refusal::BadRequest,
                        format!("remove[{}]: expected an asset path", index),
                    );
                };
                collected.push(path.to_string());
            }
            collected
        }
        Some(_) => {
            return refuse(
                Refusal::BadRequest,
                "'remove' must be an array of asset paths",
            );
        }
    };
    let empty: Vec<Value> = Vec::new();
    let files = match args.get("files") {
        Some(Value::Array(files)) => files,
        None | Some(Value::Null) if root.is_some() || !remove.is_empty() => &empty,
        _ => return missing_arg("files"),
    };
    let reinit = match ReinitMode::parse(arg_str(args, "reinit")) {
        Ok(reinit) => reinit,
        Err(message) => return refuse_text(message),
    };

    let mut writes = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let Some(name) = arg_str(file, "name").or_else(|| arg_str(file, "asset")) else {
            return refuse(
                Refusal::BadRequest,
                format!("files[{}]: missing required field: name", index),
            );
        };
        let content = match asset_content_from_request(
            &format!("files[{}] ('{}')", index, name),
            "content_base64",
            arg_str(file, "text")
                .or_else(|| arg_str(file, "source"))
                .map(str::to_string),
            arg_str(file, "content_base64")
                .or_else(|| arg_str(file, "content"))
                .map(str::to_string),
        ) {
            Ok(content) => content,
            Err(message) => return refuse_text(message),
        };
        writes.push(AssetWrite {
            name: name.to_string(),
            mimetype: arg_str(file, "mimetype").map(str::to_string),
            content,
            expected_sha256: arg_str(file, "sha256").map(str::to_string),
        });
    }

    with_root_content(&mut writes, script, root);

    match write_script_files_authorized(
        user,
        script,
        ScriptFilesChange {
            writes: &writes,
            delete: &remove,
        },
        ScriptWriteOptions::default(),
    ) {
        Ok(outcome) => {
            // Bridging back to async, as `check_script` does, so the caller is
            // told what init() did rather than that it was started.
            let init = match (reinit, outcome.changed()) {
                (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
                (ReinitMode::After, false) => {
                    json!({ "ran": false, "reason": "no files changed" })
                }
                (ReinitMode::After, true) => {
                    crate::database::run_blocking(reinitialize_after_write(script))
                }
            };
            let check = outcome
                .changed()
                .then(|| check_after_write(script, user, args))
                .flatten();
            let mut body = batch_outcome_json(script, &outcome, init);
            if let Some(object) = body.as_object_mut() {
                object.insert("success".to_string(), json!(true));
                if let Some(check) = check {
                    object.insert("check".to_string(), check);
                }
            }
            body
        }
        Err(AssetWriteError::Exists(asset)) => refuse(
            Refusal::Conflict,
            format!("Asset already exists: {}", asset),
        ),
        Err(AssetWriteError::AccessDenied(message)) => refuse(
            Refusal::Forbidden,
            format!("Failed to write assets: {}", message),
        ),
        Err(AssetWriteError::Validation(msg)) => refuse(
            Refusal::BadRequest,
            format!("Failed to write assets: {}", msg),
        ),
        Err(AssetWriteError::Storage(msg)) => {
            refuse(Refusal::Failed, format!("Failed to write assets: {}", msg))
        }
    }
}

fn tool_create_file(args: &Value, user: &UserContext) -> Value {
    write_one_file(args, user, true)
}

fn tool_edit_file(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    let Some(edits) = args.get("edits").and_then(Value::as_array) else {
        return missing_arg("edits");
    };
    let reinit = match ReinitMode::parse(arg_str(args, "reinit")) {
        Ok(reinit) => reinit,
        Err(message) => return refuse_text(message),
    };

    let prepared = match parse_tool_edits(edits) {
        Ok(prepared) => prepared,
        Err(message) => return refuse_text(message),
    };

    match patch_file_authorized(
        user,
        script,
        path,
        &prepared,
        arg_str(args, "base_sha256").or_else(|| arg_str(args, "sha256")),
        Some("mcp"),
    ) {
        Ok(outcome) => {
            // Bridging back to async, as write_files does, so the caller is
            // told what init() did rather than that it was started.
            let init = match (reinit, outcome.status) {
                (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
                (_, "unchanged") => json!({ "ran": false, "reason": "no change" }),
                (ReinitMode::After, _) => {
                    crate::database::run_blocking(reinitialize_after_write(script))
                }
            };
            let check = (outcome.status != "unchanged")
                .then(|| check_after_write(script, user, args))
                .flatten();
            let mut body = outcome.to_json();
            if let Some(object) = body.as_object_mut() {
                object.insert("success".to_string(), json!(true));
                object.insert("script".to_string(), json!(script));
                object.insert("path".to_string(), json!(path));
                object.insert("init".to_string(), init);
                if let Some(check) = check {
                    object.insert("check".to_string(), check);
                }
                if let Some(note) = deployment_note(script) {
                    object.insert("deployment".to_string(), note);
                }
                object.insert("timestamp".to_string(), json!(iso_timestamp()));
            }
            body
        }
        Err(PatchError::AccessDenied(message)) => refuse(
            Refusal::Forbidden,
            format!("Failed to edit file: {}", message),
        ),
        Err(PatchError::NotFound) => refuse(Refusal::NotFound, format!("File not found: {}", path)),
        Err(PatchError::Conflict { expected, actual }) => json!({
            "status": Refusal::Conflict.status(),
            "error": format!(
                "'{}' has changed since it was read (expected {}, stored {})",
                path, expected, actual
            ),
            "script": script,
            "path": path,
            "expected_sha256": expected,
            "sha256": actual,
        }),
        Err(PatchError::Validation(message)) => refuse(
            Refusal::BadRequest,
            format!("Failed to edit file: {}", message),
        ),
        Err(PatchError::Storage(message)) => {
            refuse(Refusal::Failed, format!("Failed to edit file: {}", message))
        }
    }
}

fn tool_delete_file(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    match delete_asset_authorized(user, script, path) {
        Ok((true, revision)) => json!({
            "success": true,
            "message": format!("File '{}' deleted successfully", path),
            "script": script,
            "path": path,
            "revision": revision,
            "timestamp": iso_timestamp(),
        }),
        Ok((false, _)) => refuse(Refusal::NotFound, format!("File '{}' not found", path)),
        Err(_) => refuse(Refusal::Forbidden, "Failed to delete file: Access denied"),
    }
}

fn owner_change_error_json(error: OwnerChangeError) -> Value {
    match error {
        OwnerChangeError::AccessDenied => refuse(
            Refusal::Forbidden,
            "Permission denied. You must be an administrator or owner",
        ),
        OwnerChangeError::LastOwner => refuse(
            Refusal::Conflict,
            "Cannot remove the last owner. Transfer ownership to another user first, or contact an administrator.",
        ),
        OwnerChangeError::Storage(details) => refuse(Refusal::Failed, details),
    }
}

fn secret_error_json(error: SecretAccessError) -> Value {
    match error {
        SecretAccessError::AccessDenied => refuse(
            Refusal::Forbidden,
            "Permission denied. You must be an administrator or owner of the script",
        ),
        SecretAccessError::Validation(details) => refuse(Refusal::BadRequest, details),
        SecretAccessError::Storage(details) => refuse(Refusal::Failed, details),
    }
}

fn tool_list_script_owners(args: &Value, _user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    match owners_authorized(uri) {
        Ok(owners) => json!({
            "uri": uri,
            "owners": owners,
            "count": owners.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(details) => refuse(Refusal::Failed, details),
    }
}

fn tool_add_script_owner(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
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
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
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
        Ok(false) => refuse(
            Refusal::NotFound,
            format!("Owner '{}' was not found for script '{}'", owner, uri),
        ),
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
        Ok(false) => refuse(
            Refusal::NotFound,
            format!("Secret '{}' not found for script '{}'", key, script),
        ),
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
        UserAdminError::AccessDenied => refuse(
            Refusal::Forbidden,
            "Permission denied. Administrator privileges are required",
        ),
        UserAdminError::UserNotFound(id) => {
            refuse(Refusal::NotFound, format!("User not found: {}", id))
        }
        UserAdminError::LastAdministrator => refuse(
            Refusal::Conflict,
            "Cannot remove the last administrator. Grant the Administrator role to another user first.",
        ),
        UserAdminError::Validation(details) => refuse(Refusal::BadRequest, details),
        UserAdminError::Storage(details) => refuse(Refusal::Failed, details),
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
    let (kind, message) = match error {
        ScriptHostError::AccessDenied => (
            Refusal::Forbidden,
            "Permission denied. Administrator privileges are required to change where a script is published".to_string(),
        ),
        ScriptHostError::ScriptNotFound(uri) => {
            (Refusal::NotFound, format!("Script not found: {}", uri))
        }
        // Includes the refusal to move a script onto a host that already
        // holds one of its paths, which is a conflict with what is stored.
        ScriptHostError::Validation(details) => (Refusal::from_message(&details), details),
        ScriptHostError::Storage(details) => (Refusal::Failed, details),
    };
    let mut body = refuse(kind, message);
    body["timestamp"] = json!(iso_timestamp());
    body
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
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    match get_script_hosts_authorized(user, uri) {
        Ok((stored, effective)) => script_hosts_json(uri, stored, effective),
        Err(error) => script_host_error_json(error),
    }
}

fn tool_set_script_hosts(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
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

fn tool_set_user_realm(args: &Value, user: &UserContext) -> Value {
    let Some(user_id) = arg_str(args, "user_id") else {
        return missing_arg("user_id");
    };
    let Some(realm) = arg_str(args, "realm") else {
        return missing_arg("realm");
    };
    match set_user_realm_authorized(user, user_id, realm) {
        Ok(realm) => json!({
            "success": true,
            "userId": user_id,
            "realm": realm,
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
    /// Every tool's description and schema is paid for in every agent's
    /// context, listed or not used. A description says what the tool does and
    /// the one rule that is not obvious; the rest is the schema's job.
    #[test]
    fn the_tool_listing_stays_small() {
        let tools = super::native_mcp_tool_descriptors();
        for tool in &tools {
            assert!(
                tool.description.len() <= 500,
                "{} has a {}-character description; keep it to what it does and one rule",
                tool.name,
                tool.description.len()
            );
        }
        let total: usize = tools
            .iter()
            .map(|tool| tool.description.len() + tool.input_schema.to_string().len())
            .sum();
        assert!(
            total <= 26_000,
            "the tool listing is {total} characters; cut before adding"
        );
    }

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
