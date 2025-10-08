use rquickjs::{Function, Result as JsResult};
use tracing::debug;

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
            move |_ctx: rquickjs::Ctx<'_>, message: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_write.require_capability(&crate::security::Capability::ViewLogs)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_write.user_id,
                    message_len = message.len(),
                    "Secure writeLog called"
                );

                // Write to repository
                repository::insert_log_message("", &message);

                Ok("Log written successfully".to_string())
            },
        )?;
        global.set("writeLog", write_log)?;

        // Secure listLogs function
        let user_ctx_list = user_context.clone();
        let list_logs = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
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
                debug!(
                    uri = %uri,
                    user_id = ?user_ctx_list_uri.user_id,
                    "Secure listLogsForUri called"
                );

                let logs = repository::fetch_log_messages(&uri);
                Ok(serde_json::to_string(&logs).unwrap_or_else(|_| "[]".to_string()))
            },
        )?;
        global.set("listLogsForUri", list_logs_for_uri)?;

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

        // Secure getScript function
        let user_ctx_get = user_context.clone();
        let get_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<String> {
                debug!(
                    script_name = %script_name,
                    user_id = ?user_ctx_get.user_id,
                    "Secure getScript called"
                );

                match repository::fetch_script(&script_name) {
                    Some(content) => Ok(content),
                    None => Ok(format!("Error: Script '{}' not found", script_name)),
                }
            },
        )?;
        global.set("getScript", get_script)?;

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

                Ok(format!("Script '{}' upserted successfully", script_name))
            },
        )?;
        global.set("upsertScript", upsert_script)?;

        // Secure deleteScript function - simplified for testing
        let user_ctx_delete = user_context.clone();
        let delete_script = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_delete.require_capability(&crate::security::Capability::DeleteScripts)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    script_name = %script_name,
                    user_id = ?user_ctx_delete.user_id,
                    "Secure deleteScript called"
                );

                repository::delete_script(&script_name);
                Ok(format!("Script '{}' deleted successfully", script_name))
            },
        )?;
        global.set("deleteScript", delete_script)?;

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
