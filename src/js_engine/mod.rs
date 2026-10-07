//! Running a script in a sandboxed QuickJS runtime.
//!
//! `handlers.rs` is every way a handler is called, through one
//! `HandlerExecution`; `registration.rs` runs a script to collect what it
//! registers; `eval.rs` and `test_runs.rs` run snippets and tests. `runtime.rs`
//! holds the budget, the job queue and transaction settling they share.
use rquickjs::Context;
use std::sync::OnceLock;
use std::time::Duration;

use crate::module_loader;
use crate::repository;

// Use the enhanced secure globals implementation
mod context;
mod eval;
mod globals;
mod handlers;
mod registration;
mod runtime;
mod test_runs;
pub use context::*;
pub use eval::*;
use globals::*;
pub use handlers::*;
pub use registration::*;
pub(crate) use runtime::*;
pub use test_runs::*;

// Type alias for route registrations map
type RouteRegistrations = repository::RouteRegistrations;

/// Transpile TypeScript/JSX/TSX to JavaScript if needed.
///
/// Resolves imports through whatever the script serves here — the live rows
/// for a script that follows head, and its own revision for one that is
/// pinned. Taking the live rows unconditionally would build a pinned script's
/// root against modules from a version it was never written for.
fn transpile_if_needed(uri: &str, content: &str) -> Result<String, String> {
    transpile_if_needed_in(uri, content, &crate::deployments::serving_view(uri))
}

/// [`transpile_if_needed`], resolving the program's imports through `view`.
fn transpile_if_needed_in(
    uri: &str,
    content: &str,
    view: &crate::source_view::SourceView,
) -> Result<String, String> {
    module_loader::prepare_executable_program_in(uri, content, view)
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
    /// Stack a script may use before QuickJS throws, in bytes.
    ///
    /// The guard against unbounded recursion: QuickJS raises a JavaScript
    /// error the script can see, where an unbounded stack takes the process
    /// down with it. Configured as `javascript.stack_size_bytes`.
    pub stack_size_bytes: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            timeout_ms: 2000,
            max_memory_mb: 50,
            max_script_size_bytes: 1_000_000, // 1MB
            stack_size_bytes: 512 * 1024,
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

thread_local! {
    /// Carries the last globals-install sub-timings (ctor, native fns, response
    /// builders) so the per-request profiler can log them
    /// *outside* any timed window — logging inside would inflate the reading.
    static GLOBALS_BREAKDOWN: std::cell::Cell<Option<(Duration, Duration, Duration)>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
mod tests;
