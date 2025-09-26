use rquickjs::{Context, Function, Runtime};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{debug, error, warn};

use crate::repository_safe as repository;

/// Represents the result of executing a JavaScript script
#[derive(Debug, Clone)]
pub struct ScriptExecutionResult {
    /// The registrations made by the script via register() calls
    pub registrations: HashMap<(String, String), String>,
    /// Whether the script executed successfully
    pub success: bool,
    /// Error message if execution failed
    pub error: Option<String>,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

/// Resource limits for JavaScript execution
#[derive(Debug, Clone)]
pub struct ExecutionLimits {
    pub timeout_ms: u64,
    pub max_memory_mb: usize,
    pub max_script_size_bytes: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            timeout_ms: 2000,
            max_memory_mb: 50,
            max_script_size_bytes: 1_000_000, // 1MB
        }
    }
}

/// JavaScript execution context with resource management
pub struct SafeJsEngine {
    limits: ExecutionLimits,
    active_executions: Arc<Mutex<usize>>,
    max_concurrent: usize,
}

impl SafeJsEngine {
    pub fn new(limits: ExecutionLimits, max_concurrent: usize) -> Self {
        Self {
            limits,
            active_executions: Arc::new(Mutex::new(0)),
            max_concurrent,
        }
    }

    /// Increment active execution count
    fn start_execution(&self) -> Result<ExecutionGuard, String> {
        match self.active_executions.lock() {
            Ok(mut count) => {
                if *count >= self.max_concurrent {
                    return Err("Too many concurrent JavaScript executions".to_string());
                }
                *count += 1;
                Ok(ExecutionGuard {
                    active_executions: Arc::clone(&self.active_executions),
                })
            }
            Err(_) => Err("Failed to acquire execution lock".to_string()),
        }
    }

    /// Validate script before execution
    fn validate_script(&self, content: &str) -> Result<(), String> {
        if content.len() > self.limits.max_script_size_bytes {
            return Err(format!(
                "Script too large: {} bytes (max: {})",
                content.len(),
                self.limits.max_script_size_bytes
            ));
        }

        // Basic syntax validation - check for obviously problematic patterns
        if content.contains("while(true)") || content.contains("while (true)") {
            warn!("Script contains potentially infinite loop pattern");
        }

        Ok(())
    }

    /// Execute script with comprehensive safety measures (async version)
    pub async fn execute_script_safe(&self, uri: &str, content: &str) -> ScriptExecutionResult {
        let start_time = Instant::now();

        // Validate script size and basic safety
        if let Err(e) = self.validate_script(content) {
            return ScriptExecutionResult {
                registrations: HashMap::new(),
                success: false,
                error: Some(e),
                execution_time_ms: start_time.elapsed().as_millis() as u64,
            };
        }

        // Check execution limits
        let _guard = match self.start_execution() {
            Ok(guard) => guard,
            Err(e) => {
                return ScriptExecutionResult {
                    registrations: HashMap::new(),
                    success: false,
                    error: Some(e),
                    execution_time_ms: start_time.elapsed().as_millis() as u64,
                };
            }
        };

        // Execute with timeout using blocking task
        let uri_owned = uri.to_string();
        let content_owned = content.to_string();
        let execution_future = tokio::task::spawn_blocking(move || {
            Self::execute_script_blocking(&uri_owned, &content_owned)
        });

        match timeout(
            Duration::from_millis(self.limits.timeout_ms),
            execution_future,
        )
        .await
        {
            Ok(Ok(result)) => {
                let execution_time = start_time.elapsed().as_millis() as u64;
                ScriptExecutionResult {
                    registrations: result.registrations,
                    success: result.success,
                    error: result.error,
                    execution_time_ms: execution_time,
                }
            }
            Ok(Err(e)) => {
                error!("Script execution task failed for {}: {}", uri, e);
                ScriptExecutionResult {
                    registrations: HashMap::new(),
                    success: false,
                    error: Some(format!("Task execution error: {}", e)),
                    execution_time_ms: start_time.elapsed().as_millis() as u64,
                }
            }
            Err(_) => {
                error!(
                    "Script execution timeout for {}: {}ms",
                    uri, self.limits.timeout_ms
                );
                ScriptExecutionResult {
                    registrations: HashMap::new(),
                    success: false,
                    error: Some(format!(
                        "Execution timeout after {}ms",
                        self.limits.timeout_ms
                    )),
                    execution_time_ms: self.limits.timeout_ms,
                }
            }
        }
    }

    /// Blocking execution logic
    fn execute_script_blocking(uri: &str, content: &str) -> ScriptExecutionResult {
        let registrations = Arc::new(Mutex::new(HashMap::new()));

        let rt = match Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                error!("Failed to create QuickJS runtime for {}: {}", uri, e);
                return ScriptExecutionResult {
                    registrations: HashMap::new(),
                    success: false,
                    error: Some(format!("Runtime creation error: {}", e)),
                    execution_time_ms: 0,
                };
            }
        };

        let ctx = match Context::full(&rt) {
            Ok(ctx) => ctx,
            Err(e) => {
                error!("Failed to create QuickJS context for {}: {}", uri, e);
                return ScriptExecutionResult {
                    registrations: HashMap::new(),
                    success: false,
                    error: Some(format!("Context creation error: {}", e)),
                    execution_time_ms: 0,
                };
            }
        };

        let result = ctx.with(|ctx| -> Result<(), rquickjs::Error> {
            let global = ctx.globals();

            // Set up the register function with thread-safe storage
            let regs_clone = Arc::clone(&registrations);
            let uri_clone = uri.to_string();
            let register = Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>,
                      path: String,
                      handler: String,
                      method: Option<String>|
                      -> Result<(), rquickjs::Error> {
                    let method = method.unwrap_or_else(|| "GET".to_string());
                    debug!(
                        "Registering route {} {} -> {} for script {}",
                        method, path, handler, uri_clone
                    );

                    if let Ok(mut regs) = regs_clone.lock() {
                        regs.insert((path, method), handler);
                    } else {
                        error!("Failed to acquire registrations lock for {}", uri_clone);
                    }
                    Ok(())
                },
            )?;
            global.set("register", register)?;

            // Set up safe host functions
            Self::setup_host_functions(&global, ctx.clone(), uri)?;

            // Execute the script
            ctx.eval::<(), _>(content)?;
            Ok(())
        });

        match result {
            Ok(_) => {
                debug!("Successfully executed script {}", uri);
                let final_regs = match registrations.lock() {
                    Ok(regs) => regs.clone(),
                    Err(_) => {
                        error!("Failed to access final registrations for {}", uri);
                        HashMap::new()
                    }
                };

                ScriptExecutionResult {
                    registrations: final_regs,
                    success: true,
                    error: None,
                    execution_time_ms: 0,
                }
            }
            Err(e) => {
                error!("Failed to execute script {}: {}", uri, e);
                ScriptExecutionResult {
                    registrations: HashMap::new(),
                    success: false,
                    error: Some(format!("Script evaluation error: {}", e)),
                    execution_time_ms: 0,
                }
            }
        }
    }

    /// Set up host functions with comprehensive error handling
    fn setup_host_functions<'js>(
        global: &rquickjs::Object<'js>,
        ctx: rquickjs::Ctx<'js>,
        uri: &str,
    ) -> Result<(), rquickjs::Error> {
        let uri_owned = uri.to_string();

        // Safe writeLog function
        let script_uri_clone = uri_owned.clone();
        let write_log = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, msg: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript writeLog: {}", msg);
                // Limit log message size
                let truncated_msg = if msg.len() > 1000 {
                    format!("{}... (truncated)", &msg[..1000])
                } else {
                    msg
                };
                repository::insert_log_message(&script_uri_clone, &truncated_msg);
                Ok(())
            },
        )?;
        global.set("writeLog", write_log)?;

        // Safe listLogs function
        let script_uri_clone2 = uri_owned.clone();
        let list_logs = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>| -> Result<Vec<String>, rquickjs::Error> {
                debug!("JavaScript listLogs called");
                let logs = repository::fetch_log_messages(&script_uri_clone2);
                // Limit number of returned logs to prevent memory issues
                if logs.len() > 100 {
                    Ok(logs.into_iter().rev().take(100).collect())
                } else {
                    Ok(logs)
                }
            },
        )?;
        global.set("listLogs", list_logs)?;

        // Safe registerWebStream function
        let script_uri_stream = uri_owned;
        let register_web_stream = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, path: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called registerWebStream with path: {}", path);
                
                // Validate path format (basic security check)
                if path.is_empty() || !path.starts_with('/') {
                    return Err(rquickjs::Error::new_from_js("Stream", "Stream path must start with '/' and not be empty"));
                }
                
                if path.len() > 200 {
                    return Err(rquickjs::Error::new_from_js("Stream", "Stream path too long (max 200 characters)"));
                }
                
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY.register_stream(&path, &script_uri_stream) {
                    Ok(()) => {
                        debug!("Successfully registered stream path '{}' for script '{}'", path, script_uri_stream);
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!("Failed to register stream path '{}': {}", path, e);
                        Err(rquickjs::Error::Exception)
                    }
                }
            },
        )?;
        global.set("registerWebStream", register_web_stream)?;

        // Safe sendStreamMessage function
        let send_stream_message = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>, json_string: String| -> Result<(), rquickjs::Error> {
                debug!("JavaScript called sendStreamMessage with message: {}", json_string);
                
                // Broadcast to all registered streams
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY.broadcast_to_all_streams(&json_string) {
                    Ok(count) => {
                        debug!("Successfully broadcast message to {} connections", count);
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!("Failed to broadcast message: {}", e);
                        Err(rquickjs::Error::Exception)
                    }
                }
            },
        )?;
        global.set("sendStreamMessage", send_stream_message)?;

        Ok(())
    }
}

/// RAII guard for execution counting
struct ExecutionGuard {
    active_executions: Arc<Mutex<usize>>,
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        if let Ok(mut count) = self.active_executions.lock() {
            if *count > 0 {
                *count -= 1;
            }
        }
    }
}

/// Legacy function that uses the safe engine
pub fn execute_script(uri: &str, content: &str) -> ScriptExecutionResult {
    let engine = SafeJsEngine::new(ExecutionLimits::default(), 10);

    // Since this is a sync function, we need to handle the async execution
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            return handle.block_on(engine.execute_script_safe(uri, content));
        }
        Err(_) => {
            // Fallback for when not in async context
            match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    error!("Failed to create Tokio runtime: {}", e);
                    return ScriptExecutionResult {
                        registrations: HashMap::new(),
                        success: false,
                        error: Some(format!("Runtime creation error: {}", e)),
                        execution_time_ms: 0,
                    };
                }
            }
        }
    };

    rt.block_on(engine.execute_script_safe(uri, content))
}

// Export the safe engine for use in main application
pub use SafeJsEngine as JsEngine;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_safe_execution() {
        let engine = SafeJsEngine::new(ExecutionLimits::default(), 5);

        let simple_script = r#"
            function hello() {
                return "world";
            }
            register('/hello', 'hello', 'GET');
        "#;

        let result = engine
            .execute_script_safe("test://simple", simple_script)
            .await;
        assert!(result.success);
        assert!(!result.registrations.is_empty());
    }

    #[tokio::test]
    async fn test_size_limits() {
        let engine = SafeJsEngine::new(ExecutionLimits::default(), 5);

        // Create a script larger than the limit
        let large_script = "x".repeat(2_000_000);

        let result = engine
            .execute_script_safe("test://large", &large_script)
            .await;
        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("too large"));
    }
}
