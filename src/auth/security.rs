// Authentication Security Integration
// Connects authentication with existing security infrastructure

use std::sync::Arc;

use crate::security::{
    CsrfProtection, DataEncryption, RateLimitKey, RateLimiter, SecurityAuditor, SecurityEvent,
    SecurityEventType, SecuritySeverity,
};

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

    /// Whether this address may register another OAuth2 client.
    ///
    /// Registration is open — an MCP client has no credential to present
    /// before it has one — so this is the only thing bounding how many rows a
    /// caller can write into `oauth_clients`. Spends a token per call, unlike
    /// the login-failure budget: here every attempt is the thing being bounded,
    /// not just the failures.
    pub async fn client_registration_allowed(&self, ip_addr: &str) -> bool {
        self.rate_limiter
            .check_rate_limit(RateLimitKey::ClientRegistration(ip_addr.to_string()), 1)
            .await
            .allowed
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
        let pool = crate::test_db::pool();
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

    /// The half the per-IP limit cannot cover: one account, guessed at from
    /// wherever. Ten wrong answers and the eleventh is refused however fresh
    /// the address it arrives from.
    #[tokio::test]
    async fn an_account_runs_out_of_wrong_guesses() {
        let pool = crate::test_db::pool();
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
        let pool = crate::test_db::pool();
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
        let pool = crate::test_db::pool();
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
