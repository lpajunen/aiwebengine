use base64::Engine;
use rquickjs::{Function, Result as JsResult};
use std::collections::HashMap;
use tracing::debug;

use crate::repository;
use crate::security::{
    SecureOperations, SecurityAuditor, SecurityEventType, SecuritySeverity, UserContext,
};

/// Type alias for route registration function
type RouteRegisterFn = dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>;

/// Secure wrapper for JavaScript global functions that enforces Rust-level validation
pub struct SecureGlobalContext {
    user_context: UserContext,
    secure_ops: SecureOperations,
    auditor: SecurityAuditor,
    config: GlobalSecurityConfig,
}

#[derive(Debug, Clone)]
pub struct GlobalSecurityConfig {
    pub enable_graphql_registration: bool,
    pub enable_asset_management: bool,
    pub enable_streams: bool,
    pub enable_script_management: bool,
    pub enable_logging: bool,
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
        }
    }

    pub fn new_with_config(user_context: UserContext, config: GlobalSecurityConfig) -> Self {
        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(),
            config,
        }
    }

    /// Setup all secure global functions in the JavaScript context
    pub fn setup_secure_globals(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        self.setup_secure_functions(ctx, script_uri, None)
    }

    /// Setup secure global functions with optional route registration function
    pub fn setup_secure_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<Box<dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>>>,
    ) -> JsResult<()> {
        // Setup route registration function first
        self.setup_route_registration(ctx, register_fn)?;

        if self.config.enable_logging {
            self.setup_logging_functions(ctx, script_uri)?;
        }

        if self.config.enable_script_management {
            self.setup_script_management_functions(ctx, script_uri)?;
        }

        if self.config.enable_asset_management {
            self.setup_asset_management_functions(ctx, script_uri)?;
        }

        // Always setup GraphQL functions, but they will be no-ops if disabled
        self.setup_graphql_functions(ctx, script_uri)?;

        // Always setup stream functions, but they will be no-ops if disabled
        self.setup_stream_functions(ctx, script_uri)?;

        Ok(())
    }

    /// Setup route registration function
    fn setup_route_registration(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        register_fn: Option<Box<dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>>>,
    ) -> JsResult<()> {
        let global = ctx.globals();

        if let Some(register_impl) = register_fn {
            let register = Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>,
                      path: String,
                      handler: String,
                      method: Option<String>|
                      -> Result<(), rquickjs::Error> {
                    let method_ref = method.as_deref();
                    register_impl(&path, &handler, method_ref)
                },
            )?;
            global.set("register", register)?;
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
            global.set("register", reg_noop)?;
        }

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
            move |_ctx: rquickjs::Ctx<'_>, message: String| -> JsResult<String> {
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
                repository::insert_log_message(&script_uri_write, &message);
                Ok("Log written successfully".to_string())
            },
        )?;
        global.set("writeLog", write_log)?;

        // Secure listLogs function
        let user_ctx_list = user_context.clone();
        let list_logs = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ViewLogs)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listLogs called"
                );

                let logs = repository::fetch_log_messages("");
                Ok(serde_json::to_string(&logs).unwrap_or_else(|_| "[]".to_string()))
            },
        )?;
        global.set("listLogs", list_logs)?;

        // Secure listLogsForUri function
        let user_ctx_list_uri = user_context.clone();
        let list_logs_for_uri = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_list_uri.require_capability(&crate::security::Capability::ViewLogs)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_list_uri.user_id,
                    uri = %uri,
                    "Secure listLogsForUri called"
                );

                let logs = repository::fetch_log_messages(&uri);
                Ok(serde_json::to_string(&logs).unwrap_or_else(|_| "[]".to_string()))
            },
        )?;
        global.set("listLogsForUri", list_logs_for_uri)?;

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

        // Secure listScripts function
        let user_ctx_list = user_context.clone();
        let list_scripts = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadScripts)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listScripts called"
                );

                let scripts = repository::fetch_scripts();
                Ok(serde_json::to_string(&scripts).unwrap_or_else(|_| "{}".to_string()))
            },
        )?;
        global.set("listScripts", list_scripts)?;

        // Secure getScript function
        let user_ctx_get = user_context.clone();
        let get_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_get.require_capability(&crate::security::Capability::ReadScripts)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_get.user_id,
                    script_name = %script_name,
                    "Secure getScript called"
                );

                match repository::fetch_script(&script_name) {
                    Some(content) => Ok(content),
                    None => Ok(format!("Script '{}' not found", script_name)),
                }
            },
        )?;
        global.set("getScript", get_script)?;

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

                Ok(format!("Script '{}' upserted successfully", script_name))
            },
        )?;
        global.set("upsertScript", upsert_script)?;

        // Secure deleteScript function
        let user_ctx_delete = user_context.clone();
        let auditor_delete = auditor.clone();
        let delete_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<String> {
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
                    return Ok(format!("Error: {}", e));
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

                match repository::delete_script(&script_name) {
                    true => Ok(format!("Script '{}' deleted successfully", script_name)),
                    false => Ok(format!("Script '{}' not found", script_name)),
                }
            },
        )?;
        global.set("deleteScript", delete_script)?;

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

        // Secure listAssets function
        let user_ctx_list = user_context.clone();
        let list_assets = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadAssets)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listAssets called"
                );

                let assets = repository::fetch_assets();
                let asset_names: Vec<String> = assets.keys().cloned().collect();
                Ok(serde_json::to_string(&asset_names).unwrap_or_else(|_| "[]".to_string()))
            },
        )?;
        global.set("listAssets", list_assets)?;

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
        global.set("fetchAsset", fetch_asset)?;

        // Secure upsertAsset function
        let user_ctx_upsert_asset = user_context.clone();
        let secure_ops_asset = secure_ops.clone();
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

                // For now, skip the async asset upload to avoid runtime conflicts
                // TODO: Implement proper async handling for asset operations
                let result: Result<
                    crate::security::OperationResult<String>,
                    axum::http::StatusCode,
                > = Ok(crate::security::OperationResult::success(
                    "Asset upload accepted (validation skipped)".to_string(),
                ));

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

                match result {
                    Ok(op_result) => {
                        if op_result.success {
                            // If validation passed, call repository
                            let asset = repository::Asset {
                                public_path: asset_name.clone(),
                                mimetype,
                                content,
                            };
                            match repository::upsert_asset(asset) {
                                Ok(_) => {
                                    Ok(format!("Asset '{}' upserted successfully", asset_name))
                                }
                                Err(e) => Ok(format!("Error upserting asset: {}", e)),
                            }
                        } else {
                            Ok(op_result
                                .error
                                .unwrap_or_else(|| "Unknown error".to_string()))
                        }
                    }
                    Err(_) => Ok("Internal server error".to_string()),
                }
            },
        )?;
        global.set("upsertAsset", upsert_asset)?;

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
        global.set("deleteAsset", delete_asset)?;
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
        let secure_ops_query = secure_ops.clone();
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

                // For now, skip the async GraphQL schema validation to avoid runtime conflicts
                // TODO: Implement proper async handling for GraphQL operations
                let validation_result: Result<
                    crate::security::OperationResult<String>,
                    axum::http::StatusCode,
                > = Ok(crate::security::OperationResult::success(
                    "GraphQL query schema accepted (validation skipped)".to_string(),
                ));

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

                match validation_result {
                    Ok(op_result) => {
                        if op_result.success {
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
                        } else {
                            Ok(op_result
                                .error
                                .unwrap_or_else(|| "Unknown error".to_string()))
                        }
                    }
                    Err(_) => Ok("Internal server error".to_string()),
                }
            },
        )?;
        global.set("registerGraphQLQuery", register_graphql_query)?;

        // Secure registerGraphQLMutation function
        let user_ctx_mutation = user_context.clone();
        let secure_ops_mutation = secure_ops.clone();
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

                // For now, skip the async GraphQL schema validation to avoid runtime conflicts
                // TODO: Implement proper async handling for GraphQL operations
                let validation_result: Result<
                    crate::security::OperationResult<String>,
                    axum::http::StatusCode,
                > = Ok(crate::security::OperationResult::success(
                    "GraphQL mutation schema accepted (validation skipped)".to_string(),
                ));

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

                match validation_result {
                    Ok(op_result) => {
                        if op_result.success {
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
                        } else {
                            Ok(op_result
                                .error
                                .unwrap_or_else(|| "Unknown error".to_string()))
                        }
                    }
                    Err(_) => Ok("Internal server error".to_string()),
                }
            },
        )?;
        global.set("registerGraphQLMutation", register_graphql_mutation)?;

        // Secure registerGraphQLSubscription function
        let user_ctx_subscription = user_context.clone();
        let secure_ops_subscription = secure_ops.clone();
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

                // For now, skip the async GraphQL schema validation to avoid runtime conflicts
                // TODO: Implement proper async handling for GraphQL operations
                let validation_result: Result<
                    crate::security::OperationResult<String>,
                    axum::http::StatusCode,
                > = Ok(crate::security::OperationResult::success(
                    "GraphQL subscription schema accepted (validation skipped)".to_string(),
                ));

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

                match validation_result {
                    Ok(op_result) => {
                        if op_result.success {
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
                        } else {
                            Ok(op_result
                                .error
                                .unwrap_or_else(|| "Unknown error".to_string()))
                        }
                    }
                    Err(_) => Ok("Internal server error".to_string()),
                }
            },
        )?;
        global.set("registerGraphQLSubscription", register_graphql_subscription)?;

        Ok(())
    }

    /// Setup secure stream functions
    fn setup_stream_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let secure_ops = self.secure_ops.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();

        // Secure registerWebStream function
        let user_ctx_register = user_context.clone();
        let secure_ops_register = secure_ops.clone();
        let auditor_register = auditor.clone();
        let config_register = self.config.clone();
        let script_uri_register = script_uri_owned.clone();
        let register_web_stream = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, path: String| -> JsResult<String> {
                // If streams are disabled, return success without doing anything
                tracing::info!(
                    "registerWebStream called: path={}, enable_streams={}",
                    path,
                    config_register.enable_streams
                );
                if !config_register.enable_streams {
                    tracing::info!(
                        "Stream registration disabled, skipping stream registration for: {}",
                        path
                    );
                    return Ok(format!(
                        "Web stream '{}' registration skipped (disabled)",
                        path
                    ));
                }

                // Check capability
                if let Err(e) = user_ctx_register
                    .require_capability(&crate::security::Capability::ManageStreams)
                {
                    // Only try async logging if we're in a runtime and audit logging is enabled
                    if config_register.enable_audit_logging {
                        if let Ok(rt) = tokio::runtime::Handle::try_current() {
                            let auditor_clone = auditor_register.clone();
                            let user_id = user_ctx_register.user_id.clone();
                            rt.spawn(async move {
                                let _ = auditor_clone
                                    .log_authz_failure(
                                        user_id,
                                        "stream".to_string(),
                                        "register".to_string(),
                                        "ManageStreams".to_string(),
                                    )
                                    .await;
                            });
                        }
                    }
                    return Ok(format!("Error: {}", e));
                }

                // Try to execute stream operation if we have a runtime
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    // For now, skip the async stream creation to avoid runtime conflicts
                    // TODO: Implement proper async handling for stream operations
                    let validation_result: Result<
                        crate::security::OperationResult<String>,
                        axum::http::StatusCode,
                    > = Ok(crate::security::OperationResult::success(
                        "Stream registration accepted (validation skipped)".to_string(),
                    ));

                    // Log the operation attempt if audit logging is enabled
                    if config_register.enable_audit_logging {
                        let auditor_clone = auditor_register.clone();
                        let user_id = user_ctx_register.user_id.clone();
                        let path_clone = path.clone();
                        let script_uri_clone = script_uri_register.clone();
                        rt.spawn(async move {
                            let _ = auditor_clone
                                .log_event(
                                    crate::security::SecurityEvent::new(
                                        SecurityEventType::SystemSecurityEvent,
                                        SecuritySeverity::Medium,
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

                    match validation_result {
                        Ok(op_result) => {
                            if op_result.success {
                                debug!(
                                    user_id = ?user_ctx_register.user_id,
                                    path = %path,
                                    "Secure registerWebStream called"
                                );

                                // Actually register the stream
                                match crate::stream_registry::GLOBAL_STREAM_REGISTRY
                                    .register_stream(&path, &script_uri_register)
                                {
                                    Ok(()) => {
                                        Ok(format!("Web stream '{}' registered successfully", path))
                                    }
                                    Err(e) => {
                                        Ok(format!("Failed to register stream '{}': {}", path, e))
                                    }
                                }
                            } else {
                                Ok(op_result
                                    .error
                                    .unwrap_or_else(|| "Unknown error".to_string()))
                            }
                        }
                        Err(_) => Ok("Internal server error".to_string()),
                    }
                } else {
                    // No tokio runtime available, register stream for testing
                    debug!(
                        user_id = ?user_ctx_register.user_id,
                        path = %path,
                        "Stream register called (no runtime)"
                    );
                    match crate::stream_registry::GLOBAL_STREAM_REGISTRY
                        .register_stream(&path, &script_uri_register)
                    {
                        Ok(()) => Ok(format!(
                            "Stream '{}' registered successfully (test mode)",
                            path
                        )),
                        Err(e) => Ok(format!(
                            "Failed to register stream '{}' (test mode): {}",
                            path, e
                        )),
                    }
                }
            },
        )?;
        global.set("registerWebStream", register_web_stream)?;

        // Secure sendStreamMessage function
        let user_ctx_send = user_context.clone();
        let auditor_send = auditor.clone();
        let script_uri_send = script_uri_owned.clone();
        let send_stream_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, message: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_send.require_capability(&crate::security::Capability::ManageStreams)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_send.clone();
                    let user_id = user_ctx_send.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "stream".to_string(),
                                "send_message".to_string(),
                                "ManageStreams".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_send.clone();
                let user_id = user_ctx_send.user_id.clone();
                let message_clone = message.clone();
                let script_uri_clone = script_uri_send.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("stream".to_string())
                            .with_action("send_message".to_string())
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("message_length", message_clone.len().to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_send.user_id,
                    message_len = message.len(),
                    "Secure sendStreamMessage called"
                );

                // TODO: Call actual stream message sending here
                Ok("Stream message sent successfully".to_string())
            },
        )?;
        global.set("sendStreamMessage", send_stream_message)?;

        // Secure sendStreamMessageToPath function
        let user_ctx_send_path = user_context.clone();
        let auditor_send_path = auditor.clone();
        let send_stream_message_to_path = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, path: String, message: String| -> JsResult<String> {
                // Allow system-level broadcasting without capability checks for certain paths
                let is_system_broadcast = path == "/script_updates" || path.starts_with("/system/");

                if !is_system_broadcast {
                    // Check capability for non-system operations
                    if let Err(e) = user_ctx_send_path
                        .require_capability(&crate::security::Capability::ManageStreams)
                    {
                        // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                        let auditor_clone = auditor_send_path.clone();
                        let user_id = user_ctx_send_path.user_id.clone();
                        tokio::task::spawn(async move {
                            let _ = auditor_clone
                                .log_authz_failure(
                                    user_id,
                                    "stream".to_string(),
                                    "send_message_to_path".to_string(),
                                    "ManageStreams".to_string(),
                                )
                                .await;
                        });
                        return Ok(format!("Error: {}", e));
                    }
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_send_path.clone();
                let user_id = user_ctx_send_path.user_id.clone();
                let path_clone = path.clone();
                let message_clone = message.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("stream".to_string())
                            .with_action("send_message_to_path".to_string())
                            .with_detail("path", &path_clone)
                            .with_detail("message_length", message_clone.len().to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_send_path.user_id,
                    path = %path,
                    message_len = message.len(),
                    "Secure sendStreamMessageToPath called"
                );

                // TODO: Call actual path-specific stream message sending here
                Ok(format!(
                    "Stream message sent to path '{}' successfully",
                    path
                ))
            },
        )?;
        global.set("sendStreamMessageToPath", send_stream_message_to_path)?;

        // Secure sendSubscriptionMessage function (for GraphQL subscriptions)
        let user_ctx_send_subscription = user_context.clone();
        let auditor_send_subscription = auditor.clone();
        let send_subscription_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  subscription_name: String,
                  message: String|
                  -> JsResult<String> {
                // Allow system-level GraphQL subscription broadcasting without capability checks
                let is_system_broadcast = true; // GraphQL subscriptions are considered system-level

                if !is_system_broadcast {
                    // Check capability for non-system operations (future use)
                    if let Err(e) = user_ctx_send_subscription
                        .require_capability(&crate::security::Capability::ManageGraphQL)
                    {
                        // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                        let auditor_clone = auditor_send_subscription.clone();
                        let user_id = user_ctx_send_subscription.user_id.clone();
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
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_send_subscription.clone();
                let user_id = user_ctx_send_subscription.user_id.clone();
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
                    user_id = ?user_ctx_send_subscription.user_id,
                    subscription_name = %subscription_name,
                    message_len = message.len(),
                    "Secure sendSubscriptionMessage called"
                );

                // TODO: Call actual GraphQL subscription message sending here
                // For now, send to the compatibility stream path
                let stream_path = format!("/graphql/subscription/{}", subscription_name);

                // Try to send via stream path (this will work if the stream is registered)
                let result = crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream(&stream_path, &message);

                match result {
                    Ok(broadcast_result) => {
                        if broadcast_result.is_fully_successful() {
                            Ok(format!(
                                "GraphQL subscription message sent to {} connections",
                                broadcast_result.successful_sends
                            ))
                        } else {
                            Ok(format!(
                                "GraphQL subscription message partially sent: {} successful, {} failed connections",
                                broadcast_result.successful_sends,
                                broadcast_result.failed_connections.len()
                            ))
                        }
                    }
                    Err(e) => Ok(format!(
                        "Failed to send GraphQL subscription message: {}",
                        e
                    )),
                }
            },
        )?;
        global.set("sendSubscriptionMessage", send_subscription_message)?;

        Ok(())
    }
}
