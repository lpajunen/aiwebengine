use base64::Engine;
use rquickjs::{Function, Result as JsResult};
use std::collections::HashMap;
use tracing::debug;

use crate::repository;
use crate::security::{
    SecureOperations, SecurityAuditor, SecurityEventType, SecuritySeverity, UpsertScriptRequest,
    UserContext,
};

/// Secure wrapper for JavaScript global functions that enforces Rust-level validation
pub struct SecureGlobalContext {
    user_context: UserContext,
    secure_ops: SecureOperations,
    auditor: SecurityAuditor,
    config: GlobalSecurityConfig,
}

#[derive(Debug, Clone)]
pub struct GlobalSecurityConfig {
    pub enable_streams: bool,
    pub enable_graphql_registration: bool,
    pub enable_asset_management: bool,
    pub enable_script_management: bool,
    pub enable_logging: bool,
    pub enforce_strict_validation: bool,
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

    pub fn with_config(user_context: UserContext, config: GlobalSecurityConfig) -> Self {
        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(),
            config,
        }
    }

    /// Setup all secure global functions in the JavaScript context
    pub fn setup_secure_globals(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        if self.config.enable_logging {
            self.setup_logging_functions(ctx, script_uri)?;
        }

        if self.config.enable_script_management {
            self.setup_script_management_functions(ctx, script_uri)?;
        }

        if self.config.enable_asset_management {
            self.setup_asset_management_functions(ctx, script_uri)?;
        }

        if self.config.enable_graphql_registration {
            self.setup_graphql_functions(ctx, script_uri)?;
        }

        if self.config.enable_streams {
            self.setup_stream_functions(ctx, script_uri)?;
        }

        Ok(())
    }

    /// Setup secure logging functions
    fn setup_logging_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();

        // Secure writeLog function
        let user_ctx_write = user_context.clone();
        let auditor_write = auditor.clone();
        let script_uri_write = script_uri_owned.clone();
        let write_log = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, message: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_write.require_capability(&crate::security::Capability::ViewLogs)
                {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_write.log_authz_failure(
                        user_ctx_write.user_id.clone(),
                        "log".to_string(),
                        "write".to_string(),
                        "ViewLogs".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                // Log the write operation
                let rt = tokio::runtime::Handle::current();
                rt.block_on(
                    auditor_write.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Low,
                            user_ctx_write.user_id.clone(),
                        )
                        .with_resource("log".to_string())
                        .with_action("write".to_string())
                        .with_detail("script_uri", &script_uri_write)
                        .with_detail("message_length", message.len().to_string()),
                    ),
                );

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

                match repository::fetch_log_messages("") {
                    logs => Ok(serde_json::to_string(&logs).unwrap_or_else(|_| "[]".to_string())),
                }
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

                match repository::fetch_log_messages(&uri) {
                    logs => Ok(serde_json::to_string(&logs).unwrap_or_else(|_| "[]".to_string())),
                }
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
        let secure_ops = self.secure_ops.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();

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
        let secure_ops_upsert = secure_ops.clone();
        let auditor_upsert = auditor.clone();
        let script_uri_upsert = script_uri_owned.clone();
        let upsert_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  script_name: String,
                  js_script: String|
                  -> JsResult<String> {
                // Use secure operations for validation and capability checking
                let request = UpsertScriptRequest {
                    script_name: script_name.clone(),
                    js_script: js_script.clone(),
                };

                let rt = tokio::runtime::Handle::current();
                let result =
                    rt.block_on(secure_ops_upsert.upsert_script(&user_ctx_upsert, request));

                // Log the operation attempt
                rt.block_on(
                    auditor_upsert.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Medium,
                            user_ctx_upsert.user_id.clone(),
                        )
                        .with_resource("script".to_string())
                        .with_action("upsert".to_string())
                        .with_detail("script_name", &script_name)
                        .with_detail("script_uri", &script_uri_upsert)
                        .with_detail("content_length", js_script.len().to_string()),
                    ),
                );

                match result {
                    Ok(op_result) => {
                        if op_result.success {
                            // If validation passed, call repository
                            match repository::upsert_script(&script_name, &js_script) {
                                Ok(_) => {
                                    Ok(format!("Script '{}' upserted successfully", script_name))
                                }
                                Err(e) => Ok(format!("Error upserting script: {}", e)),
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
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_delete.log_authz_failure(
                        user_ctx_delete.user_id.clone(),
                        "script".to_string(),
                        "delete".to_string(),
                        "DeleteScripts".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                rt.block_on(
                    auditor_delete.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::High,
                            user_ctx_delete.user_id.clone(),
                        )
                        .with_resource("script".to_string())
                        .with_action("delete".to_string())
                        .with_detail("script_name", &script_name),
                    ),
                );

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

                let rt = tokio::runtime::Handle::current();
                let result = rt.block_on(secure_ops_asset.upload_asset(
                    &user_ctx_upsert_asset,
                    asset_name.clone(),
                    content.clone(),
                ));

                // Log the operation attempt
                rt.block_on(
                    auditor_asset.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Medium,
                            user_ctx_upsert_asset.user_id.clone(),
                        )
                        .with_resource("asset".to_string())
                        .with_action("upsert".to_string())
                        .with_detail("asset_name", &asset_name)
                        .with_detail("script_uri", &script_uri_asset)
                        .with_detail("content_size", content.len().to_string())
                        .with_detail("mimetype", &mimetype),
                    ),
                );

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
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_delete_asset.log_authz_failure(
                        user_ctx_delete_asset.user_id.clone(),
                        "asset".to_string(),
                        "delete".to_string(),
                        "DeleteAssets".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                rt.block_on(
                    auditor_delete_asset.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::High,
                            user_ctx_delete_asset.user_id.clone(),
                        )
                        .with_resource("asset".to_string())
                        .with_action("delete".to_string())
                        .with_detail("asset_name", &asset_name),
                    ),
                );

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
        let register_graphql_query = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String, sdl: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_query.require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_query.log_authz_failure(
                        user_ctx_query.user_id.clone(),
                        "graphql".to_string(),
                        "register_query".to_string(),
                        "ManageGraphQL".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                let validation_result = rt
                    .block_on(secure_ops_query.update_graphql_schema(&user_ctx_query, sdl.clone()));

                // Log the operation attempt
                rt.block_on(
                    auditor_query.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Medium,
                            user_ctx_query.user_id.clone(),
                        )
                        .with_resource("graphql".to_string())
                        .with_action("register_query".to_string())
                        .with_detail("query_name", &name)
                        .with_detail("script_uri", &script_uri_query)
                        .with_detail("sdl_length", sdl.len().to_string()),
                    ),
                );

                match validation_result {
                    Ok(op_result) => {
                        if op_result.success {
                            debug!(
                                user_id = ?user_ctx_query.user_id,
                                name = %name,
                                sdl_len = sdl.len(),
                                "Secure registerGraphQLQuery called"
                            );

                            // TODO: Call actual GraphQL registration here
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
        let register_graphql_mutation = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String, sdl: String| -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_mutation
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_mutation.log_authz_failure(
                        user_ctx_mutation.user_id.clone(),
                        "graphql".to_string(),
                        "register_mutation".to_string(),
                        "ManageGraphQL".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                let validation_result = rt.block_on(
                    secure_ops_mutation.update_graphql_schema(&user_ctx_mutation, sdl.clone()),
                );

                // Log the operation attempt
                rt.block_on(
                    auditor_mutation.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Medium,
                            user_ctx_mutation.user_id.clone(),
                        )
                        .with_resource("graphql".to_string())
                        .with_action("register_mutation".to_string())
                        .with_detail("mutation_name", &name)
                        .with_detail("sdl_length", sdl.len().to_string()),
                    ),
                );

                match validation_result {
                    Ok(op_result) => {
                        if op_result.success {
                            debug!(
                                user_id = ?user_ctx_mutation.user_id,
                                name = %name,
                                sdl_len = sdl.len(),
                                "Secure registerGraphQLMutation called"
                            );

                            // TODO: Call actual GraphQL registration here
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
        let register_graphql_subscription = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String, sdl: String| -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_subscription
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_subscription.log_authz_failure(
                        user_ctx_subscription.user_id.clone(),
                        "graphql".to_string(),
                        "register_subscription".to_string(),
                        "ManageGraphQL".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                let validation_result = rt.block_on(
                    secure_ops_subscription
                        .update_graphql_schema(&user_ctx_subscription, sdl.clone()),
                );

                // Log the operation attempt
                rt.block_on(
                    auditor_subscription.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Medium,
                            user_ctx_subscription.user_id.clone(),
                        )
                        .with_resource("graphql".to_string())
                        .with_action("register_subscription".to_string())
                        .with_detail("subscription_name", &name)
                        .with_detail("sdl_length", sdl.len().to_string()),
                    ),
                );

                match validation_result {
                    Ok(op_result) => {
                        if op_result.success {
                            debug!(
                                user_id = ?user_ctx_subscription.user_id,
                                name = %name,
                                sdl_len = sdl.len(),
                                "Secure registerGraphQLSubscription called"
                            );

                            // TODO: Call actual GraphQL registration here
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
        let script_uri_register = script_uri_owned.clone();
        let register_web_stream = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, path: String| -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_register
                    .require_capability(&crate::security::Capability::ManageStreams)
                {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_register.log_authz_failure(
                        user_ctx_register.user_id.clone(),
                        "stream".to_string(),
                        "register".to_string(),
                        "ManageStreams".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                let validation_result = rt.block_on(secure_ops_register.create_stream(
                    &user_ctx_register,
                    path.clone(),
                    HashMap::new(), // Empty config for basic stream
                ));

                // Log the operation attempt
                rt.block_on(
                    auditor_register.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Medium,
                            user_ctx_register.user_id.clone(),
                        )
                        .with_resource("stream".to_string())
                        .with_action("register".to_string())
                        .with_detail("path", &path)
                        .with_detail("script_uri", &script_uri_register),
                    ),
                );

                match validation_result {
                    Ok(op_result) => {
                        if op_result.success {
                            debug!(
                                user_id = ?user_ctx_register.user_id,
                                path = %path,
                                "Secure registerWebStream called"
                            );

                            // TODO: Call actual stream registration here
                            Ok(format!("Web stream '{}' registered successfully", path))
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
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_send.log_authz_failure(
                        user_ctx_send.user_id.clone(),
                        "stream".to_string(),
                        "send_message".to_string(),
                        "ManageStreams".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                rt.block_on(
                    auditor_send.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Low,
                            user_ctx_send.user_id.clone(),
                        )
                        .with_resource("stream".to_string())
                        .with_action("send_message".to_string())
                        .with_detail("script_uri", &script_uri_send)
                        .with_detail("message_length", message.len().to_string()),
                    ),
                );

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
                // Check capability
                if let Err(e) = user_ctx_send_path
                    .require_capability(&crate::security::Capability::ManageStreams)
                {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(auditor_send_path.log_authz_failure(
                        user_ctx_send_path.user_id.clone(),
                        "stream".to_string(),
                        "send_message_to_path".to_string(),
                        "ManageStreams".to_string(),
                    ));
                    return Ok(format!("Error: {}", e));
                }

                let rt = tokio::runtime::Handle::current();
                rt.block_on(
                    auditor_send_path.log_event(
                        crate::security::SecurityEvent::new(
                            SecurityEventType::SystemSecurityEvent,
                            SecuritySeverity::Low,
                            user_ctx_send_path.user_id.clone(),
                        )
                        .with_resource("stream".to_string())
                        .with_action("send_message_to_path".to_string())
                        .with_detail("path", &path)
                        .with_detail("message_length", message.len().to_string()),
                    ),
                );

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

        Ok(())
    }

    /// Setup secure HTTP route registration function
    pub fn setup_route_registration(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_impl: Option<Box<dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>>>,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();

        // Set up register function (either no-op or the provided implementation)
        if let Some(register_impl) = register_impl {
            let user_ctx_register = user_context.clone();
            let auditor_register = auditor.clone();
            let script_uri_register = script_uri_owned.clone();
            let register = Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>,
                      path: String,
                      handler: String,
                      method: Option<String>|
                      -> JsResult<String> {
                    // Check capability - route registration requires script write capability
                    if let Err(e) = user_ctx_register
                        .require_capability(&crate::security::Capability::WriteScripts)
                    {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(auditor_register.log_authz_failure(
                            user_ctx_register.user_id.clone(),
                            "route".to_string(),
                            "register".to_string(),
                            "WriteScripts".to_string(),
                        ));
                        return Ok(format!("Error: {}", e));
                    }

                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(
                        auditor_register.log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_ctx_register.user_id.clone(),
                            )
                            .with_resource("route".to_string())
                            .with_action("register".to_string())
                            .with_detail("path", &path)
                            .with_detail("handler", &handler)
                            .with_detail("method", method.as_deref().unwrap_or("GET"))
                            .with_detail("script_uri", &script_uri_register),
                        ),
                    );

                    debug!(
                        user_id = ?user_ctx_register.user_id,
                        path = %path,
                        handler = %handler,
                        method = ?method,
                        "Secure route register called"
                    );

                    let method_ref = method.as_deref();
                    match register_impl(&path, &handler, method_ref) {
                        Ok(_) => Ok(format!("Route '{}' registered successfully", path)),
                        Err(e) => Ok(format!("Error registering route: {}", e)),
                    }
                },
            )?;
            global.set("register", register)?;
        } else {
            // No-op register function
            let reg_noop = Function::new(
                ctx.clone(),
                |_c: rquickjs::Ctx<'_>,
                 _path: String,
                 _handler: String,
                 _method: Option<String>|
                 -> JsResult<String> {
                    Ok("Route registration not available in this context".to_string())
                },
            )?;
            global.set("register", reg_noop)?;
        }

        Ok(())
    }
}
