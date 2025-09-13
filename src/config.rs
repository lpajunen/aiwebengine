/// Configuration for the aiwebengine server
#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub script_timeout_ms: u64,
    pub max_concurrent_requests: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 4000,
            script_timeout_ms: 2000,
            max_concurrent_requests: 100,
        }
    }
}

impl Config {
    /// Create config from environment variables
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("AIWEBENGINE_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: std::env::var("AIWEBENGINE_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(4000),
            script_timeout_ms: std::env::var("AIWEBENGINE_SCRIPT_TIMEOUT_MS")
                .ok()
                .and_then(|t| t.parse().ok())
                .unwrap_or(2000),
            max_concurrent_requests: std::env::var("AIWEBENGINE_MAX_CONCURRENT")
                .ok()
                .and_then(|c| c.parse().ok())
                .unwrap_or(100),
        }
    }

    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}
