//! `files`: a script's own tree, never its entrypoint.

use super::*;
use crate::repository;
use crate::security::{
    Capability, SecurityAuditor, SecurityEventType, SecuritySeverity, UserContext,
};
use base64::Engine;
use rquickjs::{Function, Result as JsResult};

/// Builds `files` over `__hostFiles`.
pub(super) const FILES_PRELUDE: &str = include_str!("../../../assets/files_prelude.js");

/// Why `files` refuses to touch a script's entrypoint.
///
/// Merging the root source into the tree made `main.*` reachable by every
/// path that reaches a file, and this one is gated by `WriteAssets` and
/// `DeleteAssets` alone — no ownership check is needed here, because a script
/// only ever reaches its *own* files. That combination would have let a script
/// rewrite or delete its own program while serving a request from anybody
/// holding the editor tier, which is a thing the file API could not do before
/// the merge and a thing nobody asked for it to start doing.
///
/// Refused rather than re-gated on `WriteScripts`, because the engine never
/// offered a script a way to edit its own program and a merge of two storage
/// shapes is not the moment to start. `engine.call("write_file", ...)` is the
/// deliberate way, and it applies the same rules the endpoint does.
pub(super) const ENTRYPOINT_IS_NOT_A_FILE: &str = "a script's entrypoint is not writable through files. \
     Use engine.call(\"write_file\", { script, path, text }), which applies \
     the checks writing a script's program takes.";

/// A file path a script may write: the same rules the repository applies.
pub(super) fn validate_file_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > repository::MAX_ASSET_URI_CHARS {
        return Err(format!(
            "a path is 1-{} characters",
            repository::MAX_ASSET_URI_CHARS
        ));
    }
    if path.contains("..") || path.contains('\\') || path.starts_with('/') {
        return Err(format!(
            "'{}' is not a path inside this script's tree",
            path
        ));
    }
    Ok(())
}

pub(super) fn millis_since_epoch(time: std::time::SystemTime) -> f64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

/// File a write or a removal through `files`, off the JavaScript thread.
pub(super) fn audit_file_change(
    auditor: &SecurityAuditor,
    user: &UserContext,
    action: &str,
    severity: SecuritySeverity,
    script_uri: &str,
    path: &str,
) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let auditor = auditor.clone();
    let user_id = user.user_id.clone();
    let action = action.to_string();
    let script_uri = script_uri.to_string();
    let path = path.to_string();
    rt.spawn(async move {
        let _ = auditor
            .log_event(
                crate::security::SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    severity,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action(action)
                .with_detail("uri", &path)
                .with_detail("script_uri", &script_uri),
            )
            .await;
    });
}

impl SecureGlobalContext {
    /// Setup secure asset management functions
    /// Install `__hostFiles` and the prelude that builds `files` over it.
    ///
    /// A script's own tree, read and written by path. This was `assetStorage`
    /// — `listAssets`, `fetchAsset`, `upsertAsset`, `deleteAsset` — which
    /// handed back base64 for every read, a sentence for a missing file that
    /// was not base64, and `"Error: ..."` as a value, so each caller decoded,
    /// then guessed which of the three it had. Every caller in practice wanted
    /// text. A read now answers text, or `null` when there is no such file;
    /// binary is asked for by name; failures throw.
    ///
    /// The entrypoint stays out of reach, for the reason
    /// [`ENTRYPOINT_IS_NOT_A_FILE`] gives.
    pub(super) fn setup_asset_management_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let host = rquickjs::Object::new(ctx.clone())?;

        let user_list = self.user_context.clone();
        let script_list = script_uri.to_string();
        let list = Function::new(ctx.clone(), move || -> String {
            if let Err(e) = user_list.require_capability(&Capability::ReadAssets) {
                return host_failure("Error", &format!("files.list: {}", e));
            }
            let mut entries: Vec<serde_json::Value> = repository::fetch_assets(&script_list)
                .values()
                .map(|asset| {
                    serde_json::json!({
                        "path": asset.uri,
                        "size": asset.content.len(),
                        "mimetype": asset.mimetype,
                        "createdAt": millis_since_epoch(asset.created_at),
                        "updatedAt": millis_since_epoch(asset.updated_at),
                    })
                })
                .collect();
            // Sorted, so a listing is the same bytes every time it is the
            // same tree — which matters to a script that puts one into a
            // prompt it wants cached.
            entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
            host_ok(serde_json::Value::Array(entries))
        })?;
        host.set("list", list)?;

        let user_read = self.user_context.clone();
        let script_read = script_uri.to_string();
        let read = Function::new(
            ctx.clone(),
            move |path: String, base64: Option<bool>| -> String {
                if let Err(e) = user_read.require_capability(&Capability::ReadAssets) {
                    return host_failure("Error", &format!("files.read: {}", e));
                }
                let Some(asset) = repository::fetch_asset(&script_read, &path) else {
                    return host_ok(serde_json::Value::Null);
                };
                if base64.unwrap_or(false) {
                    return host_ok(serde_json::Value::String(
                        base64::engine::general_purpose::STANDARD.encode(asset.content),
                    ));
                }
                match String::from_utf8(asset.content) {
                    Ok(text) => host_ok(serde_json::Value::String(text)),
                    Err(_) => host_failure(
                        "TypeError",
                        &format!(
                            "files.read: '{}' is not text; read it with {{ encoding: \"base64\" }}",
                            path
                        ),
                    ),
                }
            },
        )?;
        host.set("read", read)?;

        let user_write = self.user_context.clone();
        let auditor_write = self.auditor.clone();
        let script_write = script_uri.to_string();
        let write = Function::new(
            ctx.clone(),
            move |path: String,
                  content: String,
                  base64: Option<bool>,
                  mimetype: Option<String>|
                  -> String {
                if crate::module_loader::is_root_module_name(&path) {
                    return host_failure("Error", ENTRYPOINT_IS_NOT_A_FILE);
                }
                if let Err(e) = user_write.require_capability(&Capability::WriteAssets) {
                    return host_failure("Error", &format!("files.write: {}", e));
                }
                if let Err(message) = validate_file_path(&path) {
                    return host_failure("TypeError", &format!("files.write: {}", message));
                }
                let bytes = if base64.unwrap_or(false) {
                    match base64::engine::general_purpose::STANDARD.decode(&content) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            return host_failure(
                                "TypeError",
                                &format!("files.write: the content is not base64: {}", e),
                            );
                        }
                    }
                } else {
                    content.into_bytes()
                };
                // The storage-side limit, so this refuses exactly what the
                // repository would refuse rather than a little more.
                if bytes.len() > repository::MAX_ASSET_CONTENT_BYTES {
                    return host_failure(
                        "RangeError",
                        &format!(
                            "files.write: '{}' is {} bytes (max {})",
                            path,
                            bytes.len(),
                            repository::MAX_ASSET_CONTENT_BYTES
                        ),
                    );
                }
                let mimetype = mimetype
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| crate::engine_api::mimetype_for(&path).to_string());

                audit_file_change(
                    &auditor_write,
                    &user_write,
                    "upsert",
                    SecuritySeverity::Medium,
                    &script_write,
                    &path,
                );

                let now = std::time::SystemTime::now();
                let asset = repository::Asset {
                    uri: path.clone(),
                    name: Some(path.clone()),
                    mimetype,
                    content: bytes,
                    created_at: now,
                    updated_at: now,
                    script_uri: script_write.clone(),
                };
                match repository::upsert_asset(asset) {
                    Ok(_) => {
                        // A write here is a write to the script, and every
                        // other path that changes a script's files records
                        // what it consisted of afterwards — so a file a script
                        // writes (an agent's skill, say: model-authored content
                        // somebody may want to read back or undo) has history,
                        // can be reverted, and moves a git binding off
                        // `in_sync`. Recorded after the write and not instead
                        // of it: a history that cannot be written is worth
                        // less than the content, so `record_blocking` reports
                        // failure rather than propagating it.
                        crate::revisions::record_blocking(
                            &script_write,
                            crate::revisions::Origin::Sandbox,
                            user_write.user_id.as_deref(),
                        );
                        host_ok(serde_json::Value::Null)
                    }
                    Err(e) => host_failure("Error", &format!("files.write: {}", e)),
                }
            },
        )?;
        host.set("write", write)?;

        let user_delete = self.user_context.clone();
        let auditor_delete = self.auditor.clone();
        let script_delete = script_uri.to_string();
        let delete = Function::new(ctx.clone(), move |path: String| -> String {
            if crate::module_loader::is_root_module_name(&path) {
                return host_failure("Error", ENTRYPOINT_IS_NOT_A_FILE);
            }
            if let Err(e) = user_delete.require_capability(&Capability::DeleteAssets) {
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    let auditor = auditor_delete.clone();
                    let user_id = user_delete.user_id.clone();
                    rt.spawn(async move {
                        let _ = auditor
                            .log_authz_failure(
                                user_id,
                                "asset".to_string(),
                                "delete".to_string(),
                                "DeleteAssets".to_string(),
                            )
                            .await;
                    });
                }
                return host_failure("Error", &format!("files.delete: {}", e));
            }
            audit_file_change(
                &auditor_delete,
                &user_delete,
                "delete",
                SecuritySeverity::High,
                &script_delete,
                &path,
            );
            if !repository::delete_asset(&script_delete, &path) {
                return host_ok(serde_json::Value::Bool(false));
            }
            // After a removal nothing else in the engine still holds the
            // content, which makes this the revision most worth having.
            crate::revisions::record_blocking(
                &script_delete,
                crate::revisions::Origin::Delete,
                user_delete.user_id.as_deref(),
            );
            host_ok(serde_json::Value::Bool(true))
        })?;
        host.set("delete", delete)?;

        ctx.globals().set("__hostFiles", host)?;
        crate::bytecode::eval_program(ctx, "engine://files-prelude", FILES_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "files",
                    "prelude",
                    &format!("files prelude failed to load: {}", e),
                )
            },
        )?;
        Ok(())
    }
}
