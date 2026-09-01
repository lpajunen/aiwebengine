// Authentication Security Integration
// Connects authentication with existing security infrastructure

use base64::Engine;
use std::sync::Arc;

use crate::security::{
    CsrfProtection, DataEncryption, RateLimitKey, RateLimiter, SecurityAuditor, SecurityEvent,
    SecurityEventType, SecuritySeverity,
};

use super::error::AuthError;

/// Security context for authentication operations
/// Provides centralized access to all security components
#[derive(Clone)]
pub struct AuthSecurityContext {
    /// Security auditor for logging auth events
    pub auditor: Arc<SecurityAuditor>,

    /// Rate limiter for auth endpoints
    pub rate_limiter: Arc<RateLimiter>,

    /// CSRF protection
    pub csrf: Arc<CsrfProtection>,

    /// Data encryption for sensitive fields
    pub encryption: Arc<DataEncryption>,
}

impl AuthSecurityContext {
    /// Create a new authentication security context
    pub fn new(
        auditor: Arc<SecurityAuditor>,
        rate_limiter: Arc<RateLimiter>,
        csrf: Arc<CsrfProtection>,
        encryption: Arc<DataEncryption>,
    ) -> Self {
        Self {
            auditor,
            rate_limiter,
            csrf,
            encryption,
        }
    }

    /// Create OAuth state token
    /// Format: provider:ip:random
    pub async fn create_oauth_state(
        &self,
        provider: &str,
        ip_addr: &str,
    ) -> Result<String, AuthError> {
        // Generate a random state token
        let random_part: u64 = rand::random();
        let state = format!("{}:{}:{}", provider, ip_addr.replace('.', "_"), random_part);

        Ok(state)
    }

    /// Create OAuth state token with redirect URL encoded in it
    /// Format: provider:ip:random:base64(redirect_url)
    /// This makes it stateless and works across load-balanced servers
    pub async fn create_oauth_state_with_redirect(
        &self,
        provider: &str,
        ip_addr: &str,
        redirect_url: String,
    ) -> Result<String, AuthError> {
        // Generate base state
        let random_part: u64 = rand::random();

        // Encode redirect URL as base64 (URL-safe variant)
        let redirect_encoded =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(redirect_url.as_bytes());

        // Format: provider:ip:random:redirect_base64
        let state = format!(
            "{}:{}:{}:{}",
            provider,
            ip_addr.replace('.', "_"),
            random_part,
            redirect_encoded
        );

        Ok(state)
    }

    /// Extract redirect URL from OAuth state token
    /// Returns None if the state doesn't contain a redirect URL
    pub fn extract_redirect_url(state: &str) -> Option<String> {
        let parts: Vec<&str> = state.split(':').collect();

        // If we have 4 parts, the last one is the base64-encoded redirect URL
        if parts.len() == 4 {
            let redirect_encoded = parts[3];

            // Decode from base64
            if let Ok(decoded_bytes) =
                base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(redirect_encoded)
                && let Ok(redirect_url) = String::from_utf8(decoded_bytes)
            {
                return Some(redirect_url);
            }
        }

        None
    }

    /// Validate OAuth state token
    pub async fn validate_oauth_state(
        &self,
        state: &str,
        expected_provider: &str,
        expected_ip: &str,
    ) -> Result<bool, AuthError> {
        // Parse state token
        let parts: Vec<&str> = state.split(':').collect();

        // State can be either 3 parts (provider:ip:random) or 4 parts (provider:ip:random:redirect)
        if parts.len() < 3 || parts.len() > 4 {
            return Ok(false);
        }

        let provider = parts[0];
        let ip_addr = parts[1].replace('_', ".");

        // Validate provider and IP match
        Ok(provider == expected_provider && ip_addr == expected_ip)
    }

    /// Log authentication attempt
    pub async fn log_auth_attempt(&self, provider: &str, ip_addr: &str) {
        self.auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::AuthenticationAttempt,
                    SecuritySeverity::Low,
                    None,
                )
                .with_detail("provider", provider)
                .with_detail("ip_address", ip_addr),
            )
            .await;
    }

    /// Log authentication success
    pub async fn log_auth_success(&self, user_id: &str, provider: &str, ip_addr: Option<&str>) {
        let mut event = SecurityEvent::new(
            SecurityEventType::AuthenticationSuccess,
            SecuritySeverity::Low,
            Some(user_id.to_string()),
        )
        .with_detail("provider", provider);

        if let Some(ip) = ip_addr {
            event = event.with_detail("ip_address", ip);
        }

        self.auditor.log_event(event).await;
    }

    /// Log authentication failure
    pub async fn log_auth_failure(&self, provider: &str, reason: &str, ip_addr: Option<&str>) {
        let mut event = SecurityEvent::new(
            SecurityEventType::AuthenticationFailure,
            SecuritySeverity::Medium,
            None,
        )
        .with_detail("provider", provider)
        .with_error(reason.to_string());

        if let Some(ip) = ip_addr {
            event = event.with_detail("ip_address", ip);
        }

        self.auditor.log_event(event).await;
    }

    /// Log suspicious activity
    pub async fn log_suspicious_activity(&self, description: &str, user_id: Option<&str>) {
        self.auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SuspiciousActivity,
                    SecuritySeverity::High,
                    user_id.map(|s| s.to_string()),
                )
                .with_detail("description", description),
            )
            .await;
    }

    /// Check rate limit for authentication attempts
    pub async fn check_auth_rate_limit(&self, ip_addr: &str) -> bool {
        let result = self
            .rate_limiter
            .check_rate_limit(RateLimitKey::IpAddress(ip_addr.to_string()), 1)
            .await;

        result.allowed
    }

    /// Whether this account has any wrong guesses left.
    ///
    /// Beside the per-IP limit rather than instead of it: an attacker who
    /// spreads a guess across a thousand addresses meets no per-IP wall at all,
    /// and one known username is all such an attack needs.
    pub async fn account_login_allowed(&self, account: &str) -> bool {
        self.rate_limiter
            .remaining_tokens(&RateLimitKey::LoginFailure(
                crate::auth::local::normalize_username(account),
            ))
            .await
            >= 1.0
    }

    /// Record a wrong guess against an account.
    ///
    /// Only failures are counted, so signing in correctly — however often —
    /// never uses the budget up, and an attacker cannot lock someone out of
    /// their own account by spending it for them.
    pub async fn record_account_login_failure(&self, account: &str) {
        self.rate_limiter
            .check_rate_limit(
                RateLimitKey::LoginFailure(crate::auth::local::normalize_username(account)),
                1,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_security_context() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
        let rate_limiter =
            Arc::new(RateLimiter::new(pool.clone()).with_security_auditor(Arc::clone(&auditor)));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let context = AuthSecurityContext::new(auditor, rate_limiter, csrf, encryption);
        assert!(Arc::strong_count(&context.auditor) >= 1);
    }

    #[tokio::test]
    async fn test_csrf_protection() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
        let rate_limiter =
            Arc::new(RateLimiter::new(pool.clone()).with_security_auditor(Arc::clone(&auditor)));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let context = AuthSecurityContext::new(auditor, rate_limiter, csrf, encryption);

        let state = context
            .create_oauth_state("google", "192.168.1.1")
            .await
            .unwrap();
        assert!(state.contains("google"));
        assert!(state.contains("192_168_1_1"));
    }

    #[tokio::test]
    async fn test_oauth_state_validation() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
        let rate_limiter =
            Arc::new(RateLimiter::new(pool.clone()).with_security_auditor(Arc::clone(&auditor)));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let context = AuthSecurityContext::new(auditor, rate_limiter, csrf, encryption);

        let state = context
            .create_oauth_state("google", "192.168.1.1")
            .await
            .unwrap();
        let valid = context
            .validate_oauth_state(&state, "google", "192.168.1.1")
            .await
            .unwrap();
        assert!(valid);

        // Wrong provider
        let invalid = context
            .validate_oauth_state(&state, "microsoft", "192.168.1.1")
            .await
            .unwrap();
        assert!(!invalid);

        // Wrong IP
        let invalid = context
            .validate_oauth_state(&state, "google", "192.168.1.2")
            .await
            .unwrap();
        assert!(!invalid);
    }

    /// The half the per-IP limit cannot cover: one account, guessed at from
    /// wherever. Ten wrong answers and the eleventh is refused however fresh
    /// the address it arrives from.
    #[tokio::test]
    async fn an_account_runs_out_of_wrong_guesses() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
        let rate_limiter =
            Arc::new(RateLimiter::new(pool.clone()).with_security_auditor(Arc::clone(&auditor)));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let context = AuthSecurityContext::new(auditor, rate_limiter, csrf, encryption);

        // A bucket of its own: the table is shared by every test process.
        let account = format!("guessed-{}", uuid::Uuid::new_v4());

        assert!(
            context.account_login_allowed(&account).await,
            "an account nobody has guessed at yet has its whole budget"
        );

        for _ in 0..10 {
            context.record_account_login_failure(&account).await;
        }

        assert!(
            !context.account_login_allowed(&account).await,
            "ten wrong guesses is the budget"
        );

        // The spelling is not a way around it.
        assert!(!context.account_login_allowed(&account.to_uppercase()).await);

        let _ = sqlx::query("DELETE FROM rate_limits WHERE key = $1")
            .bind(format!("login_failure:{}", account))
            .execute(&pool)
            .await;
    }

    /// Signing in correctly, however often, never spends the budget — an
    /// attacker must not be able to lock someone out of their own account.
    #[tokio::test]
    async fn a_successful_sign_in_costs_an_account_nothing() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
        let rate_limiter =
            Arc::new(RateLimiter::new(pool.clone()).with_security_auditor(Arc::clone(&auditor)));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let context = AuthSecurityContext::new(auditor, rate_limiter, csrf, encryption);
        let account = format!("steady-{}", uuid::Uuid::new_v4());

        for _ in 0..50 {
            assert!(context.account_login_allowed(&account).await);
        }
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
        let rate_limiter =
            Arc::new(RateLimiter::new(pool.clone()).with_security_auditor(Arc::clone(&auditor)));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let context = AuthSecurityContext::new(auditor, rate_limiter, csrf, encryption);

        // The bucket behind this key lives in the `rate_limits` table, which every
        // test process shares. On a fixed IP the 10 tokens spent here accumulate
        // across runs and retries until the 60-token bucket is empty and the test
        // starts failing; a key of its own keeps each run independent.
        let ip = format!("test-{}", uuid::Uuid::new_v4());

        // First requests should succeed
        for _ in 0..10 {
            assert!(context.check_auth_rate_limit(&ip).await);
        }

        let _ = sqlx::query("DELETE FROM rate_limits WHERE key = $1")
            .bind(format!("ip:{}", ip))
            .execute(&pool)
            .await;
    }
}
