use rquickjs::{Function, Result as JsResult};
use tracing::debug;

use crate::security::{
    UserContext, SecureOperations, SecurityAuditor,
    SecurityEventType, SecuritySeverity, UpsertScriptRequest
};
use crate::repository;

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
                if let Err(e) = user_ctx_write.require_capability(&crate::security::Capability::ViewLogs) {
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
                rt.block_on(auditor_write.log_event(
                    crate::security::SecurityEvent::new(
                        SecurityEventType::SystemSecurityEvent,
                        SecuritySeverity::Low,
                        user_ctx_write.user_id.clone(),
                    )
                    .with_resource("log".to_string())
                    .with_action("write".to_string())
                    .with_detail("script_uri", &script_uri_write)
                    .with_detail("message_length", message.len().to_string())
                ));
                
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
                if let Err(e) = user_ctx_list.require_capability(&crate::security::Capability::ViewLogs) {
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
                if let Err(e) = user_ctx_list_uri.require_capability(&crate::security::Capability::ViewLogs) {
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
    fn setup_script_management_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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
                if let Err(e) = user_ctx_list.require_capability(&crate::security::Capability::ReadScripts) {
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
                if let Err(e) = user_ctx_get.require_capability(&crate::security::Capability::ReadScripts) {
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
            move |_ctx: rquickjs::Ctx<'_>, script_name: String, js_script: String| -> JsResult<String> {
                // Use secure operations for validation and capability checking
                let request = UpsertScriptRequest {
                    script_name: script_name.clone(),
                    js_script: js_script.clone(),
                };
                
                let rt = tokio::runtime::Handle::current();
                let result = rt.block_on(secure_ops_upsert.upsert_script(&user_ctx_upsert, request));
                
                // Log the operation attempt
                rt.block_on(auditor_upsert.log_event(
                    crate::security::SecurityEvent::new(
                        SecurityEventType::SystemSecurityEvent,
                        SecuritySeverity::Medium,
                        user_ctx_upsert.user_id.clone(),
                    )
                    .with_resource("script".to_string())
                    .with_action("upsert".to_string())
                    .with_detail("script_name", &script_name)
                    .with_detail("script_uri", &script_uri_upsert)
                    .with_detail("content_length", js_script.len().to_string())
                ));
                
                match result {
                    Ok(op_result) => {
                        if op_result.success {
                            // If validation passed, call repository
                            match repository::upsert_script(&script_name, &js_script) {
                                Ok(_) => Ok(format!("Script '{}' upserted successfully", script_name)),
                                Err(e) => Ok(format!("Error upserting script: {}", e)),
                            }
                        } else {
                            Ok(op_result.error.unwrap_or_else(|| "Unknown error".to_string()))
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
                if let Err(e) = user_ctx_delete.require_capability(&crate::security::Capability::DeleteScripts) {
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
                rt.block_on(auditor_delete.log_event(
                    crate::security::SecurityEvent::new(
                        SecurityEventType::SystemSecurityEvent,
                        SecuritySeverity::High,
                        user_ctx_delete.user_id.clone(),
                    )
                    .with_resource("script".to_string())
                    .with_action("delete".to_string())
                    .with_detail("script_name", &script_name)
                ));
                
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
    fn setup_asset_management_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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
                if let Err(e) = user_ctx_list.require_capability(&crate::security::Capability::ReadAssets) {
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
        
        // More asset functions to be added...
        Ok(())
    }
    
    /// Setup secure GraphQL functions  
    fn setup_graphql_functions(&self, _ctx: &rquickjs::Ctx<'_>, _script_uri: &str) -> JsResult<()> {
        // TODO: Implement GraphQL functions
        Ok(())
    }
    
    /// Setup secure stream functions
    fn setup_stream_functions(&self, _ctx: &rquickjs::Ctx<'_>, _script_uri: &str) -> JsResult<()> {
        // TODO: Implement stream functions
        Ok(())
    }
}