use rquickjs::{Context, Function, Runtime, Value};
use serde_json::Value as JsonValue;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::module_loader;
use crate::repository;
use crate::scheduler::ScheduledInvocation;
use crate::script_test::{TestCaseResult, TestRunResult};
use crate::security::UserContext;

// Use the enhanced secure globals implementation
use crate::security::secure_globals::{
    CollectedRegistration, ConsoleLine, ConsoleSink, GlobalSecurityConfig, RegistrationKind,
    RegistrationSink, SecureGlobalContext,
};

// Type alias for route registrations map
type RouteRegistrations = repository::RouteRegistrations;

/// Transpile TypeScript/JSX/TSX to JavaScript if needed
fn transpile_if_needed(uri: &str, content: &str) -> Result<String, String> {
    module_loader::prepare_executable_program(uri, content)
        .map(|prepared| prepared.code)
        .map_err(|e| format!("Transpilation error: {}", e))
}

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

/// Execution limits derived from server configuration, set once at startup.
static CONFIGURED_LIMITS: OnceLock<ExecutionLimits> = OnceLock::new();

/// Stores the configured execution limits used by all JavaScript execution paths.
/// Returns false if limits were already configured.
pub fn configure_execution_limits(limits: ExecutionLimits) -> bool {
    CONFIGURED_LIMITS.set(limits).is_ok()
}

/// The execution limits currently in effect (configured at startup, or defaults).
pub fn current_execution_limits() -> ExecutionLimits {
    CONFIGURED_LIMITS.get().cloned().unwrap_or_default()
}

/// Whether per-request phase profiling is enabled (env `AIWEBENGINE_PROFILE_REQUESTS=1`).
///
/// Read once and cached. When enabled, [`execute_script_for_request_secure`] emits a
/// single structured log line per request breaking down where wall-clock time is spent.
fn request_profiling_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("AIWEBENGINE_PROFILE_REQUESTS")
                .ok()
                .as_deref(),
            Some("1") | Some("true") | Some("yes")
        )
    })
}

/// Creates a QuickJS runtime with memory, stack, and wall-clock limits enforced.
///
/// The interrupt handler is the only mechanism that can stop a runaway script
/// (e.g. `while(true) {}`); outer tokio timeouts abandon the blocking thread
/// but cannot terminate execution running on it.
fn create_sandboxed_runtime(limits: &ExecutionLimits) -> Result<Runtime, String> {
    let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    rt.set_memory_limit(limits.max_memory_mb * 1024 * 1024);
    rt.set_max_stack_size(512 * 1024);
    let deadline = Instant::now() + Duration::from_millis(limits.timeout_ms);
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    Ok(rt)
}

/// Upper bound on microtasks drained for one invocation.
///
/// The runtime's interrupt handler is the real guard: it stops a chain that
/// re-enqueues itself at the execution deadline, leaving the promise pending.
/// This cap only exists so a queue that somehow outruns the deadline check
/// cannot spin forever.
const MAX_DRAINED_JOBS: usize = 1_000_000;

/// Runs the microtask queue to a fixed point.
///
/// Scripts have no timers and every host call blocks rather than yielding, so
/// the queue always reaches a fixed point — there is nothing to wait *for*. A
/// promise still pending once this returns can never settle, and
/// [`unwrap_settled`] says so rather than hanging.
///
/// Must be called with no `Context::with` closure on the stack: the runtime
/// lock is not reentrant, and touching the runtime from inside a context
/// panics with "RefCell already borrowed".
///
/// Returns the messages of any jobs that threw. Those are unhandled
/// rejections — a promise chain with no `catch` — and deliberately do not fail
/// the invocation that spawned them, which mirrors how a browser reports
/// `unhandledrejection`.
fn drain_jobs(rt: &Runtime) -> Vec<String> {
    let mut unhandled = Vec::new();
    let mut drained = 0usize;

    while rt.is_job_pending() {
        if drained >= MAX_DRAINED_JOBS {
            warn!(
                drained,
                "microtask queue still not drained at the job cap; abandoning the rest"
            );
            break;
        }
        drained += 1;

        match rt.execute_pending_job() {
            Ok(true) => {}
            Ok(false) => break,
            Err(exception) => {
                // `JobException` carries the context the job threw in; the
                // exception itself is retrieved with `Ctx::catch`.
                let message = exception.0.with(|ctx| {
                    let caught = ctx.catch();
                    caught
                        .as_exception()
                        .and_then(|ex| ex.message())
                        .or_else(|| caught.as_string().and_then(|s| s.to_string().ok()))
                        .unwrap_or_else(|| "unhandled promise rejection".to_string())
                });
                unhandled.push(message);
            }
        }
    }

    unhandled
}

/// Logs whatever [`drain_jobs`] collected, attributing it to the script.
fn report_unhandled(script_uri: &str, unhandled: Vec<String>) {
    for message in unhandled {
        warn!(
            script = %script_uri,
            "unhandled promise rejection: {}", message
        );
    }
}

/// Wraps `value` in a native promise via `Promise.resolve`.
///
/// Normalising every handler result through this is what lets one code path
/// serve both a plain return value and a promise. It costs a synchronous
/// handler nothing — resolving with a non-thenable settles immediately, with
/// no job queued — while a thenable that is *not* a native promise (the shape
/// an awaitable `fetch` response has) becomes one that [`unwrap_settled`] can
/// read after the drain.
pub(crate) fn promise_resolve<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: Value<'js>,
) -> Result<rquickjs::Promise<'js>, String> {
    let promise_ctor: rquickjs::Object<'js> = ctx
        .globals()
        .get("Promise")
        .map_err(|e| format!("Promise global missing: {}", e))?;
    let resolve: Function<'js> = promise_ctor
        .get("resolve")
        .map_err(|e| format!("Promise.resolve missing: {}", e))?;
    // `Promise.resolve` reads its constructor off `this`, so the receiver has to
    // be bound explicitly; passing it as a plain argument leaves `this`
    // undefined and the call throws.
    resolve
        .call::<_, rquickjs::Promise<'js>>((rquickjs::function::This(promise_ctor.clone()), value))
        .map_err(|e| format!("Promise.resolve failed: {}", extract_error_details(ctx, &e)))
}

/// Whether settling an invocation should also close the database transaction
/// the script opened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TransactionHandling {
    /// Commit once the promise resolves, roll back if it rejects. What a
    /// handler that called `database.beginTransaction()` expects.
    Auto,
    /// Leave the transaction alone — the caller owns it. The test runner wraps
    /// whole modules in a transaction it always rolls back, and must not have
    /// a passing case commit it.
    Caller,
}

/// Calls a JS function, runs the microtask queue to a fixed point, and hands
/// the settled value to `finish`.
///
/// The three phases cannot share one `ctx.with`. Draining needs the runtime,
/// and touching the runtime from inside a context panics with "RefCell already
/// borrowed", so the promise is persisted across the drain and restored after.
///
/// `what` names the invocation ("Handler 'index'") for the message a promise
/// that can never settle produces.
pub(crate) fn call_and_settle<T>(
    rt: &Runtime,
    context: &Context,
    script_uri: &str,
    what: &str,
    transaction: TransactionHandling,
    call: impl for<'js> FnOnce(&rquickjs::Ctx<'js>) -> Result<rquickjs::Promise<'js>, String>,
    finish: impl for<'js> FnOnce(&rquickjs::Ctx<'js>, Value<'js>) -> Result<T, String>,
) -> Result<T, String> {
    let saved =
        context.with(|ctx| call(&ctx).map(|promise| rquickjs::Persistent::save(&ctx, promise)))?;

    report_unhandled(script_uri, drain_jobs(rt));

    context.with(|ctx| {
        let promise = saved
            .restore(&ctx)
            .map_err(|e| format!("restore invocation result: {}", e))?;

        let value = match unwrap_settled(&ctx, promise, what) {
            Ok(value) => value,
            Err(details) => {
                if transaction == TransactionHandling::Auto {
                    finish_transaction(false)?;
                }
                return Err(details);
            }
        };

        if transaction == TransactionHandling::Auto {
            finish_transaction(true)?;
        }
        finish(&ctx, value)
    })
}

/// Reads a promise that [`drain_jobs`] has already run to a fixed point.
///
/// `what` names the thing being settled ("Handler 'index'", "The snippet") so
/// the never-settles message can point at it.
fn unwrap_settled<'js>(
    ctx: &rquickjs::Ctx<'js>,
    promise: rquickjs::Promise<'js>,
    what: &str,
) -> Result<Value<'js>, String> {
    match promise.result::<Value<'js>>() {
        Some(Ok(value)) => Ok(value),
        // `result` rethrows the rejection value into the context, so the same
        // extractor that formats a thrown error formats a rejection.
        Some(Err(e)) => Err(extract_error_details(ctx, &e)),
        None => Err(format!(
            "{} never settled. Scripts run synchronously here — host calls like fetch() \
             and database queries block rather than yielding — so a promise that is not \
             already resolved has nothing that could resolve it.",
            what
        )),
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
    /// Absolute URL of the request, origin included. See [`JsRequestContext::url`].
    pub url: Option<String>,
    pub form_data: Option<HashMap<String, String>>,
    pub raw_body: Option<String>,
    pub headers: HashMap<String, String>,
    pub user_context: UserContext,
    /// Optional OAuth authentication context for JavaScript auth API
    pub auth_context: Option<crate::auth::JsAuthContext>,
    /// Route parameters extracted from path patterns like /users/:id
    pub route_params: Option<HashMap<String, String>>,
    /// Uploaded files from multipart form data
    pub uploaded_files: Option<Vec<crate::parsers::UploadedFile>>,
    /// The request's `x-request-id`, if it came through the HTTP stack. Every
    /// log line the handler writes is filed under it, so a caller holding the
    /// response header can ask for exactly the lines its own call produced.
    pub request_id: Option<String>,
    /// The registered route pattern that matched (`/things/:id`), as opposed to
    /// the concrete `path`. Filtering logs by it aggregates every call to the
    /// handler instead of splitting them per parameter value.
    pub route_pattern: Option<String>,
}

/// Kinds of handler invocations supported by the runtime.
#[derive(Debug, Clone, Copy)]
pub enum HandlerInvocationKind {
    HttpRoute,
    GraphqlQuery,
    GraphqlMutation,
    GraphqlSubscription,
    StreamCustomization,
    /// A listener invoked by `dispatcher.sendMessage`.
    MessageListener,
    Init,
    Scheduled,
    McpTool,
    /// An MCP prompt handler. Unlike the other kinds it is never given a
    /// handler context object, but its output is still attributable.
    McpPrompt,
    Test,
    /// An ad hoc snippet run against a script's sandbox by `/engine/eval`.
    Eval,
}

impl HandlerInvocationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandlerInvocationKind::HttpRoute => "httpRoute",
            HandlerInvocationKind::GraphqlQuery => "graphqlQuery",
            HandlerInvocationKind::GraphqlMutation => "graphqlMutation",
            HandlerInvocationKind::GraphqlSubscription => "graphqlSubscription",
            HandlerInvocationKind::StreamCustomization => "streamCustomization",
            HandlerInvocationKind::MessageListener => "messageListener",
            HandlerInvocationKind::Init => "init",
            HandlerInvocationKind::Scheduled => "scheduled",
            HandlerInvocationKind::McpTool => "mcpTool",
            HandlerInvocationKind::McpPrompt => "mcpPrompt",
            HandlerInvocationKind::Test => "test",
            HandlerInvocationKind::Eval => "eval",
        }
    }

    /// Attribute a script's log output to one invocation of this kind.
    ///
    /// `route` names what was being served — the registered route pattern for
    /// an HTTP route, otherwise the job, stream, resolver or tool name — so
    /// that filtering by it collects every run of the same handler rather than
    /// one concrete path per parameter value.
    pub fn log_context(
        self,
        invocation_id: impl Into<String>,
        route: Option<String>,
    ) -> repository::LogContext {
        repository::LogContext {
            request_id: Some(invocation_id.into()),
            kind: Some(self.as_str().to_string()),
            route,
        }
    }
}

/// Normalized view of inbound request data passed to JavaScript.
#[derive(Debug, Clone, Default)]
pub struct JsRequestContext {
    pub path: Option<String>,
    /// The absolute URL the request arrived on, origin included.
    ///
    /// `path` alone cannot say which host served a request, and the engine
    /// serves several. This is also where the prelude reads the raw query
    /// string from, which is the only place duplicate parameters survive.
    pub url: Option<String>,
    pub method: Option<String>,
    pub headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub form_data: HashMap<String, String>,
    pub body: Option<String>,
    /// Route parameters extracted from path patterns like /users/:id
    pub route_params: HashMap<String, String>,
    /// Uploaded files from multipart form data
    pub uploaded_files: Vec<crate::parsers::UploadedFile>,
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
    invocation_id: Option<String>,
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
            invocation_id: None,
        }
    }

    /// Identify this invocation to the script, so a handler can echo the id
    /// that its log lines are filed under into a response or an error report.
    pub fn with_invocation_id(mut self, invocation_id: impl Into<String>) -> Self {
        self.invocation_id = Some(invocation_id.into());
        self
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
        if let Some(url) = &request.url {
            request_obj.set("url", url.as_str())?;
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

        // Uploaded files (base64-encoded data)
        let files_array = rquickjs::Array::new(ctx.clone())?;
        for (idx, file) in request.uploaded_files.iter().enumerate() {
            let file_obj = rquickjs::Object::new(ctx.clone())?;
            file_obj.set("field", file.field_name.as_str())?;
            if let Some(ref filename) = file.filename {
                file_obj.set("filename", filename.as_str())?;
            }
            if let Some(ref content_type) = file.content_type {
                file_obj.set("contentType", content_type.as_str())?;
            }
            // Encode file data as base64 for JavaScript
            let base64_data =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &file.data);
            file_obj.set("data", base64_data)?;
            file_obj.set("size", file.size as u32)?;
            files_array.set(idx, file_obj)?;
        }
        request_obj.set("files", files_array)?;

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

        // The request prelude gives this object the methods a handler expects of
        // one — `text()`, `json()`, a `Headers` that does not care how the
        // client capitalised a name. It is absent only in a context built
        // before globals were installed, where the plain object is still fine.
        let globals = ctx.globals();
        if let Ok(enhance) = globals.get::<_, rquickjs::Function>("__enhanceRequest") {
            let enhanced: rquickjs::Object = enhance.call((request_obj.clone(),))?;
            return Ok(Some(enhanced));
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
            invocation_id,
        } = self;

        let request_obj = Self::build_request_object(request, auth_context, ctx)?;

        let context_obj = rquickjs::Object::new(ctx.clone())?;
        context_obj.set("kind", kind.as_str())?;

        // The id this invocation's log lines are filed under; `/engine/script_logs`
        // takes it as `request_id`.
        if let Some(invocation_id) = invocation_id {
            context_obj.set("invocationId", invocation_id)?;
        }

        if let Some(script_uri) = script_uri {
            context_obj.set("scriptUri", script_uri)?;
        }

        if let Some(handler_name) = handler_name {
            context_obj.set("handlerName", handler_name)?;
        }

        // Ensure there's always a request object with at least an empty query object
        // This provides "query object guarantees" so scripts can safely access context.request.query
        if let Some(request_obj) = request_obj {
            context_obj.set("request", request_obj)?;
        } else {
            // Create a minimal request object with empty query for non-HTTP handlers
            let minimal_request = rquickjs::Object::new(ctx.clone())?;
            let empty_query = rquickjs::Object::new(ctx.clone())?;
            minimal_request.set("query", empty_query)?;
            context_obj.set("request", minimal_request)?;
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
) -> Result<(), rquickjs::Error> {
    let t = Instant::now();
    let secure_context = SecureGlobalContext::new_with_config(user_context, config.clone());
    let d_ctor = t.elapsed();

    // Setup secure functions with proper capability validation
    let t = Instant::now();
    secure_context.setup_secure_functions(ctx, script_uri, register_fn)?;
    let d_native = t.elapsed();

    // Add Response builder helpers
    let t = Instant::now();
    setup_response_builders(ctx)?;
    let d_resp = t.elapsed();

    // Add validation helpers
    let t = Instant::now();
    setup_validation_helpers(ctx)?;
    let d_valid = t.elapsed();

    GLOBALS_BREAKDOWN.with(|b| b.set(Some((d_ctor, d_native, d_resp, d_valid))));

    // Auth is no longer set up as a global - it's attached to req.auth by the caller

    Ok(())
}

thread_local! {
    /// Carries the last globals-install sub-timings (ctor, native fns, response
    /// builders, validation helpers) so the per-request profiler can log them
    /// *outside* any timed window — logging inside would inflate the reading.
    static GLOBALS_BREAKDOWN: std::cell::Cell<Option<(Duration, Duration, Duration, Duration)>> =
        const { std::cell::Cell::new(None) };
}

/// Sets up Response builder helpers for JavaScript handlers
///
/// Provides convenient methods for creating HTTP responses:
/// - Response.json(data, status) - JSON response
/// - Response.text(text, status) - Text response
/// - Response.html(html, status) - HTML response
/// - Response.error(status, message) - Error response
/// - Response.noContent() - 204 No Content
/// - Response.redirect(url) - 302 redirect
fn setup_response_builders(ctx: &rquickjs::Ctx<'_>) -> Result<(), rquickjs::Error> {
    // Create the ResponseBuilder object with builder methods using JavaScript
    ctx.eval::<(), _>(
        r#"
        globalThis.ResponseBuilder = {
            json: function(data, status = 200) {
                const body = JSON.stringify(data);
                return {
                    status: status,
                    body: body,
                    contentType: "application/json"
                };
            },
            text: function(text, status = 200) {
                return {
                    status: status,
                    body: text,
                    contentType: "text/plain; charset=UTF-8"
                };
            },
            html: function(html, status = 200) {
                return {
                    status: status,
                    body: html,
                    contentType: "text/html; charset=UTF-8"
                };
            },
            error: function(status, message) {
                const body = JSON.stringify({ error: message });
                return {
                    status: status,
                    body: body,
                    contentType: "application/json"
                };
            },
            noContent: function() {
                return {
                    status: 204,
                    body: "",
                    contentType: ""
                };
            },
            redirect: function(url, status = 302) {
                return {
                    status: status,
                    body: "Redirecting to " + url,
                    contentType: "text/plain; charset=UTF-8",
                    headers: {
                        "Location": url
                    }
                };
            }
        };
        "#,
    )?;

    Ok(())
}

/// Sets up validation helper functions for JavaScript execution contexts
///
/// This function provides convenient validation utilities for JavaScript handlers
/// to validate query parameters, path parameters, and other input data.
fn setup_validation_helpers(ctx: &rquickjs::Ctx<'_>) -> Result<(), rquickjs::Error> {
    // Create the validation object with helper functions using JavaScript
    ctx.eval::<(), _>(
        r#"
        globalThis.validate = {
            requireQueryParam: function(context, paramName) {
                if (!context.request || !context.request.query) {
                    throw new Error("Request context or query parameters not available");
                }
                const value = context.request.query[paramName];
                if (value === undefined || value === null || value === "") {
                    throw new Error("Required query parameter '" + paramName + "' is missing or empty");
                }
                return value;
            },

            requirePathParam: function(context, paramName) {
                if (!context.request || !context.request.params) {
                    throw new Error("Request context or path parameters not available");
                }
                const value = context.request.params[paramName];
                if (value === undefined || value === null || value === "") {
                    throw new Error("Required path parameter '" + paramName + "' is missing or empty");
                }
                return value;
            },

            validateString: function(value, options) {
                if (typeof value !== 'string') {
                    throw new Error("Expected string, got " + typeof value);
                }

                const opts = options || {};
                const minLength = opts.minLength || 0;
                const maxLength = opts.maxLength || Infinity;
                const pattern = opts.pattern;

                if (value.length < minLength) {
                    throw new Error("String too short: minimum length is " + minLength + ", got " + value.length);
                }
                if (value.length > maxLength) {
                    throw new Error("String too long: maximum length is " + maxLength + ", got " + value.length);
                }
                if (pattern && !pattern.test(value)) {
                    throw new Error("String does not match required pattern");
                }

                return value;
            },

            validateNumber: function(value, options) {
                const num = Number(value);
                if (isNaN(num)) {
                    throw new Error("Expected number, got " + value);
                }

                const opts = options || {};
                const min = opts.min;
                const max = opts.max;

                if (min !== undefined && num < min) {
                    throw new Error("Number too small: minimum is " + min + ", got " + num);
                }
                if (max !== undefined && num > max) {
                    throw new Error("Number too large: maximum is " + max + ", got " + num);
                }

                return num;
            }
        };

        // Also expose as global functions for convenience
        globalThis.requireQueryParam = function(paramName) {
            return globalThis.validate.requireQueryParam(globalThis.context, paramName);
        };

        globalThis.requirePathParam = function(paramName) {
            return globalThis.validate.requirePathParam(globalThis.context, paramName);
        };

        globalThis.validateString = function(value, minLength, maxLength) {
            return globalThis.validate.validateString(value, { minLength: minLength, maxLength: maxLength });
        };

        globalThis.validateNumber = function(value, min, max) {
            return globalThis.validate.validateNumber(value, { min: min, max: max });
        };

        globalThis.optionalQueryParam = function(paramName, defaultValue) {
            try {
                return globalThis.requireQueryParam(paramName);
            } catch (e) {
                return defaultValue;
            }
        };
        "#,
    )?;

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

    // Validate script using configured limits
    let limits = current_execution_limits();
    if let Err(e) = validate_script(content, &limits) {
        return ScriptExecutionResult::failed(e, start_time.elapsed().as_millis() as u64);
    }

    // Store the script in the repository so it can be accessed later
    let _ = repository::upsert_script(uri, content);

    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = uri.to_string();

    // Prepare the program before the runtime exists. Bundling an asset-backed
    // script fetches and transpiles every imported module, and the interrupt
    // deadline armed by `create_sandboxed_runtime` starts at runtime creation —
    // preparing inside it spends the script's execution budget on the bundle
    // whenever the caches are cold, which is exactly the state a deploy leaves
    // them in.
    let executable_code = match transpile_if_needed(&uri_owned, content) {
        Ok(code) => code,
        Err(e) => {
            return ScriptExecutionResult::failed(
                format!("Transpilation failed: {}", e),
                start_time.elapsed().as_millis() as u64,
            );
        }
    };

    match create_sandboxed_runtime(&limits) {
        Ok(rt) => match Context::full(&rt) {
            Ok(ctx) => {
                // Create a shared location for detailed error message
                let error_details: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
                let error_details_clone = Rc::clone(&error_details);

                let result = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                    // Set up all secure global functions with audit logging disabled for startup
                    let security_config = GlobalSecurityConfig {
                        // Startup is the registration phase: this pass exists to
                        // collect what the script registers.
                        registration_phase: true,
                        enable_audit_logging: false, // Disable for startup to reduce noise
                        // Startup registers for real; nothing to withhold.
                        dry_run_sink: None,
                        console_sink: None,
                        // Bringing the script up is one invocation, so its
                        // output groups under one id like any other.
                        log_context: HandlerInvocationKind::Init
                            .log_context(crate::middleware::generate_request_id(), None),
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
                    )?;

                    // Execute the script (already bundled above)
                    let eval_result =
                        crate::bytecode::eval_program(&ctx, &uri_owned, &executable_code);

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
            ScriptExecutionResult::failed(e, start_time.elapsed().as_millis() as u64)
        }
    }
}

/// Executes a JavaScript script (LEGACY - has security vulnerabilities).
/// This function creates a QuickJS runtime, sets up the register function,
/// executes the script, and returns information about the registrations made.
pub fn execute_script(uri: &str, content: &str) -> ScriptExecutionResult {
    let start_time = Instant::now();

    tracing::info!("execute_script called for URI: {}", uri);

    // Validate script using configured limits
    let limits = current_execution_limits();
    if let Err(e) = validate_script(content, &limits) {
        return ScriptExecutionResult::failed(e, start_time.elapsed().as_millis() as u64);
    }

    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = uri.to_string();

    // Bundle before arming the runtime's interrupt deadline (see
    // `execute_script_secure`).
    let executable_code = match transpile_if_needed(&uri_owned, content) {
        Ok(code) => code,
        Err(e) => {
            return ScriptExecutionResult::failed(
                format!("Transpilation failed: {}", e),
                start_time.elapsed().as_millis() as u64,
            );
        }
    };

    match create_sandboxed_runtime(&limits) {
        Ok(rt) => {
            match Context::full(&rt) {
                Ok(ctx) => {
                    let result =
                        ctx.with(|ctx| -> Result<(), rquickjs::Error> {
                            // This entry point exists to run a script and
                            // collect what it registers - it passes a real
                            // register function below - so it is a
                            // registration pass like startup and init().
                            let config = GlobalSecurityConfig {
                                registration_phase: true,
                                log_context: HandlerInvocationKind::Init
                                    .log_context(crate::middleware::generate_request_id(), None),
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
                            )?;

                            // Execute the script (already bundled above)
                            crate::bytecode::eval_program(&ctx, &uri_owned, &executable_code)?;
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
            ScriptExecutionResult::failed(e, start_time.elapsed().as_millis() as u64)
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
    mut params: RequestExecutionParams,
) -> Result<JsHttpResponse, String> {
    let script_uri_owned = params.script_uri.clone();
    let auth_context = params.auth_context.clone(); // Clone for later use

    // Everything this handler logs is filed under the request's own id, so the
    // lines it produced can be separated from every other request's. Callers
    // outside the HTTP stack (tests) pass none, and get a generated one.
    let invocation_id = params
        .request_id
        .clone()
        .unwrap_or_else(crate::middleware::generate_request_id);
    // Hand the handler the same id its log lines are filed under, whether it
    // came from the HTTP stack or was generated here.
    params.request_id = Some(invocation_id.clone());
    let log_context = HandlerInvocationKind::HttpRoute.log_context(
        invocation_id.clone(),
        params
            .route_pattern
            .clone()
            .or_else(|| Some(params.path.clone())),
    );

    // Per-request phase profiling (gated by AIWEBENGINE_PROFILE_REQUESTS). The
    // Instant::now() calls are always taken (they cost nanoseconds); only the
    // final log line is gated so profiling adds no measurable overhead when off.
    let profile = request_profiling_enabled();

    // Fetch and bundle the program *before* creating the runtime: the interrupt
    // deadline armed by `create_sandboxed_runtime` starts at runtime creation,
    // so preparing the program afterwards charges the bundle against the
    // request's execution budget. On a cold cache — the state every deploy
    // leaves behind — an asset-backed script fetches and transpiles every
    // imported module here, which was enough to exhaust the budget before the
    // handler ran.
    let phase = Instant::now();
    let owner_script = repository::fetch_script(&params.script_uri)
        .ok_or_else(|| format!("no script for uri {}", params.script_uri))?;
    let t_fetch = phase.elapsed();

    // Transpile if needed (TypeScript/JSX/TSX) — cached by (uri, source hash).
    let phase = Instant::now();
    let executable_code = transpile_if_needed(&params.script_uri, &owner_script)?;
    let t_transpile = phase.elapsed();

    let phase = Instant::now();
    let rt = create_sandboxed_runtime(&current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;
    let t_runtime = phase.elapsed();

    let phase = Instant::now();
    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up all secure global functions
        // For request handling, we don't need GraphQL registration but enable everything else
        let security_config = GlobalSecurityConfig {
            enable_audit_logging: false, // Disable for tests to avoid runtime conflicts
            log_context: log_context.clone(),
            ..Default::default()
        };

        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            params.user_context.clone(),
            &security_config,
            None,
            params.auth_context.clone(), // Pass auth context for request handling
        )?;

        Ok(())
    })
    .map_err(|e| format!("install secure host fns: {}", e))?;
    let t_globals = phase.elapsed();

    // Evaluate the script and capture detailed error information if it fails.
    // Bytecode is cached, but the top-level program still executes each request.
    let phase = Instant::now();
    ctx.with(|ctx| -> Result<(), String> {
        let result = crate::bytecode::eval_program(&ctx, &params.script_uri, &executable_code);
        if let Err(ref e) = result {
            let details = extract_error_details(&ctx, e);
            return Err(format!("owner eval: {}", details));
        }
        Ok(())
    })?;
    let t_eval = phase.elapsed();

    let phase = Instant::now();
    let response_exec = invoke_handler_and_build_response(&rt, &ctx, &params, &auth_context);

    let t_handler = phase.elapsed();

    let response_result = response_exec.map_err(|e| e.to_string())?;

    if profile {
        let total = t_runtime + t_globals + t_fetch + t_transpile + t_eval + t_handler;
        info!(
            target: "request_profile",
            uri = %params.script_uri,
            handler = %params.handler_name,
            runtime_us = t_runtime.as_micros() as u64,
            globals_us = t_globals.as_micros() as u64,
            fetch_us = t_fetch.as_micros() as u64,
            transpile_us = t_transpile.as_micros() as u64,
            eval_us = t_eval.as_micros() as u64,
            handler_us = t_handler.as_micros() as u64,
            total_us = total.as_micros() as u64,
            "request phase profile"
        );
        if let Some((ctor, native, resp, valid)) = GLOBALS_BREAKDOWN.with(|b| b.take()) {
            info!(
                target: "request_profile",
                ctor_us = ctor.as_micros() as u64,
                native_fns_us = native.as_micros() as u64,
                response_builders_us = resp.as_micros() as u64,
                validation_helpers_us = valid.as_micros() as u64,
                "globals install breakdown"
            );
        }
    }

    // Ensure clean shutdown: drop Context before Runtime
    drop(ctx);
    Ok(response_result)
}

/// Invokes the named handler and hands back its result as a native promise.
///
/// Runs inside an active `ctx.with`. The promise is not read here: the
/// microtask queue that would settle it can only be drained with no context
/// guard on the stack, so that is the caller's job.
fn call_handler<'js>(
    ctx: &rquickjs::Ctx<'js>,
    params: &RequestExecutionParams,
    auth_context: &Option<crate::auth::JsAuthContext>,
) -> Result<rquickjs::Promise<'js>, String> {
    let global = ctx.globals();
    let func: Function = global
        .get::<_, Function>(&params.handler_name)
        .map_err(|e| format!("no handler {}: {}", params.handler_name, e))?;

    let request_context = JsRequestContext {
        path: Some(params.path.clone()),
        url: params.url.clone(),
        method: Some(params.method.clone()),
        headers: params.headers.clone(),
        query_params: params.query_params.clone().unwrap_or_default(),
        form_data: params.form_data.clone().unwrap_or_default(),
        body: params.raw_body.clone(),
        route_params: params.route_params.clone().unwrap_or_default(),
        uploaded_files: params.uploaded_files.clone().unwrap_or_default(),
    };

    let mut context_builder = JsHandlerContextBuilder::new(HandlerInvocationKind::HttpRoute)
        .with_script_metadata(&params.script_uri, &params.handler_name)
        .with_request(request_context);

    if let Some(invocation_id) = params.request_id.as_ref() {
        context_builder = context_builder.with_invocation_id(invocation_id.clone());
    }

    if let Some(auth_ctx) = auth_context {
        context_builder = context_builder.with_auth_context(auth_ctx.clone());
    }

    let handler_context = context_builder
        .build(ctx)
        .map_err(|e| format!("build context: {}", e))?;

    // Set context as a global variable so personalStorage and other APIs can access it
    global
        .set("context", handler_context.clone())
        .map_err(|e| format!("set context global: {}", e))?;

    // A handler that throws before returning never reaches the queue, so its
    // transaction is finished here. One that rejects *after* an await settles
    // during the drain instead, and is finished by the caller.
    let result: Value = func.call::<_, Value>((handler_context,)).map_err(|e| {
        let details = extract_error_details(ctx, &e);
        if crate::database::get_current_transaction_active() {
            let _ = crate::database::Database::rollback_transaction();
        }
        format!("call handler: {}", details)
    })?;

    promise_resolve(ctx, result)
}

/// Commits or rolls back the request's transaction, if it opened one.
///
/// Must run *after* the microtask queue has been drained. An `async` handler
/// has not made its post-`await` writes until then, and committing earlier
/// closes the transaction out from under them.
fn finish_transaction(succeeded: bool) -> Result<(), String> {
    if !crate::database::get_current_transaction_active() {
        return Ok(());
    }
    if succeeded {
        crate::database::Database::commit_transaction()
            .map_err(|e| format!("transaction commit failed: {}", e))
    } else {
        let _ = crate::database::Database::rollback_transaction();
        Ok(())
    }
}

/// Rolls back a transaction the invocation opened and never finished.
///
/// `database.beginTransaction()` leaves the transaction open for the handler
/// boundary to finish, which is what the handler paths do. Paths without such
/// a boundary — `init()`, an evaluation, a dry run — would otherwise leave it
/// open on the thread, and since the transaction lives in thread-local storage
/// the next invocation to land on that thread would inherit it and have its
/// writes swallowed.
///
/// Only a transaction this invocation opened is rolled back. One that was
/// already active belongs to an outer scope — a test run's or an evaluation's
/// rollback guard — and finishing it here would cut that scope short.
struct StrayTransaction {
    outer_active: bool,
}

impl StrayTransaction {
    fn arm() -> Self {
        Self {
            outer_active: crate::database::get_current_transaction_active(),
        }
    }
}

impl Drop for StrayTransaction {
    fn drop(&mut self) {
        if !self.outer_active && crate::database::get_current_transaction_active() {
            warn!(
                "a transaction was left open and is being rolled back; \
                 call database.commitTransaction() to keep its writes"
            );
            let _ = crate::database::Database::rollback_transaction();
        }
    }
}

/// Maps a settled handler result into an [`JsHttpResponse`].
fn build_http_response(result: Value<'_>) -> Result<JsHttpResponse, String> {
    if let Some(response_obj) = result.as_object() {
        let status: i32 = response_obj
            .get("status")
            .map_err(|e| format!("missing status: {}", e))?;

        // Try to get bodyBase64 first (for binary data), otherwise fall back to body (for text)
        let (body, used_body_base64): (Vec<u8>, bool) = if let Ok(body_base64) =
            response_obj.get::<_, String>("bodyBase64")
        {
            // Decode base64 to bytes
            let decoded =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &body_base64)
                    .map_err(|e| format!("failed to decode bodyBase64: {}", e))?;
            (decoded, true)
        } else {
            // Fall back to string body - handle both strings and SafeHTML objects
            let body_value: rquickjs::Value = response_obj
                .get("body")
                .map_err(|e| format!("missing body or bodyBase64: {}", e))?;

            let body_string: String = if body_value.is_string() {
                // Direct string value
                body_value
                    .as_string()
                    .and_then(|s| s.to_string().ok())
                    .ok_or_else(|| "Failed to convert body to string".to_string())?
            } else if let Some(obj) = body_value.as_object() {
                // Check if it's a SafeHTML object with __html property
                if let Ok(html) = obj.get::<_, String>("__html") {
                    html
                } else if let Ok(to_string_fn) = obj.get::<_, rquickjs::Function>("toString") {
                    // Bind the receiver: an inherited `toString` — `String`'s,
                    // for one — reads the value off `this` and throws when
                    // called with none.
                    to_string_fn
                        .call::<_, String>((rquickjs::function::This(obj.clone()),))
                        .map_err(|e| format!("Failed to call toString: {}", e))?
                } else {
                    return Err("Body must be a string or have a toString() method".to_string());
                }
            } else {
                return Err("Body must be a string or object with __html property".to_string());
            };

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
}

/// Builds the per-request handler context, invokes the named handler, settles
/// whatever it returned, and maps that into an [`JsHttpResponse`].
///
/// The three phases cannot share one `ctx.with`: draining the microtask queue
/// requires the runtime, and touching the runtime from inside a context panics
/// with "RefCell already borrowed". So the handler's result is persisted, the
/// queue is drained outside the context, and the value is restored to finish.
///
/// Shared by the standard request path and the pooling prototype so both agree
/// on transaction handling and response shaping.
fn invoke_handler_and_build_response(
    rt: &Runtime,
    context: &Context,
    params: &RequestExecutionParams,
    auth_context: &Option<crate::auth::JsAuthContext>,
) -> Result<JsHttpResponse, String> {
    call_and_settle(
        rt,
        context,
        &params.script_uri,
        &format!("Handler '{}'", params.handler_name),
        TransactionHandling::Auto,
        |ctx| call_handler(ctx, params, auth_context),
        |_ctx, value| build_http_response(value),
    )
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
    let invocation_id = crate::middleware::generate_request_id();
    let log_context =
        HandlerInvocationKind::HttpRoute.log_context(invocation_id.clone(), Some(path.to_string()));

    // Fetch and bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`).
    let owner_script = repository::fetch_script(script_uri)
        .ok_or_else(|| format!("no script for uri {}", script_uri))?;
    let executable_code = transpile_if_needed(script_uri, &owner_script)?;

    let rt = create_sandboxed_runtime(&current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up all global functions using the secure helper function
        // For request handling, we don't need full GraphQL registration (no-ops)
        let config = GlobalSecurityConfig {
            enable_audit_logging: false, // Disable audit logging to avoid runtime conflicts
            log_context: log_context.clone(),
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
        )?;

        Ok(())
    })
    .map_err(|e| format!("install host fns: {}", e))?;

    ctx.with(|ctx| crate::bytecode::eval_program(&ctx, script_uri, &executable_code))
        .map_err(|e| format!("owner eval: {}", e))?;

    let (status, body, content_type) = call_and_settle(
        &rt,
        &ctx,
        script_uri,
        &format!("Handler '{}'", handler_name),
        TransactionHandling::Auto,
        |ctx| {
            let global = ctx.globals();
            let func: Function = global
                .get::<_, Function>(handler_name)
                .map_err(|e| format!("no handler {}: {}", handler_name, e))?;

            let request_context = JsRequestContext {
                path: Some(path.to_string()),
                // Not an HTTP request: nothing arrived on a URL.
                url: None,
                method: Some(method.to_string()),
                headers: HashMap::new(),
                query_params: query_params.cloned().unwrap_or_default(),
                form_data: form_data.cloned().unwrap_or_default(),
                body: raw_body.clone(),
                route_params: HashMap::new(),
                uploaded_files: Vec::new(),
            };

            let mut context_builder =
                JsHandlerContextBuilder::new(HandlerInvocationKind::HttpRoute)
                    .with_script_metadata(script_uri, handler_name)
                    .with_request(request_context)
                    .with_invocation_id(invocation_id.clone());

            context_builder = context_builder.with_auth_context(auth_ctx.clone());

            let handler_context = context_builder
                .build(ctx)
                .map_err(|e| format!("build context: {}", e))?;

            // Set context as a global variable so personalStorage and other APIs can access it
            let global = ctx.globals();
            global
                .set("context", handler_context.clone())
                .map_err(|e| format!("set context global: {}", e))?;

            let val = func
                .call::<_, Value>((handler_context,))
                .map_err(|e| format!("call error: {}", e))?;

            promise_resolve(ctx, val)
        },
        |_ctx, val| {
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
        },
    )?;

    // Ensure clean shutdown: drop Context before Runtime
    ensure_clean_shutdown(ctx, Ok((status, body, content_type)))
}

/// Executes a JavaScript handler for scheduler jobs
pub fn execute_scheduled_handler(
    script_uri: &str,
    handler_name: &str,
    invocation: &ScheduledInvocation,
) -> Result<(), String> {
    let script_uri_owned = script_uri.to_string();
    // The scheduler generated this id when it claimed the run, so the lines the
    // engine wrote about the run and the lines the job itself wrote share it.
    let log_context = HandlerInvocationKind::Scheduled.log_context(
        invocation.invocation_id.clone(),
        Some(invocation.key.clone()),
    );

    // Fetch and bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`).
    let owner_script = repository::fetch_script(script_uri)
        .ok_or_else(|| format!("no script for uri {}", script_uri))?;
    let executable_code = transpile_if_needed(script_uri, &owner_script)?;

    let rt = create_sandboxed_runtime(&current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        let security_config = GlobalSecurityConfig {
            enable_audit_logging: false,
            log_context: log_context.clone(),
            ..Default::default()
        };

        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            UserContext::admin("scheduler".to_string()),
            &security_config,
            None,
            None,
        )
    })
    .map_err(|e| format!("install scheduler globals: {}", e))?;

    ctx.with(|ctx| {
        crate::bytecode::eval_program(&ctx, script_uri, &executable_code).map_err(|e| {
            let details = extract_error_details(&ctx, &e);
            format!("script eval: {}", details)
        })
    })?;

    let handler_result = call_and_settle(
        &rt,
        &ctx,
        script_uri,
        &format!("Scheduled handler '{}'", handler_name),
        TransactionHandling::Auto,
        |ctx| {
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
                "intervalMilliseconds": invocation.interval_milliseconds,
            });

            let handler_context = JsHandlerContextBuilder::new(HandlerInvocationKind::Scheduled)
                .with_script_metadata(script_uri, handler_name)
                .with_metadata_value("schedule", schedule_meta)
                .with_invocation_id(invocation.invocation_id.clone())
                .build(ctx)
                .map_err(|e| format!("build context: {}", e))?;

            // Set context as a global variable so personalStorage and other APIs can access it
            global
                .set("context", handler_context.clone())
                .map_err(|e| format!("set context global: {}", e))?;

            // A job that throws before returning never reaches the queue, so
            // its transaction is rolled back here. One that rejects after an
            // await settles during the drain, and `call_and_settle` finishes it.
            let result = func.call::<_, Value>((handler_context,)).map_err(|e| {
                let details = extract_error_details(ctx, &e);
                if crate::database::get_current_transaction_active() {
                    let _ = crate::database::Database::rollback_transaction();
                }
                format!("call handler: {}", details)
            })?;

            promise_resolve(ctx, result)
        },
        |_ctx, _value| Ok(()),
    );

    // Ensure clean shutdown
    drop(ctx);

    handler_result?;
    Ok(())
}

/// The JavaScript authoring API (`test`, `expect`, hooks) evaluated into a test
/// context ahead of the test module, so the module can call it as it loads.
const TEST_PRELUDE: &str = include_str!("../assets/test_prelude.js");

/// Parameters for one run of a script's tests.
#[derive(Debug, Clone)]
pub struct TestRunParams {
    pub script_uri: String,
    /// The user who asked for the run. Tests execute with *their* capabilities
    /// rather than the engine's: a suite that passes only because it ran as an
    /// administrator has tested something production will never do.
    pub user_context: UserContext,
    /// Wall-clock budget for each test module. Every module gets its own
    /// runtime and so its own budget, which keeps one runaway file from
    /// spending the time the rest of them need.
    pub timeout_ms: u64,
    /// Ceiling on the whole run. Without it a script with many test files
    /// could hold a request open for modules × `timeout_ms`; with it the run
    /// stops starting modules once the budget is gone and reports what it has.
    pub run_timeout_ms: u64,
    /// Run only the cases whose name contains this substring.
    pub filter: Option<String>,
    /// Wrap each module's cases in a transaction that is always rolled back.
    /// This covers `database.*` and nothing else — asset writes, secret writes,
    /// and outbound HTTP a test performs are real and survive the run.
    pub rollback: bool,
}

/// What running one test module produced.
struct ModuleOutcome {
    cases: Vec<TestCaseResult>,
    /// The module ran out of budget, so the cases after the interrupt never
    /// ran and no verdict exists for them.
    timed_out: bool,
}

/// Run every test case in each of `test_modules` and report what happened.
///
/// Each module gets its own runtime and context. That buys two things a single
/// shared bundle cannot: every case can be attributed to the file it came from,
/// and a global one test file leaks cannot reach the next one. A module that
/// fails to bundle, or throws while loading, is reported as one failed case
/// naming the file — so a single broken file cannot hide the other files'
/// results.
pub fn execute_test_run(params: &TestRunParams, test_modules: &[String]) -> TestRunResult {
    let started = Instant::now();
    let run_deadline = started + Duration::from_millis(params.run_timeout_ms);
    let mut cases = Vec::new();
    let mut timed_out = false;

    for module_path in test_modules {
        // Stop starting work the run cannot finish. Modules already done keep
        // their verdicts — the cap bounds the request, it does not discard
        // results.
        let remaining = run_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            warn!(
                script_uri = %params.script_uri,
                "Test run hit its {}ms ceiling with modules left to run",
                params.run_timeout_ms
            );
            timed_out = true;
            break;
        }

        // No module gets more than the run has left, so the ceiling holds even
        // when a single file would otherwise use its whole budget.
        let module_budget = (remaining.as_millis() as u64).min(params.timeout_ms);

        match execute_test_module(params, module_path, module_budget) {
            Ok(outcome) => {
                timed_out |= outcome.timed_out;
                cases.extend(outcome.cases);
            }
            Err(error) => {
                warn!(
                    script_uri = %params.script_uri,
                    module = %module_path,
                    "Test module could not run: {}",
                    error
                );
                cases.push(
                    TestCaseResult::failed(module_path.clone(), error, 0)
                        .from_file(module_path.clone()),
                );
            }
        }
    }

    let duration_ms = started.elapsed().as_millis() as u64;
    if timed_out {
        TestRunResult::timed_out(params.script_uri.as_str(), cases, duration_ms)
    } else {
        TestRunResult::completed(params.script_uri.as_str(), cases, duration_ms)
    }
}

/// Bundle one test module, evaluate it to collect its cases, then run them.
///
/// `Err` means the module never got as far as producing verdicts; individual
/// case failures come back inside [`ModuleOutcome`].
fn execute_test_module(
    params: &TestRunParams,
    module_path: &str,
    budget_ms: u64,
) -> Result<ModuleOutcome, String> {
    // Bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`): on a cold cache this fetches and
    // transpiles every module the test imports, which must not be charged to
    // the budget meant for running the tests.
    let modules = [module_path.to_string()];
    let prepared = module_loader::prepare_test_program(&params.script_uri, &modules)
        .map_err(|e| format!("Failed to bundle test module: {}", e))?;

    let limits = ExecutionLimits {
        timeout_ms: budget_ms,
        ..current_execution_limits()
    };
    // Mirrors the deadline `create_sandboxed_runtime` arms the interrupt with,
    // so a failed call can be told apart from a call the interrupt stopped
    // without matching on QuickJS error text.
    let deadline = Instant::now() + Duration::from_millis(budget_ms);

    let rt = create_sandboxed_runtime(&limits)?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    // Collection happens inside a `with`; running the cases cannot, because a
    // case that awaits only settles once the microtask queue is drained and
    // draining needs the runtime. The collected functions are therefore
    // persisted out of the context and restored one at a time.
    let outcome = run_test_module(
        &rt,
        &ctx,
        params,
        module_path,
        &prepared.code,
        budget_ms,
        deadline,
    );

    // Context must drop before the runtime (see `ensure_clean_shutdown`).
    drop(ctx);

    outcome
}

/// Install the test globals into `ctx`, load `code`, and run the cases it
/// registers. Split out from [`execute_test_module`] so the context lifetime
/// has a name: the collected [`Function`]s are parameterized by it.
fn install_and_collect_tests<'js>(
    ctx: &rquickjs::Ctx<'js>,
    params: &TestRunParams,
    module_path: &str,
    code: &str,
) -> Result<Vec<(String, rquickjs::Persistent<Function<'static>>)>, String> {
    let invocation_id = crate::middleware::generate_request_id();
    let security_config = GlobalSecurityConfig {
        // A test must not mutate registries that outlive the run: routes,
        // resolvers, streams, and jobs registered here would stay registered,
        // and no rollback undoes them. `registration_phase: false` is what
        // enforces that - the APIs stay callable and report that they did
        // nothing, rather than disappearing from the test's global scope.
        registration_phase: false,
        enable_audit_logging: false,
        dry_run_sink: None,
        console_sink: None,
        log_context: HandlerInvocationKind::Test
            .log_context(invocation_id.clone(), Some(module_path.to_string())),
    };

    setup_secure_global_functions(
        ctx,
        &params.script_uri,
        params.user_context.clone(),
        &security_config,
        None,
        None,
    )
    .map_err(|e| format!("install test globals: {}", extract_error_details(ctx, &e)))?;

    let handler_context = JsHandlerContextBuilder::new(HandlerInvocationKind::Test)
        .with_script_metadata(params.script_uri.clone(), module_path)
        .with_invocation_id(invocation_id.clone())
        .build(ctx)
        .map_err(|e| format!("build context: {}", e))?;
    // Set before evaluating the module, not just before calling a case:
    // top-level code in the test file can already reach for `context`.
    ctx.globals()
        .set("context", handler_context)
        .map_err(|e| format!("set context global: {}", e))?;

    let registered = Rc::new(RefCell::new(Vec::<(String, Function<'js>)>::new()));
    let sink = Rc::clone(&registered);
    let register = Function::new(
        ctx.clone(),
        move |name: String, body: Function<'js>| -> Result<(), rquickjs::Error> {
            if let Ok(mut cases) = sink.try_borrow_mut() {
                cases.push((name, body));
            }
            Ok(())
        },
    )
    .map_err(|e| format!("build test registry: {}", e))?;
    ctx.globals()
        .set("__registerTest__", register)
        .map_err(|e| format!("install test registry: {}", e))?;

    crate::bytecode::eval_program(ctx, "engine://test-prelude", TEST_PRELUDE)
        .map_err(|e| format!("test prelude: {}", extract_error_details(ctx, &e)))?;

    // The bytecode cache overwrites by key, so a test bundle stored under the
    // script's own URI would evict the compiled program that serves requests.
    // The key doubles as the filename QuickJS puts in stack traces, hence a
    // readable separator rather than an exotic one.
    let bytecode_key = format!("{}::tests::{}", params.script_uri, module_path);
    crate::bytecode::eval_program(ctx, &bytecode_key, code)
        .map_err(|e| format!("load: {}", extract_error_details(ctx, &e)))?;

    // Take the cases out, so a `test()` call made from inside a running test
    // lands in a fresh vector and is ignored rather than conflicting with the
    // borrow the run loop holds. Each body is persisted so it survives leaving
    // the context, which the run loop must do in order to drain the queue.
    Ok(registered
        .take()
        .into_iter()
        .map(|(name, body)| (name, rquickjs::Persistent::save(ctx, body)))
        .collect())
}

/// Loads a test module and runs the cases it registers.
///
/// Each case is called, its microtask queue drained, and its promise settled
/// before the next one starts, so an `async` test body reports the verdict its
/// assertions actually reached rather than passing the moment it suspends.
fn run_test_module(
    rt: &Runtime,
    context: &Context,
    params: &TestRunParams,
    module_path: &str,
    code: &str,
    budget_ms: u64,
    deadline: Instant,
) -> Result<ModuleOutcome, String> {
    let collected =
        context.with(|ctx| install_and_collect_tests(&ctx, params, module_path, code))?;

    // Held for the whole loop and never committed: `TransactionGuard` rolls
    // back when it drops, including on an early return.
    let _rollback_guard = if params.rollback {
        Some(
            crate::database::Database::begin_transaction(Some(budget_ms))
                .map_err(|e| format!("could not isolate the run in a transaction: {}", e))?,
        )
    } else {
        None
    };

    let mut cases = Vec::with_capacity(collected.len());
    let mut timed_out = false;

    for (name, body) in collected {
        if let Some(filter) = &params.filter
            && !name.contains(filter.as_str())
        {
            continue;
        }

        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }

        let started = Instant::now();
        // The module's transaction belongs to the guard above; a passing case
        // must not commit it out from under the rollback.
        let result = call_and_settle(
            rt,
            context,
            &params.script_uri,
            &format!("Test '{}'", name),
            TransactionHandling::Caller,
            |ctx| {
                let body = body
                    .restore(ctx)
                    .map_err(|e| format!("restore test body: {}", e))?;
                let value = body
                    .call::<_, Value>(())
                    .map_err(|e| extract_error_details(ctx, &e))?;
                promise_resolve(ctx, value)
            },
            |_ctx, _value| Ok(()),
        );
        let duration_ms = started.elapsed().as_millis() as u64;

        match result {
            Ok(()) => cases.push(TestCaseResult::passed(name, duration_ms).from_file(module_path)),
            Err(details) => {
                if Instant::now() >= deadline {
                    // The interrupt ended this call, not the test itself, so
                    // there is no verdict to report for it.
                    timed_out = true;
                    break;
                }
                cases.push(
                    TestCaseResult::failed(name, details, duration_ms).from_file(module_path),
                );
            }
        }
    }

    Ok(ModuleOutcome { cases, timed_out })
}

/// Executes a JavaScript GraphQL resolver function and returns the result as a string.
/// This is used by the GraphQL system to call JavaScript resolver functions.
pub fn execute_graphql_resolver(params: GraphqlResolverExecutionParams) -> Result<String, String> {
    let script_uri_owned = params.script_uri.clone();
    let resolver_function_owned = params.resolver_function.clone();
    let args_owned = params.args.clone();
    let auth_context = params.auth_context.clone();

    let invocation_id = crate::middleware::generate_request_id();

    // Fetch and bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`). This path previously evaluated the
    // raw source, which cannot work for a TypeScript or asset-importing script.
    let script_content = repository::fetch_script(&script_uri_owned)
        .ok_or_else(|| format!("no script for uri {}", script_uri_owned))?;
    let executable_code = transpile_if_needed(&script_uri_owned, &script_content)?;

    let rt = create_sandboxed_runtime(&current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    let setup_exec = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up all global functions using the secure helper function
        // For GraphQL resolvers, we don't need GraphQL registration (no-ops) or stream registration
        let config = GlobalSecurityConfig {
            enable_audit_logging: false, // Disable audit logging to avoid runtime conflicts
            log_context: params
                .operation_kind
                .as_handler_kind()
                .log_context(invocation_id.clone(), Some(params.field_name.clone())),
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
        )?;

        // Override specific functions that have different signatures for GraphQL resolver context
        let _global = ctx.globals();

        // Execute the script (fetched and bundled above)
        crate::bytecode::eval_program(&ctx, &script_uri_owned, &executable_code)?;

        Ok(())
    });

    if let Err(e) = setup_exec {
        return Err(format!("JavaScript execution error: {}", e));
    }

    let result_exec = call_and_settle(
        &rt,
        &ctx,
        &params.script_uri,
        &format!("Resolver '{}'", params.resolver_function),
        TransactionHandling::Auto,
        |ctx| -> Result<rquickjs::Promise<'_>, String> {
            let resolver_result: rquickjs::Value = ctx
                .globals()
                .get(&resolver_function_owned)
                .map_err(|e| format!("no resolver {}: {}", resolver_function_owned, e))?;
            let resolver_func = resolver_result.as_function().ok_or_else(|| {
                format!(
                    "resolver '{}' not found, or not a function",
                    resolver_function_owned
                )
            })?;

            let request_context = JsRequestContext {
                path: Some("/graphql".to_string()),
                url: None,
                method: Some("POST".to_string()),
                headers: HashMap::new(),
                query_params: HashMap::new(),
                form_data: HashMap::new(),
                body: None,
                route_params: HashMap::new(),
                uploaded_files: Vec::new(),
            };

            let mut context_builder =
                JsHandlerContextBuilder::new(params.operation_kind.as_handler_kind())
                    .with_script_metadata(&params.script_uri, &params.resolver_function)
                    .with_request(request_context)
                    .with_invocation_id(invocation_id.clone())
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

            let handler_context = context_builder
                .build(ctx)
                .map_err(|e| format!("build context: {}", e))?;

            // Set context as a global variable so personalStorage and other APIs can access it
            let global = ctx.globals();
            global
                .set("context", handler_context.clone())
                .map_err(|e| format!("set context global: {}", e))?;

            // A resolver that throws before returning never reaches the queue, so
            // its transaction is rolled back here. One that rejects after an await
            // settles during the drain, and `call_and_settle` finishes it.
            let result_value = resolver_func
                .call::<_, rquickjs::Value>((handler_context,))
                .map_err(|e| {
                    let details = extract_error_details(ctx, &e);
                    if crate::database::get_current_transaction_active() {
                        let _ = crate::database::Database::rollback_transaction();
                    }
                    format!("call resolver: {}", details)
                })?;

            promise_resolve(ctx, result_value)
        },
        |ctx, result_value| -> Result<String, String> {
            // Convert the result to a JSON string
            if result_value.is_string() {
                result_value
                    .as_string()
                    .ok_or_else(|| "resolver result was not a string".to_string())?
                    .to_string()
                    .map_err(|e| format!("read resolver string: {}", e))
            } else {
                // Use JavaScript's JSON.stringify to convert any value to JSON
                let json_obj: rquickjs::Object = ctx
                    .globals()
                    .get("JSON")
                    .map_err(|e| format!("JSON global missing: {}", e))?;
                let json_stringify: rquickjs::Function = json_obj
                    .get("stringify")
                    .map_err(|e| format!("JSON.stringify missing: {}", e))?;
                json_stringify
                    .call((result_value,))
                    .map_err(|e| format!("stringify resolver result: {}", e))
            }
        },
    );

    let result_string = result_exec.map_err(|e| format!("JavaScript execution error: {}", e))?;

    // Ensure clean shutdown: drop Context before Runtime
    drop(ctx);
    Ok(result_string)
}

/// Execute an MCP tool handler
///
/// This function loads a script and calls the specified MCP tool handler function with the provided arguments.
///
/// # Arguments
/// * `script_uri` - The URI of the script containing the tool handler
/// * `handler_function` - The name of the handler function to call
/// * `tool_name` - The name of the MCP tool being invoked
/// * `arguments` - The tool arguments as a JSON value
///
/// # Returns
/// * `Ok(String)` - The result from the handler function (as JSON string)
/// * `Err(String)` - Error message if execution fails
pub fn execute_mcp_prompt_handler(
    script_uri: &str,
    handler_function: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let script_uri_owned = script_uri.to_string();
    let handler_function_owned = handler_function.to_string();
    let arguments_owned = arguments.clone();

    // Fetch and bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`).
    let script_content = repository::fetch_script(&script_uri_owned)
        .ok_or_else(|| format!("no script for uri {}", script_uri_owned))?;
    let executable_code = transpile_if_needed(&script_uri_owned, &script_content)?;

    let rt = create_sandboxed_runtime(&current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    let setup_exec = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up all global functions using the secure helper function
        // For MCP prompt handlers, we enable minimal features
        let config = GlobalSecurityConfig {
            enable_audit_logging: false,
            log_context: HandlerInvocationKind::McpPrompt.log_context(
                crate::middleware::generate_request_id(),
                Some(handler_function_owned.clone()),
            ),
            ..Default::default()
        };

        // MCP prompt handlers run with admin context (similar to GraphQL resolvers)
        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            UserContext::admin("mcp-prompt".to_string()),
            &config,
            None,
            None,
        )?;

        // Execute the script (fetched and bundled above)
        crate::bytecode::eval_program(&ctx, &script_uri_owned, &executable_code)?;

        Ok(())
    });

    if let Err(e) = setup_exec {
        return Err(format!("Prompt handler execution failed: {}", e));
    }

    let result_exec = call_and_settle(
        &rt,
        &ctx,
        &script_uri_owned,
        &format!("MCP prompt handler '{}'", handler_function_owned),
        TransactionHandling::Auto,
        |ctx| -> Result<rquickjs::Promise<'_>, String> {
            // Get the handler function
            let handler_result: rquickjs::Value = ctx
                .globals()
                .get(&handler_function_owned)
                .map_err(|e| format!("no handler {}: {}", handler_function_owned, e))?;
            let handler_func = handler_result.as_function().ok_or_else(|| {
                format!(
                    "handler '{}' not found, or not a function",
                    handler_function_owned
                )
            })?;

            // Parse arguments as a JavaScript object
            let args_str = arguments_owned.to_string();
            let args_obj: rquickjs::Value = ctx
                .json_parse(args_str)
                .map_err(|e| format!("parse prompt arguments: {}", e))?;

            // Call the handler with arguments
            let result: rquickjs::Value = handler_func.call((args_obj,)).map_err(|e| {
                let details = extract_error_details(ctx, &e);
                format!("call prompt handler: {}", details)
            })?;

            promise_resolve(ctx, result)
        },
        |ctx, result| {
            // Convert result to JSON
            let result_json_str = ctx
                .json_stringify(result)
                .map_err(|e| format!("stringify prompt result: {}", e))?
                .ok_or_else(|| "Failed to stringify result".to_string())?;

            let result_json: String = result_json_str
                .to_string()
                .map_err(|e| format!("read prompt result: {}", e))?;

            serde_json::from_str(&result_json)
                .map_err(|_e| "Invalid JSON from prompt handler".to_string())
        },
    );

    result_exec.map_err(|e| format!("Prompt handler execution failed: {}", e))
}

/// Execute an MCP tool handler function
///
/// # Arguments
/// * `script_uri` - The URI of the script containing the handler
/// * `handler_function` - The name of the handler function to call
/// * `tool_name` - The name of the MCP tool being executed
/// * `arguments` - The arguments to pass to the handler
///
/// # Returns
/// * `Ok(String)` - The result from the handler function (as JSON string)
/// * `Err(String)` - Error message if execution fails
pub fn execute_mcp_tool_handler(
    script_uri: &str,
    handler_function: &str,
    tool_name: &str,
    arguments: serde_json::Value,
    auth_context: Option<crate::auth::JsAuthContext>,
    user_context: UserContext,
) -> Result<String, String> {
    let script_uri_owned = script_uri.to_string();
    let handler_function_owned = handler_function.to_string();
    let tool_name_owned = tool_name.to_string();
    let arguments_owned = arguments.clone();
    let auth_context_owned = auth_context;
    let user_context_owned = user_context;
    let invocation_id = crate::middleware::generate_request_id();
    let log_context = HandlerInvocationKind::McpTool
        .log_context(invocation_id.clone(), Some(tool_name_owned.clone()));

    // Fetch and bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`).
    let script_content = repository::fetch_script(&script_uri_owned)
        .ok_or_else(|| format!("no script for uri {}", script_uri_owned))?;
    let executable_code = transpile_if_needed(&script_uri_owned, &script_content)?;

    let rt = create_sandboxed_runtime(&current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    let setup_exec = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up all global functions using the secure helper function
        // For MCP tool handlers, we enable minimal features
        let config = GlobalSecurityConfig {
            enable_audit_logging: false,
            log_context: log_context.clone(),
            ..Default::default()
        };

        // MCP tool handlers inherit the validated caller context from MCP auth middleware.
        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            user_context_owned.clone(),
            &config,
            None,
            auth_context_owned.clone(),
        )?;

        // Execute the script (fetched and bundled above)
        crate::bytecode::eval_program(&ctx, &script_uri_owned, &executable_code)?;

        Ok(())
    });

    if let Err(e) = setup_exec {
        return Err(format!("JavaScript execution error: {}", e));
    }

    let result_exec = call_and_settle(
        &rt,
        &ctx,
        &script_uri_owned,
        &format!("MCP tool handler '{}'", handler_function_owned),
        TransactionHandling::Auto,
        |ctx| -> Result<rquickjs::Promise<'_>, String> {
            let handler_result: rquickjs::Value = ctx
                .globals()
                .get(&handler_function_owned)
                .map_err(|e| format!("no handler {}: {}", handler_function_owned, e))?;
            let handler_func = handler_result.as_function().ok_or_else(|| {
                format!(
                    "handler '{}' not found, or not a function",
                    handler_function_owned
                )
            })?;

            let request_context = JsRequestContext {
                path: Some("/mcp/tools/call".to_string()),
                url: None,
                method: Some("POST".to_string()),
                headers: HashMap::new(),
                query_params: HashMap::new(),
                form_data: HashMap::new(),
                body: None,
                route_params: HashMap::new(),
                uploaded_files: Vec::new(),
            };

            let mut context_builder = JsHandlerContextBuilder::new(HandlerInvocationKind::McpTool)
                .with_script_metadata(&script_uri_owned, &handler_function_owned)
                .with_request(request_context)
                .with_invocation_id(invocation_id.clone())
                .with_args(arguments_owned)
                .with_metadata_value(
                    "mcp",
                    serde_json::json!({
                        "toolName": tool_name_owned
                    }),
                );

            if let Some(auth_context) = auth_context_owned.clone() {
                context_builder = context_builder.with_auth_context(auth_context);
            }

            let handler_context = context_builder
                .build(ctx)
                .map_err(|e| format!("build context: {}", e))?;

            // Set context as a global variable
            let global = ctx.globals();
            global
                .set("context", handler_context.clone())
                .map_err(|e| format!("set context global: {}", e))?;

            // A handler that throws before returning never reaches the queue, so
            // its transaction is rolled back here. One that rejects after an await
            // settles during the drain, and `call_and_settle` finishes it.
            let result_value = handler_func
                .call::<_, rquickjs::Value>((handler_context,))
                .map_err(|e| {
                    let details = extract_error_details(ctx, &e);
                    if crate::database::get_current_transaction_active() {
                        let _ = crate::database::Database::rollback_transaction();
                    }
                    format!("call handler: {}", details)
                })?;

            promise_resolve(ctx, result_value)
        },
        |ctx, result_value| -> Result<String, String> {
            // Convert the result to a JSON string
            if result_value.is_string() {
                result_value
                    .as_string()
                    .ok_or_else(|| "handler result was not a string".to_string())?
                    .to_string()
                    .map_err(|e| format!("read handler string: {}", e))
            } else {
                // Use JavaScript's JSON.stringify to convert any value to JSON
                let json_obj: rquickjs::Object = ctx
                    .globals()
                    .get("JSON")
                    .map_err(|e| format!("JSON global missing: {}", e))?;
                let json_stringify: rquickjs::Function = json_obj
                    .get("stringify")
                    .map_err(|e| format!("JSON.stringify missing: {}", e))?;
                json_stringify
                    .call((result_value,))
                    .map_err(|e| format!("stringify handler result: {}", e))
            }
        },
    );

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
    let invocation_id = crate::middleware::generate_request_id();
    let log_context = HandlerInvocationKind::StreamCustomization
        .log_context(invocation_id.clone(), Some(path_owned.clone()));

    // Fetch and bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`).
    let script_content = repository::fetch_script(&script_uri_owned)
        .ok_or_else(|| format!("no script for uri {}", script_uri_owned))?;
    let executable_code = transpile_if_needed(&script_uri_owned, &script_content)?;

    let rt = create_sandboxed_runtime(&current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;

    let setup_exec = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
        // Set up global functions with minimal security for customization function
        let config = GlobalSecurityConfig {
            enable_audit_logging: false,
            log_context: log_context.clone(),
            ..Default::default()
        };

        setup_secure_global_functions(
            &ctx,
            &script_uri_owned,
            UserContext::admin("stream-customization".to_string()),
            &config,
            None,
            auth_context.clone(),
        )?;

        // The script was fetched and bundled above.

        crate::bytecode::eval_program(&ctx, &script_uri_owned, &executable_code)?;

        Ok(())
    });

    if let Err(e) = setup_exec {
        return Err(format!("Customization function execution error: {}", e));
    }

    let result_exec = call_and_settle(
        &rt,
        &ctx,
        &script_uri_owned,
        &format!("Stream customization '{}'", function_name_owned),
        TransactionHandling::Auto,
        |ctx| -> Result<rquickjs::Promise<'_>, String> {
            let request_context = JsRequestContext {
                path: Some(path_owned.clone()),
                url: None,
                method: Some("GET".to_string()),
                headers: HashMap::new(),
                query_params: query_params_owned.clone(),
                form_data: HashMap::new(),
                body: None,
                route_params: HashMap::new(),
                uploaded_files: Vec::new(),
            };

            let mut context_builder =
                JsHandlerContextBuilder::new(HandlerInvocationKind::StreamCustomization)
                    .with_script_metadata(&script_uri_owned, &function_name_owned)
                    .with_request(request_context)
                    .with_invocation_id(invocation_id.clone())
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

            let handler_context = context_builder
                .build(ctx)
                .map_err(|e| format!("build context: {}", e))?;

            // Set context as a global variable so personalStorage and other APIs can access it
            let global = ctx.globals();
            global
                .set("context", handler_context.clone())
                .map_err(|e| format!("set context global: {}", e))?;

            // Get the customization function
            let customization_func: rquickjs::Function = global
                .get(&function_name_owned)
                .map_err(|_| format!("'{}' not found", function_name_owned))?;

            // A function that throws before returning never reaches the queue,
            // so its transaction is rolled back here. One that rejects after an
            // await settles during the drain, and `call_and_settle` finishes it.
            let result_value: rquickjs::Value =
                customization_func.call((handler_context,)).map_err(|e| {
                    let details = extract_error_details(ctx, &e);
                    if crate::database::get_current_transaction_active() {
                        let _ = crate::database::Database::rollback_transaction();
                    }
                    format!("call customization: {}", details)
                })?;

            promise_resolve(ctx, result_value)
        },
        |_ctx, result_value| {
            // Convert result to HashMap
            let mut filter_criteria = std::collections::HashMap::new();

            let Some(result_obj) = result_value.as_object() else {
                return Err("Expected object result".to_string());
            };

            for key_str in result_obj.keys::<String>().flatten() {
                if let Ok(value) = result_obj.get::<_, rquickjs::Value>(&key_str) {
                    if let Some(value_str) = value.as_string().and_then(|s| s.to_string().ok()) {
                        filter_criteria.insert(key_str.clone(), value_str);
                    } else {
                        return Err("Filter values must be strings".to_string());
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

/// What to evaluate, and under which budget.
pub struct EvalParams {
    pub script_uri: String,
    /// The snippet. Evaluated after the script's own program, in the same
    /// context, so it sees what that program defined.
    pub source: String,
    /// The identity the snippet runs as. The *caller's*, not the engine's — an
    /// evaluation is the caller executing code in a sandbox they may already
    /// write to, so it must not hand them capabilities they do not have. This
    /// is where an evaluation differs from `init()`, which a deploy runs as an
    /// administrator by definition.
    pub user_context: UserContext,
    pub timeout_ms: u64,
    /// Roll back the database writes the snippet makes. On by default.
    pub rollback: bool,
}

/// What one evaluation produced.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalOutcome {
    /// The snippet's value, run through `JSON.stringify` and re-parsed.
    ///
    /// Absent when the value has no JSON form — `undefined`, a function, a
    /// symbol — which `value_type` tells apart from a value that really was
    /// `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// The value's kind, always reported: `undefined`, `null`, `boolean`,
    /// `number`, `string`, `symbol`, `function`, `array` or `object`.
    ///
    /// `undefined` and `null` are indistinguishable in `value` alone — the
    /// first is absent, the second is a JSON null — and telling them apart is
    /// usually the whole question.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
    /// Why the value could not be serialized, when it could not — a circular
    /// structure, most often.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_error: Option<String>,
    /// Everything the snippet and the script's program wrote through `console`.
    pub console: Vec<ConsoleLine>,
    /// Lines dropped after [`MAX_CAPTURED_CONSOLE_LINES`], so a truncated
    /// capture is never mistaken for the whole of it.
    #[serde(skip_serializing_if = "is_zero")]
    pub console_dropped: usize,
    pub duration_ms: u64,
    /// Whether the run's transaction was rolled back. False when the caller
    /// asked for no rollback *and* when the snippet committed the transaction
    /// itself — this reports what happened, not what was requested.
    pub rolled_back: bool,
    /// The failure, when the snippet threw or ran out of budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

/// Evaluate `source` against `script_uri`'s program, capturing its output.
///
/// The script's prepared program is evaluated first, in the same context, so
/// the snippet sees what it defined: the script's own functions, the bindings
/// its entrypoint imported (the linker rewrites those into top-level
/// declarations), and `__asset_module_require__` for reaching a module the
/// entrypoint never exposed. Compilation uses `JS_EVAL_TYPE_GLOBAL`, which is
/// what puts those declarations in the realm rather than in a module scope.
///
/// Registrations do not take effect (`registration_phase: false`), for the
/// reason a test run does not either: a route or job registered here would
/// outlive the request and nothing would undo it.
///
/// Must run on a blocking thread — the isolating transaction is thread-local.
pub fn evaluate_snippet(params: &EvalParams) -> EvalOutcome {
    let started = Instant::now();

    // Declared before the rollback guard below so it drops *after* it: the
    // guard finishes its own transaction, and this only catches one the snippet
    // opened and left behind.
    let _stray = StrayTransaction::arm();

    macro_rules! fail_early {
        ($error:expr) => {
            return EvalOutcome {
                duration_ms: started.elapsed().as_millis() as u64,
                error: Some($error),
                ..EvalOutcome::default()
            }
        };
    }

    let Some(content) = repository::fetch_script(&params.script_uri) else {
        fail_early!(format!("Script '{}' not found", params.script_uri));
    };

    // Bundle before arming the interrupt deadline, as every other entry point
    // does: on a cold cache this fetches and transpiles every module the script
    // imports, which must not be charged to the budget meant for the snippet.
    let prepared = match module_loader::prepare_executable_program(&params.script_uri, &content) {
        Ok(prepared) => prepared,
        Err(e) => fail_early!(format!("Failed to bundle script: {}", e)),
    };

    let limits = ExecutionLimits {
        timeout_ms: params.timeout_ms,
        ..current_execution_limits()
    };

    let rt = match create_sandboxed_runtime(&limits) {
        Ok(rt) => rt,
        Err(e) => fail_early!(e),
    };
    let ctx = match Context::full(&rt) {
        Ok(ctx) => ctx,
        Err(e) => fail_early!(format!("Failed to create context: {}", e)),
    };

    let console: ConsoleSink = std::sync::Arc::new(std::sync::Mutex::new(
        crate::security::secure_globals::ConsoleCapture::default(),
    ));

    // Held for the whole evaluation and never committed: `TransactionGuard`
    // rolls back when it drops, including on an early return.
    let rollback_guard = if params.rollback {
        match crate::database::Database::begin_transaction(Some(params.timeout_ms)) {
            Ok(guard) => Some(guard),
            Err(e) => fail_early!(format!(
                "could not isolate the evaluation in a transaction: {}",
                e
            )),
        }
    } else {
        None
    };

    let mut outcome = run_snippet(&rt, &ctx, params, &prepared.code, &console);

    // A snippet that called `database.commitTransaction()` itself has already
    // committed the guard's transaction, so the guard has nothing left to roll
    // back. Report what is true rather than echoing the request.
    let still_open = crate::database::get_current_transaction_active();
    outcome.rolled_back = rollback_guard.is_some() && still_open;
    drop(rollback_guard);

    if let Ok(capture) = console.lock() {
        outcome.console = capture.lines.clone();
        outcome.console_dropped = capture.dropped;
    }
    outcome.duration_ms = started.elapsed().as_millis() as u64;

    // Context must drop before the runtime (see `ensure_clean_shutdown`).
    drop(ctx);
    drop(rt);
    outcome
}

/// Install the globals, load the script's program, and evaluate the snippet.
///
/// Split out so the context lifetime has a name, as `run_test_module` is.
fn run_snippet(
    rt: &Runtime,
    context: &Context,
    params: &EvalParams,
    program: &str,
    console: &ConsoleSink,
) -> EvalOutcome {
    // Installing the globals, loading the program and evaluating the snippet
    // all need the context; draining the queue the snippet may have filled
    // cannot hold one. So the snippet's value is persisted and picked up again
    // below.
    type Prepared = rquickjs::Persistent<rquickjs::Promise<'static>>;
    // Boxed so the error variant does not dwarf the success one.
    let prepared = context.with(|ctx| -> Result<Prepared, Box<EvalOutcome>> {
        let ctx = &ctx;
        let invocation_id = crate::middleware::generate_request_id();
        let security_config = GlobalSecurityConfig {
            // Same rule as a test run: the APIs stay callable and report that they
            // did nothing, rather than installing registrations that outlive the
            // request with no rollback to undo them.
            registration_phase: false,
            enable_audit_logging: false,
            dry_run_sink: None,
            console_sink: Some(std::sync::Arc::clone(console)),
            log_context: HandlerInvocationKind::Eval.log_context(invocation_id.clone(), None),
        };

        let mut outcome = EvalOutcome::default();

        if let Err(e) = setup_secure_global_functions(
            ctx,
            &params.script_uri,
            params.user_context.clone(),
            &security_config,
            None,
            None,
        ) {
            outcome.error = Some(format!(
                "install globals: {}",
                extract_error_details(ctx, &e)
            ));
            return Err(Box::new(outcome));
        }

        match JsHandlerContextBuilder::new(HandlerInvocationKind::Eval)
            .with_script_metadata(params.script_uri.clone(), "eval")
            .with_invocation_id(invocation_id.clone())
            .build(ctx)
        {
            // Set before the program runs: its top level can already reach for
            // `context`, exactly as a test module's can.
            Ok(handler_context) => {
                if let Err(e) = ctx.globals().set("context", handler_context) {
                    outcome.error = Some(format!("set context global: {}", e));
                    return Err(Box::new(outcome));
                }
            }
            Err(e) => {
                outcome.error = Some(format!("build context: {}", e));
                return Err(Box::new(outcome));
            }
        }

        if let Err(e) = crate::bytecode::eval_program(ctx, &params.script_uri, program) {
            outcome.error = Some(format!(
                "the script's own program failed to load: {}",
                extract_error_details(ctx, &e)
            ));
            return Err(Box::new(outcome));
        }

        // The bundler's module lookup is an implementation detail with an
        // implementation detail's name. Alias it so a snippet has something
        // reasonable to call, for the cases `import` cannot express — reaching a
        // module by a path computed at run time, say.
        install_require_alias(ctx);

        // Rewrite the snippet's imports the way every module's are rewritten, so
        // `import` means in a snippet exactly what it means in the script.
        let snippet = match module_loader::prepare_snippet(&params.script_uri, &params.source) {
            Ok(snippet) => snippet,
            Err(e) => {
                outcome.error = Some(e.to_string());
                return Err(Box::new(outcome));
            }
        };

        // Check the imports against the graph before running, so a path that is not
        // in the bundle is reported as what it is rather than as an unknown module
        // thrown from inside the prelude.
        if let Some(error) = unresolvable_imports(ctx, &snippet.dependencies) {
            outcome.error = Some(error);
            return Err(Box::new(outcome));
        }

        // The snippet is compiled fresh every time and deliberately not cached:
        // it is different on essentially every call, and caching it would evict
        // the script programs the cache exists for.
        let value = match ctx.eval::<rquickjs::Value, _>(snippet.code.as_bytes()) {
            Ok(value) => value,
            Err(e) => {
                outcome.error = Some(extract_error_details(ctx, &e));
                return Err(Box::new(outcome));
            }
        };

        match promise_resolve(ctx, value) {
            Ok(promise) => Ok(rquickjs::Persistent::save(ctx, promise)),
            Err(e) => {
                outcome.error = Some(e);
                Err(Box::new(outcome))
            }
        }
    });

    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(outcome) => return *outcome,
    };

    report_unhandled(&params.script_uri, drain_jobs(rt));

    context.with(|ctx| {
        let mut outcome = EvalOutcome::default();

        let promise = match prepared.restore(&ctx) {
            Ok(promise) => promise,
            Err(e) => {
                outcome.error = Some(format!("restore snippet result: {}", e));
                return outcome;
            }
        };

        // A snippet that awaits settles during the drain above. One that
        // returns a promise nothing can settle — `new Promise(() => {})` — is
        // reported as that, rather than as the opaque object it is.
        let value = match unwrap_settled(&ctx, promise, "The snippet") {
            Ok(value) => value,
            Err(details) => {
                outcome.error = Some(details);
                return outcome;
            }
        };

        outcome.value_type = Some(js_type_of(&value));

        match json_stringify(&ctx, &value) {
            // `JSON.stringify` yields no string at all for `undefined`, a
            // function or a symbol. That is not an error: `valueType` already
            // said what it was, and forcing a null here would claim the snippet
            // returned one.
            Ok(None) => {}
            Ok(Some(json)) => match serde_json::from_str(&json) {
                Ok(parsed) => outcome.value = Some(parsed),
                Err(e) => outcome.value_error = Some(format!("value is not valid JSON: {}", e)),
            },
            Err(e) => {
                outcome.value_error = Some(format!(
                    "value could not be serialized (a circular structure, most likely): {}",
                    e
                ));
            }
        }

        outcome
    })
}
/// Give the snippet a `require()` for the bundler's module table.
///
/// Absent when the script imports nothing: the bundle only emits its module
/// prelude for a program that has modules, so there is nothing to alias and
/// nothing to import either.
fn install_require_alias(ctx: &rquickjs::Ctx<'_>) {
    let _ = ctx.eval::<(), _>(
        r#"
        if (typeof __asset_module_require__ === "function" && typeof globalThis.require !== "function") {
            globalThis.require = __asset_module_require__;
        }
        "#,
    );
}

/// Module paths the bundle holds, or an empty list when it has no modules.
fn bundled_module_paths(ctx: &rquickjs::Ctx<'_>) -> Vec<String> {
    ctx.eval::<Vec<String>, _>(
        r#"
        typeof __asset_module_factories__ === "object"
            ? Object.keys(__asset_module_factories__)
            : []
        "#,
    )
    .unwrap_or_default()
}

/// Explain any import the bundle cannot satisfy, or `None` when all resolve.
///
/// The bundle holds every module reachable from the entrypoint, so a path that
/// is missing is one nothing the script imports leads to — dead code, or a
/// typo. Listing what *is* there turns the second case into a one-line fix.
fn unresolvable_imports(ctx: &rquickjs::Ctx<'_>, dependencies: &[String]) -> Option<String> {
    if dependencies.is_empty() {
        return None;
    }

    let available = bundled_module_paths(ctx);
    let missing: Vec<&str> = dependencies
        .iter()
        .filter(|dependency| !available.contains(dependency))
        .map(String::as_str)
        .collect();

    if missing.is_empty() {
        return None;
    }

    let available_list = if available.is_empty() {
        "this script imports no modules at all".to_string()
    } else {
        format!("importable here: {}", available.join(", "))
    };

    Some(format!(
        "Cannot import {} - not part of this script's module graph. A snippet can import any \
         module the script's entrypoint reaches, directly or through another module; one it \
         never reaches is not in the bundle. {}.",
        missing
            .iter()
            .map(|path| format!("'{}'", path))
            .collect::<Vec<_>>()
            .join(", "),
        available_list
    ))
}

/// The value's kind, as JavaScript names it.
///
/// Close to `typeof`, with the two deviations that make it useful in a report:
/// `null` and `array` are named rather than both collapsing into `"object"`.
/// Built from the predicates rather than the runtime's own type enum, which
/// splits `number` into int and float — an engine-internal distinction that
/// would only puzzle the reader.
fn js_type_of(value: &rquickjs::Value<'_>) -> String {
    if value.is_undefined() {
        "undefined"
    } else if value.is_null() {
        "null"
    } else if value.is_bool() {
        "boolean"
    } else if value.is_number() {
        "number"
    } else if value.is_string() {
        "string"
    } else if value.is_symbol() {
        "symbol"
    } else if value.is_function() {
        "function"
    } else if value.is_array() {
        "array"
    } else if value.is_object() {
        "object"
    } else {
        // BigInt and anything the runtime adds later.
        return format!("{:?}", value.type_of()).to_lowercase();
    }
    .to_string()
}

/// `JSON.stringify(value)`, or `None` where it yields nothing.
fn json_stringify<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: &rquickjs::Value<'js>,
) -> Result<Option<String>, String> {
    let json: rquickjs::Object<'js> = ctx
        .globals()
        .get("JSON")
        .map_err(|e| format!("JSON global missing: {}", e))?;
    let stringify: Function<'js> = json
        .get("stringify")
        .map_err(|e| format!("JSON.stringify missing: {}", e))?;
    stringify
        .call::<_, Option<String>>((value.clone(),))
        .map_err(|e| extract_error_details(ctx, &e))
}

/// A failed init() attempt, carrying whatever the script managed to register
/// before it failed.
#[derive(Debug, Clone)]
pub struct InitFailure {
    pub error: String,
    /// Routes registered before the failure. Empty when init() failed before
    /// reaching any `routeRegistry.registerRoute` call.
    pub registrations: RouteRegistrations,
}

impl std::fmt::Display for InitFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

/// Calls the init() function in a script if it exists
///
/// This function executes a script and checks if it has an `init()` function defined.
/// If found, it calls the function with the provided context.
///
/// Returns:
/// - `Ok(Some(registrations))` if init() was found and completed
/// - `Ok(None)` if no init() function exists (not an error)
/// - `Err(InitFailure)` if init() exists but threw or exceeded its budget —
///   including any routes it registered before that point
pub fn call_init_if_exists(
    script_uri: &str,
    script_content: &str,
    context: crate::script_init::InitContext,
) -> Result<Option<RouteRegistrations>, InitFailure> {
    call_init_if_exists_with_timeout(
        script_uri,
        script_content,
        context,
        current_execution_limits().timeout_ms,
    )
}

/// Like [`call_init_if_exists`], but with an explicit wall-clock budget so init()
/// can be granted a longer timeout than regular handler execution
/// (config `javascript.init_timeout_ms`).
pub fn call_init_if_exists_with_timeout(
    script_uri: &str,
    script_content: &str,
    context: crate::script_init::InitContext,
    timeout_ms: u64,
) -> Result<Option<RouteRegistrations>, InitFailure> {
    let outcome = run_registration_pass(script_uri, script_content, context, timeout_ms, None);

    match outcome.error {
        Some(error) => Err(InitFailure {
            error,
            registrations: outcome.pass.registrations,
        }),
        None if outcome.pass.had_init => Ok(Some(outcome.pass.registrations)),
        None => Ok(None),
    }
}

/// What to dry-run, and under which budget.
pub struct DryRunParams {
    pub script_uri: String,
    /// The source to check. Taken as a parameter rather than read from the
    /// repository so a candidate can be checked *before* it is deployed — the
    /// difference between finding a broken `init()` and shipping one.
    pub script_content: String,
    pub timeout_ms: u64,
    /// Roll back the database writes `init()` makes. On by default for the same
    /// reason it is for a test run: a check should not leave rows behind.
    pub rollback: bool,
    /// Where registrations are recorded, supplied by the caller rather than
    /// made here.
    ///
    /// The caller keeps a clone, which is the only way partial results survive
    /// an abandoned run: when an outer timeout gives up on the blocking thread,
    /// the thread — and every local it owns — goes with it, but an `Arc` the
    /// caller still holds does not.
    pub sink: RegistrationSink,
}

/// Run a script's registration pass for its findings alone, changing nothing.
///
/// Every registry write is withheld and recorded instead (see
/// [`GlobalSecurityConfig::dry_run_sink`]), message dispatch is suppressed, and
/// database writes are rolled back when `rollback` is set. What that does *not*
/// cover is everything else `init()` can reach: an outbound `fetch`, a secret
/// read, a write to another system. A dry run executes the script's own code, so
/// side effects the engine does not mediate still happen.
///
/// Must run on a blocking thread: the transaction that isolates the run is
/// thread-local, so the rollback only covers work done on the thread that
/// opened it — the same constraint the test runner works under.
pub fn dry_run_registration_pass(params: &DryRunParams) -> RegistrationPassOutcome {
    // Held for the whole pass and never committed: `TransactionGuard` rolls
    // back when it drops, including on an early return.
    let rollback_guard = if params.rollback {
        match crate::database::Database::begin_transaction(Some(params.timeout_ms)) {
            Ok(guard) => Some(guard),
            Err(e) => {
                return RegistrationPassOutcome {
                    pass: RegistrationPass::default(),
                    error: Some(format!(
                        "could not isolate the check in a transaction: {}",
                        e
                    )),
                };
            }
        }
    } else {
        None
    };

    let context = crate::script_init::InitContext::new(
        params.script_uri.clone(),
        /* is_startup */ false,
    );

    let outcome = run_registration_pass(
        &params.script_uri,
        &params.script_content,
        context,
        params.timeout_ms,
        Some(std::sync::Arc::clone(&params.sink)),
    );

    drop(rollback_guard);
    outcome
}

/// A registration a script made that names a script function the program does
/// not define — the delegate every dispatch path resolves by name at call time
/// (`globals.get::<_, Function>(handler_name)`), so a name that is not there is
/// a 500 on the first request rather than an error at deploy.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingHandler {
    pub kind: RegistrationKind,
    /// What the registration is keyed by — the path, operation or tool name.
    pub name: String,
    /// The delegate name that could not be resolved.
    pub handler: String,
    /// What the global turned out to be, when a global of that name exists but
    /// is not callable. `None` when nothing of that name is defined at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found_type: Option<String>,
}

/// What one registration pass produced.
#[derive(Debug, Default)]
pub struct RegistrationPass {
    /// Routes `routeRegistry.registerRoute` collected, keyed by `(path, method)`.
    pub registrations: RouteRegistrations,
    /// False when the script defines no `init()` — in which case nothing after
    /// the program's top level ran.
    pub had_init: bool,
    /// Every registration the pass saw, in the order the script made them.
    /// Populated only on a dry run, which is the only mode that records the
    /// registries beyond `registerRoute`.
    pub collected: Vec<CollectedRegistration>,
    /// Registrations whose delegate the program does not define. Checked only
    /// on a dry run.
    pub missing_handlers: Vec<MissingHandler>,
    /// Wall-clock time the pass took, covering the bundle, the program's top
    /// level and `init()` — the same three steps a deploy pays for.
    pub duration_ms: u64,
    /// True when the run hit its ceiling and was interrupted part-way.
    ///
    /// Decided by comparing the elapsed time against the deadline the interrupt
    /// was armed with, not by matching on the runtime's error text — which is
    /// the bare word "interrupted" and tells a caller nothing it can act on.
    pub timed_out: bool,
}

/// A registration pass and how it ended.
///
/// The pass is reported whether or not it failed: a script that registers its
/// routes and *then* throws has still told the caller what it registered, and
/// both callers want that — the deploy path installs partial routes so a first
/// deploy with a slow `init()` is reachable, and the check reports them as
/// findings.
#[derive(Debug)]
pub struct RegistrationPassOutcome {
    pub pass: RegistrationPass,
    /// `None` when the pass completed.
    pub error: Option<String>,
}

/// Evaluate `script_content` and run its `init()`, collecting what it registers.
///
/// With `dry_run_sink` set, no registry that outlives this call is written to
/// and every registration is recorded in the sink instead — see
/// [`GlobalSecurityConfig::dry_run_sink`]. That mode also resolves each
/// registration's delegate against the program's globals, which is the check
/// `/engine/check` exists for and which costs nothing here because the context
/// that would answer the question is still alive.
fn run_registration_pass(
    script_uri: &str,
    script_content: &str,
    context: crate::script_init::InitContext,
    timeout_ms: u64,
    dry_run_sink: Option<RegistrationSink>,
) -> RegistrationPassOutcome {
    use std::cell::RefCell;
    use std::rc::Rc;

    debug!("Checking for init() function in script: {}", script_uri);

    let started = Instant::now();
    // An `init()` that opens a transaction and never finishes it would leave it
    // on the thread for the next invocation to inherit.
    let _stray = StrayTransaction::arm();
    let sink_for_report = dry_run_sink.clone();
    let missing_handlers: Rc<RefCell<Vec<MissingHandler>>> = Rc::new(RefCell::new(Vec::new()));

    macro_rules! fail_early {
        ($error:expr) => {
            return RegistrationPassOutcome {
                pass: RegistrationPass {
                    duration_ms: started.elapsed().as_millis() as u64,
                    ..RegistrationPass::default()
                },
                error: Some($error),
            }
        };
    }

    let limits = ExecutionLimits {
        timeout_ms,
        ..current_execution_limits()
    };

    // Bundle before arming the runtime's interrupt deadline. init() is the step
    // a deploy depends on, and it runs with the caches cold by definition — with
    // the bundle inside the budget, a script with many imported modules could
    // spend its whole init() allowance fetching and transpiling them.
    let executable_code = match transpile_if_needed(script_uri, script_content) {
        Ok(code) => code,
        Err(e) => fail_early!(format!("Transpilation failed: {}", e)),
    };

    let rt = match create_sandboxed_runtime(&limits) {
        Ok(rt) => rt,
        Err(e) => fail_early!(e),
    };
    // Mirrors the deadline `create_sandboxed_runtime` just armed the interrupt
    // with, so a run the interrupt stopped can be told apart from one that
    // simply failed — without matching on QuickJS error text, which is the bare
    // word "interrupted". Taken here rather than from `started` because the
    // interrupt's own deadline runs from *after* the bundle: measuring from
    // before it would call a genuine failure a timeout whenever bundling was
    // slow. Same approach as `execute_test_module`.
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);

    let ctx = match Context::full(&rt) {
        Ok(ctx) => ctx,
        Err(e) => fail_early!(format!("Failed to create context: {}", e)),
    };

    // Create registrations map to capture routeRegistry.registerRoute() calls during init
    let registrations = Rc::new(RefCell::new(HashMap::new()));
    let uri_owned = script_uri.to_string();
    // One id for the whole init pass, so the program's top-level output and
    // init()'s own output read as the single startup they are.
    let invocation_id = crate::middleware::generate_request_id();

    // Shared location for detailed error message
    let error_details: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let error_details_clone = Rc::clone(&error_details);

    // Carries `init()`'s promise out of the context so the microtask queue can
    // be drained, which holding a context guard makes impossible.
    type InitPromise = rquickjs::Persistent<rquickjs::Promise<'static>>;
    let init_promise: Rc<RefCell<Option<InitPromise>>> = Rc::new(RefCell::new(None));
    let init_promise_clone = Rc::clone(&init_promise);

    let result = ctx
        .with(|ctx| -> Result<bool, rquickjs::Error> {
            // Set up secure global functions with minimal config for init
            let config = GlobalSecurityConfig {
                // `init()` is the registration phase, and the script's
                // top-level program runs under it too.
                registration_phase: true,
                enable_audit_logging: false,
                dry_run_sink: dry_run_sink.clone(),
                // A check reports diagnostics, not output; console writes go to
                // the script's log as they would on a deploy.
                console_sink: None,
                log_context: HandlerInvocationKind::Init.log_context(invocation_id.clone(), None),
            };

            // Create the register function that captures registrations
            let regs_clone = Rc::clone(&registrations);
            let uri_clone = uri_owned.clone();
            // Routes are collected through this closure rather than through the
            // sink, so on a dry run they have to be mirrored into it — the
            // delegate check downstream reads the sink, and a route handler is
            // the delegate most worth checking.
            let route_sink = dry_run_sink.clone();
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
                    if let Some(sink) = route_sink.as_ref()
                        && let Ok(mut collected) = sink.lock()
                    {
                        collected.push(
                            CollectedRegistration::new(RegistrationKind::Route, path)
                                .with_method(method)
                                .with_handler(route_metadata.handler_name.clone()),
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
            );

            if let Err(ref e) = setup_result {
                let details = extract_error_details(&ctx, e);
                if let Ok(mut error_ref) = error_details_clone.try_borrow_mut() {
                    *error_ref = Some(details);
                }
            }
            setup_result?;

            // Execute the script to define functions (bundled above)
            let eval_result = crate::bytecode::eval_program(&ctx, &uri_owned, &executable_code);
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
                .with_invocation_id(invocation_id.clone())
                .build(&ctx)?;

            // Call init function with context. An `async init()` suspends at
            // its first await and hands back a promise; the queue that settles
            // it can only be drained outside the context, so the promise is
            // persisted for the caller to finish.
            debug!("Calling init() function for script: {}", script_uri);
            let call_result = init_func
                .call::<_, Value>((handler_context,))
                .and_then(|value| match promise_resolve(&ctx, value) {
                    Ok(promise) => Ok(rquickjs::Persistent::save(&ctx, promise)),
                    Err(details) => Err(rquickjs::Error::new_from_js_message(
                        "init", "promise", details,
                    )),
                });

            match call_result {
                Ok(ref promise) => {
                    if let Ok(mut slot) = init_promise_clone.try_borrow_mut() {
                        *slot = Some(promise.clone());
                    }
                }
                Err(ref e) => {
                    let details = extract_error_details(&ctx, e);
                    if let Ok(mut error_ref) = error_details_clone.try_borrow_mut() {
                        *error_ref = Some(details);
                    }
                }
            }
            // Resolve delegates before propagating a failure: a script whose
            // init() registered a handful of routes and then threw still has
            // those handler names worth reporting on, and the context that can
            // answer holds only until this closure returns.
            if dry_run_sink.is_some() {
                record_missing_handlers(&ctx, &dry_run_sink, &missing_handlers);
            }

            call_result?;

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
        });

    // `init()` may have suspended at an await. Draining now runs whatever it
    // queued — including any `registerRoute` calls made after the await, which
    // is why this happens before the registrations are read below.
    let result = match result {
        Ok(had_init) => {
            report_unhandled(script_uri, drain_jobs(&rt));

            let settled = init_promise.take().map(|promise| {
                ctx.with(|ctx| {
                    let promise = promise
                        .restore(&ctx)
                        .map_err(|e| format!("restore init result: {}", e))?;
                    unwrap_settled(&ctx, promise, "init()").map(|_| ())
                })
            });

            match settled {
                Some(Err(details)) => Err(format!("Init function error: {}", details)),
                _ => {
                    if had_init {
                        info!("Successfully called init() for script: {}", script_uri);
                    }
                    Ok(had_init)
                }
            }
        }
        Err(e) => Err(e),
    };

    // Routes registered before init() threw or ran out of budget are reported
    // with the failure rather than dropped. Scripts whose init() registers first
    // and does its slow setup afterwards then come up routable even when that
    // setup does not finish; the caller decides whether to install them.
    let registered = registrations
        .try_borrow()
        .map(|regs| regs.clone())
        .unwrap_or_default();

    let (had_init, error) = match result {
        Ok(had_init) => {
            if had_init {
                info!(
                    "Init() for script {} registered {} routes",
                    script_uri,
                    registered.len()
                );
            }
            (had_init, None)
        }
        Err(error) => {
            if !registered.is_empty() {
                warn!(
                    "Init() for script {} failed after registering {} routes: {}",
                    script_uri,
                    registered.len(),
                    error
                );
            }
            // The script has an init() - reaching a failure is proof it was
            // called - so a caller that distinguishes "no init()" from "init()
            // failed" gets the second answer, not the first.
            (true, Some(error))
        }
    };

    let outcome = RegistrationPassOutcome {
        pass: RegistrationPass {
            registrations: registered,
            had_init,
            collected: sink_for_report
                .and_then(|sink| sink.lock().ok().map(|collected| collected.clone()))
                .unwrap_or_default(),
            missing_handlers: missing_handlers.take(),
            duration_ms: started.elapsed().as_millis() as u64,
            timed_out: error.is_some() && Instant::now() >= deadline,
        },
        error,
    };

    // Ensure clean shutdown: drop Context before Runtime
    match ensure_clean_shutdown(ctx, Ok::<_, String>(outcome)) {
        Ok(outcome) => outcome,
        // `ensure_clean_shutdown` only ever propagates the value it was handed,
        // which is `Ok` here; this arm exists to satisfy the type.
        Err(error) => RegistrationPassOutcome {
            pass: RegistrationPass::default(),
            error: Some(error),
        },
    }
}

/// Resolve every delegate the sink recorded against the program's globals,
/// appending the ones that do not answer to `missing`.
///
/// This mirrors dispatch exactly: each entry point looks its handler up as a
/// global function by name (`globals.get::<_, Function>(handler_name)`), so a
/// name that is absent here is a 500 on the first request that reaches it. Doing
/// it by execution rather than by parsing is what makes it exact — a handler
/// name assembled at runtime is checked the same as a literal one.
fn record_missing_handlers(
    ctx: &rquickjs::Ctx<'_>,
    sink: &Option<RegistrationSink>,
    missing: &std::rc::Rc<std::cell::RefCell<Vec<MissingHandler>>>,
) {
    let Some(sink) = sink.as_ref() else {
        return;
    };
    let Ok(collected) = sink.lock() else {
        return;
    };
    let Ok(mut missing) = missing.try_borrow_mut() else {
        return;
    };

    let globals = ctx.globals();
    for registration in collected.iter() {
        let Some(handler) = registration.handler.as_deref() else {
            continue;
        };
        let found_type = match globals.get::<_, rquickjs::Value>(handler) {
            Ok(value) if value.is_function() => continue,
            // A global of that name exists but is not callable — a config object
            // where a function was meant, usually. Naming what it is turns out
            // to be the whole fix.
            Ok(value) => Some(format!("{:?}", value.type_of()).to_lowercase()),
            Err(_) => None,
        };
        missing.push(MissingHandler {
            kind: registration.kind,
            name: registration.name.clone(),
            handler: handler.to_string(),
            found_type,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_registry;
    use std::sync::{Arc, Once, OnceLock};

    static INIT: Once = Once::new();
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

    fn setup_db() {
        INIT.call_once(|| {
            // Skip if running in offline mode (CI/CD)
            if std::env::var("DATABASE_URL").is_err() {
                // For offline mode, we can't run tests that need database
                return;
            }

            let pool = sqlx::PgPool::connect_lazy(
                "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
            )
            .unwrap();
            let db = Arc::new(crate::database::Database::from_pool(pool.clone()));
            crate::database::initialize_global_database(db);

            // Generate and initialize server ID
            let server_id = crate::notifications::generate_server_id();
            crate::notifications::initialize_server_id(server_id.clone());

            // Initialize PostgresRepository with pool and server_id
            let repo = crate::repository::PostgresRepository::new(pool, server_id);
            crate::repository::initialize_repository(repo);
        });
    }

    fn get_runtime() -> &'static tokio::runtime::Runtime {
        RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
    }

    fn unique_test_id(prefix: &str) -> String {
        format!("{}-{}", prefix, rand::random::<u64>())
    }

    #[test]
    fn test_interrupt_handler_stops_infinite_loop() {
        let limits = ExecutionLimits {
            timeout_ms: 200,
            ..ExecutionLimits::default()
        };
        let rt = create_sandboxed_runtime(&limits).expect("runtime creation failed");
        let ctx = Context::full(&rt).expect("context creation failed");
        let start = Instant::now();
        let result: Result<(), rquickjs::Error> =
            ctx.with(|ctx| ctx.eval::<(), _>("while(true){}"));
        assert!(result.is_err(), "infinite loop must be interrupted");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "interrupt should fire near the {}ms deadline, took {:?}",
            limits.timeout_ms,
            start.elapsed()
        );
    }

    #[test]
    fn test_memory_limit_stops_runaway_allocation() {
        let limits = ExecutionLimits {
            timeout_ms: 10_000,
            max_memory_mb: 8,
            ..ExecutionLimits::default()
        };
        let rt = create_sandboxed_runtime(&limits).expect("runtime creation failed");
        let ctx = Context::full(&rt).expect("context creation failed");
        let start = Instant::now();
        let result: Result<(), rquickjs::Error> = ctx.with(|ctx| {
            ctx.eval::<(), _>("const a = []; while(true) { a.push('x'.repeat(1024 * 1024)); }")
        });
        assert!(
            result.is_err(),
            "allocation beyond the memory limit must fail"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "memory limit (not the timeout) should stop the script, took {:?}",
            start.elapsed()
        );
    }

    // Check if we should skip database-dependent tests
    fn should_skip_db_tests() -> bool {
        std::env::var("DATABASE_URL").is_err()
    }

    // Shadow the super::execute_script with one that ensures setup
    fn execute_script(uri: &str, content: &str) -> ScriptExecutionResult {
        if should_skip_db_tests() {
            // Return a placeholder result when database is not available
            return ScriptExecutionResult {
                registrations: HashMap::new(),
                success: false,
                error: Some("Test skipped: DATABASE_URL not set".to_string()),
                execution_time_ms: 0,
            };
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        super::execute_script(uri, content)
    }

    // Shadow execute_script_secure
    fn execute_script_secure(
        uri: &str,
        content: &str,
        user_context: crate::security::UserContext,
    ) -> ScriptExecutionResult {
        if should_skip_db_tests() {
            return ScriptExecutionResult {
                registrations: HashMap::new(),
                success: false,
                error: Some("Test skipped: DATABASE_URL not set".to_string()),
                execution_time_ms: 0,
            };
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        super::execute_script_secure(uri, content, user_context)
    }

    // Shadow execute_script_for_request_secure
    fn execute_script_for_request_secure(
        params: RequestExecutionParams,
    ) -> Result<JsHttpResponse, String> {
        if should_skip_db_tests() {
            return Err("Test skipped: DATABASE_URL not set".to_string());
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        super::execute_script_for_request_secure(params)
    }

    // Shadow execute_graphql_resolver
    fn execute_graphql_resolver(params: GraphqlResolverExecutionParams) -> Result<String, String> {
        if should_skip_db_tests() {
            return Err("Test skipped: DATABASE_URL not set".to_string());
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        super::execute_graphql_resolver(params)
    }

    fn execute_mcp_tool_handler(
        script_uri: &str,
        handler_function: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        auth_context: Option<crate::auth::JsAuthContext>,
        user_context: crate::security::UserContext,
    ) -> Result<String, String> {
        if should_skip_db_tests() {
            return Err("Test skipped: DATABASE_URL not set".to_string());
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        super::execute_mcp_tool_handler(
            script_uri,
            handler_function,
            tool_name,
            arguments,
            auth_context,
            user_context,
        )
    }

    fn setup_db_for_test() {
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
    }

    #[test]
    fn test_execute_script_simple_registration() {
        if should_skip_db_tests() {
            return;
        }
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
    fn test_execute_mcp_tool_handler_includes_request_auth_context() {
        if should_skip_db_tests() {
            return;
        }

        let _lock = crate::repository::GLOBAL_TEST_LOCK.lock().unwrap();
        setup_db_for_test();

        let script_uri = unique_test_id("test-mcp-auth-context");
        let user_id = unique_test_id("user");
        let content = r#"
            function toolHandler(context) {
                return {
                    kind: context.kind,
                    toolName: context.meta.mcp.toolName,
                    auth: {
                        isAuthenticated: context.request.auth.isAuthenticated,
                        isAdmin: context.request.auth.isAdmin,
                        isEditor: context.request.auth.isEditor,
                        userId: context.request.auth.userId,
                        userEmail: context.request.auth.userEmail,
                        userName: context.request.auth.userName,
                        provider: context.request.auth.provider
                    }
                };
            }
        "#;

        let _ = repository::upsert_script(&script_uri, content);
        let result = execute_mcp_tool_handler(
            &script_uri,
            "toolHandler",
            "whoami",
            serde_json::json!({}),
            Some(crate::auth::JsAuthContext::authenticated(
                user_id.clone(),
                Some("user@example.com".to_string()),
                Some("Test User".to_string()),
                "github".to_string(),
                false,
                true,
            )),
            UserContext::authenticated(user_id.clone()),
        )
        .expect("MCP tool handler should execute successfully");

        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("Result should be valid JSON");

        assert_eq!(parsed["kind"], "mcpTool");
        assert_eq!(parsed["toolName"], "whoami");
        assert_eq!(parsed["auth"]["isAuthenticated"], true);
        assert_eq!(parsed["auth"]["isAdmin"], false);
        assert_eq!(parsed["auth"]["isEditor"], true);
        assert_eq!(parsed["auth"]["userId"], user_id);
        assert_eq!(parsed["auth"]["userEmail"], "user@example.com");
        assert_eq!(parsed["auth"]["userName"], "Test User");
        assert_eq!(parsed["auth"]["provider"], "github");
    }

    #[test]
    fn test_execute_mcp_tool_handler_uses_caller_capabilities() {
        if should_skip_db_tests() {
            return;
        }

        let _lock = crate::repository::GLOBAL_TEST_LOCK.lock().unwrap();
        setup_db_for_test();

        let script_uri = unique_test_id("test-mcp-caller-capabilities");
        let user_id = unique_test_id("user");
        let target_asset = unique_test_id("asset");
        let content = r#"
            function toolHandler(context) {
                return {
                    deleteResult: assetStorage.deleteAsset(context.args.targetAsset),
                    isAdmin: context.request.auth.isAdmin,
                    userId: context.request.auth.userId
                };
            }
        "#;

        let _ = repository::upsert_script(&script_uri, content);
        let result = execute_mcp_tool_handler(
            &script_uri,
            "toolHandler",
            "delete-check",
            serde_json::json!({
                "targetAsset": target_asset,
            }),
            Some(crate::auth::JsAuthContext::authenticated(
                user_id.clone(),
                Some("member@example.com".to_string()),
                Some("Member User".to_string()),
                "github".to_string(),
                false,
                false,
            )),
            UserContext::authenticated(user_id.clone()),
        )
        .expect("MCP tool handler should execute successfully");

        let parsed: serde_json::Value =
            serde_json::from_str(&result).expect("Result should be valid JSON");

        assert_eq!(parsed["isAdmin"], false);
        assert_eq!(parsed["userId"], user_id);
        // The caller holds no DeleteAssets capability, so the handler's delete
        // is refused rather than running with the engine's own rights.
        let delete_result = parsed["deleteResult"]
            .as_str()
            .expect("deleteResult should be a string");
        assert!(
            delete_result.starts_with("Error:"),
            "expected a capability error, got: {}",
            delete_result
        );
    }

    #[test]
    fn test_execute_script_multiple_registrations() {
        if should_skip_db_tests() {
            return;
        }
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
        if should_skip_db_tests() {
            return;
        }
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
        if should_skip_db_tests() {
            return;
        }
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
        if should_skip_db_tests() {
            return;
        }
        let result = execute_script("empty-script", "");

        assert!(result.success, "Empty script should succeed");
        assert!(result.error.is_none());
        assert!(result.registrations.is_empty());
    }

    #[test]
    fn test_execute_script_with_console_log() {
        if should_skip_db_tests() {
            return;
        }
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_graphql_resolver_simple() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_graphql_resolver_with_args() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_graphql_resolver_returning_object() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
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
        if should_skip_db_tests() {
            return;
        }
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_graphql_resolver_nonexistent_function() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_graphql_resolver_with_runtime_exception() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
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

        if should_skip_db_tests() {
            return;
        }
        // Setup database first
        setup_db();

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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_web_stream_invalid_path() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
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
        if should_skip_db_tests() {
            return;
        }
        setup_db();

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
        if should_skip_db_tests() {
            return;
        }
        setup_db();

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
    fn test_script_properties_validation() {
        if should_skip_db_tests() {
            return;
        }
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_default_content_types() {
        use crate::security::UserContext;
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
        };
        let result = execute_script_for_request_secure(params);

        assert!(result.is_ok(), "Request should execute successfully");
        let response = result.unwrap();
        assert_eq!(response.content_type, Some("application/json".to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_convert_markdown_to_html_simple() {
        use crate::security::UserContext;
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_convert_markdown_to_html_code_block() {
        use crate::security::UserContext;
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_convert_markdown_to_html_list() {
        use crate::security::UserContext;
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_convert_markdown_to_html_table() {
        use crate::security::UserContext;
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_convert_markdown_to_html_empty_input() {
        use crate::security::UserContext;
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_convert_markdown_to_html_complex() {
        use crate::security::UserContext;
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();

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
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            route_params: None,
            auth_context: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_response_builders() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_content = r#"
            function testHandler(context) {
                // Test ResponseBuilder.json
                const jsonResponse = ResponseBuilder.json({ message: "Hello World", code: 200 });
                if (jsonResponse.status !== 200 || jsonResponse.contentType !== "application/json") {
                    throw new Error("JSON response failed");
                }

                // Test ResponseBuilder.text
                const textResponse = ResponseBuilder.text("Plain text response", 201);
                if (textResponse.status !== 201 || textResponse.contentType !== "text/plain; charset=UTF-8") {
                    throw new Error("Text response failed");
                }

                // Test ResponseBuilder.html
                const htmlResponse = ResponseBuilder.html("<h1>Hello</h1>", 200);
                if (htmlResponse.status !== 200 || htmlResponse.contentType !== "text/html; charset=UTF-8") {
                    throw new Error("HTML response failed");
                }

                // Test ResponseBuilder.error
                const errorResponse = ResponseBuilder.error(400, "Bad Request");
                if (errorResponse.status !== 400 || !errorResponse.body.includes("Bad Request")) {
                    throw new Error("Error response failed");
                }

                // Test ResponseBuilder.noContent
                const noContentResponse = ResponseBuilder.noContent();
                if (noContentResponse.status !== 204 || noContentResponse.body !== "") {
                    throw new Error("No content response failed");
                }

                // Test ResponseBuilder.redirect
                const redirectResponse = ResponseBuilder.redirect("https://example.com", 302);
                if (redirectResponse.status !== 302 || !redirectResponse.headers.Location) {
                    throw new Error("Redirect response failed");
                }

                return ResponseBuilder.json({ success: true });
            }
        "#;

        let _ = repository::upsert_script("response-builder-test", script_content);

        let params = RequestExecutionParams {
            script_uri: "response-builder-test".to_string(),
            handler_name: "testHandler".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None,
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            auth_context: None,
            uploaded_files: None,
            route_params: None,
            request_id: None,
            route_pattern: None,
        };

        let result = execute_script_for_request_secure(params);
        if let Err(ref e) = result {
            eprintln!("Test error: {}", e);
        }
        assert!(result.is_ok(), "Response builder test should succeed");

        let response = result.unwrap();
        assert_eq!(response.status, 200);
        let body_str = String::from_utf8_lossy(&response.body);
        assert!(body_str.contains("success"));
        assert!(body_str.contains("true"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_query_object_guarantees() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_content = r#"
            function testHandler(context) {
                // Test that context.request.query is always available
                if (!context.request || !context.request.query) {
                    throw new Error("context.request.query should always be available");
                }

                // Test that it's an object (even if empty)
                if (typeof context.request.query !== 'object') {
                    throw new Error("context.request.query should be an object");
                }

                // Test that we can safely access properties
                const param1 = context.request.query.param1 || "default";
                const param2 = context.request.query.param2 || "default2";

                return ResponseBuilder.json({
                    param1: param1,
                    param2: param2,
                    queryType: typeof context.request.query
                });
            }
        "#;

        let _ = repository::upsert_script("query-guarantees-test", script_content);

        let params = RequestExecutionParams {
            script_uri: "query-guarantees-test".to_string(),
            handler_name: "testHandler".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: Some(HashMap::from([
                ("param1".to_string(), "value1".to_string()),
                ("param2".to_string(), "value2".to_string()),
            ])),
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            auth_context: None,
            uploaded_files: None,
            route_params: None,
            request_id: None,
            route_pattern: None,
        };

        let result = execute_script_for_request_secure(params);
        assert!(result.is_ok(), "Query guarantees test should succeed");

        let response = result.unwrap();
        assert_eq!(response.status, 200);
        let body_str = String::from_utf8_lossy(&response.body);
        assert!(body_str.contains("value1"));
        assert!(body_str.contains("value2"));
        assert!(body_str.contains("object"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_query_object_guarantees_empty_params() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_content = r#"
            function testHandler(context) {
                // Test that context.request.query is available even with no query params
                if (!context.request || !context.request.query) {
                    throw new Error("context.request.query should always be available");
                }

                // Should be an empty object
                const keys = Object.keys(context.request.query);
                if (keys.length !== 0) {
                    throw new Error("Expected empty query object, got: " + keys.length + " keys");
                }

                return ResponseBuilder.json({ queryEmpty: true });
            }
        "#;

        let _ = repository::upsert_script("query-empty-test", script_content);

        let params = RequestExecutionParams {
            script_uri: "query-empty-test".to_string(),
            handler_name: "testHandler".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: None, // No query params
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            auth_context: None,
            uploaded_files: None,
            route_params: None,
            request_id: None,
            route_pattern: None,
        };

        let result = execute_script_for_request_secure(params);
        assert!(result.is_ok(), "Empty query test should succeed");

        let response = result.unwrap();
        assert_eq!(response.status, 200);
        let body_str = String::from_utf8_lossy(&response.body);
        assert!(body_str.contains("queryEmpty"));
        assert!(body_str.contains("true"));
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn test_automatic_path_parameters() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_content = r#"
            function testHandler(context) {
                // Test that context.request.params contains extracted path parameters
                if (!context.request || !context.request.params) {
                    throw new Error("context.request.params should be available");
                }

                // Check that path parameters are correctly extracted
                const userId = context.request.params.userId;
                const postId = context.request.params.postId;

                if (!userId || !postId) {
                    throw new Error("Path parameters not extracted correctly");
                }

                return ResponseBuilder.json({
                    userId: userId,
                    postId: postId,
                    paramsType: typeof context.request.params
                });
            }
        "#;

        let _ = repository::upsert_script("path-params-test", script_content);

        let params = RequestExecutionParams {
            script_uri: "path-params-test".to_string(),
            handler_name: "testHandler".to_string(),
            path: "/api/users/123/posts/456".to_string(),
            method: "GET".to_string(),
            query_params: None,
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            auth_context: None,
            uploaded_files: None,
            route_params: Some(HashMap::from([
                ("userId".to_string(), "123".to_string()),
                ("postId".to_string(), "456".to_string()),
            ])),
            request_id: None,
            route_pattern: None,
        };

        let result = execute_script_for_request_secure(params);
        if let Err(e) = &result {
            eprintln!("Test failed with error: {:?}", e);
        }
        assert!(
            result.is_ok(),
            "Path parameters test should succeed: {:?}",
            result.as_ref().err()
        );

        let response = result.unwrap();
        assert_eq!(response.status, 200);
        let body_str = String::from_utf8_lossy(&response.body);
        assert!(body_str.contains("123"));
        assert!(body_str.contains("456"));
        assert!(body_str.contains("object"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_validation_helpers() {
        if should_skip_db_tests() {
            return;
        }
        let rt = get_runtime();
        let _guard = rt.enter();
        setup_db();
        let script_content = r#"
            function testHandler(context) {
                try {
                    // Test requireQueryParam with existing param
                    const existingParam = requireQueryParam("existing");
                    if (existingParam !== "value1") {
                        throw new Error("requireQueryParam failed for existing param");
                    }

                    // Test requireQueryParam with missing param (should throw)
                    try {
                        requireQueryParam("missing");
                        throw new Error("requireQueryParam should have thrown for missing param");
                    } catch (e) {
                        if (!e.message.includes("missing")) {
                            throw new Error("Wrong error message for missing param: " + e.message);
                        }
                    }

                    // Test requirePathParam
                    const userId = requirePathParam("userId");
                    if (userId !== "123") {
                        throw new Error("requirePathParam failed");
                    }

                    // Test validateString
                    const validStr = validateString("hello", 2, 10);
                    if (validStr !== "hello") {
                        throw new Error("validateString failed");
                    }

                    // Test validateString with invalid length
                    try {
                        validateString("a", 2, 10);
                        throw new Error("validateString should have thrown for short string");
                    } catch (e) {
                        // Expected
                    }

                    // Test validateNumber
                    const validNum = validateNumber("42", 0, 100);
                    if (validNum !== 42) {
                        throw new Error("validateNumber failed");
                    }

                    // Test optionalQueryParam
                    const optionalExisting = optionalQueryParam("existing", "default");
                    const optionalMissing = optionalQueryParam("missing", "default");

                    return ResponseBuilder.json({
                        success: true,
                        existingParam: existingParam,
                        userId: userId,
                        validStr: validStr,
                        validNum: validNum,
                        optionalExisting: optionalExisting,
                        optionalMissing: optionalMissing
                    });
                } catch (e) {
                    return ResponseBuilder.error(400, e.message);
                }
            }
        "#;

        let _ = repository::upsert_script("validation-helpers-test", script_content);

        let params = RequestExecutionParams {
            script_uri: "validation-helpers-test".to_string(),
            handler_name: "testHandler".to_string(),
            path: "/test".to_string(),
            method: "GET".to_string(),
            query_params: Some(HashMap::from([(
                "existing".to_string(),
                "value1".to_string(),
            )])),
            url: None,
            form_data: None,
            raw_body: None,
            headers: HashMap::new(),
            user_context: UserContext::admin("test".to_string()),
            auth_context: None,
            uploaded_files: None,
            route_params: Some(HashMap::from([("userId".to_string(), "123".to_string())])),
            request_id: None,
            route_pattern: None,
        };

        let result = execute_script_for_request_secure(params);
        assert!(result.is_ok(), "Validation helpers test should succeed");

        let response = result.unwrap();
        assert_eq!(response.status, 200);
        let body_str = String::from_utf8_lossy(&response.body);
        assert!(body_str.contains("success"));
        assert!(body_str.contains("value1"));
        assert!(body_str.contains("123"));
    }
}
