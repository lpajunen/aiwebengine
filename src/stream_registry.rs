use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Send PostgreSQL notification for stream broadcast (cross-instance sync)
async fn send_stream_broadcast_notification(path: &str, message: &str) -> Result<(), String> {
    // Get database pool if available
    let db = match crate::database::get_global_database() {
        Some(db) => db,
        None => {
            // No database available (memory mode), skip notification
            return Ok(());
        }
    };

    // Get server ID
    let server_id = match crate::notifications::get_server_id() {
        Some(id) => id,
        None => {
            // No server ID available, skip notification
            return Ok(());
        }
    };

    // Create notification payload
    let payload = serde_json::json!({
        "stream_path": path,
        "message": message,
        "timestamp": chrono::Utc::now().timestamp(),
        "server_id": server_id,
    });

    let payload_str = payload.to_string();

    // Send notification using pg_notify
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind("stream_broadcast")
        .bind(&payload_str)
        .execute(db.pool())
        .await
        .map_err(|e| {
            error!("Failed to send stream broadcast notification: {}", e);
            format!("Failed to send notification: {}", e)
        })?;

    debug!("Sent stream broadcast notification for path: {}", path);
    Ok(())
}

/// Result of a broadcast operation
#[derive(Debug, Clone, PartialEq)]
pub struct BroadcastResult {
    pub successful_sends: usize,
    pub failed_connections: Vec<String>,
    pub total_connections: usize,
}

impl BroadcastResult {
    /// Check if all sends were successful
    pub fn is_fully_successful(&self) -> bool {
        self.failed_connections.is_empty()
    }

    /// Get the failure rate as a percentage (0.0 to 1.0)
    pub fn failure_rate(&self) -> f64 {
        if self.total_connections == 0 {
            0.0
        } else {
            self.failed_connections.len() as f64 / self.total_connections as f64
        }
    }
}

/// Represents a single stream connection
#[derive(Debug, Clone)]
pub struct StreamConnection {
    /// Unique identifier for this connection
    pub connection_id: String,
    /// Timestamp when the connection was established
    pub connected_at: u64,
    /// The broadcast sender for this connection
    pub sender: broadcast::Sender<String>,
    /// Optional metadata about the client
    pub metadata: Option<HashMap<String, String>>,
}

impl Default for StreamConnection {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamConnection {
    /// Create a new stream connection
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1000); // Buffer up to 1000 messages
        Self {
            connection_id: Uuid::new_v4().to_string(),
            connected_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            sender,
            metadata: None,
        }
    }

    /// Create a new stream connection with metadata
    pub fn with_metadata(metadata: HashMap<String, String>) -> Self {
        let mut conn = Self::new();
        conn.metadata = Some(metadata);
        conn
    }

    /// Send a message to this connection
    pub fn send_message(
        &self,
        message: &str,
    ) -> Result<usize, broadcast::error::SendError<String>> {
        self.sender.send(message.to_string())
    }

    /// Get a receiver for this connection
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.sender.subscribe()
    }

    /// Get connection age in seconds
    pub fn age_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(self.connected_at)
    }
}

/// Information about a registered stream path
#[derive(Debug, Clone)]
pub struct StreamRegistration {
    /// The path pattern for this stream
    pub path: String,
    /// The script URI that registered this stream
    pub script_uri: String,
    /// Timestamp when the stream was registered
    pub registered_at: u64,
    /// Active connections for this stream path
    pub connections: HashMap<String, StreamConnection>,
    /// Optional customization function to determine connection filter criteria
    pub customization_function: Option<String>,
}

impl StreamRegistration {
    /// Create a new stream registration
    pub fn new(path: String, script_uri: String, customization_function: Option<String>) -> Self {
        Self {
            path,
            script_uri,
            registered_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            connections: HashMap::new(),
            customization_function,
        }
    }

    /// Add a new connection to this stream
    pub fn add_connection(&mut self, connection: StreamConnection) -> String {
        let connection_id = connection.connection_id.clone();
        self.connections.insert(connection_id.clone(), connection);
        debug!(
            "Added connection {} to stream path {}. Total connections: {}",
            connection_id,
            self.path,
            self.connections.len()
        );
        connection_id
    }

    /// Remove a connection from this stream
    pub fn remove_connection(&mut self, connection_id: &str) -> bool {
        let removed = self.connections.remove(connection_id).is_some();
        if removed {
            debug!(
                "Removed connection {} from stream path {}. Remaining connections: {}",
                connection_id,
                self.path,
                self.connections.len()
            );
        }
        removed
    }

    /// Broadcast a message to all connections in this stream
    pub fn broadcast_message(&self, message: &str) -> BroadcastResult {
        let mut successful_sends = 0;
        let mut failed_connections = Vec::new();

        for (connection_id, connection) in &self.connections {
            match connection.send_message(message) {
                Ok(_) => {
                    successful_sends += 1;
                    debug!(
                        "Sent message to connection {} on path {}",
                        connection_id, self.path
                    );
                }
                Err(broadcast::error::SendError(_)) => {
                    warn!(
                        "Failed to send message to connection {} on path {} (no receivers)",
                        connection_id, self.path
                    );
                    failed_connections.push(connection_id.clone());
                }
            }
        }

        let result = BroadcastResult {
            successful_sends,
            failed_connections: failed_connections.clone(),
            total_connections: self.connections.len(),
        };

        if !failed_connections.is_empty() {
            warn!(
                "Broadcast to path {}: {} successful, {} failed of {} total connections: {:?}",
                self.path,
                successful_sends,
                failed_connections.len(),
                self.connections.len(),
                failed_connections
            );
        }

        result
    }

    /// Get the number of active connections
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Clean up stale connections (older than max_age_seconds, or all if max_age_seconds is 0)
    pub fn cleanup_stale_connections(&mut self, max_age_seconds: u64) -> usize {
        let initial_count = self.connections.len();
        self.connections.retain(|connection_id, connection| {
            // Special case: if max_age_seconds is 0, clean up all connections
            let should_keep = if max_age_seconds == 0 {
                false
            } else {
                connection.age_seconds() < max_age_seconds
            };
            if !should_keep {
                debug!(
                    "Removing stale connection {} (age: {}s) from path {}",
                    connection_id,
                    connection.age_seconds(),
                    self.path
                );
            }
            should_keep
        });
        initial_count - self.connections.len()
    }
}

/// Global registry for managing stream paths and connections
#[derive(Debug)]
pub struct StreamRegistry {
    /// Map of stream paths to their registrations
    streams: Arc<Mutex<HashMap<String, StreamRegistration>>>,
}

impl StreamRegistry {
    /// Create a new stream registry
    pub fn new() -> Self {
        Self {
            streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a new stream path with optional customization function
    pub fn register_stream(
        &self,
        path: &str,
        script_uri: &str,
        customization_function: Option<String>,
    ) -> Result<(), String> {
        match self.streams.lock() {
            Ok(mut streams) => {
                if let Some(existing_registration) = streams.get(path) {
                    // If the stream is already registered and has active connections, preserve them
                    let connection_count = existing_registration.connection_count();
                    if connection_count > 0 {
                        info!(
                            "Stream path '{}' already registered with {} active connections, keeping existing registration from script '{}'",
                            path, connection_count, existing_registration.script_uri
                        );
                        return Ok(());
                    } else {
                        warn!(
                            "Stream path '{}' already registered with no active connections, replacing with new registration from script '{}'",
                            path, script_uri
                        );
                    }
                } else {
                    info!(
                        "Registering new stream path '{}' for script '{}'",
                        path, script_uri
                    );
                }

                let registration = StreamRegistration::new(
                    path.to_string(),
                    script_uri.to_string(),
                    customization_function,
                );
                streams.insert(path.to_string(), registration);
                Ok(())
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to register stream: registry lock error".to_string())
            }
        }
    }

    /// Unregister a stream path
    pub fn unregister_stream(&self, path: &str) -> Result<bool, String> {
        match self.streams.lock() {
            Ok(mut streams) => {
                let removed = streams.remove(path).is_some();
                if removed {
                    info!("Unregistered stream path '{}'", path);
                } else {
                    debug!(
                        "Attempted to unregister non-existent stream path '{}'",
                        path
                    );
                }
                Ok(removed)
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to unregister stream: registry lock error".to_string())
            }
        }
    }

    /// Check if a path is registered as a stream
    pub fn is_stream_registered(&self, path: &str) -> bool {
        match self.streams.lock() {
            Ok(streams) => streams.contains_key(path),
            Err(e) => {
                error!(
                    "Failed to acquire stream registry lock for path check: {}",
                    e
                );
                false
            }
        }
    }

    /// Get the script URI that registered a stream path
    pub fn get_stream_script_uri(&self, path: &str) -> Option<String> {
        match self.streams.lock() {
            Ok(streams) => streams.get(path).map(|reg| reg.script_uri.clone()),
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                None
            }
        }
    }

    /// Get the customization function for a stream path
    pub fn get_stream_customization_function(&self, path: &str) -> Option<String> {
        match self.streams.lock() {
            Ok(streams) => streams
                .get(path)
                .and_then(|reg| reg.customization_function.clone()),
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                None
            }
        }
    }

    /// Get both script URI and customization function for a stream path
    pub fn get_stream_info(&self, path: &str) -> Option<(String, Option<String>)> {
        match self.streams.lock() {
            Ok(streams) => streams
                .get(path)
                .map(|reg| (reg.script_uri.clone(), reg.customization_function.clone())),
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                None
            }
        }
    }

    /// Add a connection to a stream path
    pub fn add_connection(
        &self,
        path: &str,
        connection: StreamConnection,
    ) -> Result<String, String> {
        match self.streams.lock() {
            Ok(mut streams) => match streams.get_mut(path) {
                Some(registration) => {
                    let connection_id = registration.add_connection(connection);
                    Ok(connection_id)
                }
                None => {
                    error!(
                        "Attempted to add connection to unregistered stream path '{}'",
                        path
                    );
                    Err(format!("Stream path '{}' not registered", path))
                }
            },
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to add connection: registry lock error".to_string())
            }
        }
    }

    /// Remove a connection from a stream path
    pub fn remove_connection(&self, path: &str, connection_id: &str) -> Result<bool, String> {
        match self.streams.lock() {
            Ok(mut streams) => match streams.get_mut(path) {
                Some(registration) => Ok(registration.remove_connection(connection_id)),
                None => {
                    debug!(
                        "Attempted to remove connection from unregistered stream path '{}'",
                        path
                    );
                    Ok(false)
                }
            },
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to remove connection: registry lock error".to_string())
            }
        }
    }

    /// Get a connection receiver for a specific stream path
    pub fn get_connection_receiver(
        &self,
        path: &str,
    ) -> Result<Option<broadcast::Receiver<String>>, String> {
        match self.streams.lock() {
            Ok(streams) => {
                match streams.get(path) {
                    Some(_registration) => {
                        // Create a new connection and return its receiver
                        let connection = StreamConnection::new();
                        let receiver = connection.subscribe();
                        Ok(Some(receiver))
                    }
                    None => Ok(None),
                }
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to get connection receiver: registry lock error".to_string())
            }
        }
    }

    /// Broadcast a message to all connections on all stream paths
    pub fn broadcast_to_all_streams(&self, message: &str) -> Result<BroadcastResult, String> {
        match self.streams.lock() {
            Ok(mut streams) => {
                let mut total_successful = 0;
                let mut all_failed_connections = Vec::new();
                let mut total_connections = 0;
                let mut total_cleaned = 0;

                for (path, registration) in streams.iter_mut() {
                    let result = registration.broadcast_message(message);
                    total_successful += result.successful_sends;
                    all_failed_connections.extend(result.failed_connections.clone());
                    total_connections += result.total_connections;

                    // Automatically clean up failed connections
                    if !result.failed_connections.is_empty() {
                        let mut cleaned_count = 0;
                        for failed_connection_id in &result.failed_connections {
                            if registration.remove_connection(failed_connection_id) {
                                cleaned_count += 1;
                            }
                        }
                        total_cleaned += cleaned_count;
                        if cleaned_count > 0 {
                            debug!(
                                "Auto-cleaned {} failed connections from path '{}'",
                                cleaned_count, path
                            );
                        }
                    }

                    if result.successful_sends > 0 {
                        debug!(
                            "Broadcasted message to {} connections on path '{}'",
                            result.successful_sends, path
                        );
                    }
                    if !result.failed_connections.is_empty() {
                        warn!(
                            "Failed to send to {} connections on path '{}': {:?}",
                            result.failed_connections.len(),
                            path,
                            result.failed_connections
                        );
                    }
                }

                let overall_result = BroadcastResult {
                    successful_sends: total_successful,
                    failed_connections: all_failed_connections,
                    total_connections,
                };

                if !overall_result.is_fully_successful() {
                    warn!(
                        "Broadcast completed with {} successful, {} failed out of {} total connections ({}% failure rate)",
                        overall_result.successful_sends,
                        overall_result.failed_connections.len(),
                        overall_result.total_connections,
                        (overall_result.failure_rate() * 100.0).round()
                    );
                }

                if total_cleaned > 0 {
                    info!(
                        "Auto-cleaned {} total failed connections across all streams",
                        total_cleaned
                    );
                }

                Ok(overall_result)
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to broadcast: registry lock error".to_string())
            }
        }
    }

    /// Broadcast a message to all connections on a specific stream path
    pub fn broadcast_to_stream(
        &self,
        path: &str,
        message: &str,
    ) -> Result<BroadcastResult, String> {
        // Send cross-instance notification in background (non-blocking)
        let path_clone = path.to_string();
        let message_clone = message.to_string();
        tokio::spawn(async move {
            if let Err(e) = send_stream_broadcast_notification(&path_clone, &message_clone).await {
                debug!(
                    "Failed to send cross-instance broadcast notification: {}",
                    e
                );
            }
        });

        match self.streams.lock() {
            Ok(mut streams) => {
                match streams.get_mut(path) {
                    Some(registration) => {
                        let result = registration.broadcast_message(message);

                        // Automatically clean up failed connections if any
                        if !result.failed_connections.is_empty() {
                            let mut cleaned_count = 0;
                            for failed_connection_id in &result.failed_connections {
                                if registration.remove_connection(failed_connection_id) {
                                    cleaned_count += 1;
                                }
                            }
                            if cleaned_count > 0 {
                                info!(
                                    "Auto-cleaned {} failed connections from path '{}'",
                                    cleaned_count, path
                                );
                            }
                        }

                        debug!(
                            "Broadcasted message to path '{}' (success: {}, failed: {})",
                            path,
                            result.successful_sends,
                            result.failed_connections.len()
                        );
                        Ok(result)
                    }
                    None => {
                        debug!(
                            "Attempted to broadcast to unregistered stream path '{}'",
                            path
                        );
                        Ok(BroadcastResult {
                            successful_sends: 0,
                            failed_connections: Vec::new(),
                            total_connections: 0,
                        })
                    }
                }
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to broadcast to stream: registry lock error".to_string())
            }
        }
    }

    /// Broadcast a message to connections on a specific stream path that match the given metadata filter
    pub fn broadcast_to_stream_with_filter(
        &self,
        path: &str,
        message: &str,
        metadata_filter: &HashMap<String, String>,
    ) -> Result<BroadcastResult, String> {
        match self.streams.lock() {
            Ok(mut streams) => {
                match streams.get_mut(path) {
                    Some(registration) => {
                        let mut successful_sends = 0;
                        let mut failed_connections = Vec::new();
                        let mut total_matching_connections = 0;

                        for (connection_id, connection) in &registration.connections {
                            // NEW SEMANTICS: Message metadata must contain all connection metadata keys/values
                            // Connection metadata = minimum required fields that must be in message metadata
                            // Empty connection metadata matches all messages
                            let matches_filter =
                                if let Some(ref conn_metadata) = connection.metadata {
                                    if conn_metadata.is_empty() {
                                        // Empty connection metadata matches all messages
                                        true
                                    } else {
                                        // All connection metadata keys must exist in message metadata with matching values
                                        conn_metadata.iter().all(|(key, expected_value)| {
                                            metadata_filter.get(key) == Some(expected_value)
                                        })
                                    }
                                } else {
                                    // No connection metadata matches all messages
                                    true
                                };

                            if matches_filter {
                                total_matching_connections += 1;
                                match connection.send_message(message) {
                                    Ok(_) => {
                                        successful_sends += 1;
                                        debug!(
                                            "Sent filtered message to connection {} on path {}",
                                            connection_id, path
                                        );
                                    }
                                    Err(broadcast::error::SendError(_)) => {
                                        warn!(
                                            "Failed to send filtered message to connection {} on path {} (no receivers)",
                                            connection_id, path
                                        );
                                        failed_connections.push(connection_id.clone());
                                    }
                                }
                            }
                        }

                        let result = BroadcastResult {
                            successful_sends,
                            failed_connections: failed_connections.clone(),
                            total_connections: total_matching_connections,
                        };

                        if !result.failed_connections.is_empty() {
                            warn!(
                                "Filtered broadcast to path {} with filter {:?}: {} successful, {} failed of {} matching connections",
                                path,
                                metadata_filter,
                                successful_sends,
                                failed_connections.len(),
                                total_matching_connections
                            );
                        } else if successful_sends > 0 {
                            debug!(
                                "Filtered broadcast to path {} with filter {:?}: {} connections matched and received message",
                                path, metadata_filter, successful_sends
                            );
                        }

                        // Auto-cleanup failed connections
                        if !result.failed_connections.is_empty() {
                            let mut cleaned_count = 0;
                            for failed_connection_id in &result.failed_connections {
                                if registration.remove_connection(failed_connection_id) {
                                    cleaned_count += 1;
                                }
                            }
                            if cleaned_count > 0 {
                                debug!(
                                    "Auto-cleaned {} failed connections from path '{}' after filtered broadcast",
                                    cleaned_count, path
                                );
                            }
                        }

                        Ok(result)
                    }
                    None => {
                        debug!(
                            "Attempted to broadcast with filter to unregistered stream path '{}'",
                            path
                        );
                        Ok(BroadcastResult {
                            successful_sends: 0,
                            failed_connections: Vec::new(),
                            total_connections: 0,
                        })
                    }
                }
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to broadcast with filter: registry lock error".to_string())
            }
        }
    }

    /// Get statistics about all registered streams
    pub fn get_stream_stats(&self) -> Result<HashMap<String, serde_json::Value>, String> {
        match self.streams.lock() {
            Ok(streams) => {
                let mut stats = HashMap::new();
                for (path, registration) in streams.iter() {
                    let stream_stat = serde_json::json!({
                        "path": registration.path,
                        "script_uri": registration.script_uri,
                        "registered_at": registration.registered_at,
                        "connection_count": registration.connection_count(),
                        "connections": registration.connections.keys().collect::<Vec<_>>()
                    });
                    stats.insert(path.clone(), stream_stat);
                }
                Ok(stats)
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to get stream stats: registry lock error".to_string())
            }
        }
    }

    /// Clean up stale connections across all streams
    pub fn cleanup_stale_connections(&self, max_age_seconds: u64) -> Result<usize, String> {
        match self.streams.lock() {
            Ok(mut streams) => {
                let mut total_removed = 0;
                for (path, registration) in streams.iter_mut() {
                    let removed = registration.cleanup_stale_connections(max_age_seconds);
                    if removed > 0 {
                        info!(
                            "Cleaned up {} stale connections from stream path '{}'",
                            removed, path
                        );
                    }
                    total_removed += removed;
                }
                Ok(total_removed)
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to cleanup stale connections: registry lock error".to_string())
            }
        }
    }

    /// Get a list of all registered stream paths
    pub fn list_stream_paths(&self) -> Result<Vec<String>, String> {
        match self.streams.lock() {
            Ok(streams) => Ok(streams.keys().cloned().collect()),
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to list stream paths: registry lock error".to_string())
            }
        }
    }

    /// Get a list of all registered streams with metadata (path and script URI)
    pub fn list_streams_with_metadata(&self) -> Result<Vec<(String, String)>, String> {
        match self.streams.lock() {
            Ok(streams) => {
                let result: Vec<(String, String)> = streams
                    .iter()
                    .map(|(path, registration)| (path.clone(), registration.script_uri.clone()))
                    .collect();
                Ok(result)
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to list streams with metadata: registry lock error".to_string())
            }
        }
    }

    /// Gracefully shutdown all streams and connections
    pub fn shutdown_all_streams(&self) -> Result<usize, String> {
        match self.streams.lock() {
            Ok(mut streams) => {
                let mut total_connections = 0;
                for (path, registration) in streams.iter() {
                    let connection_count = registration.connection_count();
                    total_connections += connection_count;
                    info!(
                        "Shutting down stream path '{}' with {} connections",
                        path, connection_count
                    );
                }

                // Clear all streams and connections
                streams.clear();
                info!(
                    "Gracefully shutdown {} total connections across all streams",
                    total_connections
                );
                Ok(total_connections)
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock for shutdown: {}", e);
                Err("Failed to shutdown streams: registry lock error".to_string())
            }
        }
    }

    /// Get the total number of connections across all streams
    pub fn total_connection_count(&self) -> Result<usize, String> {
        match self.streams.lock() {
            Ok(streams) => {
                let total = streams.values().map(|reg| reg.connection_count()).sum();
                Ok(total)
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to get total connection count: registry lock error".to_string())
            }
        }
    }

    /// Get health status of all streams with failure statistics
    pub fn get_health_status(&self) -> Result<serde_json::Value, String> {
        match self.streams.lock() {
            Ok(streams) => {
                let mut healthy_streams = 0;
                let total_streams = streams.len();
                let mut total_connections = 0;
                let mut stream_details = Vec::new();

                for (path, registration) in streams.iter() {
                    let connection_count = registration.connection_count();
                    total_connections += connection_count;

                    let stream_health = serde_json::json!({
                        "path": path,
                        "script_uri": registration.script_uri,
                        "connection_count": connection_count,
                        "registered_at": registration.registered_at,
                        "is_healthy": connection_count > 0 || registration.registered_at > 0
                    });

                    if connection_count > 0 {
                        healthy_streams += 1;
                    }

                    stream_details.push(stream_health);
                }

                let health_status = serde_json::json!({
                    "status": if healthy_streams > 0 { "healthy" } else { "idle" },
                    "total_streams": total_streams,
                    "healthy_streams": healthy_streams,
                    "total_connections": total_connections,
                    "streams": stream_details
                });

                Ok(health_status)
            }
            Err(e) => {
                error!(
                    "Failed to acquire stream registry lock for health check: {}",
                    e
                );
                Err("Failed to get health status: registry lock error".to_string())
            }
        }
    }

    /// Clear all streams (useful for testing or shutdown)
    pub fn clear_all_streams(&self) -> Result<(), String> {
        match self.streams.lock() {
            Ok(mut streams) => {
                let count = streams.len();
                streams.clear();
                info!("Cleared all {} stream registrations", count);
                Ok(())
            }
            Err(e) => {
                error!("Failed to acquire stream registry lock: {}", e);
                Err("Failed to clear streams: registry lock error".to_string())
            }
        }
    }
}

impl Default for StreamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Global instance for the stream registry
lazy_static::lazy_static! {
    /// Global stream registry instance
    pub static ref GLOBAL_STREAM_REGISTRY: StreamRegistry = StreamRegistry::new();
}

/// Get the global stream registry instance
pub fn get_global_registry() -> &'static StreamRegistry {
    &GLOBAL_STREAM_REGISTRY
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_stream_connection_creation() {
        let conn = StreamConnection::new();
        assert!(!conn.connection_id.is_empty());
        assert!(conn.connected_at > 0);
        assert!(conn.metadata.is_none());
    }

    #[test]
    fn test_stream_connection_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("user_id".to_string(), "123".to_string());
        metadata.insert("session_id".to_string(), "abc".to_string());

        let conn = StreamConnection::with_metadata(metadata.clone());
        assert_eq!(conn.metadata.unwrap(), metadata);
    }

    #[test]
    fn test_stream_connection_messaging() {
        let conn = StreamConnection::new();
        let mut receiver = conn.subscribe();

        // Send a message
        let result = conn.send_message("test message");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1); // One receiver

        // Receive the message
        let received = receiver.try_recv();
        assert!(received.is_ok());
        assert_eq!(received.unwrap(), "test message");
    }

    #[test]
    fn test_stream_registration_creation() {
        let reg = StreamRegistration::new("/test".to_string(), "test_script.js".to_string(), None);
        assert_eq!(reg.path, "/test");
        assert_eq!(reg.script_uri, "test_script.js");
        assert!(reg.registered_at > 0);
        assert_eq!(reg.connections.len(), 0);
    }

    #[test]
    fn test_stream_registration_add_remove_connections() {
        let mut reg =
            StreamRegistration::new("/test".to_string(), "test_script.js".to_string(), None);

        // Add a connection
        let conn = StreamConnection::new();
        let conn_id = conn.connection_id.clone();
        let added_id = reg.add_connection(conn);
        assert_eq!(added_id, conn_id);
        assert_eq!(reg.connection_count(), 1);

        // Remove the connection
        let removed = reg.remove_connection(&conn_id);
        assert!(removed);
        assert_eq!(reg.connection_count(), 0);

        // Try to remove non-existent connection
        let removed = reg.remove_connection("non-existent");
        assert!(!removed);
    }

    #[test]
    fn test_stream_registration_broadcast() {
        let mut reg =
            StreamRegistration::new("/test".to_string(), "test_script.js".to_string(), None);

        // Add multiple connections
        let conn1 = StreamConnection::new();
        let conn2 = StreamConnection::new();
        let mut receiver1 = conn1.subscribe();
        let mut receiver2 = conn2.subscribe();

        reg.add_connection(conn1);
        reg.add_connection(conn2);

        // Broadcast a message
        let result = reg.broadcast_message("broadcast test");
        assert_eq!(result.successful_sends, 2);
        assert_eq!(result.failed_connections.len(), 0);
        assert_eq!(result.total_connections, 2);

        // Verify both receivers got the message
        assert_eq!(receiver1.try_recv().unwrap(), "broadcast test");
        assert_eq!(receiver2.try_recv().unwrap(), "broadcast test");
    }

    #[test]
    fn test_stream_registry_basic_operations() {
        let registry = StreamRegistry::new();

        // Register a stream
        let result = registry.register_stream("/test", "test_script.js", None);
        assert!(result.is_ok());

        // Check if registered
        assert!(registry.is_stream_registered("/test"));
        assert!(!registry.is_stream_registered("/nonexistent"));

        // Get script URI
        let script_uri = registry.get_stream_script_uri("/test");
        assert_eq!(script_uri.unwrap(), "test_script.js");

        // Unregister
        let result = registry.unregister_stream("/test");
        assert!(result.is_ok());
        assert!(result.unwrap());

        // Check if unregistered
        assert!(!registry.is_stream_registered("/test"));
    }

    #[test]
    fn test_stream_registry_connection_management() {
        let registry = StreamRegistry::new();

        // Register a stream
        registry
            .register_stream("/test", "test_script.js", None)
            .unwrap();

        // Add a connection
        let conn = StreamConnection::new();
        let conn_id = conn.connection_id.clone();
        let result = registry.add_connection("/test", conn);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), conn_id);

        // Remove the connection
        let result = registry.remove_connection("/test", &conn_id);
        assert!(result.is_ok());
        assert!(result.unwrap());

        // Try to add connection to non-existent stream
        let conn2 = StreamConnection::new();
        let result = registry.add_connection("/nonexistent", conn2);
        assert!(result.is_err());
    }

    #[test]
    fn test_stream_registry_broadcasting() {
        let registry = StreamRegistry::new();

        // Register multiple streams
        registry
            .register_stream("/stream1", "script1.js", None)
            .unwrap();
        registry
            .register_stream("/stream2", "script2.js", None)
            .unwrap();

        // Add connections
        let conn1 = StreamConnection::new();
        let conn2 = StreamConnection::new();
        let mut receiver1 = conn1.subscribe();
        let mut receiver2 = conn2.subscribe();

        registry.add_connection("/stream1", conn1).unwrap();
        registry.add_connection("/stream2", conn2).unwrap();

        // Broadcast to specific stream
        let result = registry.broadcast_to_stream("/stream1", "message1");
        assert!(result.is_ok());
        let broadcast_result = result.unwrap();
        assert_eq!(broadcast_result.successful_sends, 1);
        assert_eq!(broadcast_result.failed_connections.len(), 0);

        // Broadcast to all streams
        let result = registry.broadcast_to_all_streams("message2");
        assert!(result.is_ok());
        let broadcast_result = result.unwrap();
        assert_eq!(broadcast_result.successful_sends, 2);
        assert_eq!(broadcast_result.failed_connections.len(), 0);

        // Verify messages received
        assert_eq!(receiver1.try_recv().unwrap(), "message1");
        assert_eq!(receiver1.try_recv().unwrap(), "message2");
        assert_eq!(receiver2.try_recv().unwrap(), "message2");
    }

    #[test]
    fn test_stream_registry_stats() {
        let registry = StreamRegistry::new();

        // Register a stream and add connection
        registry
            .register_stream("/test", "test_script.js", None)
            .unwrap();
        let conn = StreamConnection::new();
        let conn_id = conn.connection_id.clone();
        registry.add_connection("/test", conn).unwrap();

        // Get stats
        let stats = registry.get_stream_stats().unwrap();
        assert_eq!(stats.len(), 1);

        let stream_stat = stats.get("/test").unwrap();
        assert_eq!(stream_stat["path"], "/test");
        assert_eq!(stream_stat["script_uri"], "test_script.js");
        assert_eq!(stream_stat["connection_count"], 1);

        let connections = stream_stat["connections"].as_array().unwrap();
        assert_eq!(connections.len(), 1);
        assert_eq!(connections[0].as_str().unwrap(), conn_id);
    }

    #[test]
    fn test_stream_registry_cleanup() {
        let registry = StreamRegistry::new();

        // Register a stream
        registry
            .register_stream("/test", "test_script.js", None)
            .unwrap();

        // Create an old connection (simulate by creating and waiting)
        let conn = StreamConnection::new();
        registry.add_connection("/test", conn).unwrap();

        // Cleanup with very short max age (should remove the connection)
        thread::sleep(Duration::from_millis(10));
        let result = registry.cleanup_stale_connections(0);
        assert!(result.is_ok());

        // Clear all streams
        let result = registry.clear_all_streams();
        assert!(result.is_ok());
        assert!(registry.list_stream_paths().unwrap().is_empty());
    }

    #[test]
    fn test_stream_connection_age() {
        let conn = StreamConnection::new();
        let initial_age = conn.age_seconds();
        assert!(initial_age <= 1); // Should be 0 or 1 second old at most

        thread::sleep(Duration::from_millis(10));
        let later_age = conn.age_seconds();
        assert!(later_age >= initial_age); // Age should not decrease
    }
}
