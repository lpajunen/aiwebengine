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
        if let Some(deadline) = self.deadline {
            if Instant::now() > deadline {
                return Err("Transaction timeout exceeded".to_string());
            }
        }
        Ok(())
    }

    fn is_active(&self) -> bool {
        self.transaction.is_some() && !self.finalized
    }
}

// Thread-local storage for the current transaction
thread_local! {
    static CURRENT_TRANSACTION: RefCell<Option<TransactionState>> = RefCell::new(None);
}

/// Get the current transaction state (if any)
pub fn get_current_transaction_active() -> bool {
    CURRENT_TRANSACTION.with(|tx| tx.borrow().as_ref().map_or(false, |t| t.is_active()))
}

/// RAII guard for automatic transaction rollback on drop
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
            .acquire_timeout(Duration::from_millis(2000))
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
}
