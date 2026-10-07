//! The sandboxed runtime: its budget, the job queue, settling a handler's promise and
//! finishing its transaction.

use super::*;
use crate::security::UserContext;
use rquickjs::{Context, Function, Runtime, Value};
use std::time::{Duration, Instant};
use tracing::warn;

/// Creates a QuickJS runtime with memory, stack, and wall-clock limits enforced.
///
/// The budget is enforced twice over, because one mechanism cannot see what the
/// other does. The interrupt handler stops a runaway script (`while (true) {}`)
/// between bytecode operations, and is blind to a call that has left JavaScript
/// to wait on the database. The [`HostCallBudget`] bounds exactly that call,
/// and is blind to a loop that never makes one. Outer tokio timeouts do neither
/// — they abandon the blocking thread while the work continues on it.
///
/// Both run from the same deadline, so a script gets one budget however it
/// chooses to spend it.
///
/// The returned guard must outlive the runtime: dropping it early leaves the
/// script running with its host calls unbounded again.
#[must_use = "the host-call budget ends when its guard is dropped"]
pub(crate) fn create_sandboxed_runtime(
    limits: &ExecutionLimits,
) -> Result<(Runtime, crate::database::HostCallBudget), String> {
    let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    rt.set_memory_limit(limits.max_memory_mb * 1024 * 1024);
    rt.set_max_stack_size(limits.stack_size_bytes);
    // Never longer than what the execution this one is nested inside has left.
    // At the top level nothing is armed and the budget is the configured one
    // untouched; nested — one script dispatching a message to the next — a
    // fresh full budget would let a chain of listeners outlive the request
    // that started it, one handler at a time.
    let budget = crate::database::within_host_budget(Duration::from_millis(limits.timeout_ms));
    let deadline = Instant::now() + budget;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    Ok((rt, crate::database::bound_host_calls(deadline)))
}

/// What a request-driven invocation runs as: the calling user, or nobody.
///
/// Stream customization functions used to run as `UserContext::admin(...)`
/// while the caller's own identity was passed alongside for the JavaScript
/// `auth` object to read — so `auth.isAdmin` could say false in the same
/// invocation that held `AdministerEngine`. It is entered by a request, and a
/// script serving a request runs under the requesting user's context.
///
/// `JsAuthContext::to_user_context` already mapped an identity onto its tier
/// and had no caller anywhere in the engine. Absent identity means anonymous,
/// so `None` is the stream connection that arrived with no session.
pub(super) fn caller_context(auth_context: Option<&crate::auth::JsAuthContext>) -> UserContext {
    auth_context
        .map(|identity| identity.to_user_context())
        .unwrap_or_else(UserContext::anonymous)
}

/// Upper bound on microtasks drained for one invocation.
///
/// The runtime's interrupt handler is the real guard: it stops a chain that
/// re-enqueues itself at the execution deadline, leaving the promise pending.
/// This cap only exists so a queue that somehow outruns the deadline check
/// cannot spin forever.
pub(super) const MAX_DRAINED_JOBS: usize = 1_000_000;

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
pub(super) fn drain_jobs(rt: &Runtime) -> Vec<String> {
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
pub(super) fn report_unhandled(script_uri: &str, unhandled: Vec<String>) {
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
    /// Commit once the promise resolves, roll back if it rejects: the
    /// boundary closing a transaction the handler left open.
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
pub(super) fn unwrap_settled<'js>(
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

/// Commits or rolls back the request's transaction, if it opened one.
///
/// Must run *after* the microtask queue has been drained. An `async` handler
/// has not made its post-`await` writes until then, and committing earlier
/// closes the transaction out from under them.
pub(super) fn finish_transaction(succeeded: bool) -> Result<(), String> {
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
/// A transaction the script left open is the handler boundary's to finish,
/// which is what the handler paths do. `database.transaction(fn)` closes its
/// own, but an async `fn` still settling when the invocation ends does not. Paths without such
/// a boundary — `init()`, an evaluation, a dry run — would otherwise leave it
/// open on the thread, and since the transaction lives in thread-local storage
/// the next invocation to land on that thread would inherit it and have its
/// writes swallowed.
///
/// Only a transaction this invocation opened is rolled back. One that was
/// already active belongs to an outer scope — a test run's or an evaluation's
/// rollback guard — and finishing it here would cut that scope short.
pub(super) struct StrayTransaction {
    pub(super) outer_active: bool,
}

impl StrayTransaction {
    pub(super) fn arm() -> Self {
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
                 do the work inside database.transaction(fn) to keep its writes"
            );
            let _ = crate::database::Database::rollback_transaction();
        }
    }
}
