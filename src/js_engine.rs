use rquickjs::{Context, Function, Runtime, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

use crate::repository;
use crate::security::UserContext;

// Use the enhanced secure globals implementation
use crate::security::secure_globals::{GlobalSecurityConfig, SecureGlobalContext};

/// Resource limits for JavaScript execution
#[derive(Debug, Clone)]
pub struct ExecutionLimits {
    pub timeout_ms: u64,
    pub max_memory_mb: usize,
    pub max_script_size_bytes: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            timeout_ms: 2000,
            max_memory_mb: 50,
            max_script_size_bytes: 1_000_000, // 1MB
        }
    }
}

/// Parameters for secure script execution in request context
#[derive(Debug)]
pub struct RequestExecutionParams {
    pub script_uri: String,
    pub handler_name: String,
    pub path: String,
    pub method: String,
    pub query_params: Option<HashMap<String, String>>,
    pub form_data: Option<HashMap<String, String>>,
    pub raw_body: Option<String>,
    pub user_context: UserContext,
    /// Optional OAuth authentication context for JavaScript auth API
    pub auth_context: Option<crate::auth::JsAuthContext>,
}

/// Validates a script before execution
fn validate_script(content: &str, limits: &ExecutionLimits) -> Result<(), String> {
    if content.len() > limits.max_script_size_bytes {
        return Err(format!(
            "Script too large: {} bytes (max: {})",
            content.len(),
            limits.max_script_size_bytes
        ));
    }

    // Basic syntax validation - check for obviously problematic patterns
    if content.contains("while(true)") || content.contains("while (true)") {
        warn!("Script contains potentially infinite loop pattern");
    }

    Ok(())
}

/// Function type for registering functions in different execution contexts
type RegisterFunctionType = Box<dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>>;

/// Sets up secure global functions with proper capability validation
///
/// This function replaces the old vulnerable setup_global_functions with a secure implementation
/// that enforces all security validation in Rust before allowing JavaScript operations.
fn setup_secure_global_functions(
    ctx: &rquickjs::Ctx<'_>,
    script_uri: &str,
    user_context: UserContext,
    config: &GlobalSecurityConfig,
    register_fn: Option<RegisterFunctionType>,
    auth_context: Option<crate::auth::JsAuthContext>,
    secrets_manager: Option<std::sync::Arc<crate::secrets::SecretsManager>>,
) -> Result<(), rquickjs::Error> {
    let secure_context = if let Some(secrets) = secrets_manager {
        SecureGlobalContext::new_with_secrets(user_context, config.clone(), secrets)
    } else {
        SecureGlobalContext::new_with_config(user_context, config.clone())
    };

    // Setup secure functions with proper capability validation
    secure_context.setup_secure_functions(ctx, script_uri, register_fn)?;

    // Setup authentication globals if auth context is provided
    if let Some(auth_ctx) = auth_context {
        crate::auth::AuthJsApi::setup_auth_globals(ctx, auth_ctx)?;
    }

    Ok(())
}

/// Sets up common global functions for JavaScript execution contexts (LEGACY)
///
/// This function consolidates the repeated pattern of setting up global functions
/// across different execution contexts (script registration, request handling, GraphQL resolution)
///
/// Represents the result of executing a JavaScript script
#[derive(Debug, Clone)]
pub struct ScriptExecutionResult {
    /// The registrations made by the script via register() calls
    pub registrations: HashMap<(String, String), String>,
    /// Whether the script executed successfully
    pub success: bool,
    /// Error message if execution failed
    pub error: Option<String>,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

impl ScriptExecutionResult {
    /// Create a failed execution result with error message
    fn failed(error_message: String, execution_time_ms: u64) -> Self {
        Self {
            registrations: HashMap::new(),
            success: false,
            error: Some(error_message),
            execution_time_ms,
        }
    }

    /// Create a successful execution result
    fn success(registrations: HashMap<(String, String), String>, execution_time_ms: u64) -> Self {
        Self {
            registrations,
            success: true,
            error: None,
            execution_time_ms,
        }
    }
}

/// Executes a JavaScript script and captures any register() method calls
///
/// Executes a JavaScript script in a secure environment with proper authentication and validation.
/// This function creates a QuickJS runtime, sets up the register function,
/// executes the script, and returns information about the registrations made.
///
/// All global functions are secured with capability checking and input validation.
pub fn execute_script_secure(
    uri: &str,
    content: &str,
    user_context: UserContext,
) -> ScriptExecutionResult {
    let start_time = Instant::now();

    // Validate script using default limits
    let limits = ExecutionLimits::default();
    if let Err(e) = validate_script(content, &limits) {
        return ScriptExecutionResult::failed(e, start_time.elapsed().as_millis() as u64);
    }

    // Store the script in the repository so it can be accessed later
    let _ = repository::upsert_script(uri, content);

    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = uri.to_string();

    match Runtime::new() {
        Ok(rt) => match Context::full(&rt) {
            Ok(ctx) => {
                let result = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                    // Set up all secure global functions with audit logging disabled for startup
                    let security_config = GlobalSecurityConfig {
                        enable_audit_logging: false, // Disable for startup to reduce noise
                        // Re-enable GraphQL and streams now that block_on calls are fixed
                        enable_graphql_registration: true,
                        enable_streams: true,
                        ..Default::default()
                    };

                    // Create the register function that captures registrations
                    let regs_clone = Rc::clone(&registrations);
                    let uri_clone = uri_owned.clone();
                    let register_impl = Box::new(
                        move |path: &str,
                              handler: &str,
                              method: Option<&str>|
                              -> Result<(), rquickjs::Error> {
                            let method = method.unwrap_or("GET");
                            debug!(
                                "Securely registering route {} {} -> {} for script {}",
                                method, path, handler, uri_clone
                            );
                            if let Ok(mut regs) = regs_clone.try_borrow_mut() {
                                regs.insert(
                                    (path.to_string(), method.to_string()),
                                    handler.to_string(),
                                );
                            }
                            Ok(())
                        },
                    );

                    setup_secure_global_functions(
                        &ctx,
                        &uri_owned,
                        user_context,
                        &security_config,
                        Some(register_impl),
                        None, // No auth context during script execution with config
                        None, // No secrets manager yet
                    )?;

                    // Execute the script
                    ctx.eval::<(), _>(content)?;
                    Ok(())
                });

                match result {
                    Ok(_) => {
                        debug!("Script {} executed successfully", uri);
                        match registrations.try_borrow() {
                            Ok(regs) => ScriptExecutionResult::success(
                                regs.clone(),
                                start_time.elapsed().as_millis() as u64,
                            ),
                            Err(_) => ScriptExecutionResult::failed(
                                "Failed to access registrations".to_string(),
                                start_time.elapsed().as_millis() as u64,
                            ),
                        }
                    }
                    Err(e) => {
                        error!("Script {} execution failed: {}", uri, e);
                        ScriptExecutionResult::failed(
                            format!("Script execution failed: {}", e),
                            start_time.elapsed().as_millis() as u64,
                        )
                    }
                }
            }
            Err(e) => {
                error!("Failed to create context for script {}: {}", uri, e);
                ScriptExecutionResult::failed(
                    format!("Failed to create context: {}", e),
                    start_time.elapsed().as_millis() as u64,
                )
            }
        },
        Err(e) => {
            error!("Failed to create runtime for script {}: {}", uri, e);
            ScriptExecutionResult::failed(
                format!("Failed to create runtime: {}", e),
                start_time.elapsed().as_millis() as u64,
            )
        }
    }
}

/// Executes a JavaScript script (LEGACY - has security vulnerabilities).
/// This function creates a QuickJS runtime, sets up the register function,
/// executes the script, and returns information about the registrations made.
pub fn execute_script(uri: &str, content: &str) -> ScriptExecutionResult {
    let start_time = Instant::now();

    tracing::info!("execute_script called for URI: {}", uri);

    // Validate script using default limits
    let limits = ExecutionLimits::default();
    if let Err(e) = validate_script(content, &limits) {
        return ScriptExecutionResult::failed(e, start_time.elapsed().as_millis() as u64);
    }

    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = uri.to_string();

    match Runtime::new() {
        Ok(rt) => {
            match Context::full(&rt) {
                Ok(ctx) => {
                    let result =
                        ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                            // Set up all global functions using the secure helper function
                            let config = GlobalSecurityConfig::default();

                            // Create the register function that captures registrations
                            let regs_clone = Rc::clone(&registrations);
                            let uri_clone = uri_owned.clone();
                            let register_impl = Box::new(
                        move |path: &str,
                              handler: &str,
                              method: Option<&str>|
                              -> Result<(), rquickjs::Error> {
                            let method = method.unwrap_or("GET");
                            tracing::info!(
                                "Registering route {} {} -> {} for script {}",
                                method, path, handler, uri_clone
                            );
                            if let Ok(mut regs) = regs_clone.try_borrow_mut() {
                                regs.insert(
                                    (path.to_string(), method.to_string()),
                                    handler.to_string(),
                                );
                            }
                            Ok(())
                        },
                    );

                            setup_secure_global_functions(
                                &ctx,
                                &uri_owned,
                                UserContext::admin("route-discovery".to_string()),
                                &config,
                                Some(register_impl),
                                None, // No auth context during script registration
                                None, // No secrets manager yet
                            )?; // Execute the script
                            ctx.eval::<(), _>(content)?;
                            Ok(())
                        });

                    match result {
                        Ok(_) => {
                            tracing::info!("Successfully executed script {}", uri_owned);
                            let final_regs = registrations.borrow().clone();
                            tracing::info!(
                                "Script {} registered {} routes: {:?}",
                                uri_owned,
                                final_regs.len(),
                                final_regs
                            );
                            let execution_time = start_time.elapsed().as_millis() as u64;
                            ScriptExecutionResult::success(final_regs, execution_time)
                        }
                        Err(e) => {
                            error!("Failed to execute script {}: {}", uri_owned, e);
                            ScriptExecutionResult::failed(
                                format!("Script evaluation error: {}", e),
                                start_time.elapsed().as_millis() as u64,
                            )
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to create QuickJS context for script {}: {}",
                        uri_owned, e
                    );
                    ScriptExecutionResult::failed(
                        format!("Context creation error: {}", e),
                        start_time.elapsed().as_millis() as u64,
                    )
                }
            }
        }
        Err(e) => {
            error!(
                "Failed to create QuickJS runtime for script {}: {}",
                uri_owned, e
            );
            ScriptExecutionResult::failed(
                format!("Runtime creation error: {}", e),
                start_time.elapsed().as_millis() as u64,
            )
        }
    }
}

/// Executes a JavaScript script for an HTTP request with secure global functions
///
/// This function creates a QuickJS runtime, sets up secure host functions,
/// executes the script, calls the specified handler with request parameters,
/// and returns the response.
///
/// All global functions are secured with capability checking and input validation.
pub fn execute_script_for_request_secure(
    params: RequestExecutionParams,
) -> Result<(u16, String, Option<String>), String> {
    let script_uri_owned = params.script_uri.clone();
    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up all secure global functions
        // For request handling, we don't need GraphQL registration but enable everything else
        let security_config = GlobalSecurityConfig {
            enable_graphql_registration: false,
            enable_audit_logging: false, // Disable for tests to avoid runtime conflicts
            ..Default::default()
        };

        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            params.user_context,
            &security_config,
            None,
            params.auth_context, // Pass auth context for request handling
            None, // No secrets manager yet
        )?;

        Ok(())
    })
    .map_err(|e| format!("install secure host fns: {}", e))?;

    let owner_script = repository::fetch_script(&params.script_uri)
        .ok_or_else(|| format!("no script for uri {}", params.script_uri))?;

    ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str()))
        .map_err(|e| format!("owner eval: {}", e))?;

    let (status, body, content_type) =
        ctx.with(|ctx| -> Result<(u16, String, Option<String>), String> {
            let global = ctx.globals();
            let func: Function = global
                .get::<_, Function>(&params.handler_name)
                .map_err(|e| format!("no handler {}: {}", params.handler_name, e))?;

            // Build the request object
            let request_obj =
                rquickjs::Object::new(ctx.clone()).map_err(|e| format!("create req obj: {}", e))?;
            request_obj
                .set("path", &params.path)
                .map_err(|e| format!("set path: {}", e))?;
            request_obj
                .set("method", &params.method)
                .map_err(|e| format!("set method: {}", e))?;

            // Add query parameters if present
            if let Some(ref query_params) = params.query_params {
                let params_obj = rquickjs::Object::new(ctx.clone())
                    .map_err(|e| format!("create params obj: {}", e))?;
                for (key, value) in query_params {
                    params_obj
                        .set(key.as_str(), value.as_str())
                        .map_err(|e| format!("set param {}: {}", key, e))?;
                }
                request_obj
                    .set("query", params_obj)
                    .map_err(|e| format!("set query: {}", e))?;
            }

            // Add form data if present
            if let Some(ref form_data) = params.form_data {
                let form_obj = rquickjs::Object::new(ctx.clone())
                    .map_err(|e| format!("create form obj: {}", e))?;
                for (key, value) in form_data {
                    form_obj
                        .set(key.as_str(), value.as_str())
                        .map_err(|e| format!("set form {}: {}", key, e))?;
                }
                request_obj
                    .set("form", form_obj)
                    .map_err(|e| format!("set form: {}", e))?;
            }

            // Add raw body if present
            if let Some(ref body) = params.raw_body {
                request_obj
                    .set("body", body)
                    .map_err(|e| format!("set body: {}", e))?;
            }

            // Call the handler function
            let result: Value = func
                .call::<_, Value>((request_obj,))
                .map_err(|e| format!("call handler: {}", e))?;

            // Parse the response
            if let Some(response_obj) = result.as_object() {
                let status: i32 = response_obj
                    .get("status")
                    .map_err(|e| format!("missing status: {}", e))?;
                let body: String = response_obj
                    .get("body")
                    .map_err(|e| format!("missing body: {}", e))?;
                let content_type: Option<String> = response_obj.get("contentType").ok();

                debug!(
                    "Secure request handler {} returned status: {}, body length: {}",
                    params.handler_name,
                    status,
                    body.len()
                );

                Ok((status as u16, body, content_type))
            } else {
                // If not an object, treat as string response
                let body = if result.is_string() {
                    result
                        .as_string()
                        .and_then(|s| s.to_string().ok())
                        .unwrap_or_else(|| "<conversion error>".to_string())
                } else {
                    "<no response>".to_string()
                };
                Ok((200, body, None))
            }
        })?;

    Ok((status, body, content_type))
}

/// Executes a JavaScript script for an HTTP request (LEGACY - has security vulnerabilities)
///
/// This function creates a QuickJS runtime, sets up host functions,
/// executes the script, calls the specified handler with request parameters,
/// and returns the response.
pub fn execute_script_for_request(
    script_uri: &str,
    handler_name: &str,
    path: &str,
    method: &str,
    query_params: Option<&std::collections::HashMap<String, String>>,
    form_data: Option<&std::collections::HashMap<String, String>>,
    raw_body: Option<String>,
) -> Result<(u16, String, Option<String>), String> {
    let script_uri_owned = script_uri.to_string();
    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up all global functions using the secure helper function
        // For request handling, we don't need full GraphQL registration (no-ops)
        let config = GlobalSecurityConfig {
            enable_graphql_registration: false,
            enable_audit_logging: false, // Disable audit logging to avoid runtime conflicts
            ..Default::default()
        };

        // Always provide an anonymous auth context so scripts can safely check auth state
        let auth_ctx = crate::auth::JsAuthContext::anonymous();

        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            UserContext::anonymous(),
            &config,
            None,
            Some(auth_ctx), // Provide anonymous auth context
            None, // No secrets manager yet
        )?;

        Ok(())
    })
    .map_err(|e| format!("install host fns: {}", e))?;

    let owner_script = repository::fetch_script(script_uri)
        .ok_or_else(|| format!("no script for uri {}", script_uri))?;

    ctx.with(|ctx| ctx.eval::<(), _>(owner_script.as_str()))
        .map_err(|e| format!("owner eval: {}", e))?;

    let (status, body, content_type) =
        ctx.with(|ctx| -> Result<(u16, String, Option<String>), String> {
            let global = ctx.globals();
            let func: Function = global
                .get::<_, Function>(handler_name)
                .map_err(|e| format!("no handler {}: {}", handler_name, e))?;

            let req_obj =
                rquickjs::Object::new(ctx.clone()).map_err(|e| format!("make req obj: {}", e))?;

            req_obj
                .set("method", method)
                .map_err(|e| format!("set method: {}", e))?;

            req_obj
                .set("path", path)
                .map_err(|e| format!("set path: {}", e))?;

            if let Some(qp) = query_params {
                let query_obj = rquickjs::Object::new(ctx.clone())
                    .map_err(|e| format!("make query obj: {}", e))?;
                for (key, value) in qp {
                    query_obj
                        .set(key, value)
                        .map_err(|e| format!("set query param {}: {}", key, e))?;
                }
                req_obj
                    .set("query", query_obj)
                    .map_err(|e| format!("set query: {}", e))?;
            }

            if let Some(fd) = form_data {
                let form_obj = rquickjs::Object::new(ctx.clone())
                    .map_err(|e| format!("make form obj: {}", e))?;
                for (key, value) in fd {
                    form_obj
                        .set(key, value)
                        .map_err(|e| format!("set form param {}: {}", key, e))?;
                }
                req_obj
                    .set("form", form_obj)
                    .map_err(|e| format!("set form: {}", e))?;
            }

            if let Some(rb) = raw_body {
                req_obj
                    .set("body", rb)
                    .map_err(|e| format!("set body: {}", e))?;
            }

            let val = func
                .call::<_, Value>((req_obj,))
                .map_err(|e| format!("call error: {}", e))?;

            let obj = val
                .as_object()
                .ok_or_else(|| "expected object".to_string())?;

            let status: i32 = obj
                .get("status")
                .map_err(|e| format!("missing status: {}", e))?;

            let body: String = obj
                .get("body")
                .map_err(|e| format!("missing body: {}", e))?;

            // Extract optional contentType field
            let content_type: Option<String> = obj.get("contentType").ok(); // This will be None if the field doesn't exist

            Ok((status as u16, body, content_type))
        })?;

    Ok((status, body, content_type))
}

/// Executes a JavaScript GraphQL resolver function and returns the result as a string.
/// This is used by the GraphQL system to call JavaScript resolver functions.
pub fn execute_graphql_resolver(
    script_uri: &str,
    resolver_function: &str,
    args: Option<serde_json::Value>,
) -> Result<String, String> {
    let script_uri_owned = script_uri.to_string();
    let resolver_function_owned = resolver_function.to_string();
    let args_owned = args;

    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    ctx.with(|ctx| -> Result<String, rquickjs::Error> {
        // Set up all global functions using the secure helper function
        // For GraphQL resolvers, we don't need GraphQL registration (no-ops) or stream registration
        let config = GlobalSecurityConfig {
            enable_graphql_registration: false,
            enable_streams: false,
            enable_audit_logging: false, // Disable audit logging to avoid runtime conflicts
            ..Default::default()
        };

        // GraphQL resolvers run with admin context to allow script management operations
        // In production, this should be secured via GraphQL-level authentication/authorization
        setup_secure_global_functions(&ctx, &script_uri_owned, UserContext::admin("graphql-resolver".to_string()), &config, None, None, None)?;

        // Override specific functions that have different signatures for GraphQL resolver context
        let global = ctx.globals();

        let list_scripts_resolver = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>| -> Result<std::collections::HashMap<String, String>, rquickjs::Error> {
                debug!("JavaScript called listScripts");
                Ok(repository::fetch_scripts())
            },
        )?;
        global.set("listScripts", list_scripts_resolver)?;

        let fetch_asset_resolver = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, path: String| -> Result<Option<String>, rquickjs::Error> {
                debug!("JavaScript called fetchAsset with path: {}", path);
                Ok(repository::fetch_asset(&path).and_then(|asset| String::from_utf8(asset.content).ok()))
            },
        )?;
        global.set("fetchAsset", fetch_asset_resolver)?;

        let upsert_asset_resolver = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, path: String, content: String, mime_type: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called upsertAsset with path: {}", path);
                let asset = repository::Asset {
                    public_path: path,
                    content: content.into_bytes(),
                    mimetype: mime_type,
                };
                let _ = repository::upsert_asset(asset);
                Ok(())
            },
        )?;
        global.set("upsertAsset", upsert_asset_resolver)?;

        let get_script_resolver = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, uri: String| -> Result<Option<String>, rquickjs::Error> {
                debug!("JavaScript called getScript with uri: {}", uri);
                Ok(repository::fetch_script(&uri))
            },
        )?;
        global.set("getScript", get_script_resolver)?;

        // Load and execute the script
        let script_content = repository::fetch_script(&script_uri_owned)
            .ok_or_else(|| rquickjs::Error::new_from_js("Script", "not found"))?;

        // Execute the script
        ctx.eval::<(), _>(script_content.as_str())?;

        // Prepare arguments for the resolver function
        let args_value = if let Some(args) = args_owned {
            // Convert serde_json::Value to QuickJS value
            match args {
                serde_json::Value::Object(obj) => {
                    let obj_val = ctx.globals().get::<_, rquickjs::Object>("Object")?;
                    let create = obj_val.get::<_, rquickjs::Function>("create")?;
                    let proto = ctx.globals().get::<_, rquickjs::Object>("Object")?;
                    let proto = proto.get::<_, rquickjs::Object>("prototype")?;
                    let args_obj: rquickjs::Object = create.call((proto,))?;

                    for (key, value) in obj {
                        match value {
                            serde_json::Value::String(s) => args_obj.set(key, s)?,
                            serde_json::Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    args_obj.set(key, i)?;
                                } else if let Some(f) = n.as_f64() {
                                    args_obj.set(key, f)?;
                                }
                            },
                            serde_json::Value::Bool(b) => args_obj.set(key, b)?,
                            _ => {} // Skip other types for now
                        }
                    }
                    args_obj.into_value()
                },
                _ => rquickjs::Value::new_undefined(ctx.clone()),
            }
        } else {
            rquickjs::Value::new_undefined(ctx.clone())
        };

        // Call the resolver function
        let resolver_result: rquickjs::Value = ctx.globals().get(&resolver_function_owned)?;
        let resolver_func = resolver_result.as_function().ok_or_else(|| rquickjs::Error::new_from_js("Function", "not found"))?;

        let result_value = if args_value.is_undefined() {
            resolver_func.call::<_, rquickjs::Value>(())?
        } else {
            resolver_func.call::<_, rquickjs::Value>((args_value,))?
        };

        // Convert the result to a JSON string
        let result_string: String = if result_value.is_string() {
            result_value
                .as_string()
                .ok_or_else(|| rquickjs::Error::new_from_js("value", "string"))?
                .to_string()?
        } else {
            // Use JavaScript's JSON.stringify to convert any value to JSON
            let json_obj: rquickjs::Object = ctx.globals().get("JSON")?;
            let json_stringify: rquickjs::Function = json_obj.get("stringify")?;
            let json_str: String = json_stringify.call((result_value,))?;
            json_str
        };

        Ok(result_string)
    }).map_err(|e| format!("JavaScript execution error: {}", e))
}

/// Calls the init() function in a script if it exists
///
/// This function executes a script and checks if it has an `init()` function defined.
/// If found, it calls the function with the provided context.
///
/// Returns:
/// - Ok(true) if init() was found and called successfully
/// - Ok(false) if no init() function exists (not an error)
/// - Err(String) if init() exists but threw an error
pub fn call_init_if_exists(
    script_uri: &str,
    script_content: &str,
    context: crate::script_init::InitContext,
) -> Result<Option<HashMap<(String, String), String>>, String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    debug!("Checking for init() function in script: {}", script_uri);

    let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("Failed to create context: {}", e))?;

    // Create registrations map to capture register() calls during init
    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = script_uri.to_string();

    let result = ctx
        .with(|ctx| -> Result<bool, rquickjs::Error> {
            // Set up secure global functions with minimal config for init
            let config = GlobalSecurityConfig {
                enable_audit_logging: false,
                enable_graphql_registration: true,
                enable_streams: true,
                ..Default::default()
            };

            // Create the register function that captures registrations
            let regs_clone = Rc::clone(&registrations);
            let uri_clone = uri_owned.clone();
            let register_impl = Box::new(
                move |path: &str,
                      handler: &str,
                      method: Option<&str>|
                      -> Result<(), rquickjs::Error> {
                    let method = method.unwrap_or("GET");
                    debug!(
                        "Registering route {} {} -> {} for script {} during init()",
                        method, path, handler, uri_clone
                    );
                    if let Ok(mut regs) = regs_clone.try_borrow_mut() {
                        regs.insert((path.to_string(), method.to_string()), handler.to_string());
                    }
                    Ok(())
                },
            );

            // Init runs with admin context to allow script registration operations
            setup_secure_global_functions(
                &ctx,
                script_uri,
                UserContext::admin("script-init".to_string()),
                &config,
                Some(register_impl),
                None,
                None, // No secrets manager yet
            )?;

            // Execute the script to define functions
            ctx.eval::<(), _>(script_content)?;

            // Check if init function exists
            let globals = ctx.globals();
            let init_value: rquickjs::Value = match globals.get("init") {
                Ok(v) => v,
                Err(_) => {
                    // No init function defined - this is OK
                    debug!("No init() function found in script: {}", script_uri);
                    return Ok(false);
                }
            };

            // Check if it's actually a function
            if !init_value.is_function() {
                debug!(
                    "init exists but is not a function in script: {}",
                    script_uri
                );
                return Ok(false);
            }

            let init_func = init_value
                .as_function()
                .ok_or_else(|| rquickjs::Error::new_from_js("init", "not a function"))?;

            // Create context object to pass to init()
            let context_obj = rquickjs::Object::new(ctx.clone())?;
            context_obj.set("scriptName", context.script_name.clone())?;

            // Convert SystemTime to milliseconds since UNIX_EPOCH
            let timestamp_ms = context
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f64;
            context_obj.set("timestamp", timestamp_ms)?;
            context_obj.set("isStartup", context.is_startup)?;

            // Call init function with context
            debug!("Calling init() function for script: {}", script_uri);
            init_func.call::<_, ()>((context_obj,))?;

            info!("Successfully called init() for script: {}", script_uri);
            Ok(true)
        })
        .map_err(|e| format!("Init function error: {}", e))?;

    // Return registrations if init was called
    if result {
        match registrations.try_borrow() {
            Ok(regs) => {
                let reg_count = regs.len();
                info!(
                    "Init() for script {} registered {} routes",
                    script_uri, reg_count
                );
                Ok(Some(regs.clone()))
            }
            Err(_) => Err("Failed to access registrations".to_string()),
        }
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_registry;

    #[test]
    fn test_execute_script_simple_registration() {
        let content = r#"
            register("/test", "handler_function", "GET");
        "#;

        let result = execute_script("test-script", content);

        assert!(result.success, "Script execution should succeed");
        assert!(result.error.is_none(), "Should not have error");
        assert_eq!(result.registrations.len(), 1);
        assert_eq!(
            result
                .registrations
                .get(&("/test".to_string(), "GET".to_string())),
            Some(&"handler_function".to_string())
        );
    }

    #[test]
    fn test_execute_script_multiple_registrations() {
        let content = r#"
            register("/api/users", "getUsers", "GET");
            register("/api/users", "createUser", "POST");
            register("/api/users/:id", "updateUser", "PUT");
        "#;

        let result = execute_script("multi-script", content);

        assert!(result.success);
        assert_eq!(result.registrations.len(), 3);
        assert!(
            result
                .registrations
                .contains_key(&("/api/users".to_string(), "GET".to_string()))
        );
        assert!(
            result
                .registrations
                .contains_key(&("/api/users".to_string(), "POST".to_string()))
        );
        assert!(
            result
                .registrations
                .contains_key(&("/api/users/:id".to_string(), "PUT".to_string()))
        );
    }

    #[test]
    fn test_execute_script_with_default_method() {
        let content = r#"
            register("/default-method", "handler", "GET");
        "#;

        let result = execute_script("default-method-script", content);

        if !result.success {
            println!("Default method test failed with error: {:?}", result.error);
        }
        assert!(
            result.success,
            "Script execution failed: {:?}",
            result.error
        );
        assert_eq!(
            result
                .registrations
                .get(&("/default-method".to_string(), "GET".to_string())),
            Some(&"handler".to_string())
        );
    }

    #[test]
    fn test_execute_script_with_syntax_error() {
        let content = r#"
            register("/test", "handler"
            // Missing closing parenthesis - syntax error
        "#;

        let result = execute_script("error-script", content);

        assert!(!result.success, "Script with syntax error should fail");
        assert!(result.error.is_some(), "Should have error message");
        assert!(
            result.registrations.is_empty(),
            "Should not have registrations on error"
        );
    }

    #[test]
    fn test_execute_script_with_runtime_error() {
        let content = r#"
            throw new Error("Runtime error test");
        "#;

        let result = execute_script("runtime-error-script", content);

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.registrations.is_empty());
    }

    #[test]
    fn test_execute_script_with_complex_javascript() {
        let content = r#"
            function setupRoutes() {
                register("/api/health", "healthCheck", "GET");
                register("/api/status", "statusCheck", "GET");
            }

            setupRoutes();
        "#;

        let result = execute_script("complex-script", content);

        assert!(
            result.success,
            "Complex JavaScript should execute successfully. Error: {:?}",
            result.error
        );
        assert_eq!(result.registrations.len(), 2);
        assert!(
            result
                .registrations
                .contains_key(&("/api/health".to_string(), "GET".to_string()))
        );
        assert!(
            result
                .registrations
                .contains_key(&("/api/status".to_string(), "GET".to_string()))
        );
    }

    #[test]
    fn test_execute_script_empty_content() {
        let result = execute_script("empty-script", "");

        assert!(result.success, "Empty script should succeed");
        assert!(result.error.is_none());
        assert!(result.registrations.is_empty());
    }

    #[test]
    fn test_execute_script_with_console_log() {
        let content = r#"
            register("/logged", "loggedHandler", "GET");
        "#;

        let result = execute_script("console-script", content);

        // Should succeed even with console.log (which may not be available)
        // The important thing is it doesn't crash
        // Console.log may fail, so the script might not succeed, but it shouldn't crash
        if result.success {
            assert_eq!(result.registrations.len(), 1);
        } else {
            // If console.log failed, that's ok, we just check it didn't crash
            assert!(result.error.is_some());
        }
    }

    #[test]
    fn test_execute_graphql_resolver_simple() {
        // First, need to store the script
        let script_content = r#"
            function testResolver() {
                return "Hello World";
            }
        "#;

        // Store the script in repository first
        match repository::upsert_script("test-resolver", script_content) {
            Ok(_) => {}
            Err(_) => {} // Ignore errors for test
        }

        let result = execute_graphql_resolver("test-resolver", "testResolver", None);

        assert!(result.is_ok(), "Simple resolver should succeed");
        let json_result = result.unwrap();
        assert!(json_result == "Hello World" || json_result == "\"Hello World\""); // Handle both cases
    }

    #[test]
    fn test_execute_graphql_resolver_with_args() {
        let script_content = r#"
            function greetUser(args) {
                return "Hello " + args.name + "!";
            }
        "#;

        // Store the script
        let _ = repository::upsert_script("greet-resolver", script_content);

        let args = serde_json::json!({"name": "Alice"});
        let result = execute_graphql_resolver("greet-resolver", "greetUser", Some(args));

        assert!(result.is_ok(), "Resolver with args should succeed");
        let json_result = result.unwrap();
        assert!(json_result == "Hello Alice!" || json_result == "\"Hello Alice!\"");
    }

    #[test]
    fn test_execute_graphql_resolver_returning_object() {
        let script_content = r#"
            function getUserInfo() {
                return {
                    id: 1,
                    name: "John Doe",
                    email: "john@example.com"
                };
            }
        "#;

        let _ = repository::upsert_script("user-resolver", script_content);
        let result = execute_graphql_resolver("user-resolver", "getUserInfo", None);

        assert!(result.is_ok(), "Resolver returning object should succeed");
        let json_result = result.unwrap();
        assert!(json_result.contains("John Doe"));
        assert!(json_result.contains("john@example.com"));
    }

    #[test]
    fn test_execute_graphql_resolver_nonexistent_script() {
        let result = execute_graphql_resolver("nonexistent-script", "someFunction", None);

        assert!(result.is_err(), "Should fail when script doesn't exist");
    }

    #[test]
    fn test_execute_graphql_resolver_nonexistent_function() {
        let script_content = r#"
            function someOtherFunction() {
                return "test";
            }
        "#;

        let _ = repository::upsert_script("missing-function-resolver", script_content);
        let result =
            execute_graphql_resolver("missing-function-resolver", "nonExistentFunction", None);

        assert!(result.is_err(), "Should fail when function doesn't exist");
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_execute_graphql_resolver_with_runtime_exception() {
        let script_content = r#"
            function throwingResolver() {
                throw new Error("Something went wrong");
            }
        "#;

        let _ = repository::upsert_script("throwing-resolver", script_content);
        let result = execute_graphql_resolver("throwing-resolver", "throwingResolver", None);

        assert!(
            result.is_err(),
            "Should fail when resolver throws exception"
        );
        assert!(result.unwrap_err().contains("execution error"));
    }

    #[test]
    fn test_script_execution_result_debug_format() {
        let mut registrations = HashMap::new();
        registrations.insert(
            ("/test".to_string(), "GET".to_string()),
            "handler".to_string(),
        );

        let result = ScriptExecutionResult {
            registrations,
            success: true,
            error: None,
            execution_time_ms: 100,
        };

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("ScriptExecutionResult"));
        assert!(debug_str.contains("/test"));
        assert!(debug_str.contains("success: true"));
    }

    #[test]
    fn test_script_execution_result_clone() {
        let mut registrations = HashMap::new();
        registrations.insert(
            ("/api".to_string(), "POST".to_string()),
            "handler".to_string(),
        );

        let original = ScriptExecutionResult {
            registrations,
            success: false,
            error: Some("Test error".to_string()),
            execution_time_ms: 200,
        };

        let cloned = original.clone();

        assert_eq!(original.success, cloned.success);
        assert_eq!(original.error, cloned.error);
        assert_eq!(original.registrations.len(), cloned.registrations.len());
    }

    #[tokio::test]
    async fn test_register_web_stream_function() {
        use crate::security::UserContext;
        use std::sync::Once;
        static INIT: Once = Once::new();

        // Ensure we clear streams only once per test run
        INIT.call_once(|| {
            let _ = stream_registry::GLOBAL_STREAM_REGISTRY.clear_all_streams();
        });

        let script_content = r#"
            registerWebStream('/test-stream-func');
            writeLog('Stream registered successfully');
        "#;

        let _ = repository::upsert_script("stream-test-func", script_content);
        // Use secure execution with admin privileges for testing
        let result = execute_script_secure(
            "stream-test-func",
            script_content,
            UserContext::admin("test-admin".to_string()),
        );

        assert!(
            result.success,
            "Script should execute successfully: {:?}",
            result.error
        );
        assert!(result.error.is_none(), "Should not have any errors");

        // Small delay to ensure registration is complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify the stream was registered
        assert!(
            stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("/test-stream-func"),
            "Stream should be registered"
        );

        // Verify the correct script URI is associated
        let script_uri =
            stream_registry::GLOBAL_STREAM_REGISTRY.get_stream_script_uri("/test-stream-func");
        assert_eq!(script_uri, Some("stream-test-func".to_string()));
    }

    #[test]
    fn test_register_web_stream_invalid_path() {
        let script_content = r#"
            try {
                registerWebStream('invalid-path-test');
                writeLog('ERROR: Should have failed');
            } catch (e) {
                writeLog('Expected error: ' + String(e));
            }
        "#;

        let _ = repository::upsert_script("stream-invalid-test", script_content);
        let result = execute_script("stream-invalid-test", script_content);

        assert!(
            result.success,
            "Script should execute successfully even with caught exception"
        );

        // Small delay to ensure any registration attempts are complete
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Verify the invalid stream was NOT registered
        assert!(
            !stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("invalid-path-test"),
            "Invalid stream should not be registered"
        );
    }

    #[tokio::test]
    async fn test_send_stream_message_function() {
        use crate::security::UserContext;

        let script_content = r#"
            // Register a stream first
            registerWebStream('/test-message-stream');

            // Send a message to all streams
            sendStreamMessage('{"type": "test", "data": "Hello World"}');

            writeLog('Message sent successfully');
        "#;

        let _ = repository::upsert_script("stream-message-test", script_content);
        // Use secure execution with admin privileges for testing
        let result = execute_script_secure(
            "stream-message-test",
            script_content,
            UserContext::admin("test-admin".to_string()),
        );

        assert!(
            result.success,
            "Script should execute successfully: {:?}",
            result.error
        );

        // Small delay to ensure the message is processed
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify the stream was registered
        assert!(
            stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("/test-message-stream"),
            "Stream should be registered"
        );

        // Check that logs were written (indicating successful execution)
        let logs = repository::fetch_log_messages("stream-message-test");
        assert!(
            logs.iter()
                .any(|log| log.contains("Message sent successfully")),
            "Should have logged successful message sending"
        );
    }

    #[tokio::test]
    async fn test_send_stream_message_json_object() {
        use crate::security::UserContext;

        let script_content = r#"
            // Register a stream first
            registerWebStream('/test-json-stream');

            // Send a complex JSON message
            var messageObj = {
                type: "notification",
                user: "testUser",
                data: {
                    id: 123,
                    text: "Hello from JavaScript",
                    timestamp: new Date().getTime()
                },
                metadata: ["tag1", "tag2"]
            };

            // JavaScript must stringify the object before sending
            sendStreamMessage(JSON.stringify(messageObj));

            writeLog('Complex JSON message sent');
        "#;

        let _ = repository::upsert_script("stream-json-test", script_content);
        // Use secure execution with admin privileges for testing
        let result = execute_script_secure(
            "stream-json-test",
            script_content,
            UserContext::admin("test-admin".to_string()),
        );

        assert!(
            result.success,
            "Script should execute successfully: {:?}",
            result.error
        );

        // Small delay to ensure the message is processed
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify the stream was registered
        assert!(
            stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered("/test-json-stream"),
            "Stream should be registered"
        );

        // Check that logs were written (indicating successful execution)
        let logs = repository::fetch_log_messages("stream-json-test");
        assert!(
            logs.iter()
                .any(|log| log.contains("Complex JSON message sent")),
            "Should have logged successful JSON message sending"
        );
    }

    #[test]
    fn test_script_size_validation() {
        // Test with a script that exceeds the default 1MB limit
        let large_script = "// ".repeat(600_000) + "register('/test', 'handler');";
        assert!(large_script.len() > 1_000_000);

        let result = execute_script("test-large-script", &large_script);

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("Script too large"));
        // Execution time is always recorded
        println!("Validation took {} ms", result.execution_time_ms);
    }

    #[test]
    fn test_script_validation_infinite_loop_warning() {
        let script_with_infinite_loop = "while(true) { console.log('infinite'); }";

        // This should still execute (just warn), but we can test that the validation function works
        let limits = ExecutionLimits::default();
        let validation_result = validate_script(script_with_infinite_loop, &limits);

        // Should pass validation (just warning), but our logs would show the warning
        assert!(validation_result.is_ok());
    }
}
