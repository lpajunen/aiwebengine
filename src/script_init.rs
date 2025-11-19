use crate::repository;
use crate::repository::{
    RepositoryError, get_all_script_metadata, get_script_metadata, mark_script_init_failed,
};
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

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

    /// Initialize a single script by URI
    pub async fn initialize_script(
        &self,
        script_uri: &str,
        is_startup: bool,
    ) -> Result<InitResult, String> {
        let start_time = std::time::Instant::now();

        // Get script metadata
        let metadata = match get_script_metadata(script_uri) {
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

        debug!("Initializing script: {}", script_uri);

        // Create init context
        let context = InitContext::new(metadata.uri.clone(), is_startup);

        // Call init with timeout
        let timeout_duration = Duration::from_millis(self.timeout_ms);
        let script_uri_clone = script_uri.to_string();
        let metadata_clone = metadata.clone();

        let result = timeout(
            timeout_duration,
            tokio::task::spawn_blocking(move || {
                crate::js_engine::call_init_if_exists(
                    &script_uri_clone,
                    &metadata_clone.content,
                    context,
                )
            }),
        )
        .await;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(init_result)) => match init_result {
                Ok(Some(registrations)) => {
                    // Init function was called successfully and returned registrations
                    if let Err(e) = repository::mark_script_initialized_with_registrations(
                        script_uri,
                        registrations,
                    ) {
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
                Err(e) => {
                    // Init function threw an error (already formatted by call_init_if_exists)
                    if let Err(err) = mark_script_init_failed(script_uri, e.clone()) {
                        warn!("Failed to mark script init as failed: {}", err);
                    }
                    // Log FATAL error to database
                    repository::insert_log_message(script_uri, &e, "FATAL");
                    warn!("✗ Script '{}' init failed: {}", script_uri, e);
                    Ok(InitResult::failed(script_uri.to_string(), e, duration_ms))
                }
            },
            Ok(Err(join_error)) => {
                // Task panicked or was cancelled
                let error_msg = format!("Init task failed: {}", join_error);
                if let Err(e) = mark_script_init_failed(script_uri, error_msg.clone()) {
                    warn!("Failed to mark script init as failed: {}", e);
                }
                // Log FATAL error to database
                repository::insert_log_message(script_uri, &error_msg, "FATAL");
                error!("✗ Script '{}' init task failed: {}", script_uri, join_error);
                Ok(InitResult::failed(
                    script_uri.to_string(),
                    error_msg,
                    duration_ms,
                ))
            }
            Err(_timeout_error) => {
                // Timeout occurred
                let error_msg = format!("Init timeout ({}ms)", self.timeout_ms);
                if let Err(e) = mark_script_init_failed(script_uri, error_msg.clone()) {
                    warn!("Failed to mark script init as failed: {}", e);
                }
                // Log FATAL error to database
                repository::insert_log_message(script_uri, &error_msg, "FATAL");
                error!(
                    "✗ Script '{}' init timeout after {}ms",
                    script_uri, self.timeout_ms
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
    pub async fn initialize_all_scripts(&self) -> Result<Vec<InitResult>, RepositoryError> {
        info!("Initializing all registered scripts...");
        let start_time = std::time::Instant::now();

        // Get all script metadata
        let all_metadata = match get_all_script_metadata() {
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

        let mut results = Vec::new();

        // Initialize scripts sequentially for now
        // TODO: Consider parallel initialization for independent scripts
        for metadata in all_metadata {
            match self.initialize_script(&metadata.uri, true).await {
                Ok(result) => {
                    results.push(result);
                }
                Err(e) => {
                    error!("Failed to initialize script {}: {}", metadata.uri, e);
                    results.push(InitResult::failed(metadata.uri.clone(), e.to_string(), 0));
                }
            }
        }

        let total_duration = start_time.elapsed().as_millis();
        let successful = results.iter().filter(|r| r.success).count();
        let failed = results.iter().filter(|r| !r.success).count();

        info!(
            "Script initialization complete: {} successful, {} failed, {}ms total",
            successful, failed, total_duration
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
