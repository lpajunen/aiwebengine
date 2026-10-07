//! Running a script to collect what it registers: startup, `init()` and the dry run
//! `check_script` makes.

use super::*;
use crate::repository;
use crate::security::secure_globals::{
    CollectedRegistration, GlobalSecurityConfig, Principal, RegistrationKind, RegistrationSink,
};
use rquickjs::{Context, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Validates a script before execution
pub(super) fn validate_script(content: &str, limits: &ExecutionLimits) -> Result<(), String> {
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
    pub(super) fn failed(error_message: String, execution_time_ms: u64) -> Self {
        Self {
            registrations: HashMap::new(),
            success: false,
            error: Some(error_message),
            execution_time_ms,
        }
    }

    /// Create a successful execution result
    pub(super) fn success(
        registrations: repository::RouteRegistrations,
        execution_time_ms: u64,
    ) -> Self {
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
    principal: Principal,
) -> ScriptExecutionResult {
    let start_time = Instant::now();

    // Validate script using configured limits
    let limits = crate::script_limits::for_script(uri);
    if let Err(e) = validate_script(content, &limits) {
        return ScriptExecutionResult::failed(e, start_time.elapsed().as_millis() as u64);
    }

    // Running a script does not store it; storing a script is what the write
    // paths are for. Writing back here would rewrite every script on every
    // boot.

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
        Ok((rt, _budget)) => match Context::full(&rt) {
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
                        // Bringing the script up is one invocation, so its
                        // output groups under one id like any other.
                        ..GlobalSecurityConfig::new(
                            principal,
                            HandlerInvocationKind::Init.log_context(
                                &uri_owned,
                                crate::middleware::generate_request_id(),
                                None,
                            ),
                        )
                    };

                    // Create the register function that captures registrations
                    let regs_clone = Rc::clone(&registrations);
                    let uri_clone = uri_owned.clone();
                    let register_impl = std::rc::Rc::new(
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
                        security_config,
                        Some(register_impl),
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
    let outcome = run_registration_pass(
        script_uri,
        script_content,
        context,
        timeout_ms,
        None,
        &crate::source_view::SourceView::Live,
    );

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
    /// Where the program's imports are resolved from.
    ///
    /// `script_content` alone only ever answered for the root. A change that
    /// spans modules, or a revision being checked without being deployed, is
    /// described by this — and a pass that took the root from one version and
    /// its imports from another would be checking a program that exists
    /// nowhere.
    pub view: crate::source_view::SourceView,
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
        &params.view,
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
/// `/engine/check_script` exists for and which costs nothing here because the context
/// that would answer the question is still alive.
pub(super) fn run_registration_pass(
    script_uri: &str,
    script_content: &str,
    context: crate::script_init::InitContext,
    timeout_ms: u64,
    dry_run_sink: Option<RegistrationSink>,
    view: &crate::source_view::SourceView,
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
    let executable_code = match transpile_if_needed_in(script_uri, script_content, view) {
        Ok(code) => code,
        Err(e) => fail_early!(format!("Transpilation failed: {}", e)),
    };

    let (rt, _budget) = match create_sandboxed_runtime(&limits) {
        Ok(armed) => armed,
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
            // `init()` is the registration phase, and the script's top-level
            // program runs under it too. A check reports diagnostics, not
            // output; console writes go to the script's log as they would on a
            // deploy.
            let config = GlobalSecurityConfig {
                registration_phase: true,
                dry_run_sink: dry_run_sink.clone(),
                ..GlobalSecurityConfig::new(
                    Principal::Engine("script-init"),
                    HandlerInvocationKind::Init.log_context(
                        script_uri,
                        invocation_id.clone(),
                        None,
                    ),
                )
            };

            // Create the register function that captures registrations
            let regs_clone = Rc::clone(&registrations);
            let uri_clone = uri_owned.clone();
            // Routes are collected through this closure rather than through the
            // sink, so on a dry run they have to be mirrored into it — the
            // delegate check downstream reads the sink, and a route handler is
            // the delegate most worth checking.
            let route_sink = dry_run_sink.clone();
            let register_impl = std::rc::Rc::new(
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

            let setup_result =
                setup_secure_global_functions(&ctx, script_uri, config, Some(register_impl));

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
pub(super) fn record_missing_handlers(
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
    for registration in collected.iter().filter(|r| r.refusal.is_none()) {
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
