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

/// Global server ID for this instance
static GLOBAL_SERVER_ID: OnceLock<String> = OnceLock::new();

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

/// Initialize the global server ID (should be called once at startup)
pub fn initialize_server_id(server_id: String) -> bool {
    GLOBAL_SERVER_ID.set(server_id).is_ok()
}

/// Get the global server ID
pub fn get_server_id() -> Option<String> {
    GLOBAL_SERVER_ID.get().cloned()
}

/// Message structure for script notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationMessage {
    pub uri: String,
    pub action: String, // "upserted" or "deleted"
    pub timestamp: i64,
    pub server_id: String,
}

/// Message structure for stream broadcast notifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamBroadcastMessage {
    pub stream_path: String,
    pub message: String,
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

        listener.listen("stream_broadcast").await.map_err(|e| {
            crate::error::AppError::Database {
                message: format!("Failed to listen on stream_broadcast: {}", e),
                source: None,
            }
        })?;

        info!(
            "Listening on PostgreSQL channels: script_upserted, script_deleted, stream_broadcast"
        );

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
                            let channel = notification.channel();
                            debug!("Received notification on channel: {}", channel);

                            match channel {
                                "script_upserted" | "script_deleted" => {
                                    // Parse as script notification
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
                                            error!("Failed to parse script notification payload: {}", e);
                                        }
                                    }
                                }
                                "stream_broadcast" => {
                                    // Parse as stream broadcast notification
                                    match serde_json::from_str::<StreamBroadcastMessage>(notification.payload()) {
                                        Ok(msg) => {
                                            // Ignore notifications from this server
                                            if msg.server_id == server_id {
                                                debug!("Ignoring own stream broadcast for {}", msg.stream_path);
                                                continue;
                                            }

                                            debug!(
                                                "Processing stream broadcast for path '{}' from server {}",
                                                msg.stream_path, msg.server_id
                                            );

                                            // Handle stream broadcast
                                            if let Err(e) = Self::handle_stream_broadcast(&msg).await {
                                                error!("Failed to handle stream broadcast: {}", e);
                                            }
                                        }
                                        Err(e) => {
                                            error!("Failed to parse stream broadcast payload: {}", e);
                                        }
                                    }
                                }
                                _ => {
                                    warn!("Unknown notification channel: {}", channel);
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

    /// Handle stream broadcast notification from another instance
    async fn handle_stream_broadcast(msg: &StreamBroadcastMessage) -> AppResult<()> {
        use crate::stream_registry;

        debug!(
            "Broadcasting message to local connections on path '{}' from remote instance {}",
            msg.stream_path, msg.server_id
        );

        // Get the global stream registry
        let registry = stream_registry::get_global_registry();

        // Broadcast to local connections on this path
        match registry.broadcast_to_stream(&msg.stream_path, &msg.message) {
            Ok(result) => {
                debug!(
                    "Broadcast to {} local connections on '{}' (from remote): {} successful, {} failed",
                    result.total_connections,
                    msg.stream_path,
                    result.successful_sends,
                    result.failed_connections.len()
                );
            }
            Err(e) => {
                warn!(
                    "Failed to broadcast to local connections on '{}': {}",
                    msg.stream_path, e
                );
            }
        }

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
