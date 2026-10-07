//! Per-script settings an administrator owns: host bindings and execution limits.

use super::*;
use crate::repository;
use crate::security::UserContext;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::info;

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

pub(super) fn script_limits_to_json(limits: &crate::script_limits::ScriptLimits) -> Value {
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
pub(super) fn validate_hosts(requested: &[String]) -> Result<Vec<String>, ScriptHostError> {
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

pub(super) fn tool_set_script_limits(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn tool_get_script_limits(args: &Value, user: &UserContext) -> Value {
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

pub(super) fn script_host_error_json(error: ScriptHostError) -> Value {
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

pub(super) fn script_hosts_json(uri: &str, stored: Vec<String>, effective: Vec<String>) -> Value {
    json!({
        "uri": uri,
        "hosts": stored,
        "publishedOn": effective,
        "servedHosts": crate::hosts::all_hosts(),
        "defaultHost": crate::hosts::default_host(),
        "timestamp": iso_timestamp(),
    })
}

pub(super) fn tool_get_script_hosts(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    match get_script_hosts_authorized(user, uri) {
        Ok((stored, effective)) => script_hosts_json(uri, stored, effective),
        Err(error) => script_host_error_json(error),
    }
}

pub(super) fn tool_set_script_hosts(args: &Value, user: &UserContext) -> Value {
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
