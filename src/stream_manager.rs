use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::stream_registry::{GLOBAL_STREAM_REGISTRY, StreamConnection};

/// Represents an active SSE connection with its management data
#[derive(Debug)]
pub struct ActiveConnection {
    /// Unique connection identifier
    pub connection_id: String,
    /// The stream path this connection is subscribed to
    pub stream_path: String,
    /// Timestamp when connection was established
    pub connected_at: u64,
    /// Broadcast receiver for messages
    pub receiver: broadcast::Receiver<String>,
    /// Optional client metadata
    pub client_metadata: Option<HashMap<String, String>>,
    /// Connection health status
    pub is_healthy: bool,
    /// Last ping/pong timestamp
    pub last_ping: u64,
}

impl ActiveConnection {
    /// Create a new active connection
    pub fn new(
        stream_path: String,
        receiver: broadcast::Receiver<String>,
        client_metadata: Option<HashMap<String, String>>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            connection_id: Uuid::new_v4().to_string(),
            stream_path,
            connected_at: now,
            receiver,
            client_metadata,
            is_healthy: true,
            last_ping: now,
        }
    }

    /// Update the last ping timestamp and health status
    pub fn update_ping(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.last_ping = now;
        self.is_healthy = true;
    }

    /// Get the age of this connection in seconds
    pub fn age_seconds(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        now.saturating_sub(self.connected_at)
    }

    /// Check if this connection has been idle for too long
    pub fn is_stale(&self, max_idle_seconds: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        now.saturating_sub(self.last_ping) > max_idle_seconds
    }
}

/// Configuration for stream connection management
#[derive(Debug, Clone)]
pub struct ConnectionManagerConfig {
    /// Maximum number of connections per stream path
    pub max_connections_per_stream: usize,
    /// Maximum total connections across all streams
    pub max_total_connections: usize,
    /// Connection idle timeout in seconds
    pub connection_idle_timeout: u64,
    /// Cleanup interval for stale connections in seconds
    pub cleanup_interval: u64,
}

impl Default for ConnectionManagerConfig {
    fn default() -> Self {
        Self {
            max_connections_per_stream: 100,
            max_total_connections: 1000,
            connection_idle_timeout: 300, // 5 minutes
            cleanup_interval: 60,         // 1 minute
        }
    }
}

/// Statistics about active connections
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    /// Total number of active connections
    pub total_connections: usize,
    /// Connections per stream path
    pub connections_per_stream: HashMap<String, usize>,
    /// Average connection age in seconds
    pub average_age_seconds: f64,
    /// Number of healthy connections
    pub healthy_connections: usize,
    /// Number of stale connections
    pub stale_connections: usize,
}

/// Manages active stream connections with health tracking and cleanup
///
/// This simplified version stores connection metadata separately from receivers
/// to avoid the Clone trait issues with broadcast::Receiver
pub struct StreamConnectionManager {
    /// Configuration for connection management
    config: ConnectionManagerConfig,
    /// Connection metadata by connection ID
    connection_metadata: Arc<Mutex<HashMap<String, ConnectionMetadata>>>,
    /// Connection IDs grouped by stream path
    connections_by_stream: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Handle for the cleanup task
    cleanup_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Connection metadata that can be cloned (without the receiver)
#[derive(Debug, Clone)]
pub struct ConnectionMetadata {
    pub connection_id: String,
    pub stream_path: String,
    pub connected_at: u64,
    pub client_metadata: Option<HashMap<String, String>>,
    pub is_healthy: bool,
    pub last_ping: u64,
}

impl StreamConnectionManager {
    /// Create a new connection manager with default configuration
    pub fn new() -> Self {
        Self::with_config(ConnectionManagerConfig::default())
    }

    /// Create a new connection manager with custom configuration
    pub fn with_config(config: ConnectionManagerConfig) -> Self {
        Self {
            config,
            connection_metadata: Arc::new(Mutex::new(HashMap::new())),
            connections_by_stream: Arc::new(Mutex::new(HashMap::new())),
            cleanup_handle: None,
        }
    }

    /// Start the connection manager with automatic cleanup
    pub async fn start(&mut self) {
        let connection_metadata = Arc::clone(&self.connection_metadata);
        let connections_by_stream = Arc::clone(&self.connections_by_stream);
        let cleanup_interval = self.config.cleanup_interval;
        let max_idle = self.config.connection_idle_timeout;

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(cleanup_interval));

            loop {
                interval.tick().await;

                if let Err(e) = Self::cleanup_stale_connections_internal(
                    &connection_metadata,
                    &connections_by_stream,
                    max_idle,
                )
                .await
                {
                    error!("Failed to cleanup stale connections: {}", e);
                }
            }
        });

        self.cleanup_handle = Some(handle);
        info!(
            "Stream connection manager started with cleanup interval: {}s",
            cleanup_interval
        );
    }

    /// Stop the connection manager
    pub async fn stop(&mut self) {
        if let Some(handle) = self.cleanup_handle.take() {
            handle.abort();
            info!("Stream connection manager stopped");
        }
    }

    /// Create a new stream connection for a client
    pub async fn create_connection(
        &self,
        stream_path: &str,
        client_metadata: Option<HashMap<String, String>>,
    ) -> Result<ActiveConnection, String> {
        // Check if the stream path is registered
        if !GLOBAL_STREAM_REGISTRY.is_stream_registered(stream_path) {
            return Err(format!("Stream path '{}' is not registered", stream_path));
        }

        // Check connection limits
        if let Err(e) = self.check_connection_limits(stream_path).await {
            return Err(e);
        }

        // Create a stream connection in the registry
        let stream_connection = if let Some(metadata) = &client_metadata {
            StreamConnection::with_metadata(metadata.clone())
        } else {
            StreamConnection::new()
        };

        let receiver = stream_connection.subscribe();

        // Add the connection to the registry
        match GLOBAL_STREAM_REGISTRY.add_connection(stream_path, stream_connection) {
            Ok(_) => {
                let active_conn =
                    ActiveConnection::new(stream_path.to_string(), receiver, client_metadata);

                // Track the connection in our manager
                self.track_connection(&active_conn).await?;

                info!(
                    "Created stream connection {} for path '{}' (total connections: {})",
                    active_conn.connection_id,
                    stream_path,
                    self.get_total_connections().await
                );

                Ok(active_conn)
            }
            Err(e) => Err(format!(
                "Failed to add connection to stream registry: {}",
                e
            )),
        }
    }

    /// Remove a connection
    pub async fn remove_connection(&self, connection_id: &str) -> Result<bool, String> {
        let stream_path = {
            match self.connection_metadata.lock() {
                Ok(mut metadata) => {
                    if let Some(conn_meta) = metadata.remove(connection_id) {
                        Some(conn_meta.stream_path)
                    } else {
                        None
                    }
                }
                Err(e) => {
                    error!("Failed to acquire metadata lock: {}", e);
                    return Err("Failed to remove connection: lock error".to_string());
                }
            }
        };

        if let Some(path) = stream_path {
            // Remove from registry
            let _ = GLOBAL_STREAM_REGISTRY.remove_connection(&path, connection_id);

            // Update connections by stream
            if let Ok(mut conn_by_stream) = self.connections_by_stream.lock() {
                if let Some(conn_ids) = conn_by_stream.get_mut(&path) {
                    conn_ids.retain(|id| id != connection_id);
                    if conn_ids.is_empty() {
                        conn_by_stream.remove(&path);
                    }
                }
            }

            info!(
                "Removed connection {} from stream '{}'",
                connection_id, path
            );
            Ok(true)
        } else {
            debug!("Connection {} not found for removal", connection_id);
            Ok(false)
        }
    }

    /// Get connection information by ID
    pub async fn get_connection_info(&self, connection_id: &str) -> Option<ConnectionMetadata> {
        match self.connection_metadata.lock() {
            Ok(metadata) => metadata.get(connection_id).cloned(),
            Err(_) => None,
        }
    }

    /// Get all connection info for a specific stream
    pub async fn get_connections_for_stream(&self, stream_path: &str) -> Vec<ConnectionMetadata> {
        let mut connections = Vec::new();

        if let Ok(metadata) = self.connection_metadata.lock() {
            for conn_meta in metadata.values() {
                if conn_meta.stream_path == stream_path {
                    connections.push(conn_meta.clone());
                }
            }
        }

        connections
    }

    /// Update connection ping/health status
    pub async fn update_connection_ping(&self, connection_id: &str) -> Result<(), String> {
        match self.connection_metadata.lock() {
            Ok(mut metadata) => {
                if let Some(conn_meta) = metadata.get_mut(connection_id) {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    conn_meta.last_ping = now;
                    conn_meta.is_healthy = true;
                    Ok(())
                } else {
                    Err(format!("Connection {} not found", connection_id))
                }
            }
            Err(e) => {
                error!("Failed to acquire metadata lock: {}", e);
                Err("Failed to update ping: lock error".to_string())
            }
        }
    }

    /// Get connection statistics
    pub async fn get_stats(&self) -> ConnectionStats {
        let mut stats = ConnectionStats {
            total_connections: 0,
            connections_per_stream: HashMap::new(),
            average_age_seconds: 0.0,
            healthy_connections: 0,
            stale_connections: 0,
        };

        let stale_threshold = self.config.connection_idle_timeout;

        match (
            self.connection_metadata.lock(),
            self.connections_by_stream.lock(),
        ) {
            (Ok(metadata), Ok(conn_by_stream)) => {
                stats.total_connections = metadata.len();

                // Count connections per stream
                for (stream_path, conn_ids) in conn_by_stream.iter() {
                    stats
                        .connections_per_stream
                        .insert(stream_path.clone(), conn_ids.len());
                }

                // Calculate age and health statistics
                if !metadata.is_empty() {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let mut total_age = 0u64;
                    for conn_meta in metadata.values() {
                        let age = now.saturating_sub(conn_meta.connected_at);
                        total_age += age;

                        let is_stale = now.saturating_sub(conn_meta.last_ping) > stale_threshold;

                        if conn_meta.is_healthy && !is_stale {
                            stats.healthy_connections += 1;
                        } else {
                            stats.stale_connections += 1;
                        }
                    }

                    stats.average_age_seconds = total_age as f64 / metadata.len() as f64;
                }
            }
            (Err(e), _) => {
                error!(
                    "Failed to acquire metadata lock for stats calculation: {}",
                    e
                );
            }
            (_, Err(e)) => {
                error!(
                    "Failed to acquire connections by stream lock for stats calculation: {}",
                    e
                );
            }
        }

        stats
    }

    /// Clean up stale connections
    pub async fn cleanup_stale_connections(&self) -> Result<usize, String> {
        Self::cleanup_stale_connections_internal(
            &self.connection_metadata,
            &self.connections_by_stream,
            self.config.connection_idle_timeout,
        )
        .await
    }

    /// Internal cleanup function (static to avoid borrowing issues)
    async fn cleanup_stale_connections_internal(
        connection_metadata: &Arc<Mutex<HashMap<String, ConnectionMetadata>>>,
        connections_by_stream: &Arc<Mutex<HashMap<String, Vec<String>>>>,
        max_idle_seconds: u64,
    ) -> Result<usize, String> {
        let mut stale_connection_ids = Vec::new();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Find stale connections
        {
            match connection_metadata.lock() {
                Ok(metadata) => {
                    for (id, conn_meta) in metadata.iter() {
                        let idle_time = now.saturating_sub(conn_meta.last_ping);
                        if idle_time > max_idle_seconds {
                            stale_connection_ids.push((id.clone(), conn_meta.stream_path.clone()));
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to acquire metadata lock: {}", e);
                    return Err("Cleanup failed: lock error".to_string());
                }
            }
        }

        let count = stale_connection_ids.len();
        if count > 0 {
            info!("Cleaning up {} stale connections", count);

            // Remove stale connections
            if let Ok(mut metadata) = connection_metadata.lock() {
                for (conn_id, stream_path) in &stale_connection_ids {
                    if metadata.remove(conn_id).is_some() {
                        // Also remove from registry
                        let _ = GLOBAL_STREAM_REGISTRY.remove_connection(stream_path, conn_id);
                        debug!(
                            "Removed stale connection {} from stream '{}'",
                            conn_id, stream_path
                        );
                    }
                }
            }

            // Update connections by stream
            if let Ok(mut conn_by_stream) = connections_by_stream.lock() {
                for (conn_id, stream_path) in &stale_connection_ids {
                    if let Some(conn_ids) = conn_by_stream.get_mut(stream_path) {
                        conn_ids.retain(|id| id != conn_id);
                        if conn_ids.is_empty() {
                            conn_by_stream.remove(stream_path);
                        }
                    }
                }
            }
        }

        Ok(count)
    }

    /// Check connection limits before creating a new connection
    async fn check_connection_limits(&self, stream_path: &str) -> Result<(), String> {
        match (
            self.connection_metadata.lock(),
            self.connections_by_stream.lock(),
        ) {
            (Ok(metadata), Ok(conn_by_stream)) => {
                // Check total connections limit
                if metadata.len() >= self.config.max_total_connections {
                    return Err(format!(
                        "Maximum total connections reached ({})",
                        self.config.max_total_connections
                    ));
                }

                // Check per-stream limit
                if let Some(stream_connections) = conn_by_stream.get(stream_path) {
                    if stream_connections.len() >= self.config.max_connections_per_stream {
                        return Err(format!(
                            "Maximum connections per stream reached for '{}' ({})",
                            stream_path, self.config.max_connections_per_stream
                        ));
                    }
                }

                Ok(())
            }
            (Err(e), _) => {
                error!("Failed to acquire metadata lock for limit check: {}", e);
                Err("Limit check failed: metadata lock error".to_string())
            }
            (_, Err(e)) => {
                error!(
                    "Failed to acquire connections by stream lock for limit check: {}",
                    e
                );
                Err("Limit check failed: connections by stream lock error".to_string())
            }
        }
    }

    /// Track a new connection in our internal structures
    async fn track_connection(&self, connection: &ActiveConnection) -> Result<(), String> {
        let conn_id = connection.connection_id.clone();
        let stream_path = connection.stream_path.clone();

        // Create metadata from the active connection
        let metadata = ConnectionMetadata {
            connection_id: conn_id.clone(),
            stream_path: stream_path.clone(),
            connected_at: connection.connected_at,
            client_metadata: connection.client_metadata.clone(),
            is_healthy: connection.is_healthy,
            last_ping: connection.last_ping,
        };

        // Add to connection metadata
        match self.connection_metadata.lock() {
            Ok(mut meta) => {
                meta.insert(conn_id.clone(), metadata);
            }
            Err(e) => {
                error!("Failed to acquire metadata lock: {}", e);
                return Err("Failed to track connection metadata: lock error".to_string());
            }
        }

        // Add to connections by stream
        match self.connections_by_stream.lock() {
            Ok(mut conn_by_stream) => {
                conn_by_stream
                    .entry(stream_path)
                    .or_insert_with(Vec::new)
                    .push(conn_id);
            }
            Err(e) => {
                error!("Failed to acquire connections by stream lock: {}", e);
                return Err("Failed to track connection by stream: lock error".to_string());
            }
        }

        Ok(())
    }

    /// Get total number of active connections
    async fn get_total_connections(&self) -> usize {
        match self.connection_metadata.lock() {
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_connection_manager_creation() {
        let manager = StreamConnectionManager::new();
        let stats = manager.get_stats().await;

        assert_eq!(stats.total_connections, 0);
        assert!(stats.connections_per_stream.is_empty());
    }

    #[tokio::test]
    async fn test_connection_manager_with_config() {
        let config = ConnectionManagerConfig {
            max_connections_per_stream: 50,
            max_total_connections: 500,
            connection_idle_timeout: 120,
            cleanup_interval: 30,
        };

        let manager = StreamConnectionManager::with_config(config.clone());
        assert_eq!(manager.config.max_connections_per_stream, 50);
        assert_eq!(manager.config.max_total_connections, 500);
    }

    #[tokio::test]
    async fn test_connection_limits_check() {
        let config = ConnectionManagerConfig {
            max_connections_per_stream: 2,
            max_total_connections: 5,
            connection_idle_timeout: 300,
            cleanup_interval: 60,
        };

        let manager = StreamConnectionManager::with_config(config);

        // This should pass with no connections
        assert!(manager.check_connection_limits("/test").await.is_ok());
    }

    #[tokio::test]
    async fn test_connection_stats() {
        let manager = StreamConnectionManager::new();
        let stats = manager.get_stats().await;

        assert_eq!(stats.total_connections, 0);
        assert_eq!(stats.healthy_connections, 0);
        assert_eq!(stats.stale_connections, 0);
        assert_eq!(stats.average_age_seconds, 0.0);
    }

    #[tokio::test]
    async fn test_active_connection_creation() {
        let (_tx, rx) = broadcast::channel(32);
        let metadata = Some([("client_id".to_string(), "test123".to_string())].into());

        let conn = ActiveConnection::new("/test".to_string(), rx, metadata.clone());

        assert_eq!(conn.stream_path, "/test");
        assert_eq!(conn.client_metadata, metadata);
        assert!(conn.is_healthy);
        assert!(!conn.connection_id.is_empty());
    }

    #[tokio::test]
    async fn test_connection_ping_update() {
        let (_tx, rx) = broadcast::channel(32);
        let mut conn = ActiveConnection::new("/test".to_string(), rx, None);

        let initial_ping = conn.last_ping;
        sleep(Duration::from_millis(10)).await;

        conn.update_ping();
        assert!(conn.last_ping >= initial_ping);
        assert!(conn.is_healthy);
    }

    #[tokio::test]
    async fn test_connection_staleness() {
        let (_tx, rx) = broadcast::channel(32);
        let mut conn = ActiveConnection::new("/test".to_string(), rx, None);

        // Connection should not be stale immediately
        assert!(!conn.is_stale(300));

        // Manually set last_ping to an old time to test staleness
        conn.last_ping = 0; // Very old timestamp

        // Connection should be stale with current threshold
        assert!(conn.is_stale(1));
    }

    #[tokio::test]
    async fn test_connection_age() {
        let (_tx, rx) = broadcast::channel(32);
        let conn = ActiveConnection::new("/test".to_string(), rx, None);

        sleep(Duration::from_millis(10)).await;

        let age = conn.age_seconds();
        // Age should be at least 0 (removing the useless comparison warning)
        assert!(age < 60); // Should be less than 60 seconds for this test
    }
}
