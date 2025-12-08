// Rate Limiting Module
// Provides token bucket rate limiting with per-IP and per-user limits

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::security::{
    SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity, ThreatDetector,
};

/// Rate limit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum tokens in the bucket
    pub max_tokens: u32,
    /// Token refill rate (tokens per second)
    pub refill_rate: f64,
    /// Window duration for rate limiting
    pub window_duration: Duration,
    /// Burst allowance (extra tokens beyond normal rate)
    pub burst_allowance: u32,
    /// Enable/disable rate limiting
    pub enabled: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_tokens: 100,
            refill_rate: 10.0, // 10 requests per second
            window_duration: Duration::minutes(1),
            burst_allowance: 20,
            enabled: true,
        }
    }
}

/// Token bucket for rate limiting
#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// Current number of tokens
    tokens: f64,
    /// Maximum number of tokens
    max_tokens: f64,
    /// Token refill rate (tokens per second)
    refill_rate: f64,
    /// Last refill timestamp
    last_refill: DateTime<Utc>,
    /// Total requests made
    total_requests: u64,
    /// Rejected requests due to rate limiting
    rejected_requests: u64,
}

impl TokenBucket {
    pub fn new(max_tokens: u32, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens as f64,
            max_tokens: max_tokens as f64,
            refill_rate,
            last_refill: Utc::now(),
            total_requests: 0,
            rejected_requests: 0,
        }
    }

    /// Attempt to consume tokens from the bucket
    pub fn consume(&mut self, tokens: u32) -> bool {
        self.refill();
        self.total_requests += 1;

        if self.tokens >= tokens as f64 {
            self.tokens -= tokens as f64;
            true
        } else {
            self.rejected_requests += 1;
            false
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = Utc::now();
        let elapsed = now.signed_duration_since(self.last_refill);
        let elapsed_seconds = elapsed.num_milliseconds() as f64 / 1000.0;

        if elapsed_seconds > 0.0 {
            let tokens_to_add = elapsed_seconds * self.refill_rate;
            self.tokens = (self.tokens + tokens_to_add).min(self.max_tokens);
            self.last_refill = now;
        }
    }

    /// Get current token count
    pub fn available_tokens(&mut self) -> f64 {
        self.refill();
        self.tokens
    }

    /// Get usage statistics
    pub fn stats(&self) -> (u64, u64, f64) {
        let success_rate = if self.total_requests > 0 {
            ((self.total_requests - self.rejected_requests) as f64 / self.total_requests as f64)
                * 100.0
        } else {
            100.0
        };
        (self.total_requests, self.rejected_requests, success_rate)
    }
}

/// Rate limit key type
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RateLimitKey {
    /// Rate limit by IP address
    IpAddress(String),
    /// Rate limit by user ID
    UserId(String),
    /// Rate limit by endpoint/resource
    Endpoint(String),
    /// Rate limit by user and endpoint combination
    UserEndpoint(String, String),
    /// Rate limit by IP and endpoint combination
    IpEndpoint(String, String),
    /// Global rate limit
    Global,
}

impl RateLimitKey {
    pub fn as_string(&self) -> String {
        match self {
            RateLimitKey::IpAddress(ip) => format!("ip:{}", ip),
            RateLimitKey::UserId(user_id) => format!("user:{}", user_id),
            RateLimitKey::Endpoint(endpoint) => format!("endpoint:{}", endpoint),
            RateLimitKey::UserEndpoint(user_id, endpoint) => {
                format!("user_endpoint:{}:{}", user_id, endpoint)
            }
            RateLimitKey::IpEndpoint(ip, endpoint) => format!("ip_endpoint:{}:{}", ip, endpoint),
            RateLimitKey::Global => "global".to_string(),
        }
    }
}

/// Rate limit result
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Whether the request was allowed
    pub allowed: bool,
    /// Number of tokens consumed
    pub tokens_consumed: u32,
    /// Remaining tokens in bucket
    pub remaining_tokens: f64,
    /// Time until next token refill (seconds)
    pub retry_after: Option<f64>,
    /// Rate limit key used
    pub key: RateLimitKey,
    /// Configuration used
    pub config: RateLimitConfig,
}

use sqlx::{PgPool, Row};

/// Rate limiter with multiple bucket types
pub struct RateLimiter {
    /// Database pool
    pool: PgPool,
    /// Configuration by key type
    configs: HashMap<String, RateLimitConfig>,
    /// Default configuration
    default_config: RateLimitConfig,
    /// Threat detector for integration
    threat_detector: Option<Arc<ThreatDetector>>,
    /// Security auditor for logging
    security_auditor: Option<Arc<SecurityAuditor>>,
}

impl RateLimiter {
    pub fn new(pool: PgPool) -> Self {
        let mut configs = HashMap::new();

        // Per-IP rate limiting (stricter)
        configs.insert(
            "ip".to_string(),
            RateLimitConfig {
                max_tokens: 60,
                refill_rate: 1.0, // 1 request per second
                window_duration: Duration::minutes(1),
                burst_allowance: 10,
                enabled: true,
            },
        );

        // Per-user rate limiting (more permissive)
        configs.insert(
            "user".to_string(),
            RateLimitConfig {
                max_tokens: 200,
                refill_rate: 5.0, // 5 requests per second
                window_duration: Duration::minutes(1),
                burst_allowance: 50,
                enabled: true,
            },
        );

        // Per-endpoint rate limiting
        configs.insert(
            "endpoint".to_string(),
            RateLimitConfig {
                max_tokens: 1000,
                refill_rate: 20.0, // 20 requests per second
                window_duration: Duration::minutes(1),
                burst_allowance: 100,
                enabled: true,
            },
        );

        Self {
            pool,
            configs,
            default_config: RateLimitConfig::default(),
            threat_detector: None,
            security_auditor: None,
        }
    }

    /// Set threat detector for integration
    pub fn with_threat_detector(mut self, threat_detector: Arc<ThreatDetector>) -> Self {
        self.threat_detector = Some(threat_detector);
        self
    }

    /// Set security auditor for logging
    pub fn with_security_auditor(mut self, security_auditor: Arc<SecurityAuditor>) -> Self {
        self.security_auditor = Some(security_auditor);
        self
    }

    /// Check rate limit for a key
    pub async fn check_rate_limit(&self, key: RateLimitKey, tokens: u32) -> RateLimitResult {
        let config = self.get_config(&key);
        let key_str = key.as_string();

        if !config.enabled {
            return RateLimitResult {
                allowed: true,
                tokens_consumed: 0,
                remaining_tokens: config.max_tokens as f64,
                retry_after: None,
                key,
                config,
            };
        }

        // Database operations
        let (allowed, remaining_tokens, retry_after) =
            match self.process_rate_limit(&key_str, tokens, &config).await {
                Ok(result) => result,
                Err(e) => {
                    warn!("Rate limit DB error: {}", e);
                    // Fail open
                    (true, config.max_tokens as f64, None)
                }
            };

        let result = RateLimitResult {
            allowed,
            tokens_consumed: if allowed { tokens } else { 0 },
            remaining_tokens,
            retry_after,
            key: key.clone(),
            config: config.clone(),
        };

        // Log rate limit events
        if !allowed {
            warn!("Rate limit exceeded for key: {}", key.as_string());
            self.log_rate_limit_violation(&key, &result).await;
        } else {
            debug!("Rate limit check passed for key: {}", key.as_string());
        }

        result
    }

    async fn process_rate_limit(
        &self,
        key: &str,
        tokens: u32,
        config: &RateLimitConfig,
    ) -> Result<(bool, f64, Option<f64>), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let row = sqlx::query(
            "SELECT tokens, last_refill, total_requests, rejected_requests FROM rate_limits WHERE key = $1 FOR UPDATE"
        )
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;

        let (mut current_tokens, last_refill, mut total_requests, mut rejected_requests) =
            if let Some(row) = row {
                let tokens: f64 = row.get("tokens");
                let last_refill: DateTime<Utc> = row.get("last_refill");
                let total: i64 = row.get("total_requests");
                let rejected: i64 = row.get("rejected_requests");
                (tokens, last_refill, total, rejected)
            } else {
                (config.max_tokens as f64, Utc::now(), 0i64, 0i64)
            };

        // Refill logic
        let now = Utc::now();
        let elapsed = now.signed_duration_since(last_refill);
        let elapsed_seconds = elapsed.num_milliseconds() as f64 / 1000.0;

        if elapsed_seconds > 0.0 {
            let tokens_to_add = elapsed_seconds * config.refill_rate;
            current_tokens = (current_tokens + tokens_to_add).min(config.max_tokens as f64);
        }

        // Consume logic
        let allowed = if current_tokens >= tokens as f64 {
            current_tokens -= tokens as f64;
            true
        } else {
            rejected_requests += 1;
            false
        };

        total_requests += 1;

        // Update DB
        sqlx::query(
            "INSERT INTO rate_limits (key, tokens, last_refill, total_requests, rejected_requests)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (key) DO UPDATE SET
             tokens = $2, last_refill = $3, total_requests = $4, rejected_requests = $5",
        )
        .bind(key)
        .bind(current_tokens)
        .bind(now)
        .bind(total_requests)
        .bind(rejected_requests)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let retry_after = if !allowed {
            Some(tokens as f64 / config.refill_rate)
        } else {
            None
        };

        Ok((allowed, current_tokens, retry_after))
    }

    /// Get configuration for a key
    fn get_config(&self, key: &RateLimitKey) -> RateLimitConfig {
        let config_key = match key {
            RateLimitKey::IpAddress(_) => "ip",
            RateLimitKey::UserId(_) => "user",
            RateLimitKey::Endpoint(_) => "endpoint",
            RateLimitKey::UserEndpoint(_, _) => "user",
            RateLimitKey::IpEndpoint(_, _) => "ip",
            RateLimitKey::Global => "global",
        };

        self.configs
            .get(config_key)
            .cloned()
            .unwrap_or_else(|| self.default_config.clone())
    }

    /// Log rate limit violation
    async fn log_rate_limit_violation(&self, key: &RateLimitKey, result: &RateLimitResult) {
        if let Some(auditor) = &self.security_auditor {
            let mut event = SecurityEvent::new(
                SecurityEventType::RateLimitExceeded,
                SecuritySeverity::Medium,
                match key {
                    RateLimitKey::UserId(user_id) => Some(user_id.clone()),
                    RateLimitKey::UserEndpoint(user_id, _) => Some(user_id.clone()),
                    _ => None,
                },
            );

            // Set additional fields
            event.ip_address = match key {
                RateLimitKey::IpAddress(ip) => Some(ip.clone()),
                RateLimitKey::IpEndpoint(ip, _) => Some(ip.clone()),
                _ => None,
            };

            event.resource = match key {
                RateLimitKey::Endpoint(endpoint) => Some(endpoint.clone()),
                RateLimitKey::UserEndpoint(_, endpoint) => Some(endpoint.clone()),
                RateLimitKey::IpEndpoint(_, endpoint) => Some(endpoint.clone()),
                _ => None,
            };

            event.action = Some("rate_limit_check".to_string());

            event.error_message = Some(format!("Rate limit exceeded for {}", key.as_string()));

            // Add details
            event.details.insert(
                "tokens_requested".to_string(),
                result.tokens_consumed.to_string(),
            );
            event.details.insert(
                "remaining_tokens".to_string(),
                result.remaining_tokens.to_string(),
            );
            if let Some(retry_after) = result.retry_after {
                event
                    .details
                    .insert("retry_after".to_string(), retry_after.to_string());
            }
            event.details.insert(
                "max_tokens".to_string(),
                result.config.max_tokens.to_string(),
            );
            event.details.insert(
                "refill_rate".to_string(),
                result.config.refill_rate.to_string(),
            );

            auditor.log_event(event).await;
        }

        // Notify threat detector of potential abuse
        if let Some(_threat_detector) = &self.threat_detector {
            match key {
                RateLimitKey::IpAddress(ip) => {
                    // Could implement additional threat analysis here
                    debug!(
                        "Notifying threat detector of rate limit violation from IP: {}",
                        ip
                    );
                }
                RateLimitKey::UserId(user_id) => {
                    debug!(
                        "Notifying threat detector of rate limit violation from user: {}",
                        user_id
                    );
                }
                _ => {}
            }
        }
    }

    /// Update configuration for a key type
    pub fn update_config(&mut self, key_type: &str, config: RateLimitConfig) {
        self.configs.insert(key_type.to_string(), config);
        info!(
            "Updated rate limit configuration for key type: {}",
            key_type
        );
    }

    /// Get statistics for all buckets
    pub async fn get_statistics(&self) -> HashMap<String, (u64, u64, f64)> {
        let rows = sqlx::query("SELECT key, total_requests, rejected_requests FROM rate_limits")
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

        let mut stats = HashMap::new();

        for row in rows {
            let key: String = row.get("key");
            let total: i64 = row.get("total_requests");
            let rejected: i64 = row.get("rejected_requests");

            let success_rate = if total > 0 {
                ((total - rejected) as f64 / total as f64) * 100.0
            } else {
                100.0
            };

            stats.insert(key, (total as u64, rejected as u64, success_rate));
        }

        stats
    }
    /// Clean up old buckets
    pub async fn cleanup_old_buckets(&self, max_age: Duration) {
        let cutoff = Utc::now() - max_age;

        let result = sqlx::query("DELETE FROM rate_limits WHERE last_refill < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await;

        match result {
            Ok(res) => {
                let count = res.rows_affected();
                if count > 0 {
                    info!("Cleaned up {} old rate limit buckets", count);
                }
            }
            Err(e) => {
                warn!("Failed to cleanup old rate limit buckets: {}", e);
            }
        }
    }

    /// Check multiple rate limits (e.g., both IP and user)
    pub async fn check_multiple_rate_limits(
        &self,
        keys: Vec<RateLimitKey>,
        tokens: u32,
    ) -> Vec<RateLimitResult> {
        let mut results = Vec::new();

        for key in keys {
            let result = self.check_rate_limit(key, tokens).await;
            results.push(result);
        }

        results
    }

    /// Check if any rate limit is exceeded
    pub async fn is_rate_limited(
        &self,
        keys: Vec<RateLimitKey>,
        tokens: u32,
    ) -> (bool, Vec<RateLimitResult>) {
        let results = self.check_multiple_rate_limits(keys, tokens).await;
        let rate_limited = results.iter().any(|r| !r.allowed);
        (rate_limited, results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;
    use tokio::time::{Duration as TokioDuration, sleep};

    static RATE_LIMIT_TEST_LOCK: Mutex<()> = Mutex::const_new(());

    #[test]
    fn test_token_bucket_creation() {
        let mut bucket = TokenBucket::new(10, 1.0);
        assert_eq!(bucket.available_tokens(), 10.0);
    }

    #[test]
    fn test_token_consumption() {
        let mut bucket = TokenBucket::new(10, 1.0);

        assert!(bucket.consume(5));
        assert_eq!(bucket.available_tokens(), 5.0);

        assert!(bucket.consume(5));
        assert_eq!(bucket.available_tokens(), 0.0);

        assert!(!bucket.consume(1));
    }

    #[tokio::test]
    async fn test_token_refill() {
        let mut bucket = TokenBucket::new(10, 10.0); // 10 tokens per second

        // Consume all tokens
        assert!(bucket.consume(10));
        assert_eq!(bucket.available_tokens(), 0.0);

        // Wait for refill
        sleep(TokioDuration::from_millis(100)).await;

        // Should have approximately 1 token after 0.1 seconds
        let available = bucket.available_tokens();
        assert!((0.5..=1.5).contains(&available));
    }

    #[test]
    fn test_rate_limit_key() {
        let ip_key = RateLimitKey::IpAddress("192.168.1.1".to_string());
        assert_eq!(ip_key.as_string(), "ip:192.168.1.1");

        let user_key = RateLimitKey::UserId("user123".to_string());
        assert_eq!(user_key.as_string(), "user:user123");

        let endpoint_key = RateLimitKey::Endpoint("/api/test".to_string());
        assert_eq!(endpoint_key.as_string(), "endpoint:/api/test");
    }

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let _lock = RATE_LIMIT_TEST_LOCK.lock().await;
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let limiter = RateLimiter::new(pool);
        let key = RateLimitKey::IpAddress("192.168.1.1".to_string());

        let result = limiter.check_rate_limit(key.clone(), 1).await;
        assert!(result.allowed);
        assert_eq!(result.tokens_consumed, 1);
    }

    #[tokio::test]
    async fn test_rate_limiter_exceed_limit() {
        let _lock = RATE_LIMIT_TEST_LOCK.lock().await;
        let pool = sqlx::PgPool::connect(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .await
        .unwrap();
        sqlx::query("DELETE FROM rate_limits WHERE key = 'ip:192.168.1.2'")
            .execute(&pool)
            .await
            .unwrap();

        let limiter = RateLimiter::new(pool);
        let key = RateLimitKey::IpAddress("192.168.1.2".to_string());

        // Consume all tokens at once (IP config has 60 max tokens)
        let result = limiter.check_rate_limit(key.clone(), 60).await;
        assert!(result.allowed);

        // Next request should be denied
        let result = limiter.check_rate_limit(key.clone(), 1).await;
        assert!(!result.allowed);
        assert!(result.retry_after.is_some());
    }

    #[tokio::test]
    async fn test_multiple_rate_limits() {
        let _lock = RATE_LIMIT_TEST_LOCK.lock().await;
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let limiter = RateLimiter::new(pool);
        let keys = vec![
            RateLimitKey::IpAddress("192.168.1.1".to_string()),
            RateLimitKey::UserId("user123".to_string()),
        ];

        let results = limiter.check_multiple_rate_limits(keys, 1).await;
        assert_eq!(results.len(), 2);
        assert!(results[0].allowed);
        assert!(results[1].allowed);
    }

    #[tokio::test]
    async fn test_rate_limiter_statistics() {
        let _lock = RATE_LIMIT_TEST_LOCK.lock().await;
        let pool = sqlx::PgPool::connect(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .await
        .unwrap();
        sqlx::query("DELETE FROM rate_limits")
            .execute(&pool)
            .await
            .unwrap();

        let limiter = RateLimiter::new(pool);
        let key = RateLimitKey::IpAddress("192.168.1.1".to_string());

        // Make some requests
        for _ in 0..5 {
            limiter.check_rate_limit(key.clone(), 1).await;
        }

        let stats = limiter.get_statistics().await;
        assert!(stats.contains_key("ip:192.168.1.1"));

        let (total, rejected, success_rate) = stats["ip:192.168.1.1"];
        assert_eq!(total, 5);
        assert_eq!(rejected, 0);
        assert_eq!(success_rate, 100.0);
    }

    #[tokio::test]
    async fn test_disabled_rate_limiting() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let mut limiter = RateLimiter::new(pool);

        // Disable IP rate limiting
        let config = RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        };
        limiter.update_config("ip", config);

        let key = RateLimitKey::IpAddress("192.168.1.1".to_string());

        // Should allow unlimited requests
        for _ in 0..1000 {
            let result = limiter.check_rate_limit(key.clone(), 1).await;
            assert!(result.allowed);
        }
    }
}
