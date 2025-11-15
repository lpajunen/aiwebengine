use base64::Engine;
use rquickjs::{Function, Result as JsResult};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::repository;
use crate::secrets::SecretsManager;
use crate::security::{
    SecureOperations, SecurityAuditor, SecurityEventType, SecuritySeverity, UserContext,
};

// Type alias for route registration callback function
type RouteRegisterFn = Box<dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>>;

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
            enable_logging: true,
            enable_secrets: true,
            enforce_strict_validation: true,
            enable_audit_logging: true, // Enable by default
        }
    }
}

impl SecureGlobalContext {
    pub fn new(user_context: UserContext) -> Self {
        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(),
            config: GlobalSecurityConfig::default(),
            secrets_manager: None,
        }
    }

    pub fn new_with_config(user_context: UserContext, config: GlobalSecurityConfig) -> Self {
        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(),
            config,
            secrets_manager: None,
        }
    }

    pub fn new_with_secrets(
        user_context: UserContext,
        config: GlobalSecurityConfig,
        secrets_manager: Arc<SecretsManager>,
    ) -> Self {
        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(),
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

        // Setup script storage functions
        self.setup_shared_storage_functions(ctx, script_uri)?;

        // Always setup GraphQL functions, but they will be no-ops if disabled
        self.setup_graphql_functions(ctx, script_uri)?;

        // Setup user management functions (admin-only)
        self.setup_user_management_functions(ctx, script_uri)?;

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
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Vec<String>> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadScripts)
                {
                    // Return empty array if no permission (JavaScript expects an array)
                    return Ok(Vec::new());
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listScripts called"
                );

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

                // Store the script using repository
                if let Err(e) = repository::upsert_script(&script_name, &js_script) {
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
                    // Clear any existing GraphQL registrations from this script before re-initializing
                    crate::graphql::clear_script_graphql_registrations(&script_name_for_init);

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

        // Create assetStorage object
        let asset_storage = rquickjs::Object::new(ctx.clone())?;

        // Secure listAssets function
        let user_ctx_list = user_context.clone();
        let list_assets = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Vec<String>> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadAssets)
                {
                    // Return empty array if no permission (JavaScript expects an array)
                    return Ok(Vec::new());
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listAssets called"
                );

                let assets = repository::fetch_assets();
                let asset_names: Vec<String> = assets.keys().cloned().collect();
                Ok(asset_names)
            },
        )?;
        asset_storage.set("listAssets", list_assets)?;

        // Secure fetchAsset function
        let user_ctx_fetch = user_context.clone();
        let fetch_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, asset_name: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_fetch.require_capability(&crate::security::Capability::ReadAssets)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_fetch.user_id,
                    asset_name = %asset_name,
                    "Secure fetchAsset called"
                );

                match repository::fetch_asset(&asset_name) {
                    Some(asset) => {
                        // Convert bytes to base64 for safe JavaScript transfer
                        Ok(base64::engine::general_purpose::STANDARD.encode(asset.content))
                    }
                    None => Ok(format!("Asset '{}' not found", asset_name)),
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
                  asset_name: String,
                  content_b64: String,
                  mimetype: String|
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

                // Validate asset name (inline validation since we can't call async)
                if asset_name.is_empty() || asset_name.len() > 255 {
                    return Ok("Invalid asset name: must be 1-255 characters".to_string());
                }
                if asset_name.contains("..") || asset_name.contains('\\') {
                    return Ok("Invalid asset name: path traversal not allowed".to_string());
                }

                // Validate content size (10MB limit)
                if content.len() > 10 * 1024 * 1024 {
                    return Ok("Asset too large (max 10MB)".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_asset.clone();
                let user_id = user_ctx_upsert_asset.user_id.clone();
                let asset_name_clone = asset_name.clone();
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
                            .with_detail("asset_name", &asset_name_clone)
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("content_size", content_len.to_string())
                            .with_detail("mimetype", &mimetype_clone),
                        )
                        .await;
                });

                // Call repository directly (sync operation)
                let asset = repository::Asset {
                    asset_name: asset_name.clone(),
                    mimetype,
                    content,
                };
                match repository::upsert_asset(asset) {
                    Ok(_) => Ok(format!("Asset '{}' upserted successfully", asset_name)),
                    Err(e) => Ok(format!("Error upserting asset: {}", e)),
                }
            },
        )?;
        asset_storage.set("upsertAsset", upsert_asset)?;

        // Secure deleteAsset function
        let user_ctx_delete_asset = user_context.clone();
        let auditor_delete_asset = auditor.clone();
        let delete_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, asset_name: String| -> JsResult<String> {
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
                            .with_action("delete".to_string())
                            .with_detail("asset_name", &asset_name_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_delete_asset.user_id,
                    asset_name = %asset_name,
                    "Secure deleteAsset called"
                );

                match repository::delete_asset(&asset_name) {
                    true => Ok(format!("Asset '{}' deleted successfully", asset_name)),
                    false => Ok(format!("Asset '{}' not found", asset_name)),
                }
            },
        )?;
        asset_storage.set("deleteAsset", delete_asset)?;

        // Set the assetStorage object on the global scope
        global.set("assetStorage", asset_storage)?;
        Ok(())
    }

    /// Setup secure secrets functions
    ///
    /// Exposes a read-only JavaScript API for secrets management:
    /// - Secrets.exists(identifier): boolean - Check if a secret exists
    /// - Secrets.list(): string[] - List all secret identifiers
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

        // Create the Secrets namespace object
        let secrets_obj = rquickjs::Object::new(ctx.clone())?;

        // Secrets.exists(identifier) - Check if a secret exists
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
        secrets_obj.set("exists", exists_fn)?;

        // Secrets.list() - List all secret identifiers
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
        secrets_obj.set("list", list_fn)?;

        // Set the Secrets object on the global scope
        global.set("Secrets", secrets_obj)?;

        debug!("Secrets JavaScript API initialized (read-only interface)");

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
                  resolver_function: String|
                  -> JsResult<String> {
                // If GraphQL registration is disabled, return success without doing anything
                tracing::info!(
                    "registerGraphQLQuery called: name={}, enable_graphql_registration={}",
                    name,
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
                            .with_detail("sdl_length", sdl_len.to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_query.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    "Secure registerGraphQLQuery called"
                );

                // Actually register the GraphQL query
                crate::graphql::register_graphql_query(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_query.clone(),
                );
                Ok(format!("GraphQL query '{}' registered successfully", name))
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
                  resolver_function: String|
                  -> JsResult<String> {
                // If GraphQL registration is disabled, return success without doing anything
                debug!(
                    "registerGraphQLMutation called: name={}, enable_graphql_registration={}",
                    name, config_mutation.enable_graphql_registration
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
                            .with_detail("sdl_length", sdl_len.to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_mutation.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    "Secure registerGraphQLMutation called"
                );

                // Actually register the GraphQL mutation
                crate::graphql::register_graphql_mutation(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_mutation.clone(),
                );
                Ok(format!(
                    "GraphQL mutation '{}' registered successfully",
                    name
                ))
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
                  resolver_function: String|
                  -> JsResult<String> {
                // If GraphQL registration is disabled, return success without doing anything
                debug!(
                    "registerGraphQLSubscription called: name={}, enable_graphql_registration={}",
                    name, config_subscription.enable_graphql_registration
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
                            .with_detail("sdl_length", sdl_len.to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_subscription.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    "Secure registerGraphQLSubscription called"
                );

                // Actually register the GraphQL subscription
                crate::graphql::register_graphql_subscription(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_subscription.clone(),
                );
                Ok(format!(
                    "GraphQL subscription '{}' registered successfully",
                    name
                ))
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
                move |_c: rquickjs::Ctx<'_>,
                      path: String,
                      handler: String,
                      method: Option<String>|
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

                    let method_ref = method.as_deref();
                    register_impl(&path, &handler, method_ref)
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
                 _m: Option<String>|
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
            move |_ctx: rquickjs::Ctx<'_>, path: String| -> JsResult<String> {
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
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .register_stream(&path, &script_uri_stream)
                {
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
                                for ((path, method), handler) in metadata.registrations {
                                    all_routes.push(serde_json::json!({
                                        "path": path,
                                        "method": method,
                                        "handler": handler,
                                        "script_uri": metadata.uri,
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

                let assets = repository::fetch_assets();
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
    fn setup_database_functions(&self, ctx: &rquickjs::Ctx<'_>, _script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();

        // Create the checkDatabaseHealth function
        let check_db_health = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Call the database health check
                let result = crate::database::Database::check_health_sync();
                Ok(result)
            },
        )?;

        global.set("checkDatabaseHealth", check_db_health)?;
        debug!("checkDatabaseHealth() function initialized");

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
        global.set("listUsers", list_users)?;

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
        global.set("addUserRole", add_user_role)?;

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
        global.set("removeUserRole", remove_user_role)?;

        debug!("User management functions initialized (admin-only)");
        Ok(())
    }
}
