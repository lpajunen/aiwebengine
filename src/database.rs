use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Transaction};
use std::cell::RefCell;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::config::RepositoryConfig;

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

/// Database connection pool manager
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Create a new database instance from an existing pool (useful for testing)
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new database connection pool
    pub async fn new(config: &RepositoryConfig) -> Result<Self> {
        let connection_string = config
            .connection_string
            .as_ref()
            .context("Database connection string is required")?;

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

        let pool = PgPoolOptions::new()
            .max_connections(5) // Default pool size
            .acquire_timeout(Duration::from_millis(5000)) // Increased for tests
            .connect(connection_string)
            .await
            .context("Failed to connect to database")?;

        info!("✓ Database connection established successfully");
        info!("✓ Connection pool created with max 5 connections");

        Ok(Self { pool })
    }

    /// Run database migrations
    pub async fn migrate(&self) -> Result<()> {
        info!("Running database migrations...");

        sqlx::migrate!("./migrations")
            .run(&self.pool)
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

    /// Synchronous health check wrapper for use from JavaScript
    /// Returns a JSON string with the health status
    pub fn check_health_sync() -> String {
        if let Some(db) = get_global_database() {
            // Create a new Tokio runtime for this synchronous call
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    return serde_json::json!({
                        "healthy": false,
                        "error": format!("Failed to create runtime: {}", e)
                    })
                    .to_string();
                }
            };

            match rt.block_on(db.health_check()) {
                Ok(()) => serde_json::json!({
                    "healthy": true,
                    "database": "ok"
                })
                .to_string(),
                Err(e) => serde_json::json!({
                    "healthy": false,
                    "error": format!("Database health check failed: {}", e)
                })
                .to_string(),
            }
        } else {
            // Database not initialized - this is acceptable when using in-memory storage
            // Return healthy with a note that database is not configured
            serde_json::json!({
                "healthy": true,
                "database": "not configured (using in-memory storage)",
                "note": "Database connection not required for in-memory storage mode"
            })
            .to_string()
        }
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
    pub fn begin_transaction(timeout_ms: Option<u64>) -> Result<TransactionGuard, String> {
        // Helper to run async code in blocking context
        fn run_blocking<F, R>(future: F) -> R
        where
            F: std::future::Future<Output = R>,
        {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => tokio::task::block_in_place(move || handle.block_on(future)),
                Err(_) => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create temporary runtime")
                    .block_on(future),
            }
        }

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
                    sqlx::query(&format!("SAVEPOINT {}", savepoint_name))
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

                let tx = run_blocking(async {
                    pool.begin()
                        .await
                        .map_err(|e| format!("Failed to begin transaction: {}", e))
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
        fn run_blocking<F, R>(future: F) -> R
        where
            F: std::future::Future<Output = R>,
        {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => tokio::task::block_in_place(move || handle.block_on(future)),
                Err(_) => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create temporary runtime")
                    .block_on(future),
            }
        }

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
                    sqlx::query(&format!("RELEASE SAVEPOINT {}", savepoint_name))
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
        fn run_blocking<F, R>(future: F) -> R
        where
            F: std::future::Future<Output = R>,
        {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => tokio::task::block_in_place(move || handle.block_on(future)),
                Err(_) => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create temporary runtime")
                    .block_on(future),
            }
        }

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
                    sqlx::query(&format!("ROLLBACK TO SAVEPOINT {}", savepoint_name))
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
        fn run_blocking<F, R>(future: F) -> R
        where
            F: std::future::Future<Output = R>,
        {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => tokio::task::block_in_place(move || handle.block_on(future)),
                Err(_) => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create temporary runtime")
                    .block_on(future),
            }
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
                sqlx::query(&format!("SAVEPOINT {}", savepoint_name))
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
        fn run_blocking<F, R>(future: F) -> R
        where
            F: std::future::Future<Output = R>,
        {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => tokio::task::block_in_place(move || handle.block_on(future)),
                Err(_) => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create temporary runtime")
                    .block_on(future),
            }
        }

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
                sqlx::query(&format!("ROLLBACK TO SAVEPOINT {}", name))
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
        fn run_blocking<F, R>(future: F) -> R
        where
            F: std::future::Future<Output = R>,
        {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => tokio::task::block_in_place(move || handle.block_on(future)),
                Err(_) => tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create temporary runtime")
                    .block_on(future),
            }
        }

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
                sqlx::query(&format!("RELEASE SAVEPOINT {}", name))
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
            storage_type: "postgresql".to_string(),
            connection_string: Some(database_url),
            max_script_size_bytes: 1024 * 1024,
            max_asset_size_bytes: 10 * 1024 * 1024,
            max_log_messages_per_script: 100,
            log_retention_hours: 24,
            auto_prune_logs: true,
            max_upload_size_bytes: 10 * 1024 * 1024,
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
