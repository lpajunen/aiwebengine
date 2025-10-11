// Authentication Security Integration
// Connects authentication with existing security infrastructure

use std::sync::Arc;

use crate::security::{
    CsrfProtection, DataEncryption, FieldEncryptor, OAuthStateManager, RateLimitKey,
    RateLimiter, SecureSessionManager, SecurityAuditor, SecurityEvent, SecurityEventType,
    SecuritySeverity, UserContext,
};

use super::{error::AuthError, AuthConfig};

/// Security context for authentication operations
/// Provides centralized access to all security components
#[derive(Clone)]
pub struct AuthSecurityContext {
    /// Security auditor for logging auth events
    pub auditor: Arc<SecurityAuditor>,

    /// Rate limiter for auth endpoints
    pub rate_limiter: Arc<RateLimiter>,

    /// Session manager with encryption
    pub session_manager: Arc<SecureSessionManager>,

    /// CSRF protection
    pub csrf: Arc<CsrfProtection>,

    /// OAuth state manager
    pub oauth_state: Arc<OAuthStateManager>,

    /// Data encryption for sensitive fields
    pub encryption: Arc<DataEncryption>,

    /// Field encryptor for OAuth tokens
    pub field_encryptor: Arc<FieldEncryptor>,
}

impl AuthSecurityContext {
    /// Create a new authentication security context
    pub fn new(config: &AuthConfig, auditor: Arc<SecurityAuditor>) -> Result<Self, AuthError> {
        // Derive encryption key from JWT secret
        let encryption_key = config.jwt_secret_bytes();

        // Create session manager
        let session_manager = Arc::new(
            SecureSessionManager::new(
                &encryption_key,
                config.session_timeout as i64,
                config.max_concurrent_sessions,
                auditor.clone(),
            )
            .map_err(|e| {
                AuthError::Internal(format!("Failed to create session manager: {}", e))
            })?,
        );

        // Create CSRF protection (24 hour lifetime for CSRF tokens)
        let csrf = Arc::new(CsrfProtection::new(encryption_key, 86400));

        // Create OAuth state manager (10 minute lifetime)
        let oauth_state = Arc::new(OAuthStateManager::new(encryption_key));

        // Create data encryption
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        // Create field encryptor
        let field_encryptor = Arc::new(FieldEncryptor::new(encryption.clone()));

        // Create rate limiter with auth-specific config
        let rate_limiter = Arc::new(
            RateLimiter::new()
                .with_security_auditor(auditor.clone())
        );

        Ok(Self {
            auditor,
            rate_limiter,
            session_manager,
            csrf,
            oauth_state,
            encryption,
            field_encryptor,
        })
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
    pub async fn log_auth_failure(&self, reason: &str, ip_addr: Option<&str>) {
        let mut event = SecurityEvent::new(
            SecurityEventType::AuthenticationFailure,
            SecuritySeverity::Medium,
            None,
        )
        .with_error(reason.to_string());

        if let Some(ip) = ip_addr {
            event = event.with_detail("ip_address", ip);
        }

        self.auditor.log_event(event).await;
    }

    /// Log suspicious activity
    pub async fn log_suspicious_activity(&self, description: &str, user_id: Option<&str>) {
        self.auditor.log_event(
            SecurityEvent::new(
                SecurityEventType::SuspiciousActivity,
                SecuritySeverity::High,
                user_id.map(|s| s.to_string()),
            )
            .with_detail("description", description),
        ).await;
    }

    /// Check rate limit for authentication attempts
    pub async fn check_auth_rate_limit(&self, ip_addr: &str) -> Result<(), AuthError> {
        let result = self
            .rate_limiter
            .check_rate_limit(RateLimitKey::IpAddress(ip_addr.to_string()), 1)
            .await;

        if !result.allowed {
            self.log_auth_failure(&format!("Rate limit exceeded for IP: {}", ip_addr), Some(ip_addr)).await;
            return Err(AuthError::RateLimitExceeded);
        }

        Ok(())
    }

    /// Create user context from session data
    pub fn create_user_context(
        &self,
        session_data: &crate::security::SessionData,
    ) -> UserContext {
        if session_data.is_admin {
            UserContext::admin(session_data.user_id.clone())
        } else {
            UserContext::authenticated(session_data.user_id.clone())
        }
    }

    /// Start background cleanup tasks
    pub fn start_cleanup_tasks(&self) {
        let session_manager = self.session_manager.clone();
        let csrf = self.csrf.clone();
        let oauth_state = self.oauth_state.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // 5 minutes

            loop {
                interval.tick().await;

                // Cleanup expired sessions
                let expired_sessions = session_manager.cleanup_expired_sessions().await;
                if expired_sessions > 0 {
                    tracing::info!("Cleaned up {} expired sessions", expired_sessions);
                }

                // Cleanup expired CSRF tokens
                let expired_csrf = csrf.cleanup_expired_tokens().await;
                if expired_csrf > 0 {
                    tracing::debug!("Cleaned up {} expired CSRF tokens", expired_csrf);
                }

                // Cleanup expired OAuth states
                let expired_states = oauth_state.cleanup_expired().await;
                if expired_states > 0 {
                    tracing::debug!("Cleaned up {} expired OAuth states", expired_states);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> AuthConfig {
        AuthConfig {
            jwt_secret: "a".repeat(32),
            session_timeout: 3600,
            max_concurrent_sessions: 3,
            ..Default::default()
        }
    }

    #[test]
    fn test_create_security_context() {
        let config = create_test_config();
        let auditor = Arc::new(SecurityAuditor::new());

        let context = AuthSecurityContext::new(&config, auditor);
        assert!(context.is_ok());
    }

    #[test]
    fn test_short_jwt_secret_fails() {
        let config = AuthConfig {
            jwt_secret: "short".to_string(),
            session_timeout: 3600,
            max_concurrent_sessions: 3,
            ..Default::default()
        };

        let auditor = Arc::new(SecurityAuditor::new());
        let context = AuthSecurityContext::new(&config, auditor);

        // Should still work (warning: padding applied internally)
        // but validation should catch this
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let config = create_test_config();
        let auditor = Arc::new(SecurityAuditor::new());
        let context = AuthSecurityContext::new(&config, auditor).unwrap();

        let ip = "192.168.1.1";

        // First requests should succeed
        for _ in 0..10 {
            assert!(context.check_auth_rate_limit(ip).await.is_ok());
        }

        // Eventually should hit rate limit
        let mut rate_limited = false;
        for _ in 0..100 {
            if context.check_auth_rate_limit(ip).await.is_err() {
                rate_limited = true;
                break;
            }
        }

        assert!(rate_limited, "Rate limiting should eventually trigger");
    }

    #[test]
    fn test_user_context_creation() {
        let config = create_test_config();
        let auditor = Arc::new(SecurityAuditor::new());
        let context = AuthSecurityContext::new(&config, auditor).unwrap();

        let session_data = crate::security::SessionData {
            session_id: "test_session".to_string(),
            user_id: "user123".to_string(),
            provider: "google".to_string(),
            email: Some("user@example.com".to_string()),
            name: Some("Test User".to_string()),
            is_admin: false,
            created_at: chrono::Utc::now(),
            last_access: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            fingerprint: crate::security::SessionFingerprint::new(
                "192.168.1.1".to_string(),
                "Mozilla/5.0",
                false,
            ),
        };

        let user_context = context.create_user_context(&session_data);
        assert_eq!(user_context.user_id, Some("user123".to_string()));
        assert!(user_context.is_authenticated);
    }
}
