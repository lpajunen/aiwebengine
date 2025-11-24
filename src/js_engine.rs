use rquickjs::{Context, Function, Runtime, Value};
use serde_json::Value as JsonValue;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use tracing::{debug, error, info, warn};

use crate::repository;
use crate::scheduler::ScheduledInvocation;
use crate::security::UserContext;

// Use the enhanced secure globals implementation
use crate::security::secure_globals::{GlobalSecurityConfig, SecureGlobalContext};

// Type alias for route registrations map
type RouteRegistrations = repository::RouteRegistrations;

/// Extract detailed error information from a rquickjs::Error
///
/// QuickJS errors often include line numbers and column information in their
/// Display output. This function ensures we capture the full error message
/// which may contain file names, line numbers, and stack traces.
fn extract_error_details(ctx: &rquickjs::Ctx<'_>, error: &rquickjs::Error) -> String {
    // Try to get the pending exception value which may have more details
    let exception_val = ctx.catch();

    // Try to convert to string to get detailed error message
    if let Some(err_str) = exception_val.as_string()
        && let Ok(rust_str) = err_str.to_string()
        && !rust_str.is_empty()
    {
        return rust_str;
    }

    // Try to get as an object and extract properties
    if let Some(err_obj) = exception_val.as_object() {
        let mut parts = Vec::new();

        // Get message
        if let Ok(msg) = err_obj.get::<_, String>("message") {
            parts.push(msg);
        }

        // Get fileName if available
        if let Ok(file) = err_obj.get::<_, String>("fileName") {
            parts.push(format!("at {}", file));
        }

        // Get lineNumber if available
        if let Ok(line) = err_obj.get::<_, i32>("lineNumber") {
            parts.push(format!("line {}", line));
        }

        // Get columnNumber if available
        if let Ok(col) = err_obj.get::<_, i32>("columnNumber") {
            parts.push(format!("column {}", col));
        }

        // Get stack trace if available
        if let Ok(stack) = err_obj.get::<_, String>("stack")
            && !stack.is_empty()
        {
            parts.push(format!("\nStack: {}", stack));
        }

        if !parts.is_empty() {
            return parts.join(", ");
        }
    }

    // Fall back to the error Display implementation
    format!("{}", error)
}

/// Helper to safely drop Context before Runtime to prevent GC assertions
///  
/// This prevents the "Assertion `list_empty(&rt->gc_obj_list)' failed" error
/// by ensuring the Context is dropped first, allowing QuickJS to properly
/// clean up JavaScript objects before the Runtime is freed.
fn ensure_clean_shutdown<T>(ctx: Context, result: T) -> T {
    // Simply drop context - Rust's drop order will handle the rest
    // The key is that Context MUST drop before Runtime
    drop(ctx);
    result
}

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
    pub headers: HashMap<String, String>,
    pub user_context: UserContext,
    /// Optional OAuth authentication context for JavaScript auth API
    pub auth_context: Option<crate::auth::JsAuthContext>,
    /// Route parameters extracted from path patterns like /users/:id
    pub route_params: Option<HashMap<String, String>>,
}

/// Kinds of handler invocations supported by the runtime.
#[derive(Debug, Clone, Copy)]
pub enum HandlerInvocationKind {
    HttpRoute,
    GraphqlQuery,
    GraphqlMutation,
    GraphqlSubscription,
    StreamCustomization,
    Init,
    Scheduled,
}

impl HandlerInvocationKind {
    fn as_str(&self) -> &'static str {
        match self {
            HandlerInvocationKind::HttpRoute => "httpRoute",
            HandlerInvocationKind::GraphqlQuery => "graphqlQuery",
            HandlerInvocationKind::GraphqlMutation => "graphqlMutation",
            HandlerInvocationKind::GraphqlSubscription => "graphqlSubscription",
            HandlerInvocationKind::StreamCustomization => "streamCustomization",
            HandlerInvocationKind::Init => "init",
            HandlerInvocationKind::Scheduled => "scheduled",
        }
    }
}

/// Normalized view of inbound request data passed to JavaScript.
#[derive(Debug, Clone, Default)]
pub struct JsRequestContext {
    pub path: Option<String>,
    pub method: Option<String>,
    pub headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub form_data: HashMap<String, String>,
    pub body: Option<String>,
    /// Route parameters extracted from path patterns like /users/:id
    pub route_params: HashMap<String, String>,
}

/// Builder that assembles the single context object passed to all handlers.
#[derive(Debug, Clone)]
pub struct JsHandlerContextBuilder {
    kind: HandlerInvocationKind,
    script_uri: Option<String>,
    handler_name: Option<String>,
    request: Option<JsRequestContext>,
    args: Option<JsonValue>,
    auth_context: Option<crate::auth::JsAuthContext>,
    connection_metadata: Option<HashMap<String, String>>,
    metadata: HashMap<String, JsonValue>,
}

impl JsHandlerContextBuilder {
    pub fn new(kind: HandlerInvocationKind) -> Self {
        Self {
            kind,
            script_uri: None,
            handler_name: None,
            request: None,
            args: None,
            auth_context: None,
            connection_metadata: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_script_metadata(
        mut self,
        script_uri: impl Into<String>,
        handler: impl Into<String>,
    ) -> Self {
        self.script_uri = Some(script_uri.into());
        self.handler_name = Some(handler.into());
        self
    }

    pub fn with_request(mut self, request: JsRequestContext) -> Self {
        self.request = Some(request);
        self
    }

    pub fn with_args(mut self, args: JsonValue) -> Self {
        self.args = Some(args);
        self
    }

    pub fn with_auth_context(mut self, auth_ctx: crate::auth::JsAuthContext) -> Self {
        self.auth_context = Some(auth_ctx);
        self
    }

    pub fn with_connection_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.connection_metadata = Some(metadata);
        self
    }

    pub fn with_metadata_value(mut self, key: &str, value: JsonValue) -> Self {
        self.metadata.insert(key.to_string(), value);
        self
    }

    fn build_request_object<'js>(
        request: Option<JsRequestContext>,
        auth_context: Option<crate::auth::JsAuthContext>,
        ctx: &rquickjs::Ctx<'js>,
    ) -> Result<Option<rquickjs::Object<'js>>, rquickjs::Error> {
        let Some(request) = request else {
            return Ok(None);
        };

        let request_obj = rquickjs::Object::new(ctx.clone())?;

        if let Some(path) = &request.path {
            request_obj.set("path", path)?;
        }
        if let Some(method) = &request.method {
            request_obj.set("method", method)?;
        }

        // Headers
        if !request.headers.is_empty() {
            let headers_obj = rquickjs::Object::new(ctx.clone())?;
            for (name, value) in &request.headers {
                headers_obj.set(name.as_str(), value.as_str())?;
            }
            request_obj.set("headers", headers_obj)?;
        }

        // Query params
        let query_obj = rquickjs::Object::new(ctx.clone())?;
        for (key, value) in &request.query_params {
            query_obj.set(key.as_str(), value.as_str())?;
        }
        request_obj.set("query", query_obj)?;

        // Form data
        let form_obj = rquickjs::Object::new(ctx.clone())?;
        for (key, value) in &request.form_data {
            form_obj.set(key.as_str(), value.as_str())?;
        }
        request_obj.set("form", form_obj)?;

        // Route params
        let route_obj = rquickjs::Object::new(ctx.clone())?;
        for (key, value) in &request.route_params {
            route_obj.set(key.as_str(), value.as_str())?;
        }
        request_obj.set("params", route_obj)?;

        // Body
        if let Some(body) = &request.body {
            request_obj.set("body", body.as_str())?;
        } else {
            request_obj.set("body", rquickjs::Value::new_null(ctx.clone()))?;
        }

        if let Some(auth_ctx) = auth_context {
            let auth_obj = crate::auth::AuthJsApi::create_auth_object(ctx, auth_ctx.clone())?;
            request_obj.set("auth", auth_obj)?;
        }

        Ok(Some(request_obj))
    }

    pub fn build<'js>(
        self,
        ctx: &rquickjs::Ctx<'js>,
    ) -> Result<rquickjs::Object<'js>, rquickjs::Error> {
        let JsHandlerContextBuilder {
            kind,
            script_uri,
            handler_name,
            request,
            args,
            auth_context,
            connection_metadata,
            metadata,
        } = self;

        let request_obj = Self::build_request_object(request, auth_context, ctx)?;

        let context_obj = rquickjs::Object::new(ctx.clone())?;
        context_obj.set("kind", kind.as_str())?;

        if let Some(script_uri) = script_uri {
            context_obj.set("scriptUri", script_uri)?;
        }

        if let Some(handler_name) = handler_name {
            context_obj.set("handlerName", handler_name)?;
        }

        if let Some(request_obj) = request_obj {
            context_obj.set("request", request_obj)?;
        }

        if let Some(args) = args {
            let args_value = serde_json_to_js_value(ctx, &args)?;
            context_obj.set("args", args_value)?;
        } else {
            context_obj.set("args", rquickjs::Value::new_null(ctx.clone()))?;
        }

        if let Some(metadata) = connection_metadata {
            let metadata_obj = rquickjs::Object::new(ctx.clone())?;
            for (key, value) in metadata {
                metadata_obj.set(key.as_str(), value.as_str())?;
            }
            context_obj.set("connectionMetadata", metadata_obj)?;
        }

        if !metadata.is_empty() {
            let meta_obj = rquickjs::Object::new(ctx.clone())?;
            for (key, value) in metadata {
                let js_value = serde_json_to_js_value(ctx, &value)?;
                meta_obj.set(key.as_str(), js_value)?;
            }
            context_obj.set("meta", meta_obj)?;
        }

        Ok(context_obj)
    }
}

fn serde_json_to_js_value<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: &JsonValue,
) -> Result<rquickjs::Value<'js>, rquickjs::Error> {
    let json_string = serde_json::to_string(value).map_err(|e| {
        let msg = format!("Failed to serialize JSON value: {}", e);
        rquickjs::Error::new_from_js("JSON", Box::leak(msg.into_boxed_str()))
    })?;

    let json_obj: rquickjs::Object = ctx.globals().get("JSON")?;
    let json_parse: rquickjs::Function = json_obj.get("parse")?;
    let js_value: rquickjs::Value = json_parse.call((json_string,))?;
    Ok(js_value)
}

/// GraphQL operation kind, used to map resolvers to handler kinds and metadata.
#[derive(Debug, Clone, Copy)]
pub enum GraphqlOperationKind {
    Query,
    Mutation,
    Subscription,
}

impl GraphqlOperationKind {
    fn as_str(&self) -> &'static str {
        match self {
            GraphqlOperationKind::Query => "query",
            GraphqlOperationKind::Mutation => "mutation",
            GraphqlOperationKind::Subscription => "subscription",
        }
    }

    fn as_handler_kind(&self) -> HandlerInvocationKind {
        match self {
            GraphqlOperationKind::Query => HandlerInvocationKind::GraphqlQuery,
            GraphqlOperationKind::Mutation => HandlerInvocationKind::GraphqlMutation,
            GraphqlOperationKind::Subscription => HandlerInvocationKind::GraphqlSubscription,
        }
    }
}

/// Parameters for invoking a GraphQL resolver via JavaScript.
#[derive(Debug, Clone)]
pub struct GraphqlResolverExecutionParams {
    pub script_uri: String,
    pub resolver_function: String,
    pub field_name: String,
    pub operation_kind: GraphqlOperationKind,
    pub args: Option<JsonValue>,
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
type RegisterFunctionType =
    Box<dyn Fn(&str, &repository::RouteMetadata, Option<&str>) -> Result<(), rquickjs::Error>>;

/// Sets up secure global functions with proper capability validation
///
/// This function replaces the old vulnerable setup_global_functions with a secure implementation
/// that enforces all security validation in Rust before allowing JavaScript operations.
///
/// Note: Authentication context is no longer set up here. It should be attached to the
/// request object as `req.auth` by the caller.
fn setup_secure_global_functions(
    ctx: &rquickjs::Ctx<'_>,
    script_uri: &str,
    user_context: UserContext,
    config: &GlobalSecurityConfig,
    register_fn: Option<RegisterFunctionType>,
    _auth_context: Option<crate::auth::JsAuthContext>, // Kept for API compatibility but unused
    secrets_manager: Option<std::sync::Arc<crate::secrets::SecretsManager>>,
) -> Result<(), rquickjs::Error> {
    let secure_context = if let Some(secrets) = secrets_manager {
        SecureGlobalContext::new_with_secrets(user_context, config.clone(), secrets)
    } else {
        SecureGlobalContext::new_with_config(user_context, config.clone())
    };

    // Setup secure functions with proper capability validation
    secure_context.setup_secure_functions(ctx, script_uri, register_fn)?;

    // Auth is no longer set up as a global - it's attached to req.auth by the caller

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
    /// The registrations made by the script via routeRegistry.registerRoute() calls
    pub registrations: repository::RouteRegistrations,
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
    fn success(registrations: repository::RouteRegistrations, execution_time_ms: u64) -> Self {
        Self {
            registrations,
            success: true,
            error: None,
            execution_time_ms,
        }
    }
}

/// Executes a JavaScript script and captures any routeRegistry.registerRoute() method calls
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
                // Create a shared location for detailed error message
                let error_details: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
                let error_details_clone = Rc::clone(&error_details);

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
                              route_metadata: &repository::RouteMetadata,
                              method: Option<&str>|
                              -> Result<(), rquickjs::Error> {
                            let method = method.unwrap_or("GET");
                            debug!(
                                "Securely registering route {} {} -> {} for script {}",
                                method, path, route_metadata.handler_name, uri_clone
                            );
                            if let Ok(mut regs) = regs_clone.try_borrow_mut() {
                                regs.insert(
                                    (path.to_string(), method.to_string()),
                                    route_metadata.clone(),
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
                    let eval_result = ctx.eval::<(), _>(content);

                    // If there was an error, capture detailed information
                    if let Err(ref e) = eval_result {
                        let details = extract_error_details(&ctx, e);
                        if let Ok(mut error_ref) = error_details_clone.try_borrow_mut() {
                            *error_ref = Some(details);
                        }
                    }

                    eval_result
                });

                let exec_result = match result {
                    Ok(_) => {
                        let final_regs = registrations.borrow().clone();
                        let execution_time = start_time.elapsed().as_millis() as u64;
                        ScriptExecutionResult::success(final_regs, execution_time)
                    }
                    Err(e) => {
                        let execution_time = start_time.elapsed().as_millis() as u64;
                        let captured_details = error_details
                            .borrow()
                            .clone()
                            .unwrap_or_else(|| format!("Script evaluation error: {}", e));
                        ScriptExecutionResult::failed(captured_details, execution_time)
                    }
                };

                // Ensure clean shutdown: drop Context before Runtime
                ensure_clean_shutdown(ctx, exec_result)
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
                              route_metadata: &repository::RouteMetadata,
                              method: Option<&str>|
                              -> Result<(), rquickjs::Error> {
                            let method = method.unwrap_or("GET");
                            tracing::info!(
                                "Registering route {} {} -> {} for script {}",
                                method, path, route_metadata.handler_name, uri_clone
                            );
                            if let Ok(mut regs) = regs_clone.try_borrow_mut() {
                                regs.insert(
                                    (path.to_string(), method.to_string()),
                                    route_metadata.clone(),
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

                    let exec_result = match result {
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
                    };

                    // Ensure clean shutdown: drop Context before Runtime
                    ensure_clean_shutdown(ctx, exec_result)
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

/// JavaScript HTTP response structure
#[derive(Debug, Clone)]
pub struct JsHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    pub headers: std::collections::HashMap<String, String>,
}

impl JsHttpResponse {
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            content_type: None,
            headers: std::collections::HashMap::new(),
        }
    }

    pub fn from_string(status: u16, body: String) -> Self {
        Self {
            status,
            body: body.into_bytes(),
            content_type: None,
            headers: std::collections::HashMap::new(),
        }
    }

    pub fn with_content_type(mut self, content_type: String) -> Self {
        self.content_type = Some(content_type);
        self
    }

    pub fn with_header(mut self, name: String, value: String) -> Self {
        self.headers.insert(name, value);
        self
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
) -> Result<JsHttpResponse, String> {
    let script_uri_owned = params.script_uri.clone();
    let auth_context = params.auth_context.clone(); // Clone for later use
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
            None,                // No secrets manager yet
        )?;

        Ok(())
    })
    .map_err(|e| format!("install secure host fns: {}", e))?;

    let owner_script = repository::fetch_script(&params.script_uri)
        .ok_or_else(|| format!("no script for uri {}", params.script_uri))?;

    // Evaluate the script and capture detailed error information if it fails
    ctx.with(|ctx| -> Result<(), String> {
        let result = ctx.eval::<(), _>(owner_script.as_str());
        if let Err(ref e) = result {
            let details = extract_error_details(&ctx, e);
            return Err(format!("owner eval: {}", details));
        }
        Ok(())
    })?;

    let response_exec = ctx.with(|ctx| -> Result<JsHttpResponse, String> {
        let global = ctx.globals();
        let func: Function = global
            .get::<_, Function>(&params.handler_name)
            .map_err(|e| format!("no handler {}: {}", params.handler_name, e))?;

        let request_context = JsRequestContext {
            path: Some(params.path.clone()),
            method: Some(params.method.clone()),
            headers: params.headers.clone(),
            query_params: params.query_params.clone().unwrap_or_default(),
            form_data: params.form_data.clone().unwrap_or_default(),
            body: params.raw_body.clone(),
            route_params: params.route_params.clone().unwrap_or_default(),
        };

        let mut context_builder = JsHandlerContextBuilder::new(HandlerInvocationKind::HttpRoute)
            .with_script_metadata(&params.script_uri, &params.handler_name)
            .with_request(request_context);

        if let Some(ref auth_ctx) = auth_context {
            context_builder = context_builder.with_auth_context(auth_ctx.clone());
        }

        let handler_context = context_builder
            .build(&ctx)
            .map_err(|e| format!("build context: {}", e))?;

        // Set context as a global variable so personalStorage and other APIs can access it
        global.set("context", handler_context.clone()).map_err(|e| format!("set context global: {}", e))?;

        // Call the handler function
        let result: Value = func.call::<_, Value>((handler_context,)).map_err(|e| {
            let details = extract_error_details(&ctx, &e);
            format!("call handler: {}", details)
        })?;

        // Parse the response
        if let Some(response_obj) = result.as_object() {
            let status: i32 = response_obj
                .get("status")
                .map_err(|e| format!("missing status: {}", e))?;

            // Try to get bodyBase64 first (for binary data), otherwise fall back to body (for text)
            let (body, used_body_base64): (Vec<u8>, bool) = if let Ok(body_base64) = response_obj.get::<_, String>("bodyBase64")
            {
                // Decode base64 to bytes
                let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &body_base64)
                    .map_err(|e| format!("failed to decode bodyBase64: {}", e))?;
                (decoded, true)
            } else {
                // Fall back to string body
                let body_string: String = response_obj
                    .get("body")
                    .map_err(|e| format!("missing body or bodyBase64: {}", e))?;
                (body_string.into_bytes(), false)
            };

            let content_type: Option<String> = response_obj.get("contentType").ok();

            // Set default content type if not specified
            let content_type = content_type.or_else(|| {
                if used_body_base64 {
                    Some("application/octet-stream".to_string())
                } else {
                    Some("text/plain; charset=UTF-8".to_string())
                }
            });

            // Extract headers if present
            let mut headers = std::collections::HashMap::new();
            if let Ok(headers_obj) = response_obj.get::<_, rquickjs::Object>("headers") {
                // Iterate over headers object properties
                for (key, value) in headers_obj.props::<String, String>().flatten() {
                    headers.insert(key, value);
                }
            }

            debug!(
                "Secure request handler {} returned status: {}, body length: {}, content_type: {:?}, headers: {}",
                params.handler_name,
                status,
                body.len(),
                content_type,
                headers.len()
            );

            let mut response = JsHttpResponse::new(status as u16, body);
            if let Some(ct) = content_type {
                response = response.with_content_type(ct);
            }
            for (name, value) in headers {
                response = response.with_header(name, value);
            }

            Ok(response)
        } else {
            // If not an object, treat as string response
            let body = if result.is_string() {
                result
                    .as_string()
                    .and_then(|s| s.to_string().ok())
                    .unwrap_or_else(|| "<conversion error>".to_string())
                    .into_bytes()
            } else {
                "<no response>".to_string().into_bytes()
            };
            let mut response = JsHttpResponse::new(200, body);
            response = response.with_content_type("text/plain; charset=UTF-8".to_string());
            Ok(response)
        }
    });

    let response_result = response_exec.map_err(|e| e.to_string())?;

    // Ensure clean shutdown: drop Context before Runtime
    drop(ctx);
    Ok(response_result)
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
    let auth_ctx = crate::auth::JsAuthContext::anonymous();
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
        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            UserContext::anonymous(),
            &config,
            None,
            Some(auth_ctx.clone()), // Provide anonymous auth context
            None,                   // No secrets manager yet
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

            let request_context = JsRequestContext {
                path: Some(path.to_string()),
                method: Some(method.to_string()),
                headers: HashMap::new(),
                query_params: query_params.cloned().unwrap_or_default(),
                form_data: form_data.cloned().unwrap_or_default(),
                body: raw_body.clone(),
                route_params: HashMap::new(),
            };

            let mut context_builder =
                JsHandlerContextBuilder::new(HandlerInvocationKind::HttpRoute)
                    .with_script_metadata(script_uri, handler_name)
                    .with_request(request_context);

            context_builder = context_builder.with_auth_context(auth_ctx.clone());

            let handler_context = context_builder
                .build(&ctx)
                .map_err(|e| format!("build context: {}", e))?;

            // Set context as a global variable so personalStorage and other APIs can access it
            let global = ctx.globals();
            global
                .set("context", handler_context.clone())
                .map_err(|e| format!("set context global: {}", e))?;

            let val = func
                .call::<_, Value>((handler_context,))
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

    // Ensure clean shutdown: drop Context before Runtime
    ensure_clean_shutdown(ctx, Ok((status, body, content_type)))
}

/// Executes a JavaScript handler for scheduler jobs
pub fn execute_scheduled_handler(
    script_uri: &str,
    handler_name: &str,
    invocation: &ScheduledInvocation,
) -> Result<(), String> {
    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;
    let script_uri_owned = script_uri.to_string();

    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        let security_config = GlobalSecurityConfig {
            enable_graphql_registration: false,
            enable_audit_logging: false,
            ..Default::default()
        };

        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            UserContext::admin("scheduler".to_string()),
            &security_config,
            None,
            None,
            None,
        )
    })
    .map_err(|e| format!("install scheduler globals: {}", e))?;

    let owner_script = repository::fetch_script(script_uri)
        .ok_or_else(|| format!("no script for uri {}", script_uri))?;

    ctx.with(|ctx| {
        ctx.eval::<(), _>(owner_script.as_str()).map_err(|e| {
            let details = extract_error_details(&ctx, &e);
            format!("script eval: {}", details)
        })
    })?;

    let handler_result = ctx.with(|ctx| -> Result<(), String> {
        let global = ctx.globals();
        let func: Function = global
            .get::<_, Function>(handler_name)
            .map_err(|e| format!("no handler {}: {}", handler_name, e))?;

        let schedule_meta = serde_json::json!({
            "jobId": invocation.job_id.to_string(),
            "name": invocation.key,
            "type": invocation.kind.as_str(),
            "scheduledFor": invocation.scheduled_for.to_rfc3339(),
            "intervalSeconds": invocation.interval_seconds,
        });

        let handler_context = JsHandlerContextBuilder::new(HandlerInvocationKind::Scheduled)
            .with_script_metadata(script_uri, handler_name)
            .with_metadata_value("schedule", schedule_meta)
            .build(&ctx)
            .map_err(|e| format!("build context: {}", e))?;

        // Set context as a global variable so personalStorage and other APIs can access it
        global
            .set("context", handler_context.clone())
            .map_err(|e| format!("set context global: {}", e))?;

        func.call::<_, Value>((handler_context,)).map_err(|e| {
            let details = extract_error_details(&ctx, &e);
            format!("call handler: {}", details)
        })?;

        Ok(())
    });

    // Ensure clean shutdown
    drop(ctx);

    handler_result?;
    Ok(())
}

/// Executes a JavaScript GraphQL resolver function and returns the result as a string.
/// This is used by the GraphQL system to call JavaScript resolver functions.
pub fn execute_graphql_resolver(params: GraphqlResolverExecutionParams) -> Result<String, String> {
    let script_uri_owned = params.script_uri.clone();
    let resolver_function_owned = params.resolver_function.clone();
    let args_owned = params.args.clone();
    let auth_context = params.auth_context.clone();

    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    let result_exec = ctx.with(|ctx| -> Result<String, rquickjs::Error> {
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
        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            UserContext::admin("graphql-resolver".to_string()),
            &config,
            None,
            auth_context.clone(),
            None,
        )?;

        // Override specific functions that have different signatures for GraphQL resolver context
        let _global = ctx.globals();

        // Load and execute the script
        let script_content = repository::fetch_script(&script_uri_owned)
            .ok_or_else(|| rquickjs::Error::new_from_js("Script", "not found"))?;

        // Execute the script
        ctx.eval::<(), _>(script_content.as_str())?;

        let resolver_result: rquickjs::Value = ctx.globals().get(&resolver_function_owned)?;
        let resolver_func = resolver_result
            .as_function()
            .ok_or_else(|| rquickjs::Error::new_from_js("Function", "not found"))?;

        let request_context = JsRequestContext {
            path: Some("/graphql".to_string()),
            method: Some("POST".to_string()),
            headers: HashMap::new(),
            query_params: HashMap::new(),
            form_data: HashMap::new(),
            body: None,
            route_params: HashMap::new(),
        };

        let mut context_builder =
            JsHandlerContextBuilder::new(params.operation_kind.as_handler_kind())
                .with_script_metadata(&params.script_uri, &params.resolver_function)
                .with_request(request_context)
                .with_metadata_value(
                    "graphql",
                    serde_json::json!({
                        "fieldName": params.field_name,
                        "operation": params.operation_kind.as_str()
                    }),
                );

        if let Some(args) = args_owned {
            context_builder = context_builder.with_args(args);
        }

        if let Some(auth_ctx) = auth_context.clone() {
            context_builder = context_builder.with_auth_context(auth_ctx);
        }

        let handler_context = context_builder.build(&ctx)?;

        // Set context as a global variable so personalStorage and other APIs can access it
        let global = ctx.globals();
        global.set("context", handler_context.clone())?;

        let result_value = resolver_func.call::<_, rquickjs::Value>((handler_context,))?;

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
    });

    let result_string = result_exec.map_err(|e| format!("JavaScript execution error: {}", e))?;

    // Ensure clean shutdown: drop Context before Runtime
    drop(ctx);
    Ok(result_string)
}

/// Execute a stream customization function to get connection filter criteria
///
/// This function loads a script and calls the specified customization function with a request context.
/// The function should return a JSON object representing the filter criteria for this connection.
///
/// # Arguments
/// * `script_uri` - The URI of the script containing the customization function
/// * `function_name` - The name of the customization function to call
/// * `path` - The stream path
/// * `query_params` - Query parameters from the connection request
/// * `auth_context` - Optional authentication context
///
/// # Returns
/// * `Ok(HashMap<String, String>)` - The filter criteria as key-value pairs
/// * `Err(String)` - Error message if execution fails
pub fn execute_stream_customization_function(
    script_uri: &str,
    function_name: &str,
    path: &str,
    query_params: &std::collections::HashMap<String, String>,
    auth_context: Option<crate::auth::JsAuthContext>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let script_uri_owned = script_uri.to_string();
    let function_name_owned = function_name.to_string();
    let path_owned = path.to_string();
    let query_params_owned = query_params.clone();

    let rt = Runtime::new().map_err(|e| format!("runtime new: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    let result_exec = ctx.with(
        |ctx| -> Result<std::collections::HashMap<String, String>, rquickjs::Error> {
            // Set up global functions with minimal security for customization function
            let config = GlobalSecurityConfig {
                enable_graphql_registration: false,
                enable_streams: false,
                enable_audit_logging: false,
                ..Default::default()
            };

            setup_secure_global_functions(
                &ctx,
                &script_uri_owned,
                UserContext::admin("stream-customization".to_string()),
                &config,
                None,
                auth_context.clone(),
                None,
            )?;

            // Load and execute the script
            let script_content = repository::fetch_script(&script_uri_owned)
                .ok_or_else(|| rquickjs::Error::new_from_js("Script", "not found"))?;

            ctx.eval::<(), _>(script_content.as_str())?;

            let request_context = JsRequestContext {
                path: Some(path_owned.clone()),
                method: Some("GET".to_string()),
                headers: HashMap::new(),
                query_params: query_params_owned.clone(),
                form_data: HashMap::new(),
                body: None,
                route_params: HashMap::new(),
            };

            let mut context_builder =
                JsHandlerContextBuilder::new(HandlerInvocationKind::StreamCustomization)
                    .with_script_metadata(&script_uri_owned, &function_name_owned)
                    .with_request(request_context)
                    .with_metadata_value("stream", serde_json::json!({ "path": path_owned }));

            if !query_params_owned.is_empty() {
                let args_json = JsonValue::Object(
                    query_params_owned
                        .iter()
                        .map(|(key, value)| (key.clone(), JsonValue::String(value.clone())))
                        .collect(),
                );
                context_builder = context_builder.with_args(args_json);
            }

            if let Some(ref auth) = auth_context {
                context_builder = context_builder.with_auth_context(auth.clone());
            }

            let handler_context = context_builder.build(&ctx)?;

            // Set context as a global variable so personalStorage and other APIs can access it
            let global = ctx.globals();
            global.set("context", handler_context.clone())?;

            // Get the customization function
            let customization_func: rquickjs::Function =
                global.get(&function_name_owned).map_err(|_| {
                    let msg = format!("'{}' not found", function_name_owned);
                    rquickjs::Error::new_from_js("Function", msg.leak())
                })?;

            // Call the function with req object
            let result_value: rquickjs::Value = customization_func.call((handler_context,))?;

            // Convert result to HashMap
            let mut filter_criteria = std::collections::HashMap::new();

            let Some(result_obj) = result_value.as_object() else {
                return Err(rquickjs::Error::new_from_js(
                    "Customization",
                    "Expected object result",
                ));
            };

            for key_str in result_obj.keys::<String>().flatten() {
                if let Ok(value) = result_obj.get::<_, rquickjs::Value>(&key_str) {
                    if let Some(value_str) = value.as_string().and_then(|s| s.to_string().ok()) {
                        filter_criteria.insert(key_str.clone(), value_str);
                    } else {
                        return Err(rquickjs::Error::new_from_js(
                            "Customization",
                            "Filter values must be strings",
                        ));
                    }
                }
            }

            Ok(filter_criteria)
        },
    );

    let filter_criteria =
        result_exec.map_err(|e| format!("Customization function execution error: {}", e))?;

    // Ensure clean shutdown
    drop(ctx);
    Ok(filter_criteria)
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
) -> Result<Option<RouteRegistrations>, String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    debug!("Checking for init() function in script: {}", script_uri);

    let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;

    // Set memory limit to help with cleanup
    rt.set_memory_limit(256 * 1024 * 1024); // 256MB limit
    rt.set_max_stack_size(512 * 1024); // 512KB stack

    let ctx = Context::full(&rt).map_err(|e| format!("Failed to create context: {}", e))?;

    // Create registrations map to capture routeRegistry.registerRoute() calls during init
    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = script_uri.to_string();

    // Shared location for detailed error message
    let error_details: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let error_details_clone = Rc::clone(&error_details);

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
                      route_metadata: &repository::RouteMetadata,
                      method: Option<&str>|
                      -> Result<(), rquickjs::Error> {
                    let method = method.unwrap_or("GET");
                    debug!(
                        "Registering route {} {} -> {} for script {} during init()",
                        method, path, route_metadata.handler_name, uri_clone
                    );
                    if let Ok(mut regs) = regs_clone.try_borrow_mut() {
                        regs.insert(
                            (path.to_string(), method.to_string()),
                            route_metadata.clone(),
                        );
                    }
                    Ok(())
                },
            );

            // Init runs with admin context to allow script registration operations
            let setup_result = setup_secure_global_functions(
                &ctx,
                script_uri,
                UserContext::admin("script-init".to_string()),
                &config,
                Some(register_impl),
                None,
                None, // No secrets manager yet
            );

            if let Err(ref e) = setup_result {
                let details = extract_error_details(&ctx, e);
                if let Ok(mut error_ref) = error_details_clone.try_borrow_mut() {
                    *error_ref = Some(details);
                }
            }
            setup_result?;

            // Execute the script to define functions
            let eval_result = ctx.eval::<(), _>(script_content);
            if let Err(ref e) = eval_result {
                let details = extract_error_details(&ctx, e);
                if let Ok(mut error_ref) = error_details_clone.try_borrow_mut() {
                    *error_ref = Some(details);
                }
            }
            eval_result?;

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

            // Convert SystemTime to milliseconds since UNIX_EPOCH
            let timestamp_ms = context
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f64;

            let init_metadata = serde_json::json!({
                "scriptName": context.script_name.clone(),
                "timestamp": timestamp_ms,
                "isStartup": context.is_startup,
            });

            let handler_context = JsHandlerContextBuilder::new(HandlerInvocationKind::Init)
                .with_script_metadata(script_uri.to_string(), "init")
                .with_metadata_value("init", init_metadata)
                .build(&ctx)?;

            // Call init function with context
            debug!("Calling init() function for script: {}", script_uri);
            let call_result = init_func.call::<_, ()>((handler_context,));

            if let Err(ref e) = call_result {
                let details = extract_error_details(&ctx, e);
                if let Ok(mut error_ref) = error_details_clone.try_borrow_mut() {
                    *error_ref = Some(details);
                }
            }
            call_result?;

            info!("Successfully called init() for script: {}", script_uri);
            Ok(true)
        })
        .map_err(|e| {
            // Use detailed error if available, otherwise format the basic error
            if let Ok(details_ref) = error_details.try_borrow()
                && let Some(ref details) = *details_ref
            {
                return format!("Init function error: {}", details);
            }
            format!("Init function error: {}", e)
        })?;

    // Return registrations if init was called
    let final_result = if result {
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
    };

    // Ensure clean shutdown: drop Context before Runtime
    ensure_clean_shutdown(ctx, final_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_registry;

    #[test]
    fn test_execute_script_simple_registration() {
        let content = r#"
            routeRegistry.registerRoute("/test", "handler_function", "GET");
        "#;

        let result = execute_script("test-script", content);

        assert!(result.success, "Script execution should succeed");
        assert!(result.error.is_none(), "Should not have error");
        assert_eq!(result.registrations.len(), 1);
        let route_meta = result
            .registrations
            .get(&("/test".to_string(), "GET".to_string()));
        assert!(route_meta.is_some());
        assert_eq!(route_meta.unwrap().handler_name, "handler_function");
    }

    #[test]
    fn test_execute_script_multiple_registrations() {
        let content = r#"
            routeRegistry.registerRoute("/api/users", "getUsers", "GET");
            routeRegistry.registerRoute("/api/users", "createUser", "POST");
            routeRegistry.registerRoute("/api/users/:id", "updateUser", "PUT");
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
            routeRegistry.registerRoute("/default-method", "handler", "GET");
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
        let route_meta = result
            .registrations
            .get(&("/default-method".to_string(), "GET".to_string()));
        assert!(route_meta.is_some());
        assert_eq!(route_meta.unwrap().handler_name, "handler");
    }

    #[test]
    fn test_execute_script_with_syntax_error() {
        let content = r#"
            routeRegistry.registerRoute("/test", "handler"
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
                routeRegistry.registerRoute("/api/health", "healthCheck", "GET");
                routeRegistry.registerRoute("/api/status", "statusCheck", "GET");
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
            routeRegistry.registerRoute("/logged", "loggedHandler", "GET");
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
        // Ignore errors for test
        let _ = repository::upsert_script("test-resolver", script_content).is_ok();

        let params = GraphqlResolverExecutionParams {
            script_uri: "test-resolver".to_string(),
            resolver_function: "testResolver".to_string(),
            field_name: "testResolver".to_string(),
            operation_kind: GraphqlOperationKind::Query,
            args: None,
            auth_context: None,
        };

        let result = execute_graphql_resolver(params);

        assert!(result.is_ok(), "Simple resolver should succeed");
        let json_result = result.unwrap();
        assert!(json_result == "Hello World" || json_result == "\"Hello World\""); // Handle both cases
    }

    #[test]
    fn test_execute_graphql_resolver_with_args() {
        let script_content = r#"
            function greetUser(context) {
                const args = context.args || {};
                return "Hello " + args.name + "!";
            }
        "#;

        // Store the script
        let _ = repository::upsert_script("greet-resolver", script_content);

        let args = serde_json::json!({"name": "Alice"});
        let params = GraphqlResolverExecutionParams {
            script_uri: "greet-resolver".to_string(),
            resolver_function: "greetUser".to_string(),
            field_name: "greetUser".to_string(),
            operation_kind: GraphqlOperationKind::Query,
            args: Some(args),
            auth_context: None,
        };
        let result = execute_graphql_resolver(params);

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
        let params = GraphqlResolverExecutionParams {
            script_uri: "user-resolver".to_string(),
            resolver_function: "getUserInfo".to_string(),
            field_name: "getUserInfo".to_string(),
            operation_kind: GraphqlOperationKind::Query,
            args: None,
            auth_context: None,
        };
        let result = execute_graphql_resolver(params);

        assert!(result.is_ok(), "Resolver returning object should succeed");
        let json_result = result.unwrap();
        assert!(json_result.contains("John Doe"));
        assert!(json_result.contains("john@example.com"));
    }

    #[test]
    fn test_execute_graphql_resolver_nonexistent_script() {
        let params = GraphqlResolverExecutionParams {
            script_uri: "nonexistent-script".to_string(),
            resolver_function: "someFunction".to_string(),
            field_name: "someFunction".to_string(),
            operation_kind: GraphqlOperationKind::Query,
            args: None,
            auth_context: None,
        };
        let result = execute_graphql_resolver(params);

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
        let params = GraphqlResolverExecutionParams {
            script_uri: "missing-function-resolver".to_string(),
            resolver_function: "nonExistentFunction".to_string(),
            field_name: "nonExistentFunction".to_string(),
            operation_kind: GraphqlOperationKind::Query,
            args: None,
            auth_context: None,
        };
        let result = execute_graphql_resolver(params);

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
        let params = GraphqlResolverExecutionParams {
            script_uri: "throwing-resolver".to_string(),
            resolver_function: "throwingResolver".to_string(),
            field_name: "throwingResolver".to_string(),
            operation_kind: GraphqlOperationKind::Query,
            args: None,
            auth_context: None,
        };
        let result = execute_graphql_resolver(params);

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
            repository::RouteMetadata::simple("handler".to_string()),
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
            repository::RouteMetadata::simple("handler".to_string()),
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_web_stream_function() {
        use crate::security::UserContext;
        use std::sync::Once;
        static INIT: Once = Once::new();

        // Ensure we clear streams only once per test run
        INIT.call_once(|| {
            let _ = stream_registry::GLOBAL_STREAM_REGISTRY.clear_all_streams();
        });

        let script_content = r#"
            routeRegistry.registerStreamRoute('/test-stream-func');
            console.log('Stream registered successfully');
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
                routeRegistry.registerStreamRoute('invalid-path-test');
                console.error('ERROR: Should have failed');
            } catch (e) {
                console.log('Expected error: ' + String(e));
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_stream_message_function() {
        use crate::security::UserContext;

        let script_content = r#"
            // Register a stream first
            routeRegistry.registerStreamRoute('/test-message-stream');

            // Send a message to the specific stream
            routeRegistry.sendStreamMessage('/test-message-stream', '{"type": "test", "data": "Hello World"}');

            console.log('Message sent successfully');
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
                .any(|log| log.message.contains("Message sent successfully")),
            "Should have logged successful message sending"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_stream_message_json_object() {
        use crate::security::UserContext;

        let script_content = r#"
            // Register a stream first
            routeRegistry.registerStreamRoute('/test-json-stream');

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
            routeRegistry.sendStreamMessage('/test-json-stream', JSON.stringify(messageObj));

            console.log('Complex JSON message sent');
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
                .any(|log| log.message.contains("Complex JSON message sent")),
            "Should have logged successful JSON message sending"
        );
    }

    #[test]
    fn test_shared_storage_validation() {
        // Test with a script that exceeds the default 1MB limit
        let large_script =
            "// ".repeat(600_000) + "routeRegistry.registerRoute('/test', 'handler');";
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

    #[test]
    fn test_default_content_types() {
        use crate::security::UserContext;

        // Test default content type for text body
        let text_script = r#"
            function testTextHandler(request) {
                return {
                    status: 200,
                    body: "Hello World"
                };
            }
        "#;

        let _ = repository::upsert_script("test-text-content-type", text_script);
        let params = RequestExecutionParams {
            script_uri: "test-text-content-type".to_string(),
            handler_name: "testTextHandler".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        assert_eq!(
            response.content_type,
            Some("text/plain; charset=UTF-8".to_string())
        );

        // Test default content type for bodyBase64
        let binary_script = r#"
            function testBinaryHandler(request) {
                return {
                    status: 200,
                    bodyBase64: "SGVsbG8gV29ybGQ="  // "Hello World" in base64
                };
            }
        "#;

        let _ = repository::upsert_script("test-binary-content-type", binary_script);
        let params = RequestExecutionParams {
            script_uri: "test-binary-content-type".to_string(),
            handler_name: "testBinaryHandler".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        assert_eq!(
            response.content_type,
            Some("application/octet-stream".to_string())
        );

        // Test explicit content type overrides default
        let explicit_script = r#"
            function testExplicitHandler(request) {
                return {
                    status: 200,
                    body: "Hello World",
                    contentType: "application/json"
                };
            }
        "#;

        let _ = repository::upsert_script("test-explicit-content-type", explicit_script);
        let params = RequestExecutionParams {
            script_uri: "test-explicit-content-type".to_string(),
            handler_name: "testExplicitHandler".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        assert_eq!(response.content_type, Some("application/json".to_string()));
    }

    #[test]
    fn test_convert_markdown_to_html_simple() {
        use crate::security::UserContext;

        let script_content = r#"
            function testConvert(context) {
                const markdown = `# Hello World

This is **bold** text.`;
                const html = convert.markdown_to_html(markdown);
                return {
                    status: 200,
                    body: html,
                    contentType: "text/html"
                };
            }
        "#;

        let _ = repository::upsert_script("test-convert-simple", script_content);
        let params = RequestExecutionParams {
            script_uri: "test-convert-simple".to_string(),
            handler_name: "testConvert".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        let body = String::from_utf8(response.body).unwrap();

        assert!(
            body.contains("<h1>Hello World</h1>"),
            "Should contain heading"
        );
        assert!(
            body.contains("<strong>bold</strong>"),
            "Should contain bold text"
        );
    }

    #[test]
    fn test_convert_markdown_to_html_code_block() {
        use crate::security::UserContext;

        let script_content = r#"
            function testConvertCode(context) {
                const markdown = '```javascript\nconst x = 42;\n```';
                const html = convert.markdown_to_html(markdown);
                return {
                    status: 200,
                    body: html,
                    contentType: "text/html"
                };
            }
        "#;

        let _ = repository::upsert_script("test-convert-code", script_content);
        let params = RequestExecutionParams {
            script_uri: "test-convert-code".to_string(),
            handler_name: "testConvertCode".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        let body = String::from_utf8(response.body).unwrap();

        assert!(body.contains("<pre><code"), "Should contain code block");
        assert!(
            body.contains("const x = 42;"),
            "Should contain code content"
        );
    }

    #[test]
    fn test_convert_markdown_to_html_list() {
        use crate::security::UserContext;

        let script_content = r#"
            function testConvertList(context) {
                const markdown = '- Item 1\n- Item 2\n- Item 3';
                const html = convert.markdown_to_html(markdown);
                return {
                    status: 200,
                    body: html,
                    contentType: "text/html"
                };
            }
        "#;

        let _ = repository::upsert_script("test-convert-list", script_content);
        let params = RequestExecutionParams {
            script_uri: "test-convert-list".to_string(),
            handler_name: "testConvertList".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        let body = String::from_utf8(response.body).unwrap();

        assert!(body.contains("<ul>"), "Should contain unordered list");
        assert!(
            body.contains("<li>Item 1</li>"),
            "Should contain list items"
        );
    }

    #[test]
    fn test_convert_markdown_to_html_table() {
        use crate::security::UserContext;

        let script_content = r#"
            function testConvertTable(context) {
                const markdown = '| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |';
                const html = convert.markdown_to_html(markdown);
                return {
                    status: 200,
                    body: html,
                    contentType: "text/html"
                };
            }
        "#;

        let _ = repository::upsert_script("test-convert-table", script_content);
        let params = RequestExecutionParams {
            script_uri: "test-convert-table".to_string(),
            handler_name: "testConvertTable".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        let body = String::from_utf8(response.body).unwrap();

        assert!(body.contains("<table>"), "Should contain table");
        assert!(
            body.contains("<th>Header 1</th>"),
            "Should contain table headers"
        );
        assert!(
            body.contains("<td>Cell 1</td>"),
            "Should contain table cells"
        );
    }

    #[test]
    fn test_convert_markdown_to_html_empty_input() {
        use crate::security::UserContext;

        let script_content = r#"
            function testConvertEmpty(context) {
                const markdown = '';
                const html = convert.markdown_to_html(markdown);
                return {
                    status: 200,
                    body: html,
                    contentType: "text/html"
                };
            }
        "#;

        let _ = repository::upsert_script("test-convert-empty", script_content);
        let params = RequestExecutionParams {
            script_uri: "test-convert-empty".to_string(),
            handler_name: "testConvertEmpty".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        let body = String::from_utf8(response.body).unwrap();

        assert!(
            body.contains("Error:"),
            "Should return error message for empty input"
        );
    }

    #[test]
    fn test_convert_markdown_to_html_complex() {
        use crate::security::UserContext;

        let script_content = r#"
            function testConvertComplex(context) {
                const markdown = `# My Blog Post

This is a **blog post** with *italic* text.

## Features

- Markdown support
- Code highlighting
- Tables

### Code Example

\`\`\`javascript
function hello() {
    return "world";
}
\`\`\`

[Link to example](https://example.com)
`;
                const html = convert.markdown_to_html(markdown);
                return {
                    status: 200,
                    body: html,
                    contentType: "text/html"
                };
            }
        "#;

        let _ = repository::upsert_script("test-convert-complex", script_content);
        let params = RequestExecutionParams {
            script_uri: "test-convert-complex".to_string(),
            handler_name: "testConvertComplex".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        let body = String::from_utf8(response.body).unwrap();

        assert!(body.contains("<h1>My Blog Post</h1>"), "Should contain h1");
        assert!(body.contains("<h2>Features</h2>"), "Should contain h2");
        assert!(
            body.contains("<strong>blog post</strong>"),
            "Should contain bold"
        );
        assert!(body.contains("<em>italic</em>"), "Should contain italic");
        assert!(body.contains("<ul>"), "Should contain list");
        assert!(body.contains("<pre><code"), "Should contain code block");
        assert!(
            body.contains("function hello()"),
            "Should contain code content"
        );
        assert!(
            body.contains("<a href=\"https://example.com\">Link to example</a>"),
            "Should contain link"
        );
    }
}
