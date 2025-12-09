use crate::error::AppResult;
use crate::{graphql, repository, scheduler, script_init};
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgListener, PgPool};
use std::sync::{Arc, OnceLock};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Global notification listener instance
static GLOBAL_LISTENER: OnceLock<Arc<NotificationListener>> = OnceLock::new();

/// Initialize the global notification listener
pub fn initialize_global_listener(listener: Arc<NotificationListener>) -> bool {
    GLOBAL_LISTENER.set(listener).is_ok()
}

/// Get the global notification listener
pub fn get_global_listener() -> Option<Arc<NotificationListener>> {
    GLOBAL_LISTENER.get().cloned()
}

/// Generate a unique server ID for this instance
pub fn generate_server_id() -> String {
    Uuid::new_v4().to_string()
}

/// Message structure for script notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationMessage {
    pub uri: String,
    pub action: String, // "upserted" or "deleted"
    pub timestamp: i64,
    pub server_id: String,
}

/// Listener for PostgreSQL notifications about script changes
pub struct NotificationListener {
    server_id: String,
    pool: PgPool,
    shutdown_tx: Arc<RwLock<Option<tokio::sync::oneshot::Sender<()>>>>,
    task_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
}

impl NotificationListener {
    /// Create a new notification listener
    pub fn new(server_id: String, pool: PgPool) -> Self {
        Self {
            server_id,
            pool,
            shutdown_tx: Arc::new(RwLock::new(None)),
            task_handle: Arc::new(RwLock::new(None)),
        }
    }

    /// Start listening for notifications
    pub async fn start(&self) -> AppResult<()> {
        let server_id = self.server_id.clone();
        let pool = self.pool.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        info!(
            "Starting PostgreSQL notification listener with server_id={}",
            server_id
        );

        // Store shutdown sender
        *self.shutdown_tx.write().await = Some(shutdown_tx);

        // Spawn the listener task
        let handle = tokio::spawn(async move {
            if let Err(e) = Self::listen_loop(server_id, pool, shutdown_rx).await {
                error!("Notification listener error: {}", e);
            }
            info!("Notification listener stopped");
        });

        // Store task handle
        *self.task_handle.write().await = Some(handle);

        Ok(())
    }

    /// Main listen loop
    async fn listen_loop(
        server_id: String,
        pool: PgPool,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> AppResult<()> {
        // Create PgListener from the pool
        let mut listener = PgListener::connect_with(&pool).await.map_err(|e| {
            crate::error::AppError::Database {
                message: format!("Failed to create PgListener: {}", e),
                source: None,
            }
        })?;

        // Listen to both channels
        listener
            .listen("script_upserted")
            .await
            .map_err(|e| crate::error::AppError::Database {
                message: format!("Failed to listen on script_upserted: {}", e),
                source: None,
            })?;

        listener
            .listen("script_deleted")
            .await
            .map_err(|e| crate::error::AppError::Database {
                message: format!("Failed to listen on script_deleted: {}", e),
                source: None,
            })?;

        info!("Listening on PostgreSQL channels: script_upserted, script_deleted");

        loop {
            tokio::select! {
                // Handle shutdown signal
                _ = &mut shutdown_rx => {
                    info!("Received shutdown signal for notification listener");
                    break;
                }

                // Handle notifications
                notification = listener.recv() => {
                    match notification {
                        Ok(notification) => {
                            debug!("Received notification on channel: {}", notification.channel());

                            // Parse the JSON payload
                            match serde_json::from_str::<NotificationMessage>(notification.payload()) {
                                Ok(msg) => {
                                    // Ignore notifications from this server
                                    if msg.server_id == server_id {
                                        debug!("Ignoring own notification for {}", msg.uri);
                                        continue;
                                    }

                                    info!(
                                        "Processing {} notification for script '{}' from server {}",
                                        msg.action, msg.uri, msg.server_id
                                    );

                                    // Handle based on action
                                    match msg.action.as_str() {
                                        "upserted" => {
                                            if let Err(e) = Self::handle_script_upserted(&msg.uri).await {
                                                error!("Failed to handle script upserted: {}", e);
                                            }
                                        }
                                        "deleted" => {
                                            if let Err(e) = Self::handle_script_deleted(&msg.uri).await {
                                                error!("Failed to handle script deleted: {}", e);
                                            }
                                        }
                                        _ => {
                                            warn!("Unknown notification action: {}", msg.action);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to parse notification payload: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Error receiving notification: {}", e);
                            // PgListener automatically reconnects on connection errors
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle script upserted notification
    async fn handle_script_upserted(uri: &str) -> AppResult<()> {
        info!("Handling script upserted for: {}", uri);

        // Initialize the script (this will call init() and register routes/GraphQL)
        let initializer = script_init::ScriptInitializer::new(10_000); // 10 second timeout
        match initializer.initialize_script(uri, false).await {
            Ok(result) => {
                if result.success {
                    info!(
                        "✓ Script '{}' reinitialized after remote upsert in {}ms",
                        uri, result.duration_ms
                    );
                } else if let Some(err) = result.error {
                    error!(
                        "✗ Script '{}' reinitialize failed after remote upsert: {}",
                        uri, err
                    );
                }
            }
            Err(e) => {
                error!("Failed to initialize script '{}': {}", uri, e);
            }
        }

        // Rebuild GraphQL schema to include any new registrations
        if let Err(e) = graphql::rebuild_schema() {
            error!(
                "Failed to rebuild GraphQL schema after script '{}' upsert: {:?}",
                uri, e
            );
        } else {
            debug!(
                "GraphQL schema rebuilt successfully after script '{}' upsert",
                uri
            );
        }

        Ok(())
    }

    /// Handle script deleted notification
    async fn handle_script_deleted(uri: &str) -> AppResult<()> {
        info!("Handling script deleted for: {}", uri);

        // Clear any scheduled jobs for this script
        scheduler::clear_script_jobs(uri);
        debug!("Cleared scheduled jobs for script '{}'", uri);

        // Clear GraphQL registrations for this script
        graphql::clear_script_graphql_registrations(uri);
        debug!("Cleared GraphQL registrations for script '{}'", uri);

        // Rebuild GraphQL schema
        if let Err(e) = graphql::rebuild_schema() {
            error!(
                "Failed to rebuild GraphQL schema after script '{}' deletion: {:?}",
                uri, e
            );
        } else {
            debug!(
                "GraphQL schema rebuilt successfully after script '{}' deletion",
                uri
            );
        }

        // Invalidate cache in repository
        if let Ok(mut guard) = repository::safe_lock_scripts() {
            guard.remove(uri);
            debug!("Removed script '{}' from cache", uri);
        }

        info!("✓ Script '{}' cleanup completed after remote deletion", uri);

        Ok(())
    }

    /// Stop the listener
    pub async fn stop(&self) -> AppResult<()> {
        info!("Stopping notification listener...");

        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.write().await.take() {
            let _ = tx.send(());
        }

        // Wait for task to complete
        if let Some(handle) = self.task_handle.write().await.take()
            && let Err(e) = handle.await
        {
            warn!("Error waiting for notification listener task: {}", e);
        }

        info!("Notification listener stopped successfully");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_server_id() {
        let id1 = generate_server_id();
        let id2 = generate_server_id();

        // Should generate different IDs
        assert_ne!(id1, id2);

        // Should be valid UUIDs
        assert!(Uuid::parse_str(&id1).is_ok());
        assert!(Uuid::parse_str(&id2).is_ok());
    }

    #[test]
    fn test_notification_message_serialization() {
        let msg = NotificationMessage {
            uri: "https://example.com/test".to_string(),
            action: "upserted".to_string(),
            timestamp: 1234567890,
            server_id: "test-server-id".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: NotificationMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.uri, msg.uri);
        assert_eq!(deserialized.action, msg.action);
        assert_eq!(deserialized.timestamp, msg.timestamp);
        assert_eq!(deserialized.server_id, msg.server_id);
    }
}
