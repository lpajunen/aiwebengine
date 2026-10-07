//! Calling a handler — an HTTP route, a job, a task, an MCP tool or prompt, an
//! authorization function — through one `HandlerExecution`.

use super::*;
use crate::repository;
use crate::scheduler::ScheduledInvocation;
use crate::security::UserContext;
use crate::security::secure_globals::{GlobalSecurityConfig, Principal};
use rquickjs::{Context, Runtime, Value};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tracing::info;

/// Whether per-request phase profiling is enabled (env `AIWEBENGINE_PROFILE_REQUESTS=1`).
///
/// Read once and cached. When enabled, [`execute_script_for_request_secure`] emits a
/// single structured log line per request breaking down where wall-clock time is spent.
pub(super) fn request_profiling_enabled() -> bool {
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

/// Which execution budget a handler runs under.
#[derive(Debug, Clone, Copy)]
pub(super) enum Budget {
    /// `javascript.execution_timeout_ms`, or the script's own override.
    Request,
    /// `javascript.job_timeout_ms`, or the script's own override: background
    /// work, which nobody is waiting on.
    Job,
}

/// How long each step of [`HandlerExecution::prepare`] took, for the request
/// profiler.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct PreparePhases {
    pub(super) fetch: Duration,
    pub(super) transpile: Duration,
    pub(super) runtime: Duration,
    pub(super) globals: Duration,
    pub(super) eval: Duration,
}

/// A script's program evaluated in a fresh sandbox, ready for one handler call.
///
/// Every way a handler is reached — an HTTP request, a scheduled job, a task, an
/// MCP tool or prompt, a stream or asset authorization — is the same sequence:
/// fetch and bundle the program, create a runtime under a budget, install the
/// globals for a [`Principal`], evaluate the program, then call one global
/// function and settle what it returned. They differ only in who the principal
/// is, which budget applies, what the function is handed and what is read
/// back, so those are the parameters and the sequence is written once.
pub(super) struct HandlerExecution {
    // Declared before `rt`, so it is dropped first: a context must not
    // outlive its runtime.
    pub(super) ctx: Context,
    pub(super) rt: Runtime,
    pub(super) _budget: crate::database::HostCallBudget,
    pub(super) script_uri: String,
    pub(super) phases: PreparePhases,
}

impl HandlerExecution {
    /// Builds the sandbox and evaluates the script's program in it.
    pub(super) fn prepare(
        script_uri: &str,
        budget: Budget,
        config: GlobalSecurityConfig,
    ) -> Result<Self, String> {
        let mut phases = PreparePhases::default();

        // Fetch and bundle the program *before* creating the runtime: the
        // interrupt deadline armed by `create_sandboxed_runtime` starts at
        // runtime creation, so preparing the program afterwards charges the
        // bundle against the execution budget. On a cold cache — the state
        // every deploy leaves behind — an asset-backed script fetches and
        // transpiles every imported module here, which was enough to exhaust
        // the budget before the handler ran.
        let phase = Instant::now();
        let source = repository::fetch_script(script_uri)
            .ok_or_else(|| format!("no script for uri {}", script_uri))?;
        phases.fetch = phase.elapsed();

        // Transpile if needed (TypeScript/JSX/TSX) — cached by (uri, source hash).
        let phase = Instant::now();
        let executable_code = transpile_if_needed(script_uri, &source)?;
        phases.transpile = phase.elapsed();

        let phase = Instant::now();
        let limits = match budget {
            Budget::Request => crate::script_limits::for_script(script_uri),
            Budget::Job => crate::script_limits::for_script_job(script_uri),
        };
        let (rt, host_budget) = create_sandboxed_runtime(&limits)?;
        let ctx = Context::full(&rt).map_err(|e| format!("context create: {}", e))?;
        phases.runtime = phase.elapsed();

        let phase = Instant::now();
        ctx.with(|ctx| setup_secure_global_functions(&ctx, script_uri, config, None))
            .map_err(|e| format!("install globals: {}", e))?;
        phases.globals = phase.elapsed();

        // Bytecode is cached, but the top-level program still executes on every
        // invocation.
        let phase = Instant::now();
        ctx.with(|ctx| {
            crate::bytecode::eval_program(&ctx, script_uri, &executable_code)
                .map_err(|e| format!("script eval: {}", extract_error_details(&ctx, &e)))
        })?;
        phases.eval = phase.elapsed();

        Ok(Self {
            ctx,
            rt,
            _budget: host_budget,
            script_uri: script_uri.to_string(),
            phases,
        })
    }

    /// Calls into the program and settles the result, committing the
    /// invocation's transaction when it succeeds and rolling it back when it
    /// does not. `what` names the call in a never-settles message.
    pub(super) fn invoke<T>(
        &self,
        what: &str,
        call: impl for<'js> FnOnce(&rquickjs::Ctx<'js>) -> Result<rquickjs::Promise<'js>, String>,
        finish: impl for<'js> FnOnce(&rquickjs::Ctx<'js>, Value<'js>) -> Result<T, String>,
    ) -> Result<T, String> {
        call_and_settle(
            &self.rt,
            &self.ctx,
            &self.script_uri,
            what,
            TransactionHandling::Auto,
            call,
            finish,
        )
    }
}

/// Makes `handler_context` the `context` global, as well as the handler's
/// argument, so `personalStorage` and the other APIs that read the caller from
/// it can find it.
pub(super) fn install_handler_context<'js>(
    ctx: &rquickjs::Ctx<'js>,
    handler_context: &rquickjs::Object<'js>,
) -> Result<(), String> {
    ctx.globals()
        .set("context", handler_context.clone())
        .map_err(|e| format!("set context global: {}", e))
}

/// Calls the global function `name` with `argument`, as a promise.
///
/// A function that throws before returning never reaches the microtask queue,
/// so its transaction is rolled back here. One that rejects after an `await`
/// settles during the drain, and [`call_and_settle`] finishes it.
pub(super) fn call_global_function<'js>(
    ctx: &rquickjs::Ctx<'js>,
    name: &str,
    argument: Value<'js>,
) -> Result<rquickjs::Promise<'js>, String> {
    let function: Value = ctx
        .globals()
        .get(name)
        .map_err(|e| format!("no handler {}: {}", name, e))?;
    let function = function
        .as_function()
        .ok_or_else(|| format!("handler '{}' not found, or not a function", name))?;

    let result: Value = function.call((argument,)).map_err(|e| {
        let details = extract_error_details(ctx, &e);
        if crate::database::get_current_transaction_active() {
            let _ = crate::database::Database::rollback_transaction();
        }
        format!("call handler: {}", details)
    })?;

    promise_resolve(ctx, result)
}

/// Runs the handler a route names for one HTTP request, as whoever made it.
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
        &script_uri_owned,
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

    let execution = HandlerExecution::prepare(
        &script_uri_owned,
        Budget::Request,
        GlobalSecurityConfig {
            client_ip: crate::security::client_ip::from_header_snapshot(&params.headers),
            ..GlobalSecurityConfig::new(Principal::Caller(params.user_context.clone()), log_context)
        },
    )?;
    let PreparePhases {
        fetch: t_fetch,
        transpile: t_transpile,
        runtime: t_runtime,
        globals: t_globals,
        eval: t_eval,
    } = execution.phases;

    let phase = Instant::now();
    let response_exec = execution.invoke(
        &format!("Handler '{}'", params.handler_name),
        |ctx| call_handler(ctx, &params, &auth_context),
        |_ctx, value| build_http_response(value),
    );

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
        if let Some((ctor, native, resp)) = GLOBALS_BREAKDOWN.with(|b| b.take()) {
            info!(
                target: "request_profile",
                ctor_us = ctor.as_micros() as u64,
                native_fns_us = native.as_micros() as u64,
                response_builders_us = resp.as_micros() as u64,
                "globals install breakdown"
            );
        }
    }

    Ok(response_result)
}

/// Invokes the named handler and hands back its result as a native promise.
///
/// Runs inside an active `ctx.with`. The promise is not read here: the
/// microtask queue that would settle it can only be drained with no context
/// guard on the stack, so that is the caller's job.
pub(super) fn call_handler<'js>(
    ctx: &rquickjs::Ctx<'js>,
    params: &RequestExecutionParams,
    auth_context: &Option<crate::auth::JsAuthContext>,
) -> Result<rquickjs::Promise<'js>, String> {
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
    install_handler_context(ctx, &handler_context)?;

    call_global_function(ctx, &params.handler_name, handler_context.into_value())
}

/// Maps a settled handler result into an [`JsHttpResponse`].
pub(super) fn build_http_response(result: Value<'_>) -> Result<JsHttpResponse, String> {
    if let Some(response_obj) = result.as_object() {
        let status: i32 = response_obj
            .get("status")
            .map_err(|e| format!("missing status: {}", e))?;

        // Try to get bodyBase64 first (for binary data), otherwise fall back to body (for text)
        let mut serialized_as_json = false;
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

            // An array or a plain object is data, and the only text form of
            // data that a handler could have meant is JSON: `toString` would
            // give "[object Object]" or the items joined by commas. Before
            // `database` answered with values a query's rows were a `String`
            // object, so `body: database.query(t)` worked; this keeps it
            // working now that they are an array.
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
                    let text = to_string_fn
                        .call::<_, String>((rquickjs::function::This(obj.clone()),))
                        .map_err(|e| format!("Failed to call toString: {}", e))?;
                    if body_value.is_array() || text == "[object Object]" {
                        serialized_as_json = true;
                        body_value
                            .ctx()
                            .json_stringify(body_value.clone())
                            .map_err(|e| format!("Failed to serialise body as JSON: {}", e))?
                            .and_then(|s| s.to_string().ok())
                            .unwrap_or_else(|| "null".to_string())
                    } else {
                        text
                    }
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
            } else if serialized_as_json {
                Some("application/json".to_string())
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
        &script_uri_owned,
        invocation.invocation_id.clone(),
        Some(invocation.key.clone()),
    );

    // A job's budget rather than a request's. The scheduler renews its claim
    // for as long as the run lasts, so the two no longer have to agree on a
    // number — but this is still what bounds the run. A job acts for nobody.
    let execution = HandlerExecution::prepare(
        script_uri,
        Budget::Job,
        GlobalSecurityConfig::new(Principal::Engine("scheduler"), log_context),
    )?;

    execution.invoke(
        &format!("Scheduled handler '{}'", handler_name),
        |ctx| {
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
            install_handler_context(ctx, &handler_context)?;

            call_global_function(ctx, handler_name, handler_context.into_value())
        },
        |_ctx, _value| Ok(()),
    )
}

/// Run one attempt of a queued task ([`crate::tasks`]).
///
/// Deliberately the same execution as [`execute_scheduled_handler`]: the same
/// job budget, the same script context, the same transaction handling. The two
/// differ in what the handler is given — a task carries a payload and its
/// attempt count, where a job carries its schedule — and in what the caller
/// does with the outcome, which for a task is a retry with backoff.
///
/// A task acts for nobody unless a person delegated it, and then for that
/// person, narrowed to what they granted ([`crate::delegation`]).
pub fn execute_task_handler(
    invocation: &crate::tasks::TaskInvocation,
    delegated: Option<&crate::delegation::Delegated>,
) -> Result<Option<serde_json::Value>, String> {
    let script_uri = invocation.script_uri.as_str();
    let handler_name = invocation.handler_name.as_str();
    let script_uri_owned = script_uri.to_string();

    let log_context = HandlerInvocationKind::Scheduled.log_context(
        &script_uri_owned,
        invocation.invocation_id.clone(),
        Some(handler_name.to_string()),
    );

    // A delegated task acts for the person who granted it, narrowed to what
    // they granted, and reaches the management tools through `Scope::Author`
    // and `Scope::Administer` — the person consented on a page that named what
    // they were consenting to. An undelegated one acts for nobody.
    //
    // The principal is also what `fetch` resolves `{{secret:...}}` against —
    // `user_secrets` for this id, then the script's — so a delegated task
    // reaches the person's own key here and nowhere else.
    let principal = match delegated {
        Some(delegated) => Principal::Delegated {
            user: delegated.user_context.clone(),
            scopes: delegated.grant.scopes.clone(),
        },
        None => Principal::Engine("tasks"),
    };

    // A queued task is background work, so it takes the job budget.
    let execution = HandlerExecution::prepare(
        script_uri,
        Budget::Job,
        GlobalSecurityConfig::new(principal, log_context),
    )?;

    execution.invoke(
        &format!("Task handler '{}'", handler_name),
        |ctx| {
            // `attempt` counts from one, because a handler reads it to say
            // "attempt 2 of 5" and nobody calls the first one attempt zero.
            let task_meta = serde_json::json!({
                "taskId": invocation.task_id.to_string(),
                "handler": handler_name,
                "attempt": invocation.attempts + 1,
                "maxAttempts": invocation.max_attempts,
                "payload": invocation.payload,
            });

            // `personalStorage` reads `context.request.auth.userId`, so a
            // delegated task has to present one. It is the identity
            // `delegation::resolve` just re-derived, never something carried in
            // the task's row — and `isAdmin`/`isEditor` are false whatever the
            // person holds, because the tier is capped at what an ordinary
            // request has.
            let mut builder = JsHandlerContextBuilder::new(HandlerInvocationKind::Scheduled)
                .with_script_metadata(script_uri, handler_name)
                .with_metadata_value("task", task_meta)
                .with_invocation_id(invocation.invocation_id.clone());

            if let Some(delegated) = delegated {
                builder = builder
                    .with_request(JsRequestContext::default())
                    .with_auth_context(crate::auth::JsAuthContext::authenticated(
                        delegated.user_id().to_string(),
                        delegated.email.clone(),
                        delegated.name.clone(),
                        "delegation".to_string(),
                        false,
                        false,
                    ));
            }

            let handler_context = builder
                .build(ctx)
                .map_err(|e| format!("build context: {}", e))?;
            install_handler_context(ctx, &handler_context)?;

            call_global_function(ctx, handler_name, handler_context.into_value())
        },
        // What the handler returned, kept rather than discarded.
        //
        // For a queued task the effects are the output and the log says what
        // it did. An MCP task is the case where somebody is waiting for a
        // *value*: `tasks/get` has to answer with what the tool
        // call would have returned synchronously, and the handler's return is
        // the only place that can come from. A conversion failure is not a task
        // failure, so it lands as `None` rather than poisoning the run.
        |ctx, value| {
            // Through `JSON.stringify` rather than a direct conversion, which
            // is how every other boundary here reads a handler's value: it is
            // the same serialization the handler would have got had it returned
            // to a synchronous call, so a task's result and a direct result
            // cannot differ in shape. `undefined` stringifies to nothing, which
            // is `None` — a handler that returned nothing has no result, and
            // storing `null` would claim it answered.
            let json: rquickjs::Object = match ctx.globals().get("JSON") {
                Ok(json) => json,
                Err(_) => return Ok(None),
            };
            let stringify: rquickjs::Function = match json.get("stringify") {
                Ok(stringify) => stringify,
                Err(_) => return Ok(None),
            };
            let text: Option<String> = stringify.call((value,)).ok();
            Ok(text.and_then(|text| serde_json::from_str(&text).ok()))
        },
    )
}

/// Execute an MCP prompt handler, which is called with its arguments rather
/// than with a handler context, as whoever asked for the prompt.
pub fn execute_mcp_prompt_handler(
    script_uri: &str,
    handler_function: &str,
    arguments: serde_json::Value,
    user_context: UserContext,
    exchange: crate::mcp_elicitation::Exchange,
) -> Result<crate::mcp::PromptOutcome, String> {
    // A prompt is answered for whoever asked for it.
    let log_context = HandlerInvocationKind::McpPrompt.log_context(
        script_uri,
        crate::middleware::generate_request_id(),
        Some(handler_function.to_string()),
    );
    let execution = HandlerExecution::prepare(
        script_uri,
        Budget::Request,
        GlobalSecurityConfig::new(Principal::Caller(user_context), log_context),
    )
    .map_err(|e| format!("Prompt handler execution failed: {}", e))?;

    let guard = crate::mcp_elicitation::ExchangeGuard::install(exchange);

    let result_exec = execution.invoke(
        &format!("MCP prompt handler '{}'", handler_function),
        |ctx| {
            let arguments: Value = ctx
                .json_parse(arguments.to_string())
                .map_err(|e| format!("parse prompt arguments: {}", e))?;
            call_global_function(ctx, handler_function, arguments)
        },
        |ctx, result| {
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

    // Read before judging the result, for the reason the tool path does: a
    // handler that asks ends by throwing, and the outcome is what it recorded.
    let exchange = guard.finish();
    // Before the questions, because a handler that did both has already queued
    // its work: answering `input_required` would leave the queue holding work
    // for a call the client is about to retry from the top.
    if let Some(handed) = exchange.handed_off() {
        return Ok(crate::mcp::Outcome::Handed(handed.clone()));
    }
    if let Some(asked) = exchange.into_asked() {
        return Ok(crate::mcp::Outcome::InputRequired(asked));
    }

    result_exec
        .map(crate::mcp::Outcome::Complete)
        .map_err(|e| format!("Prompt handler execution failed: {}", e))
}

/// Execute an MCP tool handler function.
///
/// Takes the exchange rather than building one, because whether this caller can
/// be asked anything is a property of the request that arrived — `lib.rs` reads
/// the client's declared capabilities — and not of the script. Hands back what
/// the handler asked for alongside its result, since a handler that asks
/// produces no result at all.
pub fn execute_mcp_tool_handler(
    script_uri: &str,
    handler_function: &str,
    tool_name: &str,
    arguments: serde_json::Value,
    auth_context: Option<crate::auth::JsAuthContext>,
    principal: Principal,
    exchange: crate::mcp_elicitation::Exchange,
) -> Result<crate::mcp::ToolOutcome, String> {
    let invocation_id = crate::middleware::generate_request_id();
    let log_context = HandlerInvocationKind::McpTool.log_context(
        script_uri,
        invocation_id.clone(),
        Some(tool_name.to_string()),
    );

    // Over `/mcp` the validated caller from the auth middleware: a tool call
    // is a request like any other, so it reaches `engine` as that caller.
    // From `tools.call`, whoever the calling execution acted for, delegated
    // scopes included.
    let execution = HandlerExecution::prepare(
        script_uri,
        Budget::Request,
        GlobalSecurityConfig::new(principal, log_context),
    )
    .map_err(|e| format!("JavaScript execution error: {}", e))?;

    let guard = crate::mcp_elicitation::ExchangeGuard::install(exchange);

    let result_exec = execution.invoke(
        &format!("MCP tool handler '{}'", handler_function),
        |ctx| {
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
                .with_script_metadata(script_uri, handler_function)
                .with_request(request_context)
                .with_invocation_id(invocation_id.clone())
                .with_args(arguments)
                .with_metadata_value("mcp", serde_json::json!({ "toolName": tool_name }));

            if let Some(auth_context) = auth_context {
                context_builder = context_builder.with_auth_context(auth_context);
            }

            let handler_context = context_builder
                .build(ctx)
                .map_err(|e| format!("build context: {}", e))?;
            install_handler_context(ctx, &handler_context)?;

            call_global_function(ctx, handler_function, handler_context.into_value())
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

    // What the handler asked for is read before the result is judged, because a
    // handler that asks ends by throwing: the exception is only how the
    // execution unwinds, and the outcome is decided by what was recorded. That
    // ordering is also what stops a script catching its own `McpInputRequired`
    // and returning a value as though the question had been answered.
    let exchange = guard.finish();
    // Before the questions, because a handler that did both has already queued
    // its work: answering `input_required` would leave the queue holding work
    // for a call the client is about to retry from the top.
    if let Some(handed) = exchange.handed_off() {
        return Ok(crate::mcp::Outcome::Handed(handed.clone()));
    }
    if let Some(asked) = exchange.into_asked() {
        return Ok(crate::mcp::ToolOutcome::InputRequired(asked));
    }

    let result_string = result_exec.map_err(|e| format!("JavaScript execution error: {}", e))?;
    Ok(crate::mcp::ToolOutcome::Complete(result_string))
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
) -> Result<crate::resource_access::AccessDecision, String> {
    execute_authorization_function(
        script_uri,
        function_name,
        HandlerInvocationKind::StreamCustomization,
        path,
        query_params,
        auth_context,
    )
}

/// Run the function a script named to decide who may read something the
/// engine serves without running a handler.
///
/// One implementation for both surfaces — a stream's connection and an asset
/// route — because they are the same question asked twice, and two copies
/// would eventually parse `{ deny: … }` two different ways. `kind` is what
/// the script's log lines are attributed to.
pub fn execute_authorization_function(
    script_uri: &str,
    function_name: &str,
    kind: HandlerInvocationKind,
    path: &str,
    query_params: &std::collections::HashMap<String, String>,
    auth_context: Option<crate::auth::JsAuthContext>,
) -> Result<crate::resource_access::AccessDecision, String> {
    let invocation_id = crate::middleware::generate_request_id();
    let log_context = kind.log_context(script_uri, invocation_id.clone(), Some(path.to_string()));

    // Asked on behalf of whoever is connecting or reading, anonymous included.
    let execution = HandlerExecution::prepare(
        script_uri,
        Budget::Request,
        GlobalSecurityConfig::new(
            Principal::Caller(caller_context(auth_context.as_ref())),
            log_context,
        ),
    )
    .map_err(|e| format!("Customization function execution error: {}", e))?;

    let result_exec = execution.invoke(
        &format!("{} '{}'", kind.as_str(), function_name),
        |ctx| {
            let request_context = JsRequestContext {
                path: Some(path.to_string()),
                url: None,
                method: Some("GET".to_string()),
                headers: HashMap::new(),
                query_params: query_params.clone(),
                form_data: HashMap::new(),
                body: None,
                route_params: HashMap::new(),
                uploaded_files: Vec::new(),
            };

            let mut context_builder = JsHandlerContextBuilder::new(kind)
                .with_script_metadata(script_uri, function_name)
                .with_request(request_context)
                .with_invocation_id(invocation_id.clone())
                .with_metadata_value(
                    match kind {
                        HandlerInvocationKind::AssetAuthorization => "asset",
                        _ => "stream",
                    },
                    serde_json::json!({ "path": path }),
                );

            if !query_params.is_empty() {
                let args_json = JsonValue::Object(
                    query_params
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
            install_handler_context(ctx, &handler_context)?;

            call_global_function(ctx, function_name, handler_context.into_value())
        },
        |_ctx, result_value| {
            use crate::resource_access::{AccessDecision, deny_reason, deny_status};

            let Some(result_obj) = result_value.as_object() else {
                return Err("Expected object result".to_string());
            };

            // A refusal is named rather than thrown. Throwing was the only way
            // to deny, and it landed as a 500 with the message in the body —
            // which tells the client to retry something that will be refused
            // again, and leaks whatever the script said. A `deny` key is the
            // one shape both this and an asset route answer in.
            if let Ok(deny) = result_obj.get::<_, rquickjs::Value>("deny")
                && !deny.is_undefined()
                && !deny.is_null()
            {
                // `{ deny: true }` is the short spelling of "refuse"; a number
                // is the status to refuse with.
                let requested = match deny.as_bool() {
                    Some(true) => None,
                    Some(false) => {
                        return Err(
                            "deny: false is not a decision — return the filter criteria to allow"
                                .to_string(),
                        );
                    }
                    None => match deny.as_number() {
                        Some(status) => Some(status as i64),
                        None => {
                            return Err(
                                "deny must be true or an HTTP status number, e.g. { deny: 403 }"
                                    .to_string(),
                            );
                        }
                    },
                };
                let reason = result_obj
                    .get::<_, rquickjs::Value>("reason")
                    .ok()
                    .and_then(|value| value.as_string().and_then(|s| s.to_string().ok()));
                return Ok(AccessDecision::Deny {
                    status: deny_status(requested),
                    reason: deny_reason(reason),
                });
            }

            let mut filter_criteria = std::collections::HashMap::new();
            for key_str in result_obj.keys::<String>().flatten() {
                if let Ok(value) = result_obj.get::<_, rquickjs::Value>(&key_str) {
                    if let Some(value_str) = value.as_string().and_then(|s| s.to_string().ok()) {
                        filter_criteria.insert(key_str.clone(), value_str);
                    } else {
                        return Err("Filter values must be strings".to_string());
                    }
                }
            }

            Ok(AccessDecision::Allow(filter_criteria))
        },
    );

    result_exec.map_err(|e| format!("Customization function execution error: {}", e))
}
