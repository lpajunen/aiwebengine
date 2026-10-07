//! Evaluating a snippet against a script's program (`eval_script`, `sandbox.run`).

use super::*;
use crate::module_loader;
use crate::repository;
use crate::security::UserContext;
use crate::security::secure_globals::{ConsoleLine, ConsoleSink, GlobalSecurityConfig, Principal};
use rquickjs::{Context, Function, Runtime};
use serde_json::Value as JsonValue;
use std::time::Instant;

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
    /// What the snippet is handed as `context.args`.
    ///
    /// A snippet has no parameter list — it is a program, not a function — so
    /// an argument has to arrive somewhere a program can read. `context.args`
    /// is where a handler's arguments already arrive, so a script reading it
    /// is reading something it knows.
    pub input: Option<JsonValue>,
    /// Who the snippet runs as, as JavaScript sees it.
    ///
    /// `None` builds the anonymous context an `/engine/eval_script` gets. A
    /// sub-execution passes its parent's, because `personalStorage` and
    /// `secretStorage` resolve against `context.request.auth.userId` rather
    /// than against the [`UserContext`] — so a narrowed turn that dropped this
    /// would lose the person as well as the capabilities, and a plan turn
    /// would read nobody's storage instead of a smaller part of theirs.
    pub auth_context: Option<crate::auth::JsAuthContext>,
    /// Which of the script's files the program is built from.
    ///
    /// [`crate::source_view::SourceView::Live`] for an `/engine/eval_script`, which
    /// evaluates against head. A sub-execution passes
    /// [`crate::deployments::serving_view`], so the code it runs beside is the
    /// code its caller is running rather than a newer head the caller has
    /// never executed.
    pub view: crate::source_view::SourceView,
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

pub(super) fn is_zero(value: &usize) -> bool {
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
    let prepared = match module_loader::prepare_executable_program_in(
        &params.script_uri,
        &content,
        &params.view,
    ) {
        Ok(prepared) => prepared,
        Err(e) => fail_early!(format!("Failed to bundle script: {}", e)),
    };

    let limits = ExecutionLimits {
        timeout_ms: params.timeout_ms,
        ..current_execution_limits()
    };

    let (rt, _budget) = match create_sandboxed_runtime(&limits) {
        Ok(armed) => armed,
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

    // A snippet's own `database.transaction()` is a savepoint inside the
    // guard's transaction, so committing it does not end the guard's. Report
    // what is true rather than echoing the request all the same.
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
pub(super) fn run_snippet(
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
        // Same rule as a test run: the APIs stay callable and report that they
        // did nothing, rather than installing registrations that outlive the
        // request with no rollback to undo them. Contained, because this is
        // shared by `/engine/eval_script` and `sandbox.run`, and the second
        // runs model-authored code.
        let security_config = GlobalSecurityConfig {
            console_sink: Some(std::sync::Arc::clone(console)),
            ..GlobalSecurityConfig::new(
                Principal::Contained(params.user_context.clone()),
                HandlerInvocationKind::Eval.log_context(
                    &params.script_uri,
                    invocation_id.clone(),
                    None,
                ),
            )
        };

        let mut outcome = EvalOutcome::default();

        if let Err(e) =
            setup_secure_global_functions(ctx, &params.script_uri, security_config, None)
        {
            outcome.error = Some(format!(
                "install globals: {}",
                extract_error_details(ctx, &e)
            ));
            return Err(Box::new(outcome));
        }

        let mut builder = JsHandlerContextBuilder::new(HandlerInvocationKind::Eval)
            .with_script_metadata(params.script_uri.clone(), "eval")
            .with_invocation_id(invocation_id.clone());
        if let Some(auth) = params.auth_context.clone() {
            // Paired with an empty request, as the delegated-task path pairs
            // them: `context.request` is where the auth object hangs, so an
            // identity with no request to hang it on is one `personalStorage`
            // and `secretStorage` cannot find.
            builder = builder
                .with_request(JsRequestContext::default())
                .with_auth_context(auth);
        }
        if let Some(input) = params.input.clone() {
            builder = builder.with_args(input);
        }

        match builder.build(ctx) {
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
        let snippet = match module_loader::prepare_snippet(
            &params.script_uri,
            &params.source,
            &params.view,
        ) {
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
pub(super) fn install_require_alias(ctx: &rquickjs::Ctx<'_>) {
    let _ = ctx.eval::<(), _>(
        r#"
        if (typeof __asset_module_require__ === "function" && typeof globalThis.require !== "function") {
            globalThis.require = __asset_module_require__;
        }
        "#,
    );
}

/// Module paths the bundle holds, or an empty list when it has no modules.
pub(super) fn bundled_module_paths(ctx: &rquickjs::Ctx<'_>) -> Vec<String> {
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
pub(super) fn unresolvable_imports(
    ctx: &rquickjs::Ctx<'_>,
    dependencies: &[String],
) -> Option<String> {
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
pub(super) fn js_type_of(value: &rquickjs::Value<'_>) -> String {
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
pub(super) fn json_stringify<'js>(
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
