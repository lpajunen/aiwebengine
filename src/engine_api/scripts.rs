//! Writing, reading, renaming and deleting scripts, re-running their `init()`, and
//! their owners.

use super::*;
use crate::repository;
use crate::revisions;
use crate::security::{
    Capability, SecurityEvent, SecurityEventType, SecuritySeverity, UserContext,
};
use serde_json::{Value, json};
use tracing::{info, warn};

/// Re-initialize a script after an upsert: clear its MCP registrations and
/// run init().
///
/// Every local deploy path funnels through here, so what a script upsert does
/// to a script's registrations and what a batch asset write does to them
/// cannot drift apart.
pub(super) async fn reinitialize_script(script_uri: &str) -> crate::script_init::InitResult {
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
pub(super) fn spawn_script_init(script_uri: String) {
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
pub(super) async fn resolve_revision(script_uri: &str, spec: &str) -> Result<i32, String> {
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
pub(super) async fn resolve_view(
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
pub(super) fn check_after_write(script: &str, user: &UserContext, args: &Value) -> Option<Value> {
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

pub(super) fn init_result_json(result: &crate::script_init::InitResult) -> Value {
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
    pub(super) fn as_str(&self) -> &'static str {
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
pub(super) fn authorize_script_write(user: &UserContext, uri: &str) -> Result<bool, String> {
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
/// Two rules, by file. The entrypoint is
/// the script's source and is read under `ReadScripts`, which asks nothing
/// about ownership — a solution's code is readable by any reader.
/// Every other file is an asset and takes `ReadAssets` *plus* ownership.
/// Collapsing to the first rule would publish every script's private files;
/// collapsing to the second would make source unreadable to anyone but its
/// owner.
pub(super) fn can_read_file(user: &UserContext, script_uri: &str, path: &str) -> bool {
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

/// Init status for one script; an authenticated caller with the ReadScripts
/// capability.
pub fn init_status_authorized(user: &UserContext, uri: &str) -> Option<Value> {
    if !may_administer(user) || user.require_capability(&Capability::ReadScripts).is_err() {
        return None;
    }
    let metadata = repository::get_script_metadata(uri).ok()?;
    Some(script_init_status_json(&metadata))
}

pub(super) fn script_init_status_json(metadata: &repository::ScriptMetadata) -> Value {
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

/// Whether a batch write runs the script's init() when it is done.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReinitMode {
    /// Run init() once, after the whole batch has landed, and report it.
    After,
    /// Leave init() alone — for a caller pushing one part of a larger change
    /// that is not coherent yet.
    Never,
}

impl ReinitMode {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, String> {
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

pub(super) fn tool_list_tasks(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_cancel_task(args: &Value, user: &UserContext) -> Value {
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

/// The scripts in the engine. A script is a tree; `tool_list_files` lists one
/// tree's files.
pub(super) fn tool_list_scripts(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_rename_script(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_delete_script(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_read_init_status(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn owner_change_error_json(error: OwnerChangeError) -> Value {
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

pub(super) fn tool_list_script_owners(args: &Value, _user: &UserContext) -> Value {
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

pub(super) fn tool_add_script_owner(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_remove_script_owner(args: &Value, user: &UserContext) -> Value {
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
