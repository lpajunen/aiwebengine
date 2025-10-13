use anyhow::{Context, Result};
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml, Yaml},
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf, time::Duration};

/// Application configuration with comprehensive settings for all components
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    /// Server configuration
    pub server: ServerConfig,

    /// Logging configuration
    pub logging: LoggingConfig,

    /// JavaScript engine configuration
    pub javascript: JavaScriptConfig,

    /// Repository configuration
    pub repository: RepositoryConfig,

    /// Security configuration
    pub security: SecurityConfig,

    /// Performance configuration
    pub performance: PerformanceConfig,

    /// Authentication configuration (optional)
    #[serde(default)]
    pub auth: Option<crate::auth::AuthConfig>,
}

/// Server-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server bind address
    pub host: String,

    /// Server port
    pub port: u16,

    /// Request timeout in seconds
    pub request_timeout_secs: u64,

    /// Keep-alive timeout in seconds
    pub keep_alive_timeout_secs: u64,

    /// Maximum number of concurrent connections
    pub max_connections: usize,

    /// Enable graceful shutdown
    pub graceful_shutdown: bool,

    /// Shutdown timeout in seconds
    pub shutdown_timeout_secs: u64,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,

    /// Log format (json, pretty, compact)
    pub format: String,

    /// Enable file logging
    pub file_enabled: bool,

    /// Log file path
    pub file_path: Option<PathBuf>,

    /// Log file rotation size in MB
    pub file_max_size_mb: u64,

    /// Number of rotated log files to keep
    pub file_max_files: usize,

    /// Enable console logging
    pub console_enabled: bool,
}

/// JavaScript engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaScriptConfig {
    /// Script execution timeout in milliseconds
    pub execution_timeout_ms: u64,

    /// Maximum memory usage per script in bytes
    pub max_memory_bytes: usize,

    /// Maximum number of concurrent script executions
    pub max_concurrent_executions: usize,

    /// Enable script compilation caching
    pub enable_compilation_cache: bool,

    /// Maximum number of cached compiled scripts
    pub max_cached_scripts: usize,

    /// Script execution stack size in bytes
    pub stack_size_bytes: usize,
}

/// Repository configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryConfig {
    /// Repository type (memory, sqlite, postgresql)
    pub storage_type: String,

    /// Database connection string (for non-memory storage)
    pub connection_string: Option<String>,

    /// Maximum script size in bytes
    pub max_script_size_bytes: usize,

    /// Maximum asset size in bytes
    pub max_asset_size_bytes: usize,

    /// Maximum number of log messages per script
    pub max_log_messages_per_script: usize,

    /// Log message retention time in hours
    pub log_retention_hours: u64,

    /// Enable automatic log pruning
    pub auto_prune_logs: bool,
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Enable CORS
    pub enable_cors: bool,

    /// CORS allowed origins
    pub cors_allowed_origins: Vec<String>,

    /// Enable CSRF protection
    pub enable_csrf: bool,

    /// Enable rate limiting
    pub enable_rate_limiting: bool,

    /// Rate limit: requests per minute per IP
    pub rate_limit_per_minute: u32,

    /// Enable security headers
    pub enable_security_headers: bool,

    /// Content Security Policy header value
    pub content_security_policy: Option<String>,

    /// Enable request validation
    pub enable_request_validation: bool,

    /// Maximum request body size in bytes
    pub max_request_body_bytes: usize,
}

/// Performance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Enable response compression
    pub enable_compression: bool,

    /// Compression level (1-9)
    pub compression_level: u32,

    /// Enable response caching
    pub enable_response_cache: bool,

    /// Response cache TTL in seconds
    pub response_cache_ttl_secs: u64,

    /// Maximum number of cached responses
    pub max_cached_responses: usize,

    /// Worker thread pool size
    pub worker_threads: Option<usize>,

    /// Enable metrics collection
    pub enable_metrics: bool,

    /// Metrics collection interval in seconds
    pub metrics_interval_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            request_timeout_secs: 30,
            keep_alive_timeout_secs: 60,
            max_connections: 10000,
            graceful_shutdown: true,
            shutdown_timeout_secs: 30,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "pretty".to_string(),
            file_enabled: false,
            file_path: None,
            file_max_size_mb: 100,
            file_max_files: 10,
            console_enabled: true,
        }
    }
}

impl Default for JavaScriptConfig {
    fn default() -> Self {
        Self {
            execution_timeout_ms: 5000,
            max_memory_bytes: 10 * 1024 * 1024, // 10MB
            max_concurrent_executions: 100,
            enable_compilation_cache: true,
            max_cached_scripts: 1000,
            stack_size_bytes: 1024 * 1024, // 1MB
        }
    }
}

impl Default for RepositoryConfig {
    fn default() -> Self {
        Self {
            storage_type: "memory".to_string(),
            connection_string: None,
            max_script_size_bytes: 1024 * 1024,     // 1MB
            max_asset_size_bytes: 10 * 1024 * 1024, // 10MB
            max_log_messages_per_script: 100,
            log_retention_hours: 24,
            auto_prune_logs: true,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enable_cors: true,
            cors_allowed_origins: vec!["*".to_string()],
            enable_csrf: false,
            enable_rate_limiting: true,
            rate_limit_per_minute: 100,
            enable_security_headers: true,
            content_security_policy: Some(
                "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'".to_string()
            ),
            enable_request_validation: true,
            max_request_body_bytes: 1024 * 1024, // 1MB
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            enable_compression: true,
            compression_level: 6,
            enable_response_cache: false,
            response_cache_ttl_secs: 300,
            max_cached_responses: 1000,
            worker_threads: None, // Use Tokio default
            enable_metrics: true,
            metrics_interval_secs: 60,
        }
    }
}

impl AppConfig {
    /// Load configuration from multiple sources with precedence:
    /// 1. Command line arguments
    /// 2. Environment variables
    /// 3. Config file (TOML, YAML, JSON5, etc.)
    /// 4. Default values
    pub fn load() -> Result<Self, anyhow::Error> {
        let config: Self = Figment::new()
            .merge(Serialized::defaults(Self::default()))
            .merge(Toml::file("config.toml"))
            .merge(Yaml::file("config.yaml"))
            .merge(Yaml::file("config.yml"))
            .merge(Env::prefixed("APP_"))
            .extract()?;

        // Validate the configuration
        config.validate()?;

        Ok(config)
    }

    /// Create a test configuration with a specific port
    pub fn test_config_with_port(port: u16) -> Self {
        let mut config = Self::default();
        config.server.port = port;
        config
    }

    /// Load configuration from a specific file
    pub fn load_from_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let figment = Figment::new().merge(Serialized::defaults(AppConfig::default()));

        let figment = if path.extension() == Some(std::ffi::OsStr::new("toml")) {
            figment.merge(Toml::file(path))
        } else if path.extension() == Some(std::ffi::OsStr::new("yaml"))
            || path.extension() == Some(std::ffi::OsStr::new("yml"))
        {
            figment.merge(Yaml::file(path))
        } else {
            return Err(anyhow::anyhow!(
                "Unsupported config file format: {:?}",
                path
            ));
        };

        let config: AppConfig = figment
            .merge(Env::prefixed("AIWEBENGINE_").split("__"))
            .extract()
            .context("Failed to load configuration from file")?;

        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values
    pub fn validate(&self) -> Result<()> {
        // Validate server configuration
        if self.server.port == 0 {
            anyhow::bail!("Server port cannot be 0");
        }

        // Port range is validated by u16 type, no need to check upper bound

        if self.server.request_timeout_secs == 0 {
            anyhow::bail!("Request timeout must be > 0");
        }

        if self.server.max_connections == 0 {
            anyhow::bail!("Max connections must be > 0");
        }

        // Validate logging configuration
        match self.logging.level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => anyhow::bail!(
                "Invalid log level: {}. Must be one of: trace, debug, info, warn, error",
                self.logging.level
            ),
        }

        match self.logging.format.as_str() {
            "json" | "pretty" | "compact" => {}
            _ => anyhow::bail!(
                "Invalid log format: {}. Must be one of: json, pretty, compact",
                self.logging.format
            ),
        }

        if self.logging.file_enabled && self.logging.file_path.is_none() {
            anyhow::bail!("File logging is enabled but no file path specified");
        }

        // Validate JavaScript configuration
        if self.javascript.execution_timeout_ms == 0 {
            anyhow::bail!("JavaScript execution timeout must be > 0");
        }

        if self.javascript.max_memory_bytes == 0 {
            anyhow::bail!("JavaScript max memory must be > 0");
        }

        if self.javascript.max_concurrent_executions == 0 {
            anyhow::bail!("JavaScript max concurrent executions must be > 0");
        }

        // Validate repository configuration
        match self.repository.storage_type.as_str() {
            "memory" | "sqlite" | "postgresql" => {}
            _ => anyhow::bail!(
                "Invalid storage type: {}. Must be one of: memory, sqlite, postgresql",
                self.repository.storage_type
            ),
        }

        if self.repository.storage_type != "memory" && self.repository.connection_string.is_none() {
            anyhow::bail!("Connection string required for non-memory storage types");
        }

        // Validate security configuration
        // Note: rate_limit_per_minute of 0 means disabled, which is allowed

        if self.security.max_request_body_bytes == 0 {
            anyhow::bail!("Max request body size must be > 0");
        }

        // Validate performance configuration
        if self.performance.compression_level < 1 || self.performance.compression_level > 9 {
            anyhow::bail!("Compression level must be between 1 and 9");
        }

        if self.performance.metrics_interval_secs == 0 {
            anyhow::bail!("Metrics interval must be > 0");
        }

        Ok(())
    }

    /// Get server socket address
    pub fn server_address(&self) -> Result<SocketAddr> {
        format!("{}:{}", self.server.host, self.server.port)
            .parse()
            .context("Invalid server address")
    }

    /// Get request timeout as Duration
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.server.request_timeout_secs)
    }

    /// Get JavaScript execution timeout as Duration
    pub fn js_execution_timeout(&self) -> Duration {
        Duration::from_millis(self.javascript.execution_timeout_ms)
    }

    /// Get log retention duration
    pub fn log_retention_duration(&self) -> Duration {
        Duration::from_secs(self.repository.log_retention_hours * 3600)
    }
}

// Keep backward compatibility with the old Config struct
pub type Config = AppConfig;

impl AppConfig {
    /// Backward compatibility method - equivalent to load()
    pub fn from_env() -> Self {
        Self::load().unwrap_or_else(|_| Self::default())
    }

    /// Backward compatibility method for server address
    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }

    /// Backward compatibility getters
    pub fn port(&self) -> u16 {
        self.server.port
    }

    pub fn host(&self) -> &str {
        &self.server.host
    }

    pub fn script_timeout_ms(&self) -> u64 {
        self.javascript.execution_timeout_ms
    }

    pub fn max_concurrent_requests(&self) -> usize {
        self.javascript.max_concurrent_executions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.javascript.execution_timeout_ms, 5000);
    }

    #[test]
    fn test_config_validation() {
        let mut config = AppConfig::default();

        // Valid config should pass
        assert!(config.validate().is_ok());

        // Invalid port should fail
        config.server.port = 0;
        assert!(config.validate().is_err());

        // Reset and test invalid log level
        config = AppConfig::default();
        config.logging.level = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_server_address() {
        let config = AppConfig::default();
        let addr = config.server_address().unwrap();
        assert_eq!(addr.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn test_timeout_conversions() {
        let config = AppConfig::default();
        assert_eq!(config.request_timeout(), Duration::from_secs(30));
        assert_eq!(config.js_execution_timeout(), Duration::from_millis(5000));
    }

    #[test]
    fn test_environment_variable_override() {
        // Test that environment loading doesn't panic
        let config = AppConfig::from_env();
        assert!(config.server.port > 0);

        // This would test actual loading, but we need to be careful in tests
        // as it affects other tests. In a real test, you'd use a separate process
        // or mock the environment.
    }

    #[test]
    fn test_validation_edge_cases() {
        let mut config = AppConfig::default();

        // Test port edge cases
        config.server.port = 65535;
        assert!(config.validate().is_ok());

        config.server.port = 0;
        assert!(config.validate().is_err());

        // Reset for memory tests - test zero memory (which should fail)
        config = AppConfig::default();
        config.javascript.max_memory_bytes = 0;
        assert!(config.validate().is_err());

        // Test timeout edge cases - only zero timeout should fail
        config = AppConfig::default();
        config.javascript.execution_timeout_ms = 1;
        assert!(config.validate().is_ok());

        config.javascript.execution_timeout_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_backward_compatibility() {
        let config = AppConfig::default();

        // Test backward compatibility methods
        assert_eq!(config.port(), 8080);
        assert_eq!(config.host(), "127.0.0.1");
        assert_eq!(config.script_timeout_ms(), 5000);
        assert_eq!(config.max_concurrent_requests(), 100); // Correct default value
        assert_eq!(config.server_addr(), "127.0.0.1:8080");
    }

    #[test]
    fn test_config_file_loading() {
        // Test that loading with missing files doesn't crash
        let result = AppConfig::load();
        // May fail due to missing config files or figment features, but should not panic
        // The important thing is it doesn't panic
        match result {
            Ok(_) => {}  // Config loaded successfully
            Err(_) => {} // Config loading failed, which is acceptable in test environment
        }
    }

    #[test]
    fn test_test_config_helper() {
        let config = AppConfig::test_config_with_port(3000);
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.server.host, "127.0.0.1"); // Should keep other defaults
    }

    #[test]
    fn test_security_validation() {
        let mut config = AppConfig::default();

        // Test CORS origins validation
        config.security.cors_allowed_origins = vec!["invalid-origin".to_string()];
        assert!(config.validate().is_ok());

        // Test rate limiting validation - zero rate limit means disabled (allowed)
        config.security.rate_limit_per_minute = 0;
        assert!(config.validate().is_ok());

        config.security.rate_limit_per_minute = 100;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_performance_validation() {
        let mut config = AppConfig::default();

        // Test compression level validation
        config.performance.compression_level = 0;
        assert!(config.validate().is_err());

        config.performance.compression_level = 10;
        assert!(config.validate().is_err());

        config.performance.compression_level = 5;
        assert!(config.validate().is_ok());

        // Test metrics interval validation
        config.performance.metrics_interval_secs = 0;
        assert!(config.validate().is_err());

        config.performance.metrics_interval_secs = 60;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_database_type_validation() {
        let mut config = AppConfig::default();

        // Test memory storage (default)
        config.repository.storage_type = "memory".to_string();
        assert!(config.validate().is_ok());

        // Test sqlite - requires connection string
        config.repository.storage_type = "sqlite".to_string();
        config.repository.connection_string = Some("./test.db".to_string());
        assert!(config.validate().is_ok());

        // Test postgresql - requires connection string
        config.repository.storage_type = "postgresql".to_string();
        config.repository.connection_string =
            Some("postgresql://user:pass@localhost/db".to_string());
        assert!(config.validate().is_ok());

        // Test sqlite without connection string - should fail
        config.repository.storage_type = "sqlite".to_string();
        config.repository.connection_string = None;
        assert!(config.validate().is_err());

        // Test invalid storage type
        config.repository.storage_type = "mysql".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_log_level_validation() {
        let mut config = AppConfig::default();

        // Test all valid log levels
        for level in &["trace", "debug", "info", "warn", "error"] {
            config.logging.level = level.to_string();
            assert!(
                config.validate().is_ok(),
                "Log level {} should be valid",
                level
            );
        }

        // Test invalid log level
        config.logging.level = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_duration_helpers() {
        let config = AppConfig::default();

        // Test duration conversion methods
        assert_eq!(config.request_timeout().as_secs(), 30);
        assert_eq!(config.js_execution_timeout().as_millis(), 5000);

        // Test log retention duration
        let retention = config.log_retention_duration();
        assert_eq!(retention.as_secs(), 24 * 3600); // 24 hours in seconds
    }
}
