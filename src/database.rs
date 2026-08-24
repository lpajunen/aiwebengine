use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{PgConnection, Postgres, Transaction};
use std::cell::RefCell;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::config::RepositoryConfig;

/// Drive `future` to completion from a blocking context.
///
/// Inside a tokio runtime this hands off to `block_in_place` so the reactor
/// keeps running (it requires the multi-threaded runtime — a default
/// `#[tokio::test]` will panic here). Outside one, it uses the shared fallback
/// runtime below.
pub(crate) fn run_blocking<F, R>(future: F) -> R
where
    F: std::future::Future<Output = R>,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(move || handle.block_on(future)),
        Err(_) => fallback_runtime().block_on(future),
    }
}

/// The runtime used when a database call happens outside any tokio runtime
/// (tests, CLI entry points).
///
/// It must be a *single* process-wide runtime rather than one built per call.
/// A sqlx connection is driven by a background task belonging to the runtime
/// that opened it, so a throwaway runtime leaves the pool holding connections
/// whose driver is gone the moment it is dropped. The next caller then acquires
/// one of those corpses and blocks on it until the pool's acquire timeout
/// expires — 30s by default — and the query surfaces as a "not found" rather
/// than as an error.
fn fallback_runtime() -> &'static tokio::runtime::Runtime {
    static FALLBACK: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    FALLBACK.get_or_init(|| {
        // Multi-threaded so `block_on` from several threads can make progress
        // concurrently, and so `block_in_place` stays legal inside it.
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            // Same failure mode as the per-call build this replaces: if the
            // process cannot build a tokio runtime, no database call can work.
            .expect("Failed to build the database fallback runtime")
    })
}

thread_local! {
    /// The instant after which host calls on this thread stop waiting.
    ///
    /// Armed for the length of a script execution and restored when it ends,
    /// so a pooled blocking thread never inherits an expired budget from the
    /// run before it.
    static HOST_CALL_DEADLINE: std::cell::Cell<Option<Instant>> =
        const { std::cell::Cell::new(None) };
}

/// Bounds every host call made on this thread until it is dropped.
///
/// The QuickJS interrupt handler stops a runaway script between bytecode
/// operations, which is everything except the case that matters here: a call
/// that left JavaScript and is waiting on Postgres. The interrupt cannot reach
/// into it, and the timeout around the request only abandons the blocking
/// thread — the work keeps running on it, holding whatever it holds. This is
/// how the budget follows the execution across that boundary.
///
/// What it stops is the *engine's* wait, not the database's work. Dropping a
/// query future does not cancel the statement Postgres is running; the session
/// guards do that. What it buys is control returning to JavaScript, so the
/// handler unwinds and the commit-or-rollback at its boundary actually runs
/// instead of being skipped by a thread that never came back.
pub struct HostCallBudget {
    /// Restored rather than cleared, so an execution nested inside another —
    /// one script dispatching to the next — gives the outer budget back.
    previous: Option<Instant>,
}

impl Drop for HostCallBudget {
    fn drop(&mut self) {
        HOST_CALL_DEADLINE.with(|deadline| deadline.set(self.previous));
    }
}

/// Bounds host calls on this thread until the returned guard is dropped.
pub fn bound_host_calls(deadline: Instant) -> HostCallBudget {
    let previous = HOST_CALL_DEADLINE.with(|cell| cell.replace(Some(deadline)));
    HostCallBudget { previous }
}

/// What is left of the current execution's budget, if one is armed.
///
/// An expired budget reports a zero duration rather than `None`: the call must
/// still fail, and `tokio::time::timeout` with a zero duration is the shortest
/// way to say so.
fn remaining_host_budget() -> Option<Duration> {
    HOST_CALL_DEADLINE.with(|cell| {
        cell.get()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    })
}

/// `limit`, shortened to whatever remains of the execution budget.
///
/// For a host call that already bounds its own wait — `fetch` has a request
/// timeout of its own — and only needs to be stopped from outliving the script
/// that made it. With no budget armed the limit is returned untouched.
///
/// Lives here with [`run_blocking`] because both are the same bridge: the point
/// where a script's thread leaves JavaScript to wait on something. It is not
/// specific to the database.
pub fn within_host_budget(limit: Duration) -> Duration {
    match remaining_host_budget() {
        Some(remaining) => limit.min(remaining),
        None => limit,
    }
}

/// Drives `future` to completion, giving up if the execution budget runs out.
///
/// The bounded counterpart of [`run_blocking`], and what every host call a
/// script can reach should use. Without a budget armed — engine-internal work,
/// startup, the scheduler — it behaves exactly like `run_blocking`.
///
/// Deliberately not used for committing or rolling back a transaction. Those
/// are what a timed-out call unwinds *into*, and cutting a rollback short
/// because the budget is already gone would leave open the transaction the
/// rollback exists to close.
pub(crate) fn run_bounded<F, T>(future: F) -> crate::error::AppResult<T>
where
    F: std::future::Future<Output = crate::error::AppResult<T>>,
{
    let Some(remaining) = remaining_host_budget() else {
        return run_blocking(future);
    };

    run_blocking(async move {
        match tokio::time::timeout(remaining, future).await {
            Ok(result) => result,
            Err(_) => Err(crate::error::AppError::JsTimeout {
                timeout_ms: remaining.as_millis() as u64,
            }),
        }
    })
}

/// Transaction state stored in thread-local storage
pub struct TransactionState {
    /// The active PostgreSQL transaction
    transaction: Option<Transaction<'static, Postgres>>,
    /// Stack of active savepoint names
    savepoint_stack: Vec<String>,
    /// Counter for generating unique savepoint names
    savepoint_counter: usize,
    /// Timeout deadline for the transaction
    deadline: Option<Instant>,
    /// Transaction start time
    _start_time: Instant,
    /// Whether the transaction has been finalized (committed or rolled back)
    finalized: bool,
}

impl TransactionState {
    fn new(transaction: Transaction<'static, Postgres>, timeout: Option<Duration>) -> Self {
        let start_time = Instant::now();
        let deadline = timeout.map(|d| start_time + d);

        Self {
            transaction: Some(transaction),
            savepoint_stack: Vec::new(),
            savepoint_counter: 0,
            deadline,
            _start_time: start_time,
            finalized: false,
        }
    }

    fn check_timeout(&self) -> Result<(), String> {
        if let Some(deadline) = self.deadline
            && Instant::now() > deadline
        {
            return Err("Transaction timeout exceeded".to_string());
        }
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.transaction.is_some() && !self.finalized
    }
}

// Thread-local storage for the current transaction
thread_local! {
    static CURRENT_TRANSACTION: RefCell<Option<TransactionState>> = const { RefCell::new(None) };
}

/// Get the current transaction state (if any)
pub fn get_current_transaction_active() -> bool {
    CURRENT_TRANSACTION.with(|tx| tx.borrow().as_ref().is_some_and(|t| t.is_active()))
}

/// Get a raw pointer to the current transaction for use in synchronous repository functions
///
/// # Safety
/// This is only safe to use within the same thread and within the transaction's lifetime.
/// The transaction must not be moved or dropped while the pointer is in use.
/// This is intended for use in repository functions called from within handler execution.
pub fn get_current_transaction_ptr() -> Option<*mut Transaction<'static, Postgres>> {
    CURRENT_TRANSACTION.with(|tx| {
        tx.borrow_mut()
            .as_mut()
            .and_then(|state| state.transaction.as_mut().map(|t| t as *mut _))
    })
}

/// RAII guard for automatic transaction rollback on drop
#[derive(Debug)]
pub struct TransactionGuard {
    committed: bool,
}

impl TransactionGuard {
    fn new() -> Self {
        Self { committed: false }
    }

    pub fn commit(&mut self) {
        self.committed = true;
    }

    /// Gives up the rollback-on-drop without ending the transaction.
    ///
    /// For a caller that holds the guard across the work it is protecting,
    /// dropping it is the point. `database.beginTransaction()` is the opposite
    /// case: the script expects the transaction to still be open on the next
    /// line, so the guard must not outlive the call that made it, and the
    /// transaction must. Whoever releases it takes on finishing it — for a
    /// script that is the handler boundary, which commits on success and rolls
    /// back on failure.
    pub fn release(mut self) {
        self.committed = true;
    }
}

impl Drop for TransactionGuard {
    fn drop(&mut self) {
        if !self.committed {
            // Attempt to rollback on drop (panic or early return)
            let _ = Database::rollback_transaction();
        }
    }
}

/// Safe wrapper around either a transaction or a connection pool
///
/// This type provides a safe abstraction for executing queries within or outside
/// of a transaction context. It eliminates the need for unsafe raw pointer operations
/// by providing a type-safe way to access the active transaction or fall back to the pool.
pub enum TransactionExecutor<'a> {
    /// Execute within an active transaction
    Transaction(&'a mut Transaction<'static, Postgres>),
    /// Execute directly on the connection pool
    Pool(&'a PgPool),
}

/// Get a safe executor for the current context
///
/// Returns a TransactionExecutor that wraps either the active transaction or the pool.
/// This function safely checks thread-local transaction state and provides the appropriate
/// executor without requiring unsafe pointer operations in calling code.
///
/// # Safety
/// This function uses unsafe code to extend lifetimes from thread-local storage.
/// It is safe because:
/// 1. The transaction is stored in thread-local storage and cannot be accessed from other threads
/// 2. The transaction lifetime is managed by the thread-local RefCell borrow
/// 3. The returned executor must be used immediately within the same scope
/// 4. The transaction cannot be committed/rolled back while this borrow is active
///
/// # Arguments
/// * `pool` - The connection pool to use if no transaction is active
///
/// # Returns
/// A TransactionExecutor that can be used with SQLx query execution
pub fn get_current_executor(pool: &PgPool) -> TransactionExecutor<'_> {
    // Check if we have an active transaction
    if let Some(tx_ptr) = get_current_transaction_ptr() {
        // Safety: The pointer is valid for the duration of this call because:
        // - It's stored in thread-local storage
        // - The transaction cannot be dropped while we're in a handler
        // - We're only using it within this thread
        unsafe {
            let tx_ref: &mut Transaction<'static, Postgres> = &mut *tx_ptr;
            // Transmute to extend the lifetime for the return value
            // This is safe because the transaction lives in thread-local storage
            // and will outlive this function call
            return TransactionExecutor::Transaction(std::mem::transmute::<
                &mut Transaction<'static, Postgres>,
                &mut Transaction<'static, Postgres>,
            >(tx_ref));
        }
    }

    // No active transaction, use the pool
    TransactionExecutor::Pool(pool)
}

/// Global database instance
///
/// This is initialized once during server startup and provides
/// access to the database pool for health checks and queries.
/// Access via `get_global_database()` function.
static GLOBAL_DATABASE: OnceLock<Arc<Database>> = OnceLock::new();

/// Get the global database instance
///
/// Returns None if the database has not been initialized yet.
pub fn get_global_database() -> Option<Arc<Database>> {
    GLOBAL_DATABASE.get().cloned()
}

/// Initialize the global database instance
///
/// Returns true if successfully initialized, false if already set.
pub fn initialize_global_database(database: Arc<Database>) -> bool {
    GLOBAL_DATABASE.set(database).is_ok()
}

/// The limits every pooled connection carries for its whole session.
///
/// Postgres is the only party in this system that can interrupt a database
/// call. The runtime's interrupt handler enforces a script's budget between
/// bytecode operations, and a host call is opaque to it; the outer timeout on
/// a request abandons the blocking thread but cannot stop the work running on
/// it. So a statement waiting on a lock waits forever, holding whatever locks
/// it already took, with every later writer queued behind it — the shape of a
/// wedge that outlives the request, the handler, and often the operator's
/// patience.
///
/// These three settings are the floor under all of that. They do not make a
/// blocked call correct; they make it finite, and turn a silent cluster-wide
/// stall into an error a script can catch and a line in the log.
///
/// `0` disables a setting, exactly as it does in Postgres.
#[derive(Clone, Copy)]
struct SessionGuards {
    lock_timeout_ms: u64,
    statement_timeout_ms: u64,
    idle_in_transaction_timeout_ms: u64,
}

impl SessionGuards {
    fn from_config(config: &RepositoryConfig) -> Self {
        Self {
            lock_timeout_ms: config.lock_timeout_ms,
            statement_timeout_ms: config.statement_timeout_ms,
            idle_in_transaction_timeout_ms: config.idle_in_transaction_timeout_ms,
        }
    }

    /// Every guard off. What migrations run under.
    fn none() -> Self {
        Self {
            lock_timeout_ms: 0,
            statement_timeout_ms: 0,
            idle_in_transaction_timeout_ms: 0,
        }
    }

    /// The guards a transaction with this budget runs under.
    ///
    /// A budget may only tighten. `beginTransaction(600000)` must not be a way
    /// for a script to buy itself ten minutes of lock waiting when the engine
    /// allows five seconds, so each guard is the lower of the two — treating a
    /// disabled ceiling as no ceiling at all.
    ///
    /// A budget of zero would read to Postgres as "disabled", the exact
    /// opposite of what asking for it means, so it becomes the shortest budget
    /// expressible instead.
    fn tightened_to(self, budget_ms: u64) -> Self {
        let budget_ms = budget_ms.max(1);
        let tighten = |ceiling: u64| match ceiling {
            0 => budget_ms,
            ceiling => ceiling.min(budget_ms),
        };
        Self {
            lock_timeout_ms: tighten(self.lock_timeout_ms),
            statement_timeout_ms: tighten(self.statement_timeout_ms),
            idle_in_transaction_timeout_ms: tighten(self.idle_in_transaction_timeout_ms),
        }
    }

    /// The guards as Postgres setting names and values.
    fn settings(&self) -> [(&'static str, u64); 3] {
        [
            ("lock_timeout", self.lock_timeout_ms),
            ("statement_timeout", self.statement_timeout_ms),
            (
                "idle_in_transaction_session_timeout",
                self.idle_in_transaction_timeout_ms,
            ),
        ]
    }

    /// Overrides what a session already connected with.
    ///
    /// One statement each: `SET` goes through the extended query protocol,
    /// which carries a single statement per round trip.
    async fn apply(self, conn: &mut PgConnection, scope: GuardScope) -> Result<(), sqlx::Error> {
        for (setting, milliseconds) in self.settings() {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "{} {} = {}",
                scope.keyword(),
                setting,
                milliseconds
            )))
            .execute(&mut *conn)
            .await?;
        }
        Ok(())
    }
}

/// How long an application of [`SessionGuards`] lasts.
#[derive(Clone, Copy)]
enum GuardScope {
    /// Until the connection closes.
    Session,
    /// Until the surrounding transaction ends, which is what lets a
    /// transaction's own budget bind without following the connection back
    /// into the pool.
    Transaction,
}

impl GuardScope {
    fn keyword(self) -> &'static str {
        match self {
            GuardScope::Session => "SET",
            GuardScope::Transaction => "SET LOCAL",
        }
    }
}

/// Database connection pool manager
pub struct Database {
    pool: PgPool,
    /// The ceiling a transaction's own budget is measured against.
    guards: SessionGuards,
}

impl Database {
    /// Create a new database instance from an existing pool (useful for testing)
    ///
    /// The pool arrives already configured or not at all, so there is no
    /// ceiling to measure a transaction's budget against: whatever it asks for
    /// is what it gets.
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            guards: SessionGuards::none(),
        }
    }

    /// Create a new database connection pool
    pub async fn new(config: &RepositoryConfig) -> Result<Self> {
        let connection_string = &config.connection_string;

        // Log connection attempt (hide password)
        let safe_conn_string = if let Some(at_pos) = connection_string.find('@') {
            let before_at = &connection_string[..at_pos];
            let after_at = &connection_string[at_pos..];
            if let Some(colon_pos) = before_at.rfind(':') {
                format!("{}:****{}", &before_at[..colon_pos], after_at)
            } else {
                connection_string.clone()
            }
        } else {
            connection_string.clone()
        };

        info!("Attempting to connect to database: {}", safe_conn_string);

        let max_connections = config.max_connections;

        // Carried in the startup packet rather than set by a callback after
        // connecting: the server applies them before it will run anything, so
        // there is no window in which a connection is live but unguarded, and
        // no round trip spent on saying so.
        let guards = SessionGuards::from_config(config);
        let connect_options = connection_string
            .parse::<sqlx::postgres::PgConnectOptions>()
            .context("Failed to parse the database connection string")?
            .options(
                guards
                    .settings()
                    .map(|(setting, milliseconds)| (setting, milliseconds.to_string())),
            );

        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_millis(5000)) // Increased for tests
            .connect_with(connect_options)
            .await
            .context("Failed to connect to database")?;

        info!("✓ Database connection established successfully");
        info!(
            "✓ Connection pool created with max {} connections",
            max_connections
        );

        Ok(Self { pool, guards })
    }

    /// Run database migrations.
    ///
    /// Migrations run with the session guards cleared. They are the one place
    /// where a long statement and a long lock wait are both expected —
    /// rewriting a large table, or waiting out the traffic already on it — and
    /// a `statement_timeout` that cancelled one partway would leave the schema
    /// between versions. The guards exist to bound a script's work, not the
    /// engine's own.
    pub async fn migrate(&self) -> Result<()> {
        info!("Running database migrations...");

        // Detached rather than borrowed: a connection whose guards have been
        // cleared must not go back to the pool, where the next script to
        // acquire it would run unbounded.
        let mut conn = self
            .pool
            .acquire()
            .await
            .context("Failed to acquire a connection for migrations")?
            .detach();

        SessionGuards::none()
            .apply(&mut conn, GuardScope::Session)
            .await
            .context("Failed to clear session guards for migrations")?;

        // `run_direct` rather than `run`: the latter is generic over `Acquire`,
        // and the higher-ranked lifetime that introduces defeats `Send`
        // inference for every future that transitively awaits this one.
        sqlx::migrate!("./migrations")
            .run_direct(None, &mut conn, false)
            .await
            .context("Failed to run migrations")?;

        info!("Database migrations completed successfully");
        Ok(())
    }

    /// Get a reference to the connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Check database health
    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .context("Database health check failed")?;
        Ok(())
    }

    /// Gracefully close the database connection pool
    pub async fn close(self) {
        info!("Closing database connection pool...");
        self.pool.close().await;
        info!("Database connection pool closed");
    }

    /// Begin a new database transaction
    ///
    /// If a transaction is already active, this will create a savepoint instead.
    /// Returns a TransactionGuard for automatic rollback on drop.
    ///
    /// `timeout_ms` is handed to Postgres, not just recorded. The deadline
    /// [`TransactionState`] keeps is only consulted when the engine next passes
    /// through this module — beginning, committing, taking a savepoint — and a
    /// transaction that never gets there again is exactly the one worth
    /// bounding. So the budget becomes `SET LOCAL` guards on the transaction
    /// itself: no statement in it runs longer than the budget, no lock wait
    /// exceeds it, and if the handler is abandoned mid-transaction Postgres
    /// ends it and releases its locks once the budget's worth of idleness has
    /// passed, rather than at the far more generous session default.
    ///
    /// What this does not bound is the sum: a hundred fast statements can still
    /// outlast the budget between them. Bounding that needs a check before
    /// every statement, which is the caller's execution budget's job.
    ///
    /// Nested calls take a savepoint and leave the guards alone. `SET LOCAL`
    /// belongs to the transaction rather than the savepoint, so it would not be
    /// undone on release, and an inner scope must not quietly re-bound an outer
    /// one that is still running.
    pub fn begin_transaction(timeout_ms: Option<u64>) -> Result<TransactionGuard, String> {
        CURRENT_TRANSACTION.with(|tx_cell| {
            let mut tx_option = tx_cell.borrow_mut();

            if let Some(ref mut state) = *tx_option {
                // Transaction already active - create a savepoint
                state.check_timeout()?;

                state.savepoint_counter += 1;
                let savepoint_name = format!("sp_{}", state.savepoint_counter);

                let tx_ref = state
                    .transaction
                    .as_mut()
                    .ok_or("Transaction not available")?;

                // Execute SAVEPOINT command
                run_blocking(async {
                    sqlx::query(sqlx::AssertSqlSafe(format!("SAVEPOINT {}", savepoint_name)))
                        .execute(&mut **tx_ref)
                        .await
                        .map_err(|e| format!("Failed to create savepoint: {}", e))?;
                    Ok::<(), String>(())
                })?;

                state.savepoint_stack.push(savepoint_name);
                Ok(TransactionGuard::new())
            } else {
                // No active transaction - start a new one
                let db = get_global_database().ok_or("Database not initialized")?;

                let pool = db.pool.clone();
                let guards = db.guards;

                let tx = run_blocking(async {
                    let mut tx = pool
                        .begin()
                        .await
                        .map_err(|e| format!("Failed to begin transaction: {}", e))?;

                    if let Some(budget_ms) = timeout_ms {
                        guards
                            .tightened_to(budget_ms)
                            .apply(&mut tx, GuardScope::Transaction)
                            .await
                            .map_err(|e| {
                                format!("Failed to bound the transaction to its budget: {}", e)
                            })?;
                    }

                    Ok::<_, String>(tx)
                })?;

                // Convert to 'static lifetime by leaking (will be properly cleaned up on commit/rollback)
                let tx_static: Transaction<'static, Postgres> = unsafe { std::mem::transmute(tx) };

                let timeout = timeout_ms.map(Duration::from_millis);
                *tx_option = Some(TransactionState::new(tx_static, timeout));

                Ok(TransactionGuard::new())
            }
        })
    }

    /// Commit the current transaction
    ///
    /// If savepoints are active, this will release the most recent savepoint.
    /// Otherwise, it commits the entire transaction.
    pub fn commit_transaction() -> Result<(), String> {
        CURRENT_TRANSACTION.with(|tx_cell| {
            let mut tx_option = tx_cell.borrow_mut();

            let state = tx_option
                .as_mut()
                .ok_or("No active transaction to commit")?;

            state.check_timeout()?;

            if let Some(savepoint_name) = state.savepoint_stack.pop() {
                // Release the savepoint
                let tx_ref = state
                    .transaction
                    .as_mut()
                    .ok_or("Transaction not available")?;

                run_blocking(async {
                    sqlx::query(sqlx::AssertSqlSafe(format!(
                        "RELEASE SAVEPOINT {}",
                        savepoint_name
                    )))
                    .execute(&mut **tx_ref)
                    .await
                    .map_err(|e| format!("Failed to release savepoint: {}", e))?;
                    Ok::<(), String>(())
                })?;
            } else {
                // Commit the entire transaction
                let tx = state
                    .transaction
                    .take()
                    .ok_or("Transaction not available")?;

                run_blocking(async {
                    tx.commit()
                        .await
                        .map_err(|e| format!("Failed to commit transaction: {}", e))?;
                    Ok::<(), String>(())
                })?;

                state.finalized = true;
                *tx_option = None;
            }

            Ok(())
        })
    }

    /// Rollback the current transaction
    ///
    /// If savepoints are active, this will rollback to the most recent savepoint.
    /// Otherwise, it rolls back the entire transaction.
    pub fn rollback_transaction() -> Result<(), String> {
        CURRENT_TRANSACTION.with(|tx_cell| {
            let mut tx_option = tx_cell.borrow_mut();

            let state = tx_option
                .as_mut()
                .ok_or("No active transaction to rollback")?;

            if let Some(savepoint_name) = state.savepoint_stack.pop() {
                // Rollback to the savepoint
                let tx_ref = state
                    .transaction
                    .as_mut()
                    .ok_or("Transaction not available")?;

                run_blocking(async {
                    sqlx::query(sqlx::AssertSqlSafe(format!(
                        "ROLLBACK TO SAVEPOINT {}",
                        savepoint_name
                    )))
                    .execute(&mut **tx_ref)
                    .await
                    .map_err(|e| format!("Failed to rollback to savepoint: {}", e))?;
                    Ok::<(), String>(())
                })?;
            } else {
                // Rollback the entire transaction
                let tx = state
                    .transaction
                    .take()
                    .ok_or("Transaction not available")?;

                run_blocking(async {
                    tx.rollback()
                        .await
                        .map_err(|e| format!("Failed to rollback transaction: {}", e))?;
                    Ok::<(), String>(())
                })?;

                state.finalized = true;
                *tx_option = None;
            }

            Ok(())
        })
    }

    /// Create a named savepoint
    pub fn create_savepoint(name: Option<&str>) -> Result<String, String> {
        // Caller-supplied names are interpolated into DDL, so restrict them to
        // safe SQL identifiers before touching the transaction (defense in
        // depth: the extended query protocol already blocks multi-statement
        // injection). Validated once here, so rollback/release — which only
        // accept names already on the stack — are safe by construction.
        if let Some(n) = name {
            crate::db_schema_utils::validate_identifier(n)
                .map_err(|e| format!("Invalid savepoint name: {}", e))?;
        }

        CURRENT_TRANSACTION.with(|tx_cell| {
            let mut tx_option = tx_cell.borrow_mut();

            let state = tx_option.as_mut().ok_or("No active transaction")?;

            state.check_timeout()?;

            let savepoint_name = if let Some(n) = name {
                if state.savepoint_stack.contains(&n.to_string()) {
                    return Err(format!("Savepoint already exists: {}", n));
                }
                n.to_string()
            } else {
                state.savepoint_counter += 1;
                format!("sp_{}", state.savepoint_counter)
            };

            let tx_ref = state
                .transaction
                .as_mut()
                .ok_or("Transaction not available")?;

            run_blocking(async {
                sqlx::query(sqlx::AssertSqlSafe(format!("SAVEPOINT {}", savepoint_name)))
                    .execute(&mut **tx_ref)
                    .await
                    .map_err(|e| format!("Failed to create savepoint: {}", e))?;
                Ok::<(), String>(())
            })?;

            state.savepoint_stack.push(savepoint_name.clone());
            Ok(savepoint_name)
        })
    }

    /// Rollback to a named savepoint
    pub fn rollback_to_savepoint(name: &str) -> Result<(), String> {
        CURRENT_TRANSACTION.with(|tx_cell| {
            let mut tx_option = tx_cell.borrow_mut();

            let state = tx_option.as_mut().ok_or("No active transaction")?;

            state.check_timeout()?;

            if !state.savepoint_stack.contains(&name.to_string()) {
                return Err(format!("Savepoint not found: {}", name));
            }

            let tx_ref = state
                .transaction
                .as_mut()
                .ok_or("Transaction not available")?;

            run_blocking(async {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "ROLLBACK TO SAVEPOINT {}",
                    name
                )))
                .execute(&mut **tx_ref)
                .await
                .map_err(|e| format!("Failed to rollback to savepoint: {}", e))?;
                Ok::<(), String>(())
            })?;

            // Remove this savepoint and all after it from the stack
            if let Some(pos) = state.savepoint_stack.iter().position(|s| s == name) {
                state.savepoint_stack.truncate(pos);
            }

            Ok(())
        })
    }

    /// Release a named savepoint
    pub fn release_savepoint(name: &str) -> Result<(), String> {
        CURRENT_TRANSACTION.with(|tx_cell| {
            let mut tx_option = tx_cell.borrow_mut();

            let state = tx_option.as_mut().ok_or("No active transaction")?;

            state.check_timeout()?;

            if !state.savepoint_stack.contains(&name.to_string()) {
                return Err(format!("Savepoint not found: {}", name));
            }

            let tx_ref = state
                .transaction
                .as_mut()
                .ok_or("Transaction not available")?;

            run_blocking(async {
                sqlx::query(sqlx::AssertSqlSafe(format!("RELEASE SAVEPOINT {}", name)))
                    .execute(&mut **tx_ref)
                    .await
                    .map_err(|e| format!("Failed to release savepoint: {}", e))?;
                Ok::<(), String>(())
            })?;

            // Remove this savepoint and all after it from the stack
            if let Some(pos) = state.savepoint_stack.iter().position(|s| s == name) {
                state.savepoint_stack.truncate(pos);
            }

            Ok(())
        })
    }
}

/// Initialize database connection and optionally run migrations
pub async fn init_database(config: &RepositoryConfig, auto_migrate: bool) -> Result<Database> {
    let db = Database::new(config).await?;

    if auto_migrate {
        db.migrate().await?;
    } else {
        warn!("Auto-migration is disabled. Run migrations manually with: sqlx migrate run");
    }

    // Verify connection
    db.health_check()
        .await
        .context("Database health check failed after initialization")?;

    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository config pointed at the test database, or `None` when there
    /// is none to point at.
    fn test_repository_config() -> Option<RepositoryConfig> {
        std::env::var("DATABASE_URL")
            .ok()
            .map(|connection_string| RepositoryConfig {
                connection_string,
                ..RepositoryConfig::default()
            })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_call_gives_up_when_the_execution_budget_is_gone() {
        // No database needed. What is under test is that a host call which
        // would never return on its own returns anyway — the case the
        // interrupt handler cannot see, because control has left JavaScript.
        let outcome = tokio::task::spawn_blocking(|| {
            let _budget = bound_host_calls(Instant::now() + Duration::from_millis(200));
            run_bounded(std::future::pending::<crate::error::AppResult<()>>())
        })
        .await
        .expect("task panicked");

        assert!(
            matches!(outcome, Err(crate::error::AppError::JsTimeout { .. })),
            "a call that outlives the budget must fail, got {:?}",
            outcome.map(|_| ())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_host_call_outside_an_execution_is_not_bounded() {
        // Startup, the scheduler and the notification listener all reach the
        // repository with no script budget in scope. Bounding them by an
        // arbitrary default would be a behaviour change nobody asked for.
        let outcome = tokio::task::spawn_blocking(|| {
            assert!(remaining_host_budget().is_none());
            run_bounded(async { Ok::<_, crate::error::AppError>(7) })
        })
        .await
        .expect("task panicked");

        assert_eq!(outcome.expect("unbounded call"), 7);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_nested_execution_hands_the_outer_budget_back() {
        // One script dispatching to another nests two budgets on one thread.
        // Clearing instead of restoring would leave the outer script's host
        // calls unbounded for the rest of its run.
        tokio::task::spawn_blocking(|| {
            let outer_deadline = Instant::now() + Duration::from_secs(30);
            let _outer = bound_host_calls(outer_deadline);

            {
                let _inner = bound_host_calls(Instant::now() + Duration::from_millis(50));
                assert!(remaining_host_budget().expect("inner") < Duration::from_secs(1));
            }

            let restored = remaining_host_budget().expect("outer restored");
            assert!(
                restored > Duration::from_secs(20),
                "the outer budget should be back, got {:?}",
                restored
            );
        })
        .await
        .expect("task panicked");
    }

    #[test]
    fn a_budget_may_tighten_a_guard_but_never_loosen_one() {
        let configured = SessionGuards {
            lock_timeout_ms: 5_000,
            statement_timeout_ms: 30_000,
            idle_in_transaction_timeout_ms: 300_000,
        };

        // A budget shorter than the ceiling is what binds.
        let tightened = configured.tightened_to(1_000);
        assert_eq!(tightened.lock_timeout_ms, 1_000);
        assert_eq!(tightened.statement_timeout_ms, 1_000);
        assert_eq!(tightened.idle_in_transaction_timeout_ms, 1_000);

        // A budget longer than the ceiling is not: asking for ten minutes must
        // not be a way to buy ten minutes of lock waiting where the engine
        // allows five seconds.
        let loosened = configured.tightened_to(600_000);
        assert_eq!(loosened.lock_timeout_ms, 5_000);
        assert_eq!(loosened.statement_timeout_ms, 30_000);
        assert_eq!(loosened.idle_in_transaction_timeout_ms, 300_000);

        // A disabled guard is no ceiling at all, so the budget stands alone.
        assert_eq!(
            SessionGuards::none().tightened_to(1_000).lock_timeout_ms,
            1_000
        );

        // Zero would read to Postgres as "disabled" — the opposite of asking
        // for it — so it becomes the shortest budget expressible.
        assert_eq!(configured.tightened_to(0).lock_timeout_ms, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_transaction_budget_lasts_exactly_as_long_as_the_transaction() {
        let Some(config) = test_repository_config() else {
            eprintln!("Skipping transaction budget test - DATABASE_URL not set");
            return;
        };
        let Ok(db) = Database::new(&config).await else {
            eprintln!("Skipping transaction budget test - could not connect");
            return;
        };

        let mut tx = db.pool.begin().await.expect("begin");
        SessionGuards::from_config(&config)
            .tightened_to(400)
            .apply(&mut tx, GuardScope::Transaction)
            .await
            .expect("bound the transaction");

        let inside: String = sqlx::query_scalar("SHOW statement_timeout")
            .fetch_one(&mut *tx)
            .await
            .expect("read the setting inside");
        assert_eq!(inside, "400ms");

        tx.commit().await.expect("commit");

        // `SET LOCAL` and not `SET`: the connection goes back to the pool
        // carrying the session's guards, not one transaction's budget.
        let after: String = sqlx::query_scalar("SHOW statement_timeout")
            .fetch_one(&db.pool)
            .await
            .expect("read the setting after");
        assert_eq!(
            after,
            format!("{}s", config.statement_timeout_ms / 1_000),
            "a transaction's budget must not follow its connection into the pool"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn beginning_a_transaction_with_a_budget_hands_it_to_postgres() {
        let Some(config) = test_repository_config() else {
            eprintln!("Skipping begin_transaction budget test - DATABASE_URL not set");
            return;
        };
        let Ok(db) = Database::new(&config).await else {
            eprintln!("Skipping begin_transaction budget test - could not connect");
            return;
        };

        if !initialize_global_database(Arc::new(db)) {
            eprintln!("Skipping begin_transaction budget test - global already initialized");
            return;
        }

        // On the blocking thread the engine runs handlers on: the transaction
        // lives in thread-local storage, so the whole exchange has to stay on
        // one thread.
        let observed = tokio::task::spawn_blocking(|| {
            let guard = Database::begin_transaction(Some(750))?;

            let settings = CURRENT_TRANSACTION.with(|cell| {
                let mut state = cell.borrow_mut();
                let tx = state
                    .as_mut()
                    .and_then(|s| s.transaction.as_mut())
                    .ok_or("no transaction")?;

                run_blocking(async {
                    let mut read = Vec::new();
                    for name in [
                        "lock_timeout",
                        "statement_timeout",
                        "idle_in_transaction_session_timeout",
                    ] {
                        let value: String =
                            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SHOW {}", name)))
                                .fetch_one(&mut **tx)
                                .await
                                .map_err(|e| e.to_string())?;
                        read.push(value);
                    }
                    Ok::<_, String>(read)
                })
            })?;

            // Dropping the guard is the rollback: it is what a handler that
            // never reaches its own commit relies on.
            drop(guard);
            Ok::<_, String>(settings)
        })
        .await
        .expect("task panicked")
        .expect("budgeted transaction");

        // Every guard the engine has, bound to the budget the caller asked for
        // rather than merely recorded alongside it.
        assert_eq!(observed, vec!["750ms", "750ms", "750ms"]);
    }

    #[tokio::test]
    async fn test_database_connection() {
        // This test requires a running PostgreSQL instance
        // Skip if DATABASE_URL is not set
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping database test - DATABASE_URL not set");
                return;
            }
        };

        let config = RepositoryConfig {
            connection_string: database_url,
            ..RepositoryConfig::default()
        };

        // Try to connect with a short timeout to avoid hanging
        match tokio::time::timeout(std::time::Duration::from_secs(5), Database::new(&config)).await
        {
            Ok(Ok(db)) => {
                // Connection successful, now test health check
                match tokio::time::timeout(std::time::Duration::from_secs(5), db.health_check())
                    .await
                {
                    Ok(Ok(())) => {
                        // Test passed
                    }
                    Ok(Err(e)) => {
                        panic!("Health check failed: {}", e);
                    }
                    Err(_) => {
                        panic!("Health check timed out");
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!(
                    "Skipping database test - Failed to connect to database: {}",
                    e
                );
                eprintln!("Make sure PostgreSQL is running and DATABASE_URL is correct");
                return;
            }
            Err(_) => {
                eprintln!("Skipping database test - Database connection timed out");
                eprintln!("Make sure PostgreSQL is running and accessible");
                return;
            }
        }
    }

    #[test]
    fn test_transaction_state_creation() {
        // Test that transaction state is properly initialized
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping transaction test - DATABASE_URL not set");
                return;
            }
        };

        // Create a temporary runtime for this test
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let pool = PgPoolOptions::new()
                .max_connections(1)
                .connect(&database_url)
                .await;

            if pool.is_err() {
                eprintln!("Skipping transaction test - could not connect to database");
                return;
            }

            let pool = pool.unwrap();
            let tx = pool.begin().await.unwrap();
            let tx_static: Transaction<'static, Postgres> = unsafe { std::mem::transmute(tx) };

            let state = TransactionState::new(tx_static, Some(Duration::from_secs(10)));

            assert!(state.is_active());
            assert_eq!(state.savepoint_counter, 0);
            assert!(state.savepoint_stack.is_empty());
            assert!(!state.finalized);
        });
    }

    #[test]
    fn test_transaction_state_timeout_check() {
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping transaction test - DATABASE_URL not set");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let pool = PgPoolOptions::new()
                .max_connections(1)
                .connect(&database_url)
                .await;

            if pool.is_err() {
                eprintln!("Skipping transaction test - could not connect to database");
                return;
            }

            let pool = pool.unwrap();
            let tx = pool.begin().await.unwrap();
            let tx_static: Transaction<'static, Postgres> = unsafe { std::mem::transmute(tx) };

            // Create state with very short timeout
            let state = TransactionState::new(tx_static, Some(Duration::from_millis(1)));

            // Wait for timeout to expire
            std::thread::sleep(Duration::from_millis(10));

            // Check timeout should fail
            let result = state.check_timeout();
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("timeout"));
        });
    }

    #[test]
    fn test_transaction_not_active_initially() {
        // Verify that no transaction is active by default
        assert!(!get_current_transaction_active());
    }

    #[test]
    fn test_begin_transaction_no_database() {
        // Test that beginning transaction without initialized database returns error
        let result = Database::begin_transaction(None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Database not initialized"));
    }

    #[test]
    fn test_commit_transaction_without_begin() {
        // Test that committing without active transaction returns error
        let result = Database::commit_transaction();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No active transaction"));
    }

    #[test]
    fn test_rollback_transaction_without_begin() {
        // Test that rollback without active transaction returns error
        let result = Database::rollback_transaction();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No active transaction"));
    }

    #[test]
    fn test_create_savepoint_without_transaction() {
        // Test that creating savepoint without transaction returns error
        let result = Database::create_savepoint(None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No active transaction"));
    }

    #[test]
    fn test_create_savepoint_rejects_unsafe_name() {
        // A savepoint name is interpolated into DDL; injection attempts must be
        // rejected by identifier validation before any transaction work.
        for bad in ["x; DROP TABLE users", "a b", "1foo", "foo\"bar", ""] {
            let result = Database::create_savepoint(Some(bad));
            assert!(
                result
                    .as_ref()
                    .is_err_and(|e| e.contains("Invalid savepoint name")),
                "name {:?} should be rejected as invalid, got {:?}",
                bad,
                result
            );
        }
    }

    #[test]
    fn test_rollback_to_savepoint_without_transaction() {
        // Test that rollback to savepoint without transaction returns error
        let result = Database::rollback_to_savepoint("sp1");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No active transaction"));
    }

    #[test]
    fn test_release_savepoint_without_transaction() {
        // Test that releasing savepoint without transaction returns error
        let result = Database::release_savepoint("sp1");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No active transaction"));
    }

    #[test]
    fn test_transaction_guard_commit() {
        // Test that transaction guard can be marked as committed
        let mut guard = TransactionGuard::new();
        assert!(!guard.committed);

        guard.commit();
        assert!(guard.committed);
    }

    #[tokio::test]
    async fn test_full_transaction_lifecycle() {
        // Integration test for complete transaction lifecycle
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping transaction lifecycle test - DATABASE_URL not set");
                return;
            }
        };

        // Create database with larger pool for testing
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&database_url)
            .await;

        match pool {
            Ok(pool) => {
                let db = Database::from_pool(pool);
                let db_arc = Arc::new(db);

                // Try to initialize global database (may already be set by another test)
                let _ = initialize_global_database(db_arc.clone());

                // Wait a bit for the pool to be ready
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                // Run test in blocking context (simulating handler execution environment)
                let test_result = tokio::task::spawn_blocking(|| {
                    // Begin transaction
                    let guard_result = Database::begin_transaction(Some(5000));
                    if let Err(e) = &guard_result {
                        return Err(format!("Failed to begin transaction: {}", e));
                    }

                    // Verify transaction is active
                    if !get_current_transaction_active() {
                        return Err("Transaction not active after begin".to_string());
                    }

                    // Create a savepoint
                    let sp_result = Database::create_savepoint(Some("test_sp"));
                    if let Err(e) = sp_result {
                        return Err(format!("Failed to create savepoint: {}", e));
                    }
                    let sp_name = sp_result.unwrap();
                    if sp_name != "test_sp" {
                        return Err(format!("Unexpected savepoint name: {}", sp_name));
                    }

                    // Release the savepoint
                    let release_result = Database::release_savepoint("test_sp");
                    if let Err(e) = release_result {
                        return Err(format!("Failed to release savepoint: {}", e));
                    }

                    // Commit transaction
                    let commit_result = Database::commit_transaction();
                    if let Err(e) = commit_result {
                        return Err(format!("Failed to commit transaction: {}", e));
                    }

                    // Verify transaction is no longer active
                    if get_current_transaction_active() {
                        return Err("Transaction still active after commit".to_string());
                    }

                    Ok(())
                })
                .await;

                match test_result {
                    Ok(Ok(())) => {
                        eprintln!("✓ Transaction lifecycle test passed");
                    }
                    Ok(Err(e)) => {
                        panic!("Test failed: {}", e);
                    }
                    Err(e) => {
                        panic!("Task panicked: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "Skipping transaction lifecycle test - Failed to connect: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_transaction_rollback_lifecycle() {
        // Test rollback instead of commit
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping transaction rollback test - DATABASE_URL not set");
                return;
            }
        };

        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&database_url)
            .await;

        match pool {
            Ok(pool) => {
                let db = Database::from_pool(pool);
                let db_arc = Arc::new(db);
                if !initialize_global_database(db_arc.clone()) {
                    eprintln!("Could not initialize global database (may already be set)");
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                let test_result = tokio::task::spawn_blocking(|| {
                    let guard_result = Database::begin_transaction(Some(5000));
                    if let Err(e) = &guard_result {
                        return Err(format!("Failed to begin transaction: {}", e));
                    }

                    if !get_current_transaction_active() {
                        return Err("Transaction not active after begin".to_string());
                    }

                    let rollback_result = Database::rollback_transaction();
                    if let Err(e) = rollback_result {
                        return Err(format!("Failed to rollback transaction: {}", e));
                    }

                    if get_current_transaction_active() {
                        return Err("Transaction still active after rollback".to_string());
                    }

                    Ok(())
                })
                .await;

                match test_result {
                    Ok(Ok(())) => {
                        eprintln!("✓ Transaction rollback test passed");
                    }
                    Ok(Err(e)) => {
                        panic!("Test failed: {}", e);
                    }
                    Err(e) => {
                        panic!("Task panicked: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "Skipping transaction rollback test - Failed to connect: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_nested_savepoints() {
        // Test multiple savepoints in a transaction
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping nested savepoints test - DATABASE_URL not set");
                return;
            }
        };

        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&database_url)
            .await;

        match pool {
            Ok(pool) => {
                let db = Database::from_pool(pool);
                let db_arc = Arc::new(db);
                if !initialize_global_database(db_arc.clone()) {
                    eprintln!("Could not initialize global database (may already be set)");
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                let test_result = tokio::task::spawn_blocking(|| {
                    let _guard = match Database::begin_transaction(Some(5000)) {
                        Ok(g) => g,
                        Err(e) => return Err(format!("Failed to begin transaction: {}", e)),
                    };
                    if !get_current_transaction_active() {
                        return Err("Transaction not active".to_string());
                    }

                    let sp1_name = match Database::create_savepoint(None) {
                        Ok(name) => name,
                        Err(e) => return Err(format!("Failed to create savepoint 1: {}", e)),
                    };

                    let sp2_name = match Database::create_savepoint(None) {
                        Ok(name) => name,
                        Err(e) => return Err(format!("Failed to create savepoint 2: {}", e)),
                    };

                    if sp1_name == sp2_name {
                        return Err(format!("Savepoint names should be different: {}", sp1_name));
                    }

                    if let Err(e) = Database::rollback_to_savepoint(&sp1_name) {
                        return Err(format!("Failed to rollback to savepoint: {}", e));
                    }

                    if !get_current_transaction_active() {
                        return Err("Transaction not active after rollback".to_string());
                    }

                    if let Err(e) = Database::commit_transaction() {
                        return Err(format!("Failed to commit: {}", e));
                    }
                    if get_current_transaction_active() {
                        return Err("Transaction still active after commit".to_string());
                    }

                    Ok(())
                })
                .await;

                match test_result {
                    Ok(Ok(())) => {
                        eprintln!("✓ Nested savepoints test passed");
                    }
                    Ok(Err(e)) => {
                        panic!("Test failed: {}", e);
                    }
                    Err(e) => {
                        panic!("Task panicked: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Skipping nested savepoints test - Failed to connect: {}", e);
            }
        }
    }
}
