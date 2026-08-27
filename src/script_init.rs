use crate::error::AppResult;
use crate::repository;
use crate::repository::Repository;
use crate::scheduler;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

/// Extra wall-clock time the outer init timeout allows beyond the runtime's own
/// interrupt budget, so the interrupt is what normally stops a slow init() —
/// it reports the routes registered so far, the outer timeout cannot.
const INIT_TIMEOUT_GRACE_MS: u64 = 5_000;

/// The init() budget from configuration, set once at startup.
static CONFIGURED_INIT_TIMEOUT_MS: OnceLock<u64> = OnceLock::new();

/// Record the configured init() budget (`javascript.init_timeout_ms`, falling
/// back to `javascript.execution_timeout_ms`). Returns false if already set.
pub fn configure_init_timeout(timeout_ms: u64) -> bool {
    CONFIGURED_INIT_TIMEOUT_MS.set(timeout_ms).is_ok()
}

/// The init() budget in effect. Every path that re-initializes a script — server
/// startup, a local upsert, a peer's upsert notification — must use this one
/// value: a script that initializes on boot but gets a shorter budget on deploy
/// comes back up with no routes, which is invisible until someone requests one.
pub fn configured_init_timeout_ms() -> u64 {
    CONFIGURED_INIT_TIMEOUT_MS
        .get()
        .copied()
        .unwrap_or_else(|| crate::js_engine::current_execution_limits().timeout_ms)
}

/// How many `init()` calls may be in flight at once.
///
/// Sequential initialization makes startup cost the *sum* of every script's
/// `init()`, and a script that blocks costs its whole timeout budget. Several
/// such scripts therefore delay every script behind them, and the delay grows
/// with the number of scripts deployed — a per-instance cost paid on every boot
/// in the cluster. Running them concurrently makes startup cost roughly the
/// slowest `init()` instead of the total.
///
/// Bounded rather than unlimited because each concurrent init holds a blocking
/// thread and its own QuickJS runtime: the ceiling here is memory, not
/// scheduling.
fn init_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(4, 16)
}

/// Attribute an init failure the engine reports to the script's startup.
///
/// It carries no invocation id: the run it describes either never reached the
/// sandbox or was abandoned mid-run, so there is no id from that run to share.
/// `kind` still puts the line with the rest of what bringing the script up
/// produced.
fn init_log_context(script_uri: &str) -> repository::LogContext {
    repository::LogContext {
        request_id: None,
        kind: Some(
            crate::js_engine::HandlerInvocationKind::Init
                .as_str()
                .to_string(),
        ),
        route: None,
        // An init failure belongs to the version that failed to initialise,
        // which is the one this instance is running.
        revision: crate::revisions::current(script_uri),
    }
}

/// Result of a script initialization attempt
#[derive(Debug, Clone)]
pub struct InitResult {
    pub script_uri: String,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl InitResult {
    /// Create a successful init result
    pub fn success(script_uri: String, duration_ms: u64) -> Self {
        Self {
            script_uri,
            success: true,
            error: None,
            duration_ms,
        }
    }

    /// Create a failed init result
    pub fn failed(script_uri: String, error: String, duration_ms: u64) -> Self {
        Self {
            script_uri,
            success: false,
            error: Some(error),
            duration_ms,
        }
    }

    /// Create a skipped init result (no init function)
    pub fn skipped(script_uri: String) -> Self {
        Self {
            script_uri,
            success: true,
            error: None,
            duration_ms: 0,
        }
    }
}

/// Context provided to the init() function in JavaScript
#[derive(Debug, Clone)]
pub struct InitContext {
    pub script_name: String,
    pub timestamp: SystemTime,
    pub is_startup: bool,
}

impl InitContext {
    pub fn new(script_name: String, is_startup: bool) -> Self {
        Self {
            script_name,
            timestamp: SystemTime::now(),
            is_startup,
        }
    }
}

/// Script initializer responsible for calling init() functions
pub struct ScriptInitializer {
    timeout_ms: u64,
}

impl ScriptInitializer {
    /// Create a new script initializer
    pub fn new(timeout_ms: u64) -> Self {
        Self { timeout_ms }
    }

    /// Create an initializer with the configured init() budget. Prefer this over
    /// [`ScriptInitializer::new`] outside tests, so every re-initialization path
    /// grants a script the same budget.
    pub fn with_configured_timeout() -> Self {
        Self::new(configured_init_timeout_ms())
    }

    /// Initialize a single script by URI
    /// Run the script's `init()` and record how it went against the script's
    /// newest revision.
    ///
    /// Every write records its revision before triggering this, so the newest
    /// revision is the one whose content just ran. That is what makes "put it
    /// back to the last one that worked" answerable — the engine executed the
    /// code and knows, which is the one thing a history kept outside the
    /// engine could never tell anyone.
    pub async fn initialize_script(
        &self,
        script_uri: &str,
        is_startup: bool,
    ) -> Result<InitResult, String> {
        let result = self.run_init(script_uri, is_startup).await;

        if let Ok(init) = &result {
            crate::revisions::annotate_init(script_uri, init.success, init.error.as_deref()).await;
        }

        result
    }

    async fn run_init(&self, script_uri: &str, is_startup: bool) -> Result<InitResult, String> {
        let start_time = std::time::Instant::now();
        debug!("initialize_script: getting metadata for {}", script_uri);

        // Get script metadata
        let metadata = match repository::get_repository()
            .get_script_metadata(script_uri)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                return Ok(InitResult::failed(
                    script_uri.to_string(),
                    format!("Script not found: {}", e),
                    duration_ms,
                ));
            }
        };

        // Prevent stale scheduled work from previous deployments.
        //
        // Awaited rather than blocked on. Startup runs this once per script
        // with `init_concurrency()` inits in flight, and the blocking form
        // drives its query from a thread parked outside the runtime — enough
        // of those at once and there is nobody left to deliver the readiness
        // that would wake any of them.
        scheduler::clear_script_jobs_async(script_uri).await;

        debug!("Initializing script: {}", script_uri);

        // Create init context
        let context = InitContext::new(metadata.uri.clone(), is_startup);

        // Call init with timeout.
        //
        // Two budgets bound this call: the runtime's interrupt handler inside
        // `call_init_if_exists_with_timeout`, and this outer one. The interrupt
        // is the better of the two — it returns the routes init() registered
        // before running out of time, whereas expiring here abandons the
        // blocking task and its registrations with it. So the outer budget gets
        // a grace period and serves as a last resort.
        //
        // It used to be the only cover for an init() blocked in a host call —
        // a database query, say — where no bytecode executes for the interrupt
        // to stop. The host-call budget now bounds those directly, which leaves
        // this for what neither reaches: a wait inside the engine itself.
        let timeout_duration =
            Duration::from_millis(self.timeout_ms.saturating_add(INIT_TIMEOUT_GRACE_MS));
        let script_uri_clone = script_uri.to_string();
        let metadata_clone = metadata.clone();

        debug!("Spawning blocking task for {}", script_uri);
        let init_timeout_ms = self.timeout_ms;
        let (ticket, watch) = crate::worker_census::watch(format!("init {}", script_uri));
        let result = timeout(
            timeout_duration,
            tokio::task::spawn_blocking(move || {
                let _ticket = ticket;
                debug!("Inside spawn_blocking for {}", script_uri_clone);
                crate::js_engine::call_init_if_exists_with_timeout(
                    &script_uri_clone,
                    &metadata_clone.content,
                    context,
                    init_timeout_ms,
                )
            }),
        )
        .await;
        debug!("Blocking task finished for {}", script_uri);

        let duration_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(init_result)) => match init_result {
                Ok(Some(registrations)) => {
                    // Init function was called successfully and returned registrations
                    if let Err(e) = repository::get_repository()
                        .update_script_init_status(script_uri, true, None, Some(registrations))
                        .await
                    {
                        warn!(
                            "Failed to mark script as initialized with registrations: {}",
                            e
                        );
                    }
                    info!(
                        "✓ Script '{}' initialized successfully in {}ms",
                        script_uri, duration_ms
                    );
                    Ok(InitResult::success(script_uri.to_string(), duration_ms))
                }
                Ok(None) => {
                    // No init function found - this is OK
                    debug!("Script '{}' has no init() function (skipped)", script_uri);
                    Ok(InitResult::skipped(script_uri.to_string()))
                }
                Err(failure) => {
                    // Init function threw or ran out of budget (message already
                    // formatted by call_init_if_exists). Offer the routes it
                    // registered before failing: the repository installs them
                    // only when the script has no working route table to lose,
                    // which is what makes a first deploy with a slow init()
                    // reachable instead of invisible.
                    let partial = Some(failure.registrations).filter(|regs| !regs.is_empty());
                    let e = failure.error;
                    if let Err(err) = repository::get_repository()
                        .update_script_init_status(script_uri, false, Some(e.clone()), partial)
                        .await
                    {
                        warn!("Failed to mark script init as failed: {}", err);
                    }
                    // Log FATAL error to database
                    if let Err(err) = repository::get_repository()
                        .insert_log(script_uri, &e, "FATAL", &init_log_context(script_uri))
                        .await
                    {
                        warn!("Failed to log error to database: {}", err);
                    }
                    warn!("✗ Script '{}' init failed: {}", script_uri, e);
                    Ok(InitResult::failed(script_uri.to_string(), e, duration_ms))
                }
            },
            Ok(Err(join_error)) => {
                // Task panicked or was cancelled
                let error_msg = format!("Init task failed: {}", join_error);
                if let Err(e) = repository::get_repository()
                    .update_script_init_status(script_uri, false, Some(error_msg.clone()), None)
                    .await
                {
                    warn!("Failed to mark script init as failed: {}", e);
                }
                // Log FATAL error to database
                if let Err(err) = repository::get_repository()
                    .insert_log(
                        script_uri,
                        &error_msg,
                        "FATAL",
                        &init_log_context(script_uri),
                    )
                    .await
                {
                    warn!("Failed to log error to database: {}", err);
                }
                error!("✗ Script '{}' init task failed: {}", script_uri, join_error);
                Ok(InitResult::failed(
                    script_uri.to_string(),
                    error_msg,
                    duration_ms,
                ))
            }
            Err(_timeout_error) => {
                watch.abandon();
                // The backstop fired: the blocking task is abandoned mid-run, so
                // there are no registrations to recover here.
                let error_msg = format!(
                    "Init timeout ({}ms + {}ms grace)",
                    self.timeout_ms, INIT_TIMEOUT_GRACE_MS
                );
                if let Err(e) = repository::get_repository()
                    .update_script_init_status(script_uri, false, Some(error_msg.clone()), None)
                    .await
                {
                    warn!("Failed to mark script init as failed: {}", e);
                }
                // Log FATAL error to database
                if let Err(err) = repository::get_repository()
                    .insert_log(
                        script_uri,
                        &error_msg,
                        "FATAL",
                        &init_log_context(script_uri),
                    )
                    .await
                {
                    warn!("Failed to log error to database: {}", err);
                }
                error!(
                    "✗ Script '{}' init timeout after {}ms (blocked in a host call?)",
                    script_uri,
                    self.timeout_ms + INIT_TIMEOUT_GRACE_MS
                );
                Ok(InitResult::failed(
                    script_uri.to_string(),
                    error_msg,
                    duration_ms,
                ))
            }
        }
    }

    /// Initialize all registered scripts (typically called on server startup)
    pub async fn initialize_all_scripts(&self) -> AppResult<Vec<InitResult>> {
        info!("Initializing all registered scripts...");
        let start_time = std::time::Instant::now();

        // Get all script metadata
        let all_metadata = match repository::get_repository().get_all_script_metadata().await {
            Ok(metadata) => metadata,
            Err(e) => {
                error!("Failed to get script metadata: {}", e);
                return Err(e);
            }
        };

        if all_metadata.is_empty() {
            info!("No dynamic scripts to initialize");
            return Ok(vec![]);
        }

        info!("Found {} scripts to initialize", all_metadata.len());

        let concurrency = init_concurrency();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut running = tokio::task::JoinSet::new();

        // One task per script, bounded by a semaphore — not a combinator over a
        // single task.
        //
        // `initialize_script` waits on a `spawn_blocking` for the `init()` call
        // itself, so each one needs the future to *own* a task. Driven as
        // futures inside one task (`buffer_unordered` and friends), the first
        // one to block holds the only poll slot, so the rest — including their
        // timeouts — never get polled, and initialization stalls outright
        // rather than merely running in sequence.
        for (position, metadata) in all_metadata.into_iter().enumerate() {
            let permits = permits.clone();
            let timeout_ms = self.timeout_ms;
            running.spawn(async move {
                // Held for the script's whole initialization, so `concurrency`
                // bounds the QuickJS runtimes alive at once rather than the
                // tasks queued.
                let _permit = permits.acquire_owned().await;
                let initializer = ScriptInitializer::new(timeout_ms);
                let result = match initializer.initialize_script(&metadata.uri, true).await {
                    Ok(result) => result,
                    Err(e) => {
                        error!("Failed to initialize script {}: {}", metadata.uri, e);
                        InitResult::failed(metadata.uri.clone(), e.to_string(), 0)
                    }
                };
                (position, result)
            });
        }

        let mut indexed: Vec<(usize, InitResult)> = Vec::new();
        while let Some(joined) = running.join_next().await {
            match joined {
                Ok(pair) => indexed.push(pair),
                // Only a panic or a cancellation lands here: `initialize_script`
                // reports a script's own failures as an `InitResult`.
                Err(e) => error!("Script initialization task failed: {}", e),
            }
        }

        // Back into metadata order, so the summary and its logs read the same
        // from one boot to the next.
        indexed.sort_by_key(|(position, _)| *position);
        let results: Vec<InitResult> = indexed.into_iter().map(|(_, result)| result).collect();

        let total_duration = start_time.elapsed().as_millis();
        let successful = results.iter().filter(|r| r.success).count();
        let failed = results.iter().filter(|r| !r.success).count();

        info!(
            "Script initialization complete: {} successful, {} failed, {}ms total ({} at a time)",
            successful, failed, total_duration, concurrency
        );

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_result_creation() {
        let success = InitResult::success("test".to_string(), 100);
        assert!(success.success);
        assert_eq!(success.duration_ms, 100);
        assert!(success.error.is_none());

        let failed = InitResult::failed("test".to_string(), "error".to_string(), 200);
        assert!(!failed.success);
        assert_eq!(failed.duration_ms, 200);
        assert_eq!(failed.error, Some("error".to_string()));

        let skipped = InitResult::skipped("test".to_string());
        assert!(skipped.success);
        assert_eq!(skipped.duration_ms, 0);
    }

    #[test]
    fn test_init_context_creation() {
        let context = InitContext::new("test_script".to_string(), true);
        assert_eq!(context.script_name, "test_script");
        assert!(context.is_startup);
    }
}
