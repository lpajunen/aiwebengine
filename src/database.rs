use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tracing::{info, warn};

use crate::config::RepositoryConfig;

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
    /// Create a new database connection pool
    pub async fn new(config: &RepositoryConfig) -> Result<Self> {
        let connection_string = config
            .connection_string
            .as_ref()
            .context("Database connection string is required")?;

        info!("Connecting to database...");

        let pool = PgPoolOptions::new()
            .max_connections(5) // Default pool size
            .acquire_timeout(Duration::from_millis(2000))
            .connect(connection_string)
            .await
            .context("Failed to connect to database")?;

        info!("Database connection established");

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
                    }).to_string();
                }
            };
            
            match rt.block_on(db.health_check()) {
                Ok(()) => {
                    serde_json::json!({
                        "healthy": true,
                        "database": "ok"
                    }).to_string()
                }
                Err(e) => {
                    serde_json::json!({
                        "healthy": false,
                        "error": format!("Database health check failed: {}", e)
                    }).to_string()
                }
            }
        } else {
            serde_json::json!({
                "healthy": false,
                "error": "Database not initialized"
            }).to_string()
        }
    }

    /// Gracefully close the database connection pool
    pub async fn close(self) {
        info!("Closing database connection pool...");
        self.pool.close().await;
        info!("Database connection pool closed");
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
        if std::env::var("DATABASE_URL").is_err() {
            eprintln!("Skipping database test - DATABASE_URL not set");
            return;
        }

        let config = RepositoryConfig {
            storage_type: "postgresql".to_string(),
            connection_string: std::env::var("DATABASE_URL").ok(),
            max_script_size_bytes: 1024 * 1024,
            max_asset_size_bytes: 10 * 1024 * 1024,
            max_log_messages_per_script: 100,
            log_retention_hours: 24,
            auto_prune_logs: true,
        };

        let db = Database::new(&config).await.expect("Failed to connect");
        db.health_check()
            .await
            .expect("Health check failed");
    }
}
