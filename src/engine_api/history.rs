//! Revisions, reverts and deployment pins.

use super::*;
use crate::repository;
use crate::revisions;
use crate::security::{Capability, UserContext};
use serde_json::{Value, json};

/// Tell a caller editing a pinned script that its write will not serve.
///
/// Reads and edits act on head, and a pinned script serves an older revision,
/// so the write lands somewhere the requests do not look. That is the whole
/// point of pinning and is invisible from an answer that only reports what was
/// written — so the answer says it, in the one case where it is true.
pub(super) fn deployment_note(script_uri: &str) -> Option<Value> {
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

/// Apply a deployment on this instance and tell the rest of the cluster.
///
/// Everything after the pin is what makes it take effect *here*: the source
/// cache holds what a script serves, the prepared program is built from it,
/// and `init()` registers what that version registers. The notification hands
/// the same sequence to every other instance.
pub(super) async fn activate_deployment(script: &str) -> Value {
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

pub(super) fn deployment_to_json(deployment: &crate::deployments::Deployment) -> Value {
    json!({
        "revision": deployment.revision,
        "at": deployment.deployed_at.to_rfc3339(),
        "by": deployment.deployed_by,
        "initOk": deployment.init_ok,
        "initError": deployment.init_error,
    })
}

// ============================================================================
// Script tasks
// ============================================================================

pub(super) fn file_diff_to_json(file: &revisions::FileDiff) -> Value {
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
pub(super) fn can_read_history(user: &UserContext, script_uri: &str) -> bool {
    can_access_assets(user, script_uri, &Capability::ReadAssets)
        && can_access_assets(user, script_uri, &Capability::ReadScripts)
}

/// Whether `user` may change a script's history or restore from it.
///
/// A revert writes the root as readily as it writes a module, so it takes what
/// writing either takes.
pub(super) fn can_write_history(user: &UserContext, script_uri: &str) -> bool {
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
    /// the files they list.
    pub fn changed_anything(&self) -> bool {
        !self.assets_written.is_empty() || !self.assets_deleted.is_empty()
    }

    pub(super) fn to_json(&self) -> Value {
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
pub(super) fn apply_revert(
    script_uri: &str,
    target: i32,
    writes: Vec<(String, String, Vec<u8>)>,
    deletes: Vec<String>,
    user_id: Option<&str>,
) -> Result<Option<i32>, String> {
    let _guard = crate::database::Database::begin_transaction(None)
        .map_err(|e| format!("Failed to open revert transaction: {}", e))?;

    let result = (|| -> Result<Option<i32>, String> {
        // The entrypoint is one of the writes, restored like any other file.
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

pub(super) fn revision_to_json(revision: &revisions::Revision) -> Value {
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

pub(super) fn revision_file_to_json(file: &revisions::RevisionFile) -> Value {
    json!({
        "uri": file.uri,
        "name": file.name,
        "mimetype": file.mimetype,
        "sha256": file.sha256,
        "bytes": file.bytes,
    })
}

/// Run a script's tests and return the same report the REST endpoint serves.
///
/// This runs on the blocking pool: the MCP dispatcher moves tool execution
/// there, because a run is JavaScript executed to completion under a budget
/// measured in seconds. That is also why the whole-run ceiling is enforced
/// inside the run loop rather than by an outer timeout — there is no async
/// backstop on this path, and the in-loop ceiling is the one that can still
/// report the modules that finished.
pub(super) fn tool_list_revisions(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_diff_revisions(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_deploy_script(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_get_deployment(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_label_revision(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_revert_script(args: &Value, user: &UserContext) -> Value {
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
