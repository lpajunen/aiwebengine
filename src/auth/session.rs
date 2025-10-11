// Authentication Session Management
// High-level wrapper around SecureSessionManager with auth-specific logic

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::security::{SecureSessionManager, SessionData, SessionToken};

use super::{error::AuthError, AuthSecurityContext};

/// Authentication session (user-facing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl From<SessionData> for AuthSession {
    fn from(data: SessionData) -> Self {
        Self {
            user_id: data.user_id,
            provider: data.provider,
            email: data.email,
            name: data.name,
            picture: None, // Can be added later
            is_admin: data.is_admin,
            created_at: data.created_at,
            expires_at: data.expires_at,
        }
    }
}

/// High-level authentication session manager
pub struct AuthSessionManager {
    security_context: Arc<AuthSecurityContext>,
}

impl AuthSessionManager {
    pub fn new(security_context: Arc<AuthSecurityContext>) -> Self {
        Self { security_context }
    }

    /// Create a new authenticated session
    pub async fn create_session(
        &self,
        user_id: String,
        provider: String,
        email: Option<String>,
        name: Option<String>,
        is_admin: bool,
        ip_addr: String,
        user_agent: String,
    ) -> Result<SessionToken, AuthError> {
        // Create session using secure session manager
        let token = self
            .security_context
            .session_manager
            .create_session(
                user_id.clone(),
                provider.clone(),
                email.clone(),
                name,
                is_admin,
                ip_addr.clone(),
                user_agent,
            )
            .await
            .map_err(AuthError::Session)?;

        // Log success
        self.security_context
            .log_auth_success(&user_id, &provider, Some(&ip_addr)).await;

        Ok(token)
    }

    /// Validate and retrieve session
    pub async fn get_session(
        &self,
        token: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<AuthSession, AuthError> {
        let session_data = self
            .security_context
            .session_manager
            .validate_session(token, ip_addr, user_agent)
            .await
            .map_err(AuthError::Session)?;

        Ok(AuthSession::from(session_data))
    }

    /// Invalidate session (logout)
    pub async fn invalidate_session(&self, token: &str) -> Result<(), AuthError> {
        self.security_context
            .session_manager
            .invalidate_session(token)
            .await
            .map_err(AuthError::Session)?;

        Ok(())
    }

    /// Get session count for a user
    pub async fn get_user_session_count(&self, user_id: &str) -> usize {
        self.security_context
            .session_manager
            .get_user_session_count(user_id)
            .await
    }

    /// Check if user has reached concurrent session limit
    pub async fn can_create_session(&self, user_id: &str, max_sessions: usize) -> bool {
        self.get_user_session_count(user_id).await < max_sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthConfig;
    use crate::security::SecurityAuditor;

    fn create_test_manager() -> AuthSessionManager {
        let config = AuthConfig {
            jwt_secret: "a".repeat(32),
            session_timeout: 3600,
            max_concurrent_sessions: 3,
            ..Default::default()
        };

        let auditor = Arc::new(SecurityAuditor::new());
        let security_context = Arc::new(AuthSecurityContext::new(&config, auditor).unwrap());

        AuthSessionManager::new(security_context)
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let manager = create_test_manager();

        let token = manager
            .create_session(
                "user123".to_string(),
                "google".to_string(),
                Some("user@example.com".to_string()),
                Some("Test User".to_string()),
                false,
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        let session = manager
            .get_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await
            .unwrap();

        assert_eq!(session.user_id, "user123");
        assert_eq!(session.provider, "google");
        assert_eq!(session.email, Some("user@example.com".to_string()));
        assert!(!session.is_admin);
    }

    #[tokio::test]
    async fn test_session_invalidation() {
        let manager = create_test_manager();

        let token = manager
            .create_session(
                "user123".to_string(),
                "google".to_string(),
                None,
                None,
                false,
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        // Invalidate
        manager.invalidate_session(&token.token).await.unwrap();

        // Should fail to retrieve
        let result = manager
            .get_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_count() {
        let manager = create_test_manager();

        let user_id = "user123";

        // Create 2 sessions
        for i in 0..2 {
            manager
                .create_session(
                    user_id.to_string(),
                    "google".to_string(),
                    None,
                    None,
                    false,
                    format!("192.168.1.{}", i),
                    "Mozilla/5.0".to_string(),
                )
                .await
                .unwrap();
        }

        let count = manager.get_user_session_count(user_id).await;
        assert_eq!(count, 2);

        // Can create more (limit is 3)
        assert!(manager.can_create_session(user_id, 3).await);
    }

    #[tokio::test]
    async fn test_admin_session() {
        let manager = create_test_manager();

        let token = manager
            .create_session(
                "admin123".to_string(),
                "google".to_string(),
                Some("admin@example.com".to_string()),
                Some("Admin User".to_string()),
                true, // is_admin
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        let session = manager
            .get_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await
            .unwrap();

        assert!(session.is_admin);
    }
}
