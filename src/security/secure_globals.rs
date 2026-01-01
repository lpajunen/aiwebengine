use base64::Engine;
use chrono::Duration as ChronoDuration;
use rquickjs::{Function, Result as JsResult, function::Opt};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, warn};

use crate::repository;
use crate::scheduler;
use crate::secrets::SecretsManager;
use crate::security::{
    SecureOperations, SecurityAuditor, SecurityEventType, SecuritySeverity, UserContext,
};

// Type alias for route registration callback function
type RouteRegisterFn =
    Box<dyn Fn(&str, &repository::RouteMetadata, Option<&str>) -> Result<(), rquickjs::Error>>;

/// Secure wrapper for JavaScript global functions that enforces Rust-level validation
pub struct SecureGlobalContext {
    user_context: UserContext,
    secure_ops: SecureOperations,
    auditor: SecurityAuditor,
    config: GlobalSecurityConfig,
    secrets_manager: Option<Arc<SecretsManager>>,
}

#[derive(Debug, Clone)]
pub struct GlobalSecurityConfig {
    pub enable_graphql_registration: bool,
    pub enable_asset_management: bool,
    pub enable_streams: bool,
    pub enable_script_management: bool,
    pub enable_scheduler: bool,
    pub enable_logging: bool,
    pub enable_secrets: bool,
    pub enforce_strict_validation: bool,
    pub enable_audit_logging: bool, // New flag to disable audit logging in tests
}

impl Default for GlobalSecurityConfig {
    fn default() -> Self {
        Self {
            enable_streams: true,
            enable_graphql_registration: true,
            enable_asset_management: true,
            enable_script_management: true,
            enable_scheduler: true,
            enable_logging: true,
            enable_secrets: true,
            enforce_strict_validation: true,
            enable_audit_logging: true, // Enable by default
        }
    }
}

impl SecureGlobalContext {
    pub fn new(user_context: UserContext) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(pool),
            config: GlobalSecurityConfig::default(),
            secrets_manager: None,
        }
    }

    pub fn new_with_config(user_context: UserContext, config: GlobalSecurityConfig) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(pool),
            config,
            secrets_manager: None,
        }
    }

    pub fn new_with_secrets(
        user_context: UserContext,
        config: GlobalSecurityConfig,
        secrets_manager: Arc<SecretsManager>,
    ) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(pool),
            config,
            secrets_manager: Some(secrets_manager),
        }
    }

    /// Setup all secure global functions in the JavaScript context
    pub fn setup_secure_globals<'js>(
        &self,
        ctx: &'js rquickjs::Ctx<'js>,
        script_uri: &str,
    ) -> JsResult<()> {
        self.setup_secure_functions(ctx, script_uri, None)
    }

    /// Setup secure global functions with optional route registration function
    pub fn setup_secure_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<RouteRegisterFn>,
    ) -> JsResult<()> {
        // Setup routeRegistry object with all route-related functions
        self.setup_route_registry(ctx, script_uri, register_fn)?;

        if self.config.enable_logging {
            self.setup_logging_functions(ctx, script_uri)?;
        }

        if self.config.enable_script_management {
            self.setup_script_management_functions(ctx, script_uri)?;
        }

        if self.config.enable_asset_management {
            self.setup_asset_management_functions(ctx, script_uri)?;
        }

        if self.config.enable_secrets {
            self.setup_secrets_functions(ctx, script_uri)?;
        }

        // Setup fetch() function for HTTP requests
        self.setup_fetch_function(ctx, script_uri)?;

        // Setup database functions
        self.setup_database_functions(ctx, script_uri)?;

        // Setup conversion functions (always enabled)
        self.setup_conversion_functions(ctx, script_uri)?;

        // Setup script storage functions
        self.setup_shared_storage_functions(ctx, script_uri)?;

        // Setup personal storage functions
        self.setup_personal_storage_functions(ctx, script_uri)?;

        // Always setup GraphQL functions, but they will be no-ops if disabled
        self.setup_graphql_functions(ctx, script_uri)?;

        // Setup MCP (Model Context Protocol) functions
        self.setup_mcp_functions(ctx, script_uri)?;

        // Setup user management functions (admin-only)
        self.setup_user_management_functions(ctx, script_uri)?;

        // Setup scheduler service bindings
        self.setup_scheduler_functions(ctx, script_uri)?;

        // Setup message dispatcher bindings
        self.setup_dispatcher_functions(ctx, script_uri)?;

        Ok(())
    }

    /// Setup secure logging functions
    fn setup_logging_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let config = self.config.clone();

        // Secure writeLog function
        let user_ctx_write = user_context.clone();
        let auditor_write = auditor.clone();
        let script_uri_write = script_uri_owned.clone();
        let config_write = config.clone();
        let write_log = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, message: String, level: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_write.require_capability(&crate::security::Capability::ViewLogs)
                {
                    if config_write.enable_audit_logging {
                        let rt = tokio::runtime::Handle::try_current();
                        if let Ok(_rt) = rt {
                            // Only attempt async logging if we're in a runtime
                            let auditor_clone = auditor_write.clone();
                            let user_id = user_ctx_write.user_id.clone();
                            tokio::spawn(async move {
                                let _ = auditor_clone
                                    .log_authz_failure(
                                        user_id,
                                        "log".to_string(),
                                        "write".to_string(),
                                        "ViewLogs".to_string(),
                                    )
                                    .await;
                            });
                        }
                    }
                    return Ok(format!("Error: {}", e));
                }

                // Log the write operation
                if config_write.enable_audit_logging {
                    let rt = tokio::runtime::Handle::try_current();
                    if let Ok(_rt) = rt {
                        let auditor_clone = auditor_write.clone();
                        let user_id = user_ctx_write.user_id.clone();
                        let script_uri_clone = script_uri_write.clone();
                        let message_len = message.len();
                        tokio::spawn(async move {
                            let _ = auditor_clone
                                .log_event(
                                    crate::security::SecurityEvent::new(
                                        SecurityEventType::SystemSecurityEvent,
                                        SecuritySeverity::Low,
                                        user_id,
                                    )
                                    .with_resource("log".to_string())
                                    .with_action("write".to_string())
                                    .with_detail("script_uri", &script_uri_clone)
                                    .with_detail("message_length", message_len.to_string()),
                                )
                                .await;
                        });
                    }
                }

                debug!(
                    script_uri = %script_uri_write,
                    user_id = ?user_ctx_write.user_id,
                    message_len = message.len(),
                    "Secure writeLog called"
                );

                // Call actual repository function
                repository::insert_log_message(&script_uri_write, &message, &level);
                Ok("Log written successfully".to_string())
            },
        )?;

        // Secure listLogs function
        let user_ctx_list = user_context.clone();
        let list_logs = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ViewLogs)
                {
                    // Return empty array if no permission (JavaScript expects a JSON array)
                    return Ok("[]".to_string());
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure console.listLogs called"
                );

                // Fetch all logs from all script URIs
                let logs = repository::fetch_all_log_messages();

                // Create JSON array of log objects
                let log_objects: Vec<serde_json::Value> = logs
                    .iter()
                    .map(|log_entry| {
                        // Convert SystemTime to milliseconds since UNIX_EPOCH
                        let timestamp_ms = log_entry
                            .timestamp
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as f64;

                        serde_json::json!({
                            "message": log_entry.message,
                            "level": log_entry.level,
                            "timestamp": timestamp_ms
                        })
                    })
                    .collect();

                // Serialize to JSON string
                match serde_json::to_string(&log_objects) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        warn!("Failed to serialize logs to JSON: {}", e);
                        Ok("[]".to_string())
                    }
                }
            },
        )?;

        // Secure listLogsForUri function - now returns same format as listLogs
        let user_ctx_list_uri = user_context.clone();
        let list_logs_for_uri = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list_uri.require_capability(&crate::security::Capability::ViewLogs)
                {
                    // Return empty array if no permission (JavaScript expects a JSON array)
                    return Ok("[]".to_string());
                }

                debug!(
                    user_id = ?user_ctx_list_uri.user_id,
                    uri = %uri,
                    "Secure console.listLogsForUri called"
                );

                let logs = repository::fetch_log_messages(&uri);

                // Create JSON array of log objects (same format as listLogs)
                let log_objects: Vec<serde_json::Value> = logs
                    .iter()
                    .map(|log_entry| {
                        // Convert SystemTime to milliseconds since UNIX_EPOCH
                        let timestamp_ms = log_entry
                            .timestamp
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as f64;

                        serde_json::json!({
                            "message": log_entry.message,
                            "level": log_entry.level,
                            "timestamp": timestamp_ms
                        })
                    })
                    .collect();

                // Serialize to JSON string
                match serde_json::to_string(&log_objects) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        warn!("Failed to serialize logs to JSON: {}", e);
                        Ok("[]".to_string())
                    }
                }
            },
        )?;

        // Create console object using JavaScript to avoid multiple ctx.clone() calls
        // This creates wrapper functions in JavaScript space that call write_log with different levels
        // and also attaches listLogs and listLogsForUri as methods
        global.set("__writeLog", write_log)?;
        global.set("__listLogs", list_logs)?;
        global.set("__listLogsForUri", list_logs_for_uri)?;
        // Secure pruneLogs function - allows pruning of logs per repository (keeps 20 entries per script)
        let user_ctx_prune = user_context.clone();
        let auditor_prune = auditor.clone();
        let script_uri_prune = script_uri_owned.clone();
        let config_prune = config.clone();
        let prune_logs = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_prune.require_capability(&crate::security::Capability::DeleteLogs)
                {
                    if config_prune.enable_audit_logging {
                        let rt = tokio::runtime::Handle::try_current();
                        if let Ok(_rt) = rt {
                            let auditor_clone = auditor_prune.clone();
                            let user_id = user_ctx_prune.user_id.clone();
                            tokio::spawn(async move {
                                let _ = auditor_clone
                                    .log_authz_failure(
                                        user_id,
                                        "log".to_string(),
                                        "delete".to_string(),
                                        "DeleteLogs".to_string(),
                                    )
                                    .await;
                            });
                        }
                    }
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    script_uri = %script_uri_prune,
                    user_id = ?user_ctx_prune.user_id,
                    "Secure console.pruneLogs called"
                );

                // Call repository prune
                match repository::prune_log_messages() {
                    Ok(_) => Ok("Pruned logs".to_string()),
                    Err(e) => {
                        warn!("Failed to prune logs: {}", e);
                        Ok(format!("Error: {}", e))
                    }
                }
            },
        )?;
        global.set("__pruneLogs", prune_logs)?;
        ctx.eval::<(), _>(
            r#"
            (function() {
                const writeLog = globalThis.__writeLog;
                const listLogs = globalThis.__listLogs;
                const listLogsForUri = globalThis.__listLogsForUri;
                const pruneLogs = globalThis.__pruneLogs;
                globalThis.console = {
                    log: function(msg) { return writeLog(msg, "LOG"); },
                    info: function(msg) { return writeLog(msg, "INFO"); },
                    warn: function(msg) { return writeLog(msg, "WARN"); },
                    error: function(msg) { return writeLog(msg, "ERROR"); },
                    debug: function(msg) { return writeLog(msg, "DEBUG"); },
                    listLogs: function() { return listLogs(); },
                    listLogsForUri: function(uri) { return listLogsForUri(uri); },
                    pruneLogs: function() { return pruneLogs(); }
                };
                delete globalThis.__writeLog;
                delete globalThis.__listLogs;
                delete globalThis.__listLogsForUri;
                delete globalThis.__pruneLogs;
            })();
        "#,
        )?;

        Ok(())
    }

    /// Setup secure script management functions
    fn setup_script_management_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let _secure_ops = self.secure_ops.clone();
        let auditor = self.auditor.clone();
        let _script_uri_owned = script_uri.to_string();

        // Create scriptStorage object
        let script_storage = rquickjs::Object::new(ctx.clone())?;

        // Secure listScripts function
        let user_ctx_list = user_context.clone();
        let list_scripts = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadScripts)
                {
                    // Return empty array JSON if no permission
                    return Ok("[]".to_string());
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listScripts called"
                );

                let metadata_list = match repository::get_all_script_metadata() {
                    Ok(metadata) => metadata,
                    Err(e) => {
                        warn!("Failed to get script metadata: {}", e);
                        return Ok("[]".to_string());
                    }
                };

                // Build JSON array of script metadata
                let scripts_json: Vec<serde_json::Value> = metadata_list
                    .iter()
                    .map(|meta| {
                        serde_json::json!({
                            "uri": meta.uri,
                            "name": meta.name,
                            "size": meta.content.len(),
                            "updatedAt": meta.updated_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                            "createdAt": meta.created_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                            "privileged": meta.privileged,
                            "initialized": meta.initialized,
                            "initError": meta.init_error.as_deref()
                        })
                    })
                    .collect();

                match serde_json::to_string(&scripts_json) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        warn!("Failed to serialize script metadata: {}", e);
                        Ok("[]".to_string())
                    }
                }
            },
        )?;
        script_storage.set("listScripts", list_scripts)?;

        // Secure getScript function
        let user_ctx_get = user_context.clone();
        let get_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<Option<String>> {
                // Check capability
                if let Err(e) =
                    user_ctx_get.require_capability(&crate::security::Capability::ReadScripts)
                {
                    warn!(
                        user_id = ?user_ctx_get.user_id,
                        script_name = %script_name,
                        error = %e,
                        "getScript capability check failed"
                    );
                    return Ok(None);
                }

                debug!(
                    user_id = ?user_ctx_get.user_id,
                    script_name = %script_name,
                    "Secure getScript called"
                );

                Ok(repository::fetch_script(&script_name))
            },
        )?;
        script_storage.set("getScript", get_script)?;

        // Secure getScriptInitStatus function - returns init metadata
        let user_ctx_meta = user_context.clone();
        let get_script_init_status = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<Option<String>> {
                // Check capability - same as getScript
                if let Err(_e) =
                    user_ctx_meta.require_capability(&crate::security::Capability::ReadScripts)
                {
                    return Ok(None);
                }

                debug!(
                    user_id = ?user_ctx_meta.user_id,
                    script_name = %script_name,
                    "Secure getScriptInitStatus called"
                );

                // Get script metadata from repository
                match repository::get_script_metadata(&script_name) {
                    Ok(metadata) => {
                        // Create a JSON object with init status
                        let status = serde_json::json!({
                            "scriptName": metadata.uri,
                            "initialized": metadata.initialized,
                            "initError": metadata.init_error,
                            "lastInitTime": metadata.last_init_time.and_then(|t| {
                                t.duration_since(std::time::UNIX_EPOCH)
                                    .ok()
                                    .map(|d| d.as_millis() as f64)
                            }),
                            "createdAt": metadata.created_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|d| d.as_millis() as f64),
                            "updatedAt": metadata.updated_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .ok()
                                .map(|d| d.as_millis() as f64),
                        });
                        Ok(Some(status.to_string()))
                    }
                    Err(_) => Ok(None),
                }
            },
        )?;
        script_storage.set("getScriptInitStatus", get_script_init_status)?;

        // Secure getScriptSecurityProfile function
        let user_ctx_security = user_context.clone();
        let get_script_security_profile = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<Option<String>> {
                if let Err(_e) =
                    user_ctx_security.require_capability(&crate::security::Capability::ReadScripts)
                {
                    return Ok(None);
                }

                match repository::get_script_security_profile(&script_name) {
                    Ok(profile) => {
                        let json = serde_json::to_string(&profile).unwrap_or_default();
                        Ok(Some(json))
                    }
                    Err(e) => {
                        warn!(
                            script = %script_name,
                            error = %e,
                            "Failed to get script security profile"
                        );
                        Ok(None)
                    }
                }
            },
        )?;
        script_storage.set("getScriptSecurityProfile", get_script_security_profile)?;

        // Secure setScriptPrivileged function (admin only)
        let user_ctx_set_privileged = user_context.clone();
        let set_script_privileged = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  script_name: String,
                  privileged: bool|
                  -> JsResult<bool> {
                if !user_ctx_set_privileged
                    .has_capability(&crate::security::Capability::DeleteScripts)
                {
                    return Err(rquickjs::Error::new_from_js_message(
                        "setScriptPrivileged",
                        "permission_denied",
                        "Administrator privileges required",
                    ));
                }

                repository::set_script_privileged(&script_name, privileged).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "setScriptPrivileged",
                        "repository_error",
                        &format!("{}", e),
                    )
                })?;

                Ok(true)
            },
        )?;
        script_storage.set("setScriptPrivileged", set_script_privileged)?;

        // Helper to allow UI to detect admin capability
        let user_ctx_manage_privileges = user_context.clone();
        let can_manage_privileges = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<bool> {
                Ok(user_ctx_manage_privileges
                    .has_capability(&crate::security::Capability::DeleteScripts))
            },
        )?;
        script_storage.set("canManageScriptPrivileges", can_manage_privileges)?;

        // Secure getScriptOwners function
        let _user_ctx_get_owners = user_context.clone();
        let get_script_owners = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<String> {
                // Anyone can view owners (for transparency)
                match repository::get_script_owners(&script_name) {
                    Ok(owners) => {
                        // Return as JSON array
                        match serde_json::to_string(&owners) {
                            Ok(json) => Ok(json),
                            Err(e) => Ok(format!("Error serializing owners: {}", e)),
                        }
                    }
                    Err(e) => Ok(format!("Error getting owners: {}", e)),
                }
            },
        )?;
        script_storage.set("getScriptOwners", get_script_owners)?;

        // Secure addScriptOwner function (admin or current owner only)
        let user_ctx_add_owner = user_context.clone();
        let add_script_owner = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  script_name: String,
                  new_owner_id: String|
                  -> JsResult<String> {
                // Check if user is admin or owns the script
                let is_admin =
                    user_ctx_add_owner.has_capability(&crate::security::Capability::DeleteScripts);
                let user_owns = if let Some(user_id) = &user_ctx_add_owner.user_id {
                    repository::user_owns_script(&script_name, user_id).unwrap_or(false)
                } else {
                    false
                };

                if !is_admin && !user_owns {
                    return Ok("Error: Permission denied. You must be an administrator or owner to add owners".to_string());
                }

                // Add the new owner
                match repository::add_script_owner(&script_name, &new_owner_id) {
                    Ok(_) => Ok(format!(
                        "Successfully added owner '{}' to script '{}'",
                        new_owner_id, script_name
                    )),
                    Err(e) => Ok(format!("Error adding owner: {}", e)),
                }
            },
        )?;
        script_storage.set("addScriptOwner", add_script_owner)?;

        // Secure removeScriptOwner function (admin or current owner only, prevents removing last owner)
        let user_ctx_remove_owner = user_context.clone();
        let remove_script_owner = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  script_name: String,
                  owner_id_to_remove: String|
                  -> JsResult<String> {
                // Check if user is admin or owns the script
                let is_admin = user_ctx_remove_owner
                    .has_capability(&crate::security::Capability::DeleteScripts);
                let user_owns = if let Some(user_id) = &user_ctx_remove_owner.user_id {
                    repository::user_owns_script(&script_name, user_id).unwrap_or(false)
                } else {
                    false
                };

                if !is_admin && !user_owns {
                    return Ok("Error: Permission denied. You must be an administrator or owner to remove owners".to_string());
                }

                // Non-admins cannot remove the last owner
                if !is_admin {
                    match repository::count_script_owners(&script_name) {
                        Ok(count) if count <= 1 => {
                            return Ok("Error: Cannot remove the last owner. Transfer ownership to another user first, or contact an administrator.".to_string());
                        }
                        Err(e) => {
                            return Ok(format!("Error checking owner count: {}", e));
                        }
                        _ => {}
                    }
                }

                // Remove the owner
                match repository::remove_script_owner(&script_name, &owner_id_to_remove) {
                    Ok(existed) => {
                        if existed {
                            Ok(format!(
                                "Successfully removed owner '{}' from script '{}'",
                                owner_id_to_remove, script_name
                            ))
                        } else {
                            Ok(format!(
                                "Owner '{}' was not found for script '{}'",
                                owner_id_to_remove, script_name
                            ))
                        }
                    }
                    Err(e) => Ok(format!("Error removing owner: {}", e)),
                }
            },
        )?;
        script_storage.set("removeScriptOwner", remove_script_owner)?;

        // Secure upsertScript function
        let user_ctx_upsert = user_context.clone();
        let _config_upsert = self.config.clone();
        let upsert_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  script_name: String,
                  js_script: String|
                  -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_upsert.require_capability(&crate::security::Capability::WriteScripts)
                {
                    return Ok(format!("Error: {}", e));
                }

                // Basic validation
                if script_name.is_empty() || js_script.is_empty() {
                    return Ok("Error: Script name and content cannot be empty".to_string());
                }

                // Check ownership permission for existing scripts
                if let Some(_existing_script) = repository::fetch_script(&script_name) {
                    // Script exists - check if user is admin or owns it
                    let is_admin =
                        user_ctx_upsert.has_capability(&crate::security::Capability::DeleteScripts);
                    let user_owns = if let Some(user_id) = &user_ctx_upsert.user_id {
                        repository::user_owns_script(&script_name, user_id).unwrap_or(false)
                    } else {
                        false
                    };

                    if !is_admin && !user_owns {
                        return Ok(format!(
                            "Error: Permission denied. You must be an administrator or owner to modify script '{}'",
                            script_name
                        ));
                    }
                }

                // Store the script using repository with owner
                let owner_user_id = user_ctx_upsert.user_id.as_deref();
                if let Err(e) =
                    repository::upsert_script_with_owner(&script_name, &js_script, owner_user_id)
                {
                    return Ok(format!("Error storing script: {}", e));
                }

                debug!(
                    script_name = %script_name,
                    user_id = ?user_ctx_upsert.user_id,
                    "Secure upsertScript called"
                );

                // Initialize the script asynchronously in the background
                // This calls the init() function if it exists
                let script_name_for_init = script_name.clone();
                tokio::task::spawn(async move {
                    // Clear any existing GraphQL and MCP registrations from this script before re-initializing
                    crate::graphql::clear_script_graphql_registrations(&script_name_for_init);
                    crate::mcp::clear_script_mcp_registrations(&script_name_for_init);

                    let initializer = crate::script_init::ScriptInitializer::new(5000); // 5s timeout
                    match initializer
                        .initialize_script(&script_name_for_init, false)
                        .await
                    {
                        Ok(result) => {
                            if result.success {
                                debug!(
                                    "Script '{}' initialized after upsert",
                                    script_name_for_init
                                );
                                // Rebuild GraphQL schema after script initialization
                                if let Err(e) = crate::graphql::rebuild_schema() {
                                    warn!(
                                        "Failed to rebuild GraphQL schema after script '{}' initialization: {:?}",
                                        script_name_for_init, e
                                    );
                                } else {
                                    debug!(
                                        "GraphQL schema rebuilt successfully after script '{}' initialization",
                                        script_name_for_init
                                    );
                                }
                            } else if let Some(err) = result.error {
                                warn!(
                                    "Script '{}' init failed after upsert: {}",
                                    script_name_for_init, err
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to initialize script '{}' after upsert: {}",
                                script_name_for_init, e
                            );
                        }
                    }
                });

                Ok(format!("Script '{}' upserted successfully", script_name))
            },
        )?;
        script_storage.set("upsertScript", upsert_script)?;

        // Secure deleteScript function
        let user_ctx_delete = user_context.clone();
        let auditor_delete = auditor.clone();
        let delete_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<bool> {
                // Check capability
                if let Err(e) =
                    user_ctx_delete.require_capability(&crate::security::Capability::DeleteScripts)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_delete.clone();
                    let user_id = user_ctx_delete.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "script".to_string(),
                                "delete".to_string(),
                                "DeleteScripts".to_string(),
                            )
                            .await;
                    });
                    warn!(
                        user_id = ?user_ctx_delete.user_id,
                        script_name = %script_name,
                        error = %e,
                        "deleteScript capability check failed"
                    );
                    return Ok(false);
                }

                // Check ownership permission - admins can delete any script, others only owned scripts
                let is_admin =
                    user_ctx_delete.has_capability(&crate::security::Capability::DeleteScripts);
                let user_owns = if let Some(user_id) = &user_ctx_delete.user_id {
                    repository::user_owns_script(&script_name, user_id).unwrap_or(false)
                } else {
                    false
                };

                if !is_admin && !user_owns {
                    warn!(
                        user_id = ?user_ctx_delete.user_id,
                        script_name = %script_name,
                        "deleteScript ownership check failed - user is not admin and does not own script"
                    );
                    return Ok(false);
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_delete.clone();
                let user_id = user_ctx_delete.user_id.clone();
                let script_name_clone = script_name.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::High,
                                user_id,
                            )
                            .with_resource("script".to_string())
                            .with_action("delete".to_string())
                            .with_detail("script_name", &script_name_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_delete.user_id,
                    script_name = %script_name,
                    "Secure deleteScript called"
                );

                Ok(repository::delete_script(&script_name))
            },
        )?;
        script_storage.set("deleteScript", delete_script)?;

        // Set the scriptStorage object on the global scope
        global.set("scriptStorage", script_storage)?;

        Ok(())
    }

    /// Setup secure asset management functions
    fn setup_asset_management_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let secure_ops = self.secure_ops.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let script_uri_remaining = script_uri_owned.clone(); // Clone for remaining functions

        // Create assetStorage object
        let asset_storage = rquickjs::Object::new(ctx.clone())?;

        // Secure listAssets function
        let user_ctx_list = user_context.clone();
        let script_uri_list = script_uri_owned.clone();
        let list_assets = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadAssets)
                {
                    // Return empty array JSON if no permission
                    return Ok("[]".to_string());
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listAssets called"
                );

                let assets = repository::fetch_assets(&script_uri_list);

                // Build JSON array of asset metadata (matching listScripts pattern)
                let assets_json: Vec<serde_json::Value> = assets
                    .values()
                    .map(|asset| {
                        serde_json::json!({
                            "uri": asset.uri,
                            "name": asset.name,
                            "size": asset.content.len(),
                            "mimetype": asset.mimetype,
                            "createdAt": asset.created_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                            "updatedAt": asset.updated_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                        })
                    })
                    .collect();

                match serde_json::to_string(&assets_json) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        error!("Failed to serialize assets to JSON: {}", e);
                        Ok("[]".to_string())
                    }
                }
            },
        )?;
        asset_storage.set("listAssets", list_assets)?;

        // Secure fetchAsset function
        let user_ctx_fetch = user_context.clone();
        let script_uri_fetch = script_uri_remaining.clone();
        let fetch_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_fetch.require_capability(&crate::security::Capability::ReadAssets)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_fetch.user_id,
                    uri = %uri,
                    "Secure fetchAsset called"
                );

                match repository::fetch_asset(&script_uri_fetch, &uri) {
                    Some(asset) => {
                        // Convert bytes to base64 for safe JavaScript transfer
                        Ok(base64::engine::general_purpose::STANDARD.encode(asset.content))
                    }
                    None => Ok(format!("Asset '{}' not found", uri)),
                }
            },
        )?;
        asset_storage.set("fetchAsset", fetch_asset)?;

        // Secure upsertAsset function
        let user_ctx_upsert_asset = user_context.clone();
        let _secure_ops_asset = secure_ops.clone();
        let auditor_asset = auditor.clone();
        let script_uri_asset = script_uri_owned.clone();
        let upsert_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  uri: String,
                  mimetype: String,
                  content_b64: String,
                  name: Opt<String>|
                  -> JsResult<String> {
                // Decode base64 content
                let content = match base64::engine::general_purpose::STANDARD.decode(&content_b64) {
                    Ok(c) => c,
                    Err(e) => return Ok(format!("Error decoding base64 content: {}", e)),
                };

                // Check capability
                if let Err(e) = user_ctx_upsert_asset
                    .require_capability(&crate::security::Capability::WriteAssets)
                {
                    return Ok(format!("Access denied: {}", e));
                }

                // Validate asset URI (inline validation since we can't call async)
                if uri.is_empty() || uri.len() > 255 {
                    return Ok("Invalid asset URI: must be 1-255 characters".to_string());
                }
                if uri.contains("..") || uri.contains('\\') {
                    return Ok("Invalid asset URI: path traversal not allowed".to_string());
                }

                // Validate content size (10MB limit)
                if content.len() > 10 * 1024 * 1024 {
                    return Ok("Asset too large (max 10MB)".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_asset.clone();
                let user_id = user_ctx_upsert_asset.user_id.clone();
                let uri_clone = uri.clone();
                let script_uri_clone = script_uri_asset.clone();
                let content_len = content.len();
                let mimetype_clone = mimetype.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("asset".to_string())
                            .with_action("upsert".to_string())
                            .with_detail("uri", &uri_clone)
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("content_size", content_len.to_string())
                            .with_detail("mimetype", &mimetype_clone),
                        )
                        .await;
                });

                // Call repository directly (sync operation)
                let now = std::time::SystemTime::now();
                let asset = repository::Asset {
                    uri: uri.clone(),
                    name: name.0.or_else(|| Some(uri.clone())),
                    mimetype,
                    content,
                    created_at: now,
                    updated_at: now,
                    script_uri: script_uri_owned.clone(),
                };
                match repository::upsert_asset(asset) {
                    Ok(_) => Ok(format!("Asset '{}' upserted successfully", uri)),
                    Err(e) => Ok(format!("Error upserting asset: {}", e)),
                }
            },
        )?;
        asset_storage.set("upsertAsset", upsert_asset)?;

        // Secure deleteAsset function
        let user_ctx_delete_asset = user_context.clone();
        let auditor_delete_asset = auditor.clone();
        let script_uri_delete_asset = script_uri_remaining.clone();
        let delete_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_delete_asset
                    .require_capability(&crate::security::Capability::DeleteAssets)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_delete_asset.clone();
                    let user_id = user_ctx_delete_asset.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "asset".to_string(),
                                "delete".to_string(),
                                "DeleteAssets".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_delete_asset.clone();
                let user_id = user_ctx_delete_asset.user_id.clone();
                let uri_clone = uri.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::High,
                                user_id,
                            )
                            .with_resource("asset".to_string())
                            .with_action("delete".to_string())
                            .with_detail("uri", &uri_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_delete_asset.user_id,
                    uri = %uri,
                    "Secure deleteAsset called"
                );

                match repository::delete_asset(&script_uri_delete_asset, &uri) {
                    true => Ok(format!("Asset '{}' deleted successfully", uri)),
                    false => Ok(format!("Asset '{}' not found", uri)),
                }
            },
        )?;
        asset_storage.set("deleteAsset", delete_asset)?;

        // ====================================================================
        // Privileged URI-specific asset methods (for cross-script management)
        // ====================================================================

        // Secure listAssetsForUri function (privileged scripts only)
        let user_ctx_list_uri = user_context.clone();
        let list_assets_for_uri = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list_uri.require_capability(&crate::security::Capability::ReadAssets)
                {
                    // Return empty array JSON if no permission
                    return Ok("[]".to_string());
                }

                debug!(
                    user_id = ?user_ctx_list_uri.user_id,
                    uri = %uri,
                    "Secure listAssetsForUri called"
                );

                let assets = repository::fetch_assets(&uri);

                // Build JSON array of asset metadata (matching listAssets pattern)
                let assets_json: Vec<serde_json::Value> = assets
                    .values()
                    .map(|asset| {
                        serde_json::json!({
                            "uri": asset.uri,
                            "name": asset.name,
                            "size": asset.content.len(),
                            "mimetype": asset.mimetype,
                            "createdAt": asset.created_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                            "updatedAt": asset.updated_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                        })
                    })
                    .collect();

                match serde_json::to_string(&assets_json) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        error!("Failed to serialize assets to JSON: {}", e);
                        Ok("[]".to_string())
                    }
                }
            },
        )?;
        asset_storage.set("listAssetsForUri", list_assets_for_uri)?;

        // Secure fetchAssetForUri function (privileged scripts only)
        let user_ctx_fetch_uri = user_context.clone();
        let fetch_asset_for_uri = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String, asset_name: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_fetch_uri.require_capability(&crate::security::Capability::ReadAssets)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_fetch_uri.user_id,
                    uri = %uri,
                    asset_name = %asset_name,
                    "Secure fetchAssetForUri called"
                );

                match repository::fetch_asset(&uri, &asset_name) {
                    Some(asset) => {
                        // Convert bytes to base64 for safe JavaScript transfer
                        Ok(base64::engine::general_purpose::STANDARD.encode(asset.content))
                    }
                    None => Ok(format!("Asset '{}' not found", asset_name)),
                }
            },
        )?;
        asset_storage.set("fetchAssetForUri", fetch_asset_for_uri)?;

        // Secure upsertAssetForUri function (privileged scripts only)
        let user_ctx_upsert_uri = user_context.clone();
        let auditor_upsert_uri = auditor.clone();
        let upsert_asset_for_uri = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  uri: String,
                  asset_name: String,
                  mimetype: String,
                  content_b64: String|
                  -> JsResult<String> {
                // Decode base64 content
                let content = match base64::engine::general_purpose::STANDARD.decode(&content_b64) {
                    Ok(c) => c,
                    Err(e) => return Ok(format!("Error decoding base64 content: {}", e)),
                };

                // Check capability
                if let Err(e) = user_ctx_upsert_uri
                    .require_capability(&crate::security::Capability::WriteAssets)
                {
                    return Ok(format!("Access denied: {}", e));
                }

                // Validate asset URI (inline validation since we can't call async)
                if asset_name.is_empty() || asset_name.len() > 255 {
                    return Ok("Invalid asset URI: must be 1-255 characters".to_string());
                }
                if asset_name.contains("..") || asset_name.contains('\\') {
                    return Ok("Invalid asset URI: path traversal not allowed".to_string());
                }

                // Validate content size (10MB limit)
                if content.len() > 10 * 1024 * 1024 {
                    return Ok("Asset too large (max 10MB)".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_upsert_uri.clone();
                let user_id = user_ctx_upsert_uri.user_id.clone();
                let uri_clone = uri.clone();
                let asset_name_clone = asset_name.clone();
                let content_len = content.len();
                let mimetype_clone = mimetype.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("asset".to_string())
                            .with_action("upsert_for_uri".to_string())
                            .with_detail("uri", &asset_name_clone)
                            .with_detail("script_uri", &uri_clone)
                            .with_detail("content_size", content_len.to_string())
                            .with_detail("mimetype", &mimetype_clone),
                        )
                        .await;
                });

                // Call repository directly (sync operation)
                let now = std::time::SystemTime::now();
                let asset = repository::Asset {
                    uri: asset_name.clone(),
                    name: Some(asset_name.clone()),
                    mimetype,
                    content,
                    created_at: now,
                    updated_at: now,
                    script_uri: uri.clone(),
                };
                match repository::upsert_asset(asset) {
                    Ok(_) => Ok(format!("Asset '{}' upserted successfully", asset_name)),
                    Err(e) => Ok(format!("Error upserting asset: {}", e)),
                }
            },
        )?;
        asset_storage.set("upsertAssetForUri", upsert_asset_for_uri)?;

        // Secure deleteAssetForUri function (privileged scripts only)
        let user_ctx_delete_uri = user_context.clone();
        let auditor_delete_uri = auditor.clone();
        let delete_asset_for_uri = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String, asset_name: String| -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_delete_uri
                    .require_capability(&crate::security::Capability::DeleteAssets)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_delete_uri.clone();
                    let user_id = user_ctx_delete_uri.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "asset".to_string(),
                                "delete_for_uri".to_string(),
                                "DeleteAssets".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_delete_uri.clone();
                let user_id = user_ctx_delete_uri.user_id.clone();
                let uri_clone = uri.clone();
                let asset_name_clone = asset_name.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::High,
                                user_id,
                            )
                            .with_resource("asset".to_string())
                            .with_action("delete_for_uri".to_string())
                            .with_detail("uri", &asset_name_clone)
                            .with_detail("script_uri", &uri_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_delete_uri.user_id,
                    uri = %uri,
                    asset_name = %asset_name,
                    "Secure deleteAssetForUri called"
                );

                match repository::delete_asset(&uri, &asset_name) {
                    true => Ok(format!("Asset '{}' deleted successfully", asset_name)),
                    false => Ok(format!("Asset '{}' not found", asset_name)),
                }
            },
        )?;
        asset_storage.set("deleteAssetForUri", delete_asset_for_uri)?;

        // Set the assetStorage object on the global scope
        global.set("assetStorage", asset_storage)?;
        Ok(())
    }

    /// Setup secure secrets functions
    ///
    /// Exposes a read-only JavaScript API for secrets management:
    /// - secretStorage.exists(identifier): boolean - Check if a secret exists
    /// - secretStorage.list(): string[] - List all secret identifiers
    ///
    /// SECURITY: Secret values are NEVER exposed to JavaScript. Only existence checks
    /// and identifier listing are allowed. Actual secret values are injected by Rust
    /// into HTTP requests using the {{secret:identifier}} template syntax.
    fn setup_secrets_functions(&self, ctx: &rquickjs::Ctx<'_>, _script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();

        // Get the secrets manager - try instance first, then fall back to global
        let secrets_manager = self
            .secrets_manager
            .clone()
            .or_else(crate::secrets::get_global_secrets_manager);

        // Create the secretStorage namespace object
        let secret_storage_obj = rquickjs::Object::new(ctx.clone())?;

        // secretStorage.exists(identifier) - Check if a secret exists
        let secrets_mgr_exists = secrets_manager.clone();
        let exists_fn = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, identifier: String| -> JsResult<bool> {
                if let Some(ref mgr) = secrets_mgr_exists {
                    Ok(mgr.exists(&identifier))
                } else {
                    // No secrets manager available
                    Ok(false)
                }
            },
        )?;
        secret_storage_obj.set("exists", exists_fn)?;

        // secretStorage.list() - List all secret identifiers
        let secrets_mgr_list = secrets_manager.clone();
        let list_fn = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Vec<String>> {
                if let Some(ref mgr) = secrets_mgr_list {
                    Ok(mgr.list_identifiers())
                } else {
                    // No secrets manager available
                    Ok(Vec::new())
                }
            },
        )?;
        secret_storage_obj.set("list", list_fn)?;

        // Set the secretStorage object on the global scope
        global.set("secretStorage", secret_storage_obj)?;

        debug!("secretStorage JavaScript API initialized (read-only interface)");

        Ok(())
    }

    /// Setup secure GraphQL functions  
    fn setup_graphql_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let secure_ops = self.secure_ops.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();

        // Secure registerGraphQLQuery function
        let user_ctx_query = user_context.clone();
        let _secure_ops_query = secure_ops.clone();
        let auditor_query = auditor.clone();
        let script_uri_query = script_uri_owned.clone();
        let config_query = self.config.clone();
        let register_graphql_query = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  sdl: String,
                  resolver_function: String,
                  visibility: String|
                  -> JsResult<String> {
                // If GraphQL registration is disabled, return success without doing anything
                tracing::info!(
                    "registerGraphQLQuery called: name={}, visibility={}, enable_graphql_registration={}",
                    name,
                    visibility,
                    config_query.enable_graphql_registration
                );
                if !config_query.enable_graphql_registration {
                    tracing::info!(
                        "GraphQL registration disabled, skipping query registration for: {}",
                        name
                    );
                    return Ok(format!(
                        "GraphQL query '{}' registration skipped (disabled)",
                        name
                    ));
                }

                // Check capability
                if let Err(e) =
                    user_ctx_query.require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_query.clone();
                    let user_id = user_ctx_query.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "register_query".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate GraphQL schema inline (sync validation)
                // Basic SDL validation
                if sdl.is_empty() || sdl.len() > 100_000 {
                    return Ok("Invalid SDL: must be between 1 and 100,000 characters".to_string());
                }
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid query name: must be between 1 and 100 characters".to_string()
                    );
                }
                // Check for dangerous patterns
                if sdl.contains("__proto__") || sdl.contains("constructor") {
                    return Ok("Invalid SDL: contains dangerous patterns".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_query.clone();
                let user_id = user_ctx_query.user_id.clone();
                let name_clone = name.clone();
                let script_uri_clone = script_uri_query.clone();
                let sdl_len = sdl.len();
                let visibility_clone = visibility.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("register_query".to_string())
                            .with_detail("query_name", &name_clone)
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("sdl_length", sdl_len.to_string())
                            .with_detail("visibility", &visibility_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_query.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    visibility = %visibility,
                    "Secure registerGraphQLQuery called"
                );

                // Actually register the GraphQL query
                match crate::graphql::register_graphql_query(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_query.clone(),
                    visibility,
                ) {
                    Ok(()) => Ok(format!("GraphQL query '{}' registered successfully", name)),
                    Err(e) => Ok(format!("Error registering GraphQL query '{}': {}", name, e)),
                }
            },
        )?;

        // Secure registerGraphQLMutation function
        let user_ctx_mutation = user_context.clone();
        let _secure_ops_mutation = secure_ops.clone();
        let auditor_mutation = auditor.clone();
        let script_uri_mutation = script_uri_owned.clone();
        let config_mutation = self.config.clone();
        let register_graphql_mutation = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  sdl: String,
                  resolver_function: String,
                  visibility: String|
                  -> JsResult<String> {
                // If GraphQL registration is disabled, return success without doing anything
                debug!(
                    "registerGraphQLMutation called: name={}, visibility={}, enable_graphql_registration={}",
                    name, visibility, config_mutation.enable_graphql_registration
                );
                if !config_mutation.enable_graphql_registration {
                    debug!(
                        "GraphQL registration disabled, skipping mutation registration for: {}",
                        name
                    );
                    return Ok(format!(
                        "GraphQL mutation '{}' registration skipped (disabled)",
                        name
                    ));
                }

                // Check capability
                if let Err(e) = user_ctx_mutation
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_mutation.clone();
                    let user_id = user_ctx_mutation.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "register_mutation".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate GraphQL schema inline (sync validation)
                if sdl.is_empty() || sdl.len() > 100_000 {
                    return Ok("Invalid SDL: must be between 1 and 100,000 characters".to_string());
                }
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid mutation name: must be between 1 and 100 characters".to_string(),
                    );
                }
                if sdl.contains("__proto__") || sdl.contains("constructor") {
                    return Ok("Invalid SDL: contains dangerous patterns".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_mutation.clone();
                let user_id = user_ctx_mutation.user_id.clone();
                let name_clone = name.clone();
                let sdl_len = sdl.len();
                let visibility_clone = visibility.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("register_mutation".to_string())
                            .with_detail("mutation_name", &name_clone)
                            .with_detail("sdl_length", sdl_len.to_string())
                            .with_detail("visibility", &visibility_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_mutation.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    visibility = %visibility,
                    "Secure registerGraphQLMutation called"
                );

                // Actually register the GraphQL mutation
                match crate::graphql::register_graphql_mutation(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_mutation.clone(),
                    visibility,
                ) {
                    Ok(()) => Ok(format!(
                        "GraphQL mutation '{}' registered successfully",
                        name
                    )),
                    Err(e) => Ok(format!(
                        "Error registering GraphQL mutation '{}': {}",
                        name, e
                    )),
                }
            },
        )?;

        // Secure registerGraphQLSubscription function
        let user_ctx_subscription = user_context.clone();
        let _secure_ops_subscription = secure_ops.clone();
        let auditor_subscription = auditor.clone();
        let script_uri_subscription = script_uri_owned.clone();
        let config_subscription = self.config.clone();
        let register_graphql_subscription = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  sdl: String,
                  resolver_function: String,
                  visibility: String|
                  -> JsResult<String> {
                // If GraphQL registration is disabled, return success without doing anything
                debug!(
                    "registerGraphQLSubscription called: name={}, visibility={}, enable_graphql_registration={}",
                    name, visibility, config_subscription.enable_graphql_registration
                );
                if !config_subscription.enable_graphql_registration {
                    debug!(
                        "GraphQL registration disabled, skipping subscription registration for: {}",
                        name
                    );
                    return Ok(format!(
                        "GraphQL subscription '{}' registration skipped (disabled)",
                        name
                    ));
                }

                // Check capability
                if let Err(e) = user_ctx_subscription
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_subscription.clone();
                    let user_id = user_ctx_subscription.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "register_subscription".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate GraphQL schema inline (sync validation)
                if sdl.is_empty() || sdl.len() > 100_000 {
                    return Ok("Invalid SDL: must be between 1 and 100,000 characters".to_string());
                }
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid subscription name: must be between 1 and 100 characters"
                            .to_string(),
                    );
                }
                if sdl.contains("__proto__") || sdl.contains("constructor") {
                    return Ok("Invalid SDL: contains dangerous patterns".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_subscription.clone();
                let user_id = user_ctx_subscription.user_id.clone();
                let name_clone = name.clone();
                let sdl_len = sdl.len();
                let visibility_clone = visibility.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("register_subscription".to_string())
                            .with_detail("subscription_name", &name_clone)
                            .with_detail("sdl_length", sdl_len.to_string())
                            .with_detail("visibility", &visibility_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_subscription.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    visibility = %visibility,
                    "Secure registerGraphQLSubscription called"
                );

                // Actually register the GraphQL subscription
                match crate::graphql::register_graphql_subscription(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_subscription.clone(),
                    visibility,
                ) {
                    Ok(()) => Ok(format!(
                        "GraphQL subscription '{}' registered successfully",
                        name
                    )),
                    Err(e) => Ok(format!(
                        "Error registering GraphQL subscription '{}': {}",
                        name, e
                    )),
                }
            },
        )?;

        // Secure executeGraphQL function
        let user_ctx_execute = user_context.clone();
        let auditor_execute = auditor.clone();
        let script_uri_execute = script_uri_owned.clone();
        let config_execute = self.config.clone();
        let execute_graphql = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  query: String,
                  variables_json: Option<String>|
                  -> JsResult<String> {
                // If GraphQL execution is disabled, return error
                debug!(
                    "executeGraphQL called: query_length={}, enable_graphql_execution={}",
                    query.len(),
                    config_execute.enable_graphql_registration // Reuse existing config for now
                );
                if !config_execute.enable_graphql_registration {
                    debug!("GraphQL execution disabled, rejecting executeGraphQL call");
                    return Ok(
                        "{\"errors\": [{\"message\": \"GraphQL execution is disabled\"}]}"
                            .to_string(),
                    );
                }

                // Check capability
                if let Err(e) =
                    user_ctx_execute.require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_execute.clone();
                    let user_id = user_ctx_execute.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "execute".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("{{\"errors\": [{{\"message\": \"{}\"}}]}}", e));
                }

                // Validate query
                if query.is_empty() || query.len() > 100_000 {
                    return Ok("{\"errors\": [{\"message\": \"Invalid query: must be between 1 and 100,000 characters\"}]}".to_string());
                }

                // Parse variables if provided
                let variables = if let Some(vars_json) = variables_json {
                    if vars_json.len() > 50_000 {
                        return Ok("{\"errors\": [{\"message\": \"Variables too large: max 50,000 characters\"}]}".to_string());
                    }
                    match serde_json::from_str::<serde_json::Value>(&vars_json) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            return Ok(format!(
                                "{{\"errors\": [{{\"message\": \"Invalid variables JSON: {}\"}}]}}",
                                e
                            ));
                        }
                    }
                } else {
                    None
                };

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_execute.clone();
                let user_id = user_ctx_execute.user_id.clone();
                let query_clone = query.clone();
                let script_uri_clone = script_uri_execute.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("execute".to_string())
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("query_length", query_clone.len().to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_execute.user_id,
                    query_len = query.len(),
                    has_variables = variables.is_some(),
                    "Secure executeGraphQL called"
                );

                // Execute the GraphQL query
                match crate::graphql::execute_graphql_query_sync(&query, variables) {
                    Ok(result_json) => {
                        debug!("GraphQL execution successful");
                        Ok(result_json)
                    }
                    Err(e) => {
                        tracing::error!("GraphQL execution failed: {}", e);
                        Ok(format!(
                            "{{\"errors\": [{{\"message\": \"GraphQL execution failed: {}\"}}]}}",
                            e
                        ))
                    }
                }
            },
        )?;

        // Secure sendSubscriptionMessage function
        let user_ctx_send_sub = user_context.clone();
        let auditor_send_sub = auditor.clone();
        let send_subscription_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  subscription_name: String,
                  message: String|
                  -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_send_sub
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_send_sub.clone();
                    let user_id = user_ctx_send_sub.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "send_subscription_message".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_send_sub.clone();
                let user_id = user_ctx_send_sub.user_id.clone();
                let subscription_name_clone = subscription_name.clone();
                let message_clone = message.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("send_subscription_message".to_string())
                            .with_detail("subscription_name", &subscription_name_clone)
                            .with_detail("message_length", message_clone.len().to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_send_sub.user_id,
                    subscription_name = %subscription_name,
                    message_len = message.len(),
                    "Secure sendSubscriptionMessage called"
                );

                // Send to the auto-registered stream path for this subscription
                let stream_path = format!("/engine/graphql/subscription/{}", subscription_name);

                // Call actual stream message sending (sync operation)
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream(&stream_path, &message)
                {
                    Ok(result) => {
                        if result.is_fully_successful() {
                            Ok(format!(
                                "GraphQL subscription message sent to '{}' ({} connections) successfully",
                                subscription_name, result.successful_sends
                            ))
                        } else {
                            Ok(format!(
                                "GraphQL subscription message to '{}' partially sent: {} successful, {} failed out of {} total",
                                subscription_name,
                                result.successful_sends,
                                result.failed_connections.len(),
                                result.total_connections
                            ))
                        }
                    }
                    Err(e) => Ok(format!(
                        "Failed to send GraphQL subscription message to '{}': {}",
                        subscription_name, e
                    )),
                }
            },
        )?;

        // Secure sendSubscriptionMessageFiltered function (selective broadcasting for GraphQL)
        let user_ctx_send_sub_filtered = user_context.clone();
        let auditor_send_sub_filtered = auditor.clone();
        let send_subscription_message_filtered = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  subscription_name: String,
                  message: String,
                  filter_json: Option<String>|
                  -> JsResult<String> {
                // Parse filter criteria from JSON string
                let metadata_filter: HashMap<String, String> = if let Some(json_str) = filter_json {
                    serde_json::from_str(&json_str).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "filter",
                            "MetadataFilter",
                            &format!("Invalid filter JSON: {}", e),
                        )
                    })?
                } else {
                    HashMap::new() // Empty filter matches all connections
                };

                // Allow system-level GraphQL subscription broadcasting without capability checks
                let is_system_broadcast = true; // GraphQL subscriptions are considered system-level

                if !is_system_broadcast {
                    // Check capability for non-system operations (future use)
                    if let Err(e) = user_ctx_send_sub_filtered
                        .require_capability(&crate::security::Capability::ManageGraphQL)
                    {
                        // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                        let auditor_clone = auditor_send_sub_filtered.clone();
                        let user_id = user_ctx_send_sub_filtered.user_id.clone();
                        tokio::task::spawn(async move {
                            let _ = auditor_clone
                                .log_authz_failure(
                                    user_id,
                                    "graphql".to_string(),
                                    "send_subscription_message_to_connections".to_string(),
                                    "ManageGraphQL".to_string(),
                                )
                                .await;
                        });
                        return Ok(format!("Error: {}", e));
                    }
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_send_sub_filtered.clone();
                let user_id = user_ctx_send_sub_filtered.user_id.clone();
                let subscription_name_clone = subscription_name.clone();
                let message_clone = message.clone();
                let filter_clone = metadata_filter.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("send_subscription_message_to_connections".to_string())
                            .with_detail("subscription_name", &subscription_name_clone)
                            .with_detail("message_length", message_clone.len().to_string())
                            .with_detail("filter_criteria", format!("{:?}", filter_clone)),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_send_sub_filtered.user_id,
                    subscription_name = %subscription_name,
                    message_len = message.len(),
                    filter = ?metadata_filter,
                    "Secure sendSubscriptionMessageFiltered called"
                );

                // Send to the auto-registered stream path for this subscription with filtering
                let stream_path = format!("/engine/graphql/subscription/{}", subscription_name);

                // Call selective broadcasting (sync operation)
                let result = crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream_with_filter(&stream_path, &message, &metadata_filter);

                match result {
                    Ok(broadcast_result) => {
                        if broadcast_result.is_fully_successful() {
                            Ok(format!(
                                "GraphQL subscription message sent to '{}' with filter {:?} ({} connections) successfully",
                                subscription_name,
                                metadata_filter,
                                broadcast_result.successful_sends
                            ))
                        } else {
                            Ok(format!(
                                "GraphQL subscription message to '{}' with filter {:?} partially sent: {} successful, {} failed connections",
                                subscription_name,
                                metadata_filter,
                                broadcast_result.successful_sends,
                                broadcast_result.failed_connections.len()
                            ))
                        }
                    }
                    Err(e) => Ok(format!(
                        "Failed to send GraphQL subscription message to '{}' with filter: {}",
                        subscription_name, e
                    )),
                }
            },
        )?;

        // Create graphQLRegistry object with all 6 functions
        let graphql_registry = rquickjs::Object::new(ctx.clone())?;
        graphql_registry.set("registerQuery", register_graphql_query)?;
        graphql_registry.set("registerMutation", register_graphql_mutation)?;
        graphql_registry.set("registerSubscription", register_graphql_subscription)?;
        graphql_registry.set("executeGraphQL", execute_graphql)?;
        graphql_registry.set("sendSubscriptionMessage", send_subscription_message)?;
        graphql_registry.set(
            "sendSubscriptionMessageFiltered",
            send_subscription_message_filtered,
        )?;
        global.set("graphQLRegistry", graphql_registry)?;

        Ok(())
    }

    /// Setup MCP (Model Context Protocol) registry functions
    fn setup_mcp_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let config = self.config.clone();

        // registerTool function - registers an MCP tool
        let user_ctx_register = user_context.clone();
        let auditor_register = auditor.clone();
        let script_uri_register = script_uri_owned.clone();
        let register_tool = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  description: String,
                  input_schema_json: String,
                  handler_function: String|
                  -> JsResult<String> {
                // Check if MCP is enabled (we can use the same config flag as GraphQL for now)
                if !config.enable_graphql_registration {
                    debug!(
                        "MCP tool registration disabled, skipping tool registration for: {}",
                        name
                    );
                    return Ok(format!(
                        "MCP tool '{}' registration skipped (disabled)",
                        name
                    ));
                }

                // Check capability - reuse ManageGraphQL for MCP tools
                if let Err(e) = user_ctx_register
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    let auditor_clone = auditor_register.clone();
                    let user_id = user_ctx_register.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "mcp".to_string(),
                                "register_tool".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate inputs
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid tool name: must be between 1 and 100 characters".to_string()
                    );
                }
                if description.is_empty() || description.len() > 1000 {
                    return Ok(
                        "Invalid description: must be between 1 and 1000 characters".to_string()
                    );
                }

                // Parse and validate input schema JSON
                let input_schema: serde_json::Value = serde_json::from_str(&input_schema_json)
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "schema",
                            "InputSchema",
                            &format!("Invalid input schema JSON: {}", e),
                        )
                    })?;

                // Check for dangerous patterns
                if input_schema_json.contains("__proto__")
                    || input_schema_json.contains("constructor")
                {
                    return Ok("Invalid schema: contains dangerous patterns".to_string());
                }

                // Log the operation attempt
                let auditor_clone = auditor_register.clone();
                let user_id = user_ctx_register.user_id.clone();
                let name_clone = name.clone();
                let script_uri_clone = script_uri_register.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("mcp".to_string())
                            .with_action("register_tool".to_string())
                            .with_detail("tool_name", &name_clone)
                            .with_detail("script_uri", &script_uri_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_register.user_id,
                    name = %name,
                    "Secure registerTool called for MCP"
                );

                // Actually register the MCP tool
                crate::mcp::register_mcp_tool(
                    name.clone(),
                    description,
                    input_schema,
                    handler_function,
                    script_uri_register.clone(),
                );

                Ok(format!("MCP tool '{}' registered successfully", name))
            },
        )?;

        // registerPrompt function - registers an MCP prompt
        let user_ctx_prompt = user_context.clone();
        let auditor_prompt = auditor.clone();
        let script_uri_prompt = script_uri_owned.clone();
        let config_prompt = config.clone();
        let register_prompt = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  description: String,
                  arguments_json: String,
                  handler_function: String|
                  -> JsResult<String> {
                // Check if MCP is enabled
                if !config_prompt.enable_graphql_registration {
                    debug!(
                        "MCP prompt registration disabled, skipping prompt registration for: {}",
                        name
                    );
                    return Ok(format!(
                        "MCP prompt '{}' registration skipped (disabled)",
                        name
                    ));
                }

                // Check capability - reuse ManageGraphQL for MCP prompts
                if let Err(e) =
                    user_ctx_prompt.require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    let auditor_clone = auditor_prompt.clone();
                    let user_id = user_ctx_prompt.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "mcp".to_string(),
                                "register_prompt".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate inputs
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid prompt name: must be between 1 and 100 characters".to_string()
                    );
                }
                if description.is_empty() || description.len() > 1000 {
                    return Ok(
                        "Invalid description: must be between 1 and 1000 characters".to_string()
                    );
                }
                if handler_function.is_empty() || handler_function.len() > 100 {
                    return Ok(
                        "Invalid handler function: must be between 1 and 100 characters"
                            .to_string(),
                    );
                }

                // Validate arguments JSON
                if arguments_json.contains("__proto__") || arguments_json.contains("constructor") {
                    return Ok("Invalid arguments: contains dangerous patterns".to_string());
                }

                // Log the operation attempt
                let auditor_clone = auditor_prompt.clone();
                let user_id = user_ctx_prompt.user_id.clone();
                let name_clone = name.clone();
                let script_uri_clone = script_uri_prompt.clone();
                let handler_clone = handler_function.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("mcp".to_string())
                            .with_action("register_prompt".to_string())
                            .with_detail("prompt_name", &name_clone)
                            .with_detail("handler", &handler_clone)
                            .with_detail("script_uri", &script_uri_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_prompt.user_id,
                    name = %name,
                    handler = %handler_function,
                    "Secure registerPrompt called for MCP"
                );

                // Actually register the MCP prompt
                match crate::mcp::register_mcp_prompt(
                    name.clone(),
                    description,
                    arguments_json,
                    handler_function.clone(),
                    script_uri_prompt.clone(),
                ) {
                    Ok(_) => Ok(format!(
                        "MCP prompt '{}' registered successfully with handler '{}'",
                        name, handler_function
                    )),
                    Err(e) => Ok(format!("Error registering prompt: {}", e)),
                }
            },
        )?;

        // Create mcpRegistry object
        let mcp_registry = rquickjs::Object::new(ctx.clone())?;
        mcp_registry.set("registerTool", register_tool)?;
        mcp_registry.set("registerPrompt", register_prompt)?;
        global.set("mcpRegistry", mcp_registry)?;

        Ok(())
    }

    /// Setup routeRegistry object with all route-related functions
    fn setup_route_registry(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<RouteRegisterFn>,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let config = self.config.clone();

        // Create the routeRegistry object
        let route_registry = rquickjs::Object::new(ctx.clone())?;

        // 1. registerRoute function
        if let Some(register_impl) = register_fn {
            let script_uri_for_register = script_uri_owned.clone();
            let user_ctx_route = user_context.clone();
            let register_route = Function::new(
                ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>,
                      path: String,
                      handler: String,
                      method: Option<String>,
                      metadata: Opt<rquickjs::Object>|
                      -> Result<(), rquickjs::Error> {
                    // Check if script is privileged OR user has admin privileges
                    let script_privileged =
                        match repository::is_script_privileged(&script_uri_for_register) {
                            Ok(true) => true,
                            Ok(false) => false,
                            Err(e) => {
                                return Err(rquickjs::Error::new_from_js_message(
                                    "routeRegistry.registerRoute",
                                    "privilege_lookup_failed",
                                    &format!(
                                        "Unable to verify privileges for '{}': {}",
                                        script_uri_for_register, e
                                    ),
                                ));
                            }
                        };

                    let user_is_admin =
                        user_ctx_route.has_capability(&crate::security::Capability::DeleteScripts);

                    if !script_privileged && !user_is_admin {
                        return Err(rquickjs::Error::new_from_js_message(
                            "routeRegistry.registerRoute",
                            "permission_denied",
                            &format!(
                                "Script '{}' is not privileged to register HTTP routes",
                                script_uri_for_register
                            ),
                        ));
                    }

                    // Build RouteMetadata from parameters
                    let mut route_meta = repository::RouteMetadata::simple(handler.clone());

                    if let Some(meta_obj) = metadata.0 {
                        // Extract summary
                        if let Ok(summary) = meta_obj.get::<_, Option<String>>("summary") {
                            route_meta.summary = summary;
                        }
                        // Extract description
                        if let Ok(description) = meta_obj.get::<_, Option<String>>("description") {
                            route_meta.description = description;
                        }
                        // Extract tags
                        if let Ok(tags_arr) = meta_obj.get::<_, rquickjs::Array>("tags") {
                            let mut tags = Vec::new();
                            for i in 0..tags_arr.len() {
                                if let Ok(tag) = tags_arr.get::<String>(i) {
                                    tags.push(tag);
                                }
                            }
                            route_meta.tags = tags;
                        }
                    }

                    let method_ref = method.as_deref();
                    register_impl(&path, &route_meta, method_ref)
                },
            )?;
            route_registry.set("registerRoute", register_route)?;
        } else {
            // No-op register function
            let reg_noop = Function::new(
                ctx.clone(),
                |_c: rquickjs::Ctx<'_>,
                 _p: String,
                 _h: String,
                 _m: Option<String>,
                 _meta: Opt<rquickjs::Object>|
                 -> Result<(), rquickjs::Error> { Ok(()) },
            )?;
            route_registry.set("registerRoute", reg_noop)?;
        }

        // 2. registerStreamRoute function
        let user_ctx_stream = user_context.clone();
        let auditor_stream = auditor.clone();
        let config_stream = config.clone();
        let script_uri_stream = script_uri_owned.clone();
        let register_stream_route = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  path: String,
                  customization_function: Opt<String>|
                  -> JsResult<String> {
                // Convert Opt to Option
                let customization_function = customization_function.0;
                // If streams are disabled, return success without doing anything
                if !config_stream.enable_streams {
                    return Ok(format!(
                        "Stream registration disabled (stream '{}' not registered)",
                        path
                    ));
                }

                // Check if script is privileged OR user has admin privileges
                let script_privileged = match repository::is_script_privileged(&script_uri_stream) {
                    Ok(true) => true,
                    Ok(false) => false,
                    Err(e) => {
                        return Ok(format!(
                            "Stream route '{}' denied: privilege lookup failed ({})",
                            path, e
                        ));
                    }
                };

                let user_is_admin =
                    user_ctx_stream.has_capability(&crate::security::Capability::DeleteScripts);

                if !script_privileged && !user_is_admin {
                    return Ok(format!(
                        "Stream route '{}' denied: script '{}' is not privileged",
                        path, script_uri_stream
                    ));
                }

                // Validate path format
                if path.is_empty() || !path.starts_with('/') {
                    return Ok(format!(
                        "Invalid stream path '{}': path must start with '/' and not be empty",
                        path
                    ));
                }

                if path.len() > 200 {
                    return Ok(format!(
                        "Invalid stream path '{}': path too long (max 200 characters)",
                        path
                    ));
                }

                // Validate customization function name if provided
                if let Some(ref func_name) = customization_function {
                    if func_name.is_empty() {
                        return Ok(
                            "Invalid customization function: name cannot be empty".to_string()
                        );
                    }
                    if func_name.len() > 100 {
                        return Ok(
                            "Invalid customization function: name too long (max 100 characters)"
                                .to_string(),
                        );
                    }
                    // Basic validation: should be a valid identifier
                    if !func_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        return Ok("Invalid customization function: name must contain only alphanumeric characters and underscores".to_string());
                    }
                }

                // Check capability
                if let Err(e) =
                    user_ctx_stream.require_capability(&crate::security::Capability::ManageStreams)
                {
                    if config_stream.enable_audit_logging
                        && let Ok(rt) = tokio::runtime::Handle::try_current()
                    {
                        let auditor_clone = auditor_stream.clone();
                        let user_id = user_ctx_stream.user_id.clone();
                        rt.spawn(async move {
                            let _ = auditor_clone
                                .log_event(
                                    crate::security::SecurityEvent::new(
                                        crate::security::SecurityEventType::AuthorizationFailure,
                                        crate::security::SecuritySeverity::Medium,
                                        user_id,
                                    )
                                    .with_resource("stream".to_string())
                                    .with_action("register".to_string()),
                                )
                                .await;
                        });
                    }
                    return Ok(format!("Error: {}", e));
                }

                // Validate stream path
                if path.contains("..") || path.contains('\\') {
                    return Ok("Invalid stream path: path traversal not allowed".to_string());
                }

                // Log the operation attempt
                if config_stream.enable_audit_logging
                    && let Ok(rt) = tokio::runtime::Handle::try_current()
                {
                    let auditor_clone = auditor_stream.clone();
                    let user_id = user_ctx_stream.user_id.clone();
                    let path_clone = path.clone();
                    let script_uri_clone = script_uri_stream.clone();
                    rt.spawn(async move {
                        let _ = auditor_clone
                            .log_event(
                                crate::security::SecurityEvent::new(
                                    crate::security::SecurityEventType::SystemSecurityEvent,
                                    crate::security::SecuritySeverity::Medium,
                                    user_id,
                                )
                                .with_resource("stream".to_string())
                                .with_action("register".to_string())
                                .with_detail("path", &path_clone)
                                .with_detail("script_uri", &script_uri_clone),
                            )
                            .await;
                    });
                }

                // Register the stream
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY.register_stream(
                    &path,
                    &script_uri_stream,
                    customization_function,
                ) {
                    Ok(()) => Ok(format!("Web stream '{}' registered successfully", path)),
                    Err(e) => Ok(format!("Failed to register stream '{}': {}", path, e)),
                }
            },
        )?;
        route_registry.set("registerStreamRoute", register_stream_route)?;

        // 3. registerAssetRoute function
        let user_ctx_asset = user_context.clone();
        let script_uri_asset = script_uri_owned.clone();
        let register_asset_route = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>,
                  path: String,
                  asset_name: String|
                  -> Result<String, rquickjs::Error> {
                // Check if script is privileged OR user has admin privileges
                let script_privileged = match repository::is_script_privileged(&script_uri_asset) {
                    Ok(true) => true,
                    Ok(false) => false,
                    Err(e) => {
                        return Ok(format!(
                            "Asset route '{}' denied: privilege lookup failed ({})",
                            path, e
                        ));
                    }
                };

                let user_is_admin =
                    user_ctx_asset.has_capability(&crate::security::Capability::DeleteScripts);

                if !script_privileged && !user_is_admin {
                    return Ok(format!(
                        "Asset route '{}' denied: script '{}' is not privileged",
                        path, script_uri_asset
                    ));
                }

                // Check capability
                if let Err(e) =
                    user_ctx_asset.require_capability(&crate::security::Capability::WriteAssets)
                {
                    return Ok(format!("Access denied: {}", e));
                }

                // Validate path
                if !path.starts_with('/') {
                    return Ok("Path must start with '/'".to_string());
                }
                if path.len() > 500 {
                    return Ok("Path too long (max 500 characters)".to_string());
                }

                // Validate asset name
                if asset_name.is_empty() || asset_name.len() > 255 {
                    return Ok("Invalid asset name: must be 1-255 characters".to_string());
                }
                if asset_name.contains("..")
                    || asset_name.contains('/')
                    || asset_name.contains('\\')
                {
                    return Ok("Invalid asset name: path characters not allowed".to_string());
                }

                // Verify the asset exists and belongs to this script
                match repository::fetch_asset(&script_uri_asset, &asset_name) {
                    Some(_) => {
                        // Asset exists and belongs to this script, proceed
                    }
                    None => {
                        return Ok(format!(
                            "Asset '{}' not found or not owned by script '{}'",
                            asset_name, script_uri_asset
                        ));
                    }
                }

                // Register the path in the global asset registry
                match crate::asset_registry::get_global_registry().register_path(
                    &path,
                    &asset_name,
                    &script_uri_asset,
                ) {
                    Ok(()) => Ok(format!(
                        "Asset path '{}' registered to asset '{}'",
                        path, asset_name
                    )),
                    Err(e) => Ok(format!("Failed to register asset path: {}", e)),
                }
            },
        )?;
        route_registry.set("registerAssetRoute", register_asset_route)?;

        // 4. sendStreamMessage function
        let user_ctx_send = user_context.clone();
        let auditor_send = auditor.clone();
        let send_stream_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, path: String, message: String| -> JsResult<String> {
                // Allow system-level broadcasting without capability checks for certain paths
                let is_system_broadcast = path == "/script_updates" || path.starts_with("/system/");

                if !is_system_broadcast {
                    // Check capability for non-system operations
                    if let Err(e) = user_ctx_send
                        .require_capability(&crate::security::Capability::ManageStreams)
                    {
                        let auditor_clone = auditor_send.clone();
                        let user_id = user_ctx_send.user_id.clone();
                        tokio::task::spawn(async move {
                            let _ = auditor_clone
                                .log_event(
                                    crate::security::SecurityEvent::new(
                                        crate::security::SecurityEventType::AuthorizationFailure,
                                        crate::security::SecuritySeverity::Medium,
                                        user_id,
                                    )
                                    .with_resource("stream".to_string())
                                    .with_action("send_message".to_string()),
                                )
                                .await;
                        });
                        return Ok(format!("Error: {}", e));
                    }
                }

                // Log the operation attempt
                let auditor_clone = auditor_send.clone();
                let user_id = user_ctx_send.user_id.clone();
                let path_clone = path.clone();
                let message_clone = message.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("stream".to_string())
                            .with_action("send_message".to_string())
                            .with_detail("path", &path_clone)
                            .with_detail("message_length", message_clone.len().to_string()),
                        )
                        .await;
                });

                // Send the message
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream(&path, &message)
                {
                    Ok(result) => {
                        if result.is_fully_successful() {
                            Ok(format!(
                                "Successfully sent message to {} connections on path '{}'",
                                result.successful_sends, path
                            ))
                        } else {
                            Ok(format!(
                                "Sent message to {}/{} connections on path '{}' ({} failed)",
                                result.successful_sends,
                                result.total_connections,
                                path,
                                result.failed_connections.len()
                            ))
                        }
                    }
                    Err(e) => Ok(format!("Failed to send message to path '{}': {}", path, e)),
                }
            },
        )?;
        route_registry.set("sendStreamMessage", send_stream_message)?;

        // 5. sendStreamMessageFiltered function
        let user_ctx_filtered = user_context.clone();
        let auditor_filtered = auditor.clone();
        let send_stream_message_filtered = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  path: String,
                  message: String,
                  filter_json: Option<String>|
                  -> JsResult<String> {
                // Parse filter criteria
                let metadata_filter: HashMap<String, String> = if let Some(json_str) = filter_json {
                    serde_json::from_str(&json_str).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "filter",
                            "MetadataFilter",
                            &format!("Invalid filter JSON: {}", e),
                        )
                    })?
                } else {
                    HashMap::new()
                };

                // Allow system-level broadcasting for certain paths
                let is_system_broadcast = path == "/script_updates" || path.starts_with("/system/");

                if !is_system_broadcast
                    && let Err(e) = user_ctx_filtered
                        .require_capability(&crate::security::Capability::ManageStreams)
                {
                    let auditor_clone = auditor_filtered.clone();
                    let user_id = user_ctx_filtered.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_event(
                                crate::security::SecurityEvent::new(
                                    crate::security::SecurityEventType::AuthorizationFailure,
                                    crate::security::SecuritySeverity::Medium,
                                    user_id,
                                )
                                .with_resource("stream".to_string())
                                .with_action("send_filtered_message".to_string()),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation
                let auditor_clone = auditor_filtered.clone();
                let user_id = user_ctx_filtered.user_id.clone();
                let path_clone = path.clone();
                let message_clone = message.clone();
                let filter_clone = metadata_filter.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("stream".to_string())
                            .with_action("send_filtered_message".to_string())
                            .with_detail("path", &path_clone)
                            .with_detail("message_length", message_clone.len().to_string())
                            .with_detail("filter_criteria_count", filter_clone.len().to_string()),
                        )
                        .await;
                });

                // Send filtered message
                let result = crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream_with_filter(&path, &message, &metadata_filter);

                match result {
                    Ok(broadcast_result) => {
                        if broadcast_result.is_fully_successful() {
                            Ok(format!(
                                "Successfully sent filtered message to {} connections on path '{}'",
                                broadcast_result.successful_sends, path
                            ))
                        } else {
                            Ok(format!(
                                "Sent filtered message to {}/{} connections on path '{}' ({} failed)",
                                broadcast_result.successful_sends,
                                broadcast_result.total_connections,
                                path,
                                broadcast_result.failed_connections.len()
                            ))
                        }
                    }
                    Err(e) => Ok(format!(
                        "Failed to send filtered message to path '{}': {}",
                        path, e
                    )),
                }
            },
        )?;
        route_registry.set("sendStreamMessageFiltered", send_stream_message_filtered)?;

        // 6. listRoutes function
        let user_ctx_list_routes = user_context.clone();
        let list_routes = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(_e) = user_ctx_list_routes
                    .require_capability(&crate::security::Capability::ReadScripts)
                {
                    return Ok("[]".to_string());
                }

                // Get all script metadata
                match repository::get_all_script_metadata() {
                    Ok(metadata_list) => {
                        let mut all_routes = Vec::new();
                        for metadata in metadata_list {
                            if metadata.initialized && !metadata.registrations.is_empty() {
                                for ((path, method), route_meta) in metadata.registrations {
                                    all_routes.push(serde_json::json!({
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
                        match serde_json::to_string(&all_routes) {
                            Ok(json) => Ok(json),
                            Err(e) => Ok(format!("Error serializing routes: {}", e)),
                        }
                    }
                    Err(e) => Ok(format!("Error fetching routes: {}", e)),
                }
            },
        )?;
        route_registry.set("listRoutes", list_routes)?;

        // 6b. generateOpenApi function
        let user_ctx_openapi = user_context.clone();
        let generate_openapi = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(_e) =
                    user_ctx_openapi.require_capability(&crate::security::Capability::ReadScripts)
                {
                    return Ok("{\"error\": \"Insufficient permissions\"}".to_string());
                }

                // Get Rust-generated OpenAPI spec (includes health, GraphQL, MCP, auth endpoints)
                let rust_spec_str = crate::get_rust_openapi_spec();
                let mut rust_spec: serde_json::Value = match serde_json::from_str(&rust_spec_str) {
                    Ok(spec) => spec,
                    Err(e) => {
                        return Ok(format!(
                            "{{\"error\": \"Failed to parse Rust OpenAPI spec: {}\"}}",
                            e
                        ));
                    }
                };

                // Get JavaScript-registered routes from script metadata
                match repository::get_all_script_metadata() {
                    Ok(metadata_list) => {
                        let mut js_paths = serde_json::Map::new();

                        for metadata in metadata_list {
                            if metadata.initialized && !metadata.registrations.is_empty() {
                                for ((path, method), route_meta) in metadata.registrations {
                                    // Get or create path item
                                    let path_item = js_paths
                                        .entry(path.clone())
                                        .or_insert_with(|| serde_json::json!({}));

                                    let path_obj = path_item.as_object_mut().unwrap();

                                    // Create operation object
                                    let mut operation = serde_json::Map::new();
                                    operation.insert(
                                        "summary".to_string(),
                                        serde_json::json!(
                                            route_meta
                                                .summary
                                                .unwrap_or_else(|| format!("{} {}", method, path))
                                        ),
                                    );

                                    if let Some(desc) = route_meta.description {
                                        operation.insert(
                                            "description".to_string(),
                                            serde_json::json!(desc),
                                        );
                                    }

                                    if !route_meta.tags.is_empty() {
                                        operation.insert(
                                            "tags".to_string(),
                                            serde_json::json!(route_meta.tags),
                                        );
                                    } else {
                                        operation
                                            .insert("tags".to_string(), serde_json::json!(["API"]));
                                    }

                                    // Default response
                                    operation.insert(
                                        "responses".to_string(),
                                        serde_json::json!({
                                            "200": {
                                                "description": "Success"
                                            }
                                        }),
                                    );

                                    // Add operation metadata
                                    operation.insert(
                                        "x-handler".to_string(),
                                        serde_json::json!(route_meta.handler_name),
                                    );
                                    operation.insert(
                                        "x-script-uri".to_string(),
                                        serde_json::json!(metadata.uri),
                                    );
                                    operation.insert(
                                        "x-source".to_string(),
                                        serde_json::json!("javascript"),
                                    );

                                    path_obj.insert(
                                        method.to_lowercase(),
                                        serde_json::json!(operation),
                                    );
                                }
                            }
                        }

                        // Add asset routes from the asset registry
                        let asset_registrations =
                            crate::asset_registry::get_global_registry().get_all_registrations();

                        for (path, registration) in asset_registrations {
                            // Determine MIME type based on file extension
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
                            asset_operation.insert(
                                "summary".to_string(),
                                serde_json::json!(format!(
                                    "Static asset: {}",
                                    registration.asset_name
                                )),
                            );
                            asset_operation.insert(
                                "description".to_string(),
                                serde_json::json!(format!(
                                    "Serves static asset '{}' registered by script '{}'",
                                    registration.asset_name, registration.script_uri
                                )),
                            );
                            asset_operation
                                .insert("tags".to_string(), serde_json::json!(["Assets"]));
                            asset_operation.insert(
                                "responses".to_string(),
                                serde_json::json!({
                                    "200": {
                                        "description": "Asset content",
                                        "content": {
                                            mime_type: {
                                                "schema": {
                                                    "type": "string",
                                                    "format": "binary"
                                                }
                                            }
                                        }
                                    },
                                    "404": {
                                        "description": "Asset not found"
                                    }
                                }),
                            );
                            asset_operation.insert(
                                "x-asset-name".to_string(),
                                serde_json::json!(registration.asset_name),
                            );
                            asset_operation.insert(
                                "x-script-uri".to_string(),
                                serde_json::json!(registration.script_uri),
                            );
                            asset_operation.insert(
                                "x-source".to_string(),
                                serde_json::json!("asset-registry"),
                            );

                            // Add to js_paths so it gets merged
                            let path_entry = js_paths
                                .entry(path)
                                .or_insert_with(|| serde_json::json!({}));

                            if let Some(path_obj) = path_entry.as_object_mut() {
                                path_obj
                                    .insert("get".to_string(), serde_json::json!(asset_operation));
                            }
                        }

                        // Merge JavaScript paths into Rust spec
                        if let Some(rust_paths) = rust_spec["paths"].as_object_mut() {
                            for (path, operations) in js_paths {
                                // If path exists in both, merge operations
                                if let Some(existing) = rust_paths.get_mut(&path) {
                                    if let (Some(existing_obj), Some(new_ops)) =
                                        (existing.as_object_mut(), operations.as_object())
                                    {
                                        for (method, operation) in new_ops {
                                            existing_obj.insert(method.clone(), operation.clone());
                                        }
                                    }
                                } else {
                                    // Path doesn't exist in Rust spec, add it
                                    rust_paths.insert(path, operations);
                                }
                            }
                        }

                        // Collect all unique tags
                        let mut all_tags = std::collections::HashSet::new();
                        if let Some(paths) = rust_spec["paths"].as_object() {
                            for operations in paths.values() {
                                if let Some(ops) = operations.as_object() {
                                    for operation in ops.values() {
                                        if let Some(tags) = operation["tags"].as_array() {
                                            for tag in tags {
                                                if let Some(tag_str) = tag.as_str() {
                                                    all_tags.insert(tag_str.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Update tags in the spec if there are new ones from JavaScript
                        if let Some(existing_tags) = rust_spec["tags"].as_array() {
                            for tag_obj in existing_tags {
                                if let Some(name) = tag_obj["name"].as_str() {
                                    all_tags.insert(name.to_string());
                                }
                            }
                        }

                        // Serialize the merged spec
                        match serde_json::to_string_pretty(&rust_spec) {
                            Ok(json) => Ok(json),
                            Err(e) => Ok(format!(
                                "{{\"error\": \"Failed to serialize merged OpenAPI spec: {}\"}}",
                                e
                            )),
                        }
                    }
                    Err(e) => Ok(format!(
                        "{{\"error\": \"Failed to fetch JavaScript routes: {}\"}}",
                        e
                    )),
                }
            },
        )?;
        route_registry.set("generateOpenApi", generate_openapi)?;

        // 7. listStreams function
        let user_ctx_list_streams = user_context.clone();
        let list_streams = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(_e) = user_ctx_list_streams
                    .require_capability(&crate::security::Capability::ManageStreams)
                {
                    return Ok("[]".to_string());
                }

                // Get streams with metadata
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY.list_streams_with_metadata() {
                    Ok(streams) => {
                        let stream_objects: Vec<serde_json::Value> = streams
                            .iter()
                            .map(|(path, uri)| {
                                serde_json::json!({
                                    "path": path,
                                    "uri": uri,
                                })
                            })
                            .collect();

                        match serde_json::to_string(&stream_objects) {
                            Ok(json) => Ok(json),
                            Err(e) => Ok(format!("Error serializing streams: {}", e)),
                        }
                    }
                    Err(e) => Ok(format!("Error fetching streams: {}", e)),
                }
            },
        )?;
        route_registry.set("listStreams", list_streams)?;

        // 8. listAssets function
        let user_ctx_list_assets = user_context.clone();
        let list_assets = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Vec<String>> {
                // Check capability
                if let Err(_e) = user_ctx_list_assets
                    .require_capability(&crate::security::Capability::ReadAssets)
                {
                    return Ok(Vec::new());
                }

                let assets = repository::fetch_assets("https://example.com/core");
                let asset_names: Vec<String> = assets.keys().cloned().collect();
                Ok(asset_names)
            },
        )?;
        route_registry.set("listAssets", list_assets)?;

        // Set the routeRegistry object on global scope
        global.set("routeRegistry", route_registry)?;

        Ok(())
    }

    /// Setup fetch() function for HTTP requests with secret injection
    fn setup_fetch_function(&self, ctx: &rquickjs::Ctx<'_>, _script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();

        // Create the fetch function (synchronous version)
        let fetch_fn = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  url: String,
                  options_json: Option<String>|
                  -> JsResult<String> {
                // Parse options from JSON string
                let options: crate::http_client::FetchOptions = if let Some(json_str) = options_json
                {
                    serde_json::from_str(&json_str).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "options",
                            "FetchOptions",
                            &format!("Invalid fetch options: {}", e),
                        )
                    })?
                } else {
                    Default::default()
                };

                tracing::debug!("Fetching URL: {}", url);

                // Create HTTP client
                let client = crate::http_client::HttpClient::new().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetch",
                        "client_init",
                        &format!("Failed to create HTTP client: {}", e),
                    )
                })?;

                // Perform the fetch (synchronous)
                let response = client.fetch(url.clone(), options).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetch",
                        "request_failed",
                        &format!("Fetch error: {}", e),
                    )
                })?;

                // Convert response to JSON string
                let response_json = serde_json::to_string(&response).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetch",
                        "serialize",
                        &format!("Failed to serialize response: {}", e),
                    )
                })?;

                Ok(response_json)
            },
        )?;

        global.set("fetch", fetch_fn)?;
        debug!("fetch() function initialized with secret injection support");

        Ok(())
    }

    /// Setup database functions
    fn setup_database_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        let user_context = self.user_context.clone();

        // Create the database namespace object for schema management
        let database_obj = rquickjs::Object::new(ctx.clone())?;

        // database.createTable(tableName) - Create a new table for this script
        let script_uri_create = script_uri_owned.clone();
        let user_ctx_create = user_context.clone();
        let create_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String| -> JsResult<String> {
                debug!(
                    "database.createTable called for script {} with table: {}",
                    script_uri_create, table_name
                );

                // Check permission
                if user_ctx_create
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::create_script_table(&script_uri_create, &table_name) {
                    Ok(physical_name) => Ok(format!(
                        "{{\"success\": true, \"tableName\": \"{}\", \"physicalName\": \"{}\"}}",
                        table_name, physical_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("createTable", create_table)?;

        // database.addIntegerColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_int = script_uri_owned.clone();
        let user_ctx_add_int = user_context.clone();
        let add_integer_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addIntegerColumn called for script {}",
                    script_uri_add_int
                );

                if user_ctx_add_int
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_int,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Integer,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addIntegerColumn", add_integer_column)?;

        // database.addTextColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_text = script_uri_owned.clone();
        let user_ctx_add_text = user_context.clone();
        let add_text_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addTextColumn called for script {}",
                    script_uri_add_text
                );

                if user_ctx_add_text
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_text,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Text,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addTextColumn", add_text_column)?;

        // database.addBooleanColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_bool = script_uri_owned.clone();
        let user_ctx_add_bool = user_context.clone();
        let add_boolean_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addBooleanColumn called for script {}",
                    script_uri_add_bool
                );

                if user_ctx_add_bool
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_bool,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Boolean,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addBooleanColumn", add_boolean_column)?;

        // database.addTimestampColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_ts = script_uri_owned.clone();
        let user_ctx_add_ts = user_context.clone();
        let add_timestamp_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addTimestampColumn called for script {}",
                    script_uri_add_ts
                );

                if user_ctx_add_ts
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_ts,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Timestamp,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addTimestampColumn", add_timestamp_column)?;

        // database.addReferenceColumn(tableName, columnName, referencedTableName, nullable)
        let script_uri_ref = script_uri_owned.clone();
        let user_ctx_ref = user_context.clone();
        let add_reference_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  referenced_table_name: String,
                  nullable: Opt<bool>|
                  -> JsResult<String> {
                debug!(
                    "database.addReferenceColumn called for script {}",
                    script_uri_ref
                );

                if user_ctx_ref
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);

                match crate::repository::add_reference_column(
                    &script_uri_ref,
                    &table_name,
                    &column_name,
                    &referenced_table_name,
                    nullable,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"foreignKey\": \"{}.{} -> {}\", \"nullable\": {}}}",
                        table_name, column_name, referenced_table_name, nullable
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addReferenceColumn", add_reference_column)?;

        // database.dropColumn(tableName, columnName)
        let script_uri_drop_col = script_uri_owned.clone();
        let user_ctx_drop_col = user_context.clone();
        let drop_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String|
                  -> JsResult<String> {
                debug!(
                    "database.dropColumn called for script {} with table: {}, column: {}",
                    script_uri_drop_col, table_name, column_name
                );

                if user_ctx_drop_col
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::drop_column(
                    &script_uri_drop_col,
                    &table_name,
                    &column_name,
                ) {
                    Ok(existed) => {
                        if existed {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"columnName\": \"{}\", \"dropped\": true}}",
                                table_name, column_name
                            ))
                        } else {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"columnName\": \"{}\", \"dropped\": false, \"message\": \"Column did not exist\"}}",
                                table_name, column_name
                            ))
                        }
                    }
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("dropColumn", drop_column)?;

        // database.dropTable(tableName)
        let script_uri_drop = script_uri_owned.clone();
        let user_ctx_drop = user_context.clone();
        let drop_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String| -> JsResult<String> {
                debug!(
                    "database.dropTable called for script {} with table: {}",
                    script_uri_drop, table_name
                );

                if user_ctx_drop
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::drop_script_table(&script_uri_drop, &table_name) {
                    Ok(existed) => {
                        if existed {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"dropped\": true}}",
                                table_name
                            ))
                        } else {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"dropped\": false, \"message\": \"Table did not exist\"}}",
                                table_name
                            ))
                        }
                    }
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("dropTable", drop_table)?;

        // database.query(tableName, filters, limit) - Query rows from table
        let script_uri_query = script_uri_owned.clone();
        let user_ctx_query = user_context.clone();
        let query_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  filters: Opt<String>,
                  limit: Opt<i32>|
                  -> JsResult<String> {
                debug!(
                    "database.query called for script {} on table: {}",
                    script_uri_query, table_name
                );

                if user_ctx_query
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // Parse filters from JSON string if provided
                let filters_map = if let Some(filters_str) = filters.0 {
                    match serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(
                        &filters_str,
                    ) {
                        Ok(map) => Some(map),
                        Err(e) => {
                            return Ok(format!("{{\"error\": \"Invalid filters JSON: {}\"}}", e));
                        }
                    }
                } else {
                    None
                };

                let limit_val = limit.0.map(|l| l as i64);

                match crate::repository::query_table(
                    &script_uri_query,
                    &table_name,
                    filters_map.as_ref(),
                    limit_val,
                ) {
                    Ok(rows) => match serde_json::to_string(&rows) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("query", query_table)?;

        // database.insert(tableName, data) - Insert a row
        let script_uri_insert = script_uri_owned.clone();
        let user_ctx_insert = user_context.clone();
        let insert_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String, data: String| -> JsResult<String> {
                debug!(
                    "database.insert called for script {} on table: {}",
                    script_uri_insert, table_name
                );

                if user_ctx_insert
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(format!("{{\"error\": \"Invalid data JSON: {}\"}}", e)),
                };

                match crate::repository::insert_row(&script_uri_insert, &table_name, &data_map) {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("insert", insert_row)?;

        // database.update(tableName, id, data) - Update a row
        let script_uri_update = script_uri_owned.clone();
        let user_ctx_update = user_context.clone();
        let update_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  id: i32,
                  data: String|
                  -> JsResult<String> {
                debug!(
                    "database.update called for script {} on table: {}, id: {}",
                    script_uri_update, table_name, id
                );

                if user_ctx_update
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(format!("{{\"error\": \"Invalid data JSON: {}\"}}", e)),
                };

                match crate::repository::update_row(&script_uri_update, &table_name, id, &data_map)
                {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("update", update_row)?;

        // database.delete(tableName, id) - Delete a row
        let script_uri_delete = script_uri_owned.clone();
        let user_ctx_delete = user_context.clone();
        let delete_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String, id: i32| -> JsResult<String> {
                debug!(
                    "database.delete called for script {} on table: {}, id: {}",
                    script_uri_delete, table_name, id
                );

                if user_ctx_delete
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::delete_row(&script_uri_delete, &table_name, id) {
                    Ok(deleted) => Ok(format!("{{\"success\": true, \"deleted\": {}}}", deleted)),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("delete", delete_row)?;

        // database.generateGraphQLForTable(tableName, options) - Auto-generate GraphQL operations
        let script_uri_graphql = script_uri_owned.clone();
        let user_ctx_graphql = user_context.clone();
        let generate_graphql = Function::new(
            ctx.clone(),
            move |ctx_inner: rquickjs::Ctx<'_>,
                  table_name: String,
                  options: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.generateGraphQLForTable called for script {} on table: {}",
                    script_uri_graphql, table_name
                );

                if user_ctx_graphql
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // Parse options (default: ScriptInternal visibility)
                let visibility = if let Some(opts_str) = options.0 {
                    match serde_json::from_str::<serde_json::Value>(&opts_str) {
                        Ok(opts) => opts
                            .get("visibility")
                            .and_then(|v| v.as_str())
                            .unwrap_or("script_internal")
                            .to_string(),
                        Err(_) => "script_internal".to_string(),
                    }
                } else {
                    "script_internal".to_string()
                };

                // Get table schema
                let schema =
                    match crate::repository::get_table_schema(&script_uri_graphql, &table_name) {
                        Ok(s) => s,
                        Err(e) => {
                            return Ok(format!(
                                "{{\"error\": \"Failed to get table schema: {}\"}}",
                                e
                            ));
                        }
                    };

                // Get foreign keys
                let foreign_keys =
                    match crate::repository::get_foreign_keys(&script_uri_graphql, &table_name) {
                        Ok(fks) => fks,
                        Err(e) => {
                            return Ok(format!(
                                "{{\"error\": \"Failed to get foreign keys: {}\"}}",
                                e
                            ));
                        }
                    };

                // Generate GraphQL operations
                let operations = crate::graphql_schema_gen::generate_table_operations(
                    &table_name,
                    &schema,
                    &foreign_keys,
                );

                // Inject resolver functions into JavaScript context
                for query in &operations.queries {
                    // Evaluate resolver code in the current context
                    if let Err(e) = ctx_inner.eval::<(), _>(query.resolver_code.as_str()) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to inject resolver {}: {:?}\"}}",
                            query.resolver_function_name, e
                        ));
                    }
                }

                for mutation in &operations.mutations {
                    if let Err(e) = ctx_inner.eval::<(), _>(mutation.resolver_code.as_str()) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to inject resolver {}: {:?}\"}}",
                            mutation.resolver_function_name, e
                        ));
                    }
                }

                // Register queries
                for query in &operations.queries {
                    if let Err(e) = crate::graphql::register_graphql_query(
                        query.name.clone(),
                        query.sdl.clone(),
                        query.resolver_function_name.clone(),
                        script_uri_graphql.clone(),
                        visibility.clone(),
                    ) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to register query {}: {}\"}}",
                            query.name, e
                        ));
                    }
                }

                // Register mutations
                for mutation in &operations.mutations {
                    if let Err(e) = crate::graphql::register_graphql_mutation(
                        mutation.name.clone(),
                        mutation.sdl.clone(),
                        mutation.resolver_function_name.clone(),
                        script_uri_graphql.clone(),
                        visibility.clone(),
                    ) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to register mutation {}: {}\"}}",
                            mutation.name, e
                        ));
                    }
                }

                // Return success with operation names
                let query_names: Vec<&str> =
                    operations.queries.iter().map(|q| q.name.as_str()).collect();
                let mutation_names: Vec<&str> = operations
                    .mutations
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect();

                Ok(format!(
                    "{{\"success\": true, \"table\": \"{}\", \"queries\": {:?}, \"mutations\": {:?}}}",
                    table_name, query_names, mutation_names
                ))
            },
        )?;
        database_obj.set("generateGraphQLForTable", generate_graphql)?;

        // Transaction management functions

        // database.beginTransaction(timeoutMs?) - Start a new transaction or savepoint
        let begin_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, timeout_ms: Opt<u64>| -> JsResult<String> {
                match crate::database::Database::begin_transaction(timeout_ms.0) {
                    Ok(_guard) => {
                        // Note: The guard is not stored here - it's managed internally
                        // Auto-commit/rollback happens at handler boundaries
                        Ok("{\"success\": true, \"message\": \"Transaction started\"}".to_string())
                    }
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("beginTransaction", begin_transaction)?;

        // database.commitTransaction() - Commit the current transaction or release savepoint
        let commit_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::commit_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction committed\"}".to_string(),
                    ),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("commitTransaction", commit_transaction)?;

        // database.rollbackTransaction() - Rollback the current transaction or to savepoint
        let rollback_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::rollback_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction rolled back\"}".to_string(),
                    ),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("rollbackTransaction", rollback_transaction)?;

        // database.createSavepoint(name?) - Create a named or auto-generated savepoint
        let create_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: Opt<String>| -> JsResult<String> {
                match crate::database::Database::create_savepoint(name.0.as_deref()) {
                    Ok(savepoint_name) => Ok(format!(
                        "{{\"success\": true, \"savepoint\": \"{}\"}}",
                        savepoint_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("createSavepoint", create_savepoint)?;

        // database.rollbackToSavepoint(name) - Rollback to a named savepoint
        let rollback_to_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String| -> JsResult<String> {
                match crate::database::Database::rollback_to_savepoint(&name) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"message\": \"Rolled back to savepoint: {}\"}}",
                        name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("rollbackToSavepoint", rollback_to_savepoint)?;

        // database.releaseSavepoint(name) - Release a named savepoint
        let release_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String| -> JsResult<String> {
                match crate::database::Database::release_savepoint(&name) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"message\": \"Released savepoint: {}\"}}",
                        name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("releaseSavepoint", release_savepoint)?;

        // database.checkDatabaseHealth() - Check database health status
        let check_db_health = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Call the database health check
                let result = crate::database::Database::check_health_sync();
                Ok(result)
            },
        )?;
        database_obj.set("checkDatabaseHealth", check_db_health)?;

        // Set the database object on the global scope
        global.set("database", database_obj)?;

        debug!(
            "database JavaScript API initialized for script: {}",
            script_uri
        );

        Ok(())
    }

    /// Setup conversion functions (markdown to HTML, etc.)
    fn setup_conversion_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        _script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();

        // Create the convert namespace object
        let convert_obj = rquickjs::Object::new(ctx.clone())?;

        // convert.markdown_to_html(markdown) - Convert markdown string to HTML
        let markdown_to_html = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, markdown: String| -> JsResult<String> {
                // Call the conversion function
                match crate::conversion::convert_markdown_to_html(&markdown) {
                    Ok(html) => Ok(html),
                    Err(e) => {
                        // Return error as string (following pattern of other APIs)
                        Ok(format!("Error: {}", e))
                    }
                }
            },
        )?;

        // convert.render_handlebars_template(template, data) - Render Handlebars template
        let render_handlebars_template = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, template: String, data: String| -> JsResult<String> {
                // Call the conversion function
                match crate::conversion::render_handlebars_template(&template, &data) {
                    Ok(rendered) => Ok(rendered),
                    Err(e) => {
                        // Return error as string (following pattern of other APIs)
                        Ok(format!("Error: {}", e))
                    }
                }
            },
        )?;

        convert_obj.set("markdown_to_html", markdown_to_html)?;
        convert_obj.set("render_handlebars_template", render_handlebars_template)?;
        global.set("convert", convert_obj)?;

        debug!(
            "convert.markdown_to_html() and convert.render_handlebars_template() functions initialized"
        );

        Ok(())
    }

    /// Setup secure script storage functions
    fn setup_shared_storage_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        // Create the sharedStorage namespace object
        let shared_storage_obj = rquickjs::Object::new(ctx.clone())?;

        // sharedStorage.getItem(key) - Get a storage item
        let script_uri_get = script_uri_owned.clone();
        let get_item = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<Option<String>> {
                debug!(
                    "sharedStorage.getItem called for script {} with key: {}",
                    script_uri_get, key
                );
                Ok(crate::repository::get_shared_storage_item(
                    &script_uri_get,
                    &key,
                ))
            },
        )?;
        shared_storage_obj.set("getItem", get_item)?;

        // sharedStorage.setItem(key, value) - Set a storage item
        let script_uri_set = script_uri_owned.clone();
        let set_item = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, key: String, value: String| -> JsResult<String> {
                debug!(
                    "sharedStorage.setItem called for script {} with key: {}",
                    script_uri_set, key
                );

                // Validate inputs
                if key.trim().is_empty() {
                    return Ok("Error: Key cannot be empty".to_string());
                }

                if value.len() > 1_000_000 {
                    return Ok("Error: Value too large (>1MB)".to_string());
                }

                match crate::repository::set_shared_storage_item(&script_uri_set, &key, &value) {
                    Ok(()) => Ok("Item set successfully".to_string()),
                    Err(e) => Ok(format!("Error setting item: {}", e)),
                }
            },
        )?;
        shared_storage_obj.set("setItem", set_item)?;

        // sharedStorage.removeItem(key) - Remove a storage item
        let script_uri_remove = script_uri_owned.clone();
        let remove_item = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<bool> {
                debug!(
                    "sharedStorage.removeItem called for script {} with key: {}",
                    script_uri_remove, key
                );
                Ok(crate::repository::remove_shared_storage_item(
                    &script_uri_remove,
                    &key,
                ))
            },
        )?;
        shared_storage_obj.set("removeItem", remove_item)?;

        // sharedStorage.clear() - Clear all items for this script
        let script_uri_clear = script_uri_owned.clone();
        let clear_storage = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                debug!("sharedStorage.clear called for script {}", script_uri_clear);
                match crate::repository::clear_shared_storage(&script_uri_clear) {
                    Ok(()) => Ok("Storage cleared successfully".to_string()),
                    Err(e) => Ok(format!("Error clearing storage: {}", e)),
                }
            },
        )?;
        shared_storage_obj.set("clear", clear_storage)?;

        // Set the sharedStorage object on the global scope
        global.set("sharedStorage", shared_storage_obj)?;

        debug!(
            "sharedStorage JavaScript API initialized for script: {}",
            script_uri
        );

        Ok(())
    }

    fn setup_personal_storage_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        // Create the personalStorage namespace object
        let personal_storage_obj = rquickjs::Object::new(ctx.clone())?;

        // personalStorage.getItem(key) - Get a storage item for current user
        let script_uri_get = script_uri_owned.clone();
        let get_item = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<Option<String>> {
                debug!(
                    "personalStorage.getItem called for script {} with key: {}",
                    script_uri_get, key
                );

                // Try to get current request auth from context
                let globals = ctx.globals();
                let context_obj: rquickjs::Object = match globals.get("context") {
                    Ok(v) => v,
                    Err(_) => {
                        warn!("personalStorage.getItem: no context available");
                        return Ok(None);
                    }
                };

                let request_obj: rquickjs::Object = match context_obj.get("request") {
                    Ok(v) => v,
                    Err(_) => {
                        warn!("personalStorage.getItem: no request in context");
                        return Ok(None);
                    }
                };

                let auth_obj: rquickjs::Object = match request_obj.get("auth") {
                    Ok(v) => v,
                    Err(_) => {
                        warn!("personalStorage.getItem: no auth in request");
                        return Ok(None);
                    }
                };

                let is_authenticated: bool = auth_obj.get("isAuthenticated").unwrap_or_default();

                if !is_authenticated {
                    return Ok(None);
                }

                let user_id: String = match auth_obj.get("userId") {
                    Ok(Some(v)) => v,
                    _ => return Ok(None),
                };

                Ok(crate::repository::get_personal_storage_item(
                    &script_uri_get,
                    &user_id,
                    &key,
                ))
            },
        )?;
        personal_storage_obj.set("getItem", get_item)?;

        // personalStorage.setItem(key, value) - Set a storage item for current user
        let script_uri_set = script_uri_owned.clone();
        let set_item =
            Function::new(
                ctx.clone(),
                move |ctx: rquickjs::Ctx<'_>, key: String, value: String| -> JsResult<String> {
                    debug!(
                        "personalStorage.setItem called for script {} with key: {}",
                        script_uri_set, key
                    );

                    // Try to get current request auth from context
                    let globals = ctx.globals();
                    let context_obj: rquickjs::Object =
                        match globals.get("context") {
                            Ok(v) => v,
                            Err(_) => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    let request_obj: rquickjs::Object =
                        match context_obj.get("request") {
                            Ok(v) => v,
                            Err(_) => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    let auth_obj: rquickjs::Object =
                        match request_obj.get("auth") {
                            Ok(v) => v,
                            Err(_) => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    let is_authenticated: bool =
                        auth_obj.get("isAuthenticated").unwrap_or_default();

                    if !is_authenticated {
                        return Ok(
                            "Error: Personal storage requires authentication. Please log in."
                                .to_string(),
                        );
                    }

                    let user_id: String =
                        match auth_obj.get("userId") {
                            Ok(Some(v)) => v,
                            _ => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    // Validate inputs
                    if key.trim().is_empty() {
                        return Ok("Error: Key cannot be empty".to_string());
                    }

                    if value.len() > 1_000_000 {
                        return Ok("Error: Value too large (>1MB)".to_string());
                    }

                    match crate::repository::set_personal_storage_item(
                        &script_uri_set,
                        &user_id,
                        &key,
                        &value,
                    ) {
                        Ok(()) => Ok("Item set successfully".to_string()),
                        Err(e) => Ok(format!("Error setting item: {}", e)),
                    }
                },
            )?;
        personal_storage_obj.set("setItem", set_item)?;

        // personalStorage.removeItem(key) - Remove a storage item for current user
        let script_uri_remove = script_uri_owned.clone();
        let remove_item = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<bool> {
                debug!(
                    "personalStorage.removeItem called for script {} with key: {}",
                    script_uri_remove, key
                );

                // Try to get current request auth from context
                let globals = ctx.globals();
                let context_obj: rquickjs::Object = match globals.get("context") {
                    Ok(v) => v,
                    Err(_) => return Ok(false),
                };

                let request_obj: rquickjs::Object = match context_obj.get("request") {
                    Ok(v) => v,
                    Err(_) => return Ok(false),
                };

                let auth_obj: rquickjs::Object = match request_obj.get("auth") {
                    Ok(v) => v,
                    Err(_) => return Ok(false),
                };

                let is_authenticated: bool = auth_obj.get("isAuthenticated").unwrap_or_default();

                if !is_authenticated {
                    return Ok(false);
                }

                let user_id: String = match auth_obj.get("userId") {
                    Ok(Some(v)) => v,
                    _ => return Ok(false),
                };

                Ok(crate::repository::remove_personal_storage_item(
                    &script_uri_remove,
                    &user_id,
                    &key,
                ))
            },
        )?;
        personal_storage_obj.set("removeItem", remove_item)?;

        // personalStorage.clear() - Clear all items for current user
        let script_uri_clear = script_uri_owned.clone();
        let clear_storage =
            Function::new(
                ctx.clone(),
                move |ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                    debug!(
                        "personalStorage.clear called for script {}",
                        script_uri_clear
                    );

                    // Try to get current request auth from context
                    let globals = ctx.globals();
                    let context_obj: rquickjs::Object =
                        match globals.get("context") {
                            Ok(v) => v,
                            Err(_) => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    let request_obj: rquickjs::Object =
                        match context_obj.get("request") {
                            Ok(v) => v,
                            Err(_) => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    let auth_obj: rquickjs::Object =
                        match request_obj.get("auth") {
                            Ok(v) => v,
                            Err(_) => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    let is_authenticated: bool =
                        auth_obj.get("isAuthenticated").unwrap_or_default();

                    if !is_authenticated {
                        return Ok(
                            "Error: Personal storage requires authentication. Please log in."
                                .to_string(),
                        );
                    }

                    let user_id: String =
                        match auth_obj.get("userId") {
                            Ok(Some(v)) => v,
                            _ => return Ok(
                                "Error: Personal storage requires authentication. Please log in."
                                    .to_string(),
                            ),
                        };

                    match crate::repository::clear_personal_storage(&script_uri_clear, &user_id) {
                        Ok(()) => Ok("Storage cleared successfully".to_string()),
                        Err(e) => Ok(format!("Error clearing storage: {}", e)),
                    }
                },
            )?;
        personal_storage_obj.set("clear", clear_storage)?;

        // Set the personalStorage object on the global scope
        global.set("personalStorage", personal_storage_obj)?;

        debug!(
            "personalStorage JavaScript API initialized for script: {}",
            script_uri
        );

        Ok(())
    }

    fn setup_scheduler_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        if !self.config.enable_scheduler {
            return Ok(());
        }

        let global = ctx.globals();
        let scheduler_obj = rquickjs::Object::new(ctx.clone())?;
        let scheduler_handle = scheduler::get_scheduler();

        let register_once_handle = scheduler_handle.clone();
        let script_uri_once = script_uri.to_string();
        let register_once =
            Function::new(
                ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>, options: rquickjs::Object| -> JsResult<String> {
                    if let Err(msg) = ensure_scheduler_privileged(&script_uri_once) {
                        return Ok(msg);
                    }

                    let handler: String = match options.get("handler") {
                        Ok(value) => value,
                        Err(_) => {
                            return Ok("schedulerService.registerOnce requires options.handler"
                                .to_string());
                        }
                    };
                    let handler_name = handler.trim();
                    if handler_name.is_empty() {
                        return Ok(
                            "schedulerService.registerOnce requires a non-empty handler name"
                                .to_string(),
                        );
                    }

                    let run_at_value: String = match options.get("runAt") {
                        Ok(value) => value,
                        Err(_) => return Ok(
                            "schedulerService.registerOnce requires options.runAt (UTC ISO string)"
                                .to_string(),
                        ),
                    };
                    let run_at = match scheduler::parse_utc_timestamp(&run_at_value) {
                        Ok(ts) => ts,
                        Err(err) => return Ok(format!("Scheduler error: {}", err)),
                    };

                    let name = options.get::<_, String>("name").ok();

                    match register_once_handle.register_one_off(
                        &script_uri_once,
                        handler_name,
                        name,
                        run_at,
                    ) {
                        Ok(job) => Ok(format!(
                            "Scheduled one-time job '{}' for {} (id {})",
                            job.key,
                            job.schedule.next_run().to_rfc3339(),
                            job.id
                        )),
                        Err(err) => Ok(format!("Scheduler error: {}", err)),
                    }
                },
            )?;

        let register_recurring_handle = scheduler_handle.clone();
        let script_uri_recurring = script_uri.to_string();
        let register_recurring = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, options: rquickjs::Object| -> JsResult<String> {
                if let Err(msg) = ensure_scheduler_privileged(&script_uri_recurring) {
                    return Ok(msg);
                }

                let handler: String = match options.get("handler") {
                    Ok(value) => value,
                    Err(_) => {
                        return Ok(
                            "schedulerService.registerRecurring requires options.handler"
                                .to_string(),
                        );
                    }
                };
                let handler_name = handler.trim();
                if handler_name.is_empty() {
                    return Ok(
                        "schedulerService.registerRecurring requires a non-empty handler name"
                            .to_string(),
                    );
                }

                let interval_value: f64 =
                    match options.get("intervalMinutes") {
                        Ok(value) => value,
                        Err(_) => return Ok(
                            "schedulerService.registerRecurring requires options.intervalMinutes"
                                .to_string(),
                        ),
                    };
                if !interval_value.is_finite() || interval_value < 1.0 {
                    return Ok(
                        "schedulerService.registerRecurring requires intervalMinutes >= 1"
                            .to_string(),
                    );
                }
                let interval_minutes = interval_value.floor() as i64;
                let interval = ChronoDuration::minutes(interval_minutes);

                let name = options.get::<_, String>("name").ok();
                let first_run = if let Ok(start_at) = options.get::<_, String>("startAt") {
                    match scheduler::parse_utc_timestamp(&start_at) {
                        Ok(ts) => Some(ts),
                        Err(err) => return Ok(format!("Scheduler error: {}", err)),
                    }
                } else {
                    None
                };

                match register_recurring_handle.register_recurring(
                    &script_uri_recurring,
                    handler_name,
                    name,
                    interval,
                    first_run,
                ) {
                    Ok(job) => Ok(format!(
                        "Scheduled recurring job '{}' every {} minute(s); next run {} (id {})",
                        job.key,
                        interval_minutes,
                        job.schedule.next_run().to_rfc3339(),
                        job.id
                    )),
                    Err(err) => Ok(format!("Scheduler error: {}", err)),
                }
            },
        )?;

        let script_uri_clear = script_uri.to_string();
        let clear_all = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                if let Err(msg) = ensure_scheduler_privileged(&script_uri_clear) {
                    return Ok(msg);
                }

                let removed = scheduler::clear_script_jobs(&script_uri_clear);
                Ok(format!(
                    "Cleared {} scheduled job(s) for {}",
                    removed, script_uri_clear
                ))
            },
        )?;

        scheduler_obj.set("registerOnce", register_once)?;
        scheduler_obj.set("registerRecurring", register_recurring)?;
        scheduler_obj.set("clearAll", clear_all)?;
        global.set("schedulerService", scheduler_obj)?;

        Ok(())
    }

    /// Setup user management functions (admin-only)
    fn setup_user_management_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        _script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();

        // listUsers - Get all users (admin only)
        let user_ctx_list = user_context.clone();
        let list_users = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check if user has admin capabilities (DeleteScripts is admin-only)
                if !user_ctx_list.has_capability(&crate::security::Capability::DeleteScripts) {
                    return Err(rquickjs::Error::new_from_js_message(
                        "listUsers",
                        "permission_denied",
                        "Administrator privileges required",
                    ));
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "listUsers called by admin"
                );

                // Get all users from repository
                let users = crate::user_repository::list_users().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "listUsers",
                        "error",
                        &format!("Failed to list users: {}", e),
                    )
                })?;

                // Convert users to JSON-friendly format
                let users_json: Vec<serde_json::Value> = users
                    .iter()
                    .map(|user| {
                        serde_json::json!({
                            "id": user.id,
                            "email": user.email,
                            "name": user.name,
                            "roles": user.roles.iter().map(|r| format!("{:?}", r)).collect::<Vec<_>>(),
                            "created_at": format!("{:?}", user.created_at),
                            "providers": user.providers.iter().map(|p| &p.provider_name).collect::<Vec<_>>(),
                        })
                    })
                    .collect();

                // Serialize to JSON string
                serde_json::to_string(&users_json).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "listUsers",
                        "serialize_error",
                        &format!("Failed to serialize users: {}", e),
                    )
                })
            },
        )?;

        // addUserRole - Add a role to a user (admin only)
        let user_ctx_add = user_context.clone();
        let add_user_role = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, user_id: String, role: String| -> JsResult<()> {
                // Check if user has admin capabilities (DeleteScripts is admin-only)
                if !user_ctx_add.has_capability(&crate::security::Capability::DeleteScripts) {
                    return Err(rquickjs::Error::new_from_js_message(
                        "addUserRole",
                        "permission_denied",
                        "Administrator privileges required",
                    ));
                }

                debug!(
                    admin_id = ?user_ctx_add.user_id,
                    target_user = %user_id,
                    role = %role,
                    "addUserRole called by admin"
                );

                // Parse role
                let user_role = match role.as_str() {
                    "Editor" => crate::user_repository::UserRole::Editor,
                    "Administrator" => crate::user_repository::UserRole::Administrator,
                    "Authenticated" => crate::user_repository::UserRole::Authenticated,
                    _ => {
                        return Err(rquickjs::Error::new_from_js_message(
                            "addUserRole",
                            "invalid_role",
                            &format!(
                                "Invalid role: {}. Must be Editor, Administrator, or Authenticated",
                                role
                            ),
                        ));
                    }
                };

                // Add role
                crate::user_repository::add_user_role(&user_id, user_role).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "addUserRole",
                        "error",
                        &format!("Failed to add role: {}", e),
                    )
                })?;

                tracing::info!(
                    admin_id = ?user_ctx_add.user_id,
                    target_user = %user_id,
                    role = %role,
                    "Role added successfully"
                );

                Ok(())
            },
        )?;

        // removeUserRole - Remove a role from a user (admin only)
        let user_ctx_remove = user_context.clone();
        let remove_user_role = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, user_id: String, role: String| -> JsResult<()> {
                // Check if user has admin capabilities (DeleteScripts is admin-only)
                if !user_ctx_remove.has_capability(&crate::security::Capability::DeleteScripts) {
                    return Err(rquickjs::Error::new_from_js_message(
                        "removeUserRole",
                        "permission_denied",
                        "Administrator privileges required",
                    ));
                }

                debug!(
                    admin_id = ?user_ctx_remove.user_id,
                    target_user = %user_id,
                    role = %role,
                    "removeUserRole called by admin"
                );

                // Parse role
                let user_role = match role.as_str() {
                    "Editor" => crate::user_repository::UserRole::Editor,
                    "Administrator" => crate::user_repository::UserRole::Administrator,
                    "Authenticated" => {
                        return Err(rquickjs::Error::new_from_js_message(
                            "removeUserRole",
                            "invalid_operation",
                            "Cannot remove Authenticated role",
                        ));
                    }
                    _ => {
                        return Err(rquickjs::Error::new_from_js_message(
                            "removeUserRole",
                            "invalid_role",
                            &format!("Invalid role: {}. Must be Editor or Administrator", role),
                        ));
                    }
                };

                // Remove role
                crate::user_repository::remove_user_role(&user_id, &user_role).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "removeUserRole",
                        "error",
                        &format!("Failed to remove role: {}", e),
                    )
                })?;

                tracing::info!(
                    admin_id = ?user_ctx_remove.user_id,
                    target_user = %user_id,
                    role = %role,
                    "Role removed successfully"
                );

                Ok(())
            },
        )?;

        // Create userStorage object and set methods on it
        let user_storage = rquickjs::Object::new(ctx.clone())?;
        user_storage.set("listUsers", list_users)?;
        user_storage.set("addUserRole", add_user_role)?;
        user_storage.set("removeUserRole", remove_user_role)?;
        global.set("userStorage", user_storage)?;

        debug!("User management functions initialized (admin-only)");
        Ok(())
    }

    /// Setup message dispatcher functions for inter-script communication
    fn setup_dispatcher_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let dispatcher_obj = rquickjs::Object::new(ctx.clone())?;

        // registerListener(messageType, handlerName)
        let script_uri_register = script_uri.to_string();
        let register_listener = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  message_type: String,
                  handler_name: String|
                  -> JsResult<String> {
                // Validate inputs
                if message_type.is_empty() {
                    return Ok(
                        "dispatcher.registerListener: message type cannot be empty".to_string()
                    );
                }
                if handler_name.is_empty() {
                    return Ok(
                        "dispatcher.registerListener: handler name cannot be empty".to_string()
                    );
                }

                // Register the listener
                match crate::dispatcher::GLOBAL_DISPATCHER.register_listener(
                    message_type.clone(),
                    script_uri_register.clone(),
                    handler_name.clone(),
                ) {
                    Ok(()) => {
                        debug!(
                            "Registered listener for message type '{}' in script '{}': handler={}",
                            message_type, script_uri_register, handler_name
                        );
                        Ok(format!(
                            "Registered listener for message type '{}': handler '{}'",
                            message_type, handler_name
                        ))
                    }
                    Err(e) => {
                        error!(
                            "Failed to register listener for message type '{}' in script '{}': {}",
                            message_type, script_uri_register, e
                        );
                        Ok(format!("Failed to register listener: {}", e))
                    }
                }
            },
        )?;

        // sendMessage(messageType, messageData)
        // Note: messageData should be a JSON string or will be converted to empty object
        let send_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  message_type: String,
                  message_data_json: Opt<String>|
                  -> JsResult<String> {
                // Validate message type
                if message_type.is_empty() {
                    return Ok("dispatcher.sendMessage: message type cannot be empty".to_string());
                }

                // Get message data as JSON string
                let message_data_json = message_data_json.0.unwrap_or_else(|| "{}".to_string());

                // Get listeners for this message type
                let listeners =
                    match crate::dispatcher::GLOBAL_DISPATCHER.get_listeners(&message_type) {
                        Ok(listeners) => listeners,
                        Err(e) => {
                            error!(
                                "Failed to get listeners for message type '{}': {}",
                                message_type, e
                            );
                            return Ok(format!("Failed to get listeners: {}", e));
                        }
                    };

                if listeners.is_empty() {
                    debug!(
                        "No listeners registered for message type '{}'",
                        message_type
                    );
                    return Ok(format!("No listeners for message type '{}'", message_type));
                }

                debug!(
                    "Dispatching message type '{}' to {} listener(s)",
                    message_type,
                    listeners.len()
                );

                // Invoke each listener handler
                let mut successful = 0;
                let mut failed = 0;

                for listener in listeners.iter() {
                    debug!(
                        "Invoking handler '{}' in script '{}' for message type '{}'",
                        listener.handler_name, listener.script_uri, message_type
                    );

                    // Load the script content
                    let script_content = match repository::fetch_script(&listener.script_uri) {
                        Some(content) => content,
                        None => {
                            warn!(
                                "Script '{}' not found for handler '{}'",
                                listener.script_uri, listener.handler_name
                            );
                            failed += 1;
                            continue;
                        }
                    };

                    // Execute the handler in a new context
                    match execute_message_handler(
                        listener.script_uri.clone(),
                        &script_content,
                        &listener.handler_name,
                        &message_type,
                        &message_data_json,
                    ) {
                        Ok(_) => {
                            debug!(
                                "Successfully invoked handler '{}' in script '{}'",
                                listener.handler_name, listener.script_uri
                            );
                            successful += 1;
                        }
                        Err(e) => {
                            error!(
                                "Failed to invoke handler '{}' in script '{}': {}",
                                listener.handler_name, listener.script_uri, e
                            );
                            failed += 1;
                        }
                    }
                }

                Ok(format!(
                    "Dispatched message type '{}': {} successful, {} failed",
                    message_type, successful, failed
                ))
            },
        )?;

        dispatcher_obj.set("registerListener", register_listener)?;
        dispatcher_obj.set("sendMessage", send_message)?;
        global.set("dispatcher", dispatcher_obj)?;

        debug!("Dispatcher functions initialized");
        Ok(())
    }
}

/// Execute a message handler function in a script
fn execute_message_handler(
    script_uri: String,
    script_content: &str,
    handler_name: &str,
    message_type: &str,
    message_data_json: &str,
) -> Result<(), String> {
    use rquickjs::{Context, Runtime};

    // Create a new runtime and context for handler execution
    let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("Failed to create context: {}", e))?;

    ctx.with(|ctx| -> Result<(), String> {
        // Set up minimal secure global functions for handler execution
        let user_context = UserContext::admin("dispatcher".to_string());
        let security_config = GlobalSecurityConfig {
            enable_graphql_registration: false,
            enable_asset_management: false,
            enable_streams: false,
            enable_script_management: false,
            enable_scheduler: false,
            enable_logging: true,
            enable_secrets: false,
            enforce_strict_validation: false,
            enable_audit_logging: false,
        };

        let secure_context = SecureGlobalContext::new_with_config(user_context, security_config);
        secure_context
            .setup_secure_functions(&ctx, &script_uri, None)
            .map_err(|e| format!("Failed to setup secure functions: {}", e))?;

        // Evaluate the script
        ctx.eval::<(), _>(script_content)
            .map_err(|e| format!("Script evaluation failed: {}", e))?;

        // Parse message data back to JavaScript value
        let message_data_value: rquickjs::Value = ctx
            .json_parse(message_data_json)
            .map_err(|e| format!("Failed to parse message data: {}", e))?;

        // Create context object with message data
        let context_obj = rquickjs::Object::new(ctx.clone())
            .map_err(|e| format!("Failed to create context object: {}", e))?;
        context_obj
            .set("messageType", message_type)
            .map_err(|e| format!("Failed to set messageType: {}", e))?;
        context_obj
            .set("messageData", message_data_value)
            .map_err(|e| format!("Failed to set messageData: {}", e))?;

        // Get the handler function
        let global = ctx.globals();
        let handler: rquickjs::Function = global
            .get(handler_name)
            .map_err(|e| format!("Handler function '{}' not found: {}", handler_name, e))?;

        // Call the handler with the context
        handler
            .call::<_, ()>((context_obj,))
            .map_err(|e| format!("Handler execution failed: {}", e))?;

        Ok(())
    })
    .map_err(|e| format!("Context execution failed: {}", e))?;

    Ok(())
}

fn ensure_scheduler_privileged(script_uri: &str) -> Result<(), String> {
    match repository::is_script_privileged(script_uri) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "schedulerService is restricted to privileged scripts ({}).",
            script_uri
        )),
        Err(e) => Err(format!(
            "Unable to verify scheduler privileges for '{}': {}",
            script_uri, e
        )),
    }
}
