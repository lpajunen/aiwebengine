use rquickjs::{Function, Result as JsResult};
use tracing::{debug, warn};

use crate::repository;
use crate::security::UserContext;

/// Type alias for route registration function
type RouteRegisterFn = dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>;

/// Simplified secure wrapper for JavaScript global functions for testing
pub struct SecureGlobalContext {
    user_context: UserContext,
}

#[derive(Debug, Clone)]
pub struct GlobalSecurityConfig {
    pub enable_graphql_registration: bool,
    pub enable_asset_management: bool,
    pub enable_streams: bool,
    pub enable_script_management: bool,
    pub enable_logging: bool,
    pub enforce_strict_validation: bool,
    pub enable_audit_logging: bool,
}

impl Default for GlobalSecurityConfig {
    fn default() -> Self {
        Self {
            enable_streams: true,
            enable_graphql_registration: true,
            enable_asset_management: true,
            enable_script_management: true,
            enable_logging: true,
            enforce_strict_validation: true,
            enable_audit_logging: false, // Disabled for testing
        }
    }
}

impl SecureGlobalContext {
    pub fn new(user_context: UserContext) -> Self {
        Self { user_context }
    }

    pub fn new_with_config(user_context: UserContext, _config: GlobalSecurityConfig) -> Self {
        Self { user_context }
    }

    /// Setup minimal secure logging functions for testing
    fn setup_logging_functions(&self, ctx: &rquickjs::Ctx<'_>, _script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();

        // Secure writeLog function - simplified for testing
        let user_ctx_write = user_context.clone();
        let write_log = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, message: String, level: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_write.require_capability(&crate::security::Capability::ViewLogs)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_write.user_id,
                    message_len = message.len(),
                    level = %level,
                    "Secure writeLog called"
                );

                // Write to repository with the specified level
                repository::insert_log_message("", &message, &level);

                Ok("Log written successfully".to_string())
            },
        )?;

        // Secure listLogs function
        let user_ctx_list = user_context.clone();
        let list_logs = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure console.listLogs called"
                );

                let logs = repository::fetch_log_messages("");

                // Create JSON array of log objects (same format as secure_globals.rs)
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

                Ok(serde_json::to_string(&log_objects).unwrap_or_else(|_| "[]".to_string()))
            },
        )?;

        // Secure listLogsForUri function - now returns same format as listLogs
        let user_ctx_list_uri = user_context.clone();
        let list_logs_for_uri = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                debug!(
                    uri = %uri,
                    user_id = ?user_ctx_list_uri.user_id,
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

                Ok(serde_json::to_string(&log_objects).unwrap_or_else(|_| "[]".to_string()))
            },
        )?;

        // Create console object using JavaScript to avoid multiple ctx.clone() calls
        // This creates wrapper functions in JavaScript space that call write_log with different levels
        // and also attaches listLogs and listLogsForUri as methods
        global.set("__writeLog", write_log)?;
        global.set("__listLogs", list_logs)?;
        global.set("__listLogsForUri", list_logs_for_uri)?;
        ctx.eval::<(), _>(
            r#"
            (function() {
                const writeLog = globalThis.__writeLog;
                const listLogs = globalThis.__listLogs;
                const listLogsForUri = globalThis.__listLogsForUri;
                globalThis.console = {
                    log: function(msg) { return writeLog(msg, "LOG"); },
                    info: function(msg) { return writeLog(msg, "INFO"); },
                    warn: function(msg) { return writeLog(msg, "WARN"); },
                    error: function(msg) { return writeLog(msg, "ERROR"); },
                    debug: function(msg) { return writeLog(msg, "DEBUG"); },
                    listLogs: function() { return listLogs(); },
                    listLogsForUri: function(uri) { return listLogsForUri(uri); }
                };
                delete globalThis.__writeLog;
                delete globalThis.__listLogs;
                delete globalThis.__listLogsForUri;
            })();
        "#,
        )?;

        Ok(())
    }

    /// Setup minimal script management functions for testing
    fn setup_script_management_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        _script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();

        // Create scriptStorage object
        let script_storage = rquickjs::Object::new(ctx.clone())?;

        // Secure listScripts function
        let user_ctx_list = user_context.clone();
        let list_scripts = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Vec<String>> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadScripts)
                {
                    return Ok(Vec::new());
                }

                let scripts = repository::fetch_scripts();
                Ok(scripts.keys().cloned().collect())
            },
        )?;
        script_storage.set("listScripts", list_scripts)?;

        // Secure getScript function
        let user_ctx_get = user_context.clone();
        let get_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<Option<String>> {
                debug!(
                    script_name = %script_name,
                    user_id = ?user_ctx_get.user_id,
                    "Secure getScript called"
                );

                Ok(repository::fetch_script(&script_name))
            },
        )?;
        script_storage.set("getScript", get_script)?;

        // Secure getScriptInitStatus function
        let user_ctx_meta = user_context.clone();
        let get_script_init_status = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<Option<String>> {
                debug!(
                    script_name = %script_name,
                    user_id = ?user_ctx_meta.user_id,
                    "Secure getScriptInitStatus called"
                );

                match repository::get_script_metadata(&script_name) {
                    Ok(metadata) => {
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

        // Secure upsertScript function - simplified for testing
        let user_ctx_upsert = user_context.clone();
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

                // Store the script using repository
                let _ = repository::upsert_script(&script_name, &js_script);

                debug!(
                    script_name = %script_name,
                    user_id = ?user_ctx_upsert.user_id,
                    "Secure upsertScript called"
                );

                // Initialize the script asynchronously in the background
                let script_name_for_init = script_name.clone();
                tokio::task::spawn(async move {
                    let initializer = crate::script_init::ScriptInitializer::new(5000);
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

        // Secure deleteScript function - simplified for testing
        let user_ctx_delete = user_context.clone();
        let delete_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<bool> {
                // Check capability
                if let Err(e) =
                    user_ctx_delete.require_capability(&crate::security::Capability::DeleteScripts)
                {
                    warn!(
                        script_name = %script_name,
                        user_id = ?user_ctx_delete.user_id,
                        error = %e,
                        "deleteScript capability check failed"
                    );
                    return Ok(false);
                }

                debug!(
                    script_name = %script_name,
                    user_id = ?user_ctx_delete.user_id,
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
}

/// Setup secure global functions - simplified version for testing
pub fn setup_secure_global_functions(
    ctx: &rquickjs::Ctx<'_>,
    script_uri: &str,
    user_context: UserContext,
    config: Option<GlobalSecurityConfig>,
    _register_impl: Option<Box<RouteRegisterFn>>,
) -> Result<(), rquickjs::Error> {
    let config = config.unwrap_or_default();
    let secure_context = SecureGlobalContext::new_with_config(user_context, config.clone());

    // Always setup logging functions
    secure_context.setup_logging_functions(ctx, script_uri)?;

    // Setup script management if enabled
    if config.enable_script_management {
        secure_context.setup_script_management_functions(ctx, script_uri)?;
    }

    // Setup a minimal register function for testing
    let register = Function::new(
        ctx.clone(),
        move |_ctx: rquickjs::Ctx<'_>,
              path: String,
              handler: String,
              method: Option<String>|
              -> JsResult<String> {
            debug!(
                path = %path,
                handler = %handler,
                method = ?method,
                "Secure register called"
            );

            // For testing, just return success
            Ok(format!("Route '{}' registered successfully", path))
        },
    )?;
    let global = ctx.globals();
    global.set("register", register)?;

    Ok(())
}
