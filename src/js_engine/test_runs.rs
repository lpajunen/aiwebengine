//! Running a script's `*.test.ts` modules.

use super::*;
use crate::module_loader;
use crate::script_test::{TestCaseResult, TestRunResult};
use crate::security::UserContext;
use crate::security::secure_globals::{GlobalSecurityConfig, Principal};
use rquickjs::{Context, Function, Runtime, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tracing::warn;

/// The JavaScript authoring API (`test`, `expect`, hooks) evaluated into a test
/// context ahead of the test module, so the module can call it as it loads.
pub(super) const TEST_PRELUDE: &str = include_str!("../../assets/test_prelude.js");

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
    /// Which version of the script's files the run bundles. Defaults to what
    /// is deployed; a revision runs the tests a revision contained, against
    /// the modules that revision had, rather than against whatever has been
    /// written since.
    pub view: crate::source_view::SourceView,
}

/// What running one test module produced.
pub(super) struct ModuleOutcome {
    pub(super) cases: Vec<TestCaseResult>,
    /// The module ran out of budget, so the cases after the interrupt never
    /// ran and no verdict exists for them.
    pub(super) timed_out: bool,
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
                    TestCaseResult::failed(
                        module_path.clone(),
                        crate::fix_hints::with_hint(&error),
                        0,
                    )
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
pub(super) fn execute_test_module(
    params: &TestRunParams,
    module_path: &str,
    budget_ms: u64,
) -> Result<ModuleOutcome, String> {
    // Bundle before arming the runtime's interrupt deadline (see
    // `execute_script_for_request_secure`): on a cold cache this fetches and
    // transpiles every module the test imports, which must not be charged to
    // the budget meant for running the tests.
    let modules = [module_path.to_string()];
    let prepared =
        module_loader::prepare_test_program_in(&params.script_uri, &modules, &params.view)
            .map_err(|e| format!("Failed to bundle test module: {}", e))?;

    let limits = ExecutionLimits {
        timeout_ms: budget_ms,
        ..current_execution_limits()
    };
    // Mirrors the deadline `create_sandboxed_runtime` arms the interrupt with,
    // so a failed call can be told apart from a call the interrupt stopped
    // without matching on QuickJS error text.
    let deadline = Instant::now() + Duration::from_millis(budget_ms);

    let (rt, _budget) = create_sandboxed_runtime(&limits)?;
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
pub(super) fn install_and_collect_tests<'js>(
    ctx: &rquickjs::Ctx<'js>,
    params: &TestRunParams,
    module_path: &str,
    code: &str,
) -> Result<Vec<(String, rquickjs::Persistent<Function<'static>>)>, String> {
    let invocation_id = crate::middleware::generate_request_id();
    // A test must not mutate registries that outlive the run: routes,
    // streams, and jobs registered here would stay registered, and no
    // rollback undoes them. Outside the registration phase the APIs stay
    // callable and report that they did nothing, rather than disappearing
    // from the test's global scope.
    let security_config = GlobalSecurityConfig::new(
        Principal::Contained(params.user_context.clone()),
        HandlerInvocationKind::Test.log_context(
            &params.script_uri,
            invocation_id.clone(),
            Some(module_path.to_string()),
        ),
    );

    setup_secure_global_functions(ctx, &params.script_uri, security_config, None)
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
pub(super) fn run_test_module(
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
                    TestCaseResult::failed(
                        name,
                        crate::fix_hints::with_hint(&details),
                        duration_ms,
                    )
                    .from_file(module_path),
                );
            }
        }
    }

    Ok(ModuleOutcome { cases, timed_out })
}
